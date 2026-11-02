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
                                "item": "mode=price_below：商品名 the item whose price is watched",
                                "price_sel": "可选：价格元素的 CSS 选择器 (optional: the price element's CSS selector)",
                                "price_attr": "可选：价格在元素的这个属性里 (optional: the attribute holding the price)",
                                "read": "element（按元素读）| model（模型从页面读出 JSON）— 建立监控时决定"}},
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

/// A price element's value as `(amount, currency)`: "S$25.00", "$1,299", "25,00 €", "SGD 31.43", "1299.00". The value
/// was read by its element (a known site's or the model's selector), so it already is a price: this only checks its
/// shape — one number with at most one currency of up to four characters before or after it. `None` for anything
/// else ("From S$25", two prices in one element, words).
pub fn parse_amount(value: &str) -> Option<(f64, String)> {
    let t = value.trim();
    let start = t.find(|c: char| c.is_ascii_digit())?;
    let end = t.rfind(|c: char| c.is_ascii_digit())? + 1;
    let (pre, num, post) = (t[..start].trim(), &t[start..end], t[end..].trim());
    if !num
        .chars()
        .all(|c| c.is_ascii_digit() || c == ',' || c == '.')
    {
        return None;
    }
    let currency = match (pre.is_empty(), post.is_empty()) {
        (true, true) => "",
        (false, true) => pre,
        (true, false) => post,
        (false, false) => return None,
    };
    if char_len(currency) > 4
        || currency
            .chars()
            .any(|c| c.is_whitespace() || c.is_ascii_digit())
    {
        return None;
    }
    let (commas, dots) = (num.matches(',').count(), num.matches('.').count());
    let plain = match (commas, dots) {
        (0, 0) => num.to_string(),
        (_, 0) => {
            // one comma with one or two digits after it is a decimal comma ("25,00"); otherwise thousands
            let tail = num.rsplit(',').next().unwrap_or("");
            if commas == 1 && (1..=2).contains(&tail.len()) {
                num.replace(',', ".")
            } else {
                num.replace(',', "")
            }
        }
        (0, 1) => num.to_string(),
        (0, _) => num.replace('.', ""),
        _ => {
            // both: the one that comes last is the decimal mark
            if num.rfind(',') > num.rfind('.') {
                num.replace('.', "").replace(',', ".")
            } else {
                num.replace(',', "")
            }
        }
    };
    let v: f64 = plain.parse().ok()?;
    (v.is_finite() && v > 0.0).then(|| (v, currency.to_string()))
}

/// A price as the notification and the watch card show it: "S$25.00", "€9.50", "SGD 31.43", "25.00".
pub fn show_price(currency: &str, price: f64) -> String {
    let cur = currency.trim();
    if cur.is_empty() {
        format!("{price:.2}")
    } else if cur.chars().all(|c| c.is_ascii_alphabetic()) {
        format!("{cur} {price:.2}")
    } else {
        format!("{cur}{price:.2}")
    }
}

