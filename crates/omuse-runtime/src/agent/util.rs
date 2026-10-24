//! Small helpers the agent modules share: JSON accessors with Python semantics, Python-style number and time
//! formatting, `os.path.realpath`, and the error type local tools raise.

use std::path::{Component, Path, PathBuf};

use chrono::{Local, TimeZone};
use omuse_common::pyjson;
use omuse_common::util::{py_str, truthy};
use serde_json::{Map, Value};

/// `str(v or "")`: "" for falsy values, else Python's `str()`.
pub fn pystr_or_empty(v: &Value) -> String {
    if truthy(v) {
        py_str(v)
    } else {
        String::new()
    }
}

/// `str(d.get(key) or "")`.
pub fn sget(d: &Value, key: &str) -> String {
    d.get(key).map(pystr_or_empty).unwrap_or_default()
}

/// `str(m.get("content") or "")` — the content of a transcript message as text.
pub fn content_of(m: &Value) -> String {
    sget(m, "content")
}

/// `d.get(key) or []` as a slice.
pub fn arr<'a>(d: &'a Value, key: &str) -> &'a [Value] {
    d.get(key)
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[])
}

/// `m.get("tool_calls") or []`.
pub fn tool_calls(m: &Value) -> &[Value] {
    arr(m, "tool_calls")
}

/// The `(name, arguments)` of one stored tool call: `fn.get("name", "")`, `fn.get("arguments")` parsed when it is
/// a JSON string (`{}` on a parse error), the dict as is, `{}` when missing.
pub fn stored_call(tc: &Value) -> (String, Value) {
    let f = tc
        .get("function")
        .filter(|f| truthy(f))
        .cloned()
        .unwrap_or(Value::Object(Map::new()));
    let name = sget(&f, "name");
    let args = match f.get("arguments") {
        Some(Value::String(s)) => {
            serde_json::from_str::<Value>(if s.is_empty() { "{}" } else { s })
                .unwrap_or_else(|_| Value::Object(Map::new()))
        }
        Some(v) if truthy(v) => v.clone(),
        _ => Value::Object(Map::new()),
    };
    (name, args)
}

/// `json.dumps(v, sort_keys=True, ensure_ascii=False)`: object keys sorted at every level.
pub fn dumps_sorted(v: &Value) -> String {
    pyjson::dumps(&sort_keys(v))
}

fn sort_keys(v: &Value) -> Value {
    match v {
        Value::Object(o) => {
            let mut keys: Vec<&String> = o.keys().collect();
            keys.sort();
            let mut m = Map::new();
            for k in keys {
                m.insert(k.clone(), sort_keys(&o[k]));
            }
            Value::Object(m)
        }
        Value::Array(a) => Value::Array(a.iter().map(sort_keys).collect()),
        other => other.clone(),
    }
}

/// Python `f"{x:g}"`.
pub fn fmt_g(x: f64) -> String {
    if x == 0.0 {
        return "0".into();
    }
    if !x.is_finite() {
        return if x.is_nan() {
            "nan".into()
        } else if x > 0.0 {
            "inf".into()
        } else {
            "-inf".into()
        };
    }
    let exp = x.abs().log10().floor() as i32;
    if !(-5..6).contains(&exp) {
        // scientific notation, 6 significant digits, exponent at least two digits
        let m = x / 10f64.powi(exp);
        let mant = trim_zeros(&format!("{:.5}", m));
        return format!(
            "{}e{}{:02}",
            mant,
            if exp < 0 { "-" } else { "+" },
            exp.abs()
        );
    }
    let decimals = (5 - exp).max(0) as usize;
    trim_zeros(&format!("{:.*}", decimals, x))
}

fn trim_zeros(s: &str) -> String {
    if s.contains('.') {
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    } else {
        s.to_string()
    }
}

/// Python `repr(float)` (`str(20.0)` = "20.0").
pub fn fmt_float(x: f64) -> String {
    pyjson::format_f64(x)
}

/// `time.strftime(fmt, time.localtime(ts))`.
pub fn local_strftime(ts: f64, fmt: &str) -> String {
    match Local.timestamp_opt(ts.floor() as i64, 0).single() {
        Some(dt) => dt.format(fmt).to_string(),
        None => String::new(),
    }
}

/// `time.strftime(fmt)` for now.
pub fn now_strftime(fmt: &str) -> String {
    Local::now().format(fmt).to_string()
}

