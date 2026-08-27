// ABOUTME: BTHome v2 frame decryption — AES-CCM-128, 4-byte tag, 13-byte nonce,
// ABOUTME: empty AAD, allocation-free into a caller-supplied buffer (SPEC.md §8).

use aes::Aes128;
use ccm::aead::generic_array::GenericArray;
use ccm::aead::{AeadInPlace, KeyInit};
use ccm::consts::{U13, U4};
use ccm::Ccm;

/// AES-CCM, 128-bit key, 4-byte tag, 13-byte nonce (§8).
type Cipher = Ccm<Aes128, U4, U13>;

/// Longest plaintext a legacy advertisement can carry; sized generously.
pub const MAX_PLAINTEXT: usize = 24;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecryptError {
    TooShort,
    BufferTooSmall,
    MicMismatch,
}

/// Decrypts the encrypted body of a BTHome v2 0xFCD2 service data payload.
///
/// `body` is everything after the device_info byte:
/// `ciphertext(N) || counter(4, wire order) || mic(4)`.
///
/// `mac` must be in human-readable order (`AA:BB:...` → `[0xAA, 0xBB, ...]`);
/// NimBLE delivers it reversed, and reversing is the caller's job (§8).
/// The counter bytes go into the nonce exactly as they sit on the wire.
///
/// Returns the plaintext length written into `out`.
pub fn decrypt(
    bindkey: &[u8; 16],
    mac: &[u8; 6],
    device_info: u8,
    body: &[u8],
    out: &mut [u8],
) -> Result<usize, DecryptError> {
    let ciphertext_len = body.len().checked_sub(8).ok_or(DecryptError::TooShort)?;
    let (ciphertext, tail) = body
        .split_at_checked(ciphertext_len)
        .ok_or(DecryptError::TooShort)?;
    let (counter, mic) = tail.split_at_checked(4).ok_or(DecryptError::TooShort)?;
    let counter: [u8; 4] = counter.try_into().map_err(|_| DecryptError::TooShort)?;
    let mic: [u8; 4] = mic.try_into().map_err(|_| DecryptError::TooShort)?;

    // Nonce: mac(6, human-readable order) || 0xD2 0xFC || device_info ||
    // counter(4, exactly as it sits on the wire).
    let nonce: [u8; 13] = [
        mac[0],
        mac[1],
        mac[2],
        mac[3],
        mac[4],
        mac[5],
        0xD2,
        0xFC,
        device_info,
        counter[0],
        counter[1],
        counter[2],
        counter[3],
    ];

    let buffer = out
        .get_mut(..ciphertext_len)
        .ok_or(DecryptError::BufferTooSmall)?;
    buffer.copy_from_slice(ciphertext);

    Cipher::new(bindkey.into())
        .decrypt_in_place_detached(
            &GenericArray::from(nonce),
            &[],
            buffer,
            &GenericArray::from(mic),
        )
        .map_err(|_| DecryptError::MicMismatch)?;
    Ok(ciphertext_len)
}

#[cfg(test)]
#[allow(
    clippy::indexing_slicing,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::arithmetic_side_effects
)]
mod tests {
    use super::*;

    /// Identity shared by every encrypted corpus frame (tests/fixtures/corpus.json).
    const MAC: [u8; 6] = [0xAA, 0xBB, 0xCC, 0x00, 0x00, 0x02];
    const BINDKEY: [u8; 16] = [0x02; 16];

    /// Decodes hex into a fixed buffer; returns (buffer, byte length).
    fn unhex(s: &str) -> ([u8; 32], usize) {
        fn nibble(c: u8) -> u8 {
            match c {
                b'0'..=b'9' => c - b'0',
                b'a'..=b'f' => c - b'a' + 10,
                b'A'..=b'F' => c - b'A' + 10,
                _ => panic!("bad hex digit"),
            }
        }
        let bytes = s.as_bytes();
        assert!(bytes.len().is_multiple_of(2) && bytes.len() <= 64);
        let mut buf = [0u8; 32];
        for i in 0..bytes.len() / 2 {
            buf[i] = (nibble(bytes[2 * i]) << 4) | nibble(bytes[2 * i + 1]);
        }
        (buf, bytes.len() / 2)
    }

    /// Splits a corpus `frame_hex` into (device_info, body).
    fn split_frame(frame_hex: &str) -> (u8, [u8; 32], usize) {
        let (buf, len) = unhex(frame_hex);
        let mut body = [0u8; 32];
        body[..len - 1].copy_from_slice(&buf[1..len]);
        (buf[0], body, len - 1)
    }

