//! Email providers other than Gmail: server presets, Gmail-style search -> IMAP SEARCH, modified UTF-7 folder names,
//! and Microsoft (Outlook.com / Hotmail / Microsoft 365) OAuth 2.0 device-code sign-in.
//!
//! Every provider except Outlook works with IMAP + SMTP and an app password / authorization code (授权码).
//! Microsoft turned off password sign-in for Outlook.com in Sept 2024, so Outlook uses OAuth 2.0 (XOAUTH2) with the
//! user's own app registration (client ID) — the same "bring your own key" model as Google Calendar.
//!
//! Port of `app/sentinel/mailproviders.py`.

use std::collections::HashMap;
use std::sync::LazyLock;

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use chrono::{Datelike, Duration, NaiveDate};
use regex::Regex;
use serde_json::{json, Value};

use omuse_common::util::now_ts;

// ------------------------------------------------------------------ presets

/// One provider preset. `imap` / `smtp` are `(host, port, security)`; security is `"ssl"` (implicit TLS),
/// `"starttls"` or `"none"` (loopback hosts only). Hosts may contain `{domain}` (NetEase).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Provider {
    pub key: &'static str,
    pub label: &'static str,
    pub imap: (&'static str, u16, &'static str),
    pub smtp: (&'static str, u16, &'static str),
    pub auth: &'static str,
    pub domains: &'static [&'static str],
    pub help: &'static str,
}

/// The `PROVIDERS` table, in the Python dict order (the UI shows the picker in this order).
pub static PROVIDERS: &[Provider] = &[
    Provider {
        key: "gmail",
        label: "Gmail",
        imap: ("imap.gmail.com", 993, "ssl"),
        smtp: ("smtp.gmail.com", 465, "ssl"),
        auth: "password",
        domains: &["gmail.com", "googlemail.com"],
        help: "https://myaccount.google.com/apppasswords",
    },
    Provider {
        key: "outlook",
        label: "Outlook / Hotmail / Microsoft 365",
        imap: ("outlook.office365.com", 993, "ssl"),
        smtp: ("smtp-mail.outlook.com", 587, "starttls"),
        auth: "oauth",
        domains: &[
            "outlook.com",
            "hotmail.com",
            "live.com",
            "msn.com",
            "outlook.sg",
            "hotmail.co.uk",
            "live.cn",
            "outlook.jp",
            "hotmail.fr",
            "live.com.sg",
        ],
        help:
            "https://entra.microsoft.com/#view/Microsoft_AAD_RegisteredApps/ApplicationsListBlade",
    },
    Provider {
        key: "yahoo",
        label: "Yahoo Mail",
        imap: ("imap.mail.yahoo.com", 993, "ssl"),
        smtp: ("smtp.mail.yahoo.com", 465, "ssl"),
        auth: "password",
        domains: &[
            "yahoo.com",
            "ymail.com",
            "rocketmail.com",
            "yahoo.com.sg",
            "yahoo.co.jp",
            "yahoo.co.uk",
            "yahoo.com.hk",
            "yahoo.com.tw",
        ],
        help: "https://login.yahoo.com/account/security",
    },
    Provider {
        key: "icloud",
        label: "iCloud Mail",
        imap: ("imap.mail.me.com", 993, "ssl"),
        smtp: ("smtp.mail.me.com", 587, "starttls"),
        auth: "password",
        domains: &["icloud.com", "me.com", "mac.com"],
        help: "https://account.apple.com",
    },
    Provider {
        key: "qq",
        label: "QQ Mail / Foxmail",
        imap: ("imap.qq.com", 993, "ssl"),
        smtp: ("smtp.qq.com", 465, "ssl"),
        auth: "password",
        domains: &["qq.com", "foxmail.com", "vip.qq.com"],
        help: "https://mail.qq.com",
    },
    Provider {
        key: "netease",
        label: "NetEase Mail 163 / 126 / yeah.net",
        imap: ("imap.{domain}", 993, "ssl"),
        smtp: ("smtp.{domain}", 465, "ssl"),
        auth: "password",
        domains: &["163.com", "126.com", "yeah.net"],
        help: "https://mail.163.com",
    },
    Provider {
        key: "zoho",
        label: "Zoho Mail",
        imap: ("imap.zoho.com", 993, "ssl"),
        smtp: ("smtp.zoho.com", 465, "ssl"),
        auth: "password",
        domains: &["zoho.com", "zohomail.com"],
        help: "https://accounts.zoho.com/home#security/app_password",
    },
    Provider {
        key: "aol",
        label: "AOL Mail",
        imap: ("imap.aol.com", 993, "ssl"),
        smtp: ("smtp.aol.com", 465, "ssl"),
        auth: "password",
        domains: &["aol.com"],
        help: "https://login.aol.com/account/security",
    },
    Provider {
        key: "custom",
        label: "Other (IMAP / SMTP)",
        imap: ("", 993, "ssl"),
        smtp: ("", 465, "ssl"),
        auth: "password",
        domains: &[],
        help: "",
    },
];

/// Allowed transport securities.
pub const SECURITIES: &[&str] = &["ssl", "starttls", "none"];

static LOOPBACK: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^(localhost|127\.\d+\.\d+\.\d+|::1|\[::1\])$").unwrap());

/// Is `host` a loopback address (the only place an unencrypted connection is allowed)?
pub fn is_loopback(host: &str) -> bool {
    LOOPBACK.is_match(host)
}

/// `PROVIDERS.get(key)`.
pub fn provider(key: &str) -> Option<&'static Provider> {
    PROVIDERS.iter().find(|p| p.key == key)
}

