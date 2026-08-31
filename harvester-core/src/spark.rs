// ABOUTME: On-device sparkline history ring — MAX_DEVICES x SPARK_DEPTH x 2 metrics
// ABOUTME: of i16, with i16::MIN as the missing-sample sentinel (§18: -327.68 °C is unreachable).

use crate::{MAX_DEVICES, SPARK_DEPTH};

/// Missing-sample sentinel (§18). Covers both "slot not yet filled" (fresh
/// boot) and "device offline at sample time". Unreachable as a real reading:
/// it would mean −327.68 °C. A zero-filled ring would instead draw six hours
/// of 0 °C through an outage — exactly the misreading §3's gap model prevents.
pub const NO_SAMPLE: i16 = i16::MIN;

/// Fixed-capacity ring of sparkline samples, one column per
/// `SPARK_SAMPLE_INTERVAL`, per device. ~2.3 KB total (§6.2 state budget).
/// `Clone` is the snapshot: copy under SPARK, release, render outside.
#[derive(Clone)]
pub struct Spark {
    temperature: [[i16; SPARK_DEPTH]; MAX_DEVICES],
    humidity: [[i16; SPARK_DEPTH]; MAX_DEVICES],
    /// Next write position per device; also the oldest sample when reading.
    cursor: [usize; MAX_DEVICES],
}

impl Spark {
    pub const fn new() -> Self {
        Self {
            temperature: [[NO_SAMPLE; SPARK_DEPTH]; MAX_DEVICES],
            humidity: [[NO_SAMPLE; SPARK_DEPTH]; MAX_DEVICES],
            cursor: [0; MAX_DEVICES],
        }
    }

    /// Append one sample column for a device. `None` records `NO_SAMPLE`.
    /// A `device` at or beyond `MAX_DEVICES` is silently ignored.
    pub fn push(&mut self, device: usize, temp_centi: Option<i16>, hum_centi: Option<i16>) {
        let Some(cursor) = self.cursor.get_mut(device) else {
            return;
        };
        let at = *cursor;
        // A pushed Some(i16::MIN) is already NO_SAMPLE; unwrap_or covers both.
        if let Some(slot) = self.temperature.get_mut(device).and_then(|r| r.get_mut(at)) {
            *slot = temp_centi.unwrap_or(NO_SAMPLE);
        }
        if let Some(slot) = self.humidity.get_mut(device).and_then(|r| r.get_mut(at)) {
            *slot = hum_centi.unwrap_or(NO_SAMPLE);
        }
        let next = at.wrapping_add(1);
        *cursor = if next >= SPARK_DEPTH { 0 } else { next };
    }

    /// Temperature history, oldest → newest, always exactly `SPARK_DEPTH`
    /// items; sentinel slots yield `None`, as does an out-of-range `device`.
    pub fn temperature(&self, device: usize) -> impl Iterator<Item = Option<i16>> + '_ {
        series(self.temperature.get(device), self.oldest(device))
    }

    /// Humidity history; same shape and contract as [`Spark::temperature`].
    pub fn humidity(&self, device: usize) -> impl Iterator<Item = Option<i16>> + '_ {
        series(self.humidity.get(device), self.oldest(device))
    }

    fn oldest(&self, device: usize) -> usize {
        self.cursor.get(device).copied().unwrap_or(0)
    }
}

impl Default for Spark {
    fn default() -> Self {
        Self::new()
    }
}

