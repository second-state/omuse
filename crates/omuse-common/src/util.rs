//! Small helpers shared by all OMuse services (`app/common/util.py`).

use std::sync::LazyLock;

use regex::Regex;
use serde_json::Value;

/// The version literal of this source tree (`VERSION` in `app/common/util.py`).
pub const SOURCE_VERSION: &str = "0.3.6";

/// Version string shown in the app for a release tag: `locius-v0.3.1`, `home-v0.3.1` or `v0.3.1` → `0.3.1`.
/// Anything that is not a `<product>-v<version>` style tag is returned trimmed, so plain versions pass through.
pub fn release_version(tag: &str) -> String {
    static RE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"^(?:[A-Za-z0-9_.]+-)?v(\d.*)$").unwrap());
    let t = tag.trim();
    match RE.captures(t) {
        Some(c) => c[1].to_string(),
        None => t.to_string(),
    }
}

/// The version the app reports: release images bake the git tag in at build time (`OMUSE_VERSION` build
/// argument, see the Dockerfiles); the source literal is the fallback for untagged builds.
pub static VERSION: LazyLock<String> = LazyLock::new(|| {
    let baked = option_env!("OMUSE_VERSION").unwrap_or("");
    let v = release_version(baked);
    if v.is_empty() {
        SOURCE_VERSION.to_string()
    } else {
        v
    }
});

/// Seconds since the Unix epoch, fractional (Python `time.time()`).
pub fn now_ts() -> f64 {
    let d = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    d.as_secs_f64()
}

/// Monotonic seconds (Python `time.monotonic()`), from the first call.
pub fn monotonic() -> f64 {
    static START: LazyLock<std::time::Instant> = LazyLock::new(std::time::Instant::now);
    START.elapsed().as_secs_f64()
}

/// Python `datetime.fromtimestamp(ts, tz=timezone.utc).isoformat()`:
/// `2026-10-23T12:00:00+00:00`, with `.ffffff` microseconds only when they are not zero.
pub fn iso(ts: Option<f64>) -> String {
    let ts = ts.unwrap_or_else(now_ts);
    let secs = ts.floor();
    let micros = ((ts - secs) * 1_000_000.0).round() as i64;
    let (secs, micros) = if micros >= 1_000_000 {
        (secs + 1.0, 0)
    } else {
        (secs, micros)
    };
    let dt = chrono::DateTime::from_timestamp(secs as i64, (micros * 1000) as u32)
        .unwrap_or_else(|| chrono::DateTime::from_timestamp(0, 0).unwrap());
    if micros == 0 {
        dt.format("%Y-%m-%dT%H:%M:%S+00:00").to_string()
    } else {
        dt.format("%Y-%m-%dT%H:%M:%S%.6f+00:00").to_string()
    }
}

/// `"<prefix>_<12 hex>"` (Python `new_id`).
pub fn new_id(prefix: &str) -> String {
    let u = uuid::Uuid::new_v4().simple().to_string();
    format!("{}_{}", prefix, &u[..12])
}

/// `uuid.uuid4().hex` — 32 lowercase hex characters.
pub fn uuid_hex() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}

/// Python `json.dumps(obj, ensure_ascii=False, default=str)` (see [`crate::pyjson`]).
pub fn dumps(v: &Value) -> String {
    crate::pyjson::dumps(v)
}

/// Python `loads(s, default)`: `None`/empty → default, invalid JSON → default.
pub fn loads(s: Option<&str>, default: Value) -> Value {
    match s {
        None | Some("") => default,
        Some(s) => serde_json::from_str(s).unwrap_or(default),
    }
}

/// Constant-time token comparison; false when either side is empty (Python `token_ok`).
pub fn token_ok(given: Option<&str>, expected: &str) -> bool {
    match given {
        Some(g) if !g.is_empty() && !expected.is_empty() => {
            use subtle::ConstantTimeEq;
            g.as_bytes().ct_eq(expected.as_bytes()).into()
        }
        _ => false,
    }
}

/// `secrets.token_urlsafe(n)`: `n` random bytes, URL-safe base64 without padding.
pub fn token_urlsafe(n: usize) -> String {
    use base64::Engine;
    use rand::RngCore;
    let mut b = vec![0u8; n];
    rand::thread_rng().fill_bytes(&mut b);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(b)
}

/// `secrets.token_hex(n)`.
pub fn token_hex(n: usize) -> String {
    use rand::RngCore;
    let mut b = vec![0u8; n];
    rand::thread_rng().fill_bytes(&mut b);
    hex::encode(b)
}

/// `secrets.token_urlsafe(32)` (Python `random_token`).
pub fn random_token() -> String {
    token_urlsafe(32)
}

/// Number of Unicode scalar values (Python `len(str)`).
pub fn char_len(s: &str) -> usize {
    s.chars().count()
}

/// Python `s[:n]` — the first `n` characters (not bytes).
pub fn take_chars(s: &str, n: usize) -> &str {
    match s.char_indices().nth(n) {
        Some((i, _)) => &s[..i],
        None => s,
    }
}