/// `PROVIDERS.get(key, PROVIDERS["custom"])`.
pub fn provider_or_custom(key: &str) -> &'static Provider {
    provider(key).unwrap_or_else(|| provider("custom").expect("custom preset"))
}

fn domain_of_addr(email_addr: &str) -> String {
    email_addr
        .rsplit_once('@')
        .map(|(_, d)| d)
        .unwrap_or(email_addr)
        .to_lowercase()
}

/// Provider key for an address, by domain ("" when unknown).
pub fn guess_provider(email_addr: &str) -> String {
    let dom = domain_of_addr(email_addr).trim().to_string();
    PROVIDERS
        .iter()
        .find(|p| p.domains.contains(&dom.as_str()))
        .map(|p| p.key.to_string())
        .unwrap_or_default()
}

/// Human label of a provider (unknown → the "custom" label; "" → Gmail).
pub fn label(provider_key: &str) -> String {
    let key = if provider_key.is_empty() {
        "gmail"
    } else {
        provider_key
    };
    provider_or_custom(key).label.to_string()
}

/// What the Connections page needs to show the provider picker (no secrets here).
pub fn public_presets() -> Vec<Value> {
    PROVIDERS
        .iter()
        .map(|p| {
            json!({
                "key": p.key, "label": p.label, "auth": p.auth, "domains": p.domains, "help": p.help,
                "imap": [p.imap.0, p.imap.1, p.imap.2], "smtp": [p.smtp.0, p.smtp.1, p.smtp.2],
            })
        })
        .collect()
}

/// Python `ValueError` from [`server_settings`] (the message is shown to the user as an HTTP 400).
#[derive(Debug, Clone, thiserror::Error, PartialEq, Eq)]
#[error("{0}")]
pub struct SettingsError(pub String);

fn custom_str(custom: &Value, key: &str) -> String {
    match custom.get(key) {
        None | Some(Value::Null) => String::new(),
        Some(Value::String(s)) => s.clone(),
        Some(v) => omuse_common::util::py_str(v),
    }
}

fn custom_int(custom: &Value, key: &str, default: i64) -> Result<i64, SettingsError> {
    match custom.get(key) {
        None | Some(Value::Null) => Ok(default),
        Some(Value::Bool(false)) => Ok(default),
        Some(Value::Bool(true)) => Ok(1),
        Some(Value::Number(n)) => n
            .as_i64()
            .or_else(|| n.as_f64().map(|f| f as i64))
            .ok_or_else(|| SettingsError(format!("invalid {key}"))),
        Some(Value::String(s)) if s.trim().is_empty() => Ok(default),
        Some(Value::String(s)) => s
            .trim()
            .parse::<i64>()
            .map_err(|_| SettingsError(format!("invalid literal for int() with base 10: '{s}'"))),
        Some(_) => Err(SettingsError(format!("invalid {key}"))),
    }
}

static HOST_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(?:[A-Za-z0-9.\-]{1,253}|\[?::1\]?)$").unwrap());

/// Account server fields for a provider; for "custom" they come from the form (validated here).
pub fn server_settings(
    provider_key: &str,
    email_addr: &str,
    custom: Option<&Value>,
) -> Result<Value, SettingsError> {
    let Some(p) = provider(provider_key) else {
        return Err(SettingsError(format!(
            "unknown email provider: {provider_key}"
        )));
    };
    let dom = domain_of_addr(email_addr);
    let empty = json!({});
    let custom = custom.unwrap_or(&empty);
    let mut out;
    if provider_key == "custom" {
        let imap_host = custom_str(custom, "imap_host").trim().to_string();
        let imap_port = custom_int(custom, "imap_port", 993)?;
        let mut imap_security = custom_str(custom, "imap_security");
        if imap_security.is_empty() {
            imap_security = "ssl".into();
        }
        let smtp_host = custom_str(custom, "smtp_host").trim().to_string();
        let smtp_port = custom_int(custom, "smtp_port", 465)?;
        let mut smtp_security = custom_str(custom, "smtp_security");
        if smtp_security.is_empty() {
            smtp_security = "ssl".into();
        }
        if imap_host.is_empty() || smtp_host.is_empty() {
            return Err(SettingsError(
                "请填写 IMAP 和 SMTP 服务器地址 (IMAP / SMTP host required)".into(),
            ));
        }
        for h in [&imap_host, &smtp_host] {
            if !HOST_RE.is_match(h) {
                return Err(SettingsError(format!("服务器地址无效 invalid host: {h}")));
            }
        }
        for (k, sec, host) in [
            ("imap_security", &imap_security, &imap_host),
            ("smtp_security", &smtp_security, &smtp_host),
        ] {
            if !SECURITIES.contains(&sec.as_str()) {
                return Err(SettingsError(format!("invalid {k}: {sec}")));
            }
            if sec == "none" && !is_loopback(host) {
                return Err(SettingsError(
                    "不加密的连接只允许本机地址 (unencrypted only allowed for localhost)".into(),
                ));
            }
        }
        for (k, port) in [("imap_port", imap_port), ("smtp_port", smtp_port)] {
            if !(1..=65535).contains(&port) {
                return Err(SettingsError(format!("invalid {k}")));
            }
        }
        out = json!({
            "imap_host": imap_host, "imap_port": imap_port, "imap_security": imap_security,
            "smtp_host": smtp_host, "smtp_port": smtp_port, "smtp_security": smtp_security,
        });
    } else {
        let (ih, ip, isec) = p.imap;
        let (sh, sp, ssec) = p.smtp;
        out = json!({
            "imap_host": ih.replace("{domain}", &dom), "imap_port": ip, "imap_security": isec,
            "smtp_host": sh.replace("{domain}", &dom), "smtp_port": sp, "smtp_security": ssec,
        });
        if provider_key == "outlook" && !p.domains.contains(&dom.as_str()) {
            out["smtp_host"] = json!("smtp.office365.com"); // Microsoft 365 work / school accounts
        }
    }
    let user = custom_str(custom, "username").trim().to_string();
    out["username"] = json!(if user.is_empty() {
        email_addr.to_string()
    } else {
        user
    });
    Ok(out)
}

