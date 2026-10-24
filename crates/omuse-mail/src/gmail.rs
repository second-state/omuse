//! Email connector over IMAP (read/organize/draft) and SMTP (send): Gmail and any other IMAP provider
//! (Outlook via OAuth 2.0, Yahoo, iCloud, QQ, 163/126, Zoho, AOL, custom servers) with an app password.
//!
//! Runs only inside Sentinel. The credential is fetched from the vault per call and never returned to callers.
//! On Gmail, message ids are Gmail's X-GM-MSGID (decimal string) and thread ids X-GM-THRID; on other servers
//! they are "<folder key>-<uid>" (see `GEN_ID`). The type keeps its historical name.
//!
//! Port of `app/sentinel/gmail.py`. The Python class was synchronous (imaplib / smtplib run in threads); here the
//! methods are `async` and take `&mut self` (the client caches the folder list and the Gmail flag like the Python
//! instance did). Results are `serde_json::Value` dicts with the Python keys.

use std::collections::HashMap;
use std::sync::{Arc, LazyLock};

use futures::future::BoxFuture;
use mail_parser::{MessageParser, MimeHeaders, PartType};
use regex::Regex;
use serde_json::{json, Map, Value};

use omuse_common::guard::{check_url, domain_of, is_security_message, redact_secrets};
use omuse_common::util::{py_str, take_chars, truthy};
use omuse_sentinel_core::Store;

use crate::imap::{default_dialer, quote, BoxStream, Dialer, ImapClient, ImapError, Item, Resp};
use crate::mailboxes;
use crate::mailproviders::{
    local_match, mutf7_decode, mutf7_encode, translate_query, xoauth2, MsLogin, OAuthError,
};
use crate::mime::{self, dec, Headers};
use crate::smtp::{self, SmtpFail, SmtpSettings};

pub use crate::mime::date_key;

/// Socket timeout in seconds.
pub const TIMEOUT: u64 = 30;
/// Largest attachment handed out.
pub const MAX_ATTACHMENT: usize = 25 * 1024 * 1024;
/// Most messages unsubscribed in one call.
pub const MAX_UNSUB: usize = 30;

/// Connector error; the message is user-facing (the sentinel turns it into an `ActionError`).
#[derive(Debug, Clone, thiserror::Error, PartialEq, Eq)]
#[error("{0}")]
pub struct GmailError(pub String);

impl GmailError {
    fn new(msg: impl Into<String>) -> Self {
        GmailError(msg.into())
    }
}

impl From<ImapError> for GmailError {
    fn from(e: ImapError) -> Self {
        GmailError(e.to_string())
    }
}

type R<T> = Result<T, GmailError>;

// ------------------------------------------------------------------ text helpers

fn vs(v: &Value, key: &str) -> String {
    match v.get(key) {
        None | Some(Value::Null) => String::new(),
        Some(Value::String(s)) => s.clone(),
        Some(x) => py_str(x),
    }
}

/// Python `str.splitlines()` (all Unicode line boundaries, `\r\n` as one).
fn splitlines(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut it = s.char_indices().peekable();
    while let Some((i, c)) = it.next() {
        let nl = matches!(
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
        if nl {
            out.push(&s[start..i]);
            let mut end = i + c.len_utf8();
            if c == '\r' {
                if let Some(&(j, '\n')) = it.peek() {
                    it.next();
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

static HTML_BLOCKS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?is)<script[^>]*>.*?</script>|<style[^>]*>.*?</style>|<head[^>]*>.*?</head>")
        .unwrap()
});
static HTML_BREAKS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)<br\s*/?>|</p>|</div>|</tr>|</li>|</h[1-6]>").unwrap());
static HTML_TAGS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?s)<[^>]+>").unwrap());
static HSPACE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[ \t\r\x0c\x0b]+").unwrap());
static BLANKS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\n\s*\n+").unwrap());

/// HTML → readable text (scripts/styles dropped, block ends → newlines, entities decoded).
pub fn html_to_text(h: &str) -> String {
    let h = HTML_BLOCKS.replace_all(h, " ");
    let h = HTML_BREAKS.replace_all(&h, "\n");
    let h = HTML_TAGS.replace_all(&h, " ");
    let h = html_escape::decode_html_entities(&h).into_owned();
    let h = HSPACE.replace_all(&h, " ");
    let h = BLANKS.replace_all(&h, "\n\n");
    h.trim().to_string()
}

// ------------------------------------------------------------------ MIME walking

/// One leaf part of a message, with the Python-visible properties.
struct Leaf {
    ctype: String,
    disposition: String,
    filename: Option<String>,
    charset: Option<String>,
    payload: Vec<u8>,
}

fn qp_decode(b: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'=' {
            if i + 1 < b.len() && b[i + 1] == b'\n' {
                i += 2;
                continue;
            }
            if i + 2 < b.len() && b[i + 1] == b'\r' && b[i + 2] == b'\n' {
                i += 3;
                continue;
            }
            if i + 2 < b.len() && b[i + 1].is_ascii_hexdigit() && b[i + 2].is_ascii_hexdigit() {
                let hex = std::str::from_utf8(&b[i + 1..i + 3]).unwrap_or("00");
                out.push(u8::from_str_radix(hex, 16).unwrap_or(b'?'));
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    out
}

fn b64_decode_lenient(b: &[u8]) -> Vec<u8> {
    use base64::Engine;
    let clean: Vec<u8> = b
        .iter()
        .copied()
        .filter(|c| c.is_ascii_alphanumeric() || *c == b'+' || *c == b'/')
        .collect();
    let cfg = base64::engine::GeneralPurposeConfig::new()
        .with_decode_padding_mode(base64::engine::DecodePaddingMode::Indifferent)
        .with_decode_allow_trailing_bits(true);
    let eng = base64::engine::GeneralPurpose::new(&base64::alphabet::STANDARD, cfg);
    let mut v = clean;
    // drop an incomplete trailing group (Python's binascii is lenient here too)
    if v.len() % 4 == 1 {
        v.pop();
    }
    eng.decode(&v).unwrap_or_default()
}

fn raw_header(raw: &[u8], part: &mail_parser::MessagePart<'_>, name: &str) -> Option<String> {
    part.headers
        .iter()
        .find(|h| h.name().eq_ignore_ascii_case(name))
        .map(|h| {
            let a = h.offset_start() as usize;
            let b = (h.offset_end() as usize).min(raw.len());
            mime::unfold(&String::from_utf8_lossy(raw.get(a..b).unwrap_or(b"")))
        })
}

fn collect_leaves(msg: &mail_parser::Message<'_>, out: &mut Vec<Leaf>) {
    let raw = msg.raw_message();
    for part in &msg.parts {
        match &part.body {
            PartType::Multipart(_) => continue,
            PartType::Message(inner) => {
                collect_leaves(inner, out);
                continue;
            }
            _ => {}
        }
        let ct = part.content_type();
        let ctype = match ct {
            Some(c) => format!("{}/{}", c.ctype(), c.subtype().unwrap_or("plain")).to_lowercase(),
            None => "text/plain".to_string(),
        };
        let charset = ct
            .and_then(|c| c.attribute("charset"))
            .map(|s| s.to_string());
        let disposition = raw_header(raw, part, "Content-Disposition")
            .unwrap_or_default()
            .to_lowercase();
        let filename = part.attachment_name().map(|s| s.to_string());
        let a = part.raw_body_offset() as usize;
        let b = (part.raw_end_offset() as usize).min(raw.len());
        let body = raw.get(a..b.max(a)).unwrap_or(b"");
        let cte = part
            .content_transfer_encoding()
            .unwrap_or("")
            .to_lowercase();
        let payload = match cte.as_str() {
            "base64" => b64_decode_lenient(body),
            "quoted-printable" => qp_decode(body),
            _ => body.to_vec(),
        };
        out.push(Leaf {
            ctype,
            disposition,
            filename,
            charset,
            payload,
        });
    }
}

fn leaves(raw: &[u8]) -> Vec<Leaf> {
    let mut out = Vec::new();
    if let Some(msg) = MessageParser::default().parse(raw) {
        collect_leaves(&msg, &mut out);
    }
    out
}

fn decode_charset(bytes: &[u8], charset: Option<&str>) -> Option<String> {
    let label = charset.unwrap_or("utf-8").trim().to_lowercase();
    let enc = match label.as_str() {
        "utf-8" | "utf8" => encoding_rs::UTF_8,
        "us-ascii" | "ascii" => encoding_rs::WINDOWS_1252,
        _ => encoding_rs::Encoding::for_label(label.as_bytes())?, // LookupError → skipped
    };
    Some(enc.decode_without_bom_handling(bytes).0.into_owned())
}

/// Readable body (text/plain preferred, else HTML → text, quoted `>` lines dropped) and the attachment list
/// (`{"filename", "type", "size"}`).
pub fn body_text(raw: &[u8]) -> (String, Vec<Value>) {
    let mut plain: Vec<String> = Vec::new();
    let mut html: Vec<String> = Vec::new();
    let mut attachments: Vec<Value> = Vec::new();
    for leaf in leaves(raw) {
        if leaf.filename.is_some() || leaf.disposition.contains("attachment") {
            let name = dec(leaf.filename.as_deref());
            attachments.push(json!({
                "filename": if name.is_empty() { "(unnamed)".to_string() } else { name },
                "type": leaf.ctype, "size": leaf.payload.len(),
            }));
            continue;
        }
        let Some(text) = decode_charset(&leaf.payload, leaf.charset.as_deref()) else {
            continue;
        };
        if leaf.ctype == "text/plain" {
            plain.push(text);
        } else if leaf.ctype == "text/html" {
            html.push(text);
        }
    }
    let joined = plain.join("\n");
    let mut body = joined.trim().to_string();
    if body.is_empty() {
        body = html_to_text(&html.join("\n"));
    }
    // drop long quoted history ("> " lines) to save context
    let lines: Vec<&str> = splitlines(&body)
        .into_iter()
        .filter(|l| !l.starts_with('>'))
        .collect();
    (lines.join("\n").trim().to_string(), attachments)
}

static SNIP_SOFT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"=\r?\n").unwrap());
static SNIP_BOUNDARY: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"--[A-Za-z0-9_=.\-]{10,}.*").unwrap());
static SNIP_CTYPE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"Content-[A-Za-z-]+:[^\n]*").unwrap());
static WS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s+").unwrap());

