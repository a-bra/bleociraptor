// ABOUTME: Committed behavioural tunables (SPEC.md §5.2) — tuning history lives
// ABOUTME: in git. When changing one, record the measurement in the commit message.

use core::time::Duration;

pub const LISTEN_PORT: u16 = 8183;
pub const DEVICE_TIMEOUT: Duration = Duration::from_secs(90);
pub const SCAN_INTERVAL_UNITS: u16 = 64; // x 0.625ms = 40ms  (§9)
pub const SCAN_WINDOW_UNITS: u16 = 48; // x 0.625ms = 30ms  (§9)
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
