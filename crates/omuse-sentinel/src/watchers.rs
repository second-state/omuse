//! Event sources for triggers ("when X happens, do Y").
//!
//! The runtime's scheduler polls Sentinel's /internal/watch with a source, its parameters and the cursor it
//! got last time. Sentinel (which holds the credentials) looks for new items and returns them plus a new
//! cursor. The first poll only sets the cursor, so existing backlog never fires a trigger.
//! Items written by OMuse itself (its own Slack messages, its own Notion edits) are skipped to avoid loops.

use std::sync::LazyLock;

use regex::Regex;
use serde_json::{json, Map, Value};
use sha1::Sha1;
use sha2::{Digest, Sha256};

use omuse_apps::notion::page_brief;
use omuse_common::guard;
use omuse_common::pyjson::py_repr;
use omuse_common::util::{char_len, get_str, now_ts, py_str, slice_chars, take_chars, truthy};
use omuse_mail::mailboxes;

use crate::actions::{self, ActionError};
use crate::state::AppState;

/// The trigger sources (`SOURCES`): connector, label and parameter help.
pub static SOURCES: LazyLock<Value> = LazyLock::new(|| {
    json!({
        "gmail.new_email": {"connector": "gmail", "label": "收到新邮件 New email",
                            "params": {"query": "可选搜索条件（Gmail 语法，所有邮箱通用），如 from:boss@x.com 或 is:unread",
                                       "account": "可选：只看某个邮箱"}},
        "slack.new_message": {"connector": "slack", "label": "Slack 新消息 New Slack message",
                              "params": {"channel": "频道，如 #sales", "keyword": "可选：包含关键词才触发",
                                         "mentions_only": "可选：只在 @我 时触发"}},
        "notion.db_changed": {"connector": "notion", "label": "Notion 数据库有新增/修改 Notion database changed",
                              "params": {"database_id": "数据库 ID 或链接"}},
        "web.page": {"connector": "browser", "label": "网页变化 Web page change",
                     "params": {"url": "要监控的网页 page to watch",
                                "mode": "change（内容有变化）| text（出现某段文字）| price_below（价格低于阈值）",
                                "text": "mode=text 时要等待出现的文字", "threshold": "mode=price_below 时的价格阈值，如 15",
                                "keyword": "可选：只看这个词附近的价格/文字 (e.g. the product name)"}},
    })
});
pub const MAX_EVENTS: usize = 20;

/// A watch failure (the routes answer `{"error": ..., "events": [], "cursor": <old>}`).
#[derive(Debug, Clone, thiserror::Error)]
#[error("{0}")]
pub struct WatchError(pub String);

/// `WatchError(str(e)[:300])` for connector errors.
fn werr(e: impl std::fmt::Display) -> WatchError {
    WatchError(take_chars(&e.to_string(), 300).to_string())
}

fn sarg(v: &Value, k: &str) -> String {
    match v.get(k) {
        Some(x) if truthy(x) => py_str(x),
        _ => String::new(),
    }
}

type Polled = Result<(Vec<Value>, Value), WatchError>;

async fn gmail(st: &AppState, params: &Value, cursor: Option<&Value>) -> Polled {
    let acc = sarg(params, "account").trim().to_string();
    let accounts: Vec<Value> = mailboxes::ready_accounts(&st.store)
        .map_err(werr)?
        .into_iter()
        .filter(|a| {
            acc.is_empty()
                || get_str(a, "email").to_lowercase() == acc.to_lowercase()
                || get_str(a, "id") == acc
        })
        .collect();
    if accounts.is_empty() {
        return Err(WatchError(
            "邮箱未连接或找不到指定邮箱 (mailbox not connected)".into(),
        ));
    }
    let q = sarg(params, "query").trim().to_string();
    let q = if q.is_empty() {
        "in:inbox".to_string()
    } else {
        q
    } + " newer_than:2d";
    let mut seen: Map<String, Value> = cursor
        .and_then(|c| c.get("seen"))
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let mut events = Vec::new();
    for a in &accounts {
        let aid = get_str(a, "id");
        let mut g = actions::gmail_client(st, Some(&aid)).map_err(werr)?;
        let mut msgs = Vec::new();
        for mut m in g.search(&q, 30).await.map_err(werr)? {
            actions::tag(&mut m, &g);
            msgs.push(m);
        }
        let mut old: Vec<String> = Vec::new();
        for x in seen
            .get(&aid)
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
        {
            let s = py_str(&x);
            if !old.contains(&s) {
                old.push(s);
            }
        }
        if cursor.is_some() && seen.contains_key(&aid) {
            for m in &msgs {
                if !old.contains(&get_str(m, "id")) {
                    let mut ev = Map::new();
                    for k in [
                        "id",
                        "thread_id",
                        "account",
                        "from",
                        "to",
                        "subject",
                        "date",
                        "snippet",
                        "security_message",
                    ] {
                        if let Some(v) = m.get(k).filter(|v| !v.is_null()) {
                            ev.insert(k.into(), v.clone());
                        }
                    }
                    events.push(Value::Object(ev));
                }
            }
        }
        let mut all = old.clone();
        for m in &msgs {
            let id = get_str(m, "id");
            if !old.contains(&id) {
                all.push(id);
            }
        }
        let skip = all.len().saturating_sub(500);
        seen.insert(aid, json!(all[skip..]));
    }
    Ok((events, json!({"seen": seen})))
}

