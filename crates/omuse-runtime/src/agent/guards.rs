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

use super::util::{arr, content_of, dumps_sorted, pystr_or_empty, sget, stored_call, tool_calls};
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

/// The least step budget of a form-filling skill's task (`form: on`): pages, fields, checks and the hand-over.
pub const FORM_STEPS: i64 = 70;

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
    let mut a = if truthy(&a) { a } else { json!({}) };
    // the same link, as written or as its stand-in (crate::links), is the same call
    crate::links::decode_value(&mut a);
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
    // a call that was refused (a guard's ERROR, Sentinel's DENIED) typed nothing (arrival_cards_real 2026-10-30: the
    // refused departure date then blocked the right one as "already typed")
    let refused: HashSet<String> = transcript
        .iter()
        .filter(|m| get_str(m, "role") == "tool")
        .filter(|m| {
            let c = content_of(m);
            c.starts_with("ERROR") || c.starts_with("DENIED")
        })
        .map(|m| get_str(m, "tool_call_id"))
        .collect();
    for ((nm, a), id) in prev.iter().zip(ids.iter()).rev() {
        if refused.contains(id) {
            continue;
        }
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

/// An email with no recipient, no body and no thread is refused before it reaches Sentinel: no approval dialog for an
/// empty mail
/// (arrival_cards2_real 2026-10-30: two `gmail_send` calls with every argument empty at the end of a form fill).
pub fn empty_mail_guard(call: &ToolCall) -> Option<String> {
    if !matches!(call.name.as_str(), "gmail_send" | "gmail_create_draft") {
        return None;
    }
    let args = &call.args;
    // a reply (the thread gives the recipient) or a draft with a body is a real email, even with `to` empty
    let empty = ["to", "body", "reply_to_message_id"]
        .iter()
        .all(|k| sget(args, k).trim().is_empty());
    empty.then(|| {
        format!(
            "ERROR: 没有执行 — 这封邮件没有收件人、没有正文，也不是回复。只有在要写邮件时才调用 {}。 \
Not done: this email has no recipient and no body, and replies to nothing. Call {} only when an email is meant to be \
written.",
            call.name, call.name
        )
    })
}

/// An arrival card is a web form: an email while one is being filled, with no email asked for in the request, gets a
/// note (it still goes to the user's approval as usual) (arrival_cards2_real 2026-10-30: Visit Japan Web ended with a "Send email" approval dialog).
pub fn arrival_mail_guard(transcript: &[Value], goal: &str, call: &ToolCall) -> Option<String> {
    static ASKED: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)\b(send|forward)\b|\be-?mail (it|this|that|them|the|a copy|me)\b|发邮件|发给|寄给").unwrap()
    });
    if !matches!(call.name.as_str(), "gmail_send" | "gmail_reply")
        || !arrival_run(transcript)
        || ASKED.is_match(goal)
    {
        return None;
    }
    Some(
        "入境卡是网页表单，不需要发邮件：在页面上填好，然后用 browser_request_takeover 交给用户提交。 An arrival card \
is a web form — nothing needs emailing. Fill it on the page, then hand it over with browser_request_takeover for the \
user to submit."
            .to_string(),
    )
}

/// What a final answer means, as a small classification call reads it (`Runtime::answer_traits`): the guards that act
/// on the meaning of the model's words take these instead of matching its wording (2026-10-31: regex over prose misread
/// "Neither email was sent", "verify the qualifying spend" and questions worded another way).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AnswerTraits {
    /// asks the user for a departure date, the next city, onward travel or the visa basis
    pub asks_defaults: bool,
    /// asks where the user will stay (a hotel, an address, which of their addresses)
    pub asks_stay: bool,
    /// tells the user to press, click, submit or complete something themselves on a web page
    pub tells_user_to_act: bool,
    /// asks the user to confirm an action their own message already asked for, instead of doing it
    pub reconfirms: bool,
    /// asks the user to choose a product option (colour, size) their request did not name, instead of adding it
    pub asks_option: bool,
    /// says it does not know, or asks for, the user's nationality, date of birth or home address
    pub asks_profile: bool,
}

/// The reply asks the user to confirm what their message already asked for (`AnswerTraits::reconfirms`, the
/// classifier's reading): sent back once to do it — Sentinel asks for an approval itself where one is needed
/// (drive_cleanup_real 2026-10-31: "move these two to the bin" got "Please confirm that you want me to move …").
pub fn reconfirm_hint(reconfirms: bool, zh: bool) -> Option<String> {
    reconfirms.then(|| {
        if zh {
            "（系统）用户这条消息已经明确要求了这个操作，不要再请用户确认：现在就去做。需要审批的操作 Sentinel 会自己弹出审批。".to_string()
        } else {
            "(System) The user's message already asked for this action explicitly — do not ask them to confirm it again; \
do it now. Where an approval is needed, Sentinel asks for it itself."
                .to_string()
        }
    })
}

/// The reply asks for a product option the user's request did not name (`AnswerTraits::asks_option`) in a browser
/// run: the option the page already has selected stands — sent back once to add it and say which one was taken; the
/// user can change it in the cart (shopping_real 2026-10-31: "Saddle Brown or Midnight Black?" for a case already
/// showing Saddle Brown).
pub fn page_option_hint(transcript: &[Value], asks_option: bool, zh: bool) -> Option<String> {
    (asks_option && browser_used(transcript)).then(|| {
        if zh {
            "（系统）用户没有指定颜色/尺寸：就用页面上已经选中的那个（没有选中就用第一个），现在加入购物车，并在回答里说明选了哪个——用户可以在购物车里改。".to_string()
        } else {
            "(System) The user did not name a colour or size: keep the option the page already has selected (or the \
first one), add the product now, and say in your answer which option you took — the user can change it in the cart."
                .to_string()
        }
    })
}

/// The reply says it does not know (or asks for) the user's nationality, birthday or home address
/// (`AnswerTraits::asks_profile`) while the profile has it: sent back once with the profile's values
/// (travel_info_real 2026-10-31: "I couldn't find your passport nationality" with nationality: Australia saved).
pub fn known_profile_hint(asks_profile: bool, profile: &Value, zh: bool) -> Option<String> {
    if !asks_profile {
        return None;
    }
    let known: Vec<String> = ["nationality", "birthday", "address_home"]
        .iter()
        .filter_map(|k| {
            let v = get_str(profile, k);
            (!v.trim().is_empty()).then(|| format!("{k}: {}", v.trim()))
        })
        .collect();
    if known.is_empty() {
        return None;
    }
    Some(if zh {
        format!(
            "（系统）用户资料里已经有：{}。不要再问用户，按这些信息现在就回答。",
            known.join("；")
        )
    } else {
        format!(
            "(System) The user's profile already has: {}. Do not ask the user for it — answer with it now.",
            known.join("; ")
        )
    })
}

/// Each arrival card's filing window, from its official site (the skill sheets say the same): the card's names as an
/// answer writes them, and the window.
const ARRIVAL_WINDOWS: &[(&str, &str, &str)] = &[
    (
        "th-arrival-card",
        r"(?i)\bTDAC\b|Thailand Digital Arrival Card",
        "TDAC: filed within the 3 days before arrival",
    ),
    (
        "sg-arrival-card",
        r"(?i)\bSGAC\b|SG Arrival Card|Singapore Arrival Card",
        "SG Arrival Card: filed within the 3 days before arrival (the arrival day counts)",
    ),
    (
        "my-arrival-card",
        r"(?i)\bMDAC\b|Malaysia Digital Arrival Card",
        "MDAC: filed within the 3 days before arrival",
    ),
    (
        "id-arrival-card",
        r"(?i)All Indonesia|Indonesia(n)? (e-)?arrival card",
        "All Indonesia arrival card: filed within the 3 days before arrival",
    ),
    (
        "kr-arrival-card",
        r"(?i)e-Arrival Card|Korea(n)? (e-)?arrival card",
        "Korea e-Arrival Card: filed within the 3 days before arrival",
    ),
    (
        "tw-arrival-card",
        r"(?i)\bTWAC\b|Taiwan (online )?arrival card",
        "TWAC: filed within the 3 days before arrival",
    ),
    (
        "kh-arrival-card",
        r"(?i)Cambodia(n)? (e-)?arrival card|\be-Arrival\b",
        "Cambodia e-Arrival: filed within the 7 days before arrival by air",
    ),
    (
        "ph-etravel",
        r"(?i)eTravel",
        "eTravel: filed within the 72 hours before arrival",
    ),
];

/// The answer with the filing window of each arrival card it names from a loaded card skill, when it does not state
/// it ("within the 3 days before arrival") — the window is a fixed rule, given in code (travel_info_real 2026-10-31:
/// "File 5–7 November" for a 7 November arrival, when the TDAC opens on the 4th).
pub fn with_arrival_windows(transcript: &[Value], answer: &str) -> String {
    static STATED: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)\b(3|three|7|seven)[ -]days?\b|\b72[ -]hours?\b|\b(3|7)\s*天|72\s*小时")
            .unwrap()
    });
    let loaded: Vec<String> = transcript
        .iter()
        .flat_map(tool_calls)
        .filter_map(|tc| {
            let (name, args) = stored_call(tc);
            (name == "load_skill").then(|| sget(&args, "name"))
        })
        .collect();
    let lines: Vec<&str> = ARRIVAL_WINDOWS
        .iter()
        .filter(|(skill, names, _)| {
            // the card's own skill, or the overviews that answer for every card (travel_info_real 2026-10-31: only
            // entry-requirements loaded, "Submit 5–7 November" for a 7 November TDAC)
            loaded
                .iter()
                .any(|l| l == skill || l == "entry-requirements" || l == "arrival-cards")
                && Regex::new(names).is_ok_and(|r| r.is_match(answer))
        })
        .map(|(_, _, w)| *w)
        .collect();
    if lines.is_empty() || STATED.is_match(answer) {
        return answer.to_string();
    }
    format!("{}\n\n{}.", answer.trim_end(), lines.join(". "))
}

/// Whether the run used the browser (any `browser_*` call).
pub fn browser_used(transcript: &[Value]) -> bool {
    transcript
        .iter()
        .flat_map(tool_calls)
        .any(|tc| stored_call(tc).0.starts_with("browser_"))
}

/// The words that place an address in an arrival skill's country (`sg-arrival-card` → Singapore …).
fn arrival_country(skill: &str) -> &'static [&'static str] {
    match skill.split('-').next().unwrap_or("") {
        "sg" => &["Singapore"],
        "th" => &["Thailand", "Bangkok", "Phuket", "Chiang Mai"],
        "my" => &["Malaysia", "Kuala Lumpur", "Penang"],
        "kr" => &["Korea", "Seoul", "Busan"],
        "jp" => &["Japan", "Tokyo", "Osaka", "Kyoto"],
        "cn" => &["China", "Shanghai", "Beijing", "Shenzhen"],
        "tw" => &["Taiwan", "Taipei"],
        "id" => &["Indonesia", "Bali", "Jakarta"],
        "ph" => &["Philippines", "Manila", "Cebu"],
        "kh" => &["Cambodia", "Phnom Penh", "Siem Reap"],
        _ => &[],
    }
}

/// `answer` naming the user's own addresses in the card's country when an arrival-card answer asks where they stay
/// (no hand-over yet): the profile's addresses there (`profile`: the saved profile, key → value) are listed so the user
/// picks one — read from the profile in code, whether or not the agent looked (arrival_address2_real 2026-10-31:
/// "Where will you be staying?" with two Singapore addresses saved).
pub fn with_own_addresses(
    transcript: &[Value],
    answer: &str,
    asks_stay: bool,
    profile: &Value,
    zh: bool,
) -> String {
    let Some(skill) = form_kind_skills(transcript, "arrival").into_iter().next() else {
        return answer.to_string();
    };
    let country = arrival_country(&skill);
    if country.is_empty() || !asks_stay {
        return answer.to_string();
    }
    let handed = transcript
        .iter()
        .flat_map(tool_calls)
        .any(|tc| stored_call(tc).0 == "browser_request_takeover");
    if handed {
        return answer.to_string();
    }
    let mut there: Vec<(String, String)> = Vec::new();
    if let Some(m) = profile.as_object() {
        for (k, v) in m {
            let value = omuse_common::util::py_str(v);
            let label = match k.as_str() {
                "address_home" => "Home".to_string(),
                "address_work" => "Work".to_string(),
                other => match other.strip_prefix("address:") {
                    Some(l) => l.to_string(),
                    None => continue,
                },
            };
            if country
                .iter()
                .any(|w| value.contains(w) || label.contains(w))
                && !value.trim().is_empty()
            {
                there.push((label, value));
            }
        }
    }
    let low = answer.to_lowercase();
    if there.is_empty()
        || there
            .iter()
            .all(|(l, v)| low.contains(&l.to_lowercase()) || low.contains(&v.to_lowercase()))
    {
        return answer.to_string();
    }
    let list = there
        .iter()
        .map(|(l, v)| format!("{l}: {v}"))
        .collect::<Vec<_>>()
        .join("; ");
    let line = if zh {
        format!("（你的资料里有这些当地地址：{list}。住其中一个的话告诉我是哪一个。）")
    } else {
        format!("(Your profile has these addresses there: {list}. If you stay at one of them, tell me which.)")
    };
    format!("{}\n\n{line}", answer.trim_end())
}

/// The one question an arrival card may ask, before the form is filled (a note on the first `browser_navigate`): with no
/// flight in the request, the mail, the calendar, memory or an uploaded file, ask for the flight and the stay; with two
/// or more of the user's own addresses in the destination country and none named in the request, ask which one
/// (arrival_real / arrival_address2_real 2026-10-30: a TDAC handed over with the flight and stay empty; one of two
/// Singapore addresses picked without asking).
pub fn arrival_ask_first_guard(
    transcript: &[Value],
    goal: &str,
    call: &ToolCall,
) -> Option<String> {
    static FLIGHT: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"\b(?:[A-Z]{2}|[A-Z][0-9]|[0-9][A-Z])\s?[0-9]{1,4}\b").unwrap()
    });
    static MYSELF: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)\bmyself\b|I'?ll (fill|enter|add|do) (it|them|those|that)|leave (it|them|those) (blank|empty|for me)|我自己(填|来)").unwrap()
    });
    static ADDRESS: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?m)^(.*)\((address[^)]*)\): (.+)$").unwrap());
    const MARK: &str = "ask the user first";
    if call.name != "browser_navigate" || MYSELF.is_match(goal) {
        return None;
    }
    // the arrival skills of a trip (not the UK ETA or ETIAS, which ask no flight or stay)
    let skill = form_kind_skills(transcript, "arrival")
        .into_iter()
        .find(|n| !matches!(n.as_str(), "uk-eta" | "eu-etias"))?;
    // once per task: after the refusal the agent asks, and the user's answer comes as the next message
    if transcript
        .iter()
        .any(|m| get_str(m, "role") == "tool" && content_of(m).contains(MARK))
    {
        return None;
    }
    let names = |m: &Value| -> String {
        let id = get_str(m, "tool_call_id");
        transcript
            .iter()
            .flat_map(tool_calls)
            .find(|tc| get_str(tc, "id") == id)
            .map(|tc| stored_call(tc).0)
            .unwrap_or_default()
    };
    let results: Vec<(String, String)> = transcript
        .iter()
        .filter(|m| get_str(m, "role") == "tool")
        .map(|m| (names(m), content_of(m)))
        .collect();
    // the trip's sources: the request, the mail, the calendar, memory, an uploaded ticket (not the profile)
    let trip = results
        .iter()
        .filter(|(n, _)| {
            n.starts_with("gmail_")
                || n.starts_with("calendar_")
                || n.starts_with("memory_")
                || n == "file_look"
                || n.starts_with("files_")
        })
        .map(|(_, c)| c.as_str())
        .chain(std::iter::once(goal))
        .collect::<Vec<_>>()
        .join("\n");
    if !FLIGHT.is_match(&trip) {
        return Some(format!(
            "先问用户（ask the user first）：请求、邮件、日历、记忆里都没有航班。填表之前，在聊天里只问一次：航班号和日期、住哪里（酒店名或地址），\
也可以上传电子机票 / 订单；没有连接邮箱就说一句：连接邮箱（连接页）后下次可以自己找到订单。 Ask the user first: there is no flight in the request, the mail, the calendar or memory. \
Before filling the form, ask once in the chat for the flight (number and date) and where they stay (hotel name or \
address), or to upload the e-ticket / booking; the page stays open for after the answer. With no mailbox connected, say \
that connecting one (Connections) lets you find the bookings yourself next time. ({skill})"
        ));
    }
    let country = arrival_country(&skill);
    let mut there: Vec<(String, String)> = Vec::new();
    for (_, c) in results.iter().filter(|(n, _)| n == "profile_get") {
        for m in ADDRESS.captures_iter(c) {
            let (label, value) = (
                m[1].trim().trim_start_matches("Address — ").to_string(),
                m[3].trim().to_string(),
            );
            let hit = country
                .iter()
                .any(|w| value.contains(w) || label.contains(w));
            if hit && !there.iter().any(|(_, v)| *v == value) {
                there.push((label, value));
            }
        }
    }
    let named = there.iter().any(|(l, v)| {
        let g = goal.to_lowercase();
        g.contains(&l.to_lowercase()) || g.contains(&v.to_lowercase())
    });
    (there.len() >= 2 && !named).then(|| {
        let list = there
            .iter()
            .map(|(l, v)| format!("{l}: {v}"))
            .collect::<Vec<_>>()
            .join("; ");
        format!(
            "先问用户（ask the user first）：资料里有好几个当地地址（{list}）。填表之前，在聊天里问一次住哪一个。 \
Ask the user first: the profile has several addresses there ({list}). Before filling the form, ask once in the chat \
which one they stay at. ({skill})"
        )
    })
}

