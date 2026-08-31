// ABOUTME: Firmware entry point — wiring, threads and the three mutexes (§6.2).
// ABOUTME: Bring-up skeleton: proves the toolchain, prints build info on serial.

mod config;
mod devices;

fn main() {
    // Required for esp-idf-sys patches to apply and the runtime to start.
    esp_idf_svc::sys::link_patches();
    esp_idf_svc::log::EspLogger::initialize_default();

    log::info!(
        "BLEociraptor harvester {} ({}) — {} devices configured, port {}",
        env!("CARGO_PKG_VERSION"),
        env!("HARVESTER_GIT_SHA"),
        devices::DEVICES.len(),
        config::LISTEN_PORT,
    );
    // The full tunable set on serial at every boot: the device is headless, and
    // "which timeout is this build actually running" should never be a guess.
    log::info!(
        "tunables: device_timeout={}s wedge_window={}s spark_interval={}s \
         wifi_giveup={}s ota_validate_after={}s ota_validate_deadline={}s \
         scan interval={}u window={}u",
        config::DEVICE_TIMEOUT.as_secs(),
        config::WEDGE_WINDOW.as_secs(),
        config::SPARK_SAMPLE_INTERVAL.as_secs(),
        config::WIFI_GIVEUP.as_secs(),
        config::OTA_VALIDATE_AFTER.as_secs(),
        config::OTA_VALIDATE_DEADLINE.as_secs(),
        config::SCAN_INTERVAL_UNITS,
        config::SCAN_WINDOW_UNITS,
    );
    let _ = config::tunables();
    // Anchor the secrets so the build stays warning-free; wifi.rs and ota.rs
    // are their real consumers. Never log these.
    let _ = (devices::WIFI_SSID, devices::WIFI_PSK, devices::OTA_TOKEN);
}
