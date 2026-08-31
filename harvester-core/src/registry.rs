// ABOUTME: Per-device state, staleness/gap logic and every counter — the Hot
// ABOUTME: region of §6.2. Gaps are a gauge idiom (§3); counters never gap.

use crate::parse::{Parsed, Readings};
use crate::{Device, Millis, Tunables, MAX_DEVICES};

/// Trailing-60 s beacons/min window: 60 one-second buckets advanced by the
/// 1 Hz sweep (§18).
const BPM_BUCKETS: usize = 60;

/// Log-once latch capacity (§7.1). Once full, novel object ids are still
/// counted but never logged — losing a log line is acceptable, the counter is
/// the signal.
const LATCH_CAPACITY: usize = 16;

/// Why a beacon from a configured device was unintelligible (§7.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BeaconFailure {
    Decrypt,
    Parse,
}

/// Per-device `beacons_total{result}` counters. NEVER cleared by expiry (§3):
/// removing a counter series on timeout would make `rate()` read a reset.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DeviceCounters {
    pub ok: u32,
    pub decrypt_fail: u32,
    pub parse_fail: u32,
}

/// Snapshot of one device for rendering. Gauge-feeding fields (`readings`,
/// `rssi_dbm`, `last_seen`) are cleared once offline — the §3 gap. `counters`
/// and `beacons_per_minute` survive expiry.
#[derive(Debug, Clone, Copy)]
pub struct DeviceView<'a> {
    pub device: &'a Device,
    pub readings: Readings,
    pub rssi_dbm: Option<i8>,
    pub online: bool,
    pub last_seen: Option<Millis>,
    /// Last successful sighting, NEVER cleared by expiry — the dashboard's
    /// "OFFLINE 6m" age (§18). Deliberately separate from `last_seen`, which
    /// feeds the gapping timestamp metric and must vanish with the device.
    pub last_sighting: Option<Millis>,
    pub counters: DeviceCounters,
    pub beacons_per_minute: u16,
}

#[derive(Debug, Clone, Copy)]
struct DeviceState {
    readings: Readings,
    rssi_dbm: Option<i8>,
    last_seen: Option<Millis>,
    last_sighting: Option<Millis>,
    online: bool,
    counters: DeviceCounters,
    buckets: [u8; BPM_BUCKETS],
    cursor: u8,
}

impl DeviceState {
    const fn new() -> Self {
        Self {
            readings: Readings {
                temperature_centi: None,
                humidity_centi: None,
                battery_percent: None,
                voltage_milli: None,
            },
            rssi_dbm: None,
            last_seen: None,
            last_sighting: None,
            online: false,
            counters: DeviceCounters {
                ok: 0,
                decrypt_fail: 0,
                parse_fail: 0,
            },
            buckets: [0; BPM_BUCKETS],
            cursor: 0,
        }
    }

    /// Count one received beacon — any outcome. beacons/min measures radio
    /// reception, not parse success (§18).
    fn bump_bucket(&mut self) {
        if let Some(bucket) = self.buckets.get_mut(usize::from(self.cursor)) {
            *bucket = bucket.saturating_add(1);
        }
    }

    /// One second has passed: move to the next bucket and zero it, dropping
    /// whatever was counted there 60 seconds ago.
    fn advance_bucket(&mut self) {
        let next = usize::from(self.cursor).saturating_add(1);
        self.cursor = if next >= BPM_BUCKETS {
            0
        } else {
            self.cursor.saturating_add(1)
        };
        if let Some(bucket) = self.buckets.get_mut(usize::from(self.cursor)) {
            *bucket = 0;
        }
    }

    fn beacons_per_minute(&self) -> u16 {
        // 60 × u8 tops out at 15300, well inside u16.
        self.buckets
            .iter()
            .fold(0u16, |sum, &b| sum.saturating_add(u16::from(b)))
    }

    /// The §3 gap: gauges vanish, counters and the bpm ring stay.
    fn expire(&mut self) {
        self.readings = Readings::default();
        self.rssi_dbm = None;
        self.last_seen = None;
        self.online = false;
    }
}