fn ts_f(v: &str) -> f64 {
    v.trim().parse::<f64>().unwrap_or(0.0)
}

async fn slack(st: &AppState, params: &Value, cursor: Option<&Value>) -> Polled {
    let ch = sarg(params, "channel").trim().to_string();
    if ch.is_empty() {
        return Err(WatchError(
            "请指定要监听的 Slack 频道 (channel required)".into(),
        ));
    }
    let conf = st.store.connection("slack").map_err(werr)?["config"].take();
    let me: Vec<String> = ["user_id", "bot_id"]
        .iter()
        .filter_map(|k| conf.get(*k).filter(|v| !v.is_null()).map(py_str))
        .filter(|s| !s.is_empty())
        .collect();
    let s = actions::slack_client(st).map_err(werr)?;
    let info = s.resolve_channel(&ch).await.map_err(werr)?;
    let oldest = cursor
        .and_then(|c| c.get("ts"))
        .filter(|v| !v.is_null())
        .cloned();
    let oldest_param = oldest.clone().filter(truthy).unwrap_or(Value::Null);
    let d = s
        .call(
            "conversations.history",
            &[
                ("channel", info.get("id").cloned().unwrap_or(Value::Null)),
                ("limit", json!(if oldest.is_none() { 1 } else { 50 })),
                ("oldest", oldest_param),
            ],
        )
        .await
        .map_err(werr)?;
    let mut raw: Vec<Value> = d
        .get("messages")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    raw.sort_by(|a, b| {
        ts_f(&get_str(a, "ts"))
            .partial_cmp(&ts_f(&get_str(b, "ts")))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let start = oldest
        .as_ref()
        .filter(|o| truthy(o))
        .map(py_str)
        .unwrap_or_else(|| "0".into());
    let mut newest = start;
    for m in &raw {
        let t = m.get("ts").map(py_str).unwrap_or_else(|| "0".into());
        if ts_f(&t) > ts_f(&newest) {
            newest = t;
        }
    }
    if oldest.is_none() {
        let ts = if raw.is_empty() {
            format!("{:.6}", now_ts())
        } else {
            newest
        };
        return Ok((vec![], json!({"ts": ts})));
    }
    let kw = sarg(params, "keyword").trim().to_lowercase();
    let only_me = matches!(
        sarg(params, "mentions_only").to_lowercase().as_str(),
        "1" | "true" | "yes" | "on"
    );
    let mut events = Vec::new();
    for m in &raw {
        let user = m.get("user").and_then(Value::as_str).unwrap_or("");
        let bot = m
            .get("bot_id")
            .filter(|b| truthy(b))
            .map(py_str)
            .unwrap_or_default();
        let subtype = m.get("subtype").and_then(Value::as_str).unwrap_or("");
        if me.iter().any(|x| x == user)
            || (!bot.is_empty() && me.contains(&bot))
            || subtype == "channel_join"
            || subtype == "channel_leave"
        {
            continue;
        }
        let text = sarg(m, "text");
        if !kw.is_empty() && !text.to_lowercase().contains(&kw) {
            continue;
        }
        if only_me && !me.iter().any(|u| text.contains(&format!("<@{u}>"))) {
            continue;
        }
        events.push(s.msg(m, &info).await);
    }
    Ok((events, json!({"ts": newest})))
}

async fn notion(st: &AppState, params: &Value, cursor: Option<&Value>) -> Polled {
    let db = sarg(params, "database_id").trim().to_string();
    if db.is_empty() {
        return Err(WatchError(
            "请指定要监听的 Notion 数据库 (database_id required)".into(),
        ));
    }
    let bot = get_str(
        &st.store.connection("notion").map_err(werr)?["config"],
        "bot_id",
    );
    let n = actions::notion_client(st).map_err(werr)?;
    let since = cursor
        .and_then(|c| c.get("since"))
        .filter(|v| truthy(v))
        .map(py_str)
        .unwrap_or_else(|| {
            chrono::Utc::now()
                .format("%Y-%m-%dT%H:%M:00.000Z")
                .to_string()
        });
    let mut seen: Map<String, Value> = cursor
        .and_then(|c| c.get("seen"))
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let (title, rows) = n.edited_since(&db, &since).await.map_err(werr)?;
    let mut events = Vec::new();
    for p in &rows {
        let let_ = get_str(p, "last_edited_time");
        let pid = get_str(p, "id");
        if seen.get(&pid).and_then(Value::as_str) == Some(let_.as_str()) {
            continue;
        }
        seen.insert(pid, json!(let_));
        let edited_by = p
            .get("last_edited_by")
            .and_then(|b| b.get("id"))
            .and_then(Value::as_str)
            .unwrap_or("");
        if cursor.is_none() || (!bot.is_empty() && edited_by == bot) {
            continue;
        }
        let mut ev = page_brief(p);
        ev["database"] = json!(title);
        ev["change"] = json!(if get_str(p, "created_time") == let_ {
            "new"
        } else {
            "edited"
        });
        events.push(ev);
    }
    let mut newest = since.clone();
    for p in &rows {
        let t = get_str(p, "last_edited_time");
        if t > newest {
            newest = t;
        }
    }
    let seen: Map<String, Value> = seen
        .into_iter()
        .filter(|(_, v)| v.as_str().map(|s| s >= newest.as_str()).unwrap_or(false))
        .collect();
    Ok((events, json!({"since": newest, "seen": seen})))
}

/// Poll one connector source (`gmail.new_email`, `slack.new_message`, `notion.db_changed`) for new events.
/// `cursor` `None` (or JSON null) = first poll: only the cursor is set.
pub async fn poll(
    st: &AppState,
    source: &str,
    params: &Value,
    cursor: Option<&Value>,
) -> Result<Value, WatchError> {
    if SOURCES.get(source).is_none() {
        return Err(WatchError(format!(
            "未知的触发来源 unknown source: {source}"
        )));
    }
    let cursor = cursor.filter(|c| !c.is_null());
    let empty = json!({});
    let params = if params.is_object() { params } else { &empty };
    let (events, new_cursor) = match source {
        "gmail.new_email" => gmail(st, params, cursor).await?,
        "slack.new_message" => slack(st, params, cursor).await?,
        "notion.db_changed" => notion(st, params, cursor).await?,
        other => return Err(WatchError(format!("'{other}'"))),
    };
    Ok(result(events, new_cursor))
}

fn result(mut events: Vec<Value>, cursor: Value) -> Value {
    events.truncate(MAX_EVENTS);
    let injection = if events.is_empty() {
        vec![]
    } else {
        guard::scan_injection(&events.iter().map(py_repr).collect::<Vec<_>>().join("\n"))
    };
    json!({"events": events, "cursor": cursor, "injection": injection})
}

// ------------------------------------------------------------------ web page watch (OpenMuse-style tracking)

static REF: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\[(?:f\d+)?e\d+\]\s*").unwrap());
static ARROW_URL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s→\s\S+").unwrap());
static PRICE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(?:S\$|US\$|HK\$|A\$|C\$|RM|SGD|USD|HKD|RMB|CNY|¥|￥|€|£|\$)\s?(\d{1,3}(?:,\d{3})+(?:\.\d{1,2})?|\d+(?:\.\d{1,2})?)")
        .unwrap()
});
static WS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s+").unwrap());