/// A short preview from the first bytes of a body.
pub fn snippet(raw: &[u8]) -> String {
    let mut txt = String::from_utf8_lossy(raw).into_owned();
    let low = txt.to_lowercase();
    if low.contains("<html") || low.contains("<div") {
        txt = html_to_text(&txt);
    }
    let txt = SNIP_SOFT.replace_all(&txt, "");
    let txt = SNIP_BOUNDARY.replace_all(&txt, " ");
    let txt = SNIP_CTYPE.replace_all(&txt, " ");
    let txt = WS.replace_all(&txt, " ");
    take_chars(txt.trim(), 240).to_string()
}

// ------------------------------------------------------------------ unsubscribe headers / query dedupe

static ANGLE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"<([^>]+)>").unwrap());

/// Parse RFC 2369 List-Unsubscribe (+ RFC 8058 List-Unsubscribe-Post):
/// `{"mailto"?, "https"?, "one_click", "method": "one-click"|"email"|"link"|""}`.
pub fn parse_list_unsubscribe(value: &str, post: &str) -> Value {
    let decoded = if value.is_empty() {
        String::new()
    } else {
        dec(Some(value))
    };
    let value = WS.replace_all(&decoded, "").to_string();
    let mut targets: Vec<String> = ANGLE
        .captures_iter(&value)
        .map(|c| c[1].to_string())
        .collect();
    if targets.is_empty() {
        targets = value
            .split(',')
            .filter(|x| !x.is_empty())
            .map(str::to_string)
            .collect();
    }
    let mut out = Map::new();
    for t in targets {
        let tl = t.to_lowercase();
        if tl.starts_with("mailto:") && !out.contains_key("mailto") {
            out.insert("mailto".into(), json!(t));
        } else if tl.starts_with("https://") && !out.contains_key("https") {
            out.insert("https".into(), json!(t));
        }
    }
    let one_click =
        out.contains_key("https") && post.to_lowercase().replace(' ', "").contains("one-click");
    out.insert("one_click".into(), json!(one_click));
    let method = if one_click {
        "one-click"
    } else if out.contains_key("mailto") {
        "email"
    } else if out.contains_key("https") {
        "link"
    } else {
        ""
    };
    out.insert("method".into(), json!(method));
    Value::Object(out)
}

static OR_SEP: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s+OR\s+").unwrap());

/// A looping model writes "conference OR conference OR conference …" (2026-10-04 V4-07); keep each OR term once.
pub fn dedupe_or_terms(q: &str) -> String {
    if q.matches(" OR ").count() < 3 {
        return q.to_string();
    }
    // re.split with a capturing group: text, sep, text, sep, …
    let mut parts: Vec<(bool, &str)> = Vec::new();
    let mut last = 0;
    for m in OR_SEP.find_iter(q) {
        parts.push((false, &q[last..m.start()]));
        parts.push((true, m.as_str()));
        last = m.end();
    }
    parts.push((false, &q[last..]));
    let mut out: Vec<(bool, &str)> = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    for (is_sep, part) in parts {
        if is_sep {
            out.push((true, part));
            continue;
        }
        let k = part.trim().to_lowercase();
        if seen.contains(&k) {
            if matches!(out.last(), Some((true, _))) {
                out.pop();
            }
            continue;
        }
        seen.push(k);
        out.push((false, part));
    }
    out.into_iter()
        .map(|(_, p)| p)
        .collect::<String>()
        .trim()
        .to_string()
}

// ------------------------------------------------------------------ folders

/// Generic (non-Gmail) message ids are "<folder key>-<uid>": i = INBOX, s = Sent, d = Drafts, t = Trash, j = Junk,
/// a = Archive, x<crc32> = any other folder. Gmail keeps its global X-GM-MSGID numbers.
pub const SPECIAL: &[(&str, &str)] = &[
    ("sent", "s"),
    ("drafts", "d"),
    ("trash", "t"),
    ("spam", "j"),
    ("archive", "a"),
];
static GEN_ID: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(i|s|d|t|j|a|x[0-9a-f]{8})-(\d{1,12})$").unwrap());
static GM_ID: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\d{5,25}$").unwrap());
const FOLDER_NAMES: &[(&str, &[&str])] = &[
    (
        "sent",
        &[
            "sent",
            "sent items",
            "sent messages",
            "sent mail",
            "已发送",
            "已发送邮件",
            "已发邮件",
            "寄件備份",
            "送信済み",
        ],
    ),
    ("drafts", &["drafts", "draft", "草稿箱", "草稿", "下書き"]),
    (
        "trash",
        &[
            "trash",
            "deleted",
            "deleted items",
            "deleted messages",
            "bin",
            "已删除",
            "已删除邮件",
            "垃圾桶",
            "ゴミ箱",
        ],
    ),
    (
        "spam",
        &[
            "junk",
            "spam",
            "junk email",
            "junk e-mail",
            "bulk mail",
            "垃圾邮件",
            "迷惑メール",
        ],
    ),
    (
        "archive",
        &["archive", "archives", "归档", "存档", "アーカイブ"],
    ),
];
const FLAG_KEYS: &[(&str, &str)] = &[
    ("\\All", "all"),
    ("\\Drafts", "drafts"),
    ("\\Sent", "sent"),
    ("\\Trash", "trash"),
    ("\\Junk", "spam"),
    ("\\Archive", "archive"),
    ("\\Important", "important"),
    ("\\Flagged", "starred"),
];
const STAR: &[&str] = &[
    "\\STARRED",
    "STARRED",
    "\\FLAGGED",
    "FLAGGED",
    "\\IMPORTANT",
    "IMPORTANT",
];

/// Login hints per provider.
pub const LOGIN_HINT: &[(&str, &str)] = &[
    ("gmail", "请确认 Google 账号已开启两步验证，并使用 16 位应用专用密码 (App Password)。"),
    ("outlook", "微软授权无效或已过期，请在「连接 Connections」页重新登录 Microsoft。"),
    ("qq", "请使用 QQ 邮箱「设置 → 账号」里开启 IMAP/SMTP 服务后生成的授权码，不是 QQ 密码。"),
    ("netease", "请使用网易邮箱「设置 → POP3/SMTP/IMAP」里开启 IMAP 服务后生成的授权码，不是登录密码。"),
    ("icloud", "请使用 Apple 账户里生成的 App 专用密码 (app-specific password)，并确认 iCloud 邮件已启用。"),
    ("yahoo", "请使用 Yahoo 账户安全设置里生成的应用密码 (app password)。"),
    ("aol", "请使用 AOL 账户安全设置里生成的应用密码 (app password)。"),
    ("zoho", "请在 Zoho 邮箱设置里开启 IMAP，并使用应用专用密码 (app-specific password)。"),
    ("custom", "请检查服务器地址、端口、加密方式、用户名和密码。"),
];

fn login_hint(provider: &str) -> &'static str {
    LOGIN_HINT
        .iter()
        .find(|(k, _)| *k == provider)
        .or_else(|| LOGIN_HINT.iter().find(|(k, _)| *k == "custom"))
        .map(|(_, v)| *v)
        .unwrap_or("")
}

fn crc(name: &str) -> String {
    format!("x{:08x}", crc32(name.as_bytes()))
}

/// zlib.crc32.
fn crc32(data: &[u8]) -> u32 {
    let mut c: u32 = 0xFFFF_FFFF;
    for &b in data {
        c ^= b as u32;
        for _ in 0..8 {
            c = if c & 1 != 0 {
                (c >> 1) ^ 0xEDB8_8320
            } else {
                c >> 1
            };
        }
    }
    !c
}

/// The folder list of a mailbox: raw names (as LIST returned them, quoted), display names (UTF-7 decoded) and
/// the special-use folders (`all`, `drafts`, `sent`, `trash`, `spam`, `archive`, `important`, `starred`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Folders {
    pub names: Vec<String>,
    pub display: HashMap<String, String>,
    pub special: HashMap<String, String>,
}

impl Folders {
    /// `f.get(key)`.
    pub fn get(&self, key: &str) -> Option<&str> {
        self.special.get(key).map(String::as_str)
    }
}

static LIST_LINE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"^\((?P<flags>[^)]*)\) (?:"(?P<sep>[^"]*)"|NIL) (?P<name>.+)$"#).unwrap()
});

/// Parse `LIST` data items into [`Folders`] (`gm` adds Gmail's defaults).
pub fn parse_folders(data: &[Item], gm: bool) -> Folders {
    let mut out = Folders::default();
    for raw in data {
        let line = match raw {
            Item::Literal { head, data } => {
                // name sent as a literal
                let head = String::from_utf8_lossy(head).to_string();
                let lit = String::from_utf8_lossy(data).to_string();
                let base = head
                    .rsplit_once('{')
                    .map(|(a, _)| a.to_string())
                    .unwrap_or(head);
                base + &quote(&lit)
            }
            Item::Line(l) => String::from_utf8_lossy(l).to_string(),
        };
        let Some(mm) = LIST_LINE.captures(&line) else {
            continue;
        };
        let name = mm["name"].trim().to_string();
        let flags = mm["flags"].to_string();
        if flags.contains("\\Noselect") || flags.contains("\\NonExistent") {
            continue;
        }
        let disp = mutf7_decode(&name.trim_matches('"').replace("\\\"", "\""));
        out.display.insert(name.clone(), disp);
        for (flag, key) in FLAG_KEYS {
            if flags.to_lowercase().contains(&flag.to_lowercase()) {
                out.special
                    .entry(key.to_string())
                    .or_insert_with(|| name.clone());
            }
        }
        out.names.push(name);
    }
    // servers without SPECIAL-USE flags (older QQ / 163 / custom): match well-known folder names
    for (key, names) in FOLDER_NAMES {
        if out.special.contains_key(*key) {
            continue;
        }
        for name in &out.names {
            let d = out.display[name].to_lowercase();
            let last_slash = d.rsplit('/').next().unwrap_or("");
            let last_dot = d.rsplit('.').next().unwrap_or("");
            if names.contains(&d.as_str())
                || names.contains(&last_slash)
                || names.contains(&last_dot)
            {
                out.special.insert(key.to_string(), name.clone());
                break;
            }
        }
    }
    if gm {
        out.special
            .entry("all".into())
            .or_insert_with(|| "\"[Gmail]/All Mail\"".into());
        out.special
            .entry("drafts".into())
            .or_insert_with(|| "\"[Gmail]/Drafts\"".into());
    }
    out
}

// ------------------------------------------------------------------ the client

/// Access-token provider for XOAUTH2 (Outlook).
pub type TokenFn = Arc<dyn Fn() -> BoxFuture<'static, Result<String, OAuthError>> + Send + Sync>;

