//! The workflow-tool protocol of the run loop (docs/json-tools.md): the model calls a workflow tool `T(args)` with JSON
//! arguments; code validates them; a valid call starts a code machine whose steps are code calls; the machine's JSON
//! result comes back as a code exchange `T_result`; then the model writes the reply from it.
//!
//! The transcript is the only state, so a run suspended for a takeover carries on where it was:
//!
//! ```text
//! assistant  tool_calls [T(args)]                         (the model's call)
//! tool       {"status": "running", "args": {validated}}   (or "invalid" / "needs_input": the model calls again / asks)
//! assistant  tool_calls [bkg_1 …]  /  tool  …             (the machine's steps, id prefix per machine)
//! assistant  tool_calls [res_<n>: T_result {}]            (code)
//! tool       {"status": "shown", …, "hints": […]}      (the machine's JSON result)
//! assistant  the reply                                     (the model; T is not offered on this turn)
//! ```
//!
//! A machine is *active* while its `T` call has a `running` result and no `T_result` after it, and its page was not
//! handed to the `browser-use` skill.

use serde_json::{json, Value};

use super::util::{content_of, sget, stored_call, tool_calls};
use crate::llm::ToolCall;
use omuse_common::util::get_str;

/// The id prefix of a `T_result` code exchange.
pub const RESULT_PREFIX: &str = "res_";
/// The suffix of the code exchange that carries a workflow tool's result (`travel_search_result`).
pub const RESULT_SUFFIX: &str = "_result";

/// What validation made of a workflow tool's arguments.
#[derive(Debug, Clone, PartialEq)]
pub enum Checked {
    /// the arguments to run the machine with (dates parsed, codes and enums normalised, bad optional fields dropped)
    Valid(Value),
    /// fields in the wrong form the model can fix: `{"status": "invalid", "problems": [...]}`
    Invalid(Vec<String>),
    /// fields only the user can give: `{"status": "needs_input", "missing": [...]}`, asked before any page opens
    NeedsInput(Vec<String>),
}

impl Checked {
    /// The tool result the model gets for its call.
    pub fn to_result(&self) -> Value {
        match self {
            Checked::Valid(args) => json!({"status": "running", "args": args}),
            Checked::Invalid(p) => json!({"status": "invalid", "problems": p}),
            Checked::NeedsInput(m) => json!({"status": "needs_input", "missing": m}),
        }
    }
}

/// The active call of one of `tools`: (its index in the transcript — the assistant message —, the tool's name, the
/// validated arguments of its `running` result). `None` when the latest such call has its `T_result`, was not valid,
/// or its page went to `browser-use` (a `load_skill` of it after the call).
pub fn active(transcript: &[Value], tools: &[&str]) -> Option<(usize, String, Value)> {
    let (i, id, name) = transcript
        .iter()
        .enumerate()
        .flat_map(|(i, m)| {
            tool_calls(m)
                .iter()
                .map(move |tc| (i, tc))
                .collect::<Vec<_>>()
        })
        .filter_map(|(i, tc)| {
            let (name, _) = stored_call(tc);
            tools
                .contains(&name.as_str())
                .then(|| (i, get_str(tc, "id"), name))
        })
        .next_back()?;
    let res = result_of(transcript, &id)?;
    if get_str(&res, "status") != "running" {
        return None;
    }
    let after = &transcript[i + 1..];
    if result_exchange_after(after, &name) {
        return None;
    }
    let handed = after.iter().flat_map(tool_calls).any(|tc| {
        let (n, a) = stored_call(tc);
        n == "load_skill" && sget(&a, "name") == super::booking::BROWSER_USE
    });
    if handed {
        return None;
    }
    Some((
        i,
        name,
        res.get("args").cloned().unwrap_or_else(|| json!({})),
    ))
}

/// The JSON of the tool message answering call `id`.
fn result_of(transcript: &[Value], id: &str) -> Option<Value> {
    transcript
        .iter()
        .find(|m| get_str(m, "role") == "tool" && get_str(m, "tool_call_id") == id)
        .map(|m| serde_json::from_str(&content_of(m)).unwrap_or(Value::Null))
}

fn result_exchange_after(after: &[Value], tool: &str) -> bool {
    let want = format!("{tool}{RESULT_SUFFIX}");
    after
        .iter()
        .flat_map(tool_calls)
        .any(|tc| get_str(tc, "id").starts_with(RESULT_PREFIX) && stored_call(tc).0 == want)
}

/// The code exchange `T_result` (id `res_<n>`) that carries a machine's JSON result.
pub fn result_exchange(transcript: &[Value], tool: &str, result: &Value) -> Vec<Value> {
    let n = transcript
        .iter()
        .flat_map(tool_calls)
        .filter(|tc| get_str(tc, "id").starts_with(RESULT_PREFIX))
        .count()
        + 1;
    let id = format!("{RESULT_PREFIX}{n}");
    vec![
        json!({"role": "assistant", "content": "", "tool_calls": [{"id": id, "type": "function",
               "function": {"name": format!("{tool}{RESULT_SUFFIX}"), "arguments": "{}"}}]}),
        json!({"role": "tool", "tool_call_id": id, "content": result.to_string()}),
    ]
}

