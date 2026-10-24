//! Pure helpers of the broker (`app/browser/main.py` module level): feed formatting, the registered
//! domain used for pacing, the `_display_alive` check, and the input-format rewrites for `type`.

use std::sync::LazyLock;

use omuse_common::util::{get_str, py_str, take_chars};
use regex::Regex;
use serde_json::Value;

/// Viewport of every task page (`VIEWPORT`).
pub const VIEWPORT_W: u32 = 1280;
pub const VIEWPORT_H: u32 = 800;
/// Up to this much extra, random, on top of the pace gap (`PACE_JITTER`).
pub const PACE_JITTER: f64 = 0.6;
/// Never pace-wait longer than this on one request (`PACE_MAX_WAIT`).
pub const PACE_MAX_WAIT: f64 = 8.0;
/// After a block / 429 on a site, hold further hits this long (`PACE_BLOCK_HOLD`).
pub const PACE_BLOCK_HOLD: f64 = 45.0;
/// User agent sent in headless mode when none is configured (`UA_HEADLESS`).
pub const UA_HEADLESS: &str =
    "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/141.0.0.0 Safari/537.36";
/// Download extensions that are quarantined instead of saved (`BLOCKED_EXT`).
pub const BLOCKED_EXT: &[&str] = &[
    ".exe", ".msi", ".dmg", ".pkg", ".app", ".bat", ".cmd", ".com", ".scr", ".sh", ".ps1", ".vbs",
    ".jar", ".apk", ".deb", ".rpm", ".iso", ".dll", ".so",
];
/// Largest download that is kept (`MAX_DOWNLOAD`).
pub const MAX_DOWNLOAD: u64 = 200 * 1024 * 1024;

/// `format_feed(feed, url)`: an RSS/Atom feed as a compact numbered list.
pub fn format_feed(feed: &Value, url: &str) -> String {
    let items: Vec<Value> = feed
        .get("items")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let name = match feed.get("feed") {
        Some(v) if omuse_common::util::truthy(v) => py_str(v),
        _ => url.to_string(),
    };
    let mut out = vec![format!(
        "📰 RSS/Atom feed「{}」— {} 条 items (newest first as published). 这就是完整内容，不需要重复打开 This is the whole feed; no need to open it again.",
        name,
        items.len()
    )];
    for (i, it) in items.iter().enumerate() {
        let title = match it.get("title") {
            Some(v) if omuse_common::util::truthy(v) => py_str(v),
            _ => "(no title)".to_string(),
        };
        let date = get_str(it, "date");
        let mut line = format!("{}. {}", i + 1, title);
        if !date.is_empty() {
            line.push_str(&format!("  [{}]", date));
        }
        out.push(line);
        let link = get_str(it, "link");
        if !link.is_empty() {
            out.push(format!("   {}", link));
        }
        let summary = get_str(it, "summary");
        if !summary.is_empty() {
            out.push(format!("   {}", summary));
        }
    }
    out.join("\n")
}

/// `_LOADING`: a snapshot line that is only a "Loading…" placeholder.
pub static LOADING: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"(?mi)^\s*(?:\[e\d+\]\s*)?(?:\w+\s+")?(?:Loading(?:\.{1,3}|…)?|加载中(?:\.{1,3}|…)?|正在加载(?:\.{1,3}|…)?)"?\s*$"#,
    )
    .unwrap()
});

/// True when the snapshot still shows a "Loading…" placeholder (`_LOADING.search`).
pub fn looks_loading(snapshot: &str) -> bool {
    LOADING.is_match(snapshot)
}

const CC_SLD2: &[&str] = &["com", "co", "net", "org", "gov", "edu", "ac"];

/// The registered domain used for pacing — decathlon.sg, amazon.com.sg → one key per site (`reg_domain`).
pub fn reg_domain(url: &str) -> String {
    let full = if url.contains("://") {
        url.to_string()
    } else {
        format!("https://{}", url)
    };
    let host = match url::Url::parse(&full) {
        Ok(u) => u
            .host_str()
            .unwrap_or("")
            .to_lowercase()
            .trim_matches('.')
            .to_string(),
        Err(_) => return String::new(),
    };
    let parts: Vec<&str> = host.split('.').filter(|p| !p.is_empty()).collect();
    let n = parts.len();
    if n >= 3 && parts[n - 1].len() == 2 && CC_SLD2.contains(&parts[n - 2]) {
        return parts[n - 3..].join(".");
    }
    if n >= 2 {
        parts[n - 2..].join(".")
    } else {
        host
    }
}

