// ABOUTME: Firmware entry point — wiring only (§6.1): construct state, start the
// ABOUTME: watchdogged sweep FIRST (§6.2 boot order), then Wi-Fi, SNTP, HTTP.

mod ble;
mod config;
mod devices;
mod effects;
mod housekeeping;
mod http;
mod state;
mod wifi;

use esp_idf_svc::eventloop::EspSystemEventLoop;
use esp_idf_svc::hal::peripherals::Peripherals;
use esp_idf_svc::nvs::EspDefaultNvsPartition;
use esp_idf_svc::sntp::{EspSntp, SyncStatus};
use harvester_core::Registry;
use std::sync::atomic::Ordering;
use std::sync::Mutex;

static OTA_IN_PROGRESS: Mutex<bool> = Mutex::new(false);

fn main() -> anyhow::Result<()> {
    esp_idf_svc::sys::link_patches();
    esp_idf_svc::log::EspLogger::initialize_default();

    log::info!(
        "BLEociraptor harvester {} ({}) — {} devices, port {}",
        env!("CARGO_PKG_VERSION"),
        env!("HARVESTER_GIT_SHA"),
        devices::DEVICES.len(),
        config::LISTEN_PORT,
    );
    log::info!(
        "tunables: device_timeout={}s wedge_window={}s spark_interval={}s \
         wifi_giveup={}s ota_validate_after={}s ota_validate_deadline={}s \
         scan interval={}ms window={}ms",
        config::DEVICE_TIMEOUT.as_secs(),
        config::WEDGE_WINDOW.as_secs(),
        config::SPARK_SAMPLE_INTERVAL.as_secs(),
        config::WIFI_GIVEUP.as_secs(),
        config::OTA_VALIDATE_AFTER.as_secs(),
        config::OTA_VALIDATE_DEADLINE.as_secs(),
        config::SCAN_INTERVAL_MS,
        config::SCAN_WINDOW_MS,
    );

    // Anchor: ota.rs is OTA_TOKEN's real consumer (step 13). Never log it.
    let _ = devices::OTA_TOKEN;

    let peripherals = Peripherals::take()?;
    let sysloop = EspSystemEventLoop::take()?;
    let nvs = EspDefaultNvsPartition::take()?;

    // --- boot-reason accounting before anything can restart us (§11.2) ---
    let mut ledger = effects::RebootLedger::new(nvs.clone())?;
    let (reboot_counts, boot_reason) = ledger.account_boot();
    log::info!("boot reason: {boot_reason}; reboot counters {reboot_counts:?}");

    // --- §6.2 boot order: pure memory first (cannot block) ---
    if let Ok(mut hot) = state::HOT.lock() {
        *hot = Some(Registry::new(devices::DEVICES, config::tunables()));
    }

    // --- watchdog + housekeeping BEFORE anything that can hang (§6.2) ---
    let mut twdt = housekeeping::watchdog(peripherals.twdt)?;
    let mut wifi = wifi::Wifi::new(peripherals.modem, sysloop.clone(), nvs)?;
    wifi.start()?; // non-blocking: connect() is fired, supervision retries

    // SNTP non-blocking (§11.3); beacons before sync still get correct
    // timestamps because conversion happens at render time.
    let sntp = EspSntp::new_default()?;

    // --- HTTP (§12) ---
    let deps: &'static http::HttpDeps = Box::leak(Box::new(http::HttpDeps {
        wifi_rssi: effects::wifi_rssi,
        reboots: reboot_counts,
        last_reboot_reason: boot_reason,
    }));
    let _server = http::serve(deps)?;

    // BLE last (§6.2 boot order): by now a hang lands on a watchdogged system.
    ble::spawn()?;

    // --- housekeeping thread: owns wifi + ledger, feeds the TWDT (§15) ---
    let hk = housekeeping::Housekeeping {
        wifi,
        ledger,
        ota_in_progress: &OTA_IN_PROGRESS,
    };
    std::thread::Builder::new()
        .name("sntp-watch".into())
        .stack_size(8192)
        .spawn(move || {
            // The SNTP handle must outlive main; park it on this thread.
            let sntp = sntp;
            loop {
                if sntp.get_sync_status() == SyncStatus::Completed {
                    effects::TIME_SYNCED.store(true, Ordering::Relaxed);
                    break;
                }
                std::thread::sleep(std::time::Duration::from_secs(1));
            }
            std::mem::forget(sntp);
        })?;

    housekeeping::run(hk, &mut twdt)
}