/// Python `s[-n:]` — the last `n` characters.
pub fn last_chars(s: &str, n: usize) -> &str {
    let total = s.chars().count();
    if n >= total {
        return s;
    }
    let skip = total - n;
    match s.char_indices().nth(skip) {
        Some((i, _)) => &s[i..],
        None => s,
    }
}

/// Python `s[a:b]` on characters (clamped).
pub fn slice_chars(s: &str, a: usize, b: usize) -> &str {
    let b = b.max(a);
    let start = s.char_indices().nth(a).map(|(i, _)| i).unwrap_or(s.len());
    let end = s.char_indices().nth(b).map(|(i, _)| i).unwrap_or(s.len());
    &s[start..end]
}

/// Python `truncate(text, limit)`: cut to `limit` characters and append the bilingual note.
pub fn truncate(text: &str, limit: usize) -> String {
    let n = char_len(text);
    if n <= limit {
        return text.to_string();
    }
    format!(
        "{}\n…[已截断 truncated, 共 {} 字符 chars]",
        take_chars(text, limit),
        n
    )
}

/// `str(v)` for JSON values the way Python prints them when interpolated into text.
pub fn py_str(v: &Value) -> String {
    match v {
        Value::Null => "None".to_string(),
        Value::Bool(true) => "True".to_string(),
        Value::Bool(false) => "False".to_string(),
        Value::String(s) => s.clone(),
        Value::Number(_) => crate::pyjson::dumps(v),
        _ => crate::pyjson::py_repr(v),
    }
}

/// `v.get(key)` as a string: strings as-is, numbers/bools/null via [`py_str`] (null → "").
pub fn get_str(v: &Value, key: &str) -> String {
    match v.get(key) {
        None | Some(Value::Null) => String::new(),
        Some(Value::String(s)) => s.clone(),
        Some(other) => py_str(other),
    }
}

/// Python truthiness of a JSON value.
pub fn truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().map(|f| f != 0.0).unwrap_or(true),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

/// `v.get(key)` with Python truthiness (missing → false).
pub fn get_truthy(v: &Value, key: &str) -> bool {
    v.get(key).map(truthy).unwrap_or(false)
}

/// `float(v)` for numbers and numeric strings; `None` otherwise.
pub fn as_f64(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().parse::<f64>().ok(),
        Value::Bool(b) => Some(if *b { 1.0 } else { 0.0 }),
        _ => None,
    }
}

/// `int(v)` for numbers and numeric strings; `None` otherwise.
pub fn as_i64(v: &Value) -> Option<i64> {
    match v {
        Value::Number(n) => n.as_i64().or_else(|| n.as_f64().map(|f| f as i64)),
        Value::String(s) => {
            let t = s.trim();
            t.parse::<i64>()
                .ok()
                .or_else(|| t.parse::<f64>().ok().map(|f| f as i64))
        }
        Value::Bool(b) => Some(*b as i64),
        _ => None,
    }
}

/// Create a directory tree, ignoring "already exists" (Python `os.makedirs(exist_ok=True)`).
pub fn makedirs(p: impl AsRef<std::path::Path>) -> std::io::Result<()> {
    std::fs::create_dir_all(p)
}

/// Write a file atomically (`tmp` + rename) with the given mode (Unix).
pub fn write_atomic(path: &std::path::Path, data: &[u8], mode: u32) -> std::io::Result<()> {
    use std::io::Write;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension(format!(
        "{}tmp",
        path.extension()
            .map(|e| format!("{}.", e.to_string_lossy()))
            .unwrap_or_default()
    ));
    {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(data)?;
        f.sync_all()?;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(mode))?;
    }
    #[cfg(not(unix))]
    let _ = mode;
    std::fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn release_tag_versions() {
        assert_eq!(release_version("locius-v0.3.1"), "0.3.1");
        assert_eq!(release_version("home-v0.3.1"), "0.3.1");
        assert_eq!(release_version("v0.3.1"), "0.3.1");
        assert_eq!(release_version(" 0.3.1 "), "0.3.1");
        assert_eq!(release_version(""), "");
        assert_eq!(release_version("main"), "main");
    }

    #[test]
    fn iso_matches_python() {
        assert_eq!(iso(Some(0.0)), "1970-01-01T00:00:00+00:00");
        assert_eq!(iso(Some(1.5)), "1970-01-01T00:00:01.500000+00:00");
    }

    #[test]
    fn truncate_counts_chars() {
        assert_eq!(truncate("你好世界", 10), "你好世界");
        assert_eq!(
            truncate("你好世界", 2),
            "你好\n…[已截断 truncated, 共 4 字符 chars]"
        );
        assert_eq!(take_chars("héllo", 2), "hé");
        assert_eq!(last_chars("héllo", 2), "lo");
    }

    #[test]
    fn token_compare() {
        assert!(token_ok(Some("abc"), "abc"));
        assert!(!token_ok(Some("abc"), "abd"));
        assert!(!token_ok(None, "abc"));
        assert!(!token_ok(Some("abc"), ""));
    }
}
