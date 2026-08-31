// ABOUTME: Committed behavioural tunables (SPEC.md §5.2) — tuning history lives
// ABOUTME: in git. When changing one, record the measurement in the commit message.

use core::time::Duration;

pub const LISTEN_PORT: u16 = 8183;
pub const DEVICE_TIMEOUT: Duration = Duration::from_secs(90);
// §9: 40ms interval / 30ms window = 75% duty. VERIFIED against the pinned
// esp32-nimble 0.12.0 source: interval()/window() take MILLISECONDS and divide
// by 0.625 internally — passing the spec's 0.625ms units here would silently
// mean a 64ms interval. Re-verify on any crate bump.
pub const SCAN_INTERVAL_MS: u16 = 40;
pub const SCAN_WINDOW_MS: u16 = 30;
pub const WEDGE_WINDOW: Duration = Duration::from_secs(600); // §15.1
pub const WIFI_GIVEUP: Duration = Duration::from_secs(300); // §15
pub const SPARK_SAMPLE_INTERVAL: Duration = Duration::from_secs(300);
pub const OTA_VALIDATE_AFTER: Duration = Duration::from_secs(60); // §13
pub const OTA_VALIDATE_DEADLINE: Duration = Duration::from_secs(600); // §13

/// Bundle for injection into harvester-core (§5.2): core never reaches back.
pub fn tunables() -> harvester_core::Tunables {
    harvester_core::Tunables {
        device_timeout: harvester_core::Millis(DEVICE_TIMEOUT.as_millis() as u64),
        wedge_window: harvester_core::Millis(WEDGE_WINDOW.as_millis() as u64),
        spark_sample_interval: harvester_core::Millis(SPARK_SAMPLE_INTERVAL.as_millis() as u64),
    }
}