/// `os.path.realpath(p)`: symlinks of the existing part resolved, `.`/`..` of the rest folded lexically.
pub fn realpath(p: &Path) -> PathBuf {
    let abs = if p.is_absolute() {
        p.to_path_buf()
    } else {
        std::env::current_dir()
            .map(|c| c.join(p))
            .unwrap_or_else(|_| p.to_path_buf())
    };
    let norm = normalize(&abs);
    if let Ok(c) = std::fs::canonicalize(&norm) {
        return c;
    }
    // canonicalize the longest existing ancestor, then append the remainder
    let mut rest: Vec<std::ffi::OsString> = Vec::new();
    let mut cur = norm.clone();
    loop {
        if let Ok(c) = std::fs::canonicalize(&cur) {
            let mut out = c;
            for r in rest.iter().rev() {
                out.push(r);
            }
            return out;
        }
        match (
            cur.file_name().map(|f| f.to_os_string()),
            cur.parent().map(Path::to_path_buf),
        ) {
            (Some(name), Some(parent)) => {
                rest.push(name);
                cur = parent;
            }
            _ => return norm,
        }
    }
}

fn normalize(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// `os.path.relpath(p, base)` for a path under `base` (the agent only asks for workspace files).
pub fn relpath(p: &Path, base: &Path) -> String {
    match p.strip_prefix(base) {
        Ok(r) if r.as_os_str().is_empty() => ".".into(),
        Ok(r) => r.to_string_lossy().into_owned(),
        Err(_) => p.to_string_lossy().into_owned(),
    }
}

/// `os.path.splitext`: `("a/b.c", ".d")`-style split at the last dot of the file name.
pub fn splitext(p: &str) -> (String, String) {
    let name_start = p.rfind('/').map(|i| i + 1).unwrap_or(0);
    let name = &p[name_start..];
    let trimmed = name.trim_start_matches('.');
    match trimmed.rfind('.') {
        Some(i) => {
            let dot = name_start + (name.len() - trimmed.len()) + i;
            (p[..dot].to_string(), p[dot..].to_string())
        }
        None => (p.to_string(), String::new()),
    }
}

/// `os.path.basename`.
pub fn basename(p: &str) -> String {
    p.rsplit('/').next().unwrap_or("").to_string()
}

/// What a local tool raises: Python's exception class name and message (`f"ERROR: {type(e).__name__}: {e}"`).
#[derive(Debug, Clone)]
pub struct LocalErr {
    pub name: &'static str,
    pub msg: String,
}

impl LocalErr {
    pub fn value(msg: impl Into<String>) -> Self {
        Self {
            name: "ValueError",
            msg: msg.into(),
        }
    }
    /// `KeyError('path')` → `'path'`.
    pub fn key(k: &str) -> Self {
        Self {
            name: "KeyError",
            msg: format!("'{}'", k),
        }
    }
    pub fn other(name: &'static str, msg: impl Into<String>) -> Self {
        Self {
            name,
            msg: msg.into(),
        }
    }
}

impl std::fmt::Display for LocalErr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.name, self.msg)
    }
}

impl From<std::io::Error> for LocalErr {
    fn from(e: std::io::Error) -> Self {
        LocalErr::other("OSError", e.to_string())
    }
}

impl From<anyhow::Error> for LocalErr {
    fn from(e: anyhow::Error) -> Self {
        LocalErr::other("Exception", format!("{:#}", e))
    }
}

/// `a[key]` for a required string argument (Python raised `KeyError`).
pub fn req_str(a: &Value, key: &str) -> Result<String, LocalErr> {
    match a.get(key) {
        Some(v) => Ok(py_str(v)),
        None => Err(LocalErr::key(key)),
    }
}

/// `str(v)` of a possibly-missing value: "None" when absent (Python's `str(None)`).
pub fn pystr_opt(v: Option<&Value>) -> String {
    match v {
        Some(v) => py_str(v),
        None => "None".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn g_format() {
        assert_eq!(fmt_g(60.0), "60");
        assert_eq!(fmt_g(31.43), "31.43");
        assert_eq!(fmt_g(3.0), "3");
        assert_eq!(fmt_g(0.5), "0.5");
        assert_eq!(fmt_g(1234567.0), "1.23457e+06");
        assert_eq!(fmt_g(470.0), "470");
    }

    #[test]
    fn ext_split() {
        assert_eq!(
            splitext("reports/a.b.md"),
            ("reports/a.b".into(), ".md".into())
        );
        assert_eq!(
            splitext("reports/flights"),
            ("reports/flights".into(), "".into())
        );
        assert_eq!(splitext(".hidden"), (".hidden".into(), "".into()));
        assert_eq!(basename("images/fox.png"), "fox.png");
    }

    #[test]
    fn sorted_dump() {
        assert_eq!(
            dumps_sorted(&serde_json::json!({"b": 1, "a": {"d": 2, "c": 3}})),
            "{\"a\": {\"c\": 3, \"d\": 2}, \"b\": 1}"
        );
    }
}