/// The words a person reads on the page: drop element refs and link targets so layout churn doesn't count.
pub fn page_text(snapshot: &str) -> String {
    let mut lines = Vec::new();
    for ln in crate::telegram_bot::splitlines(snapshot) {
        let a = REF.replace_all(ln, "");
        let b = ARROW_URL.replace_all(&a, "");
        let ln = b.trim();
        if !ln.is_empty() && !ln.starts_with("…[") && !ln.starts_with("(showing the part") {
            lines.push(WS.replace_all(ln, " ").into_owned());
        }
    }
    lines.join("\n")
}

const CUR: &str = r"(?:S\$|US\$|HK\$|A\$|C\$|RM|SGD|USD|HKD|RMB|CNY|¥|￥|€|£|\$)";
// shops often draw a price as separate pieces ("S$ 31 . 43", "S$31\n.43", Amazon's "S$ 31 43"): glue them back
static SPLIT: LazyLock<fancy_regex::Regex> = LazyLock::new(|| {
    fancy_regex::Regex::new(&format!(
        r"(?i)({CUR}\s?\d{{1,3}}(?:,\d{{3}})*|{CUR}\s?\d+)(?:\s+\.\s*|\s*\.\s+|\s+)(\d{{2}})(?![\d.,%])"
    ))
    .unwrap()
});
pub const KEYWORD_WINDOW: usize = 600;
pub const HEADING_WINDOW: usize = 3500;

