//! Python-compatible text formatting used by the document modules: `html.escape`, `f"{v:,.4f}"`,
//! `f"{v:.4g}"`, `round(v, n)`, `math.fsum`, `str(v)` for cell values. Kept private to the crate.

use serde_json::Value;

/// Python `html.escape(s, quote)`.
pub(crate) fn html_escape(s: &str, quote: bool) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' if quote => out.push_str("&quot;"),
            '\'' if quote => out.push_str("&#x27;"),
            c => out.push(c),
        }
    }
    out
}

/// Python `str(float)` (`repr`): `nan`, `inf`, `1.0`, `1e-05`.
pub(crate) fn float_str(f: f64) -> String {
    if f.is_nan() {
        return "nan".into();
    }
    if f.is_infinite() {
        return if f > 0.0 { "inf".into() } else { "-inf".into() };
    }
    omuse_common::pyjson::format_f64(f)
}

/// Python `str(v)` for a JSON cell value (`None` → "None", floats via `repr`).
pub(crate) fn py_str(v: &Value) -> String {
    match v {
        Value::Number(n) => match n.as_i64() {
            Some(i) => i.to_string(),
            None => match n.as_u64() {
                Some(u) => u.to_string(),
                None => float_str(n.as_f64().unwrap_or(0.0)),
            },
        },
        other => omuse_common::util::py_str(other),
    }
}

/// Python `f"{v:.{prec}f}"`.
pub(crate) fn fixed(v: f64, prec: usize) -> String {
    if v.is_nan() {
        return "nan".into();
    }
    if v.is_infinite() {
        return if v > 0.0 { "inf".into() } else { "-inf".into() };
    }
    format!("{:.*}", prec, v)
}

/// Insert thousands separators into the integer part of a plain decimal string.
pub(crate) fn commas(s: &str) -> String {
    let (sign, rest) = match s.strip_prefix('-') {
        Some(r) => ("-", r),
        None => ("", s),
    };
    let (int_part, frac) = match rest.find('.') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, ""),
    };
    if !int_part.bytes().all(|b| b.is_ascii_digit()) {
        return s.to_string();
    }
    let mut out = String::new();
    let n = int_part.len();
    for (i, ch) in int_part.chars().enumerate() {
        if i > 0 && (n - i) % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }
    format!("{sign}{out}{frac}")
}

/// Python `f"{v:,.{prec}f}"`.
pub(crate) fn fixed_commas(v: f64, prec: usize) -> String {
    commas(&fixed(v, prec))
}

/// Python `f"{i:,}"`.
pub(crate) fn int_commas(i: i64) -> String {
    commas(&i.to_string())
}

/// Python `f"{v:.{prec}g}"`.
pub(crate) fn general(v: f64, prec: usize) -> String {
    if v.is_nan() {
        return "nan".into();
    }
    if v.is_infinite() {
        return if v > 0.0 { "inf".into() } else { "-inf".into() };
    }
    let p = prec.max(1);
    if v == 0.0 {
        return if v.is_sign_negative() {
            "-0".into()
        } else {
            "0".into()
        };
    }
    // exponent of the value once rounded to `p` significant digits
    let sci = format!("{:.*e}", p - 1, v);
    let (mant, exp) = sci.split_once('e').unwrap_or((&sci, "0"));
    let exp: i32 = exp.parse().unwrap_or(0);
    if exp < -4 || exp >= p as i32 {
        let mant = strip_zeros(mant);
        let sign = if exp < 0 { '-' } else { '+' };
        format!("{mant}e{sign}{:02}", exp.abs())
    } else {
        let decimals = (p as i32 - 1 - exp).max(0) as usize;
        strip_zeros(&format!("{:.*}", decimals, v))
    }
}

fn strip_zeros(s: &str) -> String {
    if s.contains('.') {
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    } else {
        s.to_string()
    }
}

/// Python `round(v, n)` for floats: correctly rounded to `n` decimals.
pub(crate) fn round_to(v: f64, n: usize) -> f64 {
    if !v.is_finite() {
        return v;
    }
    format!("{:.*}", n, v).parse().unwrap_or(v)
}

/// Python `math.fsum`: exactly rounded sum (Shewchuk partials).
pub(crate) fn fsum<I: IntoIterator<Item = f64>>(iter: I) -> f64 {
    let mut partials: Vec<f64> = Vec::new();
    for mut x in iter {
        let mut i = 0;
        for j in 0..partials.len() {
            let mut y = partials[j];
            if x.abs() < y.abs() {
                std::mem::swap(&mut x, &mut y);
            }
            let hi = x + y;
            let lo = y - (hi - x);
            if lo != 0.0 {
                partials[i] = lo;
                i += 1;
            }
            x = hi;
        }
        partials.truncate(i);
        partials.push(x);
    }
    partials.iter().fold(0.0, |a, b| a + b)
}

/// Python `statistics.fmean` (`fsum(xs) / len(xs)`); empty → 0.0 (callers guard).
pub(crate) fn fmean(xs: &[f64]) -> f64 {
    if xs.is_empty() {
        return 0.0;
    }
    fsum(xs.iter().copied()) / xs.len() as f64
}

