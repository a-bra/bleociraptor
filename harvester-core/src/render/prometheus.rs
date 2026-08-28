// ABOUTME: The complete /metrics exposition body, rendered metric-major (§11.4):
// ABOUTME: one pass per family, HELP/TYPE once, gauges gap per §3, counters never do.

use crate::fixed::{write_centi, write_milli};
use crate::registry::{DeviceView, Registry};
use crate::render::Health;
use crate::Millis;

// Family order is FIXED so goldens are byte-stable (§11.4). Reordering breaks
// every golden test; appending a new family at a deliberate position is fine.
//
//  1. ble_sensor_temperature_celsius          gauge   {device}, gaps (§3)
//  2. ble_sensor_humidity_percent             gauge   {device}, gaps
//  3. ble_sensor_battery_percent              gauge   {device}, gaps
//  4. ble_sensor_voltage_volts                gauge   {device}, gaps
//  5. ble_sensor_rssi_dbm                     gauge   {device}, gaps
//  6. ble_sensor_last_update_timestamp_seconds gauge  {device}, gaps; needs SNTP (§11.3)
//  7. ble_sensor_seen                         gauge   {device}, EVERY device — the §3 exception
//  8. ble_harvester_uptime_seconds            gauge
//  9. ble_harvester_free_heap_bytes           gauge
// 10. ble_harvester_min_free_heap_bytes       gauge
// 11. ble_harvester_wifi_rssi_dbm             gauge   gaps while the AP is unreachable
// 12. ble_harvester_time_synced               gauge   0/1
// 13. ble_harvester_build_info                gauge   {version,git_sha}, always 1
// 14. ble_harvester_reboot_total              counter {reason} — all seven, zeros included (§3)
// 15. ble_harvester_beacons_total             counter {device,result} — full matrix, zeros included
// 16. ble_harvester_unknown_adverts_total     counter
// 17. ble_harvester_parse_unknown_object_total counter