/// Walk one device's ring oldest → newest. `None` for `row` (out-of-range
/// device) still yields exactly `SPARK_DEPTH` items, all `None`.
fn series<'a>(
    row: Option<&'a [i16; SPARK_DEPTH]>,
    oldest: usize,
) -> impl Iterator<Item = Option<i16>> + 'a {
    (0..SPARK_DEPTH).map(move |offset| {
        let row = row?;
        // Both operands are < SPARK_DEPTH, so one conditional fold-back
        // replaces a modulo and keeps the arithmetic lint trivially happy.
        let idx = oldest.wrapping_add(offset);
        let idx = if idx >= SPARK_DEPTH {
            idx.wrapping_sub(SPARK_DEPTH)
        } else {
            idx
        };
        row.get(idx).copied().filter(|&v| v != NO_SAMPLE)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn collect(it: impl Iterator<Item = Option<i16>>) -> [Option<i16>; SPARK_DEPTH] {
        let mut out = [None; SPARK_DEPTH];
        let mut n = 0usize;
        for v in it {
            if let Some(slot) = out.get_mut(n) {
                *slot = v;
            }
            n = n.wrapping_add(1);
        }
        assert_eq!(n, SPARK_DEPTH, "iterator must yield exactly SPARK_DEPTH");
        out
    }

    fn at(cols: &[Option<i16>; SPARK_DEPTH], i: usize) -> Option<i16> {
        cols.get(i).copied().flatten()
    }

    #[test]
    fn fresh_ring_is_all_none_for_both_metrics() {
        let s = Spark::new();
        assert!(collect(s.temperature(0)).iter().all(Option::is_none));
        assert!(collect(s.humidity(0)).iter().all(Option::is_none));
    }

    #[test]
    fn three_pushes_land_at_the_newest_end_in_order() {
        let mut s = Spark::new();
        s.push(0, Some(2100), Some(4500));
        s.push(0, Some(2150), Some(4550));
        s.push(0, Some(2200), Some(4600));

        let t = collect(s.temperature(0));
        assert!(t.iter().take(SPARK_DEPTH - 3).all(Option::is_none));
        assert_eq!(at(&t, SPARK_DEPTH - 3), Some(2100));
        assert_eq!(at(&t, SPARK_DEPTH - 2), Some(2150));
        assert_eq!(at(&t, SPARK_DEPTH - 1), Some(2200));

        let h = collect(s.humidity(0));
        assert_eq!(at(&h, SPARK_DEPTH - 3), Some(4500));
        assert_eq!(at(&h, SPARK_DEPTH - 1), Some(4600));
    }

    #[test]
    fn overfilling_evicts_the_oldest_samples() {
        let mut s = Spark::new();
        for i in 0..(SPARK_DEPTH.wrapping_add(5)) {
            let v = i as i16;
            s.push(0, Some(v), Some(v.wrapping_add(1000)));
        }

        let t = collect(s.temperature(0));
        assert!(t.iter().all(Option::is_some), "ring is full: no gaps");
        assert_eq!(at(&t, 0), Some(5), "oldest five evicted");
        assert_eq!(at(&t, SPARK_DEPTH - 1), Some(76), "newest survives");

        let h = collect(s.humidity(0));
        assert_eq!(at(&h, 0), Some(1005));
        assert_eq!(at(&h, SPARK_DEPTH - 1), Some(1076));
    }

    #[test]
    fn metrics_are_independent_within_a_column() {
        let mut s = Spark::new();
        s.push(0, None, Some(4321));

        let t = collect(s.temperature(0));
        let h = collect(s.humidity(0));
        assert_eq!(at(&t, SPARK_DEPTH - 1), None);
        assert_eq!(at(&h, SPARK_DEPTH - 1), Some(4321));
    }

    #[test]
    fn out_of_range_device_is_ignored_and_reads_all_none() {
        let mut s = Spark::new();
        s.push(MAX_DEVICES, Some(2100), Some(4500));

        let t = collect(s.temperature(MAX_DEVICES));
        assert!(t.iter().all(Option::is_none));
        let h = collect(s.humidity(MAX_DEVICES));
        assert!(h.iter().all(Option::is_none));
        // And nothing leaked into a real device's ring.
        assert!(collect(s.temperature(0)).iter().all(Option::is_none));
    }

    #[test]
    fn devices_are_isolated_from_each_other() {
        let mut s = Spark::new();
        s.push(0, Some(2100), Some(4500));

        assert!(collect(s.temperature(1)).iter().all(Option::is_none));
        assert!(collect(s.humidity(1)).iter().all(Option::is_none));
        assert_eq!(at(&collect(s.temperature(0)), SPARK_DEPTH - 1), Some(2100));
    }
}
