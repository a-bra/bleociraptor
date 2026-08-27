// ABOUTME: In-RAM log ring (§16) — LOG_LINES fixed slots of LOG_LINE_BYTES, no
// ABOUTME: allocation; absolute stamps derived at render time per §11.3.

use crate::{Millis, LOG_LINES, LOG_LINE_BYTES};

/// Unix seconds → UTC civil (year, month, day, hour, minute, second).
/// Checked arithmetic throughout; `None` is unreachable for valid timestamps
/// but keeps the math provably panic-free.
fn civil_datetime(secs: u64) -> Option<(i64, u32, u32, u64, u64, u64)> {
    let days = i64::try_from(secs.checked_div(86_400)?).ok()?;
    let sod = secs.checked_rem(86_400)?;
    let hour = sod.checked_div(3_600)?;
    let minute = sod.checked_rem(3_600)?.checked_div(60)?;
    let second = sod.checked_rem(60)?;
    let (year, month, day) = civil_from_days(days)?;
    Some((year, month, day, hour, minute, second))
}

/// Days since 1970-01-01 → (year, month, day). Howard Hinnant's
/// `civil_from_days`, transcribed onto checked integer ops.
fn civil_from_days(days: i64) -> Option<(i64, u32, u32)> {
    let z = days.checked_add(719_468)?;
    let era = z.checked_div_euclid(146_097)?;
    let doe = z.checked_rem_euclid(146_097)?; // day of era, [0, 146096]
    let yoe = doe
        .checked_sub(doe.checked_div(1_460)?)?
        .checked_add(doe.checked_div(36_524)?)?
        .checked_sub(doe.checked_div(146_096)?)?
        .checked_div(365)?; // year of era, [0, 399]
    let y = yoe.checked_add(era.checked_mul(400)?)?;
    let doy = doe.checked_sub(
        yoe.checked_mul(365)?
            .checked_add(yoe.checked_div(4)?)?
            .checked_sub(yoe.checked_div(100)?)?,
    )?; // day of year (March-based), [0, 365]
    let mp = doy.checked_mul(5)?.checked_add(2)?.checked_div(153)?; // month, March=0
    let d = doy
        .checked_sub(mp.checked_mul(153)?.checked_add(2)?.checked_div(5)?)?
        .checked_add(1)?;
    let m = if mp < 10 {
        mp.checked_add(3)?
    } else {
        mp.checked_sub(9)?
    };
    let year = if m <= 2 { y.checked_add(1)? } else { y };
    Some((year, u32::try_from(m).ok()?, u32::try_from(d).ok()?))
}

/// `core::fmt::Write` sink over one fixed slot. Truncates silently at the slot
/// boundary, on a char boundary, so slot contents stay valid UTF-8.
struct SlotWriter<'a> {
    buf: &'a mut [u8],
    pos: usize,
}

impl core::fmt::Write for SlotWriter<'_> {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        let remaining = self.buf.len().saturating_sub(self.pos);
        let n = if s.len() <= remaining {
            s.len()
        } else {
            let mut n = remaining;
            while n > 0 && !s.is_char_boundary(n) {
                n = n.saturating_sub(1);
            }
            n
        };
        let end = self.pos.saturating_add(n);
        if let (Some(dst), Some(src)) = (self.buf.get_mut(self.pos..end), s.as_bytes().get(..n)) {
            dst.copy_from_slice(src);
            self.pos = end;
        }
        Ok(()) // truncation is not an error; returning Err would abort fmt mid-line
    }
}

pub struct LogRing {
    slots: [[u8; LOG_LINE_BYTES]; LOG_LINES],
    lens: [u8; LOG_LINES],
    stamps: [Millis; LOG_LINES],
    /// Slot the next push writes into.
    next: usize,
    /// Valid lines, saturating at `LOG_LINES`.
    count: usize,
}

impl LogRing {
    pub const fn new() -> Self {
        Self {
            slots: [[0; LOG_LINE_BYTES]; LOG_LINES],
            lens: [0; LOG_LINES],
            stamps: [Millis(0); LOG_LINES],
            next: 0,
            count: 0,
        }
    }