fn glue(text: &str) -> String {
    SPLIT.replace_all(text, "${1}.${2}").into_owned()
}

fn hits(span: &str, first_only: bool) -> Vec<(f64, String)> {
    let n = char_len(span);
    let mut out = Vec::new();
    for m in PRICE.captures_iter(span) {
        let whole = m.get(0).unwrap();
        if first_only
            && n >= KEYWORD_WINDOW.min(HEADING_WINDOW)
            && char_len(&span[..whole.end()]) + 3 >= n
        {
            break; // cut off at the window's edge: don't read "31.4" out of "31.43"
        }
        let Ok(v) = m[1].replace(',', "").parse::<f64>() else {
            continue;
        };
        out.push((v, whole.as_str().trim().to_string()));
        if first_only {
            break;
        }
    }
    out
}

/// (value, what was read) for the prices on the page. With a keyword: only THE price of that item — the first
/// price after the first mention of the keyword that has one (later mentions are usually "related products").
pub fn price_hits(text: &str, keyword: &str) -> Vec<(f64, String)> {
    let text = glue(text);
    if keyword.is_empty() {
        return hits(&text, false);
    }
    let low = text.to_lowercase();
    let kw = regex::escape(&keyword.to_lowercase());
    // a product page: the price that follows the page's own title (a heading naming the item) is the item's price, even
    // when colour swatches and bullet points sit between them (Amazon's buy box comes ~2000 characters after the title)
    let heading = Regex::new(&format!(r"(?m)^#{{1,3}} [^\n]*{kw}")).expect("heading regex");
    for m in heading.find_iter(&low) {
        let ci = char_len(&low[..m.start()]);
        let got = hits(slice_chars(&text, ci, ci + HEADING_WINDOW), true);
        if !got.is_empty() {
            return got;
        }
    }
    let plain = Regex::new(&kw).expect("keyword regex");
    for m in plain.find_iter(&low) {
        let ci = char_len(&low[..m.start()]);
        let got = hits(slice_chars(&text, ci, ci + KEYWORD_WINDOW), true);
        if !got.is_empty() {
            return got;
        }
    }
    vec![]
}

/// The price values of [`price_hits`].
pub fn prices(text: &str, keyword: &str) -> Vec<f64> {
    price_hits(text, keyword)
        .into_iter()
        .map(|(v, _)| v)
        .collect()
}

/// Python `str.find` as a character index.
fn find_chars(hay: &str, needle: &str) -> Option<usize> {
    hay.find(needle).map(|b| char_len(&hay[..b]))
}

fn around(text: &str, needle: &str, before: usize, after: usize) -> String {
    let Some(i) = find_chars(text, needle) else {
        return String::new();
    };
    let s = slice_chars(text, i.saturating_sub(before), i + char_len(needle) + after);
    WS.replace_all(s, " ").trim().to_string()
}

