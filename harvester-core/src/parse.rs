// ABOUTME: BTHome v2 object walk over untrusted radio bytes — whitelist only,
// ABOUTME: bail on unknown ids, range-check every reading (SPEC.md §7–§7.2).

/// Readings accumulated from one frame, stored as the integers the wire
/// carries (§7): centidegrees, centipercent, millivolts. Scaling to human
/// units happens at render time, never here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Readings {
    /// Object `0x02`, `i16` LE, ×0.01 °C.
    pub temperature_centi: Option<i16>,
    /// Object `0x03`, `u16` LE, ×0.01 %.
    pub humidity_centi: Option<u16>,
    /// Object `0x01`, `u8` %.
    pub battery_percent: Option<u8>,
    /// Object `0x0C`, `u16` LE, ×0.001 V.
    pub voltage_milli: Option<u16>,
}

/// Outcome of a successful walk (§7.1: both rows classify as `ok`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Parsed {
    pub readings: Readings,
    /// `Some(id)` = the walk stopped cleanly at an unrecognised object id.
    /// Still `ok` per §7.1; the caller bumps `parse_unknown_object_total`.
    pub unknown_object: Option<u8>,
}

/// Frame-level rejections; all classify the beacon `parse_fail` (§7.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParseError {
    /// `(device_info >> 5) != 2`.
    NotBthomeV2,
    /// A whitelisted object's declared bytes run past the payload end.
    Truncated,
    /// A reading failed its §7.2 plausibility range; the whole beacon drops.
    OutOfRange,
}

/// Walk the BTHome v2 object stream. `device_info` is the first byte of the
/// `0xFCD2` service data; `payload` is the PLAINTEXT object bytes after it
/// (decryption, when needed, happens before this).
///
/// Whitelist, never a size table: a guessed length that is wrong silently
/// desyncs the walk and produces plausible garbage for every later object,
/// which is worse than no data. An unknown id stops the walk and returns
/// what was accumulated (§7).
pub fn parse(device_info: u8, payload: &[u8]) -> Result<Parsed, ParseError> {
    if (device_info >> 5) != 2 {
        return Err(ParseError::NotBthomeV2);
    }

    let mut readings = Readings::default();
    let mut rest = payload;

    while let Some((&id, tail)) = rest.split_first() {
        rest = match id {
            // packet id / the two binary sensors: consume 1 byte, ignore.
            0x00 | 0x10 | 0x11 => skip(tail, 1)?,
            0x01 => {
                let (battery, tail) = take_u8(tail)?;
                if battery > 100 {
                    return Err(ParseError::OutOfRange);
                }
                readings.battery_percent = Some(battery);
                tail
            }
            0x02 => {
                let (raw, tail) = take_u16_le(tail)?;
                let centi = raw as i16;
                if !(-4000..=8500).contains(&centi) {
                    return Err(ParseError::OutOfRange);
                }
                readings.temperature_centi = Some(centi);
                tail
            }
            0x03 => {
                let (centi, tail) = take_u16_le(tail)?;
                // u16 on the wire but i16 in the sparkline ring (§7.2):
                // an unchecked 0xFFFF would overflow on store.
                if centi > 10_000 {
                    return Err(ParseError::OutOfRange);
                }
                readings.humidity_centi = Some(centi);
                tail
            }
            0x0C => {
                let (milli, tail) = take_u16_le(tail)?;
                if milli > 4_000 {
                    return Err(ParseError::OutOfRange);
                }
                readings.voltage_milli = Some(milli);
                tail
            }
            // count, u32: consume 4 bytes, ignore. On the list because the
            // corpus measured it there (§7); its length is not a guess.
            0x3E => skip(tail, 4)?,
            unknown => {
                return Ok(Parsed {
                    readings,
                    unknown_object: Some(unknown),
                });
            }
        };
    }

    Ok(Parsed {
        readings,
        unknown_object: None,
    })
}

/// Drop `n` ignored bytes or report the frame truncated.
fn skip(bytes: &[u8], n: usize) -> Result<&[u8], ParseError> {
    bytes.get(n..).ok_or(ParseError::Truncated)
}

fn take_u8(bytes: &[u8]) -> Result<(u8, &[u8]), ParseError> {
    match bytes {
        [value, rest @ ..] => Ok((*value, rest)),
        [] => Err(ParseError::Truncated),
    }
}