/// HTTP for unsubscribe: `(post?, url) -> status code` or the error class name (`ConnectError`, …).
pub type UnsubHttp =
    Arc<dyn Fn(bool, String) -> BoxFuture<'static, Result<u16, String>> + Send + Sync>;

fn default_unsub_http() -> UnsubHttp {
    Arc::new(|post, url| {
        Box::pin(async move {
            let ua = if post {
                "OMuse-Unsubscribe/1.0"
            } else {
                "Mozilla/5.0 OMuse-Unsubscribe/1.0"
            };
            let client = reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(20))
                .redirect(reqwest::redirect::Policy::limited(5))
                .user_agent(ua)
                .no_proxy()
                .build()
                .map_err(|_| "HTTPError".to_string())?;
            let req = if post {
                client.post(&url).form(&[("List-Unsubscribe", "One-Click")])
            } else {
                client.get(&url)
            };
            match req.send().await {
                Ok(r) => Ok(r.status().as_u16()),
                Err(e) => Err(if e.is_timeout() {
                    "ReadTimeout"
                } else if e.is_connect() {
                    "ConnectError"
                } else if e.is_redirect() {
                    "TooManyRedirects"
                } else {
                    "HTTPError"
                }
                .to_string()),
            }
        })
    })
}

/// Optional constructor arguments (Python keyword-only parameters).
#[derive(Clone)]
pub struct GmailOptions {
    pub provider: String,
    pub imap_port: u16,
    pub imap_security: String,
    pub smtp_port: u16,
    pub smtp_security: String,
    pub username: String,
    pub token_fn: Option<TokenFn>,
}

impl Default for GmailOptions {
    fn default() -> Self {
        Self {
            provider: "gmail".into(),
            imap_port: 993,
            imap_security: "ssl".into(),
            smtp_port: 465,
            smtp_security: "ssl".into(),
            username: String::new(),
            token_fn: None,
        }
    }
}

/// An open IMAP session plus the selection state (`m._om_sel`, `m._om_key`).
pub struct Conn {
    pub c: ImapClient<BoxStream>,
    sel: Option<(String, bool)>,
    key: String,
}

/// A composed message (Python's `EmailMessage`): the header values callers read plus the wire bytes.
#[derive(Debug, Clone)]
pub struct Composed {
    pub from: String,
    pub to: String,
    pub cc: String,
    pub subject: String,
    pub in_reply_to: String,
    pub references: String,
    pub message_id: String,
    pub bytes: Vec<u8>,
}

/// One mailbox. Gmail servers (X-GM-EXT-1) use Gmail's own ids, labels and search; every other IMAP server
/// (Outlook, Yahoo, iCloud, QQ, 163, Zoho, custom…) uses folder+UID ids and Gmail-style queries translated to IMAP.
pub struct Gmail {
    pub email: String,
    pub password: String,
    pub imap_host: String,
    pub imap_port: u16,
    pub imap_security: String,
    pub smtp_host: String,
    pub smtp_port: u16,
    pub smtp_security: String,
    pub username: String,
    pub display_name: String,
    pub provider: String,
    pub token_fn: Option<TokenFn>,
    /// Account id (`g1`, `g2`, …) set by [`make_client`].
    pub account_id: String,
    /// Gmail server? `None` until the first login.
    pub gm: Option<bool>,
    /// Notes from the last generic search's query translation.
    pub search_notes: Vec<String>,
    folders: Option<Folders>,
    dialer: Dialer,
    unsub_http: UnsubHttp,
}

impl std::fmt::Debug for Gmail {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Gmail")
            .field("email", &self.email)
            .field("provider", &self.provider)
            .field("imap_host", &self.imap_host)
            .field("account_id", &self.account_id)
            .finish_non_exhaustive()
    }
}

impl Gmail {
    /// `Gmail(email, app_password, imap_host, smtp_host, display_name, **opts)`.
    pub fn new(
        email_addr: &str,
        app_password: &str,
        imap_host: &str,
        smtp_host: &str,
        display_name: &str,
        opts: GmailOptions,
    ) -> Self {
        let provider = if opts.provider.is_empty() {
            "gmail".to_string()
        } else {
            opts.provider
        };
        let password = if ["gmail", "yahoo", "aol", "icloud"].contains(&provider.as_str()) {
            app_password.replace(' ', "")
        } else {
            app_password.to_string()
        };
        let or = |s: String, d: &str| if s.is_empty() { d.to_string() } else { s };
        Self {
            email: email_addr.to_string(),
            password,
            imap_host: imap_host.to_string(),
            imap_port: if opts.imap_port == 0 {
                993
            } else {
                opts.imap_port
            },
            imap_security: or(opts.imap_security, "ssl"),
            smtp_host: smtp_host.to_string(),
            smtp_port: if opts.smtp_port == 0 {
                465
            } else {
                opts.smtp_port
            },
            smtp_security: or(opts.smtp_security, "ssl"),
            username: if opts.username.is_empty() {
                email_addr.to_string()
            } else {
                opts.username
            },
            display_name: display_name.to_string(),
            provider,
            token_fn: opts.token_fn,
            account_id: "g1".into(),
            gm: None,
            search_notes: Vec::new(),
            folders: None,
            dialer: default_dialer(),
            unsub_http: default_unsub_http(),
        }
    }

    /// Gmail defaults: `imap.gmail.com` / `smtp.gmail.com`.
    pub fn with_gmail_defaults(email_addr: &str, app_password: &str) -> Self {
        Self::new(
            email_addr,
            app_password,
            "imap.gmail.com",
            "smtp.gmail.com",
            "",
            GmailOptions::default(),
        )
    }

    /// Replace the IMAP transport (tests use an in-memory server).
    pub fn with_dialer(mut self, dialer: Dialer) -> Self {
        self.dialer = dialer;
        self
    }

    /// Replace the unsubscribe HTTP client (tests).
    pub fn with_unsub_http(mut self, http: UnsubHttp) -> Self {
        self.unsub_http = http;
        self
    }

    /// The cached folder list (after a call that listed folders).
    pub fn cached_folders(&self) -> Option<&Folders> {
        self.folders.as_ref()
    }

    // ------------------------------------------------------------ connection
    async fn imap(&mut self) -> R<Conn> {
        let hint = login_hint(&self.provider);
        let mut c = match (self.dialer)(
            self.imap_host.clone(),
            self.imap_port,
            self.imap_security.clone(),
        )
        .await
        {
            Ok(c) => c,
            Err(ImapError::Protocol(e)) => {
                return Err(GmailError(format!(
                    "IMAP 登录失败 (login failed): {e}. {hint}"
                )));
            }
            Err(ImapError::Io(e)) => {
                return Err(GmailError(format!(
                    "无法连接 {}:{}: {e}",
                    self.imap_host, self.imap_port
                )));
            }
        };
        let auth = match &self.token_fn {
            Some(f) => {
                let tok = f().await.map_err(|e| GmailError(e.0))?;
                c.authenticate_xoauth2(&xoauth2(&self.username, &tok)).await
            }
            None => c.login(&self.username, &self.password).await,
        };
        match auth {
            Ok(()) => {}
            Err(ImapError::Protocol(e)) => {
                return Err(GmailError(format!(
                    "IMAP 登录失败 (login failed): {e}. {hint}"
                )))
            }
            Err(ImapError::Io(e)) => {
                return Err(GmailError(format!(
                    "无法连接 {}:{}: {e}",
                    self.imap_host, self.imap_port
                )));
            }
        }
        let _ = c.capability().await;
        self.gm = Some(c.has("X-GM-EXT-1"));
        if c.has("ID") {
            // NetEase (163/126) refuses SELECT until the client identifies itself (RFC 2971)
            let _ = c
                .id("(\"name\" \"OMuse\" \"version\" \"1.0\" \"vendor\" \"OMuse\")")
                .await;
        }
        Ok(Conn {
            c,
            sel: None,
            key: "i".into(),
        })
    }

    async fn logout(m: Conn) {
        let mut m = m;
        m.c.logout().await;
    }

    /// The mailbox's folders (cached per client).
    pub async fn folders(&mut self, m: &mut Conn) -> R<Folders> {
        if let Some(f) = &self.folders {
            if !f.names.is_empty() || !f.special.is_empty() {
                return Ok(f.clone());
            }
        }
        let r = m.c.list().await?;
        let gm = self.is_gm().await?;
        let f = parse_folders(&r.data, gm);
        self.folders = Some(f.clone());
        Ok(f)
    }

    async fn select(&mut self, m: &mut Conn, folder: &str, readonly: bool) -> R<()> {
        if m.sel.as_ref() == Some(&(folder.to_string(), readonly)) {
            return Ok(());
        }
        let r = m.c.select(folder, readonly).await?;
        if !r.ok() {
            return Err(GmailError(format!(
                "无法打开文件夹 cannot open folder {}: {}",
                self.fname(folder),
                r.py_data()
            )));
        }
        m.sel = Some((folder.to_string(), readonly));
        Ok(())
    }

    async fn select_all(&mut self, m: &mut Conn, readonly: bool) -> R<()> {
        let f = self.folders(m).await?;
        let all = f.get("all").unwrap_or("").to_string();
        if self.select(m, &all, readonly).await.is_err() {
            return Err(GmailError::new(
                "无法打开「所有邮件」文件夹 (cannot select All Mail)",
            ));
        }
        Ok(())
    }

    fn fname(&self, raw: &str) -> String {
        if raw.eq_ignore_ascii_case("INBOX") {
            return "INBOX".into();
        }
        self.folders
            .as_ref()
            .and_then(|f| f.display.get(raw).cloned())
            .filter(|d| !d.is_empty())
            .unwrap_or_else(|| raw.trim_matches('"').to_string())
    }

    // generic folders <-> id keys
    async fn fkey(&mut self, m: &mut Conn, raw: &str) -> R<String> {
        if raw.eq_ignore_ascii_case("INBOX") {
            return Ok("i".into());
        }
        let f = self.folders(m).await?;
        for (key, code) in SPECIAL {
            if f.get(key) == Some(raw) {
                return Ok(code.to_string());
            }
        }
        Ok(crc(raw))
    }

    async fn fraw(&mut self, m: &mut Conn, key: &str) -> R<String> {
        if key == "i" {
            return Ok("INBOX".into());
        }
        let f = self.folders(m).await?;
        for (name, code) in SPECIAL {
            if *code == key {
                if let Some(v) = f.get(name).filter(|v| !v.is_empty()) {
                    return Ok(v.to_string());
                }
                return Err(GmailError(format!(
                    "这个邮箱没有{name}文件夹 (no {name} folder)"
                )));
            }
        }
        f.names
            .iter()
            .find(|raw| crc(raw) == key)
            .cloned()
            .ok_or_else(|| GmailError::new("找不到邮件所在的文件夹 (folder no longer exists)"))
    }

