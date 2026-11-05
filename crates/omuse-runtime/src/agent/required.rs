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
const WHATSNEXT: [&str; 1] = [super::whatsnext::WHATS_NEXT];
/// "What's next" asks its question (anything to add to today?) from a machine result (`status: confirm`); the
/// reply owes `whats_next_items` again.
const WHATSNEXT_ASKS: [&str; 1] = [super::whatsnext::WHATS_NEXT_ITEMS];
/// The steps a "What's next" result can name as owed next.
const WHATSNEXT_STEPS: [&str; 2] = [
    super::whatsnext::WHATS_NEXT_ITEMS,
    super::whatsnext::WHATS_NEXT_PUBLISH,
];

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
        s if s == super::whatsnext::SKILL => &WHATSNEXT,
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
    if let Some(need) = asked_back(transcript, last_user) {
        return Some(need);
    }
    if let Some(need) = machine_next(t) {
        return Some(need);
    }
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

/// A machine result in this request that names the next workflow tool ("What's next": `whats_next_items` after the
/// gathered mail and calendar, `whats_next_publish` after the research), with no answered call of it since: that call
/// is owed (whats_next_real 2026-11-05: the research done, the model answered in prose and no report was made).
fn machine_next(t: &[Value]) -> Option<Need> {
    let (at, next) = t.iter().enumerate().rev().find_map(|(i, m)| {
        if get_str(m, "role") != "tool"
            || !get_str(m, "tool_call_id").starts_with(super::workflow::RESULT_PREFIX)
        {
            return None;
        }
        let v = serde_json::from_str::<Value>(content_of(m).trim()).ok()?;
        let next = get_str(&v, "next_tool");
        let owed = matches!(get_str(&v, "status").as_str(), "gathered" | "researched")
            && WHATSNEXT_STEPS.contains(&next.as_str());
        Some(owed.then_some((i, next)))
    })??;
    let called = t[at..].iter().enumerate().any(|(j, m)| {
        tool_calls(m).iter().any(|tc| {
            stored_call(tc).0 == next
                && result_of(&t[at + j..], &get_str(tc, "id")).is_some_and(|c| answered(&c))
        })
    });
    let tool = WHATSNEXT_STEPS.iter().find(|x| **x == next)?;
    (!called).then(|| Need {
        skills: Vec::new(),
        tools: vec![tool],
    })
}

/// A result that asks the user: the tool's `needs_input`, or a machine's `confirm` ("What's next": anything to add?).
fn asks(v: &Value) -> bool {
    matches!(get_str(v, "status").as_str(), "needs_input" | "confirm")
}

/// The result of workflow call `id`: its own result, or — when the machine ran (`running`) — the machine's result
/// (the `<tool>_result` code exchange after it).
fn effective_result(t: &[Value], tool: &str, id: &str) -> Option<String> {
    let own = result_of(t, id)?;
    let running =
        serde_json::from_str::<Value>(own.trim()).is_ok_and(|v| get_str(&v, "status") == "running");
    if !running {
        return Some(own);
    }
    let at = t
        .iter()
        .position(|m| tool_calls(m).iter().any(|tc| get_str(tc, "id") == id))?;
    let want = format!("{tool}{}", super::workflow::RESULT_SUFFIX);
    let rid = t[at..].iter().flat_map(tool_calls).find_map(|tc| {
        (get_str(tc, "id").starts_with(super::workflow::RESULT_PREFIX) && stored_call(tc).0 == want)
            .then(|| get_str(tc, "id"))
    })?;
    result_of(t, &rid)
}

/// Every workflow tool a skill asks through.
pub(crate) fn workflow_tool(name: &str) -> Option<&'static str> {
    [
        &BOOKING[..],
        &SHOP,
        &CARD,
        &FWD,
        &BAG,
        &APDT,
        &NEWS,
        &ENTRY,
        &WHATSNEXT,
        &WHATSNEXT_ASKS,
    ]
    .into_iter()
    .flatten()
    .copied()
    .find(|x| *x == name)
}