// ------------------------------------------------------------------ modified UTF-7 (RFC 3501 §5.1.3) folder names

static MUTF7: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"&([A-Za-z0-9+,]*)-").unwrap());

fn utf16be_decode(bytes: &[u8]) -> Option<String> {
    if !bytes.len().is_multiple_of(2) {
        return None;
    }
    let units: Vec<u16> = bytes
        .chunks(2)
        .map(|c| u16::from_be_bytes([c[0], c[1]]))
        .collect();
    String::from_utf16(&units).ok()
}

/// Decode a modified-UTF-7 mailbox name (`&g0l6P3ux-` → `草稿箱`); undecodable runs are kept verbatim.
pub fn mutf7_decode(s: &str) -> String {
    MUTF7
        .replace_all(s, |caps: &regex::Captures| {
            let b64 = &caps[1];
            if b64.is_empty() {
                return "&".to_string();
            }
            let mut b = b64.replace(',', "/");
            while !b.len().is_multiple_of(4) {
                b.push('=');
            }
            match B64
                .decode(b.as_bytes())
                .ok()
                .and_then(|bytes| utf16be_decode(&bytes))
            {
                Some(txt) => txt,
                None => caps[0].to_string(),
            }
        })
        .into_owned()
}

/// Encode a mailbox name as modified UTF-7 (`草稿箱` → `&g0l6P3ux-`).
pub fn mutf7_encode(s: &str) -> String {
    let mut out = String::new();
    let mut buf: Vec<char> = Vec::new();
    fn flush(out: &mut String, buf: &mut Vec<char>) {
        if buf.is_empty() {
            return;
        }
        let text: String = buf.iter().collect();
        let bytes: Vec<u8> = text.encode_utf16().flat_map(|u| u.to_be_bytes()).collect();
        let b = B64.encode(&bytes).trim_end_matches('=').replace('/', ",");
        out.push('&');
        out.push_str(&b);
        out.push('-');
        buf.clear();
    }
    for ch in s.chars() {
        let code = ch as u32;
        if (0x20..=0x7E).contains(&code) {
            flush(&mut out, &mut buf);
            if ch == '&' {
                out.push_str("&-");
            } else {
                out.push(ch);
            }
        } else {
            buf.push(ch);
        }
    }
    flush(&mut out, &mut buf);
    out
}

// ------------------------------------------------------------------ Gmail-style query -> IMAP SEARCH

/// `in:`/`label:` words that name a well-known folder (`*` = everywhere).
pub static FOLDER_WORDS: &[(&str, &str)] = &[
    ("inbox", "inbox"),
    ("sent", "sent"),
    ("draft", "drafts"),
    ("drafts", "drafts"),
    ("trash", "trash"),
    ("bin", "trash"),
    ("spam", "spam"),
    ("junk", "spam"),
    ("archive", "archive"),
    ("anywhere", "*"),
    ("all", "*"),
];

const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// `26-Sep-2026` (no zero padding, like the Python).
pub fn imap_date(d: NaiveDate) -> String {
    format!("{}-{}-{}", d.day(), MONTHS[(d.month0()) as usize], d.year())
}

static REL_DATE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(\d+)([dwmy])$").unwrap());
static UNIX_SECS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\d{9,11}$").unwrap());
static ABS_DATE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(\d{4})[/\-.](\d{1,2})[/\-.](\d{1,2})$").unwrap());
static TOKENS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"-?[A-Za-z_]+:"[^"]*"|-?"[^"]*"|[{}()]|[^\s{}()]+"#).unwrap());
static KEY_VAL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^([A-Za-z_]+):(.*)$").unwrap());
static SIZE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^(\d+(?:\.\d+)?)([kmg]?)b?$").unwrap());

fn rel_date(v: &str, today: NaiveDate) -> Option<NaiveDate> {
    let low = v.to_lowercase();
    let m = REL_DATE.captures(&low)?;
    let n: i64 = m[1].parse().ok()?;
    let per = match &m[2] {
        "d" => 1,
        "w" => 7,
        "m" => 30,
        _ => 365,
    };
    today.checked_sub_signed(Duration::days(per * n))
}

fn abs_date(v: &str) -> Option<NaiveDate> {
    let v = v.trim();
    if UNIX_SECS.is_match(v) {
        // unix seconds (Gmail accepts these)
        let secs: i64 = v.parse().ok()?;
        return chrono::DateTime::from_timestamp(secs, 0).map(|dt| dt.date_naive());
    }
    let m = ABS_DATE.captures(v)?;
    let y: i32 = m[1].parse().ok()?;
    let mo: u32 = m[2].parse().ok()?;
    let d: u32 = m[3].parse().ok()?;
    NaiveDate::from_ymd_opt(y, mo, d)
}

fn tokens(q: &str) -> Vec<String> {
    TOKENS
        .find_iter(q)
        .map(|m| m.as_str().to_string())
        .collect()
}