    async fn folder_by_name(
        &mut self,
        m: &mut Conn,
        name: &str,
        create: bool,
    ) -> R<Option<String>> {
        let f = self.folders(m).await?;
        let want = name.trim().trim_matches('/').to_lowercase();
        if want == "inbox" || want == "\\inbox" {
            return Ok(Some("INBOX".into()));
        }
        for key in ["sent", "drafts", "trash", "spam", "archive"] {
            if want == key || want == format!("\\{key}") {
                if let Some(v) = f.get(key).filter(|v| !v.is_empty()) {
                    return Ok(Some(v.to_string()));
                }
            }
        }
        for raw in &f.names {
            let d = f.display[raw].to_lowercase();
            if d == want
                || d.rsplit('/').next() == Some(want.as_str())
                || d.rsplit('.').next() == Some(want.as_str())
            {
                return Ok(Some(raw.clone()));
            }
        }
        if !create {
            return Ok(None);
        }
        let raw = quote(&mutf7_encode(name.trim()));
        let r = m.c.create(&raw).await?;
        if !r.ok() {
            return Err(GmailError(format!(
                "无法创建文件夹 cannot create folder {name}: {}",
                r.py_data()
            )));
        }
        self.folders = None;
        self.folders(m).await?;
        Ok(Some(raw))
    }

    /// Select the message's folder and return its UID.
    async fn locate(&mut self, m: &mut Conn, msgid: &str, readonly: bool) -> R<String> {
        if self.is_gm().await? {
            self.select_all(m, readonly).await?;
            return self.uid_for_msgid(m, msgid).await;
        }
        let id = msgid.trim();
        let Some(mm) = GEN_ID.captures(id) else {
            return Err(GmailError(format!("无效的 message_id: {msgid}")));
        };
        let key = mm[1].to_string();
        let uid = mm[2].to_string();
        let raw = self.fraw(m, &key).await?;
        self.select(m, &raw, readonly).await?;
        m.key = key;
        Ok(uid)
    }

    async fn uid_for_msgid(&mut self, m: &mut Conn, msgid: &str) -> R<String> {
        if !GM_ID.is_match(msgid) {
            return Err(GmailError(format!("无效的 message_id: {msgid}")));
        }
        let r = m.c.uid("SEARCH", &["X-GM-MSGID", msgid], None).await?;
        r.numbers()
            .pop()
            .ok_or_else(|| GmailError(format!("找不到邮件 message not found: {msgid}")))
    }

    async fn fetch_meta(
        &mut self,
        m: &mut Conn,
        uids: &[String],
        with_snippet: bool,
    ) -> R<Vec<Value>> {
        if uids.is_empty() {
            return Ok(vec![]);
        }
        let uid_set = uids.join(",");
        let gm = self.is_gm().await?;
        let gmf = if gm {
            " X-GM-MSGID X-GM-THRID X-GM-LABELS"
        } else {
            ""
        };
        let mut parts = format!(
            "(UID{gmf} FLAGS BODY.PEEK[HEADER.FIELDS (FROM TO CC SUBJECT DATE MESSAGE-ID LIST-UNSUBSCRIBE LIST-UNSUBSCRIBE-POST)]"
        );
        parts += if with_snippet {
            " BODY.PEEK[TEXT]<0.3000>)"
        } else {
            ")"
        };
        let r = m.c.uid("FETCH", &[&uid_set, &parts], None).await?;
        let key = m.key.clone();
        let folder_name = self.fname(m.sel.as_ref().map(|s| s.0.as_str()).unwrap_or("INBOX"));
        Ok(parse_fetch_meta(&r, gm, &key, &folder_name))
    }

    // ------------------------------------------------------------ read

    /// Strip internal fields and mark drafts.
    pub fn public(mut msg: Value) -> Value {
        if let Some(o) = msg.as_object_mut() {
            o.remove("_unsub");
        }
        if msg.get("draft").map(truthy).unwrap_or(false) {
            msg["status"] =
                json!("草稿，从未发送 DRAFT - never sent (do not say the user replied / sent it)");
        }
        msg
    }

    /// Search with Gmail syntax (translated to IMAP on other servers). Newest first, at most `max_results` (1–100).
    pub async fn search(&mut self, query: &str, max_results: i64) -> R<Vec<Value>> {
        let max_results = (if max_results == 0 { 10 } else { max_results }).clamp(1, 100) as usize;
        let query = dedupe_or_terms(query);
        let mut m = self.imap().await?;
        let res = self.search_in(&mut m, &query, max_results).await;
        Self::logout(m).await;
        res
    }

    async fn search_in(&mut self, m: &mut Conn, query: &str, max_results: usize) -> R<Vec<Value>> {
        if !self.is_gm().await? {
            let found = self.search_generic(m, query, max_results).await?;
            return Ok(found.into_iter().map(Self::public).collect());
        }
        self.select_all(m, true).await?;
        let q = if query.trim().is_empty() {
            "in:inbox"
        } else {
            query.trim()
        };
        let r = if q.is_ascii() {
            m.c.uid("SEARCH", &["X-GM-RAW", &quote(q)], None).await?
        } else {
            m.c.uid(
                "SEARCH",
                &["CHARSET", "UTF-8", "X-GM-RAW"],
                Some(q.as_bytes()),
            )
            .await?
        };
        if !r.ok() {
            return Err(GmailError(format!(
                "搜索失败 search failed: {}",
                r.py_data()
            )));
        }
        let uids = r.numbers();
        let start = uids.len().saturating_sub(max_results);
        let metas = self.fetch_meta(m, &uids[start..], true).await?;
        Ok(metas.into_iter().map(Self::public).collect())
    }

    async fn search_targets(&mut self, m: &mut Conn, wanted: &[String]) -> R<Vec<String>> {
        let f = self.folders(m).await?;
        let mut out: Vec<String> = Vec::new();
        for w in wanted {
            let cands: Vec<Option<String>> = if w == "*" {
                vec![
                    Some("INBOX".into()),
                    f.get("archive").map(str::to_string),
                    f.get("sent").map(str::to_string),
                ]
            } else if w == "inbox" {
                vec![Some("INBOX".into())]
            } else if SPECIAL.iter().any(|(k, _)| k == w) {
                let v = f.get(w).filter(|v| !v.is_empty()).map(str::to_string);
                if w != "archive" && v.is_none() {
                    return Err(GmailError(format!(
                        "这个邮箱没有 {w} 文件夹 (no {w} folder)"
                    )));
                }
                vec![v]
            } else {
                // label:<name> -> a folder
                let name = w.split_once(':').map(|(_, n)| n).unwrap_or("").to_string();
                match self.folder_by_name(m, &name, false).await? {
                    Some(raw) => vec![Some(raw)],
                    None => {
                        let names: Vec<String> = f
                            .names
                            .iter()
                            .take(30)
                            .map(|n| f.display[n].clone())
                            .collect();
                        return Err(GmailError(format!(
                            "找不到文件夹 folder not found: {name}. 现有文件夹 folders: {}",
                            names.join(", ")
                        )));
                    }
                }
            };
            for c in cands.into_iter().flatten() {
                if !c.is_empty() && !out.contains(&c) {
                    out.push(c);
                }
            }
        }
        Ok(out)
    }

    async fn search_generic(&mut self, m: &mut Conn, query: &str, n: usize) -> R<Vec<Value>> {
        let tq = translate_query(query, None);
        self.search_notes = tq.notes.clone();
        let crit: Vec<String> = if tq.criteria.is_empty() {
            vec!["ALL".into()]
        } else {
            tq.criteria.clone()
        };
        let texts = &tq.text;
        // Only pre-filter on the server by one positive term when the positive terms are pure AND. If any OR group has
        // more than one term (e.g. "笔试" OR "在线测评"), narrowing by a single member would drop messages that match a
        // sibling, so fetch by the date/folder criteria and OR-filter everything locally instead.
        let mut counts: HashMap<usize, usize> = HashMap::new();
        for t in texts.iter().filter(|t| !t.neg) {
            *counts.entry(t.gid).or_default() += 1;
        }
        let has_or = counts.values().any(|c| *c > 1);
        let server_txt: Option<(String, String)> = if has_or {
            None
        } else {
            texts
                .iter()
                .find(|t| !t.neg)
                .map(|t| (t.key.clone(), t.val.clone()))
        };
        let local: Vec<_> = texts
            .iter()
            .filter(|t| {
                !(server_txt.is_some()
                    && !t.neg
                    && Some((t.key.clone(), t.val.clone())) == server_txt)
            })
            .cloned()
            .collect();
        let mut found: Vec<Value> = Vec::new();
        for raw in self.search_targets(m, &tq.folders).await? {
            if self.select(m, &raw, true).await.is_err() {
                continue;
            }
            m.key = self.fkey(m, &raw).await?;
            let mut uids: Option<Vec<String>> = None;
            let mut need_local = local.clone();
            let crit_refs: Vec<&str> = crit.iter().map(String::as_str).collect();
            if let Some((skey, sval)) = &server_txt {
                let mut args: Vec<&str> = vec!["CHARSET", "UTF-8"];
                args.extend(crit_refs.iter().copied());
                args.push(skey);
                match m.c.uid("SEARCH", &args, Some(sval.as_bytes())).await {
                    Ok(r) if r.ok() => uids = Some(r.numbers()),
                    Ok(_) | Err(ImapError::Protocol(_)) => {}
                    Err(e) => return Err(e.into()),
                }
                if uids.is_none() {
                    // server cannot search UTF-8 text: filter the newest messages locally
                    need_local = tq.text.clone();
                }
            }
            let mut uids = match uids {
                Some(u) => u,
                None => {
                    let r = m.c.uid("SEARCH", &crit_refs, None).await?;
                    if !r.ok() {
                        return Err(GmailError(format!(
                            "搜索失败 search failed: {}",
                            r.py_data()
                        )));
                    }
                    r.numbers()
                }
            };
            uids.sort_by_key(|u| u.parse::<u64>().unwrap_or(0));
            if !need_local.is_empty() {
                let pool = &uids[uids.len().saturating_sub(300)..];
                let mut metas = Vec::new();
                for chunk in pool.chunks(100) {
                    metas.extend(self.fetch_meta(m, chunk, true).await?);
                }
                found.extend(metas.into_iter().filter(|x| local_match(x, &need_local)));
            } else {
                let start = uids.len().saturating_sub(n);
                found.extend(self.fetch_meta(m, &uids[start..], true).await?);
            }
        }
        sort_by_date_desc(&mut found);
        found.truncate(n);
        Ok(found)
    }