/// The reply to a workflow tool's question: the latest workflow call before the user's latest message answered
/// `needs_input` (the tool asked the user), and no answered call of that tool since — the call is owed with what the
/// user gave, whatever it is (the user, 2026-11-02: apdt_real's "I don't have the documents at hand, please go
/// ahead" got prose saying the documents were required, and no call).
fn asked_back(transcript: &[Value], last_user: usize) -> Option<Need> {
    let before = &transcript[..last_user];
    let (tool, id) = before.iter().rev().find_map(|m| {
        tool_calls(m).iter().rev().find_map(|tc| {
            let (name, _) = stored_call(tc);
            workflow_tool(&name).map(|w| (w, get_str(tc, "id")))
        })
    })?;
    let asked = effective_result(before, tool, &id)
        .and_then(|c| serde_json::from_str::<Value>(c.trim()).ok())
        .is_some_and(|v| asks(&v));
    let since = &transcript[last_user..];
    let called = since.iter().enumerate().any(|(j, m)| {
        tool_calls(m).iter().any(|tc| {
            stored_call(tc).0 == tool
                && result_of(&since[j..], &get_str(tc, "id")).is_some_and(|c| answered(&c))
        })
    });
    (asked && !called).then(|| Need {
        skills: Vec::new(),
        tools: vec![tool],
    })
}