fn unq(v: &str) -> &str {
    if v.len() >= 2 && v.starts_with('"') && v.ends_with('"') {
        &v[1..v.len() - 1]
    } else {
        v
    }
}

/// IMAP quoted string.
pub fn qs(v: &str) -> String {
    format!("\"{}\"", v.replace('\\', "\\\\").replace('"', "\\\""))
}

/// A non-ASCII search term the server did not get: `(field, value, negated, OR-group id)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextTerm {
    pub key: String,
    pub val: String,
    pub neg: bool,
    pub gid: usize,
}

/// Result of [`translate_query`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Translated {
    /// Folder words: `inbox`, `sent`, `drafts`, `trash`, `spam`, `archive`, `*` or `label:<name>`.
    pub folders: Vec<String>,
    /// Complete IMAP search keys (ASCII).
    pub criteria: Vec<String>,
    /// Non-ASCII terms (one goes to the server as a UTF-8 literal, the rest are filtered locally).
    pub text: Vec<TextTerm>,
    /// What was ignored / approximated.
    pub notes: Vec<String>,
}

/// Gmail search syntax -> `{"folders": [...], "criteria": [str…] (ASCII), "text": [(field, value)…] (non-ASCII terms,
/// one goes to the server as a UTF-8 literal, the rest are filtered locally), "notes": [...]}`.
///
/// Supported: in:/label:, from: to: cc: bcc: subject:, is:unread/read/starred/important/flagged, has:attachment,
/// newer_than: older_than: after: before: (newer/older), larger: smaller:, category:promotions/social/updates/forums/
/// primary (approximated by the List-Unsubscribe header), "phrases", plain words, -negation, OR and {a b}.
pub fn translate_query(query: &str, today: Option<NaiveDate>) -> Translated {
    let today = today.unwrap_or_else(|| chrono::Local::now().date_naive());
    let mut folders: Vec<String> = Vec::new();
    let mut notes: Vec<String> = Vec::new();
    let mut text: Vec<TextTerm> = Vec::new();
    let mut items: Vec<String> = Vec::new(); // each item is a complete IMAP search key
    let mut pending_or = false;
    // OR-grouping for non-ASCII terms: the server only takes ASCII keys, so Chinese words are filtered locally.
    // Terms that share a group id are OR-ed by local_match; terms in different groups are AND-ed. Without this,
    // ("笔试" OR "在线测评") was filtered as "笔试" AND "在线测评" and matched almost nothing (2026-10-05 colleague bug).
    let mut text_gid: usize = 0;
    let mut last_text_gid: Option<usize> = None;
    let mut last_added_text = false;
    let mut in_brace = false;
    let mut brace_gid: Option<usize> = None;
    let mut group: Option<Vec<String>> = None;

    struct Push<'a> {
        items: &'a mut Vec<String>,
        pending_or: &'a mut bool,
        last_added_text: &'a mut bool,
    }
    fn push(p: Push<'_>, crit: String) {
        if *p.pending_or && !p.items.is_empty() {
            let last = p.items.pop().unwrap_or_default();
            p.items.push(format!("OR {last} {crit}"));
            *p.pending_or = false;
        } else {
            p.items.push(crit);
        }
        *p.last_added_text = false;
    }

    let toks = tokens(query);
    for raw_tok in toks {
        let mut t = raw_tok.as_str();
        if t == "{" {
            group = Some(Vec::new());
            in_brace = true;
            brace_gid = None;
            continue;
        }
        if t == "}" {
            if let Some(g) = group.take() {
                if !g.is_empty() {
                    let mut crit = g[0].clone();
                    for x in &g[1..] {
                        crit = format!("OR {crit} {x}");
                    }
                    push(
                        Push {
                            items: &mut items,
                            pending_or: &mut pending_or,
                            last_added_text: &mut last_added_text,
                        },
                        crit,
                    );
                }
            }
            in_brace = false;
            brace_gid = None;
            continue;
        }
        if t == "(" || t == ")" || t == "AND" {
            continue;
        }
        if t == "OR" {
            pending_or = true;
            continue;
        }
        let neg = t.starts_with('-') && t.len() > 1;
        if neg {
            t = &t[1..];
        }
        // add_text(key, val, neg) as a closure over the loop state
        let mut add_text = |key: &str, val: &str, neg: bool| {
            let gid = if in_brace {
                // inside { … }: one OR group (allocated on first member)
                if brace_gid.is_none() {
                    text_gid += 1;
                    brace_gid = Some(text_gid);
                }
                brace_gid.unwrap_or(text_gid)
            } else if pending_or && last_added_text && last_text_gid.is_some() {
                // "a OR b" between two non-ASCII terms
                last_text_gid.unwrap_or(text_gid)
            } else {
                text_gid += 1;
                text_gid
            };
            text.push(TextTerm {
                key: key.to_string(),
                val: val.to_string(),
                neg,
                gid,
            });
            last_text_gid = Some(gid);
            last_added_text = true;
            pending_or = false;
        };
        let crit: Option<String>;
        let kv = KEY_VAL.captures(t);
        if let (Some(m), false) = (kv, t.starts_with('"')) {
            let key = m[1].to_lowercase();
            let val = unq(&m[2]).to_string();
            match key.as_str() {
                "in" | "label" => {
                    let v = val.to_lowercase();
                    if v == "starred" || v == "important" {
                        crit = Some("FLAGGED".into());
                    } else if neg {
                        continue; // -in:spam etc.: spam / trash are not searched unless asked for
                    } else {
                        let word = FOLDER_WORDS
                            .iter()
                            .find(|(k, _)| *k == v)
                            .map(|(_, f)| f.to_string());
                        folders.push(word.unwrap_or_else(|| format!("label:{val}")));
                        continue;
                    }
                }
                "from" | "to" | "cc" | "bcc" | "subject" => {
                    if val.is_empty() {
                        continue;
                    }
                    if val.is_ascii() {
                        crit = Some(format!("{} {}", key.to_uppercase(), qs(&val)));
                    } else {
                        add_text(&key.to_uppercase(), &val, neg);
                        continue;
                    }
                }
                "is" => {
                    crit = match val.to_lowercase().as_str() {
                        "unread" => Some("UNSEEN".into()),
                        "read" => Some("SEEN".into()),
                        "starred" | "important" | "flagged" => Some("FLAGGED".into()),
                        "answered" | "replied" => Some("ANSWERED".into()),
                        _ => None,
                    };
                    if crit.is_none() {
                        notes.push(format!("is:{val} ignored"));
                        continue;
                    }
                }
                "has" => {
                    let v = val.to_lowercase();
                    if v == "attachment" || v == "attachments" {
                        crit = Some("HEADER Content-Type \"multipart/mixed\"".into());
                    } else if [
                        "userlabels",
                        "nouserlabels",
                        "drive",
                        "document",
                        "spreadsheet",
                        "presentation",
                        "youtube",
                        "yellow-star",
                    ]
                    .contains(&v.as_str())
                    {
                        notes.push(format!("has:{val} ignored"));
                        continue;
                    } else {
                        continue;
                    }
                }
                "newer_than" | "older_than" => {
                    let Some(d) = rel_date(&val, today) else {
                        notes.push(format!("{key}:{val} ignored"));
                        continue;
                    };
                    crit = Some(if key == "newer_than" {
                        format!("SINCE {}", imap_date(d))
                    } else {
                        format!("BEFORE {}", imap_date(d))
                    });
                }
                "after" | "before" | "newer" | "older" => {
                    let Some(d) = abs_date(&val) else {
                        notes.push(format!("{key}:{val} ignored"));
                        continue;
                    };
                    crit = Some(if key == "after" || key == "newer" {
                        format!("SINCE {}", imap_date(d))
                    } else {
                        format!("BEFORE {}", imap_date(d))
                    });
                }
                "larger" | "smaller" | "size" => {
                    let Some(sm) = SIZE.captures(&val) else {
                        continue;
                    };
                    let num: f64 = sm[1].parse().unwrap_or(0.0);
                    let mult = match sm[2].to_lowercase().as_str() {
                        "k" => 1024.0,
                        "m" => 1024.0 * 1024.0,
                        "g" => 1024.0 * 1024.0 * 1024.0,
                        _ => 1.0,
                    };
                    let n = (num * mult) as i64;
                    crit = Some(format!(
                        "{} {}",
                        if key == "smaller" {
                            "SMALLER"
                        } else {
                            "LARGER"
                        },
                        n
                    ));
                }
                "category" => {
                    let v = val.to_lowercase();
                    if ["promotions", "social", "updates", "forums"].contains(&v.as_str()) {
                        crit = Some("HEADER List-Unsubscribe \"\"".into());
                    } else if v == "primary" || v == "personal" {
                        crit = Some("NOT HEADER List-Unsubscribe \"\"".into());
                    } else {
                        continue;
                    }
                    notes.push(
                        "category:* approximated by the List-Unsubscribe header (bulk mail)".into(),
                    );
                }
                "filename" | "list" | "deliveredto" | "rfc822msgid" => {
                    crit = match key.as_str() {
                        "list" => Some(format!("HEADER List-Id {}", qs(&val))),
                        "deliveredto" => {
                            if val.is_ascii() {
                                Some(format!("HEADER Delivered-To {}", qs(&val)))
                            } else {
                                None
                            }
                        }
                        "rfc822msgid" => Some(format!("HEADER Message-ID {}", qs(&val))),
                        _ => {
                            if val.is_ascii() {
                                Some(format!("BODY {}", qs(&val)))
                            } else {
                                None
                            }
                        }
                    };
                    if crit.is_none() {
                        continue;
                    }
                }
                _ => {
                    // unknown operator: treat as a plain word
                    let val = t;
                    if val.is_ascii() {
                        crit = Some(format!("TEXT {}", qs(val)));
                    } else {
                        add_text("TEXT", val, neg);
                        continue;
                    }
                }
            }
        } else {
            let val = unq(t);
            if val.is_empty() {
                continue;
            }
            if val.is_ascii() {
                crit = Some(format!("TEXT {}", qs(val)));
            } else {
                add_text("TEXT", val, neg);
                continue;
            }
        }
        let mut crit = crit.unwrap_or_default();
        if neg {
            crit = format!("NOT {crit}");
        }
        if let Some(g) = group.as_mut() {
            g.push(crit);
        } else {
            push(
                Push {
                    items: &mut items,
                    pending_or: &mut pending_or,
                    last_added_text: &mut last_added_text,
                },
                crit,
            );
        }
    }
    // de-duplicate folders, keep order
    let mut fl: Vec<String> = Vec::new();
    for f in folders {
        if !fl.contains(&f) {
            fl.push(f);
        }
    }
    let mut uniq_notes: Vec<String> = Vec::new();
    for n in notes {
        if !uniq_notes.contains(&n) {
            uniq_notes.push(n);
        }
    }
    // no folder given: the inbox plus the archive folder (where "archive" moves mail to), like Gmail's default search
    Translated {
        folders: if fl.is_empty() {
            vec!["inbox".into(), "archive".into()]
        } else {
            fl
        },
        criteria: items,
        text,
        notes: uniq_notes,
    }
}

