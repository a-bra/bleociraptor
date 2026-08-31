// ABOUTME: The three mutexes (§6.2) — split by access pattern, never held two
// ABOUTME: at once, never formatted under HOT.

use harvester_core::logring::LogRing;
use harvester_core::{Registry, Spark};
use std::sync::Mutex;

/// BLE callback + housekeeping + most endpoints. ~1 KB; snapshot, release, then
/// render (§6.2 rule 3).
pub static HOT: Mutex<Option<Registry>> = Mutex::new(None);

/// Housekeeping writes one column per SPARK_SAMPLE_INTERVAL; /api/history reads.
pub static SPARK: Mutex<Spark> = Mutex::new(Spark::new());

/// Rare failure-path writes; /logs streams under this lock (§6.2 rule 4).
pub static LOGS: Mutex<LogRing> = Mutex::new(LogRing::new());

/// Push one line into the ring. Composes directly into the fixed slot —
/// no allocation, callable from the BLE callback's failure paths.
#[macro_export]
macro_rules! ring_log {
    ($now:expr, $($arg:tt)*) => {{
        if let Ok(mut ring) = $crate::state::LOGS.lock() {
            ring.push($now, format_args!($($arg)*));
        }
    }};
}
