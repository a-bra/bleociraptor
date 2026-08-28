// ABOUTME: /status JSON body — byte-for-byte the Python exporter's shape (§12,
// ABOUTME: exporter.py:77-80), spaces after colons and comma included.

/// `/status` body. The Python shape is kept exactly so §19's parallel-run
/// tooling cannot tell the implementations apart.
pub fn render_status<W: core::fmt::Write>(
    w: &mut W,
    device_timeout_seconds: u64,
    devices_active: usize,
) -> core::fmt::Result {
    write!(
        w,
        "{{\"device_timeout_seconds\": {device_timeout_seconds}, \"devices_active\": {devices_active}}}"
    )
}

#[cfg(test)]
mod tests {
    extern crate std;
    use std::string::String;

    use super::*;

    fn render(timeout: u64, active: usize) -> String {
        let mut s = String::new();
        assert!(render_status(&mut s, timeout, active).is_ok());
        s
    }

    #[test]
    fn python_shape_is_byte_exact() {
        assert_eq!(
            render(90, 5),
            "{\"device_timeout_seconds\": 90, \"devices_active\": 5}"
        );
    }

    #[test]
    fn values_are_not_hardcoded() {
        assert_eq!(
            render(120, 8),
            "{\"device_timeout_seconds\": 120, \"devices_active\": 8}"
        );
    }
}