fn msg_str(msg: &Value, key: &str) -> String {
    match msg.get(key) {
        None | Some(Value::Null) => String::new(),
        Some(Value::String(s)) => s.clone(),
        Some(v) => omuse_common::util::py_str(v),
    }
}

/// Filter for the non-ASCII terms the server did not get (subject / from / to / snippet, case-insensitive).
///
/// Terms sharing a group id match if ANY of them hits (OR); separate groups must all hit (AND); a negative term
/// must NOT hit.
pub fn local_match(msg: &Value, terms: &[TextTerm]) -> bool {
    let hay = |key: &str| -> String {
        match key {
            "FROM" => msg_str(msg, "from"),
            "TO" => msg_str(msg, "to"),
            "CC" => msg_str(msg, "cc"),
            "BCC" => String::new(),
            "SUBJECT" => msg_str(msg, "subject"),
            _ => [
                msg_str(msg, "subject"),
                msg_str(msg, "from"),
                msg_str(msg, "to"),
                msg_str(msg, "snippet"),
                msg_str(msg, "body"),
            ]
            .join(" "),
        }
    };
    let mut groups: Vec<(usize, Vec<(&str, &str)>)> = Vec::new();
    for t in terms {
        if t.neg {
            // every negative is its own AND constraint
            if hay(&t.key).to_lowercase().contains(&t.val.to_lowercase()) {
                return false;
            }
            continue;
        }
        match groups.iter_mut().find(|(g, _)| *g == t.gid) {
            Some((_, lst)) => lst.push((&t.key, &t.val)),
            None => groups.push((t.gid, vec![(&t.key, &t.val)])),
        }
    }
    for (_, lst) in groups {
        // each positive group: at least one member must hit
        if !lst
            .iter()
            .any(|(k, v)| hay(k).to_lowercase().contains(&v.to_lowercase()))
        {
            return false;
        }
    }
    true
}