    /// One message with its body (secrets redacted; security emails withheld) and attachment list.
    pub async fn get_message(&mut self, message_id: &str) -> R<Value> {
        let mut m = self.imap().await?;
        let res = self.get_message_in(&mut m, message_id).await;
        Self::logout(m).await;
        res
    }

    async fn get_message_in(&mut self, m: &mut Conn, message_id: &str) -> R<Value> {
        let uid = self.locate(m, message_id, true).await?;
        let meta = self
            .fetch_meta(m, std::slice::from_ref(&uid), false)
            .await?;
        let r = m.c.uid("FETCH", &[&uid, "(BODY.PEEK[])"], None).await?;
        let raw = r
            .data
            .iter()
            .find_map(|d| d.literal())
            .unwrap_or(b"")
            .to_vec();
        if raw.is_empty() {
            return Err(GmailError(format!(
                "找不到邮件 message not found: {message_id}"
            )));
        }
        let (body, attachments) = body_text(&raw);
        let mut info = match meta.into_iter().next() {
            Some(x) => Self::public(x),
            None => json!({"id": message_id}),
        };
        if info.get("security_message").map(truthy).unwrap_or(false) {
            info["body"] = json!("[这是一封验证码/登录/重置密码类的安全邮件，内容不会提供给 Agent。This is a security email; its content is withheld from the agent.]");
            info["redactions"] = json!(1);
        } else {
            let (body, n) = redact_secrets(&body);
            info["body"] = json!(take_chars(&body, 15000));
            info["redactions"] = json!(n);
        }
        info["attachments"] = Value::Array(attachments);
        info["references"] = json!(mime::parse_headers(&raw).get_or_empty("References"));
        Ok(info)
    }

    /// The messages of a thread (oldest first, at most 15, bodies cut to 6000 characters).
    pub async fn get_thread(&mut self, thread_id: &str) -> R<Vec<Value>> {
        if !self.is_gm().await? {
            return self.thread_generic(thread_id).await;
        }
        if !GM_ID.is_match(thread_id) {
            return Err(GmailError(format!("无效的 thread_id: {thread_id}")));
        }
        let mut m = self.imap().await?;
        let res = async {
            self.select_all(&mut m, true).await?;
            let r = m.c.uid("SEARCH", &["X-GM-THRID", thread_id], None).await?;
            let uids = r.numbers();
            let start = uids.len().saturating_sub(15);
            self.fetch_meta(&mut m, &uids[start..], false).await
        }
        .await;
        Self::logout(m).await;
        self.full_thread(res?).await
    }

    async fn full_thread(&mut self, mut metas: Vec<Value>) -> R<Vec<Value>> {
        metas.sort_by(|a, b| {
            date_key(&vs(a, "date"))
                .partial_cmp(&date_key(&vs(b, "date")))
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        let start = metas.len().saturating_sub(15);
        let mut msgs = Vec::new();
        for meta in &metas[start..] {
            let mut full = self.get_message(&vs(meta, "id")).await?;
            let body = vs(&full, "body");
            full["body"] = json!(take_chars(&body, 6000));
            msgs.push(full);
        }
        Ok(msgs)
    }

    /// No thread ids outside Gmail: follow Message-ID / References / In-Reply-To across Inbox, Archive and Sent.
    async fn thread_generic(&mut self, anchor: &str) -> R<Vec<Value>> {
        static REF: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"<[^<>\s]+>").unwrap());
        let mut m = self.imap().await?;
        let res: R<Map<String, Value>> = async {
            let uid = self.locate(&mut m, anchor, true).await?;
            let r =
                m.c.uid(
                    "FETCH",
                    &[
                        &uid,
                        "(BODY.PEEK[HEADER.FIELDS (MESSAGE-ID REFERENCES IN-REPLY-TO)])",
                    ],
                    None,
                )
                .await?;
            let raw = r
                .data
                .iter()
                .find_map(|d| d.literal())
                .unwrap_or(b"")
                .to_vec();
            let h = mime::parse_headers(&raw);
            let joined = format!(
                "{} {}",
                h.get_or_empty("References"),
                h.get_or_empty("In-Reply-To")
            );
            let refs: Vec<String> = REF
                .find_iter(&joined)
                .map(|x| x.as_str().to_string())
                .collect();
            let own = h.get_or_empty("Message-ID").trim().to_string();
            let root = refs.first().cloned().unwrap_or(own);
            let mut metas: Map<String, Value> = Map::new();
            if !root.is_empty() && root.is_ascii() {
                let q = quote(&root);
                for folder in self.search_targets(&mut m, &["*".to_string()]).await? {
                    if self.select(&mut m, &folder, true).await.is_err() {
                        continue;
                    }
                    m.key = self.fkey(&mut m, &folder).await?;
                    let r =
                        m.c.uid(
                            "SEARCH",
                            &[
                                "OR",
                                "OR",
                                "HEADER",
                                "Message-ID",
                                &q,
                                "HEADER",
                                "References",
                                &q,
                                "HEADER",
                                "In-Reply-To",
                                &q,
                            ],
                            None,
                        )
                        .await?;
                    if r.ok() {
                        let uids = r.numbers();
                        let start = uids.len().saturating_sub(15);
                        for x in self.fetch_meta(&mut m, &uids[start..], false).await? {
                            metas.insert(vs(&x, "id"), x);
                        }
                    }
                }
            }
            if !metas.contains_key(anchor) {
                metas.insert(anchor.to_string(), json!({"id": anchor, "date": ""}));
            }
            Ok(metas)
        }
        .await;
        Self::logout(m).await;
        self.full_thread(res?.into_iter().map(|(_, v)| v).collect())
            .await
    }

    /// Display names of all folders / labels.
    pub async fn list_labels(&mut self) -> R<Vec<String>> {
        let mut m = self.imap().await?;
        let res = self.folders(&mut m).await;
        Self::logout(m).await;
        let f = res?;
        Ok(f.names
            .iter()
            .map(|n| {
                f.display
                    .get(n)
                    .cloned()
                    .unwrap_or_else(|| n.trim_matches('"').to_string())
            })
            .collect())
    }

    // ------------------------------------------------------------ organize
    async fn store_labels(&mut self, ids: &[String], op: &str, labels: &[String]) -> R<i64> {
        let mut m = self.imap().await?;
        let res = async {
            self.select_all(&mut m, false).await?;
            let mut n = 0;
            for mid in ids.iter().take(50) {
                let uid = self.uid_for_msgid(&mut m, mid).await?;
                let lab = format!(
                    "({})",
                    labels
                        .iter()
                        .map(|x| if x.starts_with('\\') {
                            x.clone()
                        } else {
                            quote(x)
                        })
                        .collect::<Vec<_>>()
                        .join(" ")
                );
                let r =
                    m.c.uid("STORE", &[&uid, &format!("{op}X-GM-LABELS"), &lab], None)
                        .await?;
                if r.ok() {
                    n += 1;
                }
            }
            Ok(n)
        }
        .await;
        Self::logout(m).await;
        res
    }

    async fn mv(m: &mut Conn, uid: &str, dest: &str) -> R<bool> {
        if m.c.has("MOVE") {
            return Ok(m.c.uid("MOVE", &[uid, dest], None).await?.ok());
        }
        if !m.c.uid("COPY", &[uid, dest], None).await?.ok() {
            return Ok(false);
        }
        m.c.uid("STORE", &[uid, "+FLAGS", "(\\Deleted)"], None)
            .await?;
        if m.c.has("UIDPLUS") {
            // only this message; never a plain EXPUNGE of other \Deleted mail
            m.c.uid("EXPUNGE", &[uid], None).await?;
        }
        Ok(true)
    }

    async fn store_flag(m: &mut Conn, uid: &str, op: &str, flag: &str) -> R<bool> {
        Ok(m.c.uid("STORE", &[uid, op, flag], None).await?.ok())
    }

    async fn each_op(&mut self, m: &mut Conn, uid: &str, key: &str, op: &EachOp) -> R<bool> {
        match op {
            EachOp::Archive => {
                if key != "i" {
                    return Ok(false); // already out of the inbox
                }
                let dest = self.archive_folder(m).await?;
                Self::mv(m, uid, &dest).await
            }
            EachOp::Add(lab) => {
                let up = lab.trim().to_uppercase();
                if STAR.contains(&up.as_str()) {
                    return Self::store_flag(m, uid, "+FLAGS", "(\\Flagged)").await;
                }
                if up == "UNREAD" || up == "\\UNREAD" {
                    return Self::store_flag(m, uid, "-FLAGS", "(\\Seen)").await;
                }
                let dest = self.folder_by_name(m, lab, true).await?.unwrap_or_default();
                if dest == "INBOX" {
                    return Ok(key != "i" && Self::mv(m, uid, "INBOX").await?);
                }
                Ok(m.c.uid("COPY", &[uid, &dest], None).await?.ok())
            }
            EachOp::Remove(lab) => {
                let up = lab.trim().to_uppercase();
                if STAR.contains(&up.as_str()) {
                    return Self::store_flag(m, uid, "-FLAGS", "(\\Flagged)").await;
                }
                if up == "UNREAD" || up == "\\UNREAD" {
                    return Self::store_flag(m, uid, "+FLAGS", "(\\Seen)").await;
                }
                if up == "INBOX" || up == "\\INBOX" {
                    if key != "i" {
                        return Ok(false);
                    }
                    let dest = self.archive_folder(m).await?;
                    return Self::mv(m, uid, &dest).await;
                }
                let src = self.folder_by_name(m, lab, false).await?;
                if let Some(src) = src {
                    if m.sel.as_ref().map(|s| s.0.as_str()) == Some(src.as_str()) {
                        // "remove label" = move the message back to the inbox
                        return Self::mv(m, uid, "INBOX").await;
                    }
                }
                Ok(false)
            }
        }
    }

    async fn archive_folder(&mut self, m: &mut Conn) -> R<String> {
        let f = self.folders(m).await?;
        match f.get("archive").filter(|v| !v.is_empty()) {
            Some(a) => Ok(a.to_string()),
            None => Ok(self
                .folder_by_name(m, "Archive", true)
                .await?
                .unwrap_or_default()),
        }
    }

