// ABOUTME: NimBLE passive observer (§9) — receives adverts, hands bytes to
// ABOUTME: harvester-core, records outcomes. No decisions here beyond routing.

use esp32_nimble::{BLEAdvertisedDevice, BLEDevice, BLEScan};
use harvester_core::frame::{self, MAX_PLAINTEXT};
use harvester_core::parse;
use harvester_core::registry::BeaconFailure;
use harvester_core::Clock;

use crate::config;
use crate::effects::EspClock;
use crate::ring_log;
use crate::state::HOT;

/// Start the scanner on its own thread. Passive is mandatory (§9): active
/// scanning provokes SCAN_RSP from every sensor in range, draining their
/// CR2032s to learn nothing. The scan never terminates by itself; if the stack
/// wedges, §15.1's detector reboots us — that is the designed recovery path.
pub fn spawn() -> anyhow::Result<()> {
    std::thread::Builder::new()
        .name("ble-scan".into())
        .stack_size(6144)
        .spawn(|| {
            let device = BLEDevice::take();
            let mut scan = BLEScan::new();
            scan.active_scan(false)
                .filter_duplicates(false) // every advert counts (§18 beacons/min)
                .interval(config::SCAN_INTERVAL_MS)
                .window(config::SCAN_WINDOW_MS);
            let result = esp_idf_svc::hal::task::block_on(scan.start(
                device,
                i32::MAX, // BLE_HS_FOREVER; the callback never asks to stop
                |dev, data| {
                    on_advert(dev, data.payload());
                    None::<()>
                },
            ));
            // Unreachable in health; if the stack errors out of the scan, the
            // wedge detector notices the silence and reboots (§15.1).
            log::error!("BLE scan ended unexpectedly: {result:?}");
        })?;
    Ok(())
}

/// §6.2 rule 1: this runs on the NimBLE host task. No allocation, no flash
/// I/O; HOT is taken once, mutated, released. LOGS is touched only on the
/// rare failure/novelty paths, and never while HOT is held (rule 2).
fn on_advert(dev: &BLEAdvertisedDevice, adv_payload: &[u8]) {
    let now = EspClock.monotonic();
    // as_be_bytes: human-readable order — the §8 reversal, done by the crate.
    let mac = dev.addr().as_be_bytes();

    let (idx, bindkey) = {
        let Ok(mut guard) = HOT.lock() else { return };
        let Some(reg) = guard.as_mut() else { return };
        match reg.device_index(&mac) {
            Some(idx) => {
                let bindkey = reg.view(idx).and_then(|v| v.device.bindkey);
                (idx, bindkey)
            }
            None => {
                // Stray traffic: the §15.1 radio-liveness heartbeat.
                reg.record_unknown_advert();
                return;
            }
        }
    };

    // A configured device without BTHome service data (e.g. a scan artifact)
    // is unintelligible: parse_fail (§7.1 "no or empty 0xFCD2 service data").
    let Some(svc) = parse::fcd2_service_data(adv_payload) else {
        record(idx, now, dev.rssi(), Err(BeaconFailure::Parse));
        return;
    };
    let Some((&device_info, body)) = svc.split_first() else {
        record(idx, now, dev.rssi(), Err(BeaconFailure::Parse));
        return;
    };

    let encrypted_frame = device_info & 0x01 != 0;
    let mut plaintext = [0u8; MAX_PLAINTEXT];
    let objects: &[u8] = match (encrypted_frame, bindkey) {
        (false, None) => body,
        (true, Some(key)) => match frame::decrypt(&key, &mac, device_info, body, &mut plaintext) {
            Ok(len) => plaintext.get(..len).unwrap_or(&[]),
            Err(e) => {
                record(idx, now, dev.rssi(), Err(BeaconFailure::Decrypt));
                ring_log!(now, "decrypt failed for device {idx}: {e:?}");
                return;
            }
        },
        // A plaintext frame from a bindkey-configured device carries no MIC:
        // accepting it would let anyone spoof an "encrypted" sensor. An
        // encrypted frame for a plaintext device cannot be checked either way.
        (plain_dev_enc_frame, _) => {
            record(idx, now, dev.rssi(), Err(BeaconFailure::Decrypt));
            ring_log!(
                now,
                "auth mismatch for device {idx}: frame encrypted={plain_dev_enc_frame}"
            );
            return;
        }
    };

    match parse::parse(device_info, objects) {
        Ok(parsed) => {
            let novel = record(idx, now, dev.rssi(), Ok(parsed));
            if novel {
                ring_log!(
                    now,
                    "device {idx}: unknown BTHome object id 0x{:02X}, walk stopped (\u{a7}7)",
                    parsed.unknown_object.unwrap_or(0)
                );
            }
        }
        Err(e) => {
            record(idx, now, dev.rssi(), Err(BeaconFailure::Parse));
            ring_log!(now, "parse failed for device {idx}: {e:?}");
        }
    }
}

fn record(
    idx: usize,
    now: harvester_core::Millis,
    rssi: i8,
    outcome: Result<parse::Parsed, BeaconFailure>,
) -> bool {
    let Ok(mut guard) = HOT.lock() else {
        return false;
    };
    let Some(reg) = guard.as_mut() else {
        return false;
    };
    reg.record_beacon(idx, now, rssi, outcome)
}