// ------------------------------------------------------------------ Microsoft OAuth 2.0 (device code flow)

/// Default Microsoft identity platform URL (`[endpoints] ms_login` in config.toml overrides it).
pub const MS_LOGIN_DEFAULT: &str = "https://login.microsoftonline.com";
/// Scopes requested for IMAP + SMTP with a refresh token.
pub const MS_SCOPES: &str =
    "offline_access https://outlook.office.com/IMAP.AccessAsUser.All https://outlook.office.com/SMTP.Send";

/// OAuth failure (message is user-facing).
#[derive(Debug, Clone, thiserror::Error, PartialEq, Eq)]
#[error("{0}")]
pub struct OAuthError(pub String);

/// `consumers` for personal Microsoft accounts, `organizations` for work / school.
pub fn ms_tenant(email_addr: &str) -> String {
    let dom = domain_of_addr(email_addr);
    let outlook = provider("outlook").expect("outlook preset");
    if outlook.domains.contains(&dom.as_str()) {
        "consumers"
    } else {
        "organizations"
    }
    .to_string()
}

fn ms_err(status: u16, body: &str) -> String {
    match serde_json::from_str::<Value>(body) {
        Ok(j) if j.is_object() => {
            let err = match j.get("error") {
                Some(Value::String(s)) => s.clone(),
                Some(Value::Null) | None => status.to_string(),
                Some(v) => omuse_common::util::py_str(v),
            };
            let desc = msg_str(&j, "error_description");
            let first = desc.lines().next().unwrap_or("");
            format!("{err}: {}", omuse_common::util::take_chars(first, 300))
        }
        _ => format!("HTTP {status}"),
    }
}

/// Microsoft identity platform client (device code + refresh). `url` is `cfg.endpoints.ms_login`.
#[derive(Clone, Debug)]
pub struct MsLogin {
    pub url: String,
    http: reqwest::Client,
}

static GUID: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[0-9a-fA-F\-]{36}$").unwrap());