    async fn generic_each(&mut self, ids: &[String], op: EachOp) -> R<i64> {
        let mut m = self.imap().await?;
        let res = async {
            let mut n = 0;
            for mid in ids.iter().take(50) {
                let uid = self.locate(&mut m, mid, false).await?;
                let key = m.key.clone();
                if self.each_op(&mut m, &uid, &key, &op).await? {
                    n += 1;
                }
            }
            Ok(n)
        }
        .await;
        Self::logout(m).await;
        res
    }

    /// Gmail server? Known after the first login (X-GM-EXT-1); before that, from the account's provider.
    pub async fn is_gm(&mut self) -> R<bool> {
        if self.gm.is_none() && self.provider == "custom" {
            let m = self.imap().await?;
            Self::logout(m).await;
        }
        Ok(self.gm.unwrap_or(self.provider == "gmail"))
    }

    /// Move out of the inbox: `{"archived": n}`.
    pub async fn archive(&mut self, ids: &[String]) -> R<Value> {
        if self.is_gm().await? {
            return Ok(
                json!({"archived": self.store_labels(ids, "-", &["\\Inbox".to_string()]).await?}),
            );
        }
        Ok(json!({"archived": self.generic_each(ids, EachOp::Archive).await?}))
    }

    /// Add / remove labels (folders + `\Flagged` on other servers): `{"added"?: n, "removed"?: n}`.
    pub async fn label(&mut self, ids: &[String], add: &[String], remove: &[String]) -> R<Value> {
        let mut res = Map::new();
        if self.is_gm().await? {
            if !add.is_empty() {
                res.insert(
                    "added".into(),
                    json!(self.store_labels(ids, "+", add).await?),
                );
            }
            if !remove.is_empty() {
                res.insert(
                    "removed".into(),
                    json!(self.store_labels(ids, "-", remove).await?),
                );
            }
            return Ok(Value::Object(res));
        }
        // other providers: folders instead of labels; starred = \Flagged
        for lab in add {
            let n = self.generic_each(ids, EachOp::Add(lab.clone())).await?;
            let cur = res.get("added").and_then(Value::as_i64).unwrap_or(0);
            res.insert("added".into(), json!(cur + n));
        }
        for lab in remove {
            let n = self.generic_each(ids, EachOp::Remove(lab.clone())).await?;
            let cur = res.get("removed").and_then(Value::as_i64).unwrap_or(0);
            res.insert("removed".into(), json!(cur + n));
        }
        Ok(Value::Object(res))
    }

    /// Set / clear `\Seen`: `{"updated": n}`.
    pub async fn mark_read(&mut self, ids: &[String], read: bool) -> R<Value> {
        let mut m = self.imap().await?;
        let res = async {
            let mut n = 0;
            for mid in ids.iter().take(50) {
                let uid = self.locate(&mut m, mid, false).await?;
                if Self::store_flag(
                    &mut m,
                    &uid,
                    if read { "+FLAGS" } else { "-FLAGS" },
                    "(\\Seen)",
                )
                .await?
                {
                    n += 1;
                }
            }
            Ok(json!({"updated": n}))
        }
        .await;
        Self::logout(m).await;
        res
    }

    // ------------------------------------------------------------ compose

    /// One attachment of a message: (filename, content type, bytes). Security emails never give out attachments.
    pub async fn get_attachment(
        &mut self,
        message_id: &str,
        filename: &str,
    ) -> R<(String, String, Vec<u8>)> {
        let mut m = self.imap().await?;
        let res = async {
            let uid = self.locate(&mut m, message_id, true).await?;
            let meta = self
                .fetch_meta(&mut m, std::slice::from_ref(&uid), false)
                .await?;
            if meta
                .first()
                .and_then(|x| x.get("security_message"))
                .map(truthy)
                .unwrap_or(false)
            {
                return Err(GmailError::new(
                    "安全类邮件的附件不提供给 Agent (security email)",
                ));
            }
            let r = m.c.uid("FETCH", &[&uid, "(BODY.PEEK[])"], None).await?;
            let raw = r
                .data
                .iter()
                .find_map(|d| d.literal())
                .unwrap_or(b"")
                .to_vec();
            pick_attachment(&raw, filename)
        }
        .await;
        Self::logout(m).await;
        res
    }

    /// Build the outgoing message (`EmailMessage`): text/plain body, attachments as multipart/mixed,
    /// In-Reply-To / References on replies.
    pub fn compose(
        &self,
        to: &str,
        subject: &str,
        body: &str,
        cc: &str,
        in_reply_to: Option<&Value>,
        attachments: &[(String, String, Vec<u8>)],
    ) -> R<Composed> {
        use mail_builder::headers::raw::Raw;
        use mail_builder::headers::text::Text;
        use mail_builder::MessageBuilder;

        let from = if self.display_name.is_empty() {
            self.email.clone()
        } else {
            mime::formataddr(&self.display_name, &self.email)
        };
        let mut subject = subject.to_string();
        let mut irt = String::new();
        let mut refs = String::new();
        if let Some(orig) = in_reply_to {
            let subj = mime::unfold(&vs(orig, "subject"));
            if subject.is_empty() {
                subject = if subj.to_lowercase().starts_with("re:") {
                    subj
                } else {
                    format!("Re: {subj}")
                };
            }
            let mid = mime::unfold(&vs(orig, "message_id_header"));
            if !mid.is_empty() {
                // long References headers arrive folded over several lines (CRLF + space); unfold them
                // (E2E-1: two of three reply drafts failed on this)
                refs = mime::unfold(&format!("{} {mid}", vs(orig, "references")));
                irt = mid;
            }
        }
        let mut subject = mime::unfold(&subject);
        if subject.is_empty() {
            subject = "(no subject)".into();
        }
        let message_id = mime::make_msgid(self.email.rsplit('@').next().unwrap_or(""));
        let addr_header = |v: &str| -> String {
            if v.is_ascii() {
                v.to_string()
            } else {
                mime::getaddresses(&[v])
                    .iter()
                    .map(|(n, a)| mime::formataddr(n, a))
                    .collect::<Vec<_>>()
                    .join(", ")
            }
        };
        let mut b = MessageBuilder::new()
            .header("From", Raw::new(from.clone()))
            .header("To", Raw::new(addr_header(to)));
        if !cc.is_empty() {
            b = b.header("Cc", Raw::new(addr_header(cc)));
        }
        if !irt.is_empty() {
            b = b
                .header("In-Reply-To", Raw::new(irt.clone()))
                .header("References", Raw::new(refs.clone()));
        }
        b = b
            .header("Subject", Text::new(subject.clone()))
            .header("Date", Raw::new(mime::format_date_local()))
            .header("Message-ID", Raw::new(message_id.clone()))
            .text_body(body.to_string());
        for (name, ctype, data) in attachments {
            let ct = if ctype.is_empty() {
                "application/octet-stream".to_string()
            } else {
                ctype.clone()
            };
            let ct = if ct.contains('/') {
                ct
            } else {
                format!("{ct}/octet-stream")
            };
            b = b.attachment(ct, name.clone(), data.clone());
        }
        let bytes = b.write_to_vec().map_err(|e| GmailError(format!("{e}")))?;
        Ok(Composed {
            from,
            to: to.to_string(),
            cc: cc.to_string(),
            subject,
            in_reply_to: irt,
            references: refs,
            message_id,
            bytes,
        })
    }

    async fn original(&mut self, message_id: Option<&str>) -> R<Option<Value>> {
        let Some(message_id) = message_id.filter(|x| !x.is_empty()) else {
            return Ok(None);
        };
        let mut m = self.imap().await?;
        let res = async {
            let uid = self.locate(&mut m, message_id, true).await?;
            let r = m
                .c
                .uid("FETCH", &[&uid, "(BODY.PEEK[HEADER.FIELDS (FROM REPLY-TO TO CC SUBJECT MESSAGE-ID REFERENCES)])"], None)
                .await?;
            let raw = r.data.iter().find_map(|d| d.literal()).unwrap_or(b"").to_vec();
            let h: Headers = mime::parse_headers(&raw);
            let from = h.get("Reply-To").filter(|x| !x.is_empty()).or_else(|| h.get("From"));
            Ok(Some(json!({
                "from": dec(from), "to": dec(h.get("To")), "cc": dec(h.get("Cc")), "subject": dec(h.get("Subject")),
                "message_id_header": h.get_or_empty("Message-ID"), "references": h.get_or_empty("References"),
            })))
        }
        .await;
        Self::logout(m).await;
        res
    }

    /// `{"to", "cc", "subject"}` for replying to a message.
    pub async fn reply_defaults(&mut self, message_id: &str, reply_all: bool) -> R<Value> {
        let orig = self
            .original(Some(message_id))
            .await?
            .unwrap_or_else(|| json!({}));
        Ok(reply_defaults_from(&orig, &self.email, reply_all))
    }

    /// Save a draft (APPEND to Drafts with `\Draft \Seen`).
    pub async fn create_draft(
        &mut self,
        to: &str,
        subject: &str,
        body: &str,
        cc: &str,
        reply_to_message_id: Option<&str>,
        attachments: &[(String, String, Vec<u8>)],
    ) -> R<Value> {
        let orig = self.original(reply_to_message_id).await?;
        let msg = self.compose(to, subject, body, cc, orig.as_ref(), attachments)?;
        let mut m = self.imap().await?;
        let res = async {
            let f = self.folders(&mut m).await?;
            let drafts = match f.get("drafts").filter(|v| !v.is_empty()) {
                Some(d) => d.to_string(),
                None => self.folder_by_name(&mut m, "Drafts", true).await?.unwrap_or_default(),
            };
            let r = m.c.append(&drafts, Some("(\\Draft \\Seen)"), &mime::internaldate_now(), &msg.bytes).await?;
            if !r.ok() {
                return Err(GmailError(format!("保存草稿失败 draft failed: {}", r.py_data())));
            }
            Ok(json!({"saved": true, "to": to, "subject": msg.subject, "folder": self.fname(&drafts)}))
        }
        .await;
        Self::logout(m).await;
        res
    }

    async fn smtp_send(&self, msg: &Composed) -> R<()> {
        let hint = login_hint(&self.provider);
        let token = match &self.token_fn {
            Some(f) => Some(f().await.map_err(|e| GmailError(e.0))?),
            None => None,
        };
        let settings = SmtpSettings {
            host: self.smtp_host.clone(),
            port: self.smtp_port,
            security: self.smtp_security.clone(),
            username: self.username.clone(),
            password: self.password.clone(),
        };
        let from = mime::addresses_only(&[&msg.from])
            .into_iter()
            .next()
            .unwrap_or_else(|| self.email.clone());
        let rcpts = mime::addresses_only(&[&msg.to, &msg.cc]);
        smtp::send(&settings, token.as_deref(), &from, &rcpts, &msg.bytes)
            .await
            .map_err(|e| match e {
                SmtpFail::Auth(t) => {
                    GmailError(format!("SMTP 登录失败 (auth failed): {t}. {hint}"))
                }
                SmtpFail::Other(t) => GmailError(format!("发送失败 send failed: {t}")),
            })
    }