/// The §6.2 `Hot` region: per-device readings, staleness, counters and the
/// beacons/min rings, plus the crate-global beacon counters. `Clone` is the
/// §6.2 rule-3 snapshot: copy ~1 KB under HOT, release, render outside.
#[derive(Clone)]
pub struct Registry {
    devices: &'static [Device],
    states: [DeviceState; MAX_DEVICES],
    tunables: Tunables,
    unknown_adverts: u64,
    parse_unknown_object: u32,
    latch: [(u8, u8); LATCH_CAPACITY],
    latch_len: usize,
    /// Watermark for the wedge detector: never erased by expiry, unlike the
    /// per-device `last_seen` it shadows.
    newest_seen: Option<Millis>,
}

impl Registry {
    pub fn new(devices: &'static [Device], tunables: Tunables) -> Self {
        let count = devices.len().min(MAX_DEVICES);
        let devices = devices.get(..count).unwrap_or(&[]);
        Self {
            devices,
            states: [DeviceState::new(); MAX_DEVICES],
            tunables,
            unknown_adverts: 0,
            parse_unknown_object: 0,
            latch: [(0, 0); LATCH_CAPACITY],
            latch_len: 0,
            newest_seen: None,
        }
    }

    /// `mac` in human-readable order (caller reverses NimBLE's LE order).
    pub fn device_index(&self, mac: &[u8; 6]) -> Option<usize> {
        self.devices.iter().position(|d| &d.mac == mac)
    }

    /// Record one beacon outcome for the device at `idx`.
    ///
    /// On `Ok`: per-field merge — a `Some` overwrites, a `None` leaves the
    /// existing value, because ATC alternates two disjoint frame shapes and the
    /// voltage frame must not erase temperature (§10). Marks the device online.
    ///
    /// On `Err`: bumps the failure counter and the bpm bucket only. A failed
    /// frame is not a sighting — it must not keep a device that has stopped
    /// producing intelligible data looking alive.
    ///
    /// Returns `true` iff this beacon carried a NOVEL unknown object id for
    /// this device (first time this boot) — the caller logs it once (§7.1).
    pub fn record_beacon(
        &mut self,
        idx: usize,
        now: Millis,
        rssi_dbm: i8,
        outcome: Result<Parsed, BeaconFailure>,
    ) -> bool {
        if idx >= self.devices.len() {
            return false;
        }
        let Some(state) = self.states.get_mut(idx) else {
            return false;
        };
        state.bump_bucket();
        // The wedge watermark advances on EVERY outcome: §15.1's "silent" means
        // no frames arriving at all. A decrypt- or parse-failing beacon is not
        // a sighting, but it proves the radio path delivers end-to-end, and a
        // wedge reboot would not fix a wrong bindkey.
        self.newest_seen = Some(match self.newest_seen {
            Some(watermark) if watermark > now => watermark,
            _ => now,
        });
        match outcome {
            Ok(parsed) => {
                state.counters.ok = state.counters.ok.saturating_add(1);
                let merged = &mut state.readings;
                let fresh = parsed.readings;
                merged.temperature_centi = fresh.temperature_centi.or(merged.temperature_centi);
                merged.humidity_centi = fresh.humidity_centi.or(merged.humidity_centi);
                merged.battery_percent = fresh.battery_percent.or(merged.battery_percent);
                merged.voltage_milli = fresh.voltage_milli.or(merged.voltage_milli);
                state.rssi_dbm = Some(rssi_dbm);
                state.last_seen = Some(now);
                state.last_sighting = Some(now);
                state.online = true;
                if let Some(id) = parsed.unknown_object {
                    self.parse_unknown_object = self.parse_unknown_object.saturating_add(1);
                    // idx < devices.len() <= MAX_DEVICES, so it fits in u8.
                    return self.latch_novel(idx as u8, id);
                }
                false
            }
            Err(BeaconFailure::Decrypt) => {
                state.counters.decrypt_fail = state.counters.decrypt_fail.saturating_add(1);
                false
            }
            Err(BeaconFailure::Parse) => {
                state.counters.parse_fail = state.counters.parse_fail.saturating_add(1);
                false
            }
        }
    }