/// Python `str.find` as a character index.
fn find_chars(hay: &str, needle: &str) -> Option<usize> {
    hay.find(needle).map(|b| char_len(&hay[..b]))
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
        // a price is never looked for in the page's text: it is read by its element or by the model
        // (`poll_web`), then compared in `evaluate_price`
        return Err(WatchError(
            "mode=price_below 的价格由元素或模型读取 (a price watch is compared by evaluate_price)"
                .into(),
        ));
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

/// The threshold of a price watch (a number the model or the user gave, possibly with its currency: "20", "S$20").
fn threshold(params: &Value) -> Result<f64, WatchError> {
    parse_amount(&sarg(params, "threshold"))
        .map(|(v, _)| v)
        .ok_or_else(|| WatchError("mode=price_below 需要数字阈值 threshold, e.g. 15".into()))
}

/// Compare a price watch's observation with the previous one (the cursor). `observed` is structured:
/// `{price, currency, item, seen?, title?, url?, read}` — read by the price's element or extracted by the model, never
/// found in the page's text here. Only a price newly below the threshold (or lower again than the last alert) fires;
/// the first observation only saves the baseline.
pub fn evaluate_price(
    params: &Value,
    observed: &Value,
    cursor: Option<&Value>,
) -> Result<(Vec<Value>, Value), WatchError> {
    let cursor = cursor.filter(|c| !c.is_null());
    let limit = threshold(params)?;
    let price = observed
        .get("price")
        .and_then(Value::as_f64)
        .filter(|p| p.is_finite() && *p > 0.0)
        .ok_or_else(|| WatchError("没有读到价格 no price was read".into()))?;
    let currency = get_str(observed, "currency");
    let item = {
        let i = get_str(observed, "item");
        if i.is_empty() {
            sarg(params, "item")
        } else {
            i
        }
    };
    let seen = {
        let s = get_str(observed, "seen");
        if s.is_empty() {
            show_price(&currency, price)
        } else {
            s
        }
    };
    let (title, url) = (get_str(observed, "title"), get_str(observed, "url"));
    let empty = json!({});
    let prev = cursor.unwrap_or(&empty);
    let prev_met = prev.get("met").is_some_and(truthy);
    let met = price < limit;
    let mut cur = json!({"low": price, "met": met, "currency": take_chars(&currency, 8),
        "item": take_chars(&item, 120), "seen": take_chars(&seen, 200), "read": get_str(observed, "read"),
        "title": take_chars(&title, 200)});
    let alerted = prev
        .get("alerted")
        .filter(|a| !a.is_null())
        .and_then(Value::as_f64);
    let mut events = Vec::new();
    if met && cursor.is_some() && (!prev_met || alerted.is_some_and(|a| price < a)) {
        cur["alerted"] = json!(price);
        events.push(json!({"url": url, "title": title, "condition": format!("price {} < {}", fmt_g(price), fmt_g(limit)),
            "price": price, "currency": currency, "item": item, "seen": seen}));
    } else if met {
        cur["alerted"] = json!(alerted.unwrap_or(price));
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

/// The price of a price watch read by its element (`price_sel`, optional `price_attr`: a known site's rule or the
/// model's selector, decided when the watch was made) through the browser service's `read_elements`: the first match
/// whose value is a price (`parse_amount`).
async fn read_price_element(st: &AppState, tid: &str, params: &Value) -> Result<Value, WatchError> {
    let sel = sarg(params, "price_sel").trim().to_string();
    let attr = sarg(params, "price_attr").trim().to_string();
    let mut rule = json!({"name": "price", "sel": sel, "max": 5});
    if !attr.is_empty() {
        rule["attr"] = json!(attr);
    }
    let res = actions::broker(
        st,
        "POST",
        "/agent/read_elements",
        Some(&json!({"task_id": tid, "read": [rule]})),
        60.0,
    )
    .await
    .map_err(|e: ActionError| werr(e))?;
    let values: Vec<String> = res["elements"]["price"]
        .as_array()
        .map(|a| a.iter().map(py_str).collect())
        .unwrap_or_default();
    let Some((raw, (price, currency))) = values
        .iter()
        .find_map(|v| parse_amount(v).map(|p| (v.clone(), p)))
    else {
        return Err(WatchError(format!(
            "价格元素没有读到价格 (the price element `{sel}` shows no price: {})",
            if values.is_empty() {
                "no such element on the page".to_string()
            } else {
                take_chars(&values.join(" | "), 120).to_string()
            }
        )));
    };
    let (seen, _) = guard::redact_secrets(&WS.replace_all(&raw, " "));
    Ok(
        json!({"price": price, "currency": currency, "seen": seen.trim(), "read": "element",
              "title": sarg(&res, "title"), "url": sarg(&res, "url"), "item": sarg(params, "item")}),
    )
}

/// Watch a web page through the browser broker (`web.page`). A price watch (`mode=price_below`) reads its price by
/// the element rule it was made with (`read=element`); one made to be read by the model (`read=model`) answers
/// `{"extract": {text, title, url}}` with the page's text — the runtime has the model extract `{price, currency, item}`
/// into JSON and calls again with that as `observed`, which is compared here without loading the page again.
pub async fn poll_web(
    st: &AppState,
    params: &Value,
    cursor: Option<&Value>,
    observed: Option<&Value>,
) -> Result<Value, WatchError> {
    let price_mode = sarg(params, "mode")
        .trim()
        .eq_ignore_ascii_case("price_below");
    if let Some(obs) = observed.filter(|o| o.is_object()) {
        if !price_mode {
            return Err(WatchError(
                "observed 只用于价格监控 (observed is for price watches)".into(),
            ));
        }
        let (events, cur) = evaluate_price(params, obs, cursor)?;
        return Ok(result(events, cur));
    }
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
    let snap_url = sarg(&snap, "url");
    let page_url = if snap_url.is_empty() {
        url.clone()
    } else {
        snap_url
    };
    let title = sarg(&snap, "title");
    if price_mode {
        if !sarg(params, "price_sel").trim().is_empty() && sarg(params, "read") != "model" {
            let mut obs = read_price_element(st, &tid, params).await?;
            if get_str(&obs, "title").is_empty() {
                obs["title"] = json!(title);
            }
            if get_str(&obs, "url").is_empty() {
                obs["url"] = json!(page_url);
            }
            let (events, cur) = evaluate_price(params, &obs, cursor)?;
            return Ok(result(events, cur));
        }
        let (text, _) = guard::redact_secrets(&page_text(&sarg(&snap, "snapshot")));
        return Ok(
            json!({"extract": {"text": take_chars(&text, 20000), "title": title, "url": page_url},
                         "events": [], "cursor": cursor.cloned().unwrap_or(Value::Null)}),
        );
    }
    let (text, _) = guard::redact_secrets(&page_text(&sarg(&snap, "snapshot")));
    let (events, cur) = evaluate(params, &text, &title, &page_url, cursor)?;
    Ok(result(events, cur))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_price_elements_value_is_checked_not_searched() {
        assert_eq!(parse_amount("S$25.00"), Some((25.0, "S$".into())));
        assert_eq!(parse_amount(" $1,299.00 "), Some((1299.0, "$".into())));
        assert_eq!(parse_amount("25,00 €"), Some((25.0, "€".into())));
        assert_eq!(parse_amount("1.299,50 €"), Some((1299.5, "€".into())));
        assert_eq!(parse_amount("SGD 31.43"), Some((31.43, "SGD".into())));
        assert_eq!(parse_amount("1299"), Some((1299.0, String::new())));
        assert_eq!(parse_amount("¥12,800"), Some((12800.0, "¥".into())));
        // not one price: words around it, two prices, nothing
        for bad in [
            "From S$25.00",
            "S$25.00 S$19.99",
            "Price: 25",
            "out of stock",
            "",
            "S$0.00",
            "25 . 00",
        ] {
            assert_eq!(parse_amount(bad), None, "{bad}");
        }
        assert_eq!(show_price("S$", 25.0), "S$25.00");
        assert_eq!(show_price("SGD", 31.43), "SGD 31.43");
        assert_eq!(show_price("", 9.5), "9.50");
    }

    #[test]
    fn a_price_watch_compares_the_structured_price() {
        let p = json!({"mode": "price_below", "threshold": "20", "item": "Aurora case"});
        let obs = |price: f64| json!({"price": price, "currency": "S$", "read": "model", "url": "u", "title": "t"});
        // the first observation is the baseline, even below the threshold
        let (ev, c) = evaluate_price(&p, &obs(18.0), None).unwrap();
        assert!(ev.is_empty() && c["met"] == true);
        assert_eq!(c["seen"], "S$18.00");
        assert_eq!(c["item"], "Aurora case");
        let (ev, c) = evaluate_price(&p, &obs(25.0), Some(&c)).unwrap();
        assert!(ev.is_empty() && c["met"] == false);
        let (ev, c) = evaluate_price(&p, &obs(18.0), Some(&c)).unwrap();
        assert_eq!(ev.len(), 1);
        assert_eq!(ev[0]["condition"], "price 18 < 20");
        assert_eq!(ev[0]["price"], 18.0);
        assert_eq!(ev[0]["seen"], "S$18.00");
        // the same price again: nothing; lower again: a new alert
        let (ev, c) = evaluate_price(&p, &obs(18.0), Some(&c)).unwrap();
        assert!(ev.is_empty());
        let (ev, _) = evaluate_price(&p, &obs(16.0), Some(&c)).unwrap();
        assert_eq!(ev[0]["condition"], "price 16 < 20");
        // an element's own text is what the card shows
        let el = json!({"price": 25.0, "currency": "S$", "seen": "S$25.00", "read": "element"});
        let (_, c) = evaluate_price(&p, &el, None).unwrap();
        assert_eq!(
            (c["seen"].as_str(), c["read"].as_str()),
            (Some("S$25.00"), Some("element"))
        );
        // no price, a bad threshold: errors, never a guess
        assert!(evaluate_price(&p, &json!({"price": null}), None).is_err());
        assert!(evaluate_price(&json!({"threshold": "cheap"}), &obs(1.0), None).is_err());
        assert!(evaluate_price(&json!({"threshold": "S$20"}), &obs(1.0), None).is_ok());
    }

    #[test]
    fn web_watch_evaluate() {
        assert!(!page_text("[e3] link \"Buy\" → https://x.test/a\n# Title").contains("[e3]"));
        // a price watch is never evaluated over the page's text
        assert!(evaluate(
            &json!({"mode": "price_below", "threshold": "20"}),
            "S$18",
            "t",
            "u",
            None
        )
        .is_err());
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
