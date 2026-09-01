// ABOUTME: Wi-Fi effects (§14): STA + DHCP + hostname, power save off, exponential
// ABOUTME: backoff reconnect supervised from the 1 Hz sweep. No decisions here.

use esp_idf_svc::eventloop::EspSystemEventLoop;
use esp_idf_svc::hal::modem::Modem;
use esp_idf_svc::nvs::{EspNvsPartition, NvsDefault};
use esp_idf_svc::sys;
use esp_idf_svc::wifi::{AuthMethod, ClientConfiguration, Configuration, EspWifi};
use harvester_core::Millis;

use crate::devices;

/// §14: 1, 2, 4 … capped at 60 s.
const BACKOFF_CAP_SECS: u64 = 60;

pub struct Wifi {
    driver: EspWifi<'static>,
    /// Next reconnect attempt not before this instant.
    next_attempt: Millis,
    backoff_secs: u64,
    /// Monotonic instant we last had an IP; drives §15's giveup timer.
    pub last_connected: Option<Millis>,
}

impl Wifi {
    pub fn new(
        modem: Modem<'static>,
        sysloop: EspSystemEventLoop,
        nvs: EspNvsPartition<NvsDefault>,
    ) -> anyhow::Result<Self> {
        let mut driver = EspWifi::new(modem, sysloop, Some(nvs))?;
        driver
            .driver_mut()
            .set_configuration(&Configuration::Client(ClientConfiguration {
                ssid: devices::WIFI_SSID
                    .try_into()
                    .map_err(|_| anyhow::anyhow!("SSID longer than 32 bytes"))?,
                password: devices::WIFI_PSK
                    .try_into()
                    .map_err(|_| anyhow::anyhow!("PSK longer than 64 bytes"))?,
                auth_method: AuthMethod::WPA2Personal,
                ..Default::default()
            }))?;
        Ok(Self {
            driver,
            next_attempt: Millis(0),
            backoff_secs: 1,
            last_connected: None,
        })
    }

    pub fn start(&mut self) -> anyhow::Result<()> {
        self.driver.start()?;
        // Set the DHCP/netif hostname for router-side name resolution (§14 —
        // deliberately no mDNS responder).
        // The STA netif's raw handle, via the stable ifkey lookup — EspNetif
        // does not expose set_hostname publicly in this esp-idf-svc version.
        let netif = unsafe { sys::esp_netif_get_handle_from_ifkey(c"WIFI_STA_DEF".as_ptr()) };
        if !netif.is_null() {
            esp_idf_svc::sys::esp!(unsafe {
                sys::esp_netif_set_hostname(netif, c"harvester".as_ptr())
            })?;
        }
        // MIN_MODEM power save adds DTIM-interval latency to every scrape;
        // ~20 mA is irrelevant on mains power (§4.1).
        esp_idf_svc::sys::esp!(unsafe { sys::esp_wifi_set_ps(sys::wifi_ps_type_t_WIFI_PS_NONE) })?;
        self.driver.connect()?;
        Ok(())
    }

    /// Called once per housekeeping sweep. Reconnects with §14's backoff and
    /// tracks connection state for the giveup timer.
    pub fn supervise(&mut self, now: Millis) {
        let up = self.driver.is_up().unwrap_or(false);
        if up {
            self.last_connected = Some(now);
            self.backoff_secs = 1;
            return;
        }
        if now >= self.next_attempt {
            log::info!(
                "wifi down, reconnecting (next backoff {}s)",
                self.backoff_secs
            );
            let _ = self.driver.connect();
            self.next_attempt = now.saturating_add(Millis::from_secs(self.backoff_secs));
            self.backoff_secs = (self.backoff_secs * 2).min(BACKOFF_CAP_SECS);
        }
    }

    /// §11.2's ble_harvester_wifi_rssi_dbm; None while unassociated (the gauge gaps).
    pub fn rssi_dbm(&self) -> Option<i8> {
        let mut info = sys::wifi_ap_record_t::default();
        let ok = unsafe { sys::esp_wifi_sta_get_ap_info(&mut info) } == sys::ESP_OK;
        ok.then_some(info.rssi as i8)
    }
}

/// Helper for §15's giveup rule: seconds we have been continuously down.
pub fn down_for(wifi: &Wifi, now: Millis) -> Option<Millis> {
    match wifi.last_connected {
        Some(last) => Some(now.since(last)),
        // Never connected this boot: down since boot.
        None => Some(now),
    }
}