    /// Compose a line into the next slot. Message truncated to fit the slot.
    /// Only the monotonic push instant is stored — never a wall-clock stamp
    /// (§11.3); the absolute time is derived in `render`.
    pub fn push(&mut self, now: Millis, args: core::fmt::Arguments<'_>) {
        let idx = self.next;
        let Some(slot) = self.slots.get_mut(idx) else {
            return; // unreachable: next is always < LOG_LINES
        };
        let mut writer = SlotWriter { buf: slot, pos: 0 };
        let _ = core::fmt::write(&mut writer, args); // SlotWriter never errors
        let used = writer.pos;
        if let Some(len) = self.lens.get_mut(idx) {
            *len = u8::try_from(used).unwrap_or(u8::MAX); // used <= LOG_LINE_BYTES = 96
        }
        if let Some(stamp) = self.stamps.get_mut(idx) {
            *stamp = now;
        }
        let advanced = idx.saturating_add(1);
        self.next = if advanced >= LOG_LINES { 0 } else { advanced };
        self.count = self.count.saturating_add(1).min(LOG_LINES);
    }

    /// Newest first. `unix_now` = current wall clock if SNTP has synced.
    ///
    /// Absolute stamp per line = `unix_now - (now - line_millis)` in seconds
    /// (§11.3): derived here, at render time, so a line pushed before SNTP
    /// synced still gets a correct wall-clock stamp afterwards. Without a wall
    /// clock the line falls back to `[+Ns]`, seconds since boot at push.
    pub fn render<W: core::fmt::Write>(
        &self,
        w: &mut W,
        now: Millis,
        unix_now: Option<u64>,
    ) -> core::fmt::Result {
        let mut idx = self.next;
        for _ in 0..self.count {
            idx = match idx.checked_sub(1) {
                Some(prev) => prev,
                None => LOG_LINES.saturating_sub(1),
            };
            let Some((msg, stamp)) = self.line(idx) else {
                continue; // unreachable: idx is always < LOG_LINES
            };
            let age_secs = now.since(stamp).as_secs();
            let civil = unix_now
                .and_then(|t| t.checked_sub(age_secs))
                .and_then(civil_datetime);
            match civil {
                Some((year, month, day, hour, minute, second)) => write!(
                    w,
                    "[{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}:{second:02}] "
                )?,
                None => write!(w, "[+{}s] ", stamp.as_secs())?,
            }
            if let Ok(text) = core::str::from_utf8(msg) {
                w.write_str(text)?;
            }
            w.write_str("\n")?;
        }
        Ok(())
    }

    /// Message bytes and monotonic push instant of the line in `idx`.
    fn line(&self, idx: usize) -> Option<(&[u8], Millis)> {
        let slot = self.slots.get(idx)?;
        let len = usize::from(*self.lens.get(idx)?);
        let stamp = *self.stamps.get(idx)?;
        Some((slot.get(..len)?, stamp))
    }
}

impl Default for LogRing {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Test sink: fixed buffer + `fmt::Write`, so tests stay no_std like the crate.
    struct Sink {
        buf: [u8; 8192],
        len: usize,
    }

    impl Sink {
        fn new() -> Self {
            Self {
                buf: [0; 8192],
                len: 0,
            }
        }

        fn as_str(&self) -> &str {
            core::str::from_utf8(self.buf.get(..self.len).unwrap_or(b"")).unwrap_or("<bad utf8>")
        }
    }

    impl core::fmt::Write for Sink {
        fn write_str(&mut self, s: &str) -> core::fmt::Result {
            let end = self.len.saturating_add(s.len());
            let dst = self.buf.get_mut(self.len..end).ok_or(core::fmt::Error)?;
            dst.copy_from_slice(s.as_bytes());
            self.len = end;
            Ok(())
        }
    }

    fn rendered(ring: &LogRing, now: Millis, unix_now: Option<u64>) -> Sink {
        let mut sink = Sink::new();
        assert!(ring.render(&mut sink, now, unix_now).is_ok());
        sink
    }

    // Civil-date vectors verified independently with `date -u -d @<epoch>`.