impl MsLogin {
    /// `MsLogin::new(cfg.endpoints.ms_login.clone())`.
    pub fn new(url: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            http: omuse_common::http::client(20.0),
        }
    }

    async fn post_form(
        &self,
        path: &str,
        form: &[(&str, &str)],
    ) -> Result<(u16, String), OAuthError> {
        let r = self
            .http
            .post(format!("{}{}", self.url.trim_end_matches('/'), path))
            .form(form)
            .send()
            .await
            .map_err(|e| OAuthError(format!("{e}")))?;
        let status = r.status().as_u16();
        let body = r.text().await.unwrap_or_default();
        Ok((status, body))
    }

    /// Start the device code flow: `{device_code, user_code, verification_uri, expires_at, interval}`.
    pub async fn device_start(&self, client_id: &str, tenant: &str) -> Result<Value, OAuthError> {
        if !GUID.is_match(client_id) {
            return Err(OAuthError(
                "应用程序(客户端) ID 格式不对，应是 36 位的 GUID (Application (client) ID must be a GUID)".into(),
            ));
        }
        let (status, body) = self
            .post_form(
                &format!("/{tenant}/oauth2/v2.0/devicecode"),
                &[("client_id", client_id), ("scope", MS_SCOPES)],
            )
            .await
            .map_err(|e| OAuthError(format!("无法开始微软登录 (device code failed): {}", e.0)))?;
        if status != 200 {
            return Err(OAuthError(format!(
                "无法开始微软登录 (device code failed): {}",
                ms_err(status, &body)
            )));
        }
        let j: Value = serde_json::from_str(&body).map_err(|e| OAuthError(format!("{e}")))?;
        let num = |k: &str, d: i64| -> i64 {
            match j.get(k) {
                Some(Value::Number(n)) => n
                    .as_i64()
                    .or_else(|| n.as_f64().map(|f| f as i64))
                    .unwrap_or(d),
                Some(Value::String(s)) => s.parse().unwrap_or(d),
                _ => d,
            }
        };
        let uri = msg_str(&j, "verification_uri");
        Ok(json!({
            "device_code": j.get("device_code").cloned().unwrap_or(Value::Null),
            "user_code": j.get("user_code").cloned().unwrap_or(Value::Null),
            "verification_uri": if uri.is_empty() { "https://microsoft.com/devicelogin".to_string() } else { uri },
            "expires_at": now_ts() + num("expires_in", 900) as f64,
            "interval": num("interval", 5),
        }))
    }

    /// `None` while the user has not finished signing in; the token dict when done; `OAuthError` on failure.
    pub async fn device_poll(
        &self,
        client_id: &str,
        tenant: &str,
        device_code: &str,
    ) -> Result<Option<Value>, OAuthError> {
        let (status, body) = self
            .post_form(
                &format!("/{tenant}/oauth2/v2.0/token"),
                &[
                    ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
                    ("client_id", client_id),
                    ("device_code", device_code),
                ],
            )
            .await
            .map_err(|e| OAuthError(format!("微软登录失败 (sign-in failed): {}", e.0)))?;
        if status == 200 {
            return serde_json::from_str::<Value>(&body)
                .map(Some)
                .map_err(|e| OAuthError(format!("{e}")));
        }
        let err = serde_json::from_str::<Value>(&body)
            .ok()
            .map(|j| msg_str(&j, "error"))
            .unwrap_or_default();
        match err.as_str() {
            "authorization_pending" | "slow_down" => Ok(None),
            "expired_token" => Err(OAuthError("登录码已过期，请重新开始 (code expired)".into())),
            "access_denied" | "authorization_declined" => Err(OAuthError(
                "你在微软页面上拒绝了授权 (access denied)".into(),
            )),
            _ => Err(OAuthError(format!(
                "微软登录失败 (sign-in failed): {}",
                ms_err(status, &body)
            ))),
        }
    }

    /// Exchange a refresh token for a new access token (Microsoft may rotate the refresh token).
    pub async fn refresh(
        &self,
        client_id: &str,
        tenant: &str,
        refresh_token: &str,
    ) -> Result<Value, OAuthError> {
        let (status, body) = self
            .post_form(
                &format!("/{tenant}/oauth2/v2.0/token"),
                &[
                    ("grant_type", "refresh_token"),
                    ("client_id", client_id),
                    ("refresh_token", refresh_token),
                    ("scope", MS_SCOPES),
                ],
            )
            .await
            .map_err(|e| {
                OAuthError(format!(
                    "微软授权已失效，请在「连接」页重新登录 (refresh failed): {}",
                    e.0
                ))
            })?;
        if status != 200 {
            return Err(OAuthError(format!(
                "微软授权已失效，请在「连接」页重新登录 (refresh failed): {}",
                ms_err(status, &body)
            )));
        }
        serde_json::from_str::<Value>(&body).map_err(|e| OAuthError(format!("{e}")))
    }
}

/// SASL XOAUTH2 initial client response (`user=…\x01auth=Bearer …\x01\x01`).
pub fn xoauth2(user: &str, token: &str) -> String {
    format!("user={user}\x01auth=Bearer {token}\x01\x01")
}