/// True when an X server socket exists for DISPLAY (":99" -> /tmp/.X11-unix/X99); a stale DISPLAY must not pick
/// headed mode (`_display_alive`).
pub fn display_alive(disp: &str) -> bool {
    let n = disp.split('.').next().unwrap_or("").trim_start_matches(':');
    if n.is_empty() {
        return false;
    }
    std::path::Path::new(&format!("/tmp/.X11-unix/X{}", n)).exists()
        || !n.chars().all(|c| c.is_ascii_digit())
}

/// A card expiry from the vault ("12/2028", "12/28", "2028-12", "1228") in the form the box asks for: MM/YYYY when
/// its placeholder says YYYY (or it holds 7 characters), otherwise MM/YY (`expiry_for_field`).
pub fn expiry_for_field(value: &str, hint: &str) -> String {
    static YM: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"^(\d{4})\s*[-/.]\s*(\d{1,2})$").unwrap());
    static MY: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"^(\d{1,2})\s*[-/.\s]?\s*(\d{2}|\d{4})$").unwrap());
    static MAXLEN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"maxlength(7|9)\b").unwrap());
    let v = value.trim();
    let (mm, yy) = if let Some(c) = YM.captures(v) {
        (c[2].to_string(), c[1].to_string())
    } else if let Some(c) = MY.captures(v) {
        (c[1].to_string(), c[2].to_string())
    } else {
        return v.to_string();
    };
    let mm = if mm.len() < 2 { format!("0{}", mm) } else { mm };
    let yyyy = if yy.len() == 4 {
        yy
    } else {
        format!("20{}", yy)
    };
    let h = hint.to_lowercase();
    let long =
        h.contains("yyyy") || h.contains("aaaa") || h.contains("jjjj") || MAXLEN.is_match(&h);
    if long {
        format!("{}/{}", mm, yyyy)
    } else {
        format!("{}/{}", mm, &yyyy[2..])
    }
}

const DATE_KINDS: &[&str] = &["date", "time", "datetime-local", "month", "week"];

/// Rewrite free text into the value format a date/time `<input>` accepts (date YYYY-MM-DD, time HH:MM,
/// datetime-local YYYY-MM-DDTHH:MM, month YYYY-MM). Other inputs, or text that can't be parsed, pass through
/// (`normalize_date_input`).
pub fn normalize_date_input(kind: &str, text: &str) -> String {
    static D: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(\d{4})[-/.年](\d{1,2})[-/.月](\d{1,2})").unwrap());
    static TM: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(\d{1,2})[:：](\d{2})(?:[:：]\d{2})?\s*([AaPp]\.?[Mm]\.?|上午|下午|晚上)?")
            .unwrap()
    });
    static PRE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(上午|下午|晚上)\s*$").unwrap());
    static MO: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(\d{4})[-/.年](\d{1,2})").unwrap());
    if !DATE_KINDS.contains(&kind) {
        return text.to_string();
    }
    let t = text.trim();
    let d = D.captures(t);
    let mut hhmm = String::new();
    if let Some(tm) = TM.captures(t) {
        let h: i64 = tm[1].parse().unwrap_or(-1);
        let m: i64 = tm[2].parse().unwrap_or(-1);
        let mut ap = tm
            .get(3)
            .map(|x| x.as_str().to_lowercase().replace('.', ""))
            .unwrap_or_default();
        if ap.is_empty() {
            // Chinese puts it first: 下午 3:05
            let before = &t[..tm.get(0).unwrap().start()];
            if let Some(p) = PRE.captures(before) {
                ap = p[1].to_string();
            }
        }
        let mut h = h;
        if (ap == "pm" || ap == "下午" || ap == "晚上") && h < 12 {
            h += 12;
        }
        if (ap == "am" || ap == "上午") && h == 12 {
            h = 0;
        }
        if (0..24).contains(&h) && (0..60).contains(&m) {
            hhmm = format!("{:02}:{:02}", h, m);
        }
    }
    let ymd = match &d {
        Some(c) => match (
            c[1].parse::<i64>(),
            c[2].parse::<i64>(),
            c[3].parse::<i64>(),
        ) {
            (Ok(y), Ok(mo), Ok(da)) => format!("{:04}-{:02}-{:02}", y, mo, da),
            _ => String::new(),
        },
        None => String::new(),
    };
    match kind {
        "time" => {
            if hhmm.is_empty() {
                text.to_string()
            } else {
                hhmm
            }
        }
        "date" => {
            if ymd.is_empty() {
                text.to_string()
            } else {
                ymd
            }
        }
        "datetime-local" => {
            if ymd.is_empty() {
                text.to_string()
            } else {
                format!("{}T{}", ymd, if hhmm.is_empty() { "00:00" } else { &hhmm })
            }
        }
        "month" => match MO.captures(t) {
            Some(c) => match (c[1].parse::<i64>(), c[2].parse::<i64>()) {
                (Ok(y), Ok(mo)) => format!("{:04}-{:02}", y, mo),
                _ => text.to_string(),
            },
            None => text.to_string(),
        },
        _ => text.to_string(),
    }
}

