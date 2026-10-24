//! Number formatting as C's printf does it, which Lua's `tostring` and
//! `string.format` use. Rust's exponent formatting is correctly rounded, like
//! glibc, so the digits match.

/// C's `%.{precision}g`.
pub fn format_g(n: f64, precision: usize) -> String {
    format_g_flags(n, precision, false)
}

/// C's `%.{precision}g`, with `alt` for the `#` flag (keep trailing zeros).
pub fn format_g_flags(n: f64, precision: usize, alt: bool) -> String {
    if let Some(s) = non_finite(n) {
        return s;
    }
    let p = if precision == 0 { 1 } else { precision };
    if n == 0.0 {
        let mut s = String::from(if n.is_sign_negative() { "-0" } else { "0" });
        if alt && p > 1 {
            s.push('.');
            s.extend(std::iter::repeat_n('0', p - 1));
        }
        return s;
    }
    // Exponent after rounding to p significant digits
    let e_form = format!("{:.*e}", p - 1, n);
    let (mantissa, exp) = e_form.split_once('e').expect("exponent");
    let x: i32 = exp.parse().expect("exponent");
    if (x as i64) < p as i64 && x >= -4 {
        let decimals = (p as i32 - 1 - x) as usize;
        let s = format!("{:.*}", decimals, n);
        if alt { s } else { strip_zeros(s) }
    } else {
        let m = if alt {
            mantissa.to_string()
        } else {
            strip_zeros(mantissa.to_string())
        };
        format!("{}e{}{:02}", m, if x < 0 { '-' } else { '+' }, x.abs())
    }
}

/// C's `%.{precision}f`.
pub fn format_f(n: f64, precision: usize) -> String {
    if let Some(s) = non_finite(n) {
        return s;
    }
    format!("{:.*}", precision, n)
}

/// C's `%.{precision}e`.
pub fn format_e(n: f64, precision: usize) -> String {
    if let Some(s) = non_finite(n) {
        return s;
    }
    let e_form = format!("{:.*e}", precision, n);
    let (mantissa, exp) = e_form.split_once('e').expect("exponent");
    let x: i32 = exp.parse().expect("exponent");
    format!(
        "{}e{}{:02}",
        mantissa,
        if x < 0 { '-' } else { '+' },
        x.abs()
    )
}

fn non_finite(n: f64) -> Option<String> {
    if n.is_nan() {
        Some(if n.is_sign_negative() { "-nan" } else { "nan" }.to_string())
    } else if n.is_infinite() {
        Some(if n > 0.0 { "inf" } else { "-inf" }.to_string())
    } else {
        None
    }
}

fn strip_zeros(s: String) -> String {
    if s.contains('.') {
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    } else {
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn g14_like_lua_tostring() {
        assert_eq!(format_g(1.0, 14), "1");
        assert_eq!(format_g(0.1, 14), "0.1");
        assert_eq!(format_g(1e15, 14), "1e+15");
        assert_eq!(format_g(123456789012345.0, 14), "1.2345678901234e+14");
        assert_eq!(format_g(0.0001, 14), "0.0001");
        assert_eq!(format_g(0.00001, 14), "1e-05");
        assert_eq!(format_g(-2.5, 14), "-2.5");
        assert_eq!(format_g(1.0 / 3.0, 14), "0.33333333333333");
        assert_eq!(format_g(100.0, 14), "100");
        assert_eq!(format_g(f64::INFINITY, 14), "inf");
    }

    #[test]
    fn f_and_e() {
        assert_eq!(format_f(2.5, 0), "2");
        assert_eq!(format_f(3.5, 0), "4");
        assert_eq!(format_f(1.005, 2), "1.00");
        assert_eq!(format_e(12345.0, 2), "1.23e+04");
    }
}
