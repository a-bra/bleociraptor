// ABOUTME: /api/readings JSON body (§12) — current values, health, and a
// ABOUTME: server-computed age_seconds: one clock, one subtraction, no browser skew.

use crate::clock::Millis;
use crate::fixed::{write_centi, write_milli};
use crate::registry::Registry;
use crate::render::{Health, RebootCounts};

/// Writes `s` as a JSON string literal: quote, backslash and control
/// characters escaped. Device names are static strs under our control, but
/// the escaper is cheap insurance for every string we emit.
pub(crate) fn write_json_str<W: core::fmt::Write>(w: &mut W, s: &str) -> core::fmt::Result {
    w.write_char('"')?;
    for c in s.chars() {
        match c {
            '"' => w.write_str("\\\"")?,
            '\\' => w.write_str("\\\\")?,
            c if (c as u32) < 0x20 => write!(w, "\\u{:04x}", c as u32)?,
            c => w.write_char(c)?,
        }
    }
    w.write_char('"')
}

/// Writes a centi-scaled optional as a 2-decimal number, or `null` (§18's
/// sentinel rule: absent is null, never a fake zero).
pub(crate) fn write_centi_or_null<W: core::fmt::Write>(
    w: &mut W,
    value: Option<i32>,
) -> core::fmt::Result {
    match value {
        Some(v) => write_centi(w, v),
        None => w.write_str("null"),
    }
}

/// Writes an integer-rendering optional, or `null`.
fn write_int_or_null<W: core::fmt::Write, T: core::fmt::Display>(
    w: &mut W,
    value: Option<T>,
) -> core::fmt::Result {
    match value {
        Some(v) => write!(w, "{v}"),
        None => w.write_str("null"),
    }
}

/// All seven reboot reasons summed for the dashboard's single figure. `u64`
/// accumulator: seven u32 counters cannot overflow it.
fn reboot_total(reboots: &RebootCounts) -> u64 {
    [
        reboots.panic,
        reboots.wifi,
        reboots.ble_wedge,
        reboots.ota,
        reboots.power,
        reboots.wdt,
        reboots.unknown,
    ]
    .iter()
    .fold(0u64, |sum, &n| sum.saturating_add(u64::from(n)))
}

/// `/api/readings` body: `time_synced`, one entry per configured device (every
/// key always present, absent values null), and the health block.
///
/// `age_seconds` is server-computed from `last_sighting` (§12): deriving it
/// client-side from a timestamp would reintroduce a second clock — the
/// browser's — and a phone with skew renders confidently wrong ages. It keeps
/// counting through an outage (the dashboard's "OFFLINE 6m") and is null only
/// for a device never sighted this boot.
pub fn render_readings<W: core::fmt::Write>(
    w: &mut W,
    registry: &Registry,
    health: &Health,
    now: Millis,
    unix_now: Option<u64>,
) -> core::fmt::Result {
    write!(w, "{{\"time_synced\":{},\"devices\":[", unix_now.is_some())?;
    let mut first = true;
    for idx in 0..registry.len() {
        let Some(view) = registry.view(idx) else {
            continue;
        };
        if !first {
            w.write_char(',')?;
        }
        first = false;
        w.write_str("{\"name\":")?;
        write_json_str(w, view.device.name)?;
        write!(
            w,
            ",\"encrypted\":{},\"online\":{},\"age_seconds\":",
            view.device.is_encrypted(),
            view.online
        )?;
        write_int_or_null(w, view.last_sighting.map(|s| now.since(s).as_secs()))?;
        w.write_str(",\"temperature\":")?;
        write_centi_or_null(w, view.readings.temperature_centi.map(i32::from))?;
        w.write_str(",\"humidity\":")?;
        write_centi_or_null(w, view.readings.humidity_centi.map(i32::from))?;
        w.write_str(",\"battery\":")?;
        write_int_or_null(w, view.readings.battery_percent)?;
        w.write_str(",\"voltage\":")?;
        match view.readings.voltage_milli {
            Some(v) => write_milli(w, i32::from(v))?,
            None => w.write_str("null")?,
        }
        w.write_str(",\"rssi\":")?;
        write_int_or_null(w, view.rssi_dbm)?;
        write!(w, ",\"beacons_per_minute\":{}}}", view.beacons_per_minute)?;
    }
    write!(
        w,
        "],\"health\":{{\"uptime_seconds\":{},\"wifi_rssi\":",
        health.uptime_seconds
    )?;
    write_int_or_null(w, health.wifi_rssi_dbm)?;
    write!(
        w,
        ",\"free_heap_bytes\":{},\"min_free_heap_bytes\":{},\"unknown_adverts\":{},\"reboot_total\":{},\"last_reboot_reason\":",
        health.free_heap_bytes,
        health.min_free_heap_bytes,
        registry.unknown_adverts_total(),
        reboot_total(&health.reboots)
    )?;
    write_json_str(w, health.last_reboot_reason)?;
    w.write_str(",\"version\":")?;
    write_json_str(w, health.version)?;
    w.write_str(",\"git_sha\":")?;
    write_json_str(w, health.git_sha)?;
    w.write_str("}}")
}

#[cfg(test)]
mod tests {
    extern crate std;
    use std::string::String;

    use super::*;
    use crate::parse::{Parsed, Readings};
    use crate::registry::BeaconFailure;
    use crate::render::RebootCounts;
    use crate::{bindkey, mac, Device, Tunables};

    static DEVICES: [Device; 3] = [
        Device::plain(mac!("00:11:22:33:44:01"), "baby_room"),
        Device::encrypted(
            mac!("00:11:22:33:44:02"),
            "freezer",
            bindkey!("00112233445566778899aabbccddeeff"),
        ),
        Device::plain(mac!("00:11:22:33:44:03"), "garage"),
    ];