    /// Send over SMTP; on non-Gmail servers make sure a copy lands in Sent.
    pub async fn send(
        &mut self,
        to: &str,
        subject: &str,
        body: &str,
        cc: &str,
        reply_to_message_id: Option<&str>,
        attachments: &[(String, String, Vec<u8>)],
    ) -> R<Value> {
        let orig = self.original(reply_to_message_id).await?;
        let msg = self.compose(to, subject, body, cc, orig.as_ref(), attachments)?;
        self.smtp_send(&msg).await?;
        let mut out = json!({
            "sent": true, "to": to, "cc": cc, "subject": msg.subject, "message_id_header": msg.message_id,
            "attachments": attachments.iter().map(|a| a.0.clone()).collect::<Vec<_>>(),
        });
        if !self.is_gm().await? {
            out["saved_to_sent"] = json!(self.keep_sent_copy(&msg).await);
        }
        Ok(out)
    }

    /// Some servers (e.g. iCloud) do not file mail sent over SMTP into Sent. Check, and append a copy if missing.
    async fn keep_sent_copy(&mut self, msg: &Composed) -> bool {
        let Ok(mut m) = self.imap().await else {
            return false;
        };
        let res: R<bool> = async {
            if self.is_gm().await? {
                return Ok(true);
            }
            let f = self.folders(&mut m).await?;
            let Some(sent) = f.get("sent").filter(|v| !v.is_empty()).map(str::to_string) else {
                return Ok(false);
            };
            let q = quote(&msg.message_id);
            for wait in [2u64, 3] {
                tokio::time::sleep(std::time::Duration::from_secs(wait)).await;
                m.sel = None;
                self.select(&mut m, &sent, true).await?;
                let r =
                    m.c.uid("SEARCH", &["HEADER", "Message-ID", &q], None)
                        .await?;
                if r.ok() && !r.numbers().is_empty() {
                    return Ok(true);
                }
            }
            Ok(m.c
                .append(
                    &sent,
                    Some("(\\Seen)"),
                    &mime::internaldate_now(),
                    &msg.bytes,
                )
                .await?
                .ok())
        }
        .await;
        Self::logout(m).await;
        res.unwrap_or(false)
    }

    // ------------------------------------------------------------ unsubscribe (RFC 2369 / RFC 8058)

    /// Sender/subject/method for each message. Internal: includes the raw targets (`_unsub`, never sent to the agent).
    pub async fn unsubscribe_targets(&mut self, ids: &[String]) -> R<Vec<Value>> {
        let mut m = self.imap().await?;
        let res = async {
            let mut out = Vec::new();
            for mid in ids.iter().take(MAX_UNSUB) {
                let meta = match self.locate(&mut m, mid, true).await {
                    Ok(uid) => self.fetch_meta(&mut m, &[uid], false).await,
                    Err(e) => Err(e),
                };
                let meta = match meta {
                    Ok(x) => x,
                    Err(e) => {
                        out.push(json!({"id": mid, "error": e.0, "method": ""}));
                        continue;
                    }
                };
                let Some(x) = meta.into_iter().next() else {
                    out.push(json!({"id": mid, "error": "not found", "method": ""}));
                    continue;
                };
                let u = x.get("_unsub").cloned().unwrap_or_else(|| json!({}));
                out.push(json!({
                    "id": mid, "from": vs(&x, "from"), "subject": vs(&x, "subject"), "date": vs(&x, "date"),
                    "method": vs(&u, "method"), "_unsub": u,
                }));
            }
            Ok(out)
        }
        .await;
        Self::logout(m).await;
        res
    }

    /// Unsubscribe from each message's list: `[{id, from, subject, method, status, via?, note}]`.
    pub async fn unsubscribe(&mut self, ids: &[String]) -> R<Vec<Value>> {
        let mut results = Vec::new();
        for mut t in self.unsubscribe_targets(ids).await? {
            let u = t
                .as_object_mut()
                .and_then(|o| o.remove("_unsub"))
                .unwrap_or_else(|| json!({}));
            let mut r = Map::new();
            for k in ["id", "from", "subject", "method"] {
                r.insert(k.into(), json!(vs(&t, k)));
            }
            if t.get("error").map(truthy).unwrap_or(false) {
                r.insert("status".into(), json!("failed"));
                r.insert("note".into(), json!(vs(&t, "error")));
            } else if vs(&u, "method").is_empty() {
                r.insert("status".into(), json!("manual"));
                r.insert("note".into(), json!("这封邮件没有标准退订信息 (no List-Unsubscribe header)，请在邮件正文里手动点退订"));
            } else if let Value::Object(done) = self.do_unsub(&u).await {
                r.extend(done);
            }
            results.push(Value::Object(r));
        }
        Ok(results)
    }

    /// Run one unsubscribe: one-click POST, else the mailto email, else open the link.
    pub async fn do_unsub(&mut self, u: &Value) -> Value {
        static EMAIL: LazyLock<Regex> =
            LazyLock::new(|| Regex::new(r"^[^@\s<>,;]+@[^@\s<>,;]+\.[A-Za-z]{2,}$").unwrap());
        let mut errors: Vec<String> = Vec::new();
        let https = vs(u, "https");
        let one_click = u.get("one_click").map(truthy).unwrap_or(false);
        // 1) RFC 8058 one-click: a single POST, no page to confirm
        if one_click && !https.is_empty() {
            let (ok, why) = check_url(&https);
            if ok {
                match (self.unsub_http)(true, https.clone()).await {
                    Ok(code) if code < 400 => {
                        return json!({"status": "done", "via": "one-click", "note": format!("已一键退订 ({})", domain_of(&https))});
                    }
                    Ok(code) => errors.push(format!("one-click HTTP {code}")),
                    Err(name) => errors.push(format!("one-click {name}")),
                }
            } else {
                errors.push(why);
            }
        }
        // 2) mailto: send the unsubscribe email the sender asked for
        let mailto = vs(u, "mailto");
        if !mailto.is_empty() {
            let (to, subj, body) = parse_mailto(&mailto);
            if EMAIL.is_match(&to) {
                match self.send(&to, &subj, &body, "", None, &[]).await {
                    Ok(_) => {
                        let dom = to.rsplit('@').next().unwrap_or("");
                        return json!({"status": "done", "via": "email", "note": format!("已发送退订邮件给 {dom}")});
                    }
                    Err(e) => errors.push(e.0),
                }
            } else {
                errors.push("invalid mailto".into());
            }
        }
        // 3) plain link: open it; many senders unsubscribe on visit, some still want a click on their page
        if !https.is_empty() && !one_click {
            let (ok, why) = check_url(&https);
            if ok {
                match (self.unsub_http)(false, https.clone()).await {
                    Ok(code) if code < 400 => {
                        return json!({"status": "link_opened", "via": "link",
                            "note": format!("已打开退订页面 ({})；部分网站还需要在页面上再点一次确认", domain_of(&https))});
                    }
                    Ok(code) => errors.push(format!("link HTTP {code}")),
                    Err(name) => errors.push(format!("link {name}")),
                }
            } else {
                errors.push(why);
            }
        }
        let note = if errors.is_empty() {
            "unsupported unsubscribe method".to_string()
        } else {
            errors.join("; ")
        };
        json!({"status": "failed", "note": note})
    }

    /// Sign in and report folders + unread count (Connections page "test").
    pub async fn test(&mut self) -> R<Value> {
        let mut m = self.imap().await?;
        let res = async {
            let f = self.folders(&mut m).await?;
            let gm = self.is_gm().await?;
            let r = if gm {
                self.select_all(&mut m, true).await?;
                m.c.uid("SEARCH", &["X-GM-RAW", "\"in:inbox is:unread\""], None).await?
            } else {
                self.select(&mut m, "INBOX", true).await?;
                m.c.uid("SEARCH", &["UNSEEN"], None).await?
            };
            let unread = if r.ok() { json!(r.numbers().len()) } else { Value::Null };
            let name = |k: &str| f.get(k).filter(|v| !v.is_empty()).map(|v| json!(self.fname(v))).unwrap_or(Value::Null);
            let mut out = json!({"ok": true, "provider": self.provider, "drafts": name("drafts"), "sent": name("sent"),
                "unread_inbox": unread});
            if gm {
                out["all_mail"] = f.get("all").map(|v| json!(v)).unwrap_or(Value::Null);
            }
            Ok(out)
        }
        .await;
        Self::logout(m).await;
        res
    }
}

enum EachOp {
    Archive,
    Add(String),
    Remove(String),
}

fn sort_by_date_desc(v: &mut [Value]) {
    v.sort_by(|a, b| {
        date_key(&vs(b, "date"))
            .partial_cmp(&date_key(&vs(a, "date")))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
}

static RE_UID: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"UID (\d+)").unwrap());
static RE_FLAGS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"FLAGS \(([^)]*)\)").unwrap());
static RE_GMID: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"X-GM-MSGID (\d+)").unwrap());
static RE_THRID: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"X-GM-THRID (\d+)").unwrap());
static RE_LABELS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"X-GM-LABELS \(([^)]*)\)").unwrap());
static RE_LABEL_TOK: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#""[^"]*"|\S+"#).unwrap());