    /// An advert from a MAC not in the device list — the radio-liveness
    /// heartbeat (§15.1). `u64` because a `u32` wraps within the device's
    /// lifetime and `increase()` reads a wrap as a counter reset (§11.2).
    pub fn record_unknown_advert(&mut self) {
        self.unknown_adverts = self.unknown_adverts.saturating_add(1);
    }

    /// 1 Hz housekeeping: expire devices past `device_timeout` (the §3 gap)
    /// and advance every bpm ring by one second.
    pub fn sweep(&mut self, now: Millis) {
        for state in self.states.iter_mut().take(self.devices.len()) {
            if let Some(seen) = state.last_seen {
                if now.since(seen) >= self.tunables.device_timeout {
                    state.expire();
                }
            }
            state.advance_bucket();
        }
    }

    /// Most recent instant ANY beacon from a configured device arrived,
    /// regardless of outcome — the wedge detector's input (§15.1: "silent"
    /// means nothing arriving, and failed frames still prove radio delivery).
    /// An internal watermark, deliberately separate from the per-device
    /// `last_seen`: expiry clears the rendered value without blinding the
    /// detector, and failures feed this without counting as sightings.
    pub fn newest_last_seen(&self) -> Option<Millis> {
        self.newest_seen
    }

    /// §12 `/status`: how many configured devices are currently online.
    pub fn devices_active(&self) -> usize {
        self.states
            .iter()
            .take(self.devices.len())
            .filter(|s| s.online)
            .count()
    }