/// Python `format(v, "g")`.
pub fn fmt_g(v: f64) -> String {
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
    let sci = format!("{v:.5e}");
    let (mant, exp) = sci.split_once('e').unwrap_or((&sci, "0"));
    let exp: i32 = exp.parse().unwrap_or(0);
    let strip = |s: String| -> String {
        if s.contains('.') {
            s.trim_end_matches('0').trim_end_matches('.').to_string()
        } else {
            s
        }
    };
    if (-4..6).contains(&exp) {
        strip(format!("{:.*}", (5 - exp) as usize, v))
    } else {
        let sign = if exp < 0 { '-' } else { '+' };
        format!("{}e{}{:02}", strip(mant.to_string()), sign, exp.abs())
    }
}

/// Compare this observation with the previous one (the cursor). Only a *new* change / newly met condition fires,
/// so the same alert never repeats; the first observation only saves the baseline.
pub fn evaluate(
    params: &Value,
    text: &str,
    title: &str,
    url: &str,
    cursor: Option<&Value>,
) -> Result<(Vec<Value>, Value), WatchError> {
    let cursor = cursor.filter(|c| !c.is_null());
    let mode = sarg(params, "mode").trim().to_lowercase();
    let mode = if mode.is_empty() {
        "change".to_string()
    } else {
        mode
    };
    let lines: Vec<&str> = crate::telegram_bot::splitlines(text)
        .into_iter()
        .filter(|l| !l.trim().is_empty())
        .collect();
    let h = hex::encode(Sha256::digest(text.as_bytes()));
    let line_h: Vec<String> = lines
        .iter()
        .map(|l| hex::encode(Sha1::digest(l.as_bytes()))[..12].to_string())
        .collect();
    let skip = line_h.len().saturating_sub(600);
    let mut cur = json!({"hash": h, "lines": line_h[skip..], "title": take_chars(title, 200)});
    let empty = json!({});
    let prev = cursor.unwrap_or(&empty);
    let prev_met = prev.get("met").is_some_and(truthy);
    let mut events: Vec<Value> = Vec::new();
    if mode == "text" {
        let want = sarg(params, "text").trim().to_string();
        if want.is_empty() {
            return Err(WatchError(
                "mode=text 需要指定要等待的文字 (text required)".into(),
            ));
        }
        let idx = find_chars(&text.to_lowercase(), &want.to_lowercase());
        let met = idx.is_some();
        cur["met"] = json!(met);
        cur["seen"] = json!(format!(
            "{}{}",
            if met {
                "已出现 present: "
            } else {
                "还没有 not yet: "
            },
            take_chars(&want, 60)
        ));
        if let (Some(idx), Some(_)) = (idx, cursor) {
            if !prev_met {
                events.push(json!({"url": url, "title": title, "condition": format!("text appeared: {want}"),
                    "excerpt": slice_chars(text, idx.saturating_sub(120), idx + char_len(&want) + 200)}));
            }
        }
    } else if mode == "price_below" {
        let raw = sarg(params, "threshold").replace(',', "");
        let limit: f64 = raw
            .trim_start_matches(['$', 'S'])
            .trim()
            .parse()
            .map_err(|_| WatchError("mode=price_below 需要数字阈值 threshold, e.g. 15".into()))?;
        let kw = sarg(params, "keyword").trim().to_string();
        let got = price_hits(text, &kw);
        if got.is_empty() {
            let mut other: Vec<f64> = price_hits(text, "").into_iter().map(|(v, _)| v).collect();
            other.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            other.dedup();
            other.truncate(8);
            if !kw.is_empty() {
                let tail = if other.is_empty() {
                    "；整个页面都没有价格（可能缺货、需要选款式，或网站换了页面）no price anywhere on the page".to_string()
                } else {
                    format!(
                        "；页面上的其他价格 other prices on the page: {}",
                        other
                            .iter()
                            .map(|v| fmt_g(*v))
                            .collect::<Vec<_>>()
                            .join(", ")
                    )
                };
                return Err(WatchError(format!(
                    "页面上“{kw}”附近没找到价格 (no price found after '{kw}'){tail}"
                )));
            }
            return Err(WatchError("页面上没找到价格（可能缺货、需要先选款式，或网站换了页面）no price found on the page".into()));
        }
        let found: Vec<f64> = got.iter().map(|(v, _)| *v).collect();
        let low = found.iter().cloned().fold(f64::INFINITY, f64::min);
        let glued = glue(text);
        let read = got
            .iter()
            .min_by(|a, b| {
                a.0.partial_cmp(&b.0)
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then_with(|| a.1.cmp(&b.1))
            })
            .map(|x| x.1.clone())
            .unwrap_or_default();
        let shown = WS.replace_all(&read, "").into_owned();
        let ctx = around(&glued, &read, 60, 0);
        let extra = if !kw.is_empty() && ctx.ends_with(&read) {
            let head = slice_chars(&ctx, 0, char_len(&ctx) - char_len(&read)).trim();
            let n = char_len(head);
            format!(" · …{}", slice_chars(head, n.saturating_sub(60), n))
        } else {
            String::new()
        };
        cur["seen"] = json!(shown + &extra);
        let met = low < limit;
        cur["low"] = json!(low);
        cur["met"] = json!(met);
        let alerted = prev
            .get("alerted")
            .filter(|a| !a.is_null())
            .and_then(Value::as_f64);
        if met && cursor.is_some() && (!prev_met || alerted.is_some_and(|a| low < a)) {
            cur["alerted"] = json!(low);
            let mut uniq = found.clone();
            uniq.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            uniq.dedup();
            uniq.truncate(10);
            events.push(json!({"url": url, "title": title, "condition": format!("price {} < {}", fmt_g(low), fmt_g(limit)),
                "prices_seen": uniq, "excerpt": around(&glued, &read, 110, 0)}));
        } else if met {
            cur["alerted"] = json!(alerted.unwrap_or(low));
        }
    } else if let Some(prev_hash) = prev.get("hash").filter(|h| truthy(h)) {
        if cursor.is_some() && prev_hash.as_str() != Some(h.as_str()) {
            let old: Vec<String> = prev
                .get("lines")
                .and_then(Value::as_array)
                .map(|a| a.iter().map(py_str).collect())
                .unwrap_or_default();
            let added: Vec<String> = lines
                .iter()
                .zip(line_h.iter())
                .filter(|(_, lh)| !old.contains(lh))
                .map(|(ln, _)| truncate_line(ln, 240))
                .take(8)
                .collect();
            if !added.is_empty() || line_h.len() != old.len() {
                events.push(json!({"url": url, "title": title, "condition": "page changed", "added_lines": added}));
            }
        }
    }
    Ok((events, cur))
}

