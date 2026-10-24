//! MIME / header helpers behind `gmail.rs`: the Python `email` stdlib pieces the connector used
//! (`decode_header`/`make_header`, `message_from_bytes` for header blocks, `formataddr`, `getaddresses`,
//! `make_msgid`, `formatdate`, `parsedate_to_datetime`, `Time2Internaldate`).

use std::sync::LazyLock;

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use chrono::{DateTime, FixedOffset, Local, TimeZone};
use mail_parser::MessageParser;
use regex::Regex;

/// Collapse header folding (CR/LF + whitespace) into single spaces; EmailMessage rejects values with line breaks.
pub fn unfold(v: &str) -> String {
    static RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[\r\n]+[ \t]*").unwrap());
    RE.replace_all(v, " ").trim().to_string()
}

// ------------------------------------------------------------------ RFC 2047 (email.header.decode_header + make_header)

static ECRE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)=\?([^?]*?)\?([qb])\?(.*?)\?=").unwrap());

fn qp_header_decode(s: &str) -> Vec<u8> {
    // email.quoprimime.header_decode: "_" -> space, "=XX" -> byte, anything else verbatim
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        if c == b'_' {
            out.push(b' ');
            i += 1;
        } else if c == b'='
            && i + 2 < b.len()
            && b[i + 1].is_ascii_hexdigit()
            && b[i + 2].is_ascii_hexdigit()
        {
            let hex = &s[i + 1..i + 3];
            match u8::from_str_radix(hex, 16) {
                Ok(v) => {
                    out.push(v);
                    i += 3;
                }
                _ => {
                    out.push(c);
                    i += 1;
                }
            }
        } else {
            out.push(c);
            i += 1;
        }
    }
    out
}

fn charset_decode(bytes: &[u8], charset: &str) -> Option<String> {
    let label = charset.trim().to_lowercase();
    let enc = match label.as_str() {
        "" | "us-ascii" | "ascii" => encoding_rs::WINDOWS_1252,
        "utf8" | "utf-8" => encoding_rs::UTF_8,
        _ => encoding_rs::Encoding::for_label(label.as_bytes())?,
    };
    if label == "us-ascii" || label == "ascii" {
        if !bytes.is_ascii() {
            return None;
        }
        return Some(String::from_utf8_lossy(bytes).into_owned());
    }
    let (txt, _, had_errors) = enc.decode(bytes);
    if had_errors {
        return None; // Python decodes strictly and _dec() falls back to the raw value
    }
    Some(txt.into_owned())
}

fn nonctext(c: char) -> bool {
    c.is_whitespace() || c == '(' || c == ')' || c == '\\'
}