    fn decrypt_frame(frame_hex: &str, out: &mut [u8]) -> Result<usize, DecryptError> {
        let (info, body, body_len) = split_frame(frame_hex);
        decrypt(&BINDKEY, &MAC, info, &body[..body_len], out)
    }

    /// Real captured frames from tests/fixtures/corpus.json (identity above),
    /// paired with their independently verified plaintexts.
    const CORPUS: [(&str, &str); 4] = [
        ("41df9b1b85a19e3d7cea100069eefb40", "0c920b10001101"),
        ("4197decf1eb0a3ba98ea1000afdd285b", "11013e00000000"),
        ("41cb12b8521c77e7d6b9ea1000b5d5ab7d", "015f02df0a03350f"),
        ("41a8a15931066ff980ea1000460fc387", "0c920b10001101"),
    ];

    #[test]
    fn decrypts_corpus_frames() {
        for (frame_hex, plaintext_hex) in CORPUS {
            let mut out = [0u8; MAX_PLAINTEXT];
            let len = decrypt_frame(frame_hex, &mut out).unwrap();
            let (expected, expected_len) = unhex(plaintext_hex);
            assert_eq!(len, expected_len, "length mismatch for {frame_hex}");
            assert_eq!(
                &out[..len],
                &expected[..expected_len],
                "plaintext mismatch for {frame_hex}"
            );
        }
    }

    #[test]
    fn voltage_plaintext_is_wire_order() {
        // 0x0C 0x92 0x0B ... — object 0x0C (voltage), value LE 0x0B92 = 2962 mV.
        // Bytes 1..3 being [0x92, 0x0B] proves we did not reorder anything.
        let mut out = [0u8; MAX_PLAINTEXT];
        let len = decrypt_frame("41df9b1b85a19e3d7cea100069eefb40", &mut out).unwrap();
        assert_eq!(len, 7);
        assert_eq!(&out[1..3], &[0x92, 0x0B]);
    }

    #[test]
    fn corrupt_mic_byte_is_mic_mismatch() {
        let (info, mut body, body_len) = split_frame("41df9b1b85a19e3d7cea100069eefb40");
        body[body_len - 1] ^= 0x01;
        let mut out = [0u8; MAX_PLAINTEXT];
        let err = decrypt(&BINDKEY, &MAC, info, &body[..body_len], &mut out).unwrap_err();
        assert_eq!(err, DecryptError::MicMismatch);
    }

    #[test]
    fn corrupt_ciphertext_byte_is_mic_mismatch() {
        let (info, mut body, body_len) = split_frame("41df9b1b85a19e3d7cea100069eefb40");
        body[0] ^= 0x01;
        let mut out = [0u8; MAX_PLAINTEXT];
        let err = decrypt(&BINDKEY, &MAC, info, &body[..body_len], &mut out).unwrap_err();
        assert_eq!(err, DecryptError::MicMismatch);
    }

    #[test]
    fn wrong_bindkey_is_mic_mismatch() {
        let (info, body, body_len) = split_frame("41df9b1b85a19e3d7cea100069eefb40");
        let mut out = [0u8; MAX_PLAINTEXT];
        let err = decrypt(&[0x03; 16], &MAC, info, &body[..body_len], &mut out).unwrap_err();
        assert_eq!(err, DecryptError::MicMismatch);
    }

    #[test]
    fn body_shorter_than_counter_plus_mic_is_too_short() {
        let mut out = [0u8; MAX_PLAINTEXT];
        let err = decrypt(&BINDKEY, &MAC, 0x41, &[0u8; 7], &mut out).unwrap_err();
        assert_eq!(err, DecryptError::TooShort);
    }

    #[test]
    fn out_buffer_smaller_than_ciphertext_is_buffer_too_small() {
        let (info, body, body_len) = split_frame("41df9b1b85a19e3d7cea100069eefb40");
        let mut out = [0u8; 3];
        let err = decrypt(&BINDKEY, &MAC, info, &body[..body_len], &mut out).unwrap_err();
        assert_eq!(err, DecryptError::BufferTooSmall);
    }

    #[test]
    fn zero_length_ciphertext_with_valid_mic_returns_zero() {
        // MIC computed for empty plaintext under the corpus identity,
        // info 0x41, counter EA 10 00 00.
        let (body, body_len) = unhex("ea100000a79e7d2b");
        let mut out = [0u8; MAX_PLAINTEXT];
        let len = decrypt(&BINDKEY, &MAC, 0x41, &body[..body_len], &mut out).unwrap();
        assert_eq!(len, 0);
    }
}
