//! The compulsory workflow call (docs/json-tools.md, "The compulsory workflow call"): a skill that asks through a
//! workflow tool (the arrival cards, the claims and the insurance, the departure-tax refund, the bag trace, the news,
//! the travel bookings and the shop, the entry requirements) needs that tool's call before a final answer. The trigger
//! is structured — which skills this request loaded (`load_skill` calls whose result is the sheet) — never the
//! request's or the reply's words.
//!
//! When the model answers without the call, the run loop does not take the answer: it asks the model once more with
//! the provider's forced tool choice (`tool_choice` set to that tool, or `required` over the loaded skills' tools when
//! there are several — the model picks one of the listed tools), and the call goes through the JSON layer like any
//! workflow call (repair → schema → planner regeneration → the executor's retry → `needs_input`). When that still
//! gives no call, the answer is kept and code appends a fixed line saying the check could not be done ([`unmet_item`]).

use omuse_common::util::get_str;
use serde_json::Value;

use super::show as S;
use super::util::{content_of, sget, stored_call, tool_calls};

/// How many times one run asks the model with the forced tool choice: the call, and once more after an `invalid`
/// result (the JSON layer's executor retry; a second invalid call becomes `needs_input`).
pub const MAX_FORCED: usize = 2;

const BOOKING: [&str; 2] = [super::booking::TRAVEL_SEARCH, super::booking::TRAVEL_BOOK];
const SHOP: [&str; 1] = [super::booking::SHOP_ORDER];
const CARD: [&str; 1] = [super::driver::ARRIVAL_CARD_FILL];
const CSR: [&str; 1] = [super::claims::CLAIM_FILE];
const FWD: [&str; 2] = [super::claims::CLAIM_FILE, super::claims::INSURANCE_BUY];
const BAG: [&str; 1] = [super::claims::BAG_TRACE];
const APDT: [&str; 1] = [super::apdt::APDT_REFUND];
const NEWS: [&str; 1] = [super::news::NEWS_GATHER];
const ENTRY: [&str; 1] = [super::entry::ENTRY_REQUIREMENTS];

/// The workflow tools a skill asks through (empty: the skill has none, nothing is required). Read from the machines'
/// own tables, so a new card or booking skill is covered without a list here.
pub fn tools_of(skill: &str) -> &'static [&'static str] {
    if let Some(s) = super::booking::SPECS.iter().find(|s| s.skill == skill) {
        return if s.kind == super::booking::Kind::Shop {
            &SHOP
        } else {
            &BOOKING
        };
    }
    if super::driver::CARDS.iter().any(|c| c.skill == skill) {
        return &CARD;
    }
    if super::entry::SKILLS.contains(&skill) {
        return &ENTRY;
    }
    match skill {
        "csr-trip-claim" => &CSR,
        "fwd-travel-insurance" => &FWD,
        s if s == super::bagtrace::SKILL => &BAG,
        s if s == super::apdt::APDT.skill => &APDT,
        s if s == super::news::SKILL => &NEWS,
        _ => &[],
    }
}

/// What this request still owes: the skills loaded whose workflow tools were not called, and those tools.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Need {
    pub skills: Vec<String>,
    pub tools: Vec<&'static str>,
}

/// The tool message answering call `id` in `t`.
fn result_of(t: &[Value], id: &str) -> Option<String> {
    t.iter()
        .find(|m| get_str(m, "role") == "tool" && get_str(m, "tool_call_id") == id)
        .map(content_of)
}

/// Whether a result answers the call as made: not an error and not `invalid` (the model fixes an invalid call; a
/// `needs_input` is the tool asking the user, which is the call's answer).
fn answered(content: &str) -> bool {
    let c = content.trim();
    if c.starts_with("ERROR") {
        return false;
    }
    serde_json::from_str::<Value>(c).map_or(true, |v| get_str(&v, "status") != "invalid")
}

