// ABOUTME: Fixture-corpus harness (§17.2, acceptance criterion 1) — replays every
// ABOUTME: captured frame through decrypt+parse and checks the recorded expected values.

// The crate lints (§6.1) apply to test targets too, but this harness is std and
// host-only; frame.rs's unit tests establish the same allowances for test code.
#![allow(
    clippy::indexing_slicing,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::arithmetic_side_effects
)]

use harvester_core::frame::{decrypt, MAX_PLAINTEXT};
use harvester_core::parse::parse;
use serde::Deserialize;

#[derive(Deserialize)]
struct Corpus {
    frames: Vec<Frame>,
}

#[derive(Deserialize)]
struct Frame {
    device: String,
    mac: String,
    bindkey: Option<String>,
    encrypted: bool,
    frame_hex: String,
    shape: String,
    #[serde(default)]
    expected: Expected,
}

/// `expected` omits absent fields (§17.2); an omitted field asserts the reading
/// is None. Values are pre-scaled floats — floats live here, never in core.
/// `deny_unknown_fields` so a new expected field cannot slip past uncompared.
#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct Expected {
    #[serde(default)]
    temperature: Option<f64>,
    #[serde(default)]
    humidity: Option<f64>,
    #[serde(default)]
    battery: Option<u8>,
    #[serde(default)]
    voltage: Option<f64>,
}

fn hex_bytes(s: &str) -> Vec<u8> {
    assert!(s.len().is_multiple_of(2), "odd-length hex string {s:?}");
    (0..s.len() / 2)
        .map(|i| {
            u8::from_str_radix(&s[2 * i..2 * i + 2], 16)
                .unwrap_or_else(|e| panic!("bad hex {s:?}: {e}"))
        })
        .collect()
}

/// "AA:BB:CC:00:00:02" → bytes in human-readable order, as decrypt expects.
fn mac_bytes(s: &str) -> [u8; 6] {
    let parts: Vec<u8> = s
        .split(':')
        .map(|octet| u8::from_str_radix(octet, 16).unwrap_or_else(|e| panic!("bad MAC {s:?}: {e}")))
        .collect();
    parts
        .try_into()
        .unwrap_or_else(|_| panic!("MAC {s:?} is not 6 octets"))
}

fn bindkey_bytes(s: &str) -> [u8; 16] {
    hex_bytes(s)
        .try_into()
        .unwrap_or_else(|_| panic!("bindkey {s:?} is not 16 bytes"))
}

#[test]
fn every_corpus_frame_parses_to_its_recorded_expected_values() {
    let corpus: Corpus = serde_json::from_str(include_str!("fixtures/corpus.json"))
        .expect("corpus.json failed to deserialize");

    // Fail loudly if the file changes silently (§17.2: 33 frames).
    assert_eq!(
        corpus.frames.len(),
        33,
        "corpus frame count changed — was the fixture regenerated?"
    );

    let mut encrypted_frames = 0usize;
    let mut plaintext_frames = 0usize;

    for frame in &corpus.frames {
        let ctx = format!(
            "device={} shape={:?} frame_hex={}",
            frame.device, frame.shape, frame.frame_hex
        );

        // frame_hex byte 0 is device_info; the rest is the body (§17.2).
        let bytes = hex_bytes(&frame.frame_hex);
        let (&device_info, body) = bytes
            .split_first()
            .unwrap_or_else(|| panic!("empty frame_hex for {ctx}"));

        // Encrypted bodies are ciphertext || counter || mic for frame::decrypt;
        // plaintext bodies go straight to the object walk.
        let mut plaintext_buf = [0u8; MAX_PLAINTEXT];
        let payload: &[u8] = if frame.encrypted {
            encrypted_frames += 1;
            let bindkey = frame
                .bindkey
                .as_deref()
                .unwrap_or_else(|| panic!("encrypted frame without bindkey for {ctx}"));
            let len = decrypt(
                &bindkey_bytes(bindkey),
                &mac_bytes(&frame.mac),
                device_info,
                body,
                &mut plaintext_buf,
            )
            .unwrap_or_else(|e| panic!("decrypt failed ({e:?}) for {ctx}"));
            &plaintext_buf[..len]
        } else {
            plaintext_frames += 1;
            body
        };

        // §7.1: every corpus frame must classify `ok`.
        let parsed = parse(device_info, payload)
            .unwrap_or_else(|e| panic!("parse failed ({e:?}) for {ctx}"));

        // Every id in this corpus (including 0x3E) is whitelisted; a Some here
        // means the §7 whitelist regressed.
        assert_eq!(
            parsed.unknown_object, None,
            "walk stopped at an unknown object id — whitelist regression? {ctx}"
        );

        // Exact match in both directions: an omitted expected field asserts None.
        assert_eq!(
            parsed.readings.temperature_centi,
            frame
                .expected
                .temperature
                .map(|t| (t * 100.0).round() as i16),
            "temperature mismatch for {ctx}"
        );
        assert_eq!(
            parsed.readings.humidity_centi,
            frame.expected.humidity.map(|h| (h * 100.0).round() as u16),
            "humidity mismatch for {ctx}"
        );
        assert_eq!(
            parsed.readings.battery_percent, frame.expected.battery,
            "battery mismatch for {ctx}"
        );
        assert_eq!(
            parsed.readings.voltage_milli,
            frame.expected.voltage.map(|v| (v * 1000.0).round() as u16),
            "voltage mismatch for {ctx}"
        );
    }

    assert!(
        encrypted_frames >= 1,
        "corpus lost its encrypted coverage (0 encrypted frames)"
    );
    assert!(
        plaintext_frames >= 1,
        "corpus lost its plaintext coverage (0 plaintext frames)"
    );
}