/// `answer` with the reference the confirmation page showed after the user's hand-back: when the user submitted a form
/// in a takeover and a page read afterwards shows "<label> No. / ID / PIN / Number / reference: <code>", an answer that
/// does not give the code gets "(<label>: <code>)" (arrival_cards2_real 2026-10-30: Visit Japan Web's Registration ID
/// left out of "Your registration is complete").
pub fn with_confirmation_ref(transcript: &[Value], answer: &str) -> String {
    static REF: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)\b((?:[A-Z][\w-]*\s){0,2}(?:No\.|Number|ID|PIN|reference(?: number)?))\s*[:：]\s*([A-Z0-9][A-Z0-9-]{5,})\b").unwrap()
    });
    let Some(after) = transcript.iter().position(|m| {
        get_str(m, "role") == "tool" && {
            let c = content_of(m);
            c.contains(TAKEOVER_DONE_MARK) || c.contains(TAKEOVER_DONE_EN)
        }
    }) else {
        return answer.to_string();
    };
    let found = transcript[after + 1..]
        .iter()
        .filter(|m| get_str(m, "role") == "tool")
        .rev()
        .find_map(|m| {
            let c = content_of(m).replace("\\n", "\n");
            REF.captures_iter(&c)
                .filter(|x| x[2].chars().any(|ch| ch.is_ascii_digit()))
                .map(|x| (x[1].trim().to_string(), x[2].to_string()))
                .next()
        });
    match found {
        Some((label, code)) if !crate::links::decode(answer).contains(&code) => {
            format!("{}\n\n({label}: {code})", answer.trim_end())
        }
        _ => answer.to_string(),
    }
}

/// The label the newest page snapshot gives a field ref (`[e17] textbox "Intended date of departure"`).
fn field_label(transcript: &[Value], r: &str) -> Option<String> {
    let re = Regex::new(&format!(
        r#"\[{}\]\s+\w+\s+\\?"([^"\\]+)"#,
        regex::escape(r)
    ))
    .ok()?;
    transcript
        .iter()
        .rev()
        .filter(|m| get_str(m, "role") == "tool")
        .find_map(|m| re.captures(&content_of(m)).map(|c| c[1].to_string()))
}

/// A form date as typed: DD/MM/YYYY (or with dashes / dots) or YYYY-MM-DD, with the way to write another date alike.
fn form_date(text: &str) -> Option<(chrono::NaiveDate, &'static str)> {
    let t = text.trim();
    for f in ["%d/%m/%Y", "%Y-%m-%d", "%d-%m-%Y", "%d.%m.%Y", "%Y/%m/%d"] {
        if let Ok(d) = chrono::NaiveDate::parse_from_str(t, f) {
            return Some((d, f));
        }
    }
    None
}

/// The departure date an arrival card defaults to is the arrival date + 7 days, counted in code: a departure date
/// typed on the form that is not that gets a note with the right date, once (arrival_cards_real 2026-10-30: TDAC arrival
/// 01/11/2026, departure typed 05/11/2026). A date the user, a return / onward flight or a check-out gave is typed
/// again and goes through.
pub fn departure_date_guard(transcript: &[Value], call: &ToolCall) -> Option<String> {
    static DEPART: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?i)depart|leaving|离境|出境日期").unwrap());
    static ARRIVE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)(arriv|入境).*(date|日期)|(date|日期).*(arriv|入境)").unwrap()
    });
    const MARK: &str = "arrival + 7 days =";
    if call.name != "browser_type" || !arrival_run(transcript) {
        return None;
    }
    let (r, text) = (sget(&call.args, "ref"), sget(&call.args, "text"));
    let (typed, fmt) = form_date(&text)?;
    let label = field_label(transcript, &r)?;
    if !DEPART.is_match(&label) || ARRIVE.is_match(&label) {
        return None;
    }
    if transcript
        .iter()
        .any(|m| get_str(m, "role") == "tool" && content_of(m).contains(MARK))
    {
        return None;
    }
    let arrival = calls_before(transcript, &call.id)
        .into_iter()
        .rev()
        .filter(|(n, _)| n == "browser_type")
        .find_map(|(_, a)| {
            let l = field_label(transcript, &sget(&a, "ref"))?;
            ARRIVE.is_match(&l).then(|| form_date(&sget(&a, "text")))?
        })?
        .0;
    let want = arrival + chrono::Duration::days(7);
    (typed != want).then(|| {
        let (a, w) = (arrival.format(fmt), want.format(fmt));
        format!(
            "离境日期默认是入境日期 {a} + 7 天 = {w}（{MARK} {w}）。如果你输入的日期不是用户、回程 / 后续航班或退房日期给的，\
请用 replace=true 把这一栏改成 {w}。 The default departure date is the arrival date {a} + 7 days = {w} ({MARK} {w}). \
Unless the user, a return / onward flight or a check-out date gave the date you typed, type {w} into that field \
(replace=true)."
        )
    })
}

/// The departure date an arrival card's hand-over puts on the form (`browser_request_takeover` with `set_departure`): the
/// arrival date typed + 7 days, in its format, when nothing in the trip's sources gives another (a return or onward
/// flight, a check-out date, the user's own words) and the form's departure field does not already hold it — the
/// default the user set, applied in code (arrival_cards2_real 2026-10-31: 06/11 typed for an 02/11 arrival, after the note
/// gave 09/11). `None` when there is nothing to set.
pub fn departure_default(transcript: &[Value], goal: &str) -> Option<String> {
    static DEPART: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?i)depart|leaving|离境|出境日期").unwrap());
    static ARRIVE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)(arriv|入境).*(date|日期)|(date|日期).*(arriv|入境)").unwrap()
    });
    static OTHER: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)check-?out|return flight|onward|round[- ]trip|leav(e|ing) on|depart(ing|ure)? on|回程|退房|离开").unwrap()
    });
    if !arrival_run(transcript) || OTHER.is_match(goal) {
        return None;
    }
    // the trip's sources (mail, calendar, memory, an uploaded ticket) naming a return / onward flight or a check-out
    let named_other = transcript.iter().flat_map(tool_calls).any(|tc| {
        let name = stored_call(tc).0;
        (name.starts_with("gmail_")
            || name.starts_with("calendar_")
            || name.starts_with("memory_")
            || name == "file_look")
            && transcript.iter().any(|m| {
                get_str(m, "tool_call_id") == get_str(tc, "id") && OTHER.is_match(&content_of(m))
            })
    });
    if named_other {
        return None;
    }
    let typed: Vec<(String, String)> = transcript
        .iter()
        .flat_map(tool_calls)
        .map(stored_call)
        .filter(|(n, _)| n == "browser_type")
        .filter_map(|(_, a)| Some((field_label(transcript, &sget(&a, "ref"))?, sget(&a, "text"))))
        .collect();
    let (arrival, fmt) = typed
        .iter()
        .rev()
        .find(|(l, _)| ARRIVE.is_match(l))
        .and_then(|(_, t)| form_date(t))?;
    let want = (arrival + chrono::Duration::days(7))
        .format(fmt)
        .to_string();
    let last = typed
        .iter()
        .rev()
        .find(|(l, _)| DEPART.is_match(l) && !ARRIVE.is_match(l))
        .map(|(_, t)| t.trim().to_string());
    (last.as_deref() != Some(want.as_str())).then_some(want)
}

/// The next city an arrival card defaults to is the city the flight came from (the last city typed on the form): another
/// value gets a note with that city, once (arrival_cards_real 2026-10-31: Korea's next city typed as Seoul, the
/// destination itself).
pub fn next_city_note(transcript: &[Value], call: &ToolCall) -> Option<String> {
    static NEXT: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
            r"(?i)next (city|port)|after (this|the) (country|trip)|下一(个)?(城市|目的地|港口)",
        )
        .unwrap()
    });
    static LAST: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)last (city|port)|embark|(city|port) of departure|上一(个)?(城市|港口)|出发(城市|地)").unwrap()
    });
    const MARK: &str = "next city defaults to";
    if call.name != "browser_type" || !arrival_run(transcript) {
        return None;
    }
    let (r, text) = (sget(&call.args, "ref"), sget(&call.args, "text"));
    if text.trim().is_empty() || !NEXT.is_match(&field_label(transcript, &r)?) {
        return None;
    }
    if transcript
        .iter()
        .any(|m| get_str(m, "role") == "tool" && content_of(m).contains(MARK))
    {
        return None;
    }
    let last = calls_before(transcript, &call.id)
        .into_iter()
        .rev()
        .filter(|(n, _)| n == "browser_type")
        .find_map(|(_, a)| {
            let l = field_label(transcript, &sget(&a, "ref"))?;
            (LAST.is_match(&l) && !NEXT.is_match(&l)).then(|| sget(&a, "text"))
        })?;
    let city = last
        .split([',', '(', '/'])
        .next()
        .unwrap_or("")
        .trim()
        .to_lowercase();
    if city.is_empty() || text.to_lowercase().contains(&city) {
        return None;
    }
    Some(format!(
        "下一城市默认是航班出发的城市「{last}」（{MARK} {last}）。如果你输入的不是用户或后续航班给的，请用 replace=true 改成「{last}」。 \
The {MARK} the city the flight came from, \"{last}\". Unless the user or an onward flight gave \"{text}\", type \
\"{last}\" into that field (replace=true)."
    ))
}

/// An arrival card's final Submit is the traveller's own declaration, a claim's Submit the user's own claim: on such a
/// run a click on a button or link named submit / send / confirm / declare (a claim: submit) is refused like a
/// purchase's last click, and the runtime hands the
/// filled page over (`exec::hand_over_button`) (arrival_cards2_real 2026-10-30: "I will submit as soon as you hand
/// the browser back", then the agent pressed Submit itself).
pub fn arrival_submit_guard(transcript: &[Value], call: &ToolCall) -> Option<String> {
    static SUBMIT: LazyLock<Regex> = LazyLock::new(|| {
        // not "Confirm": a terms page's Confirm only opens the form (cards_real 2026-10-31: Korea's e-Arrival card)
        // nor "Declaration" (a heading or a tile: China's "Entry Declaration" opens the form)
        Regex::new(r"(?i)\bsubmit|\bsend\b|\bpay\b|提交|递交|送出").unwrap()
    });
    // a claim the user submits (cards_claim_real 2026-10-31: the agent pressed "Submit claim" itself): its Submit only
    static SUBMIT_CLAIM: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?i)\bsubmit|提交|递交").unwrap());
    if call.name != "browser_click" {
        return None;
    }
    let arrival = arrival_run(transcript);
    if !arrival && !claim_run(transcript) {
        return None;
    }
    let r = sget(&call.args, "ref");
    let re = Regex::new(&format!(
        r#"\[{}\]\s+(\w+)\s+\\?"([^"\\]+)"#,
        regex::escape(&r)
    ))
    .ok()?;
    let (role, label) = transcript
        .iter()
        .rev()
        .filter(|m| get_str(m, "role") == "tool")
        .find_map(|m| {
            re.captures(&content_of(m))
                .map(|c| (c[1].to_string(), c[2].trim().to_string()))
        })?;
    let submits = if arrival { &SUBMIT } else { &SUBMIT_CLAIM };
    // a button that submits; a link is navigation (Taiwan's "Submit Arrival Card" tile opens the form)
    if role != "button" || !submits.is_match(&label) {
        return None;
    }
    Some(format!(
        "ERROR: 没有执行 — 「{label}」由用户自己提交。 (\"{label}\" submits the user's own declaration or claim: \
that click is the user's own. The filled form is handed over to the user now.)"
    ))
}

/// The names of the skills this run loaded whose front matter says `form_kind: <kind>` — `arrival` (an arrival card or
/// travel authorisation the traveller submits: the arrival cards, Visit Japan Web, eTravel, the UK ETA, ETIAS) or
/// `claim` (a claim the user submits on a web form: CSR, FWD). Read from the loaded skill's own text, so an imported
/// skill with the flag gets the same guards (2026-10-31: ETIAS, missing from a hard-coded list, got none of them).
fn form_kind_skills(transcript: &[Value], kind: &str) -> Vec<String> {
    let mut out = Vec::new();
    for tc in transcript.iter().flat_map(tool_calls) {
        let (name, args) = stored_call(tc);
        if name != "load_skill" || !sget(&args, "file").trim().is_empty() {
            continue;
        }
        let id = get_str(tc, "id");
        let loaded = transcript
            .iter()
            .find(|m| get_str(m, "role") == "tool" && get_str(m, "tool_call_id") == id)
            .map(content_of)
            .unwrap_or_default();
        let skill = sget(&args, "name");
        if loaded.starts_with("ERROR") {
            continue;
        }
        let mut fm = crate::skills::front_matter(loaded.trim_start());
        if !loaded.trim_start().starts_with("---") || !fm.contains_key("form_kind") {
            // no (whole) front matter in what came back — a profile note on top, or a result the compression cut to
            // its first 500 characters, inside the long description (cards_interruption_real 2026-10-31: the claim
            // was filled and never handed over): the built-in skill's own
            let own = crate::skills::BUILTIN
                .get_file(format!("{skill}/SKILL.md"))
                .and_then(|f| f.contents_utf8())
                .map(crate::skills::front_matter)
                .unwrap_or_default();
            if !own.is_empty() {
                fm = own;
            }
        }
        if fm
            .get("form_kind")
            .is_some_and(|k| k.eq_ignore_ascii_case(kind))
        {
            out.push(skill);
        }
    }
    out
}

/// The Hong Kong departure-tax refund's face recognition is the user's own step, on their phone: a click on "Proceed with
/// Facial Recognition" (or any face-recognition button) in that run is refused, and the agent stops with the reference
/// code and the link (`with_apdt_link`).
pub fn apdt_face_guard(transcript: &[Value], call: &ToolCall) -> Option<String> {
    static FACE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)facial|face[- ]?(recognition|verification|check)|人脸|人臉|刷脸").unwrap()
    });
    // a takeover once the card is in (from the Vault, or typed by the user in an earlier takeover): only the face step
    // is left, and that is on the phone (apdt_real 2026-10-31: "take over the browser to complete that verification")
    if call.name == "browser_request_takeover" && apdt_card_done(transcript) {
        return Some(
            "ERROR: 没有交接 — 银行卡已经填好，剩下的人脸识别由用户本人在手机上完成，不需要接管浏览器。停在这里，把参考编号和申请链接告诉用户。 \
Not handed over: the card is filled in and what is left, the face recognition, is the user's own step on their phone — \
no takeover is needed. Stop here and give the user the reference number and the link to their application."
                .to_string(),
        );
    }
    if !matches!(call.name.as_str(), "browser_click" | "browser_click_at") {
        return None;
    }
    if !apdt_run(transcript) {
        return None;
    }
    let label = field_label(transcript, &sget(&call.args, "ref")).unwrap_or_default();
    FACE.is_match(&label).then(|| {
        "ERROR: 没有执行 — 人脸识别由用户本人在手机上完成。停在这里，把参考编号和申请链接告诉用户。 Not done: the face recognition \
is the user's own step, on their phone. Stop here and give the user the reference number and the link to their application."
            .to_string()
    })
}

/// Whether the run loaded the Hong Kong departure-tax refund skill.
fn apdt_run(transcript: &[Value]) -> bool {
    transcript.iter().flat_map(tool_calls).any(|tc| {
        let (name, args) = stored_call(tc);
        name == "load_skill" && sget(&args, "name") == "hk-apdt-refund"
    })
}

/// A departure-tax refund run whose refund card is in: the Vault card's number filled, or a completed takeover (the
/// user typed it). What is left is the face recognition, on the user's phone — nothing to hand over in the browser.
pub fn apdt_card_done(transcript: &[Value]) -> bool {
    if !apdt_run(transcript) {
        return false;
    }
    let calls: Vec<(String, String, Value)> = transcript
        .iter()
        .flat_map(tool_calls)
        .map(|tc| {
            let (n, a) = stored_call(tc);
            (get_str(tc, "id"), n, a)
        })
        .collect();
    transcript.iter().any(|m| {
        get_str(m, "role") == "tool"
            && !content_of(m).starts_with("ERROR")
            && calls.iter().any(|(id, n, a)| {
                *id == get_str(m, "tool_call_id")
                    && ((n == "browser_fill_secret" && sget(a, "field") == "number")
                        || n == "browser_request_takeover")
            })
    })
}