/// The need of this request (the transcript after the user's latest message): every skill it loaded that asks
/// through workflow tools — and the union of those tools — when none of them has an answered call after the first such
/// load. `None` when nothing is owed.
pub fn pending(transcript: &[Value]) -> Option<Need> {
    let last_user = transcript
        .iter()
        .rposition(|m| get_str(m, "role") == "user")
        .unwrap_or(0);
    let t = &transcript[last_user..];
    let mut need = Need {
        skills: Vec::new(),
        tools: Vec::new(),
    };
    let mut first: Option<usize> = None;
    for (i, m) in t.iter().enumerate() {
        for tc in tool_calls(m) {
            let (name, args) = stored_call(tc);
            if name != "load_skill" || !sget(&args, "file").trim().is_empty() {
                continue;
            }
            let skill = sget(&args, "name").trim().to_string();
            let tools = tools_of(&skill);
            if tools.is_empty() || need.skills.contains(&skill) {
                continue;
            }
            // a load that was refused (another bot's skill) loaded nothing
            if !result_of(&t[i..], &get_str(tc, "id")).is_some_and(|c| !c.starts_with("ERROR")) {
                continue;
            }
            first.get_or_insert(i);
            need.skills.push(skill);
            for x in tools {
                if !need.tools.contains(x) {
                    need.tools.push(x);
                }
            }
        }
    }
    let from = first?;
    let called = t[from..].iter().enumerate().any(|(j, m)| {
        tool_calls(m).iter().any(|tc| {
            let (name, _) = stored_call(tc);
            need.tools.contains(&name.as_str())
                && result_of(&t[from + j..], &get_str(tc, "id")).is_some_and(|c| answered(&c))
        })
    });
    (!called).then_some(need)
}

/// A turn that gives no workflow call: no call at all, or only questions to the user (`ask_user`). Either way a
/// loaded skill's workflow call is still owed: the tool asks the user what it needs itself (`needs_input`).
pub fn only_questions(calls: &[crate::llm::ToolCall]) -> bool {
    calls.iter().all(|c| c.name == "ask_user")
}

/// The provider's `tool_choice` for the forced call: the one tool by name, or `required` (one of the tools offered).
pub fn tool_choice(tools: &[&str]) -> Value {
    match tools {
        [one] => serde_json::json!({"type": "function", "function": {"name": one}}),
        _ => serde_json::json!("required"),
    }
}

/// What the user is told a tool checks, for the fixed line (English, Chinese).
fn what(tool: &str) -> (&'static str, &'static str) {
    match tool {
        super::entry::ENTRY_REQUIREMENTS => (
            "the entry requirements (visa-free days, arrival-card windows, official links)",
            "入境要求（免签天数、入境卡填写时间、官方链接）",
        ),
        super::driver::ARRIVAL_CARD_FILL => ("the arrival card", "电子入境卡"),
        super::booking::TRAVEL_SEARCH | super::booking::TRAVEL_BOOK => {
            ("the travel search", "旅行搜索")
        }
        super::booking::SHOP_ORDER => ("the shop's listings", "商店商品"),
        super::claims::CLAIM_FILE | super::claims::INSURANCE_BUY => {
            ("the claim or the policy", "理赔或保单")
        }
        super::claims::BAG_TRACE => ("the bag's status", "行李状态"),
        super::apdt::APDT_REFUND => ("the departure tax refund", "离境税退款"),
        super::news::NEWS_GATHER => ("the news", "新闻"),
        _ => ("the check", "这项检查"),
    }
}

