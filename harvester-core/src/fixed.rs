// ABOUTME: Integer fixed-point decimal formatting — no floats (SPEC.md §6.1).
// ABOUTME: Renders centi-/milli-scaled integers (2696→"26.96", 2939→"2.939") into a fmt::Write sink.

/// Writes `value_centi` as a decimal with 2 fraction digits: 2696→"26.96",
/// -523→"-5.23", -50→"-0.50", 1→"0.01", 0→"0.00".
pub fn write_centi<W: core::fmt::Write>(w: &mut W, value_centi: i32) -> core::fmt::Result {
    write_scaled(w, value_centi, 100, 2)
}

/// Writes `value_milli` as a decimal with 3 fraction digits: 2939→"2.939",
/// -1→"-0.001".
pub fn write_milli<W: core::fmt::Write>(w: &mut W, value_milli: i32) -> core::fmt::Result {
    write_scaled(w, value_milli, 1000, 3)
}

/// Splits `value` into integer and fraction parts of `value / scale` and writes
/// them with `frac_digits` zero-padded fraction digits. `unsigned_abs` handles
/// `i32::MIN` (whose `abs()` would panic), so the full i32 range renders exactly.
fn write_scaled<W: core::fmt::Write>(
    w: &mut W,
    value: i32,
    scale: u32,
    frac_digits: usize,
) -> core::fmt::Result {
    if value < 0 {
        w.write_char('-')?;
    }
    let magnitude = value.unsigned_abs();
    let integer = magnitude.checked_div(scale).unwrap_or(0);
    let fraction = magnitude.checked_rem(scale).unwrap_or(0);
    write!(w, "{integer}.{fraction:0frac_digits$}")
}

#[cfg(test)]
mod tests {
    extern crate std;
    use std::string::String;

    use super::*;

    fn centi(v: i32) -> String {
        let mut s = String::new();
        assert!(write_centi(&mut s, v).is_ok());
        s
    }

    fn milli(v: i32) -> String {
        let mut s = String::new();
        assert!(write_milli(&mut s, v).is_ok());
        s
    }

    #[test]
    fn centi_positive() {
        assert_eq!(centi(2696), "26.96");
        assert_eq!(centi(2062), "20.62");
    }

    #[test]
    fn centi_negative() {
        assert_eq!(centi(-5), "-0.05");
        assert_eq!(centi(-900), "-9.00");
        assert_eq!(centi(-4000), "-40.00");
        assert_eq!(centi(-50), "-0.50");
        assert_eq!(centi(-523), "-5.23");
    }

    #[test]
    fn centi_zero_padding() {
        assert_eq!(centi(1), "0.01");
        assert_eq!(centi(10), "0.10");
        assert_eq!(centi(100), "1.00");
        assert_eq!(centi(0), "0.00");
    }

    #[test]
    fn centi_extremes() {
        assert_eq!(centi(i32::MIN), "-21474836.48");
        assert_eq!(centi(i32::MAX), "21474836.47");
    }

    #[test]
    fn milli_cases() {
        assert_eq!(milli(2939), "2.939");
        assert_eq!(milli(2586), "2.586");
        assert_eq!(milli(0), "0.000");
        assert_eq!(milli(-1), "-0.001");
        assert_eq!(milli(3175), "3.175");
    }

    #[test]
    fn milli_extremes() {
        assert_eq!(milli(i32::MIN), "-2147483.648");
        assert_eq!(milli(i32::MAX), "2147483.647");
    }
}
