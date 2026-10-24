//! JSON text the way Python's `json.dumps` writes it.
//!
//! Many strings produced by the Python services (tool results, prompts, audit details) are
//! `json.dumps(obj, ensure_ascii=False)` output, and tests plus the scripted test model match on
//! substrings such as `"status": "ended"` — including the space after the colon. serde_json's
//! compact form has no spaces, so everything that was `json.dumps` in Python goes through here.

use serde_json::Value;
use std::fmt::Write;

/// `json.dumps(v, ensure_ascii=False, default=str)`: separators `", "` and `": "`, no indent.
pub fn dumps(v: &Value) -> String {
    let mut out = String::new();
    write_value(&mut out, v, None, 0);
    out
}

/// `json.dumps(v, ensure_ascii=False, indent=n)`.
pub fn dumps_indent(v: &Value, indent: usize) -> String {
    let mut out = String::new();
    write_value(&mut out, v, Some(indent), 0);
    out
}

/// `json.dumps(v)` with `ensure_ascii=True` (the Python default): non-ASCII escaped as `\uXXXX`.
pub fn dumps_ascii(v: &Value) -> String {
    let s = dumps(v);
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        if ch.is_ascii() {
            out.push(ch);
        } else {
            let mut buf = [0u16; 2];
            for u in ch.encode_utf16(&mut buf) {
                let _ = write!(out, "\\u{:04x}", u);
            }
        }
    }
    out
}

/// Python `repr()` of a JSON value (dicts with single quotes, `True`/`False`/`None`).
pub fn py_repr(v: &Value) -> String {
    match v {
        Value::Null => "None".into(),
        Value::Bool(true) => "True".into(),
        Value::Bool(false) => "False".into(),
        Value::Number(_) => format_number(v),
        Value::String(s) => py_repr_str(s),
        Value::Array(a) => format!("[{}]", a.iter().map(py_repr).collect::<Vec<_>>().join(", ")),
        Value::Object(o) => format!(
            "{{{}}}",
            o.iter()
                .map(|(k, v)| format!("{}: {}", py_repr_str(k), py_repr(v)))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

fn py_repr_str(s: &str) -> String {
    let quote = if s.contains('\'') && !s.contains('"') {
        '"'
    } else {
        '\''
    };
    let mut out = String::new();
    out.push(quote);
    for ch in s.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c == quote => {
                out.push('\\');
                out.push(c);
            }
            c if (c as u32) < 0x20 || c as u32 == 0x7f => {
                let _ = write!(out, "\\x{:02x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push(quote);
    out
}

fn write_value(out: &mut String, v: &Value, indent: Option<usize>, level: usize) {
    match v {
        Value::Null => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Number(_) => out.push_str(&format_number(v)),
        Value::String(s) => write_str(out, s),
        Value::Array(a) => {
            if a.is_empty() {
                out.push_str("[]");
                return;
            }
            out.push('[');
            for (i, item) in a.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                    if indent.is_none() {
                        out.push(' ');
                    }
                }
                newline(out, indent, level + 1);
                write_value(out, item, indent, level + 1);
            }
            newline(out, indent, level);
            out.push(']');
        }
        Value::Object(o) => {
            if o.is_empty() {
                out.push_str("{}");
                return;
            }
            out.push('{');
            for (i, (k, item)) in o.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                    if indent.is_none() {
                        out.push(' ');
                    }
                }
                newline(out, indent, level + 1);
                write_str(out, k);
                out.push_str(": ");
                write_value(out, item, indent, level + 1);
            }
            newline(out, indent, level);
            out.push('}');
        }
    }
}

fn newline(out: &mut String, indent: Option<usize>, level: usize) {
    if let Some(n) = indent {
        out.push('\n');
        for _ in 0..(n * level) {
            out.push(' ');
        }
    }
}

/// Python's `json` string escaping with `ensure_ascii=False`: only `"`, `\` and control characters are escaped.
pub fn write_str(out: &mut String, s: &str) {
    out.push('"');
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0c}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

/// Python `repr(float)` / `str(int)`: shortest round-trip, `1.0` keeps its `.0`, exponents as `1e-05` / `1e+16`.
pub fn format_number(v: &Value) -> String {
    let n = match v {
        Value::Number(n) => n,
        _ => return String::new(),
    };
    if let Some(i) = n.as_i64() {
        return i.to_string();
    }
    if let Some(u) = n.as_u64() {
        return u.to_string();
    }
    let f = n.as_f64().unwrap_or(0.0);
    format_f64(f)
}

/// Python `repr(float)`.
pub fn format_f64(f: f64) -> String {
    if f.is_nan() {
        return "NaN".into();
    }
    if f.is_infinite() {
        return if f > 0.0 {
            "Infinity".into()
        } else {
            "-Infinity".into()
        };
    }
    if f == 0.0 {
        return if f.is_sign_negative() {
            "-0.0".into()
        } else {
            "0.0".into()
        };
    }
    let abs = f.abs();
    if (1e-4..1e16).contains(&abs) {
        // Rust's shortest repr is the same digits as Python's; Rust prints "1" for 1.0, Python "1.0".
        let s = format!("{}", f);
        if s.contains('.') || s.contains('e') {
            s
        } else {
            format!("{}.0", s)
        }
    } else {
        // scientific: Python writes 1e-05, 1.5e+16
        let s = format!("{:e}", f); // e.g. "1e-5", "1.5e16"
        let (mant, exp) = s.split_once('e').unwrap_or((&s, "0"));
        let exp_i: i32 = exp.parse().unwrap_or(0);
        let sign = if exp_i < 0 { '-' } else { '+' };
        format!("{}e{}{:02}", mant, sign, exp_i.abs())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn separators_like_python() {
        let v = json!({"a": 1, "b": [1, 2.5, "x"], "c": {"d": null, "e": true}});
        assert_eq!(
            dumps(&v),
            r#"{"a": 1, "b": [1, 2.5, "x"], "c": {"d": null, "e": true}}"#
        );
        assert_eq!(dumps(&json!([])), "[]");
        assert_eq!(dumps(&json!({})), "{}");
    }

    #[test]
    fn indent_like_python() {
        let v = json!({"a": [1, 2], "b": {}});
        assert_eq!(
            dumps_indent(&v, 2),
            "{\n  \"a\": [\n    1,\n    2\n  ],\n  \"b\": {}\n}"
        );
    }

    #[test]
    fn floats_like_python() {
        assert_eq!(format_f64(100.0), "100.0");
        assert_eq!(format_f64(4.9), "4.9");
        assert_eq!(format_f64(0.00001), "1e-05");
        assert_eq!(format_f64(1e16), "1e+16");
        assert_eq!(format_f64(1.5e16), "1.5e+16");
        assert_eq!(format_f64(0.0001), "0.0001");
        assert_eq!(format_f64(-2.0), "-2.0");
    }

    #[test]
    fn strings_keep_unicode() {
        assert_eq!(dumps(&json!("你好 \"q\" \n")), "\"你好 \\\"q\\\" \\n\"");
        assert_eq!(dumps_ascii(&json!("é")), "\"\\u00e9\"");
        assert_eq!(
            py_repr(&json!({"a": "b", "c": [1, true, null]})),
            "{'a': 'b', 'c': [1, True, None]}"
        );
    }
}