/// Python `statistics.median`.
pub(crate) fn median(xs: &[f64]) -> f64 {
    let mut s = xs.to_vec();
    s.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let n = s.len();
    if n == 0 {
        return f64::NAN;
    }
    if n % 2 == 1 {
        s[n / 2]
    } else {
        (s[n / 2 - 1] + s[n / 2]) / 2.0
    }
}

/// Sum of squared deviations from the mean (`statistics._ss`, mean via fsum).
fn ss(xs: &[f64]) -> f64 {
    let c = fmean(xs);
    fsum(xs.iter().map(|x| (x - c) * (x - c)))
}

/// Python `statistics.variance` (sample; callers guard n >= 2).
pub(crate) fn variance(xs: &[f64]) -> f64 {
    ss(xs) / (xs.len() as f64 - 1.0)
}

/// Python `statistics.pvariance`.
pub(crate) fn pvariance(xs: &[f64]) -> f64 {
    ss(xs) / xs.len() as f64
}

/// Python `statistics.stdev`.
pub(crate) fn stdev(xs: &[f64]) -> f64 {
    variance(xs).sqrt()
}

/// Python `statistics.pstdev`.
pub(crate) fn pstdev(xs: &[f64]) -> f64 {
    pvariance(xs).sqrt()
}

/// Python `repr()` of a JSON value (strings quoted the Python way).
pub(crate) fn py_repr(v: &Value) -> String {
    omuse_common::pyjson::py_repr(v)
}

/// Python `s[:n]` (characters).
pub(crate) fn take_chars(s: &str, n: usize) -> &str {
    omuse_common::util::take_chars(s, n)
}

/// Python's name for the type of a JSON value (for `TypeError` texts).
pub(crate) fn py_type(v: &Value) -> &'static str {
    match v {
        Value::Null => "NoneType",
        Value::Bool(_) => "bool",
        Value::Number(n) => {
            if n.is_f64() {
                "float"
            } else {
                "int"
            }
        }
        Value::String(_) => "str",
        Value::Array(_) => "list",
        Value::Object(_) => "dict",
    }
}

/// Python `for x in v`: lists give their items, strings their characters, dicts their keys.
pub(crate) fn py_iter(v: &Value) -> Result<Vec<Value>, String> {
    match v {
        Value::Array(a) => Ok(a.clone()),
        Value::String(s) => Ok(s.chars().map(|c| Value::String(c.to_string())).collect()),
        Value::Object(o) => Ok(o.keys().map(|k| Value::String(k.clone())).collect()),
        other => Err(format!("'{}' object is not iterable", py_type(other))),
    }
}

/// Python `str.splitlines()` (universal line boundaries, no trailing empty line).
pub(crate) fn splitlines(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut iter = s.char_indices().peekable();
    while let Some((i, c)) = iter.next() {
        let is_break = matches!(
            c,
            '\n' | '\r'
                | '\x0b'
                | '\x0c'
                | '\x1c'
                | '\x1d'
                | '\x1e'
                | '\u{85}'
                | '\u{2028}'
                | '\u{2029}'
        );
        if is_break {
            out.push(&s[start..i]);
            let mut end = i + c.len_utf8();
            if c == '\r' {
                if let Some((j, '\n')) = iter.peek().copied() {
                    iter.next();
                    end = j + 1;
                }
            }
            start = end;
        }
    }
    if start < s.len() {
        out.push(&s[start..]);
    }
    out
}

/// A float as a JSON value (`NaN`/`inf` have no JSON form and become their Python text).
pub(crate) fn float_value(f: f64) -> Value {
    match serde_json::Number::from_f64(f) {
        Some(n) => Value::Number(n),
        None => Value::String(float_str(f)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn python_formats() {
        assert_eq!(fixed_commas(1234567.891, 4), "1,234,567.8910");
        assert_eq!(fixed_commas(-1234.5, 0), "-1,234");
        assert_eq!(fixed_commas(2.5, 0), "2");
        assert_eq!(int_commas(15000), "15,000");
        assert_eq!(general(0.00001234, 4), "1.234e-05");
        assert_eq!(general(1234567.0, 6), "1.23457e+06");
        assert_eq!(general(100.0, 6), "100");
        assert_eq!(general(0.5, 6), "0.5");
        assert_eq!(general(7.0, 6), "7");
        assert_eq!(round_to(2.675, 2), 2.67);
        assert_eq!(round_to(0.30000000000000004, 10), 0.3);
        assert_eq!(fsum([0.1, 0.2, 0.3]), 0.6);
        assert_eq!(html_escape("a<b>&\"'", true), "a&lt;b&gt;&amp;&quot;&#x27;");
        assert_eq!(html_escape("a<b>&\"'", false), "a&lt;b&gt;&amp;\"'");
        assert_eq!(float_str(96.25000000000001), "96.25000000000001");
        assert_eq!(float_str(284.0), "284.0");
    }
}