    #[test]
    fn civil_epoch_zero_is_1970_01_01_midnight() {
        assert_eq!(civil_datetime(0), Some((1970, 1, 1, 0, 0, 0)));
    }

    #[test]
    fn civil_last_second_of_first_day() {
        assert_eq!(civil_datetime(86_399), Some((1970, 1, 1, 23, 59, 59)));
    }

    #[test]
    fn civil_leap_day_2000() {
        // date -u -d @951782400 → Tue Feb 29 00:00:00 UTC 2000
        assert_eq!(civil_datetime(951_782_400), Some((2000, 2, 29, 0, 0, 0)));
    }

    #[test]
    fn civil_2026_08_27_noon() {
        // date -u -d @1787832000 → Thu Aug 27 12:00:00 UTC 2026
        assert_eq!(civil_datetime(1_787_832_000), Some((2026, 8, 27, 12, 0, 0)));
    }

    #[test]
    fn one_line_synced_renders_exact_string() {
        let mut ring = LogRing::new();
        ring.push(Millis(5_000), format_args!("hello {}", 42));
        let sink = rendered(&ring, Millis(5_000), Some(1_787_832_000));
        assert_eq!(sink.as_str(), "[2026-08-27 12:00:00] hello 42\n");
    }

    #[test]
    fn unsynced_renders_seconds_since_boot_at_push() {
        let mut ring = LogRing::new();
        ring.push(Millis(834_000), format_args!("boot"));
        let sink = rendered(&ring, Millis(900_000), None);
        assert_eq!(sink.as_str(), "[+834s] boot\n");
    }

    #[test]
    fn overflow_drops_oldest_and_serves_newest_first() {
        let mut ring = LogRing::new();
        for i in 0..130u32 {
            ring.push(Millis(u64::from(i)), format_args!("line {i};"));
        }
        let sink = rendered(&ring, Millis(1_000), None);
        let text = sink.as_str();
        assert_eq!(text.lines().count(), LOG_LINES);
        // Newest first: line 129 leads, then 128, ...
        assert!(text.starts_with("[+0s] line 129;"), "got: {}", text);
        assert!(text.contains("line 2;"));
        // Oldest two evicted. Trailing ';' keeps "line 12" from matching "line 12x".
        assert!(!text.contains("line 1;"));
        assert!(!text.contains("line 0;"));
        // Order check: 129 before 128.
        let p129 = text.find("line 129;").unwrap_or(usize::MAX);
        let p128 = text.find("line 128;").unwrap_or(0);
        assert!(p129 < p128);
    }

    #[test]
    fn oversized_message_truncates_without_panic_and_keeps_newline() {
        // 100 chars; formatted twice = a 200-byte message into a 96-byte slot.
        const HUNDRED: &str =
            "0123456789012345678901234567890123456789012345678901234567890123456789012345678901234567890123456789";
        let mut ring = LogRing::new();
        ring.push(Millis(0), format_args!("{HUNDRED}{HUNDRED}"));
        let sink = rendered(&ring, Millis(0), None);
        let text = sink.as_str();
        assert!(text.ends_with('\n'));
        // "[+0s] " (6 bytes) + 96 truncated bytes + "\n" = 103
        assert_eq!(text.len(), 103);
        let expected_msg = HUNDRED.get(..LOG_LINE_BYTES).unwrap_or("");
        assert!(text.contains(expected_msg));
    }

    #[test]
    fn absolute_stamp_is_derived_at_render_time() {
        // Pushed pre-sync at +5s; rendered at +65s with wall clock T. The line
        // is 60s old, so it must be stamped T-60s — never the 1970 push-time.
        let mut ring = LogRing::new();
        ring.push(Millis(5_000), format_args!("early beacon"));
        let sink = rendered(&ring, Millis(65_000), Some(1_787_832_000));
        // date -u -d @1787831940 → Thu Aug 27 11:59:00 UTC 2026
        assert_eq!(sink.as_str(), "[2026-08-27 11:59:00] early beacon\n");
    }

    #[test]
    fn empty_ring_renders_nothing() {
        let ring = LogRing::new();
        let sink = rendered(&ring, Millis(0), Some(1_787_832_000));
        assert_eq!(sink.as_str(), "");
    }
}