/// Render the full `/metrics` body. `now` is the monotonic instant at render;
/// `unix_now` is wall-clock seconds if SNTP has synced (§11.3).
///
/// Metric-major (§11.4): one pass per family, `# HELP` then `# TYPE` then every
/// sample, never interleaved — a second TYPE line for a family fails the whole
/// scrape. A family whose every sample is absent still emits its metadata.
pub fn render_metrics<W: core::fmt::Write>(
    w: &mut W,
    registry: &Registry,
    health: &Health,
    now: Millis,
    unix_now: Option<u64>,
) -> core::fmt::Result {
    const TEMPERATURE: &str = "ble_sensor_temperature_celsius";
    family(
        w,
        TEMPERATURE,
        "Sensor temperature in degrees Celsius.",
        "gauge",
    )?;
    for v in online(registry) {
        if let Some(temp) = v.readings.temperature_centi {
            device_sample(w, TEMPERATURE, v.device.name)?;
            write_centi(w, i32::from(temp))?;
            w.write_char('\n')?;
        }
    }

    const HUMIDITY: &str = "ble_sensor_humidity_percent";
    family(w, HUMIDITY, "Sensor relative humidity in percent.", "gauge")?;
    for v in online(registry) {
        if let Some(hum) = v.readings.humidity_centi {
            device_sample(w, HUMIDITY, v.device.name)?;
            write_centi(w, i32::from(hum))?;
            w.write_char('\n')?;
        }
    }

    const BATTERY: &str = "ble_sensor_battery_percent";
    family(
        w,
        BATTERY,
        "Sensor-reported battery level in percent.",
        "gauge",
    )?;
    for v in online(registry) {
        if let Some(batt) = v.readings.battery_percent {
            device_sample(w, BATTERY, v.device.name)?;
            writeln!(w, "{batt}")?;
        }
    }

    const VOLTAGE: &str = "ble_sensor_voltage_volts";
    family(w, VOLTAGE, "Sensor battery voltage in volts.", "gauge")?;
    for v in online(registry) {
        if let Some(volt) = v.readings.voltage_milli {
            device_sample(w, VOLTAGE, v.device.name)?;
            write_milli(w, i32::from(volt))?;
            w.write_char('\n')?;
        }
    }

    const RSSI: &str = "ble_sensor_rssi_dbm";
    family(
        w,
        RSSI,
        "Signal strength of the sensor's last beacon in dBm.",
        "gauge",
    )?;
    for v in online(registry) {
        if let Some(rssi) = v.rssi_dbm {
            device_sample(w, RSSI, v.device.name)?;
            writeln!(w, "{rssi}")?;
        }
    }

    const LAST_UPDATE: &str = "ble_sensor_last_update_timestamp_seconds";
    family(
        w,
        LAST_UPDATE,
        "Unix timestamp of the sensor's last successful reading.",
        "gauge",
    )?;
    if let Some(unix) = unix_now {
        for v in online(registry) {
            if let Some(seen) = v.last_seen {
                // §11.3 derive-at-render: unix_now − monotonic age, in seconds.
                // checked_sub: an age exceeding the wall clock cannot happen in
                // practice, but underflow must never render garbage — omit.
                if let Some(ts) = unix.checked_sub(now.since(seen).as_secs()) {
                    device_sample(w, LAST_UPDATE, v.device.name)?;
                    writeln!(w, "{ts}")?;
                }
            }
        }
    }

    const SEEN: &str = "ble_sensor_seen";
    family(
        w,
        SEEN,
        "Whether the sensor is currently online (1) or timed out (0).",
        "gauge",
    )?;
    for v in views(registry) {
        device_sample(w, SEEN, v.device.name)?;
        writeln!(w, "{}", u8::from(v.online))?;
    }

    family(
        w,
        "ble_harvester_uptime_seconds",
        "Seconds since the harvester booted.",
        "gauge",
    )?;
    writeln!(w, "ble_harvester_uptime_seconds {}", health.uptime_seconds)?;

    family(
        w,
        "ble_harvester_free_heap_bytes",
        "Current free heap in bytes.",
        "gauge",
    )?;
    writeln!(
        w,
        "ble_harvester_free_heap_bytes {}",
        health.free_heap_bytes
    )?;

    family(
        w,
        "ble_harvester_min_free_heap_bytes",
        "Lowest free heap since boot in bytes.",
        "gauge",
    )?;
    writeln!(
        w,
        "ble_harvester_min_free_heap_bytes {}",
        health.min_free_heap_bytes
    )?;

    family(
        w,
        "ble_harvester_wifi_rssi_dbm",
        "Wi-Fi access point signal strength in dBm.",
        "gauge",
    )?;
    if let Some(rssi) = health.wifi_rssi_dbm {
        writeln!(w, "ble_harvester_wifi_rssi_dbm {rssi}")?;
    }

    family(
        w,
        "ble_harvester_time_synced",
        "Whether SNTP has synced wall-clock time (1) or not (0).",
        "gauge",
    )?;
    writeln!(
        w,
        "ble_harvester_time_synced {}",
        u8::from(unix_now.is_some())
    )?;

    family(
        w,
        "ble_harvester_build_info",
        "Build identity; the value is always 1.",
        "gauge",
    )?;
    w.write_str("ble_harvester_build_info{version=\"")?;
    write_escaped(w, health.version)?;
    w.write_str("\",git_sha=\"")?;
    write_escaped(w, health.git_sha)?;
    w.write_str("\"} 1\n")?;

    family(
        w,
        "ble_harvester_reboot_total",
        "Reboots by reason, persisted across restarts.",
        "counter",
    )?;
    // All seven reasons, always, zeros included (§3): a counter that springs
    // into existence on first increment gives increase() nothing to subtract.
    let reboots = [
        ("panic", health.reboots.panic),
        ("wifi", health.reboots.wifi),
        ("ble_wedge", health.reboots.ble_wedge),
        ("ota", health.reboots.ota),
        ("power", health.reboots.power),
        ("wdt", health.reboots.wdt),
        ("unknown", health.reboots.unknown),
    ];
    for (reason, count) in reboots {
        writeln!(
            w,
            "ble_harvester_reboot_total{{reason=\"{reason}\"}} {count}"
        )?;
    }

    family(
        w,
        "ble_harvester_beacons_total",
        "Beacons received per device by outcome.",
        "counter",
    )?;
    // Full device × result matrix, zeros included, grouped per device (§3):
    // counters never gap, expiry does not remove these series.
    for v in views(registry) {
        let results = [
            ("ok", v.counters.ok),
            ("decrypt_fail", v.counters.decrypt_fail),
            ("parse_fail", v.counters.parse_fail),
        ];
        for (result, count) in results {
            w.write_str("ble_harvester_beacons_total{device=\"")?;
            write_escaped(w, v.device.name)?;
            writeln!(w, "\",result=\"{result}\"}} {count}")?;
        }
    }

    family(
        w,
        "ble_harvester_unknown_adverts_total",
        "Adverts received from MACs not in the device list.",
        "counter",
    )?;
    writeln!(
        w,
        "ble_harvester_unknown_adverts_total {}",
        registry.unknown_adverts_total()
    )?;

    family(
        w,
        "ble_harvester_parse_unknown_object_total",
        "Beacons whose object walk stopped at an unrecognised object id.",
        "counter",
    )?;
    writeln!(
        w,
        "ble_harvester_parse_unknown_object_total {}",
        registry.parse_unknown_object_total()
    )
}