/// The fixed answer on the departure-tax refund's upload page: OMuse submits for the applicant, so "I have authorized a
/// third party to submit my information" is always ticked — in code, before the agent's first click on that page moves
/// on (apdt_real 2026-10-31: the declaration ticked, the authorisation left blank). The `browser_fill_form` args.
pub fn apdt_fixed_ticks(transcript: &[Value], call: &ToolCall) -> Option<Value> {
    static PAGE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r#"source="web page (https?://[^"\s]+)"#).unwrap());
    if call.name != "browser_click" || !apdt_run(transcript) {
        return None;
    }
    let url = transcript
        .iter()
        .rev()
        .filter(|m| get_str(m, "role") == "tool")
        .find_map(|m| PAGE.captures(&content_of(m)).map(|c| c[1].to_string()))?;
    if !(url.contains("apdtrefund.hk") && url.contains("apdt_upload")) {
        return None;
    }
    // `on_box`: the click is on that box itself — it becomes the tick (a click could untick it)
    let label = field_label(transcript, &sget(&call.args, "ref")).unwrap_or_default();
    let on_box = label.to_lowercase().contains("third party");
    let fills: Vec<Value> = [crate::forms::tick(
        "third_party",
        "",
        "authorized a third party",
    )]
    .iter()
    .filter_map(crate::forms::Op::to_json)
    .collect();
    Some(json!({"form": "hk-apdt-refund", "fills": fills, "on_box": on_box}))
}

/// Whether the run loaded a skill whose claim the user submits themselves on a web form (`form_kind: claim`).
pub fn claim_run(transcript: &[Value]) -> bool {
    !form_kind_skills(transcript, "claim").is_empty()
}

static WEBSITE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)https?://|\bwww\.|\b[a-z0-9-]+\.(com|org|net|gov|edu|io|co|sg|hk|jp|uk|au|cn|tw|kr|th|my|travel)\b|\b(web ?site|web ?page|site|portal|online form|search page)s?\b|网站|网页|官网|页面")
        .unwrap()
});

/// Whether the user's request names a website or a page to open (a URL, a domain, "the … site / page / portal"):
/// a skill's `browser: off` then leaves the browser on. Read from the user's words, never the model's.
pub fn names_website(goal: &str) -> bool {
    WEBSITE.is_match(goal)
}

/// Whether the run loaded an arrival-card skill (`form_kind: arrival`).
pub fn arrival_run(transcript: &[Value]) -> bool {
    !form_kind_skills(transcript, "arrival").is_empty()
}

/// On an arrival card, clearing a field the agent typed on this page gets a note (`exec_call` notes): its values are the defaults the user
/// asked for (departure = arrival + 7 days, next city = the flight's origin), which the user reviews and corrects on
/// the page after the hand-over (arrival_real 2026-10-30: the TDAC's next city and departure date typed, then wiped,
/// and the user asked for them in the chat).
pub fn clear_guard(transcript: &[Value], call: &ToolCall) -> Option<String> {
    let args = &call.args;
    let r = sget(args, "ref");
    if call.name != "browser_type" || r.is_empty() || !sget(args, "text").trim().is_empty() {
        return None;
    }
    if !arrival_run(transcript) {
        return None;
    }
    let typed = calls_before(transcript, &call.id)
        .iter()
        .rev()
        .take_while(|(nm, _)| nm != "browser_navigate" && nm != "browser_back")
        .any(|(nm, a)| nm == "browser_type" && sget(a, "ref") == r && !sget(a, "text").is_empty());
    typed.then(|| {
        format!(
            "你清空了 {r}：那是用户要的默认值（离境日期 = 入境 + 7 天，下一城市 = 航班出发城市，健康和申报问题「否」），\
除非用户说过别的，请把它填回去；用户接手后会在页面上核对。 You cleared {r}: keep the value in {r} — it held the default \
the user asked for (departure = arrival + 7 days, next city = the city the flight comes from, health and declaration \
questions No). Type it back unless the user said otherwise; the user reviews it on the page after the hand-over, and \
the browser_request_takeover reason says what you assumed."
        )
    })
}

/// A hint for an arrival-card run that ends asking the user, in the chat, for a value the form has a default for —
/// with no hand-over: fill the default and hand over; and for a claim or application the user submits (CSR, FWD, the
/// Hong Kong refund, a baggage claim) that filled its form and ended without the hand-over (`None` when the run handed the page over, or the answer asks
/// nothing of the kind).
pub fn arrival_default_hint(transcript: &[Value], asks_defaults: bool, zh: bool) -> Option<String> {
    // the form was being filled (typed into, a Vault value put in): ending without the hand-over is the same mistake,
    // whatever the answer asks (arrival_cards3_real 2026-10-30: eTravel's middle name asked in the chat)
    let filled = transcript.iter().flat_map(tool_calls).any(|tc| {
        matches!(
            stored_call(tc).0.as_str(),
            "browser_type" | "browser_select" | "browser_fill_secret"
        )
    });
    let handed = transcript.iter().any(|m| {
        get_str(m, "role") == "tool"
            && !content_of(m).starts_with("ERROR")
            && transcript.iter().flat_map(tool_calls).any(|tc| {
                get_str(tc, "id") == get_str(m, "tool_call_id")
                    && stored_call(tc).0 == "browser_request_takeover"
            })
    });
    if handed {
        return None;
    }
    // a claim or an application the user submits: filled, then ended without handing the page over
    // (cards_interruption_real 2026-10-30: "The Trip Interruption claim is filled on eClaimsLine and has not been
    // submitted", no takeover)
    let submits_own = claim_run(transcript);
    if !arrival_run(transcript) {
        return (submits_own && filled).then(|| {
            if zh {
                "（系统）表格已经填好但还没交给用户。现在调用 browser_request_takeover，原因里写明请用户核对、补上缺的内容并自己提交；\
不要在聊天里问。".to_string()
            } else {
                "(System) The form is filled but not handed over. Call browser_request_takeover now, with a reason asking the \
user to review it, fill in anything missing and submit it themselves; don't ask in the chat.".to_string()
            }
        });
    }
    if !(filled || asks_defaults) {
        return None;
    }
    Some(if zh {
        "（系统）不要在聊天里问这些：离境日期默认入境 + 7 天，下一城市默认航班出发的城市，健康和申报问题默认「否」，邮件里没有签证就按免签入境。\
把这些默认值填进页面；没有值的项（例如中间名）留空，不要在聊天里问。然后调用 browser_request_takeover，原因里写明你假设了什么、\
留给用户的是什么（留空的项、验证码、提交）。".to_string()
    } else {
        "(System) Don't ask these in the chat: the departure date defaults to arrival + 7 days, the next city to the city \
the flight comes from, every health and declaration question to No, the entry basis to visa-free when no visa is in \
the mail. Fill those defaults on the page and leave a field you have no value for (a middle name) empty instead of \
asking in the chat; then call browser_request_takeover saying what you assumed and what is left for the user (the \
empty fields, the CAPTCHA, Submit).".to_string()
    })
}

/// A hint for any run that used the browser and ends telling the user to press or submit something themselves, but
/// never handed the page over: the user has no live view to do it in (refunds_real 2026-10-31: "I've left the final
/// Request cancellation for you", no takeover).
pub fn self_press_hint(transcript: &[Value], tells_user_to_act: bool, zh: bool) -> Option<String> {
    if !tells_user_to_act || apdt_card_done(transcript) {
        return None;
    }
    let calls: Vec<(String, String)> = transcript
        .iter()
        .flat_map(tool_calls)
        .map(|tc| (get_str(tc, "id"), stored_call(tc).0))
        .collect();
    if !calls.iter().any(|(_, n)| n.starts_with("browser_")) {
        return None;
    }
    let handed = transcript.iter().any(|m| {
        get_str(m, "role") == "tool"
            && !content_of(m).starts_with("ERROR")
            && calls
                .iter()
                .any(|(id, n)| *id == get_str(m, "tool_call_id") && n == "browser_request_takeover")
    });
    (!handed).then(|| {
        if zh {
            "（系统）你让用户自己点，但还没把浏览器交给用户。现在调用 browser_request_takeover，原因里写明要点哪个按钮；交回后读结果页告诉用户。"
                .to_string()
        } else {
            "(System) You tell the user to press it themselves, but the browser was never handed over. Call \
browser_request_takeover now, naming the button in the reason; after the hand-back read the result page and tell the user."
                .to_string()
        }
    })
}

/// A hint when the answer after the user handed the browser back is only a line or two: the whole request's answer —
/// what the run found and worked out before the hand-over — is written again with what the page shows now
/// (insurance_real 2026-10-31: "Your claim was submitted, reference CL-…", the delay and the amount gone).
pub fn short_after_handback_hint(transcript: &[Value], answer: &str, zh: bool) -> Option<String> {
    let handed_back = transcript.iter().any(|m| {
        get_str(m, "role") == "tool" && {
            let c = content_of(m);
            c.contains(TAKEOVER_DONE_MARK) || c.contains(TAKEOVER_DONE_EN)
        }
    });
    if !handed_back || answer.trim().chars().count() >= 400 {
        return None;
    }
    Some(if zh {
        "（系统）这个回答太短了。重新写完整的回答：先写你在交给用户之前查到和算出的内容（金额、日期、延误时长、期限、编号），再写页面现在显示的结果。不要提到这次检查。"
            .to_string()
    } else {
        "(System) That answer is too short. Write the complete answer again: what you found and worked out before the \
hand-over (amounts, dates, the delay, deadlines, references), then what the page shows now. Don't mention this check."
            .to_string()
    })
}

/// Whether a run ends with a filled form the user must finish but no hand-over: an arrival card or a claim whose
/// form was typed into, or any browser run whose answer tells the user to press it themselves — after the one hint
/// (`arrival_default_hint`, `self_press_hint`) the runtime then hands the page over itself (cards_claim_real
/// 2026-10-31: "submit it yourself when ready", twice, no takeover).
pub fn needs_handover(transcript: &[Value], traits: AnswerTraits) -> bool {
    let calls: Vec<(String, String)> = transcript
        .iter()
        .flat_map(tool_calls)
        .map(|tc| (get_str(tc, "id"), stored_call(tc).0))
        .collect();
    // handed over = a completed takeover after the last fill: a sign-in before the form is not the hand-over
    // (arrival_cards2_real 2026-10-31: Visit Japan Web, signed in, filled, never handed over)
    let fills = |n: &str| {
        matches!(
            n,
            "browser_type"
                | "browser_select"
                | "browser_fill_secret"
                | "form_fill"
                | "browser_fill_form"
        )
    };
    let last_fill = transcript
        .iter()
        .rposition(|m| tool_calls(m).iter().any(|tc| fills(&stored_call(tc).0)));
    let handed = transcript.iter().enumerate().any(|(i, m)| {
        last_fill.is_none_or(|lf| i > lf)
            && get_str(m, "role") == "tool"
            && !content_of(m).starts_with("ERROR")
            && calls
                .iter()
                .any(|(id, n)| *id == get_str(m, "tool_call_id") && n == "browser_request_takeover")
    });
    if handed || apdt_card_done(transcript) {
        return false;
    }
    let filled = calls.iter().any(|(_, n)| {
        matches!(
            n.as_str(),
            "browser_type" | "browser_select" | "browser_fill_secret"
        )
    });
    let form_filled = calls
        .iter()
        .any(|(_, n)| n == "form_fill" || n == "browser_fill_form");
    let took_over_before = calls.iter().any(|(_, n)| n == "browser_request_takeover");
    (filled && (arrival_run(transcript) || claim_run(transcript)))
        // an arrival card whose answer asks for what has a default (departure date, Yes / No) instead of filling it:
        // the hand-over fills those in code (arrival_cards2_real 2026-10-31: Visit Japan Web, signed in, then asked)
        // — only once the run is on the form: something filled, or an earlier hand-over (the sign-in) came back
        // (arrival_cards_real 2026-10-31: a Korea landing page was handed over before anything was done)
        || (traits.asks_defaults && arrival_run(transcript) && (filled || form_filled || took_over_before))
        || (purchase_approved(transcript, &calls) && browser_used(transcript))
        || self_press_hint(transcript, traits.tells_user_to_act, false).is_some()
}

/// What the user changed in approval dialogs during this run: (the record's id, field, the user's value), read from
/// results that carry `user_edited` (Sentinel adds it when the user edits an action before approving).
fn user_edits(transcript: &[Value]) -> Vec<(String, String, String)> {
    let mut out = Vec::new();
    for m in transcript.iter().filter(|m| get_str(m, "role") == "tool") {
        let text = content_of(m);
        let Some(i) = text.find('{') else { continue };
        // a result cut by the transcript's compression is no longer JSON: its `user_edited` object (Sentinel puts it
        // first) is read on its own
        static EDITED: LazyLock<Regex> =
            LazyLock::new(|| Regex::new(r#""user_edited":\s*(\{[^{}]*\})"#).unwrap());
        let v = match serde_json::from_str::<Value>(&text[i..]) {
            Ok(v) => v,
            Err(_) => match EDITED
                .captures(&text)
                .and_then(|c| serde_json::from_str::<Value>(&c[1]).ok())
            {
                Some(e) => json!({"user_edited": e}),
                None => continue,
            },
        };
        let Some(edited) = v.get("user_edited").and_then(Value::as_object) else {
            continue;
        };
        let id = v
            .get("event")
            .and_then(|e| e.get("id"))
            .or_else(|| v.get("id"))
            .map(omuse_common::util::py_str)
            .unwrap_or_default();
        for (k, val) in edited {
            out.push((id.clone(), k.clone(), omuse_common::util::py_str(val)));
        }
    }
    out
}

/// An update that would set back what the user changed in an approval dialog does not run: the change is the user's
/// own (gcal_real 2026-10-31: the user moved the invite to 15:30, the agent called calendar_update_event with 15:00
/// again despite the note). Other fields of the update are the agent's business.
pub fn user_edit_guard(transcript: &[Value], call: &ToolCall) -> Option<String> {
    if !call.name.ends_with("_update_event") && !call.name.ends_with("_update") {
        return None;
    }
    let id = ["event_id", "id", "page_id"]
        .iter()
        .map(|k| sget(&call.args, k))
        .find(|v| !v.is_empty())?;
    let reverted: Vec<String> = user_edits(transcript)
        .into_iter()
        .filter(|(rid, field, val)| {
            *rid == id && {
                let now = sget(&call.args, field);
                !now.is_empty() && now.trim() != val.trim()
            }
        })
        .map(|(_, field, val)| format!("{field} = {val}"))
        .collect();
    if reverted.is_empty() {
        return None;
    }
    Some(format!(
        "ERROR: 没有执行 — 这是用户在审批时自己改的：{0}，保持不变。 (Not changed: the user set {0} themselves in the approval \
dialog; it stays. Tell the user the event is saved with their change.)",
        reverted.join("; ")
    ))
}

/// `answer` with what the user changed in approval dialogs, so the answer never presents the user's own change as a
/// mistake (gcal_real 2026-10-31: "Google Calendar reports it as 3:30 … rather than the 3:00 slot I requested").
pub fn with_user_edits(transcript: &[Value], answer: &str, zh: bool) -> String {
    let edits = user_edits(transcript);
    if edits.is_empty() {
        return answer.to_string();
    }
    let what: Vec<String> = edits.iter().map(|(_, f, v)| format!("{f} = {v}")).collect();
    let line = if zh {
        format!(
            "（你在审批时改了：{}，已按你的修改保存。）",
            what.join("；")
        )
    } else {
        format!(
            "(You changed this in the approval: {} — saved with your change.)",
            what.join("; ")
        )
    };
    format!("{}\n\n{line}", answer.trim_end())
}

/// The claim deadlines a claim skill's rules give, counted from the loss date in code: (skill, [(days after the loss,
/// what is due — English, Chinese)]). The days are the benefit's terms (the CSR guide to benefits: notice within 20
/// days, the documents within 90).
/// One deadline of a claim: days after the loss, what is due (English, Chinese).
type Deadline = (i64, &'static str, &'static str);

const CLAIM_DEADLINES: &[(&str, &[Deadline])] = &[(
    "csr-trip-claim",
    &[
        (
            20,
            "notify the claims administrator (eClaimsLine)",
            "通知理赔管理方（eClaimsLine）",
        ),
        (90, "send the claim documents", "提交理赔材料"),
    ],
)];

/// A claim form's benefit choice ("Trip Cancellation").
static BENEFIT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^\s*trip (cancell?ation|interruption|delay)\b").unwrap());

/// `answer` with the dates its claim must meet, computed from the loss date typed on the claim form: the date in the
/// field labelled loss / incident, else the only date typed in the run (cards_claim_real 2026-10-31: no dates given;
/// before that, the model counted the days itself). Always added — a fact code owns.
pub fn with_claim_deadlines(transcript: &[Value], answer: &str, zh: bool) -> String {
    let loaded: Vec<String> = transcript
        .iter()
        .flat_map(tool_calls)
        .filter_map(|tc| {
            let (name, args) = stored_call(tc);
            (name == "load_skill").then(|| sget(&args, "name"))
        })
        .collect();
    let Some((_, rules)) = CLAIM_DEADLINES
        .iter()
        .find(|(skill, _)| loaded.iter().any(|l| l == skill))
    else {
        return answer.to_string();
    };
    static LOSS: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)loss|incident|cancell?ation date|interruption date|事故|损失").unwrap()
    });
    let typed: Vec<(Option<String>, chrono::NaiveDate)> = transcript
        .iter()
        .flat_map(tool_calls)
        .filter_map(|tc| {
            let (name, args) = stored_call(tc);
            if name != "browser_type" {
                return None;
            }
            let (d, _) = form_date(&sget(&args, "text"))?;
            Some((field_label(transcript, &sget(&args, "ref")), d))
        })
        .collect();
    let loss = typed
        .iter()
        .find(|(l, _)| l.as_deref().is_some_and(|l| LOSS.is_match(l)))
        .map(|(_, d)| *d)
        .or_else(|| (typed.len() == 1).then(|| typed[0].1));
    let Some(loss) = loss else {
        return answer.to_string();
    };
    let fmt = |d: chrono::NaiveDate| d.format("%-d %B %Y").to_string();
    let items: Vec<String> = rules
        .iter()
        .map(|(days, en, cn)| {
            let due = fmt(loss + chrono::Duration::days(*days));
            if zh {
                format!("{due} 前{cn}（损失日 + {days} 天）")
            } else {
                format!("{en} by {due} (loss date + {days} days)")
            }
        })
        .collect();
    // the benefit picked on the form ("Trip Cancellation"), named with the deadlines when the answer leaves it out
    // (cards_claim_real 2026-10-31: the answer after the hand-back named neither benefit)
    let benefit = transcript
        .iter()
        .flat_map(tool_calls)
        .filter_map(|tc| {
            let (name, args) = stored_call(tc);
            (name == "browser_select").then(|| sget(&args, "value"))
        })
        .rfind(|v| BENEFIT.is_match(v))
        .filter(|v| !answer.to_lowercase().contains(&v.to_lowercase()))
        .map(|v| format!("{} ", v.trim()))
        .unwrap_or_default();
    let line = if zh {
        format!(
            "**{benefit}理赔期限**（损失日 {}）：{}。",
            fmt(loss),
            items.join("；")
        )
    } else {
        format!(
            "**{benefit}Claim deadlines** (loss date {}): {}.",
            fmt(loss),
            items.join("; ")
        )
    };
    format!("{}\n\n{line}", answer.trim_end())
}

