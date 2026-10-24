//! Small Python-compatibility helpers shared by the connectors: `urllib.parse.quote` / `urlencode`,
//! the `httpx` exception names that appear in error strings, `format(x, "g")` and `round(x, n)`.

use std::fmt::Write as _;

/// `urllib.parse.quote(s, safe=safe)`: percent-encode every UTF-8 byte except `A-Za-z0-9_.-~` and `safe`.
pub fn quote(s: &str, safe: &str) -> String {
    let mut out = String::with_capacity(s.len() * 3);
    for &b in s.as_bytes() {
        let c = b as char;
        if b.is_ascii_alphanumeric()
            || matches!(c, '_' | '.' | '-' | '~')
            || (b.is_ascii() && safe.contains(c))
        {
            out.push(c);
        } else {
            let _ = write!(out, "%{:02X}", b);
        }
    }
    out
}

/// `urllib.parse.quote_plus(s)`: like [`quote`] with no extra safe characters, spaces become `+`.
pub fn quote_plus(s: &str) -> String {
    quote(s, " ").replace(' ', "+")
}

/// `urllib.parse.urlencode(dict)` (keys in the given order).
pub fn urlencode(pairs: &[(&str, &str)]) -> String {
    pairs
        .iter()
        .map(|(k, v)| format!("{}={}", quote_plus(k), quote_plus(v)))
        .collect::<Vec<_>>()
        .join("&")
}

/// The `httpx` exception class name (`type(e).__name__`) that best matches a reqwest error — the Python
/// connectors put it into their bilingual error strings.
pub fn httpx_name(e: &reqwest::Error) -> &'static str {
    if e.is_timeout() {
        if e.is_connect() {
            "ConnectTimeout"
        } else {
            "ReadTimeout"
        }
    } else if e.is_connect() {
        "ConnectError"
    } else if e.is_redirect() {
        "TooManyRedirects"
    } else if e.is_decode() {
        "DecodingError"
    } else if e.is_body() || e.is_request() {
        "ReadError"
    } else {
        "HTTPError"
    }
}

/// Python `round(v, n)` for display: correctly rounded on the exact binary value (half to even on ties).
pub fn pyround(v: f64, n: usize) -> f64 {
    if !v.is_finite() {
        return v;
    }
    format!("{:.*}", n, v).parse().unwrap_or(v)
}

/// Python `format(v, "g")`: 6 significant digits, trailing zeros removed, exponent form below 1e-4 / from 1e6.
pub fn pyg(v: f64) -> String {
    if v.is_nan() {
        return "nan".into();
    }
    if v.is_infinite() {
        return if v > 0.0 { "inf".into() } else { "-inf".into() };
    }
    if v == 0.0 {
        return if v.is_sign_negative() {
            "-0".into()
        } else {
            "0".into()
        };
    }
    let sci = format!("{:.5e}", v); // e.g. "1.23457e6", "-4.00000e2"
    let (mant, exp) = sci.split_once('e').unwrap_or((&sci, "0"));
    let exp: i32 = exp.parse().unwrap_or(0);
    if !(-4..6).contains(&exp) {
        let m = strip_zeros(mant);
        let sign = if exp < 0 { '-' } else { '+' };
        format!("{}e{}{:02}", m, sign, exp.abs())
    } else {
        let prec = (5 - exp).max(0) as usize;
        strip_zeros(&format!("{:.*}", prec, v))
    }
}

fn strip_zeros(s: &str) -> String {
    if s.contains('.') {
        let t = s.trim_end_matches('0');
        t.trim_end_matches('.').to_string()
    } else {
        s.to_string()
    }
}

/// `int(v)` as Python would coerce a tool argument: numbers, numeric strings, booleans; `None` otherwise.
pub fn py_int(v: &serde_json::Value) -> Option<i64> {
    omuse_common::util::as_i64(v)
}

/// `str(v or "")`: falsy values give "", everything else `str(v)`.
pub fn str_or_empty(v: &serde_json::Value) -> String {
    if omuse_common::util::truthy(v) {
        omuse_common::util::py_str(v)
    } else {
        String::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quoting_matches_urllib() {
        assert_eq!(quote("a b/ü", ""), "a%20b%2F%C3%BC");
        assert_eq!(quote_plus("a b&c=~_."), "a+b%26c%3D~_.");
        assert_eq!(
            urlencode(&[("text", "[测试] A"), ("x", "1 2")]),
            "text=%5B%E6%B5%8B%E8%AF%95%5D+A&x=1+2"
        );
    }

    #[test]
    fn g_format_matches_python() {
        assert_eq!(pyg(400.0), "400");
        assert_eq!(pyg(25000.0), "25000");
        assert_eq!(pyg(7.4), "7.4");
        assert_eq!(pyg(0.1234), "0.1234");
        assert_eq!(pyg(123456.7), "123457");
        assert_eq!(pyg(1234567.0), "1.23457e+06");
        assert_eq!(pyg(0.00001234), "1.234e-05");
        assert_eq!(pyg(999999.5), "1e+06");
        assert_eq!(pyg(-15.25), "-15.25");
        assert_eq!(pyg(1.4e12), "1.4e+12");
    }

    #[test]
    fn rounding_matches_python() {
        assert_eq!(pyround(2.675, 2), 2.67);
        assert_eq!(pyround(404.0, 2), 404.0);
        assert_eq!(pyround(7.123456, 4), 7.1235);
        assert_eq!(pyround(1.005, 2), 1.0);
    }
}