    const TUNABLES: Tunables = Tunables {
        device_timeout: Millis(90_000),
        wedge_window: Millis(600_000),
        spark_sample_interval: Millis(300_000),
    };

    fn temp_hum_batt() -> Result<Parsed, BeaconFailure> {
        Ok(Parsed {
            readings: Readings {
                temperature_centi: Some(2696),
                humidity_centi: Some(3815),
                battery_percent: Some(92),
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
                voltage_milli: Some(2939),
            },
            unknown_object: None,
        })
    }

    /// One online device with every reading, one expired (offline but sighted
    /// this boot), one never seen this boot.
    fn fixture() -> Registry {
        let mut r = Registry::new(&DEVICES, TUNABLES);
        // freezer: one good frame long ago, then expires at the sweep.
        r.record_beacon(1, Millis(1_000), -80, temp_hum_batt());
        // baby_room: both ATC frame shapes, recent enough to stay online.
        r.record_beacon(0, Millis(80_000), -60, temp_hum_batt());
        r.record_beacon(0, Millis(90_000), -61, voltage_only());
        r.sweep(Millis(91_000)); // freezer is exactly 90 s stale: expires
        r.record_unknown_advert();
        r.record_unknown_advert();
        r
    }

    fn health() -> Health {
        Health {
            uptime_seconds: 3600,
            free_heap_bytes: 180_000,
            min_free_heap_bytes: 150_000,
            wifi_rssi_dbm: Some(-55),
            version: "0.1.0",
            git_sha: "abc1234",
            reboots: RebootCounts {
                panic: 1,
                wifi: 2,
                ble_wedge: 0,
                ota: 1,
                power: 3,
                wdt: 0,
                unknown: 0,
            },
            last_reboot_reason: "power",
        }
    }

    fn render(registry: &Registry, unix_now: Option<u64>) -> String {
        let mut s = String::new();
        assert!(render_readings(&mut s, registry, &health(), Millis(94_000), unix_now).is_ok());
        s
    }

    fn parse(s: &str) -> serde_json::Value {
        match serde_json::from_str(s) {
            Ok(v) => v,
            Err(e) => unreachable!("renderer must emit valid JSON: {e}"),
        }
    }

    /// serde_json's `Index` is total (missing → `Null`), but the crate-wide
    /// indexing lint cannot know that; a get-based walker satisfies it.
    fn field<'a>(v: &'a serde_json::Value, key: &str) -> &'a serde_json::Value {
        match v.get(key) {
            Some(f) => f,
            None => unreachable!("missing key {key}"),
        }
    }

    fn device(v: &serde_json::Value, idx: usize) -> &serde_json::Value {
        match field(v, "devices").get(idx) {
            Some(d) => d,
            None => unreachable!("missing devices[{idx}]"),
        }
    }

    #[test]
    fn golden_body_is_byte_exact() {
        let body = render(&fixture(), Some(1_724_000_000));
        assert_eq!(
            body,
            concat!(
                "{\"time_synced\":true,\"devices\":[",
                "{\"name\":\"baby_room\",\"encrypted\":false,\"online\":true,",
                "\"age_seconds\":4,\"temperature\":26.96,\"humidity\":38.15,",
                "\"battery\":92,\"voltage\":2.939,\"rssi\":-61,\"beacons_per_minute\":2},",
                "{\"name\":\"freezer\",\"encrypted\":true,\"online\":false,",
                "\"age_seconds\":93,\"temperature\":null,\"humidity\":null,",
                "\"battery\":null,\"voltage\":null,\"rssi\":null,\"beacons_per_minute\":1},",
                "{\"name\":\"garage\",\"encrypted\":false,\"online\":false,",
                "\"age_seconds\":null,\"temperature\":null,\"humidity\":null,",
                "\"battery\":null,\"voltage\":null,\"rssi\":null,\"beacons_per_minute\":0}",
                "],\"health\":{\"uptime_seconds\":3600,\"wifi_rssi\":-55,",
                "\"free_heap_bytes\":180000,\"min_free_heap_bytes\":150000,",
                "\"unknown_adverts\":2,\"reboot_total\":7,",
                "\"last_reboot_reason\":\"power\",\"version\":\"0.1.0\",\"git_sha\":\"abc1234\"}}"
            )
        );
    }

    #[test]
    fn golden_body_is_valid_json_with_null_and_number_ages() {
        let v = parse(&render(&fixture(), Some(1_724_000_000)));
        // Offline device: gauge values null, but the sticky sighting age counts up.
        assert!(field(device(&v, 1), "temperature").is_null());
        assert!(field(device(&v, 1), "age_seconds").is_number());
        // Never seen this boot: no sighting, no age.
        assert!(field(device(&v, 2), "age_seconds").is_null());
    }

    #[test]
    fn unsynced_clock_reports_time_synced_false() {
        let body = render(&fixture(), None);
        assert!(body.starts_with("{\"time_synced\":false,"), "{body}");
        assert_eq!(
            field(&parse(&body), "time_synced"),
            &serde_json::Value::Bool(false)
        );
    }

    #[test]
    fn escaper_round_trips_hostile_strings() {
        let hostile = "a\"b\\c\nd";
        let mut s = String::new();
        assert!(write_json_str(&mut s, hostile).is_ok());
        let parsed: String = match serde_json::from_str(&s) {
            Ok(v) => v,
            Err(e) => unreachable!("escaped string must be valid JSON: {e}"),
        };
        assert_eq!(parsed, hostile);
    }
}