/// `answer` with every phone number of 8+ digits that no tool result showed replaced by a pointer to the official
/// page: an emergency or embassy line must come from a source, never from the model's memory (emergency_real
/// 2026-10-31: "+61 2 8055 1700" and "+81 3 6206 3070", on none of the pages read). Digits are compared without
/// spaces, dashes or a leading +; short local numbers (110, 119, 999) stay.
pub fn with_grounded_numbers(transcript: &[Value], answer: &str, zh: bool) -> String {
    static PHONE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"\+?\(?\d[\d\s().-]{6,}\d").unwrap());
    let digits = |x: &str| x.chars().filter(char::is_ascii_digit).collect::<String>();
    let sources: String = transcript
        .iter()
        .filter(|m| matches!(get_str(m, "role").as_str(), "tool" | "user"))
        .map(|m| digits(&content_of(m)))
        .collect::<Vec<_>>()
        .join(" ");
    if sources.is_empty() {
        return answer.to_string();
    }
    let note = if zh {
        "（号码未在读过的页面上核实——请查官方网站）"
    } else {
        "(number not on the pages read — check the official site)"
    };
    PHONE
        .replace_all(answer, |c: &regex::Captures| {
            let n = digits(&c[0]);
            // a date, an amount or a reference is no phone number: only 8–15 digits written as a number is
            static DATE: LazyLock<Regex> = LazyLock::new(|| {
                Regex::new(
                    r"^\s*(\d{4}[-/.]\d{1,2}[-/.]\d{1,2}|\d{1,2}[-/.]\d{1,2}[-/.]\d{2,4})\s*$",
                )
                .unwrap()
            });
            let phoneish =
                (c[0].trim_start().starts_with('+') || c[0].contains(' ') || c[0].contains('-'))
                    && !DATE.is_match(&c[0]);
            if n.len() < 8 || n.len() > 15 || !phoneish || sources.contains(&n) {
                c[0].to_string()
            } else {
                note.to_string()
            }
        })
        .into_owned()
}

/// A field-by-field fill on a page a form function knows, before `form_fill` ran: a note that the form is filled in
/// code (cards_real 2026-10-31: the agent typed Korea's terms page itself and never called form_fill). The page is
/// the newest browser result's ("web page <url>").
pub fn form_fill_note(transcript: &[Value], call: &ToolCall) -> Option<String> {
    let opening = call.name == "browser_navigate";
    let back = call.name == "browser_request_takeover";
    if !opening
        && !back
        && !matches!(
            call.name.as_str(),
            "browser_type" | "browser_select" | "browser_click" | "browser_fill_secret"
        )
    {
        return None;
    }
    // form_fill done, unless it stopped for the user's step (a sign-in, an emailed code, a photo)
    let done = transcript
        .iter()
        .filter(|m| get_str(m, "role") == "tool")
        .any(|m| {
            let c = content_of(m);
            c.contains("\"filled\"")
                && c.contains("\"url\"")
                && !c.contains("\"stop\": true")
                && !c.contains("\"stop\":true")
        });
    if done {
        return None;
    }
    static PAGE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r#"source="web page (https?://[^"\s]+)"#).unwrap());
    let url = if opening {
        sget(&call.args, "url")
    } else {
        transcript
            .iter()
            .rev()
            .filter(|m| get_str(m, "role") == "tool")
            .find_map(|m| PAGE.captures(&content_of(m)).map(|c| c[1].to_string()))?
    };
    let (form, _) = crate::forms::form_for(&url)?;
    Some(if back {
        format!(
            "This page is a known form ({form}): now call form_fill once with the trip facts — code fills the form from \
here. Do not navigate away or reload: the form keeps the user's step (a sign-in, a code) only on this page."
        )
    } else {
        format!(
            "This page is a known form ({form}): call form_fill once with the trip facts — code fills the whole form, its \
steps and the passport from the Vault — instead of filling it field by field. A step that is the user's (a sign-in, an \
emailed code, a photo) is named in its result: hand the page over for it, then call form_fill again."
        )
    })
}

/// A duration of an hour or more given only in minutes gets its hours next to it — "a delay of 435 minutes (7 h 15
/// min)" — so the user reads it at a glance (insurance_real 2026-10-31: the claim's 7 h 15 min delay as "435 minutes").
pub fn with_hours(answer: &str) -> String {
    static MINS: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)\b(\d{2,4})(\s?(?:minutes|mins?)\b|\s?分钟)(\s*[（(])?").unwrap()
    });
    MINS.replace_all(answer, |c: &regex::Captures| {
        let n: u64 = c[1].parse().unwrap_or(0);
        // already followed by a bracket (most likely the hours), under an hour, or over a day (a year, a count): as
        // written
        if !(60..=1440).contains(&n) || c.get(3).is_some() {
            return c[0].to_string();
        }
        let (h, m) = (n / 60, n % 60);
        let zh = c[2].contains('分');
        let hours = match (zh, m) {
            (true, 0) => format!("{h} 小时"),
            (true, _) => format!("{h} 小时 {m} 分钟"),
            (false, 0) => format!("{h} h"),
            (false, _) => format!("{h} h {m} min"),
        };
        if zh {
            format!("{}{}（{hours}）", &c[1], &c[2])
        } else {
            format!("{}{} ({hours})", &c[1], &c[2])
        }
    })
    .into_owned()
}

/// A hand-over on a known form before code has filled it (`form_fill` / `browser_fill_form` never ran) is refused
/// once: the form function fills it first, and a step that is the user's (a photo, a sign-in) is named in its result
/// (arrival_cards2_real 2026-10-31: the China card handed over empty for the passport photo, then filled after the
/// user's Submit). A second takeover goes ahead.
pub fn takeover_before_fill_guard(transcript: &[Value], call: &ToolCall) -> Option<String> {
    const MARK: &str = "Not handed over yet: this page is a known form";
    // the hand-overs code makes itself (the automatic one, after a refused final click) always go ahead
    if call.name != "browser_request_takeover"
        || call.id.starts_with(AUTO_HANDOVER)
        || call.id.ends_with("_handover")
    {
        return None;
    }
    let calls: Vec<String> = transcript
        .iter()
        .flat_map(tool_calls)
        .map(|tc| stored_call(tc).0)
        .collect();
    if calls
        .iter()
        .any(|n| n == "form_fill" || n == "browser_fill_form")
    {
        return None;
    }
    if transcript
        .iter()
        .any(|m| get_str(m, "role") == "tool" && content_of(m).contains(MARK))
    {
        return None;
    }
    static PAGE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r#"source="web page (https?://[^"\s]+)"#).unwrap());
    let url = transcript
        .iter()
        .rev()
        .filter(|m| get_str(m, "role") == "tool")
        .find_map(|m| PAGE.captures(&content_of(m)).map(|c| c[1].to_string()))?;
    let (form, _) = crate::forms::form_for(&url)?;
    Some(format!(
        "ERROR: {MARK} ({form}) that code has not filled. Call form_fill once with the trip facts first — it fills the \
form, its steps and the passport from the Vault, and names any step that is the user's (a photo, a sign-in, a code); \
then hand the page over with browser_request_takeover."
    ))
}

/// Whether the run loaded the news research skill (its searches stay in the browser).
pub fn news_research_run(transcript: &[Value]) -> bool {
    transcript.iter().flat_map(tool_calls).any(|tc| {
        let (name, args) = stored_call(tc);
        name == "load_skill" && sget(&args, "name") == "news-research"
    })
}

/// "email it" after a news report: the note that says which edition and which files, read from the chat's own file
/// cards (news_report_real 2026-11-01: "email it" got "What would you like me to email, and to whom?").
pub fn report_followup_note(goal: &str, msgs: &[Value]) -> Option<String> {
    static ASK: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
            r"(?i)^\W*(please\W+)?(e-?mail|mail|send)\b[^\n]{0,40}$|^\W*(发|寄)(个|到)?(邮件|邮箱)",
        )
        .unwrap()
    });
    if !ASK.is_match(goal.trim()) {
        return None;
    }
    let mut paths: Vec<String> = Vec::new();
    for m in msgs.iter().rev().filter(|m| get_str(m, "role") == "system") {
        let Ok(card) = serde_json::from_str::<Value>(&get_str(m, "content")) else {
            continue;
        };
        let mut found: Vec<String> = match get_str(&card, "type").as_str() {
            "file" => vec![get_str(&card, "path")],
            "files" => arr(&card, "items")
                .iter()
                .map(|i| get_str(i, "path"))
                .collect(),
            _ => continue,
        };
        found.retain(|p| p.starts_with("artifacts/news/"));
        if !found.is_empty() {
            found.sort_by_key(|p| !p.ends_with(".pdf")); // the PDF first
            paths = found;
            break;
        }
    }
    if paths.is_empty() {
        return None;
    }
    Some(format!(
        "\n\n(System) This refers to the latest news report in this chat: send it with gmail_send to the user's own \
address (the first connected mailbox), subject the report's title, two or three lines of its highlights as the body, \
attachments = {paths:?}."
    ))
}

/// The last news report this run published (`news_report`'s result): `{title, html_url, html_path, pdf_url?, pdf_path?}`.
pub fn published_report(transcript: &[Value]) -> Option<Value> {
    let ids: Vec<String> = transcript
        .iter()
        .flat_map(tool_calls)
        .filter(|tc| stored_call(tc).0 == "news_report")
        .map(|tc| get_str(tc, "id"))
        .collect();
    transcript.iter().rev().find_map(|m| {
        if get_str(m, "role") != "tool" || !ids.contains(&get_str(m, "tool_call_id")) {
            return None;
        }
        let c = content_of(m);
        let v: Value = serde_json::from_str(c.lines().next().unwrap_or("")).ok()?;
        get_truthy(&v, "published").then_some(v)
    })
}

fn ran_ok(transcript: &[Value], tools: &[&str]) -> bool {
    let ids: Vec<String> = transcript
        .iter()
        .flat_map(tool_calls)
        .filter(|tc| tools.contains(&stored_call(tc).0.as_str()))
        .map(|tc| get_str(tc, "id"))
        .collect();
    transcript.iter().any(|m| {
        get_str(m, "role") == "tool"
            && ids.contains(&get_str(m, "tool_call_id"))
            && !content_of(m).starts_with("ERROR")
            && !content_of(m).starts_with("DENIED")
    })
}

static REPORT_EMAIL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(e-?mail|gmail|mail (it|them|each|every))\b|\bsend (it|them|each|every)[^.\n]{0,40}\bto me\b|邮件|邮箱|发到我的?邮")
        .unwrap()
});
static REPORT_FREQ: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(daily|weekly|monthly|fortnightly|every (day|week|month|morning|evening|weekday|monday|tuesday|wednesday|thursday|friday|saturday|sunday)|each (day|week|month|morning))\b|每天|每日|每周|每星期|每月|每个?(早上|星期)")
        .unwrap()
});

/// A news report published, and what the request asked for around it is not done yet (one-time hint): a scheduled
/// edition whose schedule asks to email it, with a mailbox connected and nothing sent → send it now; a request that
/// asks for the report regularly ("weekly", "every morning") with no schedule made → make it now.
pub fn news_report_hint(
    transcript: &[Value],
    goal: &str,
    scheduled: bool,
    mail_ready: bool,
    zh: bool,
) -> Option<String> {
    let rep = published_report(transcript)?;
    let mut atts = vec![get_str(&rep, "pdf_path"), get_str(&rep, "html_path")];
    atts.retain(|a| !a.is_empty());
    if scheduled
        && mail_ready
        && REPORT_EMAIL.is_match(goal)
        && !ran_ok(transcript, &["gmail_send"])
    {
        return Some(if zh {
            format!("（系统）这个定时任务要求把每一期报告发到用户邮箱：现在用 gmail_send 发给用户自己（第一个已连接的邮箱），主题「{}」，正文写两三句本期要点，attachments = {:?}；然后再回答。", get_str(&rep, "title"), atts)
        } else {
            format!("(System) This schedule asks for each edition by email: send it now with gmail_send to the user's own \
address (the first connected mailbox), subject \"{}\", two or three lines of this edition's highlights as the body, \
attachments = {:?}; then answer.", get_str(&rep, "title"), atts)
        });
    }
    if !scheduled
        && REPORT_FREQ.is_match(goal)
        && !ran_ok(transcript, &["schedule_create", "goal_create"])
    {
        let email = if REPORT_EMAIL.is_match(goal) {
            " and email it to me"
        } else {
            ""
        };
        return Some(if zh {
            format!("（系统）用户要定期收到这份报告：现在用 schedule_create 创建定时任务（kind=cron；每天 '0 8 * * *'，每周 '0 8 * * 1'，或用户说的时间），goal 写 \"News report on <主题>: research the latest news with the news-research skill and publish the newspaper report{email}\"；然后再回答。")
        } else {
            format!("(System) The user asked for this report regularly: create the schedule now with schedule_create \
(kind=cron; daily '0 8 * * *', weekly '0 8 * * 1', or the time they named), goal \"News report on <subject>: research \
the latest news with the news-research skill and publish the newspaper report{email}\"; then answer.")
        });
    }
    None
}

/// A run that published a news report: its two links are in the answer (added when the model left one out), and with a
/// mailbox connected and nothing sent, the answer offers to email the edition.
pub fn with_report_links(transcript: &[Value], answer: &str, mail_ready: bool, zh: bool) -> String {
    let Some(rep) = published_report(transcript) else {
        return answer.to_string();
    };
    let mut out = answer.trim_end().to_string();
    let (html, pdf) = (get_str(&rep, "html_url"), get_str(&rep, "pdf_url"));
    if (!html.is_empty() && !out.contains(&html)) || (!pdf.is_empty() && !out.contains(&pdf)) {
        let mut links = vec![format!("[HTML]({html})")];
        if !pdf.is_empty() {
            links.push(format!("[PDF]({pdf})"));
        }
        out.push_str(&format!(
            "\n\n📰 {} {}",
            if zh { "报告：" } else { "The report:" },
            links.join(" · ")
        ));
    }
    if mail_ready && !ran_ok(transcript, &["gmail_send"]) {
        out.push_str(if zh {
            "\n\n📧 想把这一期发到你的 Gmail 吗？回复「发邮件」，我会把 PDF 作为附件发给你。"
        } else {
            "\n\n📧 Want this edition in your Gmail? Reply **email it** and I'll send it to you with the PDF attached."
        });
    }
    out
}