/// Parse a `UID FETCH (… FLAGS BODY.PEEK[HEADER.FIELDS …] BODY.PEEK[TEXT]<0.3000>)` response into message dicts
/// (newest first), with snippets redacted and security emails hidden.
pub fn parse_fetch_meta(r: &Resp, gm: bool, key: &str, folder_name: &str) -> Vec<Value> {
    let mut results: Map<String, Value> = Map::new();
    let mut cur: Option<String> = None;
    for item in &r.data {
        let Item::Literal { head, data } = item else {
            continue;
        };
        let head = String::from_utf8_lossy(head).to_string();
        if let Some(uid) = RE_UID.captures(&head) {
            let uid = uid[1].to_string();
            let entry = results
                .entry(uid.clone())
                .or_insert_with(|| json!({"uid": uid}));
            cur = Some(uid.clone());
            let fl = RE_FLAGS.captures(&head).map(|c| c[1].to_string());
            if gm {
                if let Some(g) = RE_GMID.captures(&head) {
                    entry["id"] = json!(g[1].to_string());
                }
                if let Some(t) = RE_THRID.captures(&head) {
                    entry["thread_id"] = json!(t[1].to_string());
                }
                if let Some(lb) = RE_LABELS.captures(&head) {
                    let labels: Vec<String> = RE_LABEL_TOK
                        .find_iter(&lb[1])
                        .map(|x| x.as_str().trim_matches('"').replace("\\\\", "\\"))
                        .collect();
                    entry["labels"] = json!(labels);
                }
            } else {
                let id = format!("{key}-{uid}");
                entry["id"] = json!(id);
                entry["thread_id"] = json!(id);
                let mut labels = vec![folder_name.to_string()];
                if fl
                    .as_deref()
                    .map(|f| f.contains("\\Flagged"))
                    .unwrap_or(false)
                {
                    labels.push("\\Starred".into());
                }
                entry["labels"] = json!(labels);
            }
            if let Some(f) = &fl {
                entry["unread"] = json!(!f.contains("\\Seen"));
                if f.contains("\\Draft") {
                    entry["draft"] = json!(true);
                }
            }
            let is_draft = entry
                .get("labels")
                .and_then(Value::as_array)
                .map(|ls| {
                    ls.iter().any(|x| {
                        let l = py_str(x).to_lowercase();
                        l == "\\draft" || l == "drafts" || l == "[gmail]/drafts"
                    })
                })
                .unwrap_or(false);
            if is_draft {
                entry["draft"] = json!(true);
            }
        }
        let Some(uid) = cur.clone() else {
            continue;
        };
        let entry = results.get_mut(&uid).expect("current entry");
        if head.contains("HEADER.FIELDS") {
            let hdr = mime::parse_headers(data);
            entry["from"] = json!(dec(hdr.get("From")));
            entry["to"] = json!(dec(hdr.get("To")));
            entry["cc"] = json!(dec(hdr.get("Cc")));
            entry["subject"] = json!(dec(hdr.get("Subject")));
            entry["date"] = json!(hdr.get_or_empty("Date"));
            entry["message_id_header"] = json!(hdr.get_or_empty("Message-ID"));
            entry["_unsub"] = parse_list_unsubscribe(
                &hdr.get_or_empty("List-Unsubscribe"),
                &hdr.get_or_empty("List-Unsubscribe-Post"),
            );
        } else if head.contains("BODY[TEXT]") {
            entry["snippet"] = json!(snippet(data));
        }
    }
    let mut out = Vec::new();
    for (_, mut r) in results {
        let sec = is_security_message(&vs(&r, "subject"), "");
        r["security_message"] = json!(sec);
        if sec {
            r["snippet"] = json!("[安全类邮件，内容已隐藏 security email hidden]");
        } else {
            let (s, _) = redact_secrets(&vs(&r, "snippet"));
            r["snippet"] = json!(s);
        }
        if let Some(o) = r.as_object_mut() {
            o.remove("uid");
        }
        let method = r.get("_unsub").map(|u| vs(u, "method")).unwrap_or_default();
        if !method.is_empty() {
            r["unsubscribe"] = json!(method); // one-click | email | link — the URL itself stays inside Sentinel
        }
        out.push(r);
    }
    sort_by_date_desc(&mut out);
    out
}

/// Find an attachment in a raw message (empty `filename` = the first one).
pub fn pick_attachment(raw: &[u8], filename: &str) -> R<(String, String, Vec<u8>)> {
    let want = filename.trim().to_lowercase();
    let mut names: Vec<String> = Vec::new();
    for leaf in leaves(raw) {
        let fname = if leaf.filename.is_some() {
            dec(leaf.filename.as_deref())
        } else {
            String::new()
        };
        if fname.is_empty() && !leaf.disposition.contains("attachment") {
            continue;
        }
        names.push(if fname.is_empty() {
            "(unnamed)".into()
        } else {
            fname.clone()
        });
        if want.is_empty() || fname.to_lowercase() == want {
            if leaf.payload.len() > MAX_ATTACHMENT {
                return Err(GmailError(format!(
                    "附件太大 attachment too large ({} MB)",
                    leaf.payload.len() / 1_000_000
                )));
            }
            let name = if fname.is_empty() {
                "attachment".to_string()
            } else {
                fname
            };
            return Ok((name, leaf.ctype, leaf.payload));
        }
    }
    let list = if names.is_empty() {
        "none".to_string()
    } else {
        names.join(", ")
    };
    Err(GmailError(format!(
        "找不到附件 attachment not found: {filename}. Attachments: {list}"
    )))
}

/// Reply recipients / subject from the original's headers (`reply_defaults`).
pub fn reply_defaults_from(orig: &Value, own_email: &str, reply_all: bool) -> Value {
    let to = vs(orig, "from");
    let mut cc = String::new();
    if reply_all {
        let to_h = vs(orig, "to");
        let cc_h = vs(orig, "cc");
        let others: Vec<String> = mime::getaddresses(&[&to_h, &cc_h])
            .into_iter()
            .filter(|(_, a)| !a.is_empty() && a.to_lowercase() != own_email.to_lowercase())
            .map(|(n, a)| mime::formataddr(&n, &a))
            .collect();
        cc = others.join(", ");
    }
    let subj = vs(orig, "subject");
    let subject = if subj.to_lowercase().starts_with("re:") {
        subj
    } else {
        format!("Re: {subj}")
    };
    json!({"to": to, "cc": cc, "subject": subject})
}

/// `mailto:` target → (address, subject, body) with the Python defaults ("unsubscribe") and limits.
pub fn parse_mailto(mailto: &str) -> (String, String, String) {
    let rest = mailto.split_once(':').map(|(_, r)| r).unwrap_or(mailto);
    let (path, query) = rest.split_once('?').unwrap_or((rest, ""));
    let to = percent_encoding::percent_decode_str(path)
        .decode_utf8_lossy()
        .to_string();
    let first = |key: &str| -> Option<String> {
        form_urlencoded::parse(query.as_bytes())
            .find(|(k, v)| k == key && !v.is_empty())
            .map(|(_, v)| v.to_string())
    };
    let subj = first("subject").unwrap_or_else(|| "unsubscribe".into());
    let body = first("body").unwrap_or_else(|| "unsubscribe".into());
    (
        to,
        take_chars(&subj, 200).to_string(),
        take_chars(&body, 1000).to_string(),
    )
}

// ------------------------------------------------------------------ clients per account (actions.py helpers)

static MS_TOKENS: LazyLock<tokio::sync::Mutex<HashMap<String, (String, f64)>>> =
    LazyLock::new(|| tokio::sync::Mutex::new(HashMap::new()));

/// Forget a cached Outlook access token (`actions._MS_TOKENS.pop(aid)`).
pub async fn forget_ms_token(aid: &str) {
    MS_TOKENS.lock().await.remove(aid);
}

/// Access token for an Outlook mailbox: cached ~1 hour, refreshed with the stored refresh token (which Microsoft
/// may rotate — the new one is saved back to the vault).
pub fn ms_token_fn(store: Option<Arc<Store>>, aid: &str, oauth: Value, ms: MsLogin) -> TokenFn {
    let aid = aid.to_string();
    Arc::new(move || {
        let store = store.clone();
        let aid = aid.clone();
        let oauth = oauth.clone();
        let ms = ms.clone();
        Box::pin(async move {
            let mut cache = MS_TOKENS.lock().await;
            if let Some((tok, exp)) = cache.get(&aid) {
                if !tok.is_empty() && exp - 120.0 > omuse_common::util::now_ts() {
                    return Ok(tok.clone());
                }
            }
            let stored = match &store {
                Some(st) => st.get_secret(&mailboxes::handle(&aid)).ok().flatten(),
                None => None,
            };
            let o = stored
                .and_then(|s| s.get("oauth").cloned())
                .filter(truthy)
                .unwrap_or(oauth);
            let tenant = vs(&o, "tenant");
            let tenant = if tenant.is_empty() {
                "common".to_string()
            } else {
                tenant
            };
            let j = ms
                .refresh(&vs(&o, "client_id"), &tenant, &vs(&o, "refresh_token"))
                .await?;
            let new_rt = vs(&j, "refresh_token");
            if !new_rt.is_empty() && new_rt != vs(&o, "refresh_token") {
                if let Some(st) = &store {
                    let mut o2 = o.clone();
                    o2["refresh_token"] = json!(new_rt);
                    let _ = st.put_secret(
                        "gmail",
                        &json!({"oauth": o2}),
                        Some(&mailboxes::handle(&aid)),
                    );
                }
            }
            let at = vs(&j, "access_token");
            let exp = j.get("expires_in").and_then(|v| {
                v.as_f64()
                    .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
            });
            let exp = exp.filter(|e| *e != 0.0).unwrap_or(3600.0);
            cache.insert(
                aid.clone(),
                (at.clone(), omuse_common::util::now_ts() + exp),
            );
            Ok(at)
        })
    })
}

/// Client for an account dict (from [`mailboxes::accounts`]) and its vault secret (`actions.make_client`).
pub fn make_client(acc: &Value, secret: &Value, store: Option<Arc<Store>>, ms: &MsLogin) -> Gmail {
    let oauth = secret.get("oauth").filter(|o| truthy(o)).cloned();
    let aid = {
        let id = vs(acc, "id");
        if id.is_empty() {
            "g1".to_string()
        } else {
            id
        }
    };
    let token_fn = oauth.map(|o| ms_token_fn(store, &aid, o, ms.clone()));
    let or = |k: &str, d: &str| {
        let v = vs(acc, k);
        if v.is_empty() {
            d.to_string()
        } else {
            v
        }
    };
    let port = |k: &str, d: u16| -> u16 {
        acc.get(k)
            .and_then(|v| {
                v.as_u64()
                    .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
            })
            .filter(|p| *p != 0)
            .map(|p| p as u16)
            .unwrap_or(d)
    };
    let email = vs(acc, "email");
    let mut g = Gmail::new(
        &email,
        &vs(secret, "app_password"),
        &or("imap_host", "imap.gmail.com"),
        &or("smtp_host", "smtp.gmail.com"),
        &vs(acc, "display_name"),
        GmailOptions {
            provider: or("provider", "gmail"),
            imap_port: port("imap_port", 993),
            imap_security: or("imap_security", "ssl"),
            smtp_port: port("smtp_port", 465),
            smtp_security: or("smtp_security", "ssl"),
            username: or("username", &email),
            token_fn,
        },
    );
    g.account_id = aid;
    g
}

#[cfg(test)]
mod tests;