/// One family's metadata: HELP then TYPE, exactly once, before any sample.
fn family<W: core::fmt::Write>(w: &mut W, name: &str, help: &str, kind: &str) -> core::fmt::Result {
    writeln!(w, "# HELP {name} {help}")?;
    writeln!(w, "# TYPE {name} {kind}")
}

/// Every configured device, in configuration order.
fn views(registry: &Registry) -> impl Iterator<Item = DeviceView<'_>> + '_ {
    (0..registry.len()).filter_map(move |idx| registry.view(idx))
}

/// Only devices currently online — the per-family §3 gap filter.
fn online(registry: &Registry) -> impl Iterator<Item = DeviceView<'_>> + '_ {
    views(registry).filter(|v| v.online)
}

/// `name{device="<escaped>"} ` — sample prefix for the {device} families.
fn device_sample<W: core::fmt::Write>(w: &mut W, name: &str, device: &str) -> core::fmt::Result {
    w.write_str(name)?;
    w.write_str("{device=\"")?;
    write_escaped(w, device)?;
    w.write_str("\"} ")
}

/// Exposition-format label-value escaping: backslash, double quote and newline
/// become \\ \" \n. Names are static strs under our control, but the escaper
/// is cheap insurance against one weird name corrupting the whole scrape.
fn write_escaped<W: core::fmt::Write>(w: &mut W, s: &str) -> core::fmt::Result {
    for c in s.chars() {
        match c {
            '\\' => w.write_str("\\\\")?,
            '"' => w.write_str("\\\"")?,
            '\n' => w.write_str("\\n")?,
            _ => w.write_char(c)?,
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    extern crate std;
    use std::string::String;
    use std::vec::Vec;

    use super::*;
    use crate::parse::{Parsed, Readings};
    use crate::registry::BeaconFailure;
    use crate::render::RebootCounts;
    use crate::{Device, Tunables};

    const TUNABLES: Tunables = Tunables {
        device_timeout: Millis(90_000),
        wedge_window: Millis(600_000),
        spark_sample_interval: Millis(300_000),
    };

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

    fn health() -> Health {
        Health {
            uptime_seconds: 3600,
            free_heap_bytes: 150_000,
            min_free_heap_bytes: 120_000,
            wifi_rssi_dbm: Some(-55),
            version: "1.0.0",
            git_sha: "abc1234",
            reboots: RebootCounts {
                power: 1,
                ..RebootCounts::default()
            },
            last_reboot_reason: "power",
        }
    }

    fn render_to_string(
        registry: &Registry,
        health: &Health,
        now: Millis,
        unix_now: Option<u64>,
    ) -> String {
        let mut out = String::new();
        assert!(render_metrics(&mut out, registry, health, now, unix_now).is_ok());
        out
    }

    static THREE: [Device; 3] = [
        Device::plain([0xA4, 0xC1, 0x38, 0x00, 0x00, 0x01], "kitchen"),
        Device::plain([0xA4, 0xC1, 0x38, 0x00, 0x00, 0x02], "freezer"),
        Device::plain([0xA4, 0xC1, 0x38, 0x00, 0x00, 0x03], "attic"),
    ];

    /// kitchen online with all four readings + rssi; freezer seen then swept
    /// past timeout (the §3 gap); attic never seen at all.
    fn golden_registry() -> Registry {
        let mut r = Registry::new(&THREE, TUNABLES);
        // freezer: one good beacon, one parse failure, then expiry.
        r.record_beacon(1, Millis(1_000), -80, temp_hum_batt());
        r.record_beacon(1, Millis(2_000), -80, Err(BeaconFailure::Parse));
        r.sweep(Millis(91_000)); // 90 s after freezer's last sighting: expired
                                 // kitchen: both ATC frame shapes, well after the sweep.
        r.record_beacon(0, Millis(100_000), -60, temp_hum_batt());
        r.record_beacon(0, Millis(102_000), -61, voltage_only());
        for _ in 0..7 {
            r.record_unknown_advert();
        }
        r
    }

    /// The §17.1 gap test: assert the ENTIRE exposition byte-for-byte. freezer
    /// and attic appear in NO gauge family, both read 0 in ble_sensor_seen, and
    /// freezer's counters survive expiry with their counts intact (§3).
    #[test]
    fn golden_full_exposition() {
        let r = golden_registry();
        // kitchen last_seen = 102 s, render at 120 s → 18 s old;
        // 1_700_000_000 − 18 = 1_699_999_982.
        let out = render_to_string(&r, &health(), Millis(120_000), Some(1_700_000_000));
        let expected = "\
# HELP ble_sensor_temperature_celsius Sensor temperature in degrees Celsius.
# TYPE ble_sensor_temperature_celsius gauge
ble_sensor_temperature_celsius{device=\"kitchen\"} 23.65
# HELP ble_sensor_humidity_percent Sensor relative humidity in percent.
# TYPE ble_sensor_humidity_percent gauge
ble_sensor_humidity_percent{device=\"kitchen\"} 38.15
# HELP ble_sensor_battery_percent Sensor-reported battery level in percent.
# TYPE ble_sensor_battery_percent gauge
ble_sensor_battery_percent{device=\"kitchen\"} 48
# HELP ble_sensor_voltage_volts Sensor battery voltage in volts.
# TYPE ble_sensor_voltage_volts gauge
ble_sensor_voltage_volts{device=\"kitchen\"} 2.587
# HELP ble_sensor_rssi_dbm Signal strength of the sensor's last beacon in dBm.
# TYPE ble_sensor_rssi_dbm gauge
ble_sensor_rssi_dbm{device=\"kitchen\"} -61
# HELP ble_sensor_last_update_timestamp_seconds Unix timestamp of the sensor's last successful reading.
# TYPE ble_sensor_last_update_timestamp_seconds gauge
ble_sensor_last_update_timestamp_seconds{device=\"kitchen\"} 1699999982
# HELP ble_sensor_seen Whether the sensor is currently online (1) or timed out (0).
# TYPE ble_sensor_seen gauge
ble_sensor_seen{device=\"kitchen\"} 1
ble_sensor_seen{device=\"freezer\"} 0
ble_sensor_seen{device=\"attic\"} 0
# HELP ble_harvester_uptime_seconds Seconds since the harvester booted.
# TYPE ble_harvester_uptime_seconds gauge
ble_harvester_uptime_seconds 3600
# HELP ble_harvester_free_heap_bytes Current free heap in bytes.
# TYPE ble_harvester_free_heap_bytes gauge
ble_harvester_free_heap_bytes 150000
# HELP ble_harvester_min_free_heap_bytes Lowest free heap since boot in bytes.
# TYPE ble_harvester_min_free_heap_bytes gauge
ble_harvester_min_free_heap_bytes 120000
# HELP ble_harvester_wifi_rssi_dbm Wi-Fi access point signal strength in dBm.
# TYPE ble_harvester_wifi_rssi_dbm gauge
ble_harvester_wifi_rssi_dbm -55
# HELP ble_harvester_time_synced Whether SNTP has synced wall-clock time (1) or not (0).
# TYPE ble_harvester_time_synced gauge
ble_harvester_time_synced 1
# HELP ble_harvester_build_info Build identity; the value is always 1.
# TYPE ble_harvester_build_info gauge
ble_harvester_build_info{version=\"1.0.0\",git_sha=\"abc1234\"} 1
# HELP ble_harvester_reboot_total Reboots by reason, persisted across restarts.
# TYPE ble_harvester_reboot_total counter
ble_harvester_reboot_total{reason=\"panic\"} 0
ble_harvester_reboot_total{reason=\"wifi\"} 0
ble_harvester_reboot_total{reason=\"ble_wedge\"} 0
ble_harvester_reboot_total{reason=\"ota\"} 0
ble_harvester_reboot_total{reason=\"power\"} 1
ble_harvester_reboot_total{reason=\"wdt\"} 0
ble_harvester_reboot_total{reason=\"unknown\"} 0
# HELP ble_harvester_beacons_total Beacons received per device by outcome.
# TYPE ble_harvester_beacons_total counter
ble_harvester_beacons_total{device=\"kitchen\",result=\"ok\"} 2
ble_harvester_beacons_total{device=\"kitchen\",result=\"decrypt_fail\"} 0
ble_harvester_beacons_total{device=\"kitchen\",result=\"parse_fail\"} 0
ble_harvester_beacons_total{device=\"freezer\",result=\"ok\"} 1
ble_harvester_beacons_total{device=\"freezer\",result=\"decrypt_fail\"} 0
ble_harvester_beacons_total{device=\"freezer\",result=\"parse_fail\"} 1
ble_harvester_beacons_total{device=\"attic\",result=\"ok\"} 0
ble_harvester_beacons_total{device=\"attic\",result=\"decrypt_fail\"} 0
ble_harvester_beacons_total{device=\"attic\",result=\"parse_fail\"} 0
# HELP ble_harvester_unknown_adverts_total Adverts received from MACs not in the device list.
# TYPE ble_harvester_unknown_adverts_total counter
ble_harvester_unknown_adverts_total 7
# HELP ble_harvester_parse_unknown_object_total Beacons whose object walk stopped at an unrecognised object id.
# TYPE ble_harvester_parse_unknown_object_total counter
ble_harvester_parse_unknown_object_total 0
";
        assert_eq!(out, expected);
    }

    /// The family order documented in the implementation, used by the
    /// structural test to pin ordering (byte-stable goldens depend on it).
    const FAMILY_ORDER: [&str; 17] = [
        "ble_sensor_temperature_celsius",
        "ble_sensor_humidity_percent",
        "ble_sensor_battery_percent",
        "ble_sensor_voltage_volts",
        "ble_sensor_rssi_dbm",
        "ble_sensor_last_update_timestamp_seconds",
        "ble_sensor_seen",
        "ble_harvester_uptime_seconds",
        "ble_harvester_free_heap_bytes",
        "ble_harvester_min_free_heap_bytes",
        "ble_harvester_wifi_rssi_dbm",
        "ble_harvester_time_synced",
        "ble_harvester_build_info",
        "ble_harvester_reboot_total",
        "ble_harvester_beacons_total",
        "ble_harvester_unknown_adverts_total",
        "ble_harvester_parse_unknown_object_total",
    ];

    /// Metric-major invariants (§11.4): TYPE exactly once per family, every
    /// sample under the family it belongs to, families in the documented order.
    #[test]
    fn exposition_is_strictly_metric_major() {
        let r = golden_registry();
        let out = render_to_string(&r, &health(), Millis(120_000), Some(1_700_000_000));

        let mut families: Vec<&str> = Vec::new();
        let mut current: Option<&str> = None;
        for line in out.lines() {
            if let Some(rest) = line.strip_prefix("# TYPE ") {
                let name = rest.split(' ').next().unwrap_or("");
                assert!(
                    !families.contains(&name),
                    "second TYPE line for {name} — fails the whole scrape"
                );
                families.push(name);
                current = Some(name);
            } else if let Some(rest) = line.strip_prefix("# HELP ") {
                let name = rest.split(' ').next().unwrap_or("");
                assert!(
                    !families.contains(&name),
                    "HELP for {name} after its TYPE — metadata must lead"
                );
            } else if !line.is_empty() {
                let name = line.split(['{', ' ']).next().unwrap_or("");
                assert_eq!(
                    Some(name),
                    current,
                    "sample {line:?} interleaved outside its family"
                );
            }
        }
        assert_eq!(families, FAMILY_ORDER);
    }

    /// §11.3: until SNTP syncs, last_update emits HELP/TYPE with zero samples
    /// and time_synced reads 0. Everything else is unaffected.
    #[test]
    fn unsynced_clock_omits_timestamps_and_reports_time_synced_zero() {
        let r = golden_registry();
        let out = render_to_string(&r, &health(), Millis(120_000), None);
        assert!(out.contains(
            "# TYPE ble_sensor_last_update_timestamp_seconds gauge\n# HELP ble_sensor_seen"
        ));
        assert!(out.contains("\nble_harvester_time_synced 0\n"));
        // The device is still online — its other gauges are untouched.
        assert!(out.contains("ble_sensor_temperature_celsius{device=\"kitchen\"} 23.65\n"));
    }

    /// §11.3 derive-at-render: last_update = unix_now − (now − last_seen).
    #[test]
    fn last_update_is_derived_from_monotonic_age_at_render_time() {
        static ONE: [Device; 1] = [Device::plain([0xA4, 0xC1, 0x38, 0x00, 0x00, 0x09], "porch")];
        let mut r = Registry::new(&ONE, TUNABLES);
        r.record_beacon(0, Millis(5_000), -60, temp_hum_batt());
        let out = render_to_string(&r, &health(), Millis(65_000), Some(1_787_832_000));
        assert!(
            out.contains("ble_sensor_last_update_timestamp_seconds{device=\"porch\"} 1787831940\n")
        );
    }

    /// §11.2: wifi_rssi gaps like any other gauge — HELP/TYPE, zero samples.
    #[test]
    fn wifi_rssi_none_emits_family_with_zero_samples() {
        let r = golden_registry();
        let mut h = health();
        h.wifi_rssi_dbm = None;
        let out = render_to_string(&r, &h, Millis(120_000), Some(1_700_000_000));
        assert!(out.contains(
            "# TYPE ble_harvester_wifi_rssi_dbm gauge\n# HELP ble_harvester_time_synced"
        ));
        assert!(!out.contains("\nble_harvester_wifi_rssi_dbm -"));
    }

    /// Exposition-format label escaping: backslash, double-quote, newline.
    #[test]
    fn label_values_are_escaped() {
        static WEIRD: [Device; 1] = [Device::plain(
            [0xA4, 0xC1, 0x38, 0x00, 0x00, 0x0A],
            r#"we"ird\name"#,
        )];
        let r = Registry::new(&WEIRD, TUNABLES);
        let out = render_to_string(&r, &health(), Millis(1_000), None);
        assert!(out.contains(r#"ble_sensor_seen{device="we\"ird\\name"} 0"#));
    }
}
