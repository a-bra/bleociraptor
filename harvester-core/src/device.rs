// ABOUTME: Device definition plus const-fn hex parsers behind mac!/bindkey! —
// ABOUTME: a malformed MAC or bindkey is a build error, not a silent no-show (§5.3).

/// A configured sensor. Defined here so the type and its parsers are
/// host-testable; the actual DEVICES array is instantiated in the gitignored
/// `harvester/src/devices.rs` and passed to core as `&'static [Device]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Device {
    pub mac: [u8; 6],
    pub name: &'static str,
    pub bindkey: Option<[u8; 16]>,
}

impl Device {
    pub const fn plain(mac: [u8; 6], name: &'static str) -> Self {
        Self {
            mac,
            name,
            bindkey: None,
        }
    }

    pub const fn encrypted(mac: [u8; 6], name: &'static str, bindkey: [u8; 16]) -> Self {
        Self {
            mac,
            name,
            bindkey: Some(bindkey),
        }
    }

    pub const fn is_encrypted(&self) -> bool {
        self.bindkey.is_some()
    }
}

// The parsers below run at const-eval time only (via mac!/bindkey!). A panic
// during const evaluation IS the feature: it surfaces as a compile error at the
// exact line of the malformed literal. None of this executes at runtime, so the
// crate-level panic-free lints are deliberately waived here and only here.
#[allow(
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::panic
)]
pub const fn parse_mac(s: &str) -> [u8; 6] {
    let b = s.as_bytes();
    if b.len() != 17 {
        panic!("MAC must be exactly AA:BB:CC:DD:EE:FF");
    }
    let mut out = [0u8; 6];
    let mut i = 0;
    while i < 6 {
        let p = i * 3;
        out[i] = (hex_val(b[p]) << 4) | hex_val(b[p + 1]);
        if i < 5 && b[p + 2] != b':' {
            panic!("MAC bytes must be separated by ':'");
        }
        i += 1;
    }
    out
}

#[allow(
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::panic
)]
pub const fn parse_bindkey(s: &str) -> [u8; 16] {
    let b = s.as_bytes();
    if b.len() != 32 {
        panic!("bindkey must be exactly 32 hex characters");
    }
    let mut out = [0u8; 16];
    let mut i = 0;
    while i < 16 {
        out[i] = (hex_val(b[i * 2]) << 4) | hex_val(b[i * 2 + 1]);
        i += 1;
    }
    out
}

#[allow(clippy::panic, clippy::arithmetic_side_effects)]
const fn hex_val(c: u8) -> u8 {
    match c {
        b'0'..=b'9' => c - b'0',
        b'a'..=b'f' => c - b'a' + 10,
        b'A'..=b'F' => c - b'A' + 10,
        _ => panic!("not a hex digit"),
    }
}

/// `mac!("AA:BB:CC:DD:EE:FF")` → `[u8; 6]` at compile time.
#[macro_export]
macro_rules! mac {
    ($s:literal) => {
        $crate::device::parse_mac($s)
    };
}

/// `bindkey!("00112233445566778899aabbccddeeff")` → `[u8; 16]` at compile time.
#[macro_export]
macro_rules! bindkey {
    ($s:literal) => {
        $crate::device::parse_bindkey($s)
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    // Malformed inputs are compile errors by design and cannot be asserted in a
    // runtime test; these confirm the happy path decodes byte-exactly.
    const MAC: [u8; 6] = mac!("Aa:bB:CC:0f:00:2A");
    const KEY: [u8; 16] = bindkey!("00112233445566778899aabbccddeeff");

    #[test]
    fn mac_parses_mixed_case_at_compile_time() {
        assert_eq!(MAC, [0xAA, 0xBB, 0xCC, 0x0F, 0x00, 0x2A]);
    }

    #[test]
    fn bindkey_parses_at_compile_time() {
        assert_eq!(
            KEY,
            [
                0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xAA, 0xBB, 0xCC, 0xDD,
                0xEE, 0xFF
            ]
        );
    }

    #[test]
    fn constructors_set_encryption() {
        let plain = Device::plain(MAC, "humidor");
        let enc = Device::encrypted(MAC, "living_room", KEY);
        assert!(!plain.is_encrypted());
        assert!(enc.is_encrypted());
    }
}
