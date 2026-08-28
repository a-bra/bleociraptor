// ABOUTME: /api/history JSON body (§12) — the sparkline series, fetched on load
// ABOUTME: and per sample interval; missing samples are null (§18's sentinel rule).

use crate::registry::Registry;
use crate::render::readings::{write_centi_or_null, write_json_str};
use crate::spark::Spark;
use crate::SPARK_DEPTH;

/// `/api/history` body: `interval_seconds` travels with the series so the page
/// never hard-codes the sample interval (§12). Every configured device appears,
/// online or not — history persists through outages. Each metric array is
/// exactly `SPARK_DEPTH` values, oldest → newest, `null` where no sample.
pub fn render_history<W: core::fmt::Write>(
    w: &mut W,
    registry: &Registry,
    spark: &Spark,
    interval_seconds: u64,
) -> core::fmt::Result {
    write!(
        w,
        "{{\"interval_seconds\":{interval_seconds},\"depth\":{SPARK_DEPTH},\"devices\":["
    )?;
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
        w.write_str(",\"temperature\":[")?;
        write_series(w, spark.temperature(idx))?;
        w.write_str("],\"humidity\":[")?;
        write_series(w, spark.humidity(idx))?;
        w.write_str("]}")?;
    }
    w.write_str("]}")
}

/// One metric's ring as comma-separated JSON values; sentinel slots are null.
fn write_series<W: core::fmt::Write>(
    w: &mut W,
    series: impl Iterator<Item = Option<i16>>,
) -> core::fmt::Result {
    let mut first = true;
    for sample in series {
        if !first {
            w.write_char(',')?;
        }
        first = false;
        write_centi_or_null(w, sample.map(i32::from))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    extern crate std;
    use std::string::String;

    use super::*;
    use crate::clock::Millis;
    use crate::{mac, Device, Tunables};

    static DEVICES: [Device; 2] = [
        Device::plain(mac!("00:11:22:33:44:01"), "baby_room"),
        Device::plain(mac!("00:11:22:33:44:02"), "freezer"),
    ];

    const TUNABLES: Tunables = Tunables {
        device_timeout: Millis(90_000),
        wedge_window: Millis(600_000),
        spark_sample_interval: Millis(300_000),
    };

    fn render(spark: &Spark) -> String {
        let registry = Registry::new(&DEVICES, TUNABLES);
        let mut s = String::new();
        assert!(render_history(&mut s, &registry, spark, 300).is_ok());
        s
    }

    fn parse(s: &str) -> serde_json::Value {
        match serde_json::from_str(s) {
            Ok(v) => v,
            Err(e) => unreachable!("renderer must emit valid JSON: {e}"),
        }
    }

    /// `n` JSON nulls followed by a comma each: "null,null,...,".
    fn nulls(n: usize) -> String {
        "null,".repeat(n)
    }

    #[test]
    fn golden_body_is_byte_exact() {
        let mut spark = Spark::new();
        spark.push(0, Some(2100), Some(4500));
        spark.push(0, None, Some(4550)); // device offline at sample time
        spark.push(0, Some(2200), None);

        let mut expected = String::from("{\"interval_seconds\":300,\"depth\":72,\"devices\":[");
        expected.push_str("{\"name\":\"baby_room\",\"temperature\":[");
        expected.push_str(&nulls(SPARK_DEPTH - 3));
        expected.push_str("21.00,null,22.00],\"humidity\":[");
        expected.push_str(&nulls(SPARK_DEPTH - 3));
        expected.push_str("45.00,45.50,null]},");
        expected.push_str("{\"name\":\"freezer\",\"temperature\":[");
        expected.push_str(&nulls(SPARK_DEPTH - 1));
        expected.push_str("null],\"humidity\":[");
        expected.push_str(&nulls(SPARK_DEPTH - 1));
        expected.push_str("null]}]}");

        assert_eq!(render(&spark), expected);
    }

    #[test]
    fn every_metric_array_is_exactly_spark_depth() {
        let mut spark = Spark::new();
        spark.push(0, Some(2100), Some(4500));

        let v = parse(&render(&spark));
        let devices = match v.get("devices").and_then(serde_json::Value::as_array) {
            Some(d) => d,
            None => unreachable!("devices must be an array"),
        };
        assert_eq!(devices.len(), 2, "all configured devices, online or not");
        for device in devices {
            for metric in ["temperature", "humidity"] {
                let series = match device.get(metric).and_then(serde_json::Value::as_array) {
                    Some(s) => s,
                    None => unreachable!("{metric} must be an array"),
                };
                assert_eq!(series.len(), SPARK_DEPTH);
            }
        }
    }
}