/// `str(make_header(decode_header(value)))` — RFC 2047 words decoded, the raw value when anything fails.
pub fn dec(value: Option<&str>) -> String {
    let Some(value) = value else {
        return String::new();
    };
    if !ECRE.is_match(value) {
        return value.to_string();
    }
    // words: (text, encoding, charset)
    let mut words: Vec<(String, Option<char>, Option<String>)> = Vec::new();
    for line in value.lines() {
        let mut last = 0;
        let mut first = true;
        for m in ECRE.captures_iter(line) {
            let whole = m.get(0).expect("match");
            let mut unencoded = line[last..whole.start()].to_string();
            if first {
                unencoded = unencoded.trim_start().to_string();
                first = false;
            }
            if !unencoded.is_empty() {
                words.push((unencoded, None, None));
            }
            let enc = m[2].to_lowercase().chars().next();
            words.push((m[3].to_string(), enc, Some(m[1].to_lowercase())));
            last = whole.end();
        }
        let mut tail = line[last..].to_string();
        if first {
            tail = tail.trim_start().to_string();
        }
        if !tail.is_empty() {
            words.push((tail, None, None));
        }
    }
    // drop whitespace-only words between two encoded words
    let mut drop: Vec<usize> = Vec::new();
    for n in 0..words.len() {
        if n > 1
            && words[n].1.is_some()
            && words[n - 2].1.is_some()
            && !words[n - 1].0.is_empty()
            && words[n - 1].0.chars().all(char::is_whitespace)
        {
            drop.push(n - 1);
        }
    }
    for d in drop.into_iter().rev() {
        words.remove(d);
    }
    // decode each word to bytes
    let mut decoded: Vec<(Vec<u8>, Option<String>)> = Vec::new();
    for (text, enc, charset) in words {
        match enc {
            None => decoded.push((text.into_bytes(), None)),
            Some('q') => decoded.push((qp_header_decode(&text), charset)),
            Some(_) => {
                let mut t = text.clone();
                let pad = t.len() % 4;
                if pad != 0 {
                    t.push_str(&"==="[..4 - pad]);
                }
                match B64.decode(t.as_bytes()) {
                    Ok(b) => decoded.push((b, charset)),
                    Err(_) => return value.to_string(),
                }
            }
        }
    }
    // collapse consecutive runs of the same charset
    let mut collapsed: Vec<(Vec<u8>, Option<String>)> = Vec::new();
    for (word, charset) in decoded {
        match collapsed.last_mut() {
            Some((last_word, last_cs)) if *last_cs == charset => {
                if charset.is_none() {
                    last_word.push(b' ');
                }
                last_word.extend_from_slice(&word);
            }
            _ => collapsed.push((word, charset)),
        }
    }
    // make_header + Header.__str__
    let mut chunks: Vec<(String, bool)> = Vec::new(); // (text, is_us_ascii)
    for (bytes, charset) in collapsed {
        match charset {
            None => {
                if !bytes.is_ascii() {
                    return value.to_string();
                }
                chunks.push((String::from_utf8_lossy(&bytes).into_owned(), true));
            }
            Some(cs) => match charset_decode(&bytes, &cs) {
                Some(txt) => {
                    let ascii_cs = cs == "us-ascii" || cs == "ascii";
                    chunks.push((txt, ascii_cs));
                }
                None => return value.to_string(),
            },
        }
    }
    let mut out = String::new();
    let mut last_ascii: Option<bool> = None; // None = nothing yet / reset
    let mut lastspace = false;
    for (i, (string, is_ascii)) in chunks.iter().enumerate() {
        let mut next_ascii = *is_ascii;
        if i > 0 {
            let hasspace = string.chars().next().map(nonctext).unwrap_or(false);
            if matches!(last_ascii, Some(false)) {
                if next_ascii && !hasspace {
                    out.push(' ');
                    next_ascii = true;
                }
            } else if !next_ascii && !lastspace {
                out.push(' ');
            }
        }
        lastspace = string.chars().last().map(nonctext).unwrap_or(false);
        last_ascii = Some(next_ascii);
        out.push_str(string);
    }
    out
}

// ------------------------------------------------------------------ header blocks (email.message_from_bytes(...).get(...))

/// Parsed header block: `(name, unfolded value)` in order.
#[derive(Debug, Clone, Default)]
pub struct Headers(pub Vec<(String, String)>);

impl Headers {
    /// First header with that name (case-insensitive), like `Message.get(name)`.
    pub fn get(&self, name: &str) -> Option<&str> {
        self.0
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    /// `get(name)` or `""`.
    pub fn get_or_empty(&self, name: &str) -> String {
        self.get(name).unwrap_or("").to_string()
    }
}

/// Parse the header section of a message (everything up to the first empty line). Folded lines are unfolded
/// to a single space; 8-bit bytes are decoded as UTF-8 (lossy).
pub fn parse_headers(raw: &[u8]) -> Headers {
    let text = String::from_utf8_lossy(raw);
    let mut out: Vec<(String, String)> = Vec::new();
    for line in text.split('\n') {
        let line = line.strip_suffix('\r').unwrap_or(line);
        if line.is_empty() {
            break;
        }
        if line.starts_with(' ') || line.starts_with('\t') {
            if let Some((_, v)) = out.last_mut() {
                v.push(' ');
                v.push_str(line.trim_start());
            }
            continue;
        }
        if let Some((name, value)) = line.split_once(':') {
            out.push((name.trim().to_string(), value.trim().to_string()));
        }
    }
    Headers(out)
}

// ------------------------------------------------------------------ addresses (email.utils)

static SPECIALS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"[\]\[\\()<>@,:;".]"#).unwrap());

fn qp_header_encode(bytes: &[u8]) -> String {
    let mut out = String::new();
    for &b in bytes {
        if b == b' ' {
            out.push('_');
        } else if b.is_ascii_alphanumeric() || b"-!*+/_".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("={b:02X}"));
        }
    }
    out
}

