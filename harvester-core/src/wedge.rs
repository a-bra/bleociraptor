// ABOUTME: Radio-wedge verdict (SPEC.md §15.1) — pure decision, no reboot.
// ABOUTME: harvester/effects.rs acts on the verdict; the 1 Hz sweep calls check().

use crate::Millis;

/// Outcome of one wedge check. `NotArmed` until the first advert of any kind
/// arrives since boot — a wedge is a transition from working to not-working,
/// and a stall cannot be detected in something that never started (§15.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WedgeVerdict {
    NotArmed,
    Healthy,
    Wedged,
}

/// Detects a wedged BLE stack from two liveness signals: configured-device
/// beacons and the stray-traffic counter. Wedged iff armed AND both have been
/// flat for `wedge_window` AND no OTA is streaming (§13 suppression).
pub struct WedgeDetector {
    armed: bool,
    /// Watermark: last instant either input changed.
    last_activity: Millis,
    last_unknown_total: u64,
    last_newest_seen: Option<Millis>,
}

impl WedgeDetector {
    pub const fn new() -> Self {
        Self {
            armed: false,
            last_activity: Millis(0),
            last_unknown_total: 0,
            last_newest_seen: None,
        }
    }

    /// Call once per sweep. `newest_last_seen` = the most recent `last_seen`
    /// across all configured devices (None if none ever seen this boot).
    pub fn check(
        &mut self,
        now: Millis,
        newest_last_seen: Option<Millis>,
        unknown_adverts_total: u64,
        ota_in_progress: bool,
        wedge_window: Millis,
    ) -> WedgeVerdict {
        // Any change to either input is activity. Inequality (not ordering)
        // deliberately: a decreasing counter is a bug upstream, but it still
        // proves the radio path executed, and comparing with != cannot
        // underflow the way a subtraction-based delta would.
        let activity = newest_last_seen != self.last_newest_seen
            || unknown_adverts_total != self.last_unknown_total;

        if activity {
            self.armed = true;
            self.last_activity = now;
            self.last_newest_seen = newest_last_seen;
            self.last_unknown_total = unknown_adverts_total;
        }

        if !self.armed {
            return WedgeVerdict::NotArmed;
        }
        // §13: an OTA in progress suppresses the verdict at this call only —
        // the watermark is untouched, so persistent silence fires later.
        if !ota_in_progress && now.since(self.last_activity) >= wedge_window {
            return WedgeVerdict::Wedged;
        }
        WedgeVerdict::Healthy
    }
}

impl Default for WedgeDetector {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WINDOW: Millis = Millis::from_secs(600);

    /// Sweep helper: drive the detector once per second like the housekeeping
    /// thread does, with both inputs frozen, and return the final verdict.
    fn sweep_silence(
        det: &mut WedgeDetector,
        from: Millis,
        to: Millis,
        newest: Option<Millis>,
        unknown: u64,
        ota: bool,
    ) -> WedgeVerdict {
        let mut verdict = WedgeVerdict::NotArmed;
        let mut t = from;
        while t <= to {
            verdict = det.check(t, newest, unknown, ota, WINDOW);
            t = t.saturating_add(Millis::from_secs(1));
        }
        verdict
    }

    #[test]
    fn fresh_boot_total_silence_never_arms_never_wedges() {
        // The boot-loop §15.1 warns about: quiet-RF bench board, no adverts
        // ever. Three full windows of silence must stay NotArmed throughout.
        let mut det = WedgeDetector::new();
        let mut t = Millis(0);
        let end = Millis(WINDOW.0.saturating_mul(3));
        while t <= end {
            assert_eq!(
                det.check(t, None, 0, false, WINDOW),
                WedgeVerdict::NotArmed,
                "at t={t:?}"
            );
            t = t.saturating_add(Millis::from_secs(1));
        }
    }

    #[test]
    fn beacon_then_silence_wedges_at_exactly_window_boundary() {
        let mut det = WedgeDetector::new();
        let beacon_at = Millis::from_secs(10);
        assert_eq!(
            det.check(beacon_at, Some(beacon_at), 0, false, WINDOW),
            WedgeVerdict::Healthy
        );
        // Healthy for the whole open interval before the boundary…
        let just_before = Millis(beacon_at.0.saturating_add(WINDOW.0).saturating_sub(1));
        assert_eq!(
            det.check(just_before, Some(beacon_at), 0, false, WINDOW),
            WedgeVerdict::Healthy
        );
        // …and Wedged at exactly beacon_at + window (>= fires).
        let boundary = beacon_at.saturating_add(WINDOW);
        assert_eq!(
            det.check(boundary, Some(beacon_at), 0, false, WINDOW),
            WedgeVerdict::Wedged
        );
    }

