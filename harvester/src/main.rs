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
}
