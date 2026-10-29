//! Loop guards, page hints and answer checks: the module-level helpers of `app/runtime/agent.py`.
//!
//! All of them are pure functions over the transcript (a list of chat messages as JSON) and the task's event log,
//! so they are unit-tested directly (`tests/test_027.py`, `tests/test_units.py`).

use std::collections::HashSet;
use std::sync::LazyLock;

use fancy_regex::Regex as FRegex;
use indexmap::IndexMap;
use omuse_common::guard::urlparse;
use omuse_common::pyjson;
use omuse_common::util::{get_str, get_truthy, last_chars, take_chars, truncate, truthy};
use regex::Regex;
use serde_json::{json, Map, Value};

use super::util::{content_of, dumps_sorted, pystr_or_empty, sget, stored_call, tool_calls};
use crate::llm::ToolCall;

/// Loop guard: small local models sometimes repeat the exact same call until the step budget is gone
/// (e.g. opening one RSS feed 24 times in a row). Identical calls are refused after REPEAT_STREAK in a row,
/// and read-type calls after REPEAT_TOTAL anywhere in the task. Tools where repeating is normal are exempt from the streak rule.
pub const REPEAT_STREAK: usize = 2;
pub const REPEAT_TOTAL: usize = 3;
/// update_plan calls in a row (with nothing else between) before the next one is refused
pub const PLAN_STREAK: usize = 3;
pub const REPEAT_STREAK_OK: [&str; 7] = [
    "browser_scroll",
    "browser_press",
    "browser_wait",
    "browser_click",
    "browser_back",
    "update_plan",
    "phone_call_status", // polling a live call is expected to repeat
];
static REPEAT_READ: LazyLock<Regex> =
    LazyLock::new(|| rx(r"navigate|search|read|list|_get|fetch|query"));

/// Dead-end sources: a web host that failed (error, bot wall, refused repeat) HOST_FAIL_MAX times in one task is
/// blocked for the rest of the task, so the agent has to switch source instead of hammering the same link.
/// When every call in a turn is refused, the agent is stuck: 1st time it gets a firm nudge, 2nd time a re-plan that is
/// told which sources to avoid, 3rd time (or after MAX_REPLANS re-plans) it stops and answers with what it has.
pub const HOST_FAIL_MAX: i64 = 3;
pub const MAX_REPLANS: i64 = 4;
/// chars of context: compress old tool results in one batch
pub const COMPRESS_HIGH: usize = 80000;
pub const COMPRESS_LOW: usize = 40000;
/// share of the time budget (Settings → max_minutes) after which the agent is told to wrap up
pub const TIME_NUDGE: f64 = 0.7;

pub const COMPRESSED_MARK: &str = "older result compressed";

// step budget: keep the last steps for producing / sending what the user asked for
pub const BUDGET_RESERVE: i64 = 5;
pub const RESEARCH_NUDGE_PAGES: i64 = 10;
pub const BUDGET_MARK: &str = "step budget";
pub const BUDGET_MARK_PAGES: &str = "research check";
pub const TIME_MARK: &str = "time budget";
pub const WEB_MARK: &str = "web budget";
/// web tool calls in one task before the agent is told to wrap up
pub const WEB_NUDGE: i64 = 18;
/// the 2026-10-02 itinerary run re-made the same Excel file 9 times (≈5 minutes of generation)
pub const REMAKE_MAX: i64 = 4;

pub const TAKEOVER_DONE_MARK: &str = "用户已完成接管";
pub const TAKEOVER_DONE_EN: &str = "Takeover finished";

fn rx(p: &str) -> Regex {
    Regex::new(p).unwrap()
}

fn frx(p: &str) -> FRegex {
    FRegex::new(p).unwrap()
}

/// Order- and case-insensitive key for a web search, so a re-worded copy of an earlier search is recognised.
pub fn search_key(q: &Value) -> String {
    static W: LazyLock<Regex> = LazyLock::new(|| rx(r"[\w一-鿿]+"));
    let s = pystr_or_empty(q).to_lowercase();
    let words: std::collections::BTreeSet<&str> = W.find_iter(&s).map(|m| m.as_str()).collect();
    words.into_iter().collect::<Vec<_>>().join(" ")
}

/// `name + " " + json.dumps(args or {}, ensure_ascii=False, sort_keys=True)`; JSON text arguments are parsed first.
pub fn call_sig(name: &str, args: &Value) -> String {
    let mut a = args.clone();
    if let Value::String(s) = &a {
        if let Ok(v) = serde_json::from_str::<Value>(if s.is_empty() { "{}" } else { s }) {
            a = v;
        }
    }
    let a = if truthy(&a) { a } else { json!({}) };
    format!("{} {}", name, dumps_sorted(&a))
}

/// `(name, args)` of every tool call in the transcript before `call_id` (all of them when the id is not found).
fn calls_before(transcript: &[Value], call_id: &str) -> Vec<(String, Value)> {
    let mut prev = Vec::new();
    for m in transcript {
        for tc in tool_calls(m) {
            if get_str(tc, "id") == call_id {
                return prev;
            }
            prev.push(stored_call(tc));
        }
    }
    prev
}

/// The page a browser call ended on (the `"url"` its result reports), by call id.
fn result_urls(transcript: &[Value]) -> std::collections::HashMap<String, String> {
    static URL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#""url":\s*"([^"]+)""#).unwrap());
    transcript
        .iter()
        .filter(|m| get_str(m, "role") == "tool")
        .filter_map(|m| {
            let c = content_of(m);
            URL.captures(&c)
                .map(|u| (get_str(m, "tool_call_id"), u[1].to_string()))
        })
        .collect()
}

