// ABOUTME: Effects only — the Clock impl, heap probes, NVS reboot accounting and
// ABOUTME: esp_restart (§6.1: wedge.rs decides, this file reboots).

use esp_idf_svc::nvs::{EspNvs, EspNvsPartition, NvsDefault};
use esp_idf_svc::sys;
use harvester_core::render::RebootCounts;
use harvester_core::{Clock, Millis};
use std::sync::atomic::{AtomicBool, AtomicI8, Ordering};

/// Set once SNTP reports sync; read by EspClock. Lock-free on purpose — it is
/// touched from the SNTP callback and read on every render.
pub static TIME_SYNCED: AtomicBool = AtomicBool::new(false);

/// Latest Wi-Fi RSSI, published by the housekeeping sweep and read by HTTP
/// handlers. 0 dBm is physically impossible and doubles as "unassociated".
pub static WIFI_RSSI: AtomicI8 = AtomicI8::new(0);

pub fn publish_wifi_rssi(rssi: Option<i8>) {
    WIFI_RSSI.store(rssi.unwrap_or(0), Ordering::Relaxed);
}

pub fn wifi_rssi() -> Option<i8> {
    match WIFI_RSSI.load(Ordering::Relaxed) {
        0 => None,
        v => Some(v),
    }
}

/// The one Clock implementation (§11.3): monotonic from esp_timer, wall clock
/// only once SNTP has synced.
pub struct EspClock;

impl Clock for EspClock {
    fn monotonic(&self) -> Millis {
        // esp_timer_get_time is µs since boot, monotonic, 64-bit.
        let us = unsafe { sys::esp_timer_get_time() };
        Millis(u64::try_from(us).unwrap_or(0) / 1000)
    }

    fn unix_seconds(&self) -> Option<u64> {
        if !TIME_SYNCED.load(Ordering::Relaxed) {
            return None;
        }
        let mut tv = sys::timeval {
            tv_sec: 0,
            tv_usec: 0,
        };
        let ok = unsafe { sys::gettimeofday(&mut tv, std::ptr::null_mut()) } == 0;
        (ok && tv.tv_sec > 0).then(|| u64::try_from(tv.tv_sec).unwrap_or(0))
    }
}

pub fn free_heap_bytes() -> u32 {
    unsafe { sys::esp_get_free_heap_size() }
}

pub fn min_free_heap_bytes() -> u32 {
    unsafe { sys::esp_get_minimum_free_heap_size() }
}

/// Reboot-reason accounting (§11.2). `esp_reset_reason()` collapses every
/// deliberate `esp_restart()` into ESP_RST_SW, so the reason is recorded in NVS
/// BEFORE restarting and reconciled at the next boot.
pub struct RebootLedger {
    nvs: EspNvs<NvsDefault>,
}

const PENDING_KEY: &str = "pending";
const REASONS: [&str; 7] = [
    "panic",
    "wifi",
    "ble_wedge",
    "ota",
    "power",
    "wdt",
    "unknown",
];

impl RebootLedger {
    pub fn new(partition: EspNvsPartition<NvsDefault>) -> anyhow::Result<Self> {
        Ok(Self {
            nvs: EspNvs::new(partition, "reboot", true)?,
        })
    }

    /// Boot path (§11.2): classify this boot, bump the matching NVS counter,
    /// clear the pending reason (a stale one would mislabel the NEXT reset),
    /// and return the counters plus this boot's reason.
    pub fn account_boot(&mut self) -> (RebootCounts, &'static str) {
        let reset = unsafe { sys::esp_reset_reason() };
        let mut pending_buf = [0u8; 16];
        let pending = self
            .nvs
            .get_str(PENDING_KEY, &mut pending_buf)
            .ok()
            .flatten()
            .map(str::to_owned);
        let _ = self.nvs.remove(PENDING_KEY);

        #[allow(non_upper_case_globals)]
        let reason: &'static str = match reset {
            sys::esp_reset_reason_t_ESP_RST_SW => match pending.as_deref() {
                Some("wifi") => "wifi",
                Some("ble_wedge") => "ble_wedge",
                Some("ota") => "ota",
                // An ESP_RST_SW with no declared reason is itself a signal:
                // something called esp_restart() down an unlabelled path (§11.2).
                _ => "unknown",
            },
            sys::esp_reset_reason_t_ESP_RST_PANIC => "panic",
            sys::esp_reset_reason_t_ESP_RST_TASK_WDT
            | sys::esp_reset_reason_t_ESP_RST_INT_WDT
            | sys::esp_reset_reason_t_ESP_RST_WDT => "wdt",
            sys::esp_reset_reason_t_ESP_RST_POWERON | sys::esp_reset_reason_t_ESP_RST_BROWNOUT => {
                "power"
            }
            _ => "unknown",
        };

        let key = reason; // NVS key per reason; two writes per boot is negligible wear
        let count = self.nvs.get_u32(key).ok().flatten().unwrap_or(0) + 1;
        let _ = self.nvs.set_u32(key, count);

        let read =
            |ledger: &EspNvs<NvsDefault>, k: &str| ledger.get_u32(k).ok().flatten().unwrap_or(0);
        let counts = RebootCounts {
            panic: read(&self.nvs, "panic"),
            wifi: read(&self.nvs, "wifi"),
            ble_wedge: read(&self.nvs, "ble_wedge"),
            ota: read(&self.nvs, "ota"),
            power: read(&self.nvs, "power"),
            wdt: read(&self.nvs, "wdt"),
            unknown: read(&self.nvs, "unknown"),
        };
        let _ = REASONS; // canonical list, kept adjacent to the match above
        (counts, reason)
    }

    /// Intentional-reboot path (§11.2): declare the reason, commit, restart.
    /// Never returns.
    pub fn restart_with_reason(&mut self, reason: &str) -> ! {
        let _ = self.nvs.set_str(PENDING_KEY, reason);
        log::warn!("restarting: {reason}");
        unsafe { sys::esp_restart() };
    }
}