    #[test]
    fn unknown_adverts_alone_arm_the_detector() {
        // No configured beacon ever; unknown_total goes 0→1, then flatlines.
        let mut det = WedgeDetector::new();
        let armed_at = Millis::from_secs(5);
        assert_eq!(
            det.check(armed_at, None, 1, false, WINDOW),
            WedgeVerdict::Healthy
        );
        let boundary = armed_at.saturating_add(WINDOW);
        let verdict = sweep_silence(
            &mut det,
            armed_at.saturating_add(Millis::from_secs(1)),
            boundary,
            None,
            1,
            false,
        );
        assert_eq!(verdict, WedgeVerdict::Wedged);
    }

    #[test]
    fn trickling_unknown_adverts_hold_healthy_with_all_devices_silent() {
        // The AND-condition: stray traffic proves the radio is alive, so dead
        // sensors alone never trigger a reboot verdict.
        let mut det = WedgeDetector::new();
        let mut unknown: u64 = 0;
        let mut t = Millis(0);
        let end = Millis(WINDOW.0.saturating_mul(3));
        while t <= end {
            unknown = unknown.saturating_add(1);
            let verdict = det.check(t, None, unknown, false, WINDOW);
            assert_eq!(verdict, WedgeVerdict::Healthy, "at t={t:?}");
            t = t.saturating_add(Millis::from_secs(1));
        }
    }

    #[test]
    fn steady_configured_beacons_hold_healthy_with_unknown_flat() {
        let mut det = WedgeDetector::new();
        let mut t = Millis::from_secs(1);
        let end = Millis(WINDOW.0.saturating_mul(3));
        while t <= end {
            // newest_last_seen advances every sweep; unknown pinned at 0.
            let verdict = det.check(t, Some(t), 0, false, WINDOW);
            assert_eq!(verdict, WedgeVerdict::Healthy, "at t={t:?}");
            t = t.saturating_add(Millis::from_secs(1));
        }
    }

    #[test]
    fn ota_suppresses_wedge_but_does_not_reset_the_silence_window() {
        let mut det = WedgeDetector::new();
        let beacon_at = Millis::from_secs(10);
        assert_eq!(
            det.check(beacon_at, Some(beacon_at), 0, false, WINDOW),
            WedgeVerdict::Healthy
        );
        // Silence well past the window, but an OTA is streaming: suppressed.
        let past = beacon_at.saturating_add(WINDOW).saturating_add(WINDOW);
        assert_eq!(
            det.check(past, Some(beacon_at), 0, true, WINDOW),
            WedgeVerdict::Healthy
        );
        // OTA ends, silence persists: fires immediately on the next sweep —
        // the OTA did not reset the watermark.
        let next = past.saturating_add(Millis::from_secs(1));
        assert_eq!(
            det.check(next, Some(beacon_at), 0, false, WINDOW),
            WedgeVerdict::Wedged
        );
    }

    #[test]
    fn decreasing_unknown_total_counts_as_activity_without_underflow() {
        // Only possible on a counter bug, but must not panic or underflow —
        // any change to the counter is treated as activity.
        let mut det = WedgeDetector::new();
        let armed_at = Millis::from_secs(5);
        assert_eq!(
            det.check(armed_at, None, 100, false, WINDOW),
            WedgeVerdict::Healthy
        );
        // Just before the window would elapse, the counter goes DOWN.
        let dip_at = Millis(armed_at.0.saturating_add(WINDOW.0).saturating_sub(1));
        assert_eq!(
            det.check(dip_at, None, 40, false, WINDOW),
            WedgeVerdict::Healthy
        );
        // The dip refreshed the watermark: the original boundary stays Healthy…
        let old_boundary = armed_at.saturating_add(WINDOW);
        assert_eq!(
            det.check(old_boundary, None, 40, false, WINDOW),
            WedgeVerdict::Healthy
        );
        // …and the window now runs from the dip.
        let new_boundary = dip_at.saturating_add(WINDOW);
        assert_eq!(
            det.check(new_boundary, None, 40, false, WINDOW),
            WedgeVerdict::Wedged
        );
    }
}