    pub fn view(&self, idx: usize) -> Option<DeviceView<'_>> {
        let device = self.devices.get(idx)?;
        let state = self.states.get(idx)?;
        Some(DeviceView {
            device,
            readings: state.readings,
            rssi_dbm: state.rssi_dbm,
            online: state.online,
            last_seen: state.last_seen,
            last_sighting: state.last_sighting,
            counters: state.counters,
            beacons_per_minute: state.beacons_per_minute(),
        })
    }

    /// Configured device count.
    pub fn len(&self) -> usize {
        self.devices.len()
    }

    pub fn is_empty(&self) -> bool {
        self.devices.is_empty()
    }

    pub fn unknown_adverts_total(&self) -> u64 {
        self.unknown_adverts
    }

    pub fn parse_unknown_object_total(&self) -> u32 {
        self.parse_unknown_object
    }

    /// Log-once latch (§7.1): `true` iff `(device, id)` is new this boot AND a
    /// latch slot was free. A full latch returns `false` for genuinely novel
    /// ids — counted, never logged.
    fn latch_novel(&mut self, device: u8, id: u8) -> bool {
        let seen = self
            .latch
            .iter()
            .take(self.latch_len)
            .any(|&entry| entry == (device, id));
        if seen {
            return false;
        }
        if let Some(slot) = self.latch.get_mut(self.latch_len) {
            *slot = (device, id);
            self.latch_len = self.latch_len.saturating_add(1);
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    static DEVICES: [Device; 2] = [
        Device::plain([0xAA, 0xBB, 0xCC, 0x00, 0x00, 0x01], "baby_room"),
        Device::plain([0xAA, 0xBB, 0xCC, 0x00, 0x00, 0x02], "freezer"),
    ];
    static NO_DEVICES: [Device; 0] = [];

    const TUNABLES: Tunables = Tunables {
        device_timeout: Millis(90_000),
        wedge_window: Millis(600_000),
        spark_sample_interval: Millis(300_000),
    };

    fn registry() -> Registry {
        Registry::new(&DEVICES, TUNABLES)
    }

    fn view(r: &Registry, idx: usize) -> DeviceView<'_> {
        match r.view(idx) {
            Some(v) => v,
            None => unreachable!("configured device index must have a view"),
        }
    }

    /// The two disjoint ATC frame shapes (§10).
    fn temp_hum_batt() -> Result<Parsed, BeaconFailure> {
        Ok(Parsed {
            readings: Readings {
                temperature_centi: Some(2365),
                humidity_centi: Some(3815),
                battery_percent: Some(48),
                voltage_milli: None,
            },
            unknown_object: None,
        })
    }

    fn voltage_only() -> Result<Parsed, BeaconFailure> {
        Ok(Parsed {
            readings: Readings {
                temperature_centi: None,
                humidity_centi: None,
                battery_percent: None,
                voltage_milli: Some(2587),
            },
            unknown_object: None,
        })
    }

    fn unknown_object(id: u8) -> Result<Parsed, BeaconFailure> {
        Ok(Parsed {
            readings: Readings::default(),
            unknown_object: Some(id),
        })
    }

    #[test]
    fn two_shape_merge_voltage_does_not_erase_temperature() {
        let mut r = registry();
        r.record_beacon(0, Millis(1_000), -60, temp_hum_batt());
        r.record_beacon(0, Millis(3_000), -61, voltage_only());
        let v = view(&r, 0);
        assert_eq!(v.readings.temperature_centi, Some(2365));
        assert_eq!(v.readings.humidity_centi, Some(3815));
        assert_eq!(v.readings.battery_percent, Some(48));
        assert_eq!(v.readings.voltage_milli, Some(2587));
        assert_eq!(v.rssi_dbm, Some(-61));
        assert_eq!(v.last_seen, Some(Millis(3_000)));
        assert!(v.online);
        assert_eq!(v.counters.ok, 2);
    }

    #[test]
    fn expiry_gaps_gauges_but_never_counters() {
        let mut r = registry();
        r.record_beacon(0, Millis(1_000), -60, temp_hum_batt());
        r.sweep(Millis(91_000)); // exactly device_timeout later
        let v = view(&r, 0);
        assert_eq!(v.readings, Readings::default());
        assert_eq!(v.rssi_dbm, None);
        assert_eq!(v.last_seen, None);
        assert!(!v.online);
        // Counters never gap (§3), and the bpm ring still holds the beacon.
        assert_eq!(v.counters.ok, 1);
        assert_eq!(v.beacons_per_minute, 1);
        // Expiry must not blind the wedge detector.
        assert_eq!(r.newest_last_seen(), Some(Millis(1_000)));
    }

    #[test]
    fn expiry_boundary_is_at_exactly_device_timeout() {
        let mut r = registry();
        r.record_beacon(0, Millis(1_000), -60, temp_hum_batt());
        r.sweep(Millis(90_999)); // 89.999 s elapsed: still online
        assert!(view(&r, 0).online);
        r.sweep(Millis(91_000)); // 90 s exactly: offline
        assert!(!view(&r, 0).online);
    }

    #[test]
    fn expired_device_comes_back_online_on_next_beacon() {
        let mut r = registry();
        r.record_beacon(0, Millis(1_000), -60, temp_hum_batt());
        r.sweep(Millis(91_000));
        assert!(!view(&r, 0).online);
        r.record_beacon(0, Millis(95_000), -55, voltage_only());
        let v = view(&r, 0);
        assert!(v.online);
        assert_eq!(v.last_seen, Some(Millis(95_000)));
        assert_eq!(v.rssi_dbm, Some(-55));
        assert_eq!(v.readings.voltage_milli, Some(2587));
        // Expiry wiped temperature; the new frame did not carry one.
        assert_eq!(v.readings.temperature_centi, None);
        assert_eq!(r.newest_last_seen(), Some(Millis(95_000)));
    }

    #[test]
    fn failed_beacon_is_not_a_sighting() {
        let mut r = registry();
        r.record_beacon(0, Millis(1_000), -60, Err(BeaconFailure::Decrypt));
        r.record_beacon(0, Millis(2_000), -60, Err(BeaconFailure::Parse));
        let v = view(&r, 0);
        assert!(!v.online);
        assert_eq!(v.last_seen, None);
        assert_eq!(v.readings, Readings::default());
        assert_eq!(v.rssi_dbm, None);
        assert_eq!(v.counters.decrypt_fail, 1);
        assert_eq!(v.counters.parse_fail, 1);
        // Every outcome counts toward reception (§18)...
        assert_eq!(v.beacons_per_minute, 2);
        // ...and feeds the wedge watermark: §15.1's "silent" means nothing
        // arriving at all, and a failed frame still proves radio delivery —
        // a wedge reboot would not fix a wrong bindkey.
        assert_eq!(r.newest_last_seen(), Some(Millis(2_000)));
        assert_eq!(r.devices_active(), 0);
    }

    #[test]
    fn beacons_per_minute_is_an_exact_trailing_60s_count() {
        let mut r = registry();
        for _ in 0..5 {
            r.record_beacon(0, Millis(1_000), -60, temp_hum_batt());
        }
        assert_eq!(view(&r, 0).beacons_per_minute, 5);
        for s in 0..59u64 {
            r.sweep(Millis(2_000).saturating_add(Millis::from_secs(s)));
            assert_eq!(view(&r, 0).beacons_per_minute, 5);
        }
        // 60th sweep: the bucket holding the 5 falls out of the window.
        r.sweep(Millis(61_000));
        assert_eq!(view(&r, 0).beacons_per_minute, 0);
    }

    #[test]
    fn beacons_per_minute_bucket_saturates_instead_of_wrapping() {
        let mut r = registry();
        for _ in 0..300 {
            r.record_beacon(0, Millis(1_000), -60, temp_hum_batt());
        }
        assert_eq!(view(&r, 0).beacons_per_minute, 255);
    }

    #[test]
    fn novel_object_latch_is_per_device_and_id() {
        let mut r = registry();
        assert!(r.record_beacon(0, Millis(1_000), -60, unknown_object(0xF0)));
        assert!(!r.record_beacon(0, Millis(2_000), -60, unknown_object(0xF0)));
        assert!(r.record_beacon(0, Millis(3_000), -60, unknown_object(0xF1)));
        assert!(r.record_beacon(1, Millis(4_000), -60, unknown_object(0xF0)));
        // Counter climbs once per occurrence, latched or not.
        assert_eq!(r.parse_unknown_object_total(), 4);
    }

    #[test]
    fn full_latch_counts_but_never_logs() {
        let mut r = registry();
        for id in 0x80..0x90u8 {
            assert!(r.record_beacon(0, Millis(1_000), -60, unknown_object(id)));
        }
        // 17th distinct entry: latch full — counted, not logged.
        assert!(!r.record_beacon(0, Millis(2_000), -60, unknown_object(0x90)));
        assert_eq!(r.parse_unknown_object_total(), 17);
    }

    #[test]
    fn device_index_finds_exact_mac_only() {
        let r = registry();
        assert_eq!(
            r.device_index(&[0xAA, 0xBB, 0xCC, 0x00, 0x00, 0x02]),
            Some(1)
        );
        assert_eq!(
            r.device_index(&[0xAA, 0xBB, 0xCC, 0x00, 0x00, 0x01]),
            Some(0)
        );
        assert_eq!(r.device_index(&[0xDE, 0xAD, 0xBE, 0xEF, 0x00, 0x00]), None);
    }

    #[test]
    fn devices_active_counts_only_online() {
        let mut r = registry();
        assert_eq!(r.devices_active(), 0);
        r.record_beacon(0, Millis(1_000), -60, temp_hum_batt());
        assert_eq!(r.devices_active(), 1);
        r.record_beacon(1, Millis(2_000), -70, voltage_only());
        assert_eq!(r.devices_active(), 2);
        r.sweep(Millis(91_000)); // device 0 expires; device 1 has 89 s left
        assert_eq!(r.devices_active(), 1);
    }

    #[test]
    fn unknown_adverts_total_climbs_as_u64() {
        let mut r = registry();
        r.record_unknown_advert();
        r.record_unknown_advert();
        assert_eq!(r.unknown_adverts_total(), 2u64);
    }

    #[test]
    fn len_is_configured_count_and_views_stop_there() {
        let r = registry();
        assert_eq!(r.len(), 2);
        assert!(!r.is_empty());
        assert!(r.view(1).is_some());
        assert!(r.view(2).is_none());
        assert_eq!(view(&r, 0).device.name, "baby_room");

        let empty = Registry::new(&NO_DEVICES, TUNABLES);
        assert_eq!(empty.len(), 0);
        assert!(empty.is_empty());
        assert!(empty.view(0).is_none());
    }

    #[test]
    fn hot_region_stays_near_the_1kb_budget() {
        // §6.2 budgets ~0.8 KB for Hot on the 32-bit target; allow padding
        // slack for the 64-bit host this test runs on.
        assert!(core::mem::size_of::<Registry>() <= 1_200);
    }
}