/// `email.utils.formataddr((name, addr))`: `Name <addr>`, quoted when the name has specials, RFC 2047 (utf-8,
/// shortest of B/Q) when it is not ASCII.
pub fn formataddr(name: &str, addr: &str) -> String {
    if name.is_empty() {
        return addr.to_string();
    }
    if name.is_ascii() {
        let quoted = if SPECIALS.is_match(name) {
            format!("\"{}\"", name.replace('\\', "\\\\").replace('"', "\\\""))
        } else {
            name.to_string()
        };
        return format!("{quoted} <{addr}>");
    }
    let bytes = name.as_bytes();
    let b64 = B64.encode(bytes);
    let qp = qp_header_encode(bytes);
    let encoded = if b64.len() < qp.len() {
        format!("=?utf-8?b?{b64}?=")
    } else {
        format!("=?utf-8?q?{qp}?=")
    };
    format!("{encoded} <{addr}>")
}

/// `email.utils.getaddresses(values)`: `(name, address)` pairs from comma-separated address lists.
pub fn getaddresses(values: &[&str]) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for v in values {
        if v.trim().is_empty() {
            continue;
        }
        let raw = format!("To: {}\r\n\r\n", v.replace(['\r', '\n'], " "));
        let Some(msg) = MessageParser::default().parse(raw.as_bytes()) else {
            continue;
        };
        if let Some(addr) = msg.to() {
            for a in addr.iter() {
                let email = a.address().unwrap_or("").to_string();
                if email.is_empty() {
                    continue;
                }
                out.push((a.name().unwrap_or("").to_string(), email));
            }
        }
    }
    out
}

/// Bare addresses of an address list (for the SMTP envelope).
pub fn addresses_only(values: &[&str]) -> Vec<String> {
    getaddresses(values).into_iter().map(|(_, a)| a).collect()
}

// ------------------------------------------------------------------ dates and ids

/// `email.utils.parsedate_to_datetime(d).timestamp()`; `0.0` when unparsable.
pub fn date_key(d: &str) -> f64 {
    let d = d.trim();
    if d.is_empty() {
        return 0.0;
    }
    if let Ok(dt) = DateTime::parse_from_rfc2822(d) {
        return dt.timestamp() as f64;
    }
    // lenient: strip comments, map zone names, allow a missing weekday / seconds
    static COMMENT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\([^)]*\)").unwrap());
    let mut s = COMMENT.replace_all(d, " ").to_string();
    for (name, off) in [
        ("UTC", "+0000"),
        ("GMT", "+0000"),
        ("UT", "+0000"),
        ("Z", "+0000"),
        ("EDT", "-0400"),
        ("EST", "-0500"),
        ("CDT", "-0500"),
        ("CST", "-0600"),
        ("MDT", "-0600"),
        ("MST", "-0700"),
        ("PDT", "-0700"),
        ("PST", "-0800"),
        ("ADT", "-0300"),
        ("AST", "-0400"),
    ] {
        let re = Regex::new(&format!(r"\b{name}\b$")).expect("zone regex");
        if re.is_match(s.trim()) {
            s = re.replace(s.trim(), off).to_string();
            break;
        }
    }
    let s = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if let Ok(dt) = DateTime::parse_from_rfc2822(&s) {
        return dt.timestamp() as f64;
    }
    if let Some(dt) = mail_parser::DateTime::parse_rfc822(&s) {
        return dt.to_timestamp() as f64;
    }
    // "21 Sep 2026 10:00 GMT" style without seconds
    static NOSEC: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"^(?:[A-Za-z]{3},?\s+)?(\d{1,2}\s+[A-Za-z]{3}\s+\d{4})\s+(\d{1,2}:\d{2})(?::(\d{2}))?\s*([+-]\d{4})?$").unwrap()
    });
    if let Some(m) = NOSEC.captures(&s) {
        let sec = m.get(3).map(|x| x.as_str()).unwrap_or("00");
        let zone = m.get(4).map(|x| x.as_str()).unwrap_or("+0000");
        let txt = format!("{} {}:{} {}", &m[1], &m[2], sec, zone);
        if let Ok(dt) = DateTime::parse_from_str(&txt, "%e %b %Y %H:%M:%S %z") {
            return dt.timestamp() as f64;
        }
    }
    0.0
}

/// `email.utils.formatdate(localtime=True)`.
pub fn format_date_local() -> String {
    Local::now().format("%a, %d %b %Y %H:%M:%S %z").to_string()
}