/// An entry-requirements answer that mentions a visa exemption without its length gets one hint to state the number
/// of days for the user's passport, read from an official source (travel_info_real 2026-11-01: Thailand's exemption
/// with "verify the visa length", no days). The days come from the page read, never from memory.
pub fn visa_days_hint(transcript: &[Value], answer: &str, zh: bool) -> Option<String> {
    static VISA: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)visa[- ]?(free|exempt|exemption|waiver)|免签|免簽").unwrap()
    });
    static DAYS: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?i)\b\d{1,3}[- ]?(days?|nights?)\b|\d{1,3}\s?天").unwrap());
    let loaded = transcript.iter().flat_map(tool_calls).any(|tc| {
        let (name, args) = stored_call(tc);
        name == "load_skill" && sget(&args, "name") == "entry-requirements"
    });
    if !loaded || !VISA.is_match(answer) {
        return None;
    }
    // a day count within the sentence that names the exemption: fine
    let stated = VISA.find_iter(answer).all(|m| {
        let from = answer[..m.start()].rfind(['.', '\n']).map_or(0, |i| i + 1);
        let to = answer[m.end()..]
            .find(['.', '\n'])
            .map_or(answer.len(), |i| m.end() + i);
        DAYS.is_match(&answer[from..to])
    });
    if stated {
        return None;
    }
    Some(if zh {
        "（系统）回答提到了免签但没有说能停留几天：按用户的护照，从官方来源（该国外交部或移民局的页面，先 browser_read）查到免签停留天数；来源说法不一时以官方页面为准并注明日期。然后把完整的回答重写一遍（原来的每一项，包括入境卡、链接和提醒），加上天数。".to_string()
    } else {
        "(System) The answer mentions a visa exemption but not how long it allows: for the user's passport, find the \
visa-free stay in days on an official source (the country's foreign ministry or immigration page — browser_read it); \
when sources disagree, give the official one with its date. Then write the WHOLE answer again — every part of it \
(each country, the arrival cards with their links, the reminders) — with the days added.".to_string()
    })
}

/// Whether an answer asks the user for something: a question, or request wording without one ("I still need your
/// flight number…", "please send…", "you can upload…") — arrival_real 2026-10-31: the flight and the hotel asked for
/// with no question mark, and the classifier saw no question.
pub fn requests_info(answer: &str) -> bool {
    static ASK: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)[?？]|\bI (still |also )?need (your|the|to know)\b|\bplease (send|tell|provide|share|give|confirm|upload|let me know)\b|\b(could|can) you (send|tell|provide|share|give|confirm|upload)\b|\byou can (send|upload|share|tell)\b|\blet me know\b|请(告诉|提供|发送|上传|确认)|还需要你")
            .unwrap()
    });
    ASK.is_match(answer)
}

/// An arrival-card answer that asks the user for the trip, with no mailbox connected, says that connecting one lets
/// OMuse read the flight and the stay from the booking emails (a fact of the connections, arrival_real 2026-10-31:
/// "nomail" asked for the flight and the hotel without a word of the mailbox).
pub fn with_mailbox_hint(
    transcript: &[Value],
    answer: &str,
    mail_ready: bool,
    asks: bool,
    zh: bool,
) -> String {
    if mail_ready || !asks || !arrival_run(transcript) {
        return answer.to_string();
    }
    let line = if zh {
        "（在「连接」页连接邮箱后，我可以直接从预订邮件里读取航班和住宿。）"
    } else {
        "(Connect your mailbox in Connections and I can read the flight and the stay from your booking emails myself.)"
    };
    format!("{}\n\n{line}", answer.trim_end())
}

/// The reason the runtime's own hand-over gives the user: an approved purchase is an order to place; anything else is
/// a form to review and submit (shopping_real 2026-10-31: the checkout was handed over as "the form is filled in").
pub fn handover_reason(transcript: &[Value], generic: &str) -> String {
    let calls: Vec<(String, String)> = transcript
        .iter()
        .flat_map(tool_calls)
        .map(|tc| (get_str(tc, "id"), stored_call(tc).0))
        .collect();
    if purchase_approved(transcript, &calls) {
        "The checkout is filled in with the order you approved. Review it and place the order yourself — the final \
place-order / pay click is yours; hand the browser back when it is done."
            .to_string()
    } else {
        generic.to_string()
    }
}

/// Whether the user approved a purchase in this run (`purchase_confirm` came back approved): the order is then placed
/// by the user, so a browser run that ends without a hand-over hands the checkout over (a purchase made with no page
/// open — a connector, a booking API — has nothing to hand over) (shopping_real 2026-10-31: the card
/// and address filled, "I did not press Place your order", nothing handed over).
fn purchase_approved(transcript: &[Value], calls: &[(String, String)]) -> bool {
    transcript.iter().any(|m| {
        get_str(m, "role") == "tool"
            && calls.iter().any(|(id, n)| {
                *id == get_str(m, "tool_call_id")
                    && n.rsplit("__").next() == Some("purchase_confirm")
            })
            && content_of(m).contains("\"status\": \"approved\"")
    })
}

/// `answer` for the Hong Kong departure-tax refund always ending with the reference code and a link to that application
/// for the user's own face recognition: when the run (hk-apdt-refund) opened an app.apdtrefund.hk page carrying a
/// reference (`bizId=`) and the answer lacks the code or a link to the site, the step is added — the application's own
/// page (stopped before "Proceed with Facial Recognition") and the site's "Resume Application" with the code
/// (apdt_real 2026-10-31: "submitted successfully", no code, no link; the user's rule: the reference before any face
/// recognition, and a link so they complete it themselves).
pub fn with_apdt_link(transcript: &[Value], answer: &str, zh: bool) -> String {
    static URL: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r#"https://app\.apdtrefund\.hk/hkTaxMobile/[^\s"'<>\\]*[?&]bizId=([A-Za-z0-9]{4,12})[^\s"'<>\\]*"#).unwrap()
    });
    const HOME: &str = "https://app.apdtrefund.hk/hkTaxMobile/?lang=en";
    let apdt = transcript.iter().flat_map(tool_calls).any(|tc| {
        let (name, args) = stored_call(tc);
        name == "load_skill" && sget(&args, "name") == "hk-apdt-refund"
    });
    if !apdt {
        return answer.to_string();
    }
    // the newest application page with its reference; the submit page (before the face step) when there is one
    let mut found: Option<(String, String)> = None;
    for m in transcript.iter().filter(|m| get_str(m, "role") == "tool") {
        // results hold their links as stand-ins (crate::links): read them as the pages' own addresses
        let c = crate::links::decode(&content_of(m));
        for cap in URL.captures_iter(&c) {
            let (url, code) = (cap[0].to_string(), cap[1].to_string());
            let better = found
                .as_ref()
                .is_none_or(|(u, _)| !u.contains("apdt_submit") || url.contains("apdt_submit"));
            if better {
                found = Some((url, code));
            }
        }
    }
    let Some((url, code)) = found else {
        return answer.to_string();
    };
    // a fact code owns: always written from the page, even when the answer already gives the reference
    // (apdt_real 2026-10-31: "continue with reference EEL7GH" with no link and no word of the phone)
    let line = if zh {
        format!(
            "**人脸识别——由你自己完成：** 参考编号 **{code}**。在手机上打开[你的退税申请 {code}]({url})，点「Proceed with Facial Recognition」，\
用手机前置摄像头完成。如果要求重新登录，在 {HOME} 选「Resume Application」，输入旅行证件号码和参考编号 {code}。"
        )
    } else {
        format!(
            "**Face recognition — your step:** reference number **{code}**. Open [your refund application {code}]({url}) \
on your phone and press \"Proceed with Facial Recognition\" (the phone's front camera). If it asks you to sign in again, \
choose \"Resume Application\" at {HOME} with your travel document number and the reference {code}."
        )
    };
    format!("{}\n\n{line}", answer.trim_end())
}

/// `answer` giving a delayed flight's original time next to the new one: when a page the run read shows "Scheduled
/// HH:MM … Estimated HH:MM" with different times (an airline's status page) and the answer gives the estimated time but
/// not the scheduled one, "(Scheduled departure 09:25, now estimated 17:10.)" is added (claims_real 2026-10-31).
pub fn with_flight_times(transcript: &[Value], answer: &str, zh: bool) -> String {
    static SCHED: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?i)\bscheduled\b[^0-9\n]{0,20}(\d{1,2}:\d{2})").unwrap());
    static EST: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)\b(estimated|expected|revised|new time)\b[^0-9\n]{0,20}(\d{1,2}:\d{2})")
            .unwrap()
    });
    for m in transcript
        .iter()
        .rev()
        .filter(|m| get_str(m, "role") == "tool")
    {
        let text = content_of(m).replace("\\n", "\n");
        let (Some(s), Some(e)) = (SCHED.captures(&text), EST.captures(&text)) else {
            continue;
        };
        let (old, new) = (s[1].to_string(), e[2].to_string());
        if old == new || !answer.contains(&new) || answer.contains(&old) {
            return answer.to_string();
        }
        let line = if zh {
            format!("（原定起飞 {old}，现在预计 {new}。）")
        } else {
            format!("(Scheduled departure {old}, now estimated {new}.)")
        };
        return format!("{}\n\n{line}", answer.trim_end());
    }
    answer.to_string()
}

/// The file formats the user asked for (PDF, Excel, Word) that no file of the run has: a one-time note sends the run
/// back to make them (travel_receipts_real 2026-10-31: "give me the Excel and the PDF" got the Excel only). The formats
/// come from the user's words, the files from the run's successful make_* / files_write calls.
pub fn missing_format_hint(transcript: &[Value], goal: &str, zh: bool) -> Option<String> {
    static PDF: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)\bpdf\b").unwrap());
    static XLSX: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?i)\b(excel|xlsx|spreadsheet)\b|表格").unwrap());
    static DOCX: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?i)\b(docx|word document|word file)\b").unwrap());
    let ok_ids: std::collections::HashSet<String> = transcript
        .iter()
        .filter(|m| get_str(m, "role") == "tool" && !content_of(m).starts_with("ERROR"))
        .map(|m| get_str(m, "tool_call_id"))
        .collect();
    let made: Vec<String> = transcript
        .iter()
        .flat_map(tool_calls)
        .filter(|tc| ok_ids.contains(&get_str(tc, "id")))
        .filter_map(|tc| {
            let (n, a) = stored_call(tc);
            match n.as_str() {
                "make_pdf" | "make_xlsx" | "make_docx" | "files_write" => {
                    Some(format!("{} {} {n}", sget(&a, "output"), sget(&a, "path")).to_lowercase())
                }
                _ => None,
            }
        })
        .collect();
    let has = |ext: &str, tool: &str| made.iter().any(|m| m.contains(ext) || m.ends_with(tool));
    let mut missing: Vec<&str> = Vec::new();
    if PDF.is_match(goal) && !has(".pdf", "make_pdf") {
        missing.push("PDF");
    }
    if XLSX.is_match(goal) && !has(".xlsx", "make_xlsx") {
        missing.push("Excel");
    }
    if DOCX.is_match(goal) && !has(".docx", "make_docx") {
        missing.push("Word");
    }
    if missing.is_empty() || made.is_empty() {
        return None;
    }
    let what = missing.join(" and ");
    Some(if zh {
        format!("（系统）用户要了 {what} 文件，还没有生成：现在生成（make_pdf / make_xlsx / make_docx，用同一份内容），再用 send_file 发给用户。")
    } else {
        format!("(System) The user asked for the {what} too and it was not made: make it now from the same content (make_pdf / \
make_xlsx / make_docx), then send it with send_file.")
    })
}

/// The files the run wrote (`files_write`, `make_pdf`, `make_xlsx`, `make_docx`, without an error) that never reached the
/// chat (`send_file`), when the request asks for a file: the runtime sends them before the answer (chat_real
/// 2026-10-31: "as a Markdown file I can download" — written, never sent, "Download the packing list" led nowhere).
pub fn unsent_files(transcript: &[Value], goal: &str) -> Vec<String> {
    static WANTS_FILE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)\b(file|download|attachment|pdf|excel|xlsx|spreadsheet|docx|word document|markdown|csv)\b|\.md\b|文件|下载|附件|表格").unwrap()
    });
    if !WANTS_FILE.is_match(goal) {
        return Vec::new();
    }
    let calls: Vec<(String, String, Value)> = transcript
        .iter()
        .flat_map(tool_calls)
        .map(|tc| {
            let (n, a) = stored_call(tc);
            (get_str(tc, "id"), n, a)
        })
        .collect();
    let ok = |id: &str| {
        transcript.iter().any(|m| {
            get_str(m, "role") == "tool"
                && get_str(m, "tool_call_id") == id
                && !content_of(m).starts_with("ERROR")
        })
    };
    let sent: Vec<String> = calls
        .iter()
        .filter(|(_, n, _)| n == "send_file")
        .map(|(_, _, a)| sget(a, "path"))
        .collect();
    let mut out: Vec<String> = Vec::new();
    for (id, n, a) in &calls {
        let path = match n.as_str() {
            "files_write" => sget(a, "path"),
            "make_pdf" | "make_xlsx" | "make_docx" => {
                let o = sget(a, "output");
                if o.is_empty() {
                    sget(a, "path")
                } else {
                    o
                }
            }
            _ => continue,
        };
        if path.is_empty() || !ok(id) || sent.iter().any(|s| s == &path) || out.contains(&path) {
            continue;
        }
        out.push(path);
    }
    out
}

/// ETIAS's status, as the eu-etias skill states it (checked 2026-10-30): not in operation, launch expected in 2027.
pub const ETIAS_STATUS_EN: &str =
    "ETIAS is not in operation yet (launch expected in 2027), so there is nothing to apply \
for now; check https://travel-europe.europa.eu/etias before you travel.";
const ETIAS_STATUS_ZH: &str = "ETIAS 目前还没有上线（预计 2027 年启用），现在不需要也无法申请；出发前请看 https://travel-europe.europa.eu/etias。";

/// `answer` carrying ETIAS's status when the eu-etias skill ran and the answer does not say it is not in operation —
/// the skill's central fact, stated in code (arrival_cards4_real 2026-10-31: an answer about the visa only).
pub fn with_etias_status(transcript: &[Value], answer: &str, zh: bool) -> String {
    let etias = transcript.iter().flat_map(tool_calls).any(|tc| {
        let (name, args) = stored_call(tc);
        name == "load_skill" && sget(&args, "name") == "eu-etias"
    });
    if !etias {
        return answer.to_string();
    }
    format!(
        "{}\n\n{}",
        answer.trim_end(),
        if zh { ETIAS_STATUS_ZH } else { ETIAS_STATUS_EN }
    )
}

/// `answer` telling the user when content the run read (a page, an email, a file) carried instructions aimed at the
/// assistant (`injection_warning` on its untrusted wrapper) — the security rule's "tell the user" done in code, unless
/// the answer already says so.
pub fn with_injection_notice(transcript: &[Value], answer: &str, zh: bool) -> String {
    let flagged: Vec<String> = transcript
        .iter()
        .filter(|m| get_str(m, "role") == "tool")
        .filter_map(|m| {
            let c = content_of(m);
            let i = c.find("injection_warning=")?;
            let src = c[..i]
                .rsplit("source=\"")
                .next()?
                .split('"')
                .next()?
                .to_string();
            Some(src)
        })
        .collect();
    if flagged.is_empty() {
        return answer.to_string();
    }
    let mut srcs: Vec<String> = Vec::new();
    for f in flagged {
        if !srcs.contains(&f) {
            srcs.push(f);
        }
    }
    let list = srcs.into_iter().take(3).collect::<Vec<_>>().join("; ");
    let line = if zh {
        format!("（注意：{list} 里有针对助手的指令，已忽略，没有照做。）")
    } else {
        format!("(Note: {list} contained instructions aimed at the assistant; they were ignored.)")
    };
    format!("{}\n\n{line}", answer.trim_end())
}

/// `answer` with every full card number masked to its last four digits ("•••• 1111"): a 13–19 digit run (spaces or
/// dashes between groups) that passes the Luhn check never reaches the user's chat, wherever the model read it from
/// (a photo of a card, a page) — the secrets rule in code.
pub fn with_masked_cards(answer: &str) -> String {
    static RUN: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"\b\d(?:[ -]?\d){12,18}\b").unwrap());
    let luhn = |digits: &[u32]| {
        let sum: u32 = digits
            .iter()
            .rev()
            .enumerate()
            .map(|(i, d)| {
                if i % 2 == 1 {
                    let x = d * 2;
                    if x > 9 {
                        x - 9
                    } else {
                        x
                    }
                } else {
                    *d
                }
            })
            .sum();
        sum.is_multiple_of(10)
    };
    RUN.replace_all(answer, |c: &regex::Captures| {
        let digits: Vec<u32> = c[0].chars().filter_map(|ch| ch.to_digit(10)).collect();
        // a card network's first digit (3 Amex / JCB, 4 Visa, 5 Mastercard, 6 Discover / UnionPay) and the Luhn check
        if (13..=19).contains(&digits.len()) && (3..=6).contains(&digits[0]) && luhn(&digits) {
            let last: String = digits[digits.len() - 4..]
                .iter()
                .map(|d| char::from_digit(*d, 10).unwrap_or('0'))
                .collect();
            format!("•••• {last}")
        } else {
            c[0].to_string()
        }
    })
    .into_owned()
}