/// browser_type into a field the agent already filled on this page, with different text, is almost always a mix-up
/// (e.g. typing the first name into the booking-number box and wiping it). Ask it to use the right field instead.
/// Refs are per page: a field typed on another page (2026-10-29: "Japan" in the quote page's e1, then the application
/// page's e1 is the full name — reached by a plain link click) is a different field.
pub fn retype_guard(transcript: &[Value], call: &ToolCall) -> Option<String> {
    if call.name != "browser_type" {
        return None;
    }
    let args = &call.args;
    let (r, text) = (sget(args, "ref"), sget(args, "text"));
    if r.is_empty() || get_truthy(args, "replace") {
        return None;
    }
    let urls = result_urls(transcript);
    let mut ids: Vec<String> = Vec::new();
    'outer: for m in transcript {
        for tc in tool_calls(m) {
            if get_str(tc, "id") == call.id {
                break 'outer;
            }
            ids.push(get_str(tc, "id"));
        }
    }
    // the page the agent is on now: the last browser call's
    let here = ids.iter().rev().find_map(|i| urls.get(i)).cloned();
    let prev = calls_before(transcript, &call.id);
    for ((nm, a), id) in prev.iter().zip(ids.iter()).rev() {
        if nm == "browser_type" && sget(a, "ref") == r {
            if let (Some(then), Some(now)) = (urls.get(id), here.as_ref()) {
                if then != now {
                    return None;
                }
            }
        }
        if nm == "browser_navigate" || nm == "browser_back" {
            return None;
        }
        if nm == "browser_click" && get_truthy(a, "submit") {
            return None;
        }
        if nm == "browser_type" && sget(a, "ref") == r {
            if get_truthy(a, "submit") {
                // a search box: typing a new query after a submitted one is a new search
                return None;
            }
            let old = sget(a, "text");
            if !old.is_empty() && old != text {
                let (o, t) = (take_chars(&old, 40), take_chars(&text, 40));
                return Some(format!(
                    "ERROR: 没有执行 — 你刚才已经在 {r} 里输入了「{o}」，再输入「{t}」会把它覆盖掉。\
多半是填错了格子：先看快照里每个输入框的名字，把「{t}」填到对应的那个 ref。\
 Not done: you already typed \"{o}\" into {r}; typing \"{t}\" there would replace it. \
You probably meant the next field — check each textbox's name in the snapshot and use its ref. \
If you really want to replace it, call browser_type again with replace=true."
                ));
            }
            return None;
        }
    }
    None
}

/// A 'final answer' that is really a cut-off fragment (2026-10-05 S1-04: the whole answer was 'Sheng Siong 搜索"gard'
/// after 27 steps of price lookups). Such text must not end the task.
pub fn cut_off_final(text: &str, finish_reason: Option<&str>, open_steps: usize) -> bool {
    let t = text.trim();
    if t.is_empty() {
        return false;
    }
    if finish_reason == Some("length") {
        return true;
    }
    if t.chars().count() >= 80 {
        return false;
    }
    let count = |c: char| t.chars().filter(|x| *x == c).count();
    let unbalanced = count('"') % 2 == 1
        || count('「') > count('」')
        || count('“') > count('”')
        || count('(') > count(')');
    let last = t.chars().last().unwrap_or(' ');
    let ends = "。.!！?？)）」】”\"*|`~>:：…".contains(last) || t.ends_with("...");
    unbalanced || (!ends && open_steps > 0)
}

/// Concrete different mail queries for a model that keeps repeating one (it copies a ready query more readily).
pub fn search_alternatives(q: &str) -> Vec<String> {
    static INBOX: LazyLock<Regex> = LazyLock::new(|| rx(r"\bin:inbox\b"));
    static FROM: LazyLock<Regex> = LazyLock::new(|| rx(r"(?:from|subject):([\w.\-]+)"));
    let q = q.trim();
    let mut out: Vec<String> = Vec::new();
    if INBOX.is_match(q) {
        out.push(INBOX.replace_all(q, "in:anywhere").into_owned()); // receipts are often archived, not in the inbox
    }
    let mut words: Vec<String> = Vec::new();
    for c in FROM.captures_iter(q) {
        let w = c[1].to_string();
        if !words.contains(&w) {
            words.push(w);
        }
    }
    words.truncate(3);
    if !words.is_empty() {
        out.push(format!(
            "in:anywhere ({}) (receipt OR invoice OR order OR 收据 OR 发票 OR 订单)",
            words.join(" OR ")
        ));
    }
    let mut seen = Vec::new();
    for x in out {
        if !x.is_empty() && x != q && !seen.contains(&x) {
            seen.push(x);
        }
    }
    seen.truncate(2);
    seen
}