/// `str(e)[:n]` for error messages.
pub fn cut(s: &str, n: usize) -> String {
    take_chars(s, n).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn feed_formatting() {
        let txt = format_feed(
            &json!({"feed": "Robotics", "items": [{"title": "A", "link": "https://a", "date": "d", "summary": "s"}]}),
            "u",
        );
        assert!(
            txt.contains("Robotics")
                && txt.contains("1. A  [d]")
                && txt.contains("https://a")
                && txt.contains("不需要重复打开")
        );
    }

    #[test]
    fn reg_domain_keys() {
        assert_eq!(reg_domain("https://www.decathlon.sg/p/1"), "decathlon.sg");
        assert_eq!(reg_domain("https://m.amazon.com.sg/x"), "amazon.com.sg");
        assert_eq!(reg_domain("https://shop.test/"), "shop.test");
        assert_eq!(reg_domain("shop.test:8099/x"), "shop.test");
        assert_eq!(reg_domain(""), "");
    }

    #[test]
    fn date_time_inputs_get_their_iso_format() {
        let n = normalize_date_input;
        assert_eq!(n("time", "2026-10-03 12:00"), "12:00");
        assert_eq!(n("time", "12:00 PM"), "12:00");
        assert_eq!(n("time", "7:30 pm"), "19:30");
        assert_eq!(n("time", "12:15 AM"), "00:15");
        assert_eq!(n("time", "下午 3:05"), "15:05");
        assert_eq!(n("date", "2026/10/3"), "2026-10-03");
        assert_eq!(n("date", "2026年12月20日"), "2026-12-20");
        assert_eq!(n("datetime-local", "2026-10-03 9:00"), "2026-10-03T09:00");
        assert_eq!(n("month", "2026-9"), "2026-09");
        assert_eq!(n("text", "12:00 PM"), "12:00 PM");
        assert_eq!(n("time", "noon"), "noon");
    }

    #[test]
    fn expiry_formats() {
        assert_eq!(expiry_for_field("12/2028", "MM/YY"), "12/28");
        assert_eq!(expiry_for_field("2028-12", ""), "12/28");
        assert_eq!(expiry_for_field("12/28", "MM/YYYY"), "12/2028");
        assert_eq!(expiry_for_field("1228", "maxlength7"), "12/2028");
        assert_eq!(expiry_for_field("nonsense", ""), "nonsense");
    }

    #[test]
    fn loading_detection() {
        assert!(looks_loading("header\n  Loading...\nfooter"));
        assert!(looks_loading("[e3] div \"加载中…\""));
        assert!(!looks_loading("  Kiprun socks Run 100 S$4.90"));
    }

    #[test]
    fn display_alive_rules() {
        assert!(!display_alive(""));
        assert!(!display_alive(":"));
        assert!(display_alive("myhost:0"));
        assert!(!display_alive(":987654"));
    }
}