/// The id prefix of the hand-over the runtime makes itself (`Runtime::run`, `needs_handover`).
pub const AUTO_HANDOVER: &str = "auto_handover_";

/// `answer` opened by the answer the agent gave before the runtime handed its form over (carried on the hand-over call),
/// when the answer after the hand-back does not already contain it (cards_claim_real 2026-10-31: the benefit, amount
/// and deadlines were in the answer before the hand-over, and the user only got "the browser was handed back").
pub fn with_handover_answer(transcript: &[Value], answer: &str) -> String {
    let Some(before) = transcript.iter().rev().find_map(|m| {
        tool_calls(m)
            .iter()
            .any(|tc| get_str(tc, "id").starts_with(AUTO_HANDOVER))
            .then(|| content_of(m).trim().to_string())
    }) else {
        return answer.to_string();
    };
    if before.is_empty() || answer.contains(take_chars(&before, 120)) {
        return answer.to_string();
    }
    format!("{before}\n\n{}", answer.trim())
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

static COMPARE_URL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"compare_url\\?"\s*:\s*\\?"(https?://[^"\\\s]+)"#).unwrap());

/// The objects of a tool result's JSON (inside its `<untrusted_content>` wrapper) that have both `field`s.
fn result_objects(content: &str, a: &str, b: &str, out: &mut Vec<(String, String)>) {
    fn walk(v: &Value, a: &str, b: &str, out: &mut Vec<(String, String)>) {
        match v {
            Value::Object(m) => {
                if let (Some(x), Some(y)) = (
                    m.get(a).and_then(Value::as_str),
                    m.get(b).and_then(Value::as_str),
                ) {
                    out.push((x.to_string(), y.to_string()));
                }
                m.values().for_each(|x| walk(x, a, b, out));
            }
            Value::Array(xs) => xs.iter().for_each(|x| walk(x, a, b, out)),
            // an MCP tool's result: its JSON inside a text string (travel_nomail_real 2026-10-31)
            Value::String(t) if t.trim_start().starts_with('{') => {
                if let Ok(v) = serde_json::from_str::<Value>(t) {
                    walk(&v, a, b, out);
                }
            }
            _ => {}
        }
    }
    let (Some(i), Some(j)) = (content.find('{'), content.rfind('}')) else {
        return;
    };
    if let Ok(v) = serde_json::from_str::<Value>(&content[i..=j]) {
        walk(&v, a, b, out);
    }
}

/// The answer with each hotel it names linked to that hotel's booking page: a hotel search result gives every hotel a
/// `booking_url`, and a name the answer shows without its link gets it (2026-10-30: a real model listed four hotels and
/// linked one).
pub fn with_hotel_links(transcript: &[Value], answer: &str) -> String {
    let mut named: Vec<(String, String)> = Vec::new();
    for m in transcript.iter().filter(|m| get_str(m, "role") == "tool") {
        let c = content_of(m);
        if c.contains("\"hotels\"") || c.contains("\\\"hotels\\\"") {
            result_objects(&c, "name", "booking_url", &mut named);
        }
    }
    let mut out = answer.to_string();
    for (name, url) in named {
        let name = name.trim();
        if name.chars().count() < 4
            || out.contains(url.as_str())
            || out.contains(&format!("[{name}]"))
        {
            continue;
        }
        // named in any case ("the square hotel GINZA" for "The Square Hotel Ginza", travel_user_real 2026-10-31): the
        // answer's own words become the link
        let (low_out, low_name) = (out.to_lowercase(), name.to_lowercase());
        let at = out.find(name).or_else(|| {
            (low_out.len() == out.len() && low_name.len() == name.len())
                .then(|| low_out.find(&low_name))
                .flatten()
        });
        if let Some(i) = at {
            let shown = out[i..i + name.len()].to_string();
            if out[..i].ends_with('[') {
                continue;
            }
            out.replace_range(i..i + name.len(), &format!("[{shown}]({url})"));
        }
    }
    out
}

/// The answer with each flight it names linked to that offer's booking page: a flight search result gives every offer a
/// `flight_no` and a `booking_url`, and the first mention of a flight number without its link gets it — so a later
/// "book it" has the page (travel_flight_real 2026-10-31: "Scoot TR 765" named, the agency's page lost, and the next
/// message could not book it).
pub fn with_flight_links(transcript: &[Value], answer: &str) -> String {
    let mut offers: Vec<(String, String)> = Vec::new();
    for m in transcript.iter().filter(|m| get_str(m, "role") == "tool") {
        let c = content_of(m);
        if c.contains("flight_no") && c.contains("booking_url") {
            result_objects(&c, "flight_no", "booking_url", &mut offers);
        }
    }
    let mut out = answer.to_string();
    for (no, url) in offers {
        let no = no.trim();
        if no.chars().count() < 3 || out.contains(url.as_str()) {
            continue;
        }
        // "TR 765" or "TR765" as the answer writes it, not already inside a link's text
        let tight = no.replace(' ', "");
        let at = out
            .find(no)
            .map(|i| (i, no.len()))
            .or_else(|| out.find(&tight).map(|i| (i, tight.len())));
        if let Some((i, n)) = at {
            if out[..i].ends_with('[')
                || out[..i]
                    .rfind('[')
                    .is_some_and(|b| out[b..i].find(']').is_none())
            {
                continue;
            }
            let shown = out[i..i + n].to_string();
            out.replace_range(i..i + n, &format!("[{shown}]({url})"));
        }
    }
    out
}

/// The answer with the pages a hotel search's web fallback read, when it links none of them: the hotels it names come
/// from those pages (no booking_url of their own), and the user is given where to see and book them
/// (travel_brave_real 2026-10-31: two Ginza hotels from a booking site's page, only the Kayak link given).
const FALLBACK_PAGES_SHOWN: usize = 2;

pub fn with_fallback_pages(transcript: &[Value], answer: &str, zh: bool) -> String {
    fn find(v: &Value, out: &mut Vec<(String, String)>) {
        match v {
            Value::Object(m) => {
                if m.contains_key("hotels_fallback") {
                    for p in m
                        .get("pages")
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten()
                    {
                        let (u, t) = (get_str(p, "url"), get_str(p, "title"));
                        if u.starts_with("http") {
                            out.push((u, t));
                        }
                    }
                }
                m.values().for_each(|x| find(x, out));
            }
            Value::Array(xs) => xs.iter().for_each(|x| find(x, out)),
            Value::String(t) if t.trim_start().starts_with('{') => {
                if let Ok(v) = serde_json::from_str::<Value>(t) {
                    find(&v, out);
                }
            }
            _ => {}
        }
    }
    // a long result is cut (no longer whole JSON): the page addresses after its `pages` key, read as text
    static PAGE_URL: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r#"\\?"url\\?"\s*:\s*\\?"(https?://[^"\\\s]+)"#).unwrap());
    let mut pages: Vec<(String, String)> = Vec::new();
    for m in transcript.iter().filter(|m| get_str(m, "role") == "tool") {
        let c = content_of(m);
        if !c.contains("hotels_fallback") {
            continue;
        }
        let before = pages.len();
        if let (Some(i), Some(j)) = (c.find('{'), c.rfind('}')) {
            if let Ok(v) = serde_json::from_str::<Value>(&c[i..=j]) {
                find(&v, &mut pages);
            }
        }
        if pages.len() == before {
            if let Some(at) = c.find("pages\\\"").or_else(|| c.find("\"pages\"")) {
                pages.extend(
                    PAGE_URL
                        .captures_iter(&c[at..])
                        .take(FALLBACK_PAGES_SHOWN)
                        .map(|cap| (cap[1].to_string(), String::new())),
                );
            }
        }
    }
    // the fallback read none (or the run read more itself): the pages the run's browser_read opened after it
    let fell_back = transcript
        .iter()
        .filter(|m| get_str(m, "role") == "tool")
        .any(|m| content_of(m).contains("hotels_fallback"));
    if fell_back && pages.is_empty() {
        for tc in transcript.iter().flat_map(tool_calls) {
            let (name, args) = stored_call(tc);
            if name == "browser_read" {
                for u in arr(&args, "urls").iter().filter_map(Value::as_str) {
                    if u.starts_with("http") && pages.len() < FALLBACK_PAGES_SHOWN {
                        pages.push((u.to_string(), String::new()));
                    }
                }
            }
        }
    }
    pages.dedup();
    if pages.is_empty() || pages.iter().any(|(u, _)| answer.contains(u.as_str())) {
        return answer.to_string();
    }
    let links = pages
        .iter()
        .map(|(u, t)| {
            let host = url::Url::parse(u)
                .ok()
                .and_then(|x| x.host_str().map(str::to_string))
                .unwrap_or_else(|| u.clone());
            let t = if t.trim().is_empty() {
                host.as_str()
            } else {
                t.trim()
            };
            format!("[{}]({u})", t.replace(['[', ']'], ""))
        })
        .collect::<Vec<_>>()
        .join(" · ");
    let label = if zh {
        "酒店信息来自这些页面"
    } else {
        "The hotels above are from these pages"
    };
    format!("{}\n\n{label}: {links}", answer.trim_end())
}

/// The answer with the comparison it showed as cards, when it gives none of the cards' prices: the options with their
/// details (as checked against the pages), so the answer stands on its own — in Telegram, a notification, a later
/// "book the second one" (travel_brave_real 2026-10-31: "you can choose from the cards shown", no price in the answer).
pub fn with_choices_listing(transcript: &[Value], answer: &str, zh: bool) -> String {
    static PRICE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?:S\$|US\$|A\$|HK\$|SGD|USD|JPY|EUR|GBP|¥|€|£|\$)\s?\d").unwrap()
    });
    let Some(listing) = transcript
        .iter()
        .rev()
        .filter(|m| get_str(m, "role") == "tool")
        .map(content_of)
        .find(|c| c.contains("Shown to the user as") && c.contains("verified against the pages"))
    else {
        return answer.to_string();
    };
    let lines: Vec<&str> = listing
        .lines()
        .filter(|l| {
            l.split_once(". ")
                .is_some_and(|(n, _)| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()))
        })
        .collect();
    if lines.is_empty() || !lines.iter().any(|l| PRICE.is_match(l)) || PRICE.is_match(answer) {
        return answer.to_string();
    }
    let head = if zh { "选项" } else { "The options" };
    format!("{}\n\n{head}:\n{}", answer.trim_end(), lines.join("\n"))
}

/// The day "today" / "tomorrow" in the user's request means, in the user's own time zone: a phone call's purpose that
/// names one other date gets this one instead (phone_real 2026-11-01: "tomorrow, Sunday, November 1" when tomorrow
/// was Monday 2 November for the user, and a second call to put it right). `None`: nothing to fix.
pub fn fix_relative_day(goal: &str, purpose: &str, today: chrono::NaiveDate) -> Option<String> {
    use chrono::{Datelike, NaiveDate};
    static REL: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?i)\b(tomorrow|today|tonight)\b|明天|今天|今晚").unwrap());
    static DATE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)(?:\b(?:mon|tues|wednes|thurs|fri|satur|sun)day,?\s+)?\b(?:(january|february|march|april|may|june|july|august|september|october|november|december)\s+(\d{1,2})|(\d{1,2})\s+(january|february|march|april|may|june|july|august|september|october|november|december))\b(?:,?\s+(\d{4}))?")
            .unwrap()
    });
    let rel = REL.find(goal)?.as_str().to_lowercase();
    let want = if rel == "tomorrow" || rel == "明天" {
        today.succ_opt()?
    } else {
        today
    };
    let months = [
        "january",
        "february",
        "march",
        "april",
        "may",
        "june",
        "july",
        "august",
        "september",
        "october",
        "november",
        "december",
    ];
    let mut found: Vec<(std::ops::Range<usize>, NaiveDate)> = Vec::new();
    for c in DATE.captures_iter(purpose) {
        let (mon, day) = match (c.get(1), c.get(2), c.get(3), c.get(4)) {
            (Some(m), Some(d), _, _) | (_, _, Some(d), Some(m)) => {
                (m.as_str().to_lowercase(), d.as_str())
            }
            _ => continue,
        };
        let m = months.iter().position(|x| *x == mon)? as u32 + 1;
        let y = c
            .get(5)
            .and_then(|y| y.as_str().parse().ok())
            .unwrap_or(want.year());
        let d = NaiveDate::from_ymd_opt(y, m, day.parse().ok()?)?;
        found.push((c.get(0)?.range(), d));
    }
    // one date named, and it is not the day the user meant
    let first = found.first()?.1;
    if found.iter().any(|(_, d)| *d != first) || first == want {
        return None;
    }
    let mut out = purpose.to_string();
    for (r, _) in found.iter().rev() {
        out.replace_range(r.clone(), &want.format("%A, %B %-d, %Y").to_string());
    }
    Some(out)
}

/// A flight answer gives the day it is for: the date of the run's flight search, when the answer does not show it in
/// any usual writing, goes in front (travel_kiwi_real 2026-11-01: "The cheapest result is SGD 304 …", no day).
pub fn with_flight_date(transcript: &[Value], answer: &str, zh: bool) -> String {
    let date = transcript
        .iter()
        .flat_map(tool_calls)
        .filter_map(|tc| {
            let (name, args) = stored_call(tc);
            (name == "flights_search").then(|| sget(&args, "date"))
        })
        .filter_map(|d| chrono::NaiveDate::parse_from_str(d.trim(), "%Y-%m-%d").ok())
        .next_back();
    let Some(d) = date else {
        return answer.to_string();
    };
    let forms = [
        d.format("%Y-%m-%d").to_string(),
        d.format("%-d %B").to_string(),
        d.format("%-d %b").to_string(),
        d.format("%B %-d").to_string(),
        d.format("%b %-d").to_string(),
        d.format("%-m月%-d日").to_string(),
        d.format("%-d/%-m").to_string(),
    ];
    if forms.iter().any(|f| answer.contains(f.as_str())) || answer.trim().is_empty() {
        return answer.to_string();
    }
    if zh {
        format!(
            "**{}**\n\n{}",
            d.format("%Y年%-m月%-d日"),
            answer.trim_start()
        )
    } else {
        format!(
            "**{}**\n\n{}",
            d.format("%A, %-d %B %Y"),
            answer.trim_start()
        )
    }
}

/// The answer with the times of each flight leg it names without them: a flight search result gives every leg its
/// `flight_number`, `from` / `depart` and `to` / `arrive`; a leg named whose departure time the answer does not show
/// gets a line with both times (travel_brave_real 2026-10-31: "VJ814 … arriving 1:00 a.m.", no departure time).
pub fn with_leg_times(transcript: &[Value], answer: &str, zh: bool) -> String {
    fn walk(v: &Value, out: &mut Vec<Value>) {
        match v {
            Value::Object(m) => {
                if m.contains_key("flight_number")
                    && m.contains_key("depart")
                    && m.contains_key("arrive")
                {
                    out.push(v.clone());
                }
                m.values().for_each(|x| walk(x, out));
            }
            Value::Array(xs) => xs.iter().for_each(|x| walk(x, out)),
            Value::String(t) if t.trim_start().starts_with('{') => {
                if let Ok(v) = serde_json::from_str::<Value>(t) {
                    walk(&v, out);
                }
            }
            _ => {}
        }
    }
    let mut legs: Vec<Value> = Vec::new();
    for m in transcript.iter().filter(|m| get_str(m, "role") == "tool") {
        let c = content_of(m);
        if !c.contains("flight_number") {
            continue;
        }
        if let (Some(i), Some(j)) = (c.find('{'), c.rfind('}')) {
            if let Ok(v) = serde_json::from_str::<Value>(&c[i..=j]) {
                walk(&v, &mut legs);
            }
        }
    }
    // "2026-11-30 11:15:00" → "11:15", and "1:00" for "01:00" (12-hour writing)
    let hm = |t: &str| -> Option<(String, String)> {
        let time = t.split_whitespace().nth(1)?;
        let hm: String = time.chars().take(5).collect();
        let short = hm.strip_prefix('0').unwrap_or(&hm).to_string();
        let h: u32 = hm.get(..2)?.parse().ok()?;
        let twelve = format!(
            "{}:{}",
            if h.is_multiple_of(12) { 12 } else { h % 12 },
            hm.get(3..5)?
        );
        Some((hm, if h >= 12 || h == 0 { twelve } else { short }))
    };
    let mut lines: Vec<String> = Vec::new();
    for l in &legs {
        let no = get_str(l, "flight_number");
        if no.chars().count() < 3 || !answer.replace(' ', "").contains(&no.replace(' ', "")) {
            continue;
        }
        let (dep, arr_t) = (get_str(l, "depart"), get_str(l, "arrive"));
        let Some((d24, d12)) = hm(&dep) else { continue };
        if answer.contains(&d24) || answer.contains(&d12) {
            continue;
        }
        let line = format!(
            "{no}: {} {} → {} {}",
            get_str(l, "from"),
            dep.get(..16).unwrap_or(&dep),
            get_str(l, "to"),
            arr_t.get(..16).unwrap_or(&arr_t)
        );
        if !lines.contains(&line) {
            lines.push(line);
        }
    }
    if lines.is_empty() {
        return answer.to_string();
    }
    let head = if zh {
        "航班时间（当地时间）"
    } else {
        "Flight times (local)"
    };
    format!("{}\n\n{head}: {}.", answer.trim_end(), lines.join("; "))
}