/// `email.utils.make_msgid(domain=...)`.
pub fn make_msgid(domain: &str) -> String {
    let ts = (omuse_common::util::now_ts() * 100.0) as i64;
    let pid = std::process::id();
    let r: u64 = rand::random();
    format!("<{ts}.{pid}.{r}@{domain}>")
}

/// `imaplib.Time2Internaldate(time.time())` (quoted, e.g. `"23-Oct-2026 10:00:00 +0800"`).
pub fn internaldate_now() -> String {
    format!("\"{}\"", Local::now().format("%d-%b-%Y %H:%M:%S %z"))
}

/// Local-time `DateTime` for a unix timestamp (used by tests).
pub fn local_from_ts(ts: i64) -> Option<DateTime<FixedOffset>> {
    Local
        .timestamp_opt(ts, 0)
        .single()
        .map(|d| d.fixed_offset())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_header_words() {
        assert_eq!(
            dec(Some("=?utf-8?b?6LSi5Yqh?= <finance@acme.com>")),
            "财务 <finance@acme.com>"
        );
        assert_eq!(dec(Some("=?utf-8?b?6LSi5Yqh?=<x@y>")), "财务 <x@y>");
        assert_eq!(dec(Some("abc =?utf-8?q?caf=C3=A9?= def")), "abc café def");
        assert_eq!(
            dec(Some("=?utf-8?b?5Y+R56Wo?= =?utf-8?b?5oql6ZSA?=")),
            "发票报销"
        );
        assert_eq!(dec(Some("=?gb2312?b?1tC5+g==?= (x)")), "中国 (x)");
        assert_eq!(dec(Some("plain")), "plain");
        assert_eq!(dec(None), "");
        assert_eq!(dec(Some("=?utf-8?b?!!!?= x")), "=?utf-8?b?!!!?= x"); // undecodable -> raw
        assert_eq!(dec(Some("=?nosuchcs?q?ab?=")), "=?nosuchcs?q?ab?=");
    }

    #[test]
    fn header_block() {
        let h = parse_headers(b"From: John <john@acme.com>\r\nSubject: Meeting\r\n Tuesday?\r\nList-Unsubscribe: <a>,\r\n <b>\r\n\r\nbody: x\r\n");
        assert_eq!(h.get("from"), Some("John <john@acme.com>"));
        assert_eq!(h.get("Subject"), Some("Meeting Tuesday?"));
        assert_eq!(h.get("List-Unsubscribe"), Some("<a>, <b>"));
        assert_eq!(h.get("body"), None);
        assert_eq!(unfold("a\r\n  b\n\tc "), "a b c");
    }

    #[test]
    fn addresses() {
        assert_eq!(formataddr("Lucas Lu", "a@b.c"), "Lucas Lu <a@b.c>");
        assert_eq!(formataddr("Lu, Lucas", "a@b.c"), "\"Lu, Lucas\" <a@b.c>");
        assert_eq!(formataddr("财务", "a@b.c"), "=?utf-8?b?6LSi5Yqh?= <a@b.c>");
        assert_eq!(
            formataddr("café x", "a@b.c"),
            "=?utf-8?q?caf=C3=A9_x?= <a@b.c>"
        );
        assert_eq!(formataddr("", "a@b.c"), "a@b.c");
        assert_eq!(
            getaddresses(&["Boss <boss@acme.com>, \"Lu, L\" <l@x.com>", "x@y.com"]),
            vec![
                ("Boss".to_string(), "boss@acme.com".to_string()),
                ("Lu, L".to_string(), "l@x.com".to_string()),
                ("".to_string(), "x@y.com".to_string())
            ]
        );
    }

    #[test]
    fn dates() {
        assert_eq!(date_key("Mon, 21 Sep 2026 10:00:00 +0800"), 1789956000.0);
        assert_eq!(date_key("21 Sep 2026 10:00 GMT"), 1789984800.0);
        assert_eq!(date_key("Wed, 1 Oct 2026 08:00:00 PST"), 1790870400.0);
        assert_eq!(
            date_key("Thu, 23 Oct 2026 10:00:00 +0000 (UTC)"),
            1792749600.0
        );
        assert_eq!(date_key("bad"), 0.0);
        assert_eq!(date_key(""), 0.0);
        assert!(make_msgid("x.com").ends_with("@x.com>"));
        assert!(internaldate_now().starts_with('"'));
    }
}