fn take_u16_le(bytes: &[u8]) -> Result<(u16, &[u8]), ParseError> {
    match bytes {
        [lo, hi, rest @ ..] => Ok((u16::from_le_bytes([*lo, *hi]), rest)),
        _ => Err(ParseError::Truncated),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Walk completed cleanly with no readings and no unknown id.
    const EMPTY_OK: Parsed = Parsed {
        readings: Readings {
            temperature_centi: None,
            humidity_centi: None,
            battery_percent: None,
            voltage_milli: None,
        },
        unknown_object: None,
    };

    // Corpus frame "4000290130023d0903e70e" (baby_room, info=40 [00 01 02 03]).
    #[test]
    fn corpus_frame_battery_temperature_humidity() {
        let payload = [0x00, 0x29, 0x01, 0x30, 0x02, 0x3d, 0x09, 0x03, 0xe7, 0x0e];
        assert_eq!(
            parse(0x40, &payload),
            Ok(Parsed {
                readings: Readings {
                    temperature_centi: Some(2365),
                    humidity_centi: Some(3815),
                    battery_percent: Some(48),
                    voltage_milli: None,
                },
                unknown_object: None,
            })
        );
    }

    // Corpus frame "40002a0c1b0a10001101" (baby_room, info=40 [00 0C 10 11]).
    #[test]
    fn corpus_frame_voltage_only() {
        let payload = [0x00, 0x2a, 0x0c, 0x1b, 0x0a, 0x10, 0x00, 0x11, 0x01];
        assert_eq!(
            parse(0x40, &payload),
            Ok(Parsed {
                readings: Readings {
                    temperature_centi: None,
                    humidity_centi: None,
                    battery_percent: None,
                    voltage_milli: Some(2587),
                },
                unknown_object: None,
            })
        );
    }

    // Corpus plaintext "4011013e00000000": 0x3E's four bytes consume the
    // payload exactly. Structurally perfect, zero readings — §7.1's
    // valid-but-empty frame, and it MUST be `ok`.
    #[test]
    fn corpus_frame_valid_but_empty() {
        let payload = [0x11, 0x01, 0x3e, 0x00, 0x00, 0x00, 0x00];
        assert_eq!(parse(0x40, &payload), Ok(EMPTY_OK));
    }

    #[test]
    fn negative_temperature_decodes_as_i16() {
        // −9.00 °C = −900 centi = 0xFC7C, little-endian "7c fc".
        let payload = [0x02, 0x7c, 0xfc];
        let parsed = parse(0x40, &payload);
        assert!(parsed.is_ok());
        assert_eq!(parsed.map(|p| p.readings.temperature_centi), Ok(Some(-900)));
    }

    #[test]
    fn unknown_object_at_start_stops_cleanly() {
        let payload = [0xf0, 0x12, 0x34];
        assert_eq!(
            parse(0x40, &payload),
            Ok(Parsed {
                readings: Readings::default(),
                unknown_object: Some(0xf0),
            })
        );
    }

    #[test]
    fn unknown_object_keeps_accumulated_readings() {
        let payload = [0x02, 0x3d, 0x09, 0xf0, 0xde, 0xad];
        assert_eq!(
            parse(0x40, &payload),
            Ok(Parsed {
                readings: Readings {
                    temperature_centi: Some(2365),
                    ..Readings::default()
                },
                unknown_object: Some(0xf0),
            })
        );
    }

    #[test]
    fn truncated_temperature_is_truncated_not_garbage() {
        assert_eq!(parse(0x40, &[0x02, 0x3d]), Err(ParseError::Truncated));
    }

    #[test]
    fn truncated_battery_and_count_are_truncated() {
        assert_eq!(parse(0x40, &[0x01]), Err(ParseError::Truncated));
        assert_eq!(
            parse(0x40, &[0x3e, 0x00, 0x00, 0x00]),
            Err(ParseError::Truncated)
        );
    }

    #[test]
    fn temperature_above_85c_rejects_beacon() {
        // 8501 centi = 0x2135, LE "35 21".
        assert_eq!(
            parse(0x40, &[0x02, 0x35, 0x21]),
            Err(ParseError::OutOfRange)
        );
    }

    #[test]
    fn temperature_below_minus_40c_rejects_beacon() {
        // −4001 centi = 0xF05F, LE "5f f0".
        assert_eq!(
            parse(0x40, &[0x02, 0x5f, 0xf0]),
            Err(ParseError::OutOfRange)
        );
    }

    #[test]
    fn humidity_ffff_rejects_beacon() {
        // The u16→i16 sparkline overflow §7.2 exists for: 655.35 % is garbage.
        assert_eq!(
            parse(0x40, &[0x03, 0xff, 0xff]),
            Err(ParseError::OutOfRange)
        );
    }

    #[test]
    fn battery_over_100_rejects_beacon() {
        assert_eq!(parse(0x40, &[0x01, 101]), Err(ParseError::OutOfRange));
    }

    #[test]
    fn voltage_over_4v_rejects_beacon() {
        // 4001 mV = 0x0FA1, LE "a1 0f".
        assert_eq!(
            parse(0x40, &[0x0c, 0xa1, 0x0f]),
            Err(ParseError::OutOfRange)
        );
    }

    #[test]
    fn range_boundaries_are_inclusive() {
        // 8500 centi = 0x2134; −4000 = 0xF060; 10000 = 0x2710; 4000 = 0x0FA0.
        let extremes = [
            0x02, 0x34, 0x21, // temp 85.00 °C
            0x03, 0x10, 0x27, // humidity 100.00 %
            0x01, 100, // battery 100 %
            0x0c, 0xa0, 0x0f, // voltage 4.000 V
        ];
        assert_eq!(
            parse(0x40, &extremes),
            Ok(Parsed {
                readings: Readings {
                    temperature_centi: Some(8500),
                    humidity_centi: Some(10_000),
                    battery_percent: Some(100),
                    voltage_milli: Some(4000),
                },
                unknown_object: None,
            })
        );
        let low_temp = [0x02, 0x60, 0xf0]; // −40.00 °C exactly
        assert_eq!(
            parse(0x40, &low_temp).map(|p| p.readings.temperature_centi),
            Ok(Some(-4000))
        );
    }

    #[test]
    fn wrong_version_is_not_bthome_v2() {
        // 0x20 >> 5 == 1, not 2 — reject before touching the payload.
        assert_eq!(parse(0x20, &[0x01, 0x30]), Err(ParseError::NotBthomeV2));
    }

    #[test]
    fn empty_payload_is_ok_with_defaults() {
        assert_eq!(parse(0x40, &[]), Ok(EMPTY_OK));
    }

    #[test]
    fn duplicate_object_id_last_write_wins() {
        let payload = [0x01, 0x32, 0x01, 0x30];
        assert_eq!(
            parse(0x40, &payload).map(|p| p.readings.battery_percent),
            Ok(Some(0x30))
        );
    }
}