/// The fixed line appended when the required call was never made: the check could not be done.
pub fn unmet_item(need: &Need, zh: bool) -> Value {
    let (en, cn) = need
        .tools
        .first()
        .map(|t| what(t))
        .unwrap_or(("the check", "这项检查"));
    let line = if zh {
        format!("OMuse 这次没能用代码查询{cn}：请先以官方来源为准")
    } else {
        format!("OMuse could not look up {en} in code for this answer — check the official sources before you rely on it")
    };
    S::fact("Not checked", "未检查", &line)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn call(id: &str, name: &str, args: Value) -> Value {
        json!({"role": "assistant", "content": "", "tool_calls": [{"id": id, "type": "function",
               "function": {"name": name, "arguments": args.to_string()}}]})
    }
    fn tool(id: &str, content: &str) -> Value {
        json!({"role": "tool", "tool_call_id": id, "content": content})
    }

    #[test]
    fn a_question_in_place_of_the_call_still_owes_it() {
        // arrival_cards4_real 2026-11-02: entry-requirements loaded, then ask_user for an arrival date the tool does
        // not need — no call, the user asked
        let tc = |name: &str| crate::llm::ToolCall {
            id: "c1".into(),
            name: name.into(),
            args: json!({}),
        };
        assert!(only_questions(&[]));
        assert!(only_questions(&[tc("ask_user")]));
        assert!(!only_questions(&[tc("entry_requirements")]));
        assert!(!only_questions(&[tc("ask_user"), tc("web_search")]));
    }

    #[test]
    fn the_skills_tools_come_from_the_machines_tables() {
        assert_eq!(tools_of("th-arrival-card"), ["arrival_card_fill"]);
        assert_eq!(tools_of("flight-booking"), ["travel_search", "travel_book"]);
        assert_eq!(tools_of("online-shopping"), ["shop_order"]);
        assert_eq!(
            tools_of("fwd-travel-insurance"),
            ["claim_file", "insurance_buy"]
        );
        assert_eq!(tools_of("baggage-delayed-lost"), ["bag_trace"]);
        assert_eq!(tools_of("hk-apdt-refund"), ["apdt_refund"]);
        assert_eq!(tools_of("news-research"), ["news_gather"]);
        assert_eq!(tools_of("entry-requirements"), ["entry_requirements"]);
        assert_eq!(tools_of("arrival-cards"), ["entry_requirements"]);
        assert!(tools_of("tokyo").is_empty() && tools_of("credit-card-picker").is_empty());
    }

    #[test]
    fn a_loaded_skill_owes_its_call_until_an_answered_one() {
        let mut t = vec![
            json!({"role": "user", "content": "what do I need for Bangkok?"}),
            call("c1", "load_skill", json!({"name": "entry-requirements"})),
            tool("c1", "---\nname: entry-requirements\n---"),
            call(
                "c2",
                "load_skill",
                json!({"name": "entry-requirements", "file": "x.md"}),
            ),
            tool("c2", "x"),
        ];
        let need = pending(&t).unwrap();
        assert_eq!(need.skills, ["entry-requirements"]);
        assert_eq!(need.tools, ["entry_requirements"]);
        assert_eq!(
            tool_choice(&need.tools),
            json!({"type": "function", "function": {"name": "entry_requirements"}})
        );
        // an invalid call does not count; the answered one does
        t.push(call("c3", "entry_requirements", json!({})));
        t.push(tool(
            "c3",
            r#"{"status": "invalid", "problems": ["destinations: required"]}"#,
        ));
        assert!(pending(&t).is_some());
        t.push(call(
            "c4",
            "entry_requirements",
            json!({"destinations": [{"place": "Bangkok"}]}),
        ));
        t.push(tool("c4", r#"{"status": "running", "args": {}}"#));
        assert!(pending(&t).is_none());
    }

    #[test]
    fn several_skills_owe_one_of_their_tools_and_a_refused_load_owes_nothing() {
        let t = vec![
            json!({"role": "user", "content": "x"}),
            call("c1", "load_skill", json!({"name": "entry-requirements"})),
            tool("c1", "---"),
            call("c2", "load_skill", json!({"name": "th-arrival-card"})),
            tool("c2", "---"),
            call("c3", "load_skill", json!({"name": "flight-booking"})),
            tool(
                "c3",
                "ERROR: [other_bot] the skill flight-booking belongs to the bot(s) travel",
            ),
        ];
        let need = pending(&t).unwrap();
        assert_eq!(need.tools, ["entry_requirements", "arrival_card_fill"]);
        assert_eq!(tool_choice(&need.tools), json!("required"));
        // a skill loaded in an earlier request owes nothing in this one
        let mut later = t.clone();
        later.push(json!({"role": "user", "content": "thanks"}));
        assert!(pending(&later).is_none());
        // no skill with a workflow tool: nothing owed
        let t = vec![
            json!({"role": "user", "content": "x"}),
            call("c1", "load_skill", json!({"name": "tokyo"})),
            tool("c1", "---"),
        ];
        assert!(pending(&t).is_none());
    }

    #[test]
    fn the_unmet_line_is_fixed_and_names_what_was_not_checked() {
        let need = Need {
            skills: vec!["entry-requirements".into()],
            tools: vec!["entry_requirements"],
        };
        let l = S::render(&unmet_item(&need, false), false).unwrap();
        assert!(
            l.starts_with("- Not checked: OMuse could not look up the entry requirements"),
            "{l}"
        );
    }
}