/// Refusal text if `call` repeats an identical earlier call too often, else None.
pub fn repeat_guard(transcript: &[Value], call: &ToolCall) -> Option<String> {
    let name = call.name.as_str();
    if name == "update_plan" {
        // 2026-10-04 TC17: 58 update_plan calls in a row (no real work between them) used up all 80 steps
        let names: Vec<String> = transcript
            .iter()
            .flat_map(tool_calls)
            .filter(|tc| get_str(tc, "id") != call.id)
            .map(|tc| stored_call(tc).0)
            .collect();
        if names.len() >= PLAN_STREAK
            && names[names.len() - PLAN_STREAK..]
                .iter()
                .all(|n| n == "update_plan")
        {
            return Some(
                "ERROR: 计划已经更新过好几次了，这次没有执行。不要再调用 update_plan：现在就执行下一步（调用真正干活的工具），\
或者信息已经够了就直接写最终回答。\
 The plan was already updated several times in a row; this call was not run. Stop calling update_plan: \
do the next step with a real tool now, or write the final answer if you have enough."
                    .to_string(),
            );
        }
        return None;
    }
    let sigs: Vec<String> = calls_before(transcript, &call.id)
        .iter()
        .map(|(n, a)| call_sig(n, a))
        .collect();
    let me = call_sig(name, &call.args);
    let streak = sigs.iter().rev().take_while(|s| **s == me).count();
    let total = sigs.iter().filter(|s| **s == me).count();
    if total > 0 && total < REPEAT_TOTAL + 2 && last_result_compressed(transcript, call, &me) {
        // 2026-10-04 M2-06: the earlier result was compressed out of the context, so the model cannot see it any more —
        // blocking the re-read left it asking for the same two searches until it gave up
        return None;
    }
    let searchy = name == "browser_type" && get_truthy(&call.args, "submit"); // a site search box (V3-03)
    if (name == "browser_click" || name == "browser_click_at") && total >= 4 {
        // 2026-10-04 V3-10: the same "Add to cart" ref was clicked 22 times while the cart stayed at 0 items
        return Some(format!(
            "ERROR: 同一个元素已经点了 {total} 次，没有效果，这次没有执行。它可能不是你要的按钮，或者点了也不会变化。\
先 browser_find(\"<按钮上的字>\") 或 browser_look 找到真正的按钮（注意是否要先选规格/配送选项），\
或换一种方式；还是不行就如实告诉用户。\
 This element was already clicked {total}x with no effect; not clicked again. It is probably not the right \
button or needs another step first: browser_find the real button (check options first) or report honestly."
        ));
    }
    if (!REPEAT_STREAK_OK.contains(&name) && streak >= REPEAT_STREAK)
        || ((REPEAT_READ.is_match(name) || searchy) && total >= REPEAT_TOTAL)
    {
        let n = total + 1;
        if name == "gmail_search" {
            // 2026-10-04 M2-06 / M3-08: the model re-ran the same mail search 5-11 times instead of moving on
            let alts = search_alternatives(&sget(&call.args, "query"));
            let alt_txt = if alts.is_empty() {
                String::new()
            } else {
                format!(
                    "\n可以直接试这些不同的查询 Different queries to try: {}",
                    alts.join(" | ")
                )
            };
            return Some(format!(
                "ERROR: 这个搜索已经做过 {total} 次了，结果就在上文，这次没有执行。下一步：从上文结果里挑出相关邮件的 id，\
统计金额就调用 gmail_read_amounts(message_ids=[…])，要看全文就调用 gmail_get_message；\
如果上文结果里没有需要的邮件，就换一个不同的查询（例如只用 from:发件人域名）。\
 This exact search already ran {total}x and its result is above; it was not run again. Next step: take \
the ids of the relevant emails from that result and call gmail_read_amounts(message_ids=[…]) for amounts, \
or gmail_get_message for full text. If the result has nothing useful, run a DIFFERENT query \
(e.g. only from:<sender domain>).{alt_txt}"
            ));
        }
        return Some(format!(
            "ERROR: 重复调用已拦截 — 这是第 {n} 次用完全相同的参数调用 {name}，结果不会改变，本次没有执行。\
请直接使用前面已经拿到的结果继续下一步；如果这个来源读不到需要的内容，就跳过它，换别的来源，或者用已有的内容完成任务。\
 Repeated identical call blocked ({name}, {n}x): the result will not change. Use what you already have, \
skip this source, and move on to the next step."
        ));
    }
    None
}

fn last_result_compressed(transcript: &[Value], call: &ToolCall, me: &str) -> bool {
    let mut last_id: Option<String> = None;
    for m in transcript {
        for tc in tool_calls(m) {
            let (n, a) = stored_call(tc);
            if get_str(tc, "id") != call.id && call_sig(&n, &a) == me {
                last_id = Some(get_str(tc, "id"));
            }
        }
    }
    let Some(last_id) = last_id else {
        return false;
    };
    transcript.iter().any(|m| {
        get_str(m, "role") == "tool"
            && get_str(m, "tool_call_id") == last_id
            && content_of(m).contains(COMPRESSED_MARK)
    })
}

/// Mail ids must come from a tool result: 2026-10-04 M2-10 the model "continued" a list of ids by arithmetic
/// (1878038389737846821, …86788965087, …83840083353 …) and burned 20 minutes on "message not found".
const ID_ARGS: [(&str, &str); 3] = [
    ("gmail_get_message", "message_id"),
    ("gmail_get_thread", "thread_id"),
    ("gmail_save_attachment", "message_id"),
];