/// The open question of a chat's previous task, to carry into the next task's transcript: its latest workflow call
/// and that call's result, when the result is `needs_input` (the tool asked the user). The history between tasks is
/// text only, so without this the reply to the tool's question could not owe its call, and the tool could not see
/// what it already asked (apdt_real 2026-11-02).
pub fn open_question(prev: &[Value]) -> Vec<Value> {
    let Some((tc, id)) = prev.iter().rev().find_map(|m| {
        tool_calls(m).iter().rev().find_map(|tc| {
            workflow_tool(&stored_call(tc).0).map(|_| (tc.clone(), get_str(tc, "id")))
        })
    }) else {
        return Vec::new();
    };
    let tool = stored_call(&tc).0;
    let Some(result) = effective_result(prev, &tool, &id) else {
        return Vec::new();
    };
    let asked = serde_json::from_str::<Value>(result.trim()).is_ok_and(|v| asks(&v));
    if !asked {
        return Vec::new();
    }
    vec![
        serde_json::json!({"role": "assistant", "content": "", "tool_calls": [tc]}),
        serde_json::json!({"role": "tool", "tool_call_id": id, "content": result}),
    ]
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
        super::whatsnext::WHATS_NEXT | super::whatsnext::WHATS_NEXT_ITEMS => {
            ("the plan from your mail and calendar", "邮件和日历里的日程")
        }
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
    fn after_whats_nexts_research_the_report_is_owed() {
        let mut t = vec![json!({"role": "user", "content": "nothing to add"})];
        t.push(call("c1", "whats_next_items", json!({"confirmed": true})));
        t.push(tool("c1", r#"{"status": "running", "args": {}}"#));
        t.extend(super::super::workflow::result_exchange(
            &t,
            "whats_next_items",
            &json!({"status": "researched", "next_tool": "whats_next_publish"}),
        ));
        assert_eq!(
            pending(&t).map(|n| n.tools),
            Some(vec!["whats_next_publish"])
        );
        t.push(call("c2", "whats_next_publish", json!({"summary": "x"})));
        t.push(tool("c2", r#"{"status": "running", "args": {}}"#));
        assert_eq!(pending(&t), None);
        // the question (confirm) is the user's to answer: nothing owed in this request
        let mut q = vec![json!({"role": "user", "content": "what's next?"})];
        q.extend(super::super::workflow::result_exchange(
            &q,
            "whats_next_items",
            &json!({"status": "confirm", "next_tool": "whats_next_items"}),
        ));
        assert_eq!(pending(&q), None);
    }

    #[test]
    fn whats_nexts_question_is_carried_to_the_next_task_and_the_reply_owes_the_items_call() {
        // the user, 2026-11-04: asked in the chat, today's list and "anything to add?"; the reply (a new task in the
        // same chat) must call whats_next_items again with the items and what the user added
        let items = json!({"items": [{"kind": "meeting", "title": "Q4 review", "date": "2026-11-04", "item": 1}], "confirmed": false});
        let mut prev = vec![
            json!({"role": "user", "content": "what's next?"}),
            call("c1", "whats_next_items", items.clone()),
            tool(
                "c1",
                &json!({"status": "running", "args": items}).to_string(),
            ),
        ];
        prev.extend(super::super::workflow::result_exchange(
            &prev,
            "whats_next_items",
            &json!({"status": "confirm", "items": items["items"], "question": "Is anything missing from today?"}),
        ));
        prev.push(json!({"role": "assistant", "content": "Here is today. Anything to add?"}));
        let carried = open_question(&prev);
        assert_eq!(carried.len(), 2, "{carried:?}");
        assert!(content_of(&carried[1]).contains("\"confirm\""));
        let mut next = carried.clone();
        next.push(json!({"role": "user", "content": "Add dinner at Den at 7pm"}));
        assert_eq!(
            pending(&next).map(|n| n.tools),
            Some(vec!["whats_next_items"])
        );
        // once called again, nothing is owed
        next.push(call("c2", "whats_next_items", json!({"confirmed": true})));
        next.push(tool("c2", r#"{"status": "running", "args": {}}"#));
        assert_eq!(pending(&next), None);
        assert_eq!(tools_of("whats-next"), ["whats_next"]);
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

    #[test]
    fn the_reply_to_a_workflow_tools_question_owes_its_call() {
        // apdt_real 2026-11-02: the tool asked for the documents (optional); the user skipped them and said go ahead;
        // the model answered in prose — the call is owed with what the user gave
        let asked = r#"{"status": "needs_input", "missing": ["the documents still missing, if the user has them"]}"#;
        let mut t = vec![
            json!({"role": "user", "content": "apply for my HK departure tax refund"}),
            call("c1", "load_skill", json!({"name": "hk-apdt-refund"})),
            tool("c1", "skill text"),
            call("c2", "apdt_refund", json!({"out_flight": "CX 500"})),
            tool("c2", asked),
            json!({"role": "assistant", "content": "Please attach the documents."}),
            json!({"role": "user", "content": "I don't have them at hand, please go ahead"}),
        ];
        assert_eq!(pending(&t).map(|n| n.tools), Some(vec!["apdt_refund"]));
        // the call made and answered: nothing owed
        t.push(call("c3", "apdt_refund", json!({"out_flight": "CX 500"})));
        t.push(tool("c3", r#"{"status": "running", "args": {}}"#));
        assert_eq!(pending(&t), None);
        // a tool that answered (not a question): the next message owes nothing
        let t2 = vec![
            json!({"role": "user", "content": "x"}),
            call("c2", "apdt_refund", json!({})),
            tool("c2", r#"{"status": "running", "args": {}}"#),
            json!({"role": "user", "content": "thanks"}),
        ];
        assert_eq!(pending(&t2), None);
    }

    #[test]
    fn a_previous_tasks_open_question_is_carried_as_its_call_and_result() {
        let asked = r#"{"status": "needs_input", "missing": ["refund_method"]}"#;
        let prev = vec![
            json!({"role": "user", "content": "apply"}),
            call("c2", "apdt_refund", json!({"out_flight": "CX 500"})),
            tool("c2", asked),
            json!({"role": "assistant", "content": "How should the refund be paid?"}),
        ];
        let carried = open_question(&prev);
        assert_eq!(carried.len(), 2);
        assert_eq!(get_str(&carried[1], "content"), asked);
        // in the new task: the reply owes the call
        let mut t = vec![json!({"role": "system", "content": ""})];
        t.extend(carried);
        t.push(json!({"role": "user", "content": "to my card"}));
        assert_eq!(pending(&t).map(|n| n.tools), Some(vec!["apdt_refund"]));
        // a previous task whose tool answered: nothing carried
        let done = vec![
            call("c2", "apdt_refund", json!({})),
            tool("c2", r#"{"status": "running"}"#),
        ];
        assert!(open_question(&done).is_empty());
        assert!(open_question(&[]).is_empty());
    }
}