/// Convenience: provider table as `key -> Provider` map.
pub fn providers_map() -> HashMap<&'static str, &'static Provider> {
    PROVIDERS.iter().map(|p| (p.key, p)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 10, 3).unwrap()
    }

    fn term(key: &str, val: &str, neg: bool, gid: usize) -> TextTerm {
        TextTerm {
            key: key.into(),
            val: val.into(),
            neg,
            gid,
        }
    }

    #[test]
    fn query_translation() {
        let r = translate_query(
            "in:inbox newer_than:7d -category:promotions is:unread",
            Some(d()),
        );
        assert_eq!(r.folders, vec!["inbox"]);
        assert_eq!(
            r.criteria,
            vec![
                "SINCE 26-Sep-2026",
                "NOT HEADER List-Unsubscribe \"\"",
                "UNSEEN"
            ]
        );
        assert_eq!(
            translate_query("from:boss@x.com", Some(d())).folders,
            vec!["inbox", "archive"]
        );
        let r = translate_query(
            "{from:a@x.com from:b@y.com} subject:\"hello world\" 发票 -报销",
            Some(d()),
        );
        assert_eq!(
            r.criteria,
            vec![
                "OR FROM \"a@x.com\" FROM \"b@y.com\"",
                "SUBJECT \"hello world\""
            ]
        );
        assert_eq!(
            r.text,
            vec![
                term("TEXT", "发票", false, 1),
                term("TEXT", "报销", true, 2)
            ]
        );
        assert_eq!(
            translate_query("label:工作 in:sent", Some(d())).folders,
            vec!["label:工作", "sent"]
        );
        assert_eq!(
            translate_query("after:2026/09/01 before:2026-09-30 larger:2M", Some(d())).criteria,
            vec!["SINCE 1-Sep-2026", "BEFORE 30-Sep-2026", "LARGER 2097152"]
        );
        assert_eq!(
            translate_query("invoice OR receipt", Some(d())).criteria,
            vec!["OR TEXT \"invoice\" TEXT \"receipt\""]
        );
        assert_eq!(translate_query("in:anywhere", Some(d())).folders, vec!["*"]);
        assert!(translate_query("", Some(d())).criteria.is_empty());
    }

    #[test]
    fn or_of_non_ascii_terms_stays_or() {
        let rq = translate_query(
            "newer_than:30d (\"笔试\" OR \"笔试通知\" OR \"在线测评\" OR \"测评链接\")",
            Some(d()),
        );
        assert_eq!(rq.criteria, vec!["SINCE 3-Sep-2026"]);
        assert!(rq.text.iter().all(|t| t.gid == 1) && rq.text.len() == 4);
        let hit = json!({"subject": "笔试通知：下周二在线笔试", "from": "hr@x.com", "to": "", "snippet": "请参加", "body": ""});
        assert!(local_match(&hit, &rq.text));
        assert!(!local_match(
            &json!({"subject": "周报", "from": "a@x.com", "to": "", "snippet": "", "body": ""}),
            &rq.text
        ));
        let b = translate_query("{笔试 测评}", Some(d())).text;
        assert!(b.iter().all(|t| t.gid == 1));
        let g2 = translate_query("笔试 测评", Some(d())).text;
        let gids: std::collections::HashSet<usize> = g2.iter().map(|t| t.gid).collect();
        assert_eq!(gids.len(), 2);
        assert!(!local_match(&hit, &g2));
    }

    #[test]
    fn presets_and_utf7() {
        assert_eq!(mutf7_decode("&g0l6P3ux-"), "草稿箱");
        assert_eq!(mutf7_encode("草稿箱"), "&g0l6P3ux-");
        assert_eq!(mutf7_decode(&mutf7_encode("A&B 工作/x")), "A&B 工作/x");
        assert_eq!(mutf7_decode("&XeVPXA-"), "工作");
        assert_eq!(guess_provider("x@QQ.com"), "qq");
        assert_eq!(guess_provider("a@hotmail.com"), "outlook");
        assert_eq!(guess_provider("a@corp.io"), "");
        let s = server_settings("netease", "me@126.com", None).unwrap();
        assert_eq!(s["imap_host"], "imap.126.com");
        assert_eq!(s["smtp_host"], "smtp.126.com");
        assert_eq!(s["username"], "me@126.com");
        assert_eq!(
            server_settings("outlook", "me@contoso.com", None).unwrap()["smtp_host"],
            "smtp.office365.com"
        );
        assert_eq!(
            server_settings("icloud", "me@icloud.com", None).unwrap()["smtp_security"],
            "starttls"
        );
        for bad in [
            json!({"imap_host": "", "smtp_host": "s"}),
            json!({"imap_host": "i.x.com", "smtp_host": "s.x.com", "imap_security": "none"}),
            json!({"imap_host": "i x", "smtp_host": "s"}),
        ] {
            assert!(server_settings("custom", "me@x.com", Some(&bad)).is_err());
        }
        let ok = server_settings(
            "custom",
            "me@x.com",
            Some(&json!({"imap_host": "127.0.0.1", "imap_port": 1143, "imap_security": "none",
                "smtp_host": "smtp.x.com", "smtp_port": "587", "smtp_security": "starttls", "username": "me"})),
        )
        .unwrap();
        assert_eq!(ok["imap_port"], 1143);
        assert_eq!(ok["smtp_port"], 587);
        assert_eq!(ok["username"], "me");
        assert_eq!(
            server_settings(
                "custom",
                "me@x.com",
                Some(
                    &json!({"imap_host": "10.0.0.5", "smtp_host": "s.x", "imap_security": "none"})
                )
            )
            .unwrap_err()
            .0,
            "不加密的连接只允许本机地址 (unencrypted only allowed for localhost)"
        );
        assert_eq!(PROVIDERS.len(), 9);
        assert_eq!(public_presets()[1]["auth"], "oauth");
        assert_eq!(label("qq"), "QQ Mail / Foxmail");
        assert_eq!(label(""), "Gmail");
        assert_eq!(label("nope"), "Other (IMAP / SMTP)");
        assert_eq!(ms_tenant("me@outlook.com"), "consumers");
        assert_eq!(ms_tenant("me@contoso.com"), "organizations");
        assert_eq!(xoauth2("u", "t"), "user=u\x01auth=Bearer t\x01\x01");
    }

    #[test]
    fn dates_and_sizes() {
        assert_eq!(
            imap_date(NaiveDate::from_ymd_opt(2026, 1, 5).unwrap()),
            "5-Jan-2026"
        );
        assert_eq!(
            abs_date("1700000000"),
            NaiveDate::from_ymd_opt(2023, 11, 14)
        );
        assert_eq!(abs_date("2026.13.01"), None);
        let r = translate_query("is:bogus has:drive smaller:10kb newer_than:1x", Some(d()));
        assert_eq!(r.criteria, vec!["SMALLER 10240"]);
        assert_eq!(
            r.notes,
            vec![
                "is:bogus ignored",
                "has:drive ignored",
                "newer_than:1x ignored"
            ]
        );
    }

    #[test]
    fn ms_error_text() {
        assert_eq!(
            ms_err(
                400,
                r#"{"error": "unauthorized_client", "error_description": "AADSTS700016: app not found\nmore"}"#
            ),
            "unauthorized_client: AADSTS700016: app not found"
        );
        assert_eq!(ms_err(500, "<html>"), "HTTP 500");
        assert_eq!(ms_err(400, r#"{"x": 1}"#), "400: ");
    }
}