pub fn invented_id_guard(
    transcript: &[Value],
    call: &ToolCall,
    known: Option<&HashSet<String>>,
) -> Option<String> {
    static SPLIT: LazyLock<Regex> = LazyLock::new(|| rx(r"[,\s]+"));
    let key = ID_ARGS
        .iter()
        .find(|(n, _)| *n == call.name)
        .map(|(_, k)| *k);
    let mut ids: Vec<String> = Vec::new();
    if let Some(key) = key {
        ids.push(sget(&call.args, key));
    } else if call.name == "gmail_read_amounts" {
        match call.args.get("message_ids") {
            Some(Value::Array(v)) => ids.extend(v.iter().map(omuse_common::util::py_str)),
            Some(v) if truthy(v) => {
                ids.extend(
                    SPLIT
                        .split(&omuse_common::util::py_str(v))
                        .map(str::to_string),
                );
            }
            _ => {}
        }
    }
    let ids: Vec<String> = ids
        .into_iter()
        .filter(|i| !i.is_empty())
        .map(|i| i.trim().to_string())
        .filter(|i| !i.is_empty())
        .collect();
    if ids.is_empty() {
        return None;
    }
    let seen: String = transcript
        .iter()
        .filter(|m| matches!(get_str(m, "role").as_str(), "tool" | "user" | "system"))
        .map(content_of)
        .collect::<Vec<_>>()
        .join("\n");
    let empty = HashSet::new();
    let known = known.unwrap_or(&empty);
    let bad: Vec<&String> = ids
        .iter()
        .filter(|i| {
            let tail = i.split_once(':').map(|(_, t)| t).unwrap_or(i);
            !known.contains(i.as_str()) && !known.contains(tail) && !seen.contains(tail)
        })
        .collect();
    if bad.is_empty() {
        return None;
    }
    if bad.len() < ids.len() {
        // drop the made-up ones, keep the real ones
        return None;
    }
    let shown = bad
        .iter()
        .take(3)
        .map(|s| s.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    Some(format!(
        "ERROR: 这些 id 没有出现在任何搜索结果里（{shown}…），不要自己编 id 或按规律推算 id。\
先用 gmail_search 找到邮件，再原样复制它返回的 id（包括 g2: 这样的前缀）。\
 These ids do not appear in any search result ({shown}…). Never invent or extrapolate ids: \
run gmail_search and copy the ids it returns exactly (including prefixes like g2:)."
    ))
}

/// Reading the same mail / page / file again while its full result is still in the context only makes the context (and
/// every following model call) bigger: answer with a pointer instead. A compressed or failed earlier result is re-read.
static REREAD_TOOLS: LazyLock<Regex> = LazyLock::new(|| {
    rx(
        r"^(gmail_get_message|gmail_get_thread|notion_get_page|notion_read|files_read|web_read|browser_read|web_search|gmail_search|gmail_find_receipts|gmail_read_amounts)$",
    )
});

pub fn reread_guard(transcript: &[Value], call: &ToolCall) -> Option<String> {
    let name = call.name.as_str();
    if !REREAD_TOOLS.is_match(name) {
        return None;
    }
    let me = call_sig(name, &call.args);
    let mut earlier: HashSet<String> = HashSet::new();
    for m in transcript {
        for tc in tool_calls(m) {
            if get_str(tc, "id") == call.id {
                continue;
            }
            let (n, a) = stored_call(tc);
            if call_sig(&n, &a) == me {
                earlier.insert(get_str(tc, "id"));
            }
        }
    }
    if earlier.is_empty() {
        return None;
    }
    for m in transcript {
        if get_str(m, "role") == "tool" && earlier.contains(&get_str(m, "tool_call_id")) {
            let c = content_of(m);
            let head = take_chars(&c, 10);
            if !c.is_empty()
                && !c.starts_with("ERROR")
                && !c.contains(COMPRESSED_MARK)
                && !head.contains("已跳过")
                && !head.contains("已读取过")
            {
                let id = get_str(m, "tool_call_id");
                return Some(format!(
                    "已读取过 — 这个内容在上文调用 {id} 的结果里，完整保留着，没有再读一次。请直接使用它；\
如果需要的东西都已经有了，就继续下一步或写最终回答。\
 Already read: the full result is above (call {id}); use it instead of reading again. \
If you have what you need, move on or write the final answer.{}",
                    next_step_hint(name, call, &c)
                ));
            }
        }
    }
    None
}

/// A concrete, copyable next call for a model that keeps repeating a search (2026-10-04 TC27, M3-21).
fn next_step_hint(name: &str, call: &ToolCall, earlier: &str) -> String {
    static URL: LazyLock<Regex> = LazyLock::new(|| rx(r#"https?://[^\s"'<>]+"#));
    if name == "web_search" {
        let mut urls: Vec<String> = Vec::new();
        for m in URL.find_iter(earlier) {
            let u = m
                .as_str()
                .trim_end_matches(['.', ',', ')', '"', '\''])
                .to_string();
            if !urls.contains(&u) {
                urls.push(u);
            }
        }
        urls.truncate(3);
        if !urls.is_empty() {
            return format!(
                "\n下一步 Next: 打开这些结果页读取详情 open the result pages: browser_read(urls={})\
，或者换一个不同的搜索词 or search with DIFFERENT words.",
                pyjson::dumps_ascii(&json!(urls))
            );
        }
    }
    if name == "gmail_search" {
        let alts = search_alternatives(&sget(&call.args, "query"));
        return if alts.is_empty() {
            "\n下一步 Next: 用结果里的 id 调用 gmail_read_amounts / gmail_get_message (ids from the result above).".to_string()
        } else {
            format!("\n下一步 Next: 用结果里的 id 调用 gmail_read_amounts / gmail_get_message；或换查询 or try: {}", alts.join(" | "))
        };
    }
    String::new()
}

/// `urlparse(url).hostname` lowercased without a leading `www.` ("" when unparsable).
pub fn host_of(url: &Value) -> String {
    let s = pystr_or_empty(url);
    let h = match urlparse(&s) {
        Ok(p) => p.hostname().unwrap_or_default().to_lowercase(),
        Err(_) => return String::new(),
    };
    h.strip_prefix("www.").map(str::to_string).unwrap_or(h)
}

/// Failures per web host in this task, from the event log (survives restarts and context compression).
pub fn source_failures(events: &[Value]) -> IndexMap<String, i64> {
    let mut calls: IndexMap<String, Value> = IndexMap::new();
    let mut fails: IndexMap<String, i64> = IndexMap::new();
    for e in events {
        let d = e
            .get("data")
            .filter(|d| truthy(d))
            .cloned()
            .unwrap_or_else(|| json!({}));
        let ty = get_str(e, "type");
        if ty == "tool_call" {
            let a = d
                .get("args")
                .filter(|a| a.is_object())
                .cloned()
                .unwrap_or_else(|| json!({}));
            calls.insert(sget(&d, "call_id"), a);
        } else if ty == "tool_result"
            && d.get("ok") == Some(&Value::Bool(false))
            && !get_truthy(&d, "skipped")
        {
            let args = calls
                .get(&sget(&d, "call_id"))
                .cloned()
                .unwrap_or_else(|| json!({}));
            let host = host_of(args.get("url").unwrap_or(&Value::Null));
            if !host.is_empty() {
                *fails.entry(host).or_insert(0) += 1;
            }
        }
    }
    fails
}

pub fn dead_ends_text(fails: &IndexMap<String, i64>, lang: &str) -> String {
    let mut bad: Vec<(&String, &i64)> = fails.iter().filter(|(_, n)| **n >= 2).collect();
    bad.sort_by_key(|(_, n)| -**n); // stable: ties keep insertion order, like Python's sorted
    bad.truncate(8);
    if bad.is_empty() {
        return String::new();
    }
    let lst = bad
        .iter()
        .map(|(h, n)| format!("{} ({}x)", h, n))
        .collect::<Vec<_>>()
        .join(", ");
    if lang == "en" {
        return format!("Dead ends in this task (failed or blocked repeatedly; do NOT use them again, pick a different source): {lst}");
    }
    format!(
        "本任务里已经反复失败/被拦截的来源（不要再用，换别的网站或方法）: {lst}\
\nDead ends in this task (do NOT use them again, pick a different source): {lst}"
    )
}

/// Refusal text if the call goes to a web host that already failed HOST_FAIL_MAX times in this task.
/// `events` is read only when the call has a URL.
pub fn host_guard(events: impl FnOnce() -> Vec<Value>, call: &ToolCall) -> Option<String> {
    let host = if call.args.is_object() {
        host_of(call.args.get("url").unwrap_or(&Value::Null))
    } else {
        String::new()
    };
    if host.is_empty() {
        return None;
    }
    let n = source_failures(&events()).get(&host).copied().unwrap_or(0);
    if n < HOST_FAIL_MAX {
        return None;
    }
    Some(format!(
        "ERROR: 已拦截 — {host} 在本任务里已经失败或被拦截 {n} 次，本次没有执行，之后也不会再执行。\
不要再访问这个网站：换一个完全不同的网站或数据来源（例如同类信息的其他网站、官方网站、新闻或资料站），\
或者用已经拿到的内容完成任务，并告诉用户哪些数据拿不到。\
 Blocked: {host} already failed or was refused {n} times in this task, so it will not be called again. \
Switch to a completely different website or data source, or finish with what you already have and tell the user \
what could not be fetched."
    ))
}

/// `str(args.get("output") or args.get("title") or "").strip().lower()`.
pub fn out_key(args: &Value) -> String {
    let v = args
        .get("output")
        .filter(|v| truthy(v))
        .or_else(|| args.get("title").filter(|v| truthy(v)));
    v.map(omuse_common::util::py_str)
        .unwrap_or_default()
        .trim()
        .to_lowercase()
}

static ERRORISH: LazyLock<Regex> = LazyLock::new(|| {
    rx(
        r"(?i)(system[-_]?error|/error|errorpage|error\.html|session[-_]?(expired|timeout)|timeout|expired|/sorry|invalid[-_]?(request|access)|access[-_]?denied)",
    )
});

static PRODUCT_PAGE: LazyLock<Regex> = LazyLock::new(|| {
    rx(
        r"(?i)amazon\.[a-z.]+/(?:[^/]+/)?(?:dp|gp/product)/|lazada\.[a-z.]+/products/|shopee\.[a-z.]+/.+-i\.\d+\.\d+|fairprice\.com\.sg/product/|courts\.com\.sg/.+\.html|challenger\.sg/products/",
    )
});

static PARKED: LazyLock<Regex> = LazyLock::new(|| {
    rx(
        r"(?i)(this|the) domain (name )?(\S+ )?(is|may be) for sale|\S+\.\w+ is for sale|domain is for sale|buy this domain|is parked free|parked (domain|by|courtesy)|sedoparking|domain parking|godaddy\.com/domain|域名.{0,6}(出售|转让)",
    )
});

/// A guessed address that is a parked / for-sale domain (2026-10-04 V3-07: "scoot.com" instead of flyscoot.com).
pub fn parked_domain_hint(_url: &str, title: &str, snapshot: &str) -> String {
    let probe = format!("{}\n{}", title, take_chars(snapshot, 3000));
    if !PARKED.is_match(&probe) {
        return String::new();
    }
    "（提示）这是一个停放/出售中的域名，不是这家公司的官网——网址可能猜错了。用 web_search 搜索\
「<公司名> official site」找到正确网址再继续。 (Hint) This is a parked / for-sale domain, not the company's site — \
the address was probably guessed wrong. web_search \"<company> official site\" for the right one."
        .to_string()
}

/// On a shop's product page the next step is the cart button, not another search (2026-10-04 V3-04: the agent bounced
/// between the search page and the product page eight times and never pressed Add to Cart).
pub fn product_page_hint(url: &str, task_goal: &str) -> String {
    static CART: LazyLock<Regex> =
        LazyLock::new(|| rx(r"(?i)购物车|加购|cart|basket|买|buy|下单|order"));
    if !PRODUCT_PAGE.is_match(url) || !CART.is_match(task_goal) {
        return String::new();
    }
    "（提示）这是商品详情页。下一步：有颜色/尺寸选项就先选，然后 browser_find(\"Add to Cart\") 并点击它的 ref；\
不要回到搜索页重新打开。 (Hint) This is the product page: pick options if any, then browser_find(\"Add to Cart\") \
and click its ref — don't go back to the search results."
        .to_string()
}

/// A deep link (from an email, a bookmark) that lands on the site's error page usually means "no session yet",
/// not "the site is down": say so, so the agent enters through the home page instead of retrying for hours.
pub fn deep_link_hint(asked: &str, landed: &str, title: &str) -> String {
    static ERR_TITLE: LazyLock<Regex> = LazyLock::new(|| rx(r"(?i)\berror\b|エラー|错误"));
    let (Ok(a), Ok(b)) = (urlparse(asked), urlparse(landed)) else {
        return String::new();
    };
    if a.netloc.is_empty()
        || b.netloc.is_empty()
        || asked.trim_end_matches('/') == landed.trim_end_matches('/')
    {
        return String::new();
    }
    if !(ERRORISH.is_match(&format!("{}?{}", b.path, b.query)) || ERR_TITLE.is_match(title)) {
        return String::new();
    }
    if ERRORISH.is_match(&a.path) {
        return String::new();
    }
    "[DEEP LINK → ERROR PAGE] You opened a deep link and were sent to an error page. On airline/bank/booking \
sites this almost always means the link needs a session that a fresh browser doesn't have — it does NOT mean \
the site is down. Don't retry the same link later. Open the company's main home page (search for it if \
needed — booking systems often live on a different subdomain) and use its own menu (My Booking / Manage booking / Sign in) to get there; that page usually asks for the booking \
number AND a passenger's first/last name."
        .to_string()
}

static REFUSED: LazyLock<Regex> = LazyLock::new(|| {
    rx(
        r"(?i)heavy traffic|cannot be accepted at this time|大変混み合って|混み合っております|document registration process could not be completed|please try again later|request (?:was |has been )?(?:rejected|refused|blocked)|access (?:has been )?denied|unusual (?:traffic|activity)|temporarily unavailable|服务繁忙|系统繁忙",
    )
});

/// A transactional page (booking lookup, check-in, account) answering "busy / try later / system error" right after
/// the agent submitted a form or opened it. When the site's normal pages load fine, this is usually the site refusing
/// this automated browser, not an outage — retrying on a timer only repeats it. Say so plainly.
pub fn refusal_hint(url: &str, title: &str, text: &str, submitted: bool) -> String {
    static SYSERR: LazyLock<Regex> = LazyLock::new(|| rx(r"(?i)system[-_]?error"));
    if !REFUSED.is_match(&format!("{}\n{}", title, take_chars(text, 3000))) && !SYSERR.is_match(url)
    {
        return String::new();
    }
    format!(
        "[SITE REFUSED THE REQUEST] The site answered with a busy / try-later / system-error page{}. If its ordinary pages (home page, info pages) \
load fine, this is most likely the site refusing this automated browser — not maintenance. First check your \
inputs once (each field got the right value?) and try ONE more time; if it happens again, stop: tell the \
user plainly that the site refused the automated browser, and that they can do it themselves on their own \
device (give the steps and the details they need), or take over this browser (browser_request_takeover). \
Don't schedule retries, don't call it maintenance unless the site says so for this date, and never try to \
get around the refusal.",
        if submitted { " right after you submitted" } else { "" }
    )
}

fn norm_goal(s: &str) -> String {
    static R: LazyLock<Regex> = LazyLock::new(|| rx(r"[\s\W_]+"));
    take_chars(&R.replace_all(&s.to_lowercase(), ""), 300).to_string()
}

/// An enabled schedule doing the same job (same name, or the same goal text).
pub fn same_schedule(schedules: &[Value], name: &str, goal: &str) -> Option<Value> {
    let ng = norm_goal(goal);
    for s in schedules {
        if !get_truthy(s, "enabled") {
            continue;
        }
        if (!name.is_empty() && get_str(s, "name") == take_chars(name, 80))
            || (ng.chars().count() >= 20 && norm_goal(&sget(s, "goal")) == ng)
        {
            return Some(s.clone());
        }
    }
    None
}

static POINTS_BACK: LazyLock<Regex> = LazyLock::new(|| {
    rx(
        r"(?i)\babove\b|\bpreceding\b|\bearlier (table|summary|message)\b|上面|上方|如上|上述|前面(的|那)",
    )
});

static ASK_RE: LazyLock<Regex> = LazyLock::new(|| {
    rx(
        r"(?i)(请(你)?(告诉|提供|确认|发给|补充)|能(否|不能)告诉我|需要你(提供|告诉|确认)|你(想|要)(选|用)哪|哪一(个|家|天|封)|please (tell|confirm|provide|send|let me know)|could you (tell|confirm|provide|share)|which (one|of)|do you want me to)",
    )
});

/// The answer ends by asking the user for a missing detail or a choice (needs-info), rather than reporting a failure.
/// 2026-10-04 TC34: "find my dentist appointment and call to move it" ended correctly by asking which appointment /
/// the clinic's number, but the task showed FAILED because the searches had come back empty.
pub fn asks_user(final_: &str) -> bool {
    let tail = last_chars(final_, 500);
    ASK_RE.is_match(tail) && (tail.contains('?') || tail.contains('？'))
}

fn is_system_user(m: &Value) -> bool {
    let c = content_of(m);
    c.starts_with("(System)") || c.starts_with("（系统）")
}

/// 2026-10-02 R7-02: the model wrote the full answer (a table of bills) as text next to an update_plan call, then
/// finished with "the summary table above covers …" — the chat only shows the final message, so the table was lost.
/// When the final answer is short and points back, put the last substantial mid-task text in front of it.
pub fn merge_stranded_answer(transcript: &[Value], final_: &str) -> String {
    let f = final_.trim();
    if f.chars().count() > 700 || !POINTS_BACK.is_match(f) {
        return final_.to_string();
    }
    for m in transcript.iter().rev() {
        if get_str(m, "role") == "user" && !is_system_user(m) {
            break; // only look inside this run
        }
        let txt = if get_str(m, "role") == "assistant" && m.get("tool_calls").is_some_and(truthy) {
            content_of(m).trim().to_string()
        } else {
            String::new()
        };
        if txt.chars().count() >= 300 && !f.contains(&txt) && !f.contains(take_chars(&txt, 160)) {
            // V2-10: same answer twice when only the end differed
            return format!("{}\n\n{}", txt, f);
        }
    }
    final_.to_string()
}

static NUM: LazyLock<FRegex> = LazyLock::new(|| {
    frx(r"(?<![\w.\-/:])(\d{1,3}(?:,\d{3})+(?:\.\d+)?|\d+\.\d+|\d{3,})(\s*万)?(?![\w\-/:]|\.\d)")
});
static DATEISH: LazyLock<Regex> = LazyLock::new(|| rx(r"\d{4}[-/.年]\d{1,2}|\d{1,2}[:：]\d{2}"));
pub const CALC_TOOLS: [&str; 2] = ["calculate", "data_query"];

fn num(tok: &str, wan: bool) -> f64 {
    let v: f64 = tok.replace(',', "").parse().unwrap_or(0.0);
    if wan {
        v * 10000.0
    } else {
        v
    }
}

static MONEY_GOAL: LazyLock<Regex> = LazyLock::new(|| {
    rx(
        r"(?i)算|计算|多少钱|总价|合计|总共|一共|花多少|最便宜|划算|到手价|每人|cheapest|total|how much|calculate|per person",
    )
});
static CURRENCY: LazyLock<Regex> =
    LazyLock::new(|| rx(r"S\$|SGD|US\$|USD|JPY|¥|円|日元|新元|美元|€|£|\$\d"));
static SELF_FIX: LazyLock<Regex> = LazyLock::new(|| {
    rx(
        r"(?i)等一下|等等，|不对[：:，,。]|需要修正|我算错|算错了|更正一下|\bwait,|\bcorrection:|\bactually, that'?s wrong",
    )
});

/// The answer changes its mind half-way ("……10.5 超过 10？不对……等一下，这里需要修正") — the user gets two results.
pub fn self_corrects(final_: &str) -> bool {
    SELF_FIX.is_match(final_)
}

fn source_text(transcript: &[Value], goal: &str) -> String {
    let mut s = goal.to_string();
    s.push('\n');
    s.push_str(
        &transcript
            .iter()
            .filter(|m| matches!(get_str(m, "role").as_str(), "tool" | "user" | "system"))
            .map(content_of)
            .collect::<Vec<_>>()
            .join("\n"),
    );
    s
}

/// Figures in a computed answer that no tool result (or the request itself) contains — the model made them up.
/// Only for tasks that used calculate / data_query since the last user turn; years, dates, times and small whole
/// numbers are ignored; a figure counts as supported when it matches a source number to the precision it is written
/// with (or within 0.1%, for rounded figures like "约 16,300").
pub fn ungrounded_numbers(transcript: &[Value], final_: &str, goal: &str) -> Vec<String> {
    static SMALL: LazyLock<FRegex> = LazyLock::new(|| frx(r"(?<![\d.])\d{1,2}(?:\.\d+)?(?![\d.])"));
    let mut run: Vec<&Value> = Vec::new();
    for m in transcript.iter().rev() {
        run.push(m);
        if get_str(m, "role") == "user" && !is_system_user(m) {
            break;
        }
    }
    let used: HashSet<String> = run
        .iter()
        .flat_map(|m| tool_calls(m))
        .map(|c| stored_call(c).0)
        .collect();
    let uses_calc = CALC_TOOLS.iter().any(|t| used.contains(*t));
    if !uses_calc
        && !(MONEY_GOAL.is_match(goal) && CURRENCY.is_match(&format!("{}{}", goal, final_)))
    {
        // 2026-10-05 S3-09: a voucher-stacking sum done in the head (S$89.78 instead of S$90.25), no calculate at all
        return vec![];
    }
    let src_text = source_text(transcript, goal);
    let mut src: Vec<f64> = NUM
        .captures_iter(&src_text)
        .filter_map(Result::ok)
        .map(|c| {
            num(
                c.get(1).map(|m| m.as_str()).unwrap_or(""),
                c.get(2).is_some(),
            )
        })
        .collect();
    src.extend(
        SMALL
            .find_iter(&src_text)
            .filter_map(Result::ok)
            .filter_map(|m| m.as_str().parse::<f64>().ok()),
    );
    let text = DATEISH.replace_all(final_, " ");
    let mut bad: Vec<String> = Vec::new();
    for c in NUM.captures_iter(&text).filter_map(Result::ok) {
        let tok = c.get(1).map(|m| m.as_str()).unwrap_or("");
        let wan = c.get(2).map(|m| m.as_str());
        let v = num(tok, wan.is_some());
        let plain = wan.is_none() && !tok.contains('.');
        if (plain && v < 1000.0) || (plain && (1900.0..=2100.0).contains(&v) && !tok.contains(','))
        {
            continue;
        }
        let dec = tok.split_once('.').map(|(_, d)| d.len()).unwrap_or(0) as i32;
        let tol = 0.5 * 10f64.powi(-dec) * if wan.is_some() { 10000.0 } else { 1.0 };
        if src
            .iter()
            .any(|x| (v - x).abs() <= tol.max(0.001 * x.abs()))
        {
            continue;
        }
        if src
            .iter()
            .any(|x| (v + x).abs() <= tol.max(0.001 * x.abs()))
        {
            // sign shown differently
            continue;
        }
        let lbl = format!("{}{}", tok, wan.unwrap_or(""));
        if !bad.contains(&lbl) {
            bad.push(lbl);
        }
    }
    bad
}

static FLIGHT: LazyLock<FRegex> =
    LazyLock::new(|| frx(r"(?<![A-Za-z0-9])([A-Z]{2}|[A-Z]\d|\d[A-Z])\s?(\d{2,4})(?![\d])"));
static TRAVEL: LazyLock<Regex> =
    LazyLock::new(|| rx(r"(?i)航班|flight|起飞|落地|抵达|airline|航空|depart|arriv"));

/// Flight numbers in a travel answer that no tool result, nor the request, contains (made up).
pub fn ungrounded_codes(transcript: &[Value], final_: &str, goal: &str) -> Vec<String> {
    static WS: LazyLock<Regex> = LazyLock::new(|| rx(r"\s+"));
    if !TRAVEL.is_match(final_) {
        return vec![];
    }
    let src = WS
        .replace_all(&source_text(transcript, goal), "")
        .to_uppercase();
    let mut bad: Vec<String> = Vec::new();
    for c in FLIGHT.captures_iter(final_).filter_map(Result::ok) {
        let code = format!("{}{}", &c[1], &c[2]);
        if src.contains(&code) || src.contains(&code.to_uppercase()) {
            continue;
        }
        let lbl = c
            .get(0)
            .map(|m| m.as_str().trim().to_string())
            .unwrap_or_default();
        if !bad.contains(&lbl) {
            bad.push(lbl);
        }
    }
    bad
}

const CC_SLD: [&str; 10] = [
    "co", "com", "net", "org", "gov", "ac", "edu", "or", "ne", "go",
];

/// opentable.com / opentable.sg / m.opentable.sg → 'opentable' (one site, many domains).
pub fn site_brand(url: &str) -> String {
    let u = if url.contains("://") {
        url.to_string()
    } else {
        format!("https://{}", url)
    };
    let host = urlparse(&u)
        .ok()
        .and_then(|p| p.hostname())
        .unwrap_or_default()
        .to_lowercase();
    let host = host.trim_matches('.').to_string();
    let parts: Vec<&str> = host.split('.').filter(|p| !p.is_empty()).collect();
    if parts.len() >= 3
        && parts[parts.len() - 1].len() == 2
        && CC_SLD.contains(&parts[parts.len() - 2])
    {
        return parts[parts.len() - 3].to_string();
    }
    if parts.len() >= 2 {
        parts[parts.len() - 2].to_string()
    } else {
        host
    }
}

/// The site of `url` showed a bot wall earlier in this task and the user hasn't taken over since → its brand.
pub fn site_blocked(transcript: &[Value], url: &str) -> String {
    let brand = site_brand(url);
    if brand.is_empty() {
        return String::new();
    }
    let mark = format!("[SITE BLOCKED site={}]", brand);
    for m in transcript.iter().rev() {
        let c = content_of(m);
        let is_tool = get_str(m, "role") == "tool";
        if is_tool && (c.contains(TAKEOVER_DONE_MARK) || c.contains(TAKEOVER_DONE_EN)) {
            return String::new();
        }
        if is_tool && c.starts_with(&mark) {
            return brand;
        }
    }
    String::new()
}

/// Tool arguments for the event log: long strings cut to 300 characters.
pub fn preview_args(a: &Value) -> Value {
    let mut out = Map::new();
    if let Some(o) = a.as_object() {
        for (k, v) in o {
            out.insert(
                k.clone(),
                match v {
                    Value::String(s) => Value::String(truncate(s, 300)),
                    other => other.clone(),
                },
            );
        }
    }
    Value::Object(out)
}