/// The answer with the Kayak compare link of every flight or hotel search in the task that it left out: each search
/// result carries one (`compare_url`), and the user is always given it — added in code, not left to the model.
pub fn with_compare_links(transcript: &[Value], answer: &str, zh: bool) -> String {
    let mut urls: Vec<String> = Vec::new();
    for m in transcript.iter().filter(|m| get_str(m, "role") == "tool") {
        for c in COMPARE_URL.captures_iter(&content_of(m)) {
            let u = c[1].to_string();
            if !urls.contains(&u) {
                urls.push(u);
            }
        }
    }
    let missing: Vec<&String> = urls
        .iter()
        .filter(|u| !answer.contains(u.as_str()))
        .collect();
    if missing.is_empty() || urls.iter().any(|u| answer.contains(u.as_str())) {
        return answer.to_string();
    }
    let label = if zh {
        "在 Kayak 上比价"
    } else {
        "Compare prices on Kayak"
    };
    let links = missing
        .iter()
        .map(|u| format!("[{label}]({u})"))
        .collect::<Vec<_>>()
        .join(" · ");
    format!("{}\n\n{links}", answer.trim_end())
}

static CATALOG_AS_OF: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"# Credit cards — .*catalog as of (\d{4})-(\d{2})\)").unwrap());

/// `answer` with the date of the card terms it used: when the run read a credit-card-picker catalog table (`# Credit
/// cards — US (…; catalog as of 2026-08)`) and the answer gives neither that month nor a full date in it, a line
/// with the month is added — the user must always see how old the terms behind a pick are (cards_real 2026-10-30:
/// the right pick, no date).
pub fn with_catalog_date(transcript: &[Value], answer: &str, zh: bool) -> String {
    let Some((y, m)) = transcript
        .iter()
        .filter(|t| get_str(t, "role") == "tool")
        .find_map(|t| {
            let text = content_of(t);
            let c = CATALOG_AS_OF.captures(&text)?;
            Some((c[1].parse::<i32>().ok()?, c[2].parse::<u32>().ok()?))
        })
    else {
        return answer.to_string();
    };
    let Some(d) = chrono::NaiveDate::from_ymd_opt(y, m, 1) else {
        return answer.to_string();
    };
    let given = [
        format!("{y}-{m:02}"),
        format!("{y}/{m:02}"),
        format!("{y}年{m}月"),
        d.format("%B %Y").to_string(),
        d.format("%b %Y").to_string(),
        d.format("%b. %Y").to_string(),
    ];
    let low = answer.to_lowercase();
    if given.iter().any(|g| low.contains(&g.to_lowercase())) {
        return answer.to_string();
    }
    let line = if zh {
        format!("（信用卡权益数据截至 {y} 年 {m} 月；条款可能变化。）")
    } else {
        format!(
            "(Card terms from the catalog as of {}; terms change.)",
            d.format("%B %Y")
        )
    };
    format!("{}\n\n{line}", answer.trim_end())
}

/// `answer` saying which of the cards it names are only unverified seeds in the shipped catalog (`verified: false`): "(The catalog's terms for DBS Altitude are unverified seeds; check them on the issuer's
/// page.)" — unless the answer already says so (cards_markets_real 2026-10-31: Singapore picks with seed rates, nothing
/// said).
pub fn with_unverified_rows(transcript: &[Value], answer: &str, zh: bool) -> String {
    // a card answer (the run read a card table) naming catalog cards that are only seeds: judged against the shipped
    // catalog's `verified`, not the table rows this run happened to read (the table can sit in another task's turn)
    let card_run = transcript
        .iter()
        .any(|m| get_str(m, "role") == "tool" && content_of(m).contains("# Credit cards — "));
    if !card_run {
        return answer.to_string();
    }
    let low = answer.to_lowercase();
    let mut named: Vec<String> = Vec::new();
    for c in catalog_cards() {
        if c.get("verified").is_some_and(|v| v.as_bool() == Some(true)) {
            continue;
        }
        let full = format!("{} {}", sget(c, "issuer"), sget(c, "product"));
        let aliases: Vec<String> = arr(c, "aliases")
            .iter()
            .map(omuse_common::util::py_str)
            .collect();
        let hit = aliases.iter().any(|a| low.contains(&a.to_lowercase()))
            || low.contains(&full.to_lowercase());
        let label = aliases.first().cloned().unwrap_or(full);
        if hit && !named.contains(&label) {
            named.push(label);
        }
    }
    if named.is_empty() {
        return answer.to_string();
    }
    let list = named.join(", ");
    let line = if zh {
        format!("（{list} 在信用卡数据库里的条款是未核实的初始数据，请以发卡行网页为准。）")
    } else {
        format!("(The catalog's terms for {list} are unverified seeds; check them on the issuer's page.)")
    };
    format!("{}\n\n{line}", answer.trim_end())
}

/// The shipped card catalog's cards (`skills/credit-card-picker/cards/catalog.json`), parsed once.
fn catalog_cards() -> &'static [Value] {
    static CARDS: LazyLock<Vec<Value>> = LazyLock::new(|| {
        crate::skills::BUILTIN
            .get_file("credit-card-picker/cards/catalog.json")
            .and_then(|f| f.contents_utf8())
            .and_then(|t| serde_json::from_str::<Value>(t).ok())
            .map(|v| arr(&v, "cards").to_vec())
            .unwrap_or_default()
    });
    &CARDS
}

/// The catalog card a user's card name means: one whose name or an alias contains the name's most telling word
/// (`card_word`), the issuing market's first.
fn catalog_card(name: &str, market: &str) -> Option<&'static Value> {
    let word = card_word(name);
    if word.is_empty() {
        return None;
    }
    let hits: Vec<&Value> = catalog_cards()
        .iter()
        .filter(|c| {
            std::iter::once(format!("{} {}", sget(c, "issuer"), sget(c, "product")))
                .chain(arr(c, "aliases").iter().map(omuse_common::util::py_str))
                .any(|n| n.to_lowercase().contains(&word))
        })
        .collect();
    hits.iter()
        .find(|c| market.is_empty() || sget(c, "region") == market)
        .or_else(|| hits.first())
        .copied()
}

/// `answer` giving the foreign transaction fee of the user's cards issued in another country than the merchant's — the
/// market of the card table the run read last — from the catalog's structured `fx_fee_pct` (researched per card, with
/// its source): a card with a fee above 0 is named with it, a card without one is not mentioned, a card whose fee the
/// catalog does not know is said to be unknown — unless the answer already speaks of foreign fees (cards_markets_real
/// 2026-10-31: a Hong Kong dinner, the Singapore cards' fee left out).
pub fn with_fx_fees(transcript: &[Value], answer: &str, zh: bool) -> String {
    static MARKET: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"# Credit cards — ([A-Z]{2}) ").unwrap());
    static LISTED: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?m)^- id=\S+ · (.+?) · issued in ([^·]+?) · card · ").unwrap()
    });
    let country = |c: &str| -> String {
        match c.trim().to_lowercase().as_str() {
            "sg" | "singapore" | "新加坡" => "SG",
            "hk" | "hong kong" | "香港" => "HK",
            "us" | "usa" | "united states" | "united states of america" | "美国" => "US",
            other => return other.to_uppercase(),
        }
        .to_string()
    };
    let place = |code: &str| -> String {
        match (code, zh) {
            ("SG", false) => "Singapore".into(),
            ("HK", false) => "Hong Kong".into(),
            ("US", false) => "the United States".into(),
            ("SG", true) => "新加坡".into(),
            ("HK", true) => "香港".into(),
            ("US", true) => "美国".into(),
            (c, _) => c.to_string(),
        }
    };
    let mut market = String::new();
    let mut cards: Vec<(String, String)> = Vec::new();
    for m in transcript {
        if get_str(m, "role") == "tool" {
            let text = content_of(m).replace("\\n", "\n");
            if let Some(c) = MARKET.captures_iter(&text).last() {
                market = c[1].to_string();
            }
            for c in LISTED.captures_iter(&text) {
                cards.push((c[1].trim().to_string(), country(&c[2])));
            }
        }
        for tc in tool_calls(m) {
            let (name, args) = stored_call(tc);
            if name == "vault_add_card" && !sget(&args, "country").trim().is_empty() {
                cards.push((
                    sget(&args, "name").trim().to_string(),
                    country(&sget(&args, "country")),
                ));
            }
        }
    }
    if market.is_empty() {
        return answer.to_string();
    }
    let (mut fees, mut unknown): (Vec<String>, Vec<String>) = (Vec::new(), Vec::new());
    let mut seen: Vec<&str> = Vec::new();
    for (card, from) in &cards {
        if from == &market || seen.contains(&card.as_str()) {
            continue;
        }
        seen.push(card);
        match catalog_card(card, from)
            .and_then(|c| c.get("fx_fee_pct"))
            .and_then(Value::as_f64)
        {
            Some(fee) if fee > 0.0 => fees.push(format!("{card} {}%", pyfmt_pct(fee))),
            Some(_) => {}
            None => unknown.push(card.clone()),
        }
    }
    let mut lines: Vec<String> = Vec::new();
    if !fees.is_empty() {
        lines.push(if zh {
            format!(
                "（在{}以外发行的卡在这里会收境外交易手续费：{}（数据库）。）",
                place(&market),
                fees.join("，")
            )
        } else {
            format!(
                "(Cards issued outside {} add a foreign transaction fee here: {} (catalog).)",
                place(&market),
                fees.join(", ")
            )
        });
    }
    if !unknown.is_empty() {
        lines.push(if zh {
            format!("（{} 的境外交易手续费不在数据库里，请以发卡行条款为准。）", unknown.join("、"))
        } else {
            format!(
                "(The catalog does not know the foreign transaction fee of {}; check the issuer's terms.)",
                unknown.join(", ")
            )
        });
    }
    if lines.is_empty() {
        return answer.to_string();
    }
    format!("{}\n\n{}", answer.trim_end(), lines.join("\n"))
}

/// A percentage as written: 3.25, 2.7, 3 (no trailing zeros).
fn pyfmt_pct(v: f64) -> String {
    let s = format!("{v:.2}");
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

/// The user's cards as this run saw them, by name: the `card` items of `vault_list` results and the cards
/// `vault_add_card` added or found ("added to the Vault: Citi Prestige (id=…)").
pub fn vault_cards(transcript: &[Value]) -> Vec<String> {
    static LISTED: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?m)^- id=\S+ · (.+?)(?: · issued in [^·]+)? · card · ").unwrap()
    });
    static ADDED: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?:added to the Vault|already in the Vault): (.+?) \(id=").unwrap()
    });
    let mut out: Vec<String> = Vec::new();
    for t in transcript.iter().filter(|m| get_str(m, "role") == "tool") {
        let text = content_of(t);
        for c in LISTED
            .captures_iter(&text)
            .chain(ADDED.captures_iter(&text))
        {
            let label = c[1].trim().to_string();
            if !out.iter().any(|x| x.eq_ignore_ascii_case(&label)) {
                out.push(label);
            }
        }
    }
    out
}

/// The most telling word of a card's name (its longest word that is not generic): how an answer names the card.
fn card_word(label: &str) -> String {
    const GENERIC: &[&str] = &[
        "card",
        "cards",
        "credit",
        "debit",
        "the",
        "visa",
        "mastercard",
        "signature",
        "infinite",
        "world",
        "elite",
        "american",
        "express",
    ];
    label
        .to_lowercase()
        .split(|ch: char| !ch.is_alphanumeric())
        .filter(|w| w.chars().count() >= 2 && !GENERIC.contains(w))
        .max_by_key(|w| w.chars().count())
        .unwrap_or("")
        .to_string()
}

/// A note sending the model back once when a card pick names only one of the user's several cards: the answer must
/// compare the pick with the others (rate and value each) and give the foreign transaction fee where it applies
/// (cards_markets_real 2026-10-30: "Use your Hang Seng Travel+", nothing about the user's two other cards). `None`
/// when no catalog was read, the user holds fewer than two cards, or the answer names two or more.
pub fn card_comparison_hint(transcript: &[Value], answer: &str, zh: bool) -> Option<String> {
    let read_catalog = transcript
        .iter()
        .any(|m| get_str(m, "role") == "tool" && content_of(m).contains("# Credit cards — "));
    let cards = vault_cards(transcript);
    if !read_catalog || cards.len() < 2 {
        return None;
    }
    let low = answer.to_lowercase();
    let named = cards
        .iter()
        .filter(|c| {
            let w = card_word(c);
            !w.is_empty() && low.contains(&w)
        })
        .count();
    if named >= 2 {
        return None;
    }
    let list = cards.join(", ");
    Some(if zh {
        format!("（系统）用户有这些卡：{list}。你的回答只提到了一张。请把推荐的卡和用户的其他卡逐一比较（按数据库里的费率算出每张卡的回报），\
发卡国家和商户国家不同的卡要写出境外交易手续费；然后重新写完整的回答。回答直接从内容开始，不要提到这次核对。")
    } else {
        format!("(System) The user holds these cards: {list}. Your answer names only one. Compare the pick with each of \
the user's other cards (the rate and the value each returns, from the catalog tables), and give the foreign \
transaction fee for any card issued in another country than the merchant's; then write the complete answer again. \
Start straight with the content; don't mention this check.")
    })
}

