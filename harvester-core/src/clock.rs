// ABOUTME: Monotonic time newtype and the injected Clock trait — the only way
// ABOUTME: time enters harvester-core (SPEC.md §6.1, §11.3).

/// Milliseconds since boot. Never steps, never goes backwards.
/// `std::time::Instant` does not exist in `no_std`, so core owns its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Millis(pub u64);

impl Millis {
    pub const fn from_secs(secs: u64) -> Self {
        Self(secs.saturating_mul(1000))
    }

    pub const fn as_secs(self) -> u64 {
        self.0.wrapping_div(1000)
    }

    pub const fn saturating_add(self, other: Self) -> Self {
        Self(self.0.saturating_add(other.0))
    }

    /// Elapsed time since `earlier`. Saturates to zero rather than underflowing,
    /// so a race between reader and writer can never produce a huge bogus age.
    pub const fn since(self, earlier: Self) -> Self {
        Self(self.0.saturating_sub(earlier.0))
    }
}

/// Two methods, deliberately (§11.3): staleness logic uses `monotonic` only, so
/// an NTP step can never make a device look spuriously stale or fresh, and
/// `unix_seconds() == None` is exactly the condition for omitting
/// `ble_sensor_last_update_timestamp_seconds` while reporting `time_synced 0`.
pub trait Clock {
    fn monotonic(&self) -> Millis;
    fn unix_seconds(&self) -> Option<u64>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn since_saturates_instead_of_underflowing() {
        assert_eq!(Millis(5).since(Millis(9)), Millis(0));
        assert_eq!(Millis(90_000).since(Millis(30_000)), Millis(60_000));
    }

    #[test]
    fn seconds_round_trip() {
        assert_eq!(Millis::from_secs(90), Millis(90_000));
        assert_eq!(Millis(90_999).as_secs(), 90);
        assert_eq!(Millis::from_secs(u64::MAX).0, u64::MAX); // saturates, no panic
    }
}
