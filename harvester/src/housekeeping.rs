// ABOUTME: The watchdogged 1 Hz sweep (§6.2, §15): staleness, wedge check, spark
// ABOUTME: sampling, Wi-Fi supervision. Decisions come from core; effects happen here.

use esp_idf_svc::hal::task::watchdog::{
    config::Config as TwdtConfig, WatchdogSubscription, TWDTDriver, TWDT,
};
use harvester_core::{Clock, Millis, WedgeDetector, WedgeVerdict};
use std::sync::Mutex;
use std::time::Duration;

use crate::effects::{EspClock, RebootLedger};
use crate::state::{HOT, SPARK};
use crate::wifi::{self, Wifi};
use crate::{config, ring_log};

/// Everything the sweep supervises. Owned by the housekeeping thread.
pub struct Housekeeping {
    pub wifi: Wifi,
    pub ledger: RebootLedger,
    /// True while an OTA body is streaming (§13): suppresses BOTH deliberate
    /// reboot paths. Set/cleared by the OTA handler via an RAII guard.
    pub ota_in_progress: &'static Mutex<bool>,
}

/// Runs forever on the main task. The caller subscribed that task to the TWDT
/// BEFORE Wi-Fi/BLE bring-up (§6.2 boot order): a blocking hang anywhere in
/// bring-up must reboot, and idle-task watching alone would not catch it.
pub fn run(mut hk: Housekeeping, mut watch: WatchdogSubscription<'_>) -> ! {
    let mut wedge = WedgeDetector::new();
    let mut ota_validated = false;
    let mut last_spark_sample = Millis(0);
    let spark_interval = Millis(config::SPARK_SAMPLE_INTERVAL.as_millis() as u64);

    loop {
        let now = EspClock.monotonic();

        // --- staleness + wedge/validation inputs, one short HOT hold (rule 2) ---
        let (newest, unknown_total, any_ok_beacon) = {
            let mut guard = match HOT.lock() {
                Ok(g) => g,
                Err(_) => {
                    // A poisoned HOT means a panic mid-update; the panic path
                    // is already rebooting us. Spin until it does.
                    std::thread::sleep(Duration::from_millis(100));
                    continue;
                }
            };
            match guard.as_mut() {
                Some(reg) => {
                    reg.sweep(now);
                    // Per-boot ok counters: §13's "successfully parsed beacon".
                    let any_ok =
                        (0..reg.len()).any(|i| reg.view(i).is_some_and(|v| v.counters.ok > 0));
                    (reg.newest_last_seen(), reg.unknown_adverts_total(), any_ok)
                }
                None => (None, 0, false),
            }
        };

        // --- sparkline sampling every SPARK_SAMPLE_INTERVAL (§18) ---
        if now.since(last_spark_sample) >= spark_interval {
            last_spark_sample = now;
            sample_spark();
        }

        // --- Wi-Fi supervision + §15 giveup ---
        hk.wifi.supervise(now);
        crate::effects::publish_wifi_rssi(hk.wifi.rssi_dbm());
        let ota_busy = hk.ota_in_progress.lock().map(|g| *g).unwrap_or(false);
        if !ota_busy {
            // §15: unconditional after 5 min down, INCLUDING a boot that never
            // associated — wrong-credential builds reboot-loop visibly rather
            // than sitting dark forever (§13 documents and accepts this).
            if let Some(down) = wifi::down_for(&hk.wifi, now) {
                let giveup = Millis(config::WIFI_GIVEUP.as_millis() as u64);
                if down >= giveup {
                    ring_log!(now, "wifi unrecoverable for {}s, rebooting", down.as_secs());
                    hk.ledger.restart_with_reason("wifi");
                }
            }
        }

        // --- OTA (§13): queued post-upload restart, then validation ---
        if crate::ota::OTA_RESTART_REQUESTED.swap(false, std::sync::atomic::Ordering::SeqCst) {
            hk.ledger.restart_with_reason("ota");
        }
        // §13: a successfully parsed beacon OR a stray advert. Failed beacons
        // deliberately do NOT validate — they feed only the wedge watermark.
        let radio_evidence = any_ok_beacon || unknown_total > 0;
        crate::ota::validate_if_due(now.as_secs(), radio_evidence, &mut ota_validated);

        // --- wedge verdict (§15.1): core decides, this file reboots ---
        let verdict = wedge.check(
            now,
            newest,
            unknown_total,
            ota_busy,
            Millis(config::WEDGE_WINDOW.as_millis() as u64),
        );
        if verdict == WedgeVerdict::Wedged {
            ring_log!(now, "radio wedge detected, rebooting");
            hk.ledger.restart_with_reason("ble_wedge");
        }

        // Feed the watchdog only at the END of a healthy sweep.
        let _ = watch.feed();
        std::thread::sleep(Duration::from_secs(1));
    }
}

/// Copy one column of current readings into the spark ring. Takes HOT then
/// SPARK sequentially, never nested (§6.2 rule 2).
fn sample_spark() {
    let mut columns: [(Option<i16>, Option<i16>); harvester_core::MAX_DEVICES] =
        [(None, None); harvester_core::MAX_DEVICES];
    let mut count = 0usize;
    if let Ok(guard) = HOT.lock() {
        if let Some(reg) = guard.as_ref() {
            count = reg.len();
            for (idx, slot) in columns.iter_mut().enumerate().take(count) {
                if let Some(v) = reg.view(idx) {
                    // Offline devices sample as None — the §18 gap, not 0 °C.
                    let hum_i16 = v
                        .readings
                        .humidity_centi
                        .and_then(|h| i16::try_from(h).ok());
                    *slot = (v.readings.temperature_centi, hum_i16);
                }
            }
        }
    } // HOT released before SPARK is taken.
    if let Ok(mut spark) = SPARK.lock() {
        for (idx, (temp, hum)) in columns.iter().enumerate().take(count) {
            spark.push(idx, *temp, *hum);
        }
    }
}

/// Build the TWDT driver. Separate from run() so main can construct it before
/// anything that can hang (§6.2 boot order).
pub fn watchdog(twdt: TWDT<'static>) -> anyhow::Result<TWDTDriver<'static>> {
    let config = TwdtConfig {
        duration: Duration::from_secs(10),
        panic_on_trigger: true, // panic → reboot → OTA rollback if pending (§15)
        ..Default::default()
    };
    Ok(TWDTDriver::new(twdt, &config)?)
}