/// Every market's table of the card catalog the build ships (`skills/credit-card-picker/cards/*.md`).
fn shipped_card_catalog() -> String {
    crate::skills::BUILTIN
        .get_dir("credit-card-picker/cards")
        .map(|d| {
            d.files()
                .filter(|f| f.path().extension().is_some_and(|e| e == "md"))
                .filter_map(|f| f.contents_utf8())
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default()
}

/// `answer` saying which of the user's cards the card catalog lacks: when a run read a catalog table and the user's
/// Vault cards (`vault_list`), an answer that names a card with no row in the tables and does not already say so gets
/// "(<card> is not in the card catalog; its terms were not verified there.)" — a rate the answer gives for it was
/// looked up or assumed, and the user must know (cards_real 2026-10-30: "assuming it earns 2%").
pub fn with_uncatalogued_cards(transcript: &[Value], answer: &str, zh: bool) -> String {
    const GENERIC: &[&str] = &[
        "card",
        "cards",
        "credit",
        "debit",
        "the",
        "visa",
        "mastercard",
        "signature",
        "infinite",
        "world",
        "elite",
        "american",
        "express",
    ];
    let tools: Vec<String> = transcript
        .iter()
        .filter(|m| get_str(m, "role") == "tool")
        .map(content_of)
        .collect();
    if !tools.iter().any(|t| t.contains("# Credit cards — ")) {
        return answer.to_string();
    }
    // the whole shipped catalog, every market — not only the tables this run read (cards_markets_real 2026-10-30:
    // the Hong Kong card flagged as missing after the agent read another market's table)
    let catalog: String = shipped_card_catalog().to_lowercase();
    let low = answer.to_lowercase();
    let mut lines = Vec::new();
    let mut seen = HashSet::new();
    {
        for label in vault_cards(transcript) {
            let words: Vec<String> = label
                .to_lowercase()
                .split(|ch: char| !ch.is_alphanumeric())
                .filter(|w| w.chars().count() >= 2 && !GENERIC.contains(w))
                .map(str::to_string)
                .collect();
            if words.is_empty() || !seen.insert(words.join(" ")) {
                continue;
            }
            let listed = words.iter().all(|w| catalog.contains(w.as_str()));
            // named in the answer: its most telling word (the longest)
            let key = words
                .iter()
                .max_by_key(|w| w.chars().count())
                .cloned()
                .unwrap_or_default();
            if !listed && low.contains(&key) {
                lines.push(if zh {
                    format!("（{label} 不在信用卡权益数据库里，它的条款没有核实。）")
                } else {
                    format!(
                        "({label} is not in the card catalog; its terms were not verified there.)"
                    )
                });
            }
        }
    }
    if lines.is_empty() {
        return answer.to_string();
    }
    format!("{}\n\n{}", answer.trim_end(), lines.join("\n"))
}

/// A scheduled run's answer with what it notified the user: when the run sent `notify_user` and the answer names
/// none of it (its title), the notification is added — a run's Results must not contradict the 🔔 the user got
/// (2026-10-30, assistant_real: a summary of 2 emails notified, the answer "No new emails have arrived").
pub fn with_notifications(transcript: &[Value], answer: &str, zh: bool) -> String {
    let sent: HashSet<String> = transcript
        .iter()
        .filter(|m| get_str(m, "role") == "tool" && !content_of(m).starts_with("ERROR"))
        .map(|m| get_str(m, "tool_call_id"))
        .collect();
    let low = answer.to_lowercase();
    let mut lines = Vec::new();
    for tc in transcript.iter().flat_map(tool_calls) {
        let (name, args) = stored_call(tc);
        if name != "notify_user" || !sent.contains(&get_str(tc, "id")) {
            continue;
        }
        let title = sget(&args, "title");
        let msg = sget(&args, "message");
        if title.trim().is_empty() || low.contains(&title.to_lowercase()) {
            continue;
        }
        lines.push(if zh {
            format!("🔔 已通知你：{title} — {}", take_chars(&msg, 400))
        } else {
            format!("🔔 Notified you: {title} — {}", take_chars(&msg, 400))
        });
    }
    if lines.is_empty() {
        return answer.to_string();
    }
    format!("{}\n\n{}", answer.trim_end(), lines.join("\n"))
}

/// `answer` with the amount the user approved: when the run had a purchase approved (`purchase_confirm` →
/// `"status": "approved"`) and the answer does not give its total, or not what it was for (the first word of the item's
/// name — travel_michelin_e2e 2026-10-30: "¥28,000 paid" and no restaurant), a line names both — whatever
/// happened next, the user must see what they agreed to pay (travel_michelin_e2e 2026-10-30: the table was booked with
/// a JPY 28,000 deposit and the answer gave only the booking reference).
pub fn with_purchase_total(transcript: &[Value], answer: &str, zh: bool) -> String {
    let approved: HashSet<String> = transcript
        .iter()
        .filter(|m| {
            get_str(m, "role") == "tool" && content_of(m).contains("\"status\": \"approved\"")
        })
        .map(|m| get_str(m, "tool_call_id"))
        .collect();
    let digits = answer.replace([',', ' ', '\u{a0}'], "");
    let mut lines = Vec::new();
    for tc in transcript.iter().flat_map(tool_calls) {
        let (name, args) = stored_call(tc);
        if name != "purchase_confirm" || !approved.contains(&get_str(tc, "id")) {
            continue;
        }
        let Some(total) = args.get("total").and_then(omuse_common::util::as_f64) else {
            continue;
        };
        let whole = format!("{total:.0}");
        let given = if total.fract() == 0.0 {
            Regex::new(&format!(r"(^|\D){whole}(\.0+)?(\D|$)"))
        } else {
            Regex::new(&format!(
                r"(^|\D){}(\D|$)",
                regex::escape(&format!("{total:.2}"))
            ))
        };
        let what: Vec<String> = arr(&args, "items")
            .iter()
            .map(|x| sget(x, "name"))
            .filter(|x| !x.is_empty())
            .collect();
        // what was bought, by the first word of its name ("Kappo" of "Kappo Yamabuki …", "SQ638" also as "SQ 638")
        let named = what.first().is_none_or(|w| {
            let first: String = w
                .split(|c: char| !c.is_alphanumeric())
                .find(|t| t.chars().count() >= 3)
                .unwrap_or("")
                .to_lowercase();
            first.is_empty() || digits.to_lowercase().contains(&first)
        });
        if named && given.is_ok_and(|r| r.is_match(&digits)) {
            continue;
        }
        let cur = sget(&args, "currency");
        let amount = if total.fract() == 0.0 {
            fmt_thousands(&whole)
        } else {
            let s = format!("{total:.2}");
            let (i, f) = s.split_once('.').unwrap_or((&s, "00"));
            format!("{}.{f}", fmt_thousands(i))
        };
        let what = if what.is_empty() {
            String::new()
        } else {
            format!(" ({})", what.join("; "))
        };
        lines.push(if zh {
            format!("你批准的金额：{cur} {amount}{what}")
        } else {
            format!("Amount you approved: {cur} {amount}{what}")
        });
    }
    if lines.is_empty() {
        return answer.to_string();
    }
    format!("{}\n\n{}", answer.trim_end(), lines.join("\n"))
}

/// `answer` covering the user's preferred airline: when the run searched flights (`flights_search`, or a connector's of
/// the same name) and the answer names neither the preferred airline (an alliance: nor any member) nor one of its flight
/// numbers, a line gives the cheapest preferred offer in the results with its price, or says there is none and offers
/// to look for one (travel_ask_real 2026-10-30: the travel connector's search, which has no airline filter, found only
/// Singapore Airlines and the answer never mentioned ANA, the user's preference).
pub fn with_preferred_airline(
    transcript: &[Value],
    answer: &str,
    prefs: &[String],
    zh: bool,
) -> String {
    let prefs: Vec<String> = prefs
        .iter()
        .map(|p| p.trim().to_string())
        .filter(|p| !p.is_empty())
        .collect();
    if prefs.is_empty() {
        return answer.to_string();
    }
    let searched: HashSet<String> = transcript
        .iter()
        .flat_map(tool_calls)
        .filter(|tc| stored_call(tc).0.rsplit("__").next() == Some("flights_search"))
        .map(|tc| get_str(tc, "id"))
        .collect();
    let results: Vec<Value> = transcript
        .iter()
        .filter(|m| get_str(m, "role") == "tool" && searched.contains(&get_str(m, "tool_call_id")))
        .filter_map(|m| first_json(&content_of(m)))
        .collect();
    if results.is_empty() {
        return answer.to_string();
    }
    let names = omuse_apps::loyalty::expand(&prefs, "airline");
    let codes = omuse_apps::loyalty::airline_codes(&prefs).unwrap_or_default();
    // every name an airline goes by ("ANA", "All Nippon Airways") for the preferred codes
    let aliases: Vec<String> = omuse_apps::loyalty::AIRLINE_CODES
        .iter()
        .filter(|(_, cs)| cs.iter().any(|c| codes.iter().any(|x| x == c)))
        .flat_map(|(ns, _)| ns.iter().map(|n| n.to_string()))
        .collect();
    let name_rx: Vec<Regex> = names
        .iter()
        .chain(prefs.iter())
        .chain(aliases.iter())
        .filter_map(|n| Regex::new(&format!(r"(?i)\b{}\b", regex::escape(n))).ok())
        .collect();
    let code_rx: Vec<Regex> = codes
        .iter()
        .filter_map(|c| Regex::new(&format!(r"\b{}\s?\d{{1,4}}\b", regex::escape(c))).ok())
        .collect();
    let preferred = |text: &str| {
        name_rx.iter().any(|r| r.is_match(text)) || code_rx.iter().any(|r| r.is_match(text))
    };
    if preferred(answer) {
        return answer.to_string();
    }
    let mut offers: Vec<(f64, String, String)> = Vec::new(); // (price, currency, flight)
    for r in &results {
        collect_offers(r, &mut offers, &preferred);
    }
    offers.sort_by(|a, b| a.0.total_cmp(&b.0));
    let who = prefs.join(", ");
    let line = match offers.first() {
        Some((price, cur, flight)) => {
            let amount = fmt_thousands(&format!("{price:.0}"));
            if zh {
                format!("你常用的航空公司（{who}）：这次结果里最便宜的是 {flight}，{cur} {amount}。")
            } else {
                format!("Your preferred airline ({who}): the cheapest in these results is {flight}, {cur} {amount}.")
            }
        }
        None if zh => format!("你常用的航空公司（{who}）：这次结果里没有它的航班。要我专门查一下 {who} 的航班吗？"),
        None => format!("Your preferred airline ({who}): none of its flights are in these results. Shall I look for {who} flights?"),
    };
    format!("{}\n\n{line}", answer.trim_end())
}

/// The first JSON value in a tool result (the text may wrap it, e.g. in `<untrusted_content …>`).
fn first_json(text: &str) -> Option<Value> {
    let i = text.find(['{', '['])?;
    serde_json::Deserializer::from_str(&text[i..])
        .into_iter::<Value>()
        .next()?
        .ok()
}

/// Every flight offer in `v` on a preferred airline, as (price, currency, flight): an object with a price
/// (`price`, `fare_amount`, `total_amount`) whose own text (airline, title, flight numbers, route) is a preferred one.
fn collect_offers(
    v: &Value,
    out: &mut Vec<(f64, String, String)>,
    preferred: &dyn Fn(&str) -> bool,
) {
    match v {
        Value::Array(xs) => xs.iter().for_each(|x| collect_offers(x, out, preferred)),
        Value::Object(m) => {
            let price = ["price", "fare_amount", "total_amount"]
                .iter()
                .find_map(|k| m.get(*k).and_then(omuse_common::util::as_f64))
                .filter(|p| *p > 0.0);
            let text: String = ["airline", "airlines", "title", "flight_no", "route"]
                .iter()
                .map(|k| m.get(*k).map(pystr_or_empty).unwrap_or_default())
                .collect::<Vec<_>>()
                .join(" ");
            match price {
                Some(p) if !text.trim().is_empty() && preferred(&text) => {
                    let flight = ["title", "flight_no", "airlines"]
                        .iter()
                        .map(|k| m.get(*k).map(pystr_or_empty).unwrap_or_default())
                        .find(|x| !x.is_empty())
                        .unwrap_or_default();
                    out.push((p, get_str(v, "currency"), flight));
                }
                _ => m.values().for_each(|x| collect_offers(x, out, preferred)),
            }
        }
        _ => {}
    }
}

/// `"28000"` → `"28,000"`.
fn fmt_thousands(int: &str) -> String {
    let (sign, d) = int.strip_prefix('-').map_or(("", int), |d| ("-", d));
    let mut out = String::new();
    for (i, c) in d.chars().enumerate() {
        if i > 0 && (d.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    format!("{sign}{out}")
}

/// The travel searches, which share ONE search per request (the user's message) and per model turn: `flights_search`,
/// `hotels_search`, `cars_search`, `places_search`, or a travel connector's `mcp_<server>__…` of the same name — what
/// it searches (`(English, 中文)`), `None` for any other tool. Web and news search are not counted.
pub fn travel_search(name: &str) -> Option<(&'static str, &'static str)> {
    match name.rsplit("__").next().unwrap_or(name) {
        "flights_search" => Some(("flights", "航班")),
        "hotels_search" => Some(("hotels", "酒店")),
        "cars_search" => Some(("rental cars", "租车")),
        "places_search" => Some(("places", "餐厅和地点")),
        _ => None,
    }
}

static TRAVEL_WORDS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(flights?|fly|airfares?|fares?|hotels?|hostels?|resorts?|rooms?|rental cars?|car rentals?|rent a car|suv|restaurants?|dinner|lunch)\b|机票|航班|酒店|住宿|租车|餐厅|订位").unwrap()
});
static SEARCH_INTENT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(find|search|look(ing)? for|cheapest|cheap|best|options?|recommend|any (good )?|need an?|want an?|book (me )?an?|available|availability|compare|what is the|what's the)\b|查|找|搜|最便宜|推荐|有没有|订一|订个").unwrap()
});
static NOT_A_SEARCH: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(fill|form|arrival card|e-?arrival|visa|etias|eta|insurance|claim|expense|report|check[- ]?in online|boarding pass|declaration|refund)\b|入境卡|签证|保险|理赔|报销|退款|填写|申报").unwrap()
});
static MAIL_OR_BOOKED: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(e-?mails?|g-?mail|inbox|outlook|mailbox|booked|did i (book|reserve)|have i (booked|reserved)|my (\w+ )?(bookings?|reservations?|flights?|hotels?|trips?|itinerar(y|ies)|tickets?)|confirmations?|itinerar(y|ies)|receipts?)\b|邮件|邮箱|已订|订好|我的(航班|酒店|行程|机票|预订)|确认函|收据").unwrap()
});

/// No mail before a travel search: a request to search for a new flight, hotel, car or restaurant (its words name one and
/// ask to find, search, compare … — not to fill a form, a visa, insurance, a claim or a report) that says nothing about
/// mail or something already booked never reads the user's mail — a `gmail_*` call there is refused.
/// "Find my hotel confirmation in Gmail, then search a hotel nearby" or "which hotel did I book" may read it.
pub fn mail_before_travel_guard(goal: &str, call: &ToolCall) -> Option<String> {
    let base = call.name.rsplit("__").next().unwrap_or(&call.name);
    let new_search =
        TRAVEL_WORDS.is_match(goal) && SEARCH_INTENT.is_match(goal) && !NOT_A_SEARCH.is_match(goal);
    if !base.starts_with("gmail_") || !new_search || MAIL_OR_BOOKED.is_match(goal) {
        return None;
    }
    Some(
        "ERROR: 已拦截 — 搜索或预订新的机票、酒店、租车或餐厅不读用户的邮件（用户没有要求从邮件里找东西）：直接用请求、个人资料和记忆去搜索。\
 Blocked: a new flight, hotel, car or restaurant search never reads the user's mail — the user did not ask for anything \
from their mail or about a booking they already have. Search with what the request, the profile and memory say."
            .to_string(),
    )
}

/// One travel search per request: a travel search in a task that already ran one (a successful result in its events —
/// a call that failed, e.g. on a bad date, may be corrected) is refused, naming that search.
pub fn one_search_guard(events: impl FnOnce() -> Vec<Value>, call: &ToolCall) -> Option<String> {
    travel_search(&call.name)?;
    let done = events().into_iter().find(|e| {
        get_str(e, "type") == "tool_result"
            && e["data"]["ok"].as_bool() == Some(true)
            && travel_search(&get_str(&e["data"], "name")).is_some()
            && get_str(&e["data"], "call_id") != call.id
    })?;
    let first = get_str(&done["data"], "call_id");
    let (what, zh) = travel_search(&get_str(&done["data"], "name")).unwrap_or(("travel", "旅行"));
    Some(format!(
        "ERROR: 已拦截 — 这个请求已经做过它唯一的一次旅行搜索（调用 {first}，{zh}），每个请求只搜一次。用那次的结果回答，\
其余的（换条件，或机票之外的酒店、租车）在回答里问用户，用户下一条消息再搜。\
 Blocked: this request already ran its one travel search (call {first}: {what}); ONE travel search per request. \
Answer from that result; offer the rest in the answer (other conditions, or the hotel / car after the flight) — the \
user's next message can search again."
    ))
}

/// The travel searches a request asks for, in the order its words name them (the tool's name and where it is
/// first named): "the cheapest flight, a 4-star hotel near Ginza and a compact rental car" → flights, hotels, cars.
/// A flight number ("flight SQ 12") names no flight search.
pub fn searches_asked(goal: &str) -> Vec<&'static str> {
    static KINDS: LazyLock<[(&'static str, Regex); 4]> = LazyLock::new(|| {
        [
            ("flights_search", Regex::new(r"(?i)\b(flights?|airfares?|plane tickets?)\b(?:\s+[A-Z]{2}\s?\d{2,4}\b)?|机票|航班").unwrap()),
            ("hotels_search", Regex::new(r"(?i)\b(hotels?|hostels?|resorts?|accommodation)\b|酒店|住宿").unwrap()),
            ("cars_search", Regex::new(r"(?i)\b(rental cars?|car rentals?|rent(ing)? a car|car hire|hire car)\b|租车").unwrap()),
            ("places_search", Regex::new(r"(?i)\b(restaurants?|cafes?|bars?)\b|餐厅|餐馆").unwrap()),
        ]
    });
    static FLIGHT_NO: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?i)^flight\s+[A-Z]{2}\s?\d{2,4}\b").unwrap());
    let mut at: Vec<(usize, &'static str)> = KINDS
        .iter()
        .filter_map(|(tool, re)| {
            re.find_iter(goal)
                .find(|m| !FLIGHT_NO.is_match(&goal[m.start()..]))
                .map(|m| (m.start(), *tool))
        })
        .collect();
    at.sort();
    at.into_iter().map(|(_, t)| t).collect()
}

/// The first travel search of a request that asks for several is the one it names first (travel_brave_real
/// 2026-10-31: "the cheapest flight, a 4-star hotel and a rental car" searched the hotel and never the flight).
/// Part of the one-search rule: a search of another kind before it is refused, naming the one to run; a request that
/// names one kind, or a task that already searched, is not touched.
pub fn search_order_guard(goal: &str, searched: bool, call: &ToolCall) -> Option<String> {
    let base = call.name.rsplit("__").next().unwrap_or(&call.name);
    travel_search(base)?;
    if searched {
        return None;
    }
    let asked = searches_asked(goal);
    let first = *asked.first()?;
    if asked.len() < 2 || first == base || !asked.contains(&base) {
        return None;
    }
    let (what, zh) = travel_search(first).unwrap_or(("travel", "旅行"));
    let (mine, mine_zh) = travel_search(base).unwrap_or(("travel", "旅行"));
    Some(format!(
        "ERROR: 已拦截 — 用户先要的是{zh}：这个请求唯一的一次旅行搜索是 {first}；{mine_zh}在回答里提出，用户下一条消息再搜。\
 Blocked: the request names the {what} first, so its one travel search is {first}; offer the {mine} in the answer for \
the user's next message."
    ))
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