/// The machine's JSON result of the latest `T_result` after the user's latest message (what the reply must be written
/// from, and what the final review checks it against).
pub fn latest_result(transcript: &[Value]) -> Option<Value> {
    let last_user = transcript
        .iter()
        .rposition(|m| get_str(m, "role") == "user")
        .unwrap_or(0);
    let after = &transcript[last_user..];
    let id = after
        .iter()
        .flat_map(tool_calls)
        .filter(|tc| {
            get_str(tc, "id").starts_with(RESULT_PREFIX)
                && stored_call(tc).0.ends_with(RESULT_SUFFIX)
        })
        .map(|tc| get_str(tc, "id"))
        .next_back()?;
    result_of(after, &id)
}

/// Whether the model's next turn is the reply to a machine's result: the last message is a `T_result`. The workflow
/// tools are not offered on that turn — except after a result that read the mail for missing facts
/// (`claims::MAIL_READ`): the model then calls the tool again with the facts from those emails.
pub fn reply_turn(transcript: &[Value]) -> bool {
    transcript
        .last()
        .filter(|m| get_str(m, "role") == "tool")
        .filter(|m| get_str(m, "tool_call_id").starts_with(RESULT_PREFIX))
        .is_some_and(|m| {
            serde_json::from_str::<Value>(&content_of(m))
                .map(|v| get_str(&v, "status") != super::claims::MAIL_READ)
                .unwrap_or(true)
        })
}

/// The facts a machine's result says the reply must carry (`hints`).
pub fn hints(result: &Value) -> Vec<String> {
    result
        .get("hints")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .filter(|s| !s.trim().is_empty())
        .collect()
}

/// One code call of a machine (id `<prefix><n>`).
pub fn code_call(prefix: &str, n: usize, name: &str, args: Value) -> ToolCall {
    ToolCall {
        id: format!("{prefix}{n}"),
        name: name.into(),
        args,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(id: &str, name: &str, args: Value) -> Value {
        json!({"role": "assistant", "content": "", "tool_calls": [{"id": id, "type": "function",
               "function": {"name": name, "arguments": args.to_string()}}]})
    }
    fn tool(id: &str, content: &str) -> Value {
        json!({"role": "tool", "tool_call_id": id, "content": content})
    }

    #[test]
    fn a_running_call_is_active_until_its_result_exchange() {
        let mut t = vec![
            json!({"role": "user", "content": "flights to Tokyo"}),
            call("c1", "travel_search", json!({"kind": "flight"})),
            tool(
                "c1",
                &Checked::Valid(json!({"kind": "flight"}))
                    .to_result()
                    .to_string(),
            ),
        ];
        let (i, name, args) = active(&t, &["travel_search"]).unwrap();
        assert_eq!(
            (i, name.as_str(), args["kind"].as_str()),
            (1, "travel_search", Some("flight"))
        );
        assert!(!reply_turn(&t));
        let res = json!({"status": "shown", "hints": ["SQ 638 SGD 620"]});
        let ex = result_exchange(&t, "travel_search", &res);
        t.extend(ex);
        assert!(active(&t, &["travel_search"]).is_none());
        assert!(reply_turn(&t));
        assert_eq!(latest_result(&t).unwrap()["status"], "shown");
        assert_eq!(hints(&latest_result(&t).unwrap()), vec!["SQ 638 SGD 620"]);
    }

    #[test]
    fn an_invalid_or_needs_input_call_starts_nothing() {
        for c in [
            Checked::Invalid(vec!["date: not a date".into()]),
            Checked::NeedsInput(vec!["date".into()]),
        ] {
            let t = vec![
                json!({"role": "user", "content": "x"}),
                call("c1", "travel_search", json!({})),
                tool("c1", &c.to_result().to_string()),
            ];
            assert!(active(&t, &["travel_search"]).is_none());
        }
    }

    #[test]
    fn a_page_handed_to_browser_use_ends_the_machine() {
        let t = vec![
            json!({"role": "user", "content": "x"}),
            call("c1", "travel_book", json!({})),
            tool("c1", &Checked::Valid(json!({})).to_result().to_string()),
            call("bkg_1", "load_skill", json!({"name": "browser-use"})),
            tool("bkg_1", "---"),
        ];
        assert!(active(&t, &["travel_book"]).is_none());
    }

    #[test]
    fn a_result_of_an_earlier_request_is_not_the_latest() {
        let mut t = vec![json!({"role": "user", "content": "x"})];
        t.extend(result_exchange(
            &t,
            "travel_search",
            &json!({"status": "shown"}),
        ));
        t.push(json!({"role": "user", "content": "y"}));
        assert!(latest_result(&t).is_none());
    }
}