/// `s` cut to `n` characters with "…".
pub fn truncate_line(s: &str, n: usize) -> String {
    if char_len(s) <= n {
        s.to_string()
    } else {
        format!("{}…", take_chars(s, n))
    }
}

/// Watch a web page through the browser broker (`web.page`).
pub async fn poll_web(
    st: &AppState,
    params: &Value,
    cursor: Option<&Value>,
) -> Result<Value, WatchError> {
    let mut url = sarg(params, "url").trim().to_string();
    if !url.is_empty() && !url.contains("://") {
        url = format!("https://{url}");
    }
    let (ok, why) = guard::check_url(&url);
    if !ok {
        return Err(WatchError(why));
    }
    let dom = guard::domain_of(&url);
    let blocked: Vec<String> = st.store.connection("browser").map_err(werr)?["config"]
        .get("blocked_domains")
        .and_then(Value::as_array)
        .map(|a| a.iter().map(py_str).collect())
        .unwrap_or_default();
    if !dom.is_empty() && blocked.contains(&dom) {
        return Err(WatchError(format!(
            "域名 {dom} 在黑名单中 (blocked domain)"
        )));
    }
    let tid = format!("watch-{}", &hex::encode(Sha1::digest(url.as_bytes()))[..10]);
    let snap = actions::broker(
        st,
        "POST",
        "/agent/navigate",
        Some(&json!({"task_id": tid, "url": url, "max_chars": 40000})),
        90.0,
    )
    .await
    .map_err(|e: ActionError| werr(e))?;
    if let Some(b) = snap.get("blocked").filter(|b| truthy(b)) {
        let detail = b.get("detail").map(py_str).unwrap_or_default();
        return Err(WatchError(format!(
            "网站拦截了自动浏览器 (bot wall: {detail})"
        )));
    }
    let (text, _) = guard::redact_secrets(&page_text(&sarg(&snap, "snapshot")));
    let snap_url = sarg(&snap, "url");
    let (events, cur) = evaluate(
        params,
        &text,
        &sarg(&snap, "title"),
        if snap_url.is_empty() { &url } else { &snap_url },
        cursor,
    )?;
    Ok(result(events, cur))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn web_watch_evaluate() {
        assert_eq!(
            prices(
                "Aurora Case\nPrice: S$18.00\nOther: Basic S$12.90",
                "aurora"
            ),
            vec![18.0]
        );
        assert_eq!(prices("A $1,299.00 B S$15.50", ""), vec![1299.0, 15.5]);
        assert_eq!(prices("Price: S$ 31 . 43 x", ""), vec![31.43]);
        assert_eq!(prices("S$31\n.43", ""), vec![31.43]);
        let amz = format!(
            "# SUPFINE Case {}\nVisit the SUPFINE Store\n4.6\nS$31.43\n## Related\nSUPFINE Clear S$19.99",
            "x".repeat(200)
        );
        assert_eq!(prices(&amz, "supfine"), vec![31.43]);
        let page = format!(
            "Customers also viewed\nSUPFINE Clear S$19.99\n# SUPFINE Magnetic Case Deep Blue\nColour Name: Deep Blue\n{}\nS$31.43\n## Related\nSUPFINE Stand S$29.18",
            "bullet ".repeat(300)
        );
        assert_eq!(prices(&page, "SUPFINE"), vec![31.43]);
        assert_eq!(
            prices(&format!("SUPFINE {}S$31.43", "y".repeat(587)), "SUPFINE"),
            Vec::<f64>::new()
        );
        assert!(evaluate(
            &json!({"mode": "price_below", "threshold": "20", "keyword": "zebra"}),
            "Aurora S$18",
            "t",
            "u",
            None
        )
        .is_err());
        let (_ev, c) = evaluate(
            &json!({"mode": "price_below", "threshold": "20", "keyword": "aurora"}),
            "Aurora case S$18",
            "t",
            "u",
            None,
        )
        .unwrap();
        assert!(get_str(&c, "seen").contains("S$18"));
        assert!(!page_text("[e3] link \"Buy\" → https://x.test/a\n# Title").contains("[e3]"));
        let p = json!({"mode": "price_below", "threshold": "20"});
        let (ev, c) = evaluate(&p, "S$18", "t", "u", None).unwrap();
        assert!(ev.is_empty() && c["met"] == true);
        let (_ev, c) = evaluate(&p, "S$25", "t", "u", Some(&c)).unwrap();
        let (ev, c) = evaluate(&p, "S$18", "t", "u", Some(&c)).unwrap();
        assert_eq!(ev.len(), 1);
        assert_eq!(ev[0]["condition"], "price 18 < 20");
        assert!(evaluate(&p, "S$18", "t", "u", Some(&c))
            .unwrap()
            .0
            .is_empty());
        let (_ev, c) = evaluate(&json!({"mode": "change"}), "a\nb", "t", "u", None).unwrap();
        let (ev, _c) = evaluate(&json!({"mode": "change"}), "a\nb\nc", "t", "u", Some(&c)).unwrap();
        assert_eq!(ev[0]["added_lines"], json!(["c"]));
    }

    #[test]
    fn text_mode_and_formatting() {
        let p = json!({"mode": "text", "text": "In stock"});
        let (ev, c) = evaluate(&p, "Sold out", "t", "u", None).unwrap();
        assert!(ev.is_empty() && c["met"] == false);
        let (ev, _) = evaluate(&p, "now IN STOCK!", "t", "u", Some(&c)).unwrap();
        assert_eq!(ev[0]["condition"], "text appeared: In stock");
        assert_eq!(fmt_g(18.0), "18");
        assert_eq!(fmt_g(15.5), "15.5");
        assert_eq!(fmt_g(1299.0), "1299");
        assert_eq!(fmt_g(1234567.0), "1.23457e+06");
        assert_eq!(fmt_g(0.0001), "0.0001");
        assert_eq!(fmt_g(0.00001), "1e-05");
        assert_eq!(truncate_line("abc", 2), "ab…");
    }

    #[test]
    fn sources_table() {
        assert_eq!(get_str(&SOURCES["web.page"], "connector"), "browser");
        assert_eq!(
            SOURCES
                .as_object()
                .unwrap()
                .keys()
                .collect::<Vec<_>>()
                .len(),
            4
        );
    }
}
