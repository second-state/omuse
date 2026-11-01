//! The decision API: when a step genuinely needs the model's judgement, the model gets the question and a JSON array
//! of choices and must answer with exactly one of them (`{"choice": "…"}`) — or, for a multi-select, a subset
//! (`{"choices": [...]}`). Anything else is retried once, then the caller's default stands. Code acts on the choice;
//! the model's prose never reaches a decision (CLAUDE.md, "Skills: code decides").

use omuse_common::util::take_chars;
use serde_json::{json, Value};

use super::runtime::opts;
use super::Runtime;
use crate::llm::{extract_json, ChatOpts};

/// The system prompt of a single choice.
pub const DECIDE_SYSTEM: &str = "You make one decision for a program. You get a question, the context it is about and \
the possible choices as a JSON array. Answer with JSON only, exactly {\"choice\": \"<one of the choices>\"} — one of \
the listed strings, copied exactly. No other keys, no explanation.";

/// The system prompt of a multi-select.
pub const DECIDE_MANY_SYSTEM: &str = "You make one decision for a program. You get a question, the context it is about \
and the possible choices as a JSON array. Answer with JSON only, exactly {\"choices\": [\"<choice>\", ...]} — every \
choice that applies, each one of the listed strings copied exactly. No other keys, no explanation.";

/// The listed choice `answer` names (exact, case- and space-insensitive), in the list's own spelling.
fn canonical(answer: &str, choices: &[&str]) -> Option<String> {
    let a = answer.trim();
    choices
        .iter()
        .find(|c| c.eq_ignore_ascii_case(a))
        .map(|c| c.to_string())
}

/// The choice in a model reply: `{"choice": "<listed>"}`; `None` for anything else (prose, an unlisted value, JSON
/// without the key).
pub fn parse_choice(reply: &str, choices: &[&str]) -> Option<String> {
    let v = extract_json(reply)?;
    canonical(v.get("choice")?.as_str()?, choices)
}

/// The choices in a model reply: `{"choices": [<listed>, ...]}` with at least one entry, every entry listed
/// (duplicates dropped, the reply's order kept); `None` for anything else.
pub fn parse_choices(reply: &str, choices: &[&str]) -> Option<Vec<String>> {
    let v = extract_json(reply)?;
    let items = v.get("choices")?.as_array()?;
    if items.is_empty() {
        return None;
    }
    let mut out: Vec<String> = Vec::new();
    for it in items {
        let c = canonical(it.as_str()?, choices)?;
        if !out.contains(&c) {
            out.push(c);
        }
    }
    Some(out)
}

/// The user message of a decision: the question, the context and the choices as JSON.
fn decision_prompt(question: &str, context: &str, choices: &[&str]) -> String {
    format!(
        "Question: {}\n\nContext:\n{}\n\nChoices: {}",
        take_chars(question, 500),
        take_chars(context, 16000),
        json!(choices)
    )
}

/// What a decision came to, for the `decision` event.
struct Outcome<T> {
    picked: T,
    retried: bool,
    defaulted: bool,
}

impl Runtime {
    /// The model's one-of choice for `question` over `context`: one of `choices`, or `default` when the model answers
    /// anything else twice (or cannot be reached). Temperature 0, a small reply, no tools; a `decision` event records
    /// `{question, choices, picked, retried, defaulted}`.
    pub async fn decide(
        &self,
        task_id: &str,
        question: &str,
        context: &str,
        choices: &[&str],
        default: &str,
    ) -> String {
        let out = self
            .ask_decision(
                task_id,
                DECIDE_SYSTEM,
                question,
                context,
                choices,
                |r| parse_choice(r, choices),
                default.to_string(),
            )
            .await;
        self.event(
            task_id,
            "decision",
            json!({"question": question, "choices": choices, "picked": out.picked,
                   "retried": out.retried, "defaulted": out.defaulted}),
        )
        .await;
        out.picked
    }

    /// The multi-select twin of [`Runtime::decide`]: every listed choice that applies, or `default` when the model
    /// answers anything else twice.
    pub async fn decide_many(
        &self,
        task_id: &str,
        question: &str,
        context: &str,
        choices: &[&str],
        default: &[&str],
    ) -> Vec<String> {
        let out = self
            .ask_decision(
                task_id,
                DECIDE_MANY_SYSTEM,
                question,
                context,
                choices,
                |r| parse_choices(r, choices),
                default.iter().map(|d| d.to_string()).collect(),
            )
            .await;
        self.event(
            task_id,
            "decision",
            json!({"question": question, "choices": choices, "picked": out.picked,
                   "retried": out.retried, "defaulted": out.defaulted}),
        )
        .await;
        out.picked
    }

    /// One decision call, a second one when the first answer is not a listed choice (the bad answer and the list again,
    /// fixed text), then the default. A model that cannot be reached gives the default at once.
    #[allow(clippy::too_many_arguments)]
    async fn ask_decision<T>(
        &self,
        task_id: &str,
        system: &str,
        question: &str,
        context: &str,
        choices: &[&str],
        parse: impl Fn(&str) -> Option<T>,
        default: T,
    ) -> Outcome<T> {
        let user = json!(decision_prompt(question, context, choices));
        self.ask_with(task_id, system, user, choices, None, parse, default)
            .await
    }

    /// [`Runtime::ask_decision`] with the user message's content given (text, or text and a picture) and a model of
    /// its own (the vision model).
    #[allow(clippy::too_many_arguments)]
    async fn ask_with<T>(
        &self,
        task_id: &str,
        system: &str,
        user: Value,
        choices: &[&str],
        model: Option<String>,
        parse: impl Fn(&str) -> Option<T>,
        default: T,
    ) -> Outcome<T> {
        let o = ChatOpts {
            max_tokens: Some(60),
            temperature: Some(0.0),
            no_think: true,
            model,
            ..opts("decide", task_id)
        };
        let mut msgs = vec![
            json!({"role": "system", "content": system}),
            json!({"role": "user", "content": user}),
        ];
        for attempt in 0..2 {
            let Ok(r) = self.llm.chat(&msgs, None, &o).await else {
                break;
            };
            if let Some(v) = parse(&r.content) {
                return Outcome {
                    picked: v,
                    retried: attempt > 0,
                    defaulted: false,
                };
            }
            msgs.push(json!({"role": "assistant", "content": take_chars(&r.content, 300)}));
            msgs.push(json!({"role": "user", "content": format!(
                "That is not one of the choices. Answer with JSON only, using only these strings: {}", json!(choices))}));
        }
        Outcome {
            picked: default,
            retried: msgs.len() > 2,
            defaulted: true,
        }
    }
}

/// A picture for a decision: base64 bytes and their media type.
pub struct Picture<'a> {
    pub b64: &'a str,
    pub mime: &'a str,
}

/// The `{"<key>": true|false}` flag in a model reply; `None` for anything else.
pub fn parse_flag(reply: &str, key: &str) -> Option<bool> {
    extract_json(reply)?.get(key)?.as_bool()
}

impl Runtime {
    /// The user message of a decision about a picture: the question and the choices as JSON, then the picture.
    fn picture_prompt(question: &str, choices: &[&str], pic: &Picture<'_>) -> Value {
        json!([
            {"type": "text", "text": format!("Question: {}\n\nChoices: {}", take_chars(question, 500), json!(choices))},
            {"type": "image_url", "image_url": {"url": format!("data:{};base64,{}", pic.mime, pic.b64)}},
        ])
    }

    /// The vision model's one-of choice about a picture (a labelled screenshot): `system` says how the picture is marked
    /// and that the answer is `{"choice": "<listed>"}`; one of `choices`, else `default` after one retry. A `decision`
    /// event records it. The model's prose never reaches the caller.
    pub async fn decide_on_picture(
        &self,
        task_id: &str,
        system: &str,
        question: &str,
        pic: &Picture<'_>,
        choices: &[&str],
        default: &str,
    ) -> String {
        let out = self
            .ask_with(
                task_id,
                system,
                Self::picture_prompt(question, choices, pic),
                choices,
                self.vision_model(),
                |r| parse_choice(r, choices),
                default.to_string(),
            )
            .await;
        self.event(
            task_id,
            "decision",
            json!({"question": question, "choices": if choices.len() > 12 { json!(format!("{} choices", choices.len())) } else { json!(choices) },
                   "picked": out.picked, "retried": out.retried, "defaulted": out.defaulted}),
        )
        .await;
        out.picked
    }

    /// The vision model's yes / no about a picture as JSON (`{"<key>": true|false}`), `default` after one retry.
    pub async fn decide_flag_on_picture(
        &self,
        task_id: &str,
        system: &str,
        question: &str,
        pic: &Picture<'_>,
        key: &str,
        default: bool,
    ) -> bool {
        let keys = [key];
        let out = self
            .ask_with(
                task_id,
                system,
                Self::picture_prompt(question, &[], pic),
                &keys,
                self.vision_model(),
                |r| parse_flag(r, key),
                default,
            )
            .await;
        self.event(
            task_id,
            "decision",
            json!({"question": question, "choices": [format!("{key}: true"), format!("{key}: false")],
                   "picked": out.picked, "retried": out.retried, "defaulted": out.defaulted}),
        )
        .await;
        out.picked
    }

    /// The vision model of the settings, `None` for the main model.
    fn vision_model(&self) -> Option<String> {
        let vm = super::util::sget(&self.settings(), "vision_model");
        (!vm.is_empty()).then_some(vm)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_choice_is_read_only_from_the_json_and_only_when_listed() {
        let c = ["error_page", "normal_page", "login_page"];
        assert_eq!(
            parse_choice("{\"choice\": \"error_page\"}", &c).as_deref(),
            Some("error_page")
        );
        // case and spaces, a fence, the list's own spelling back
        assert_eq!(
            parse_choice("```json\n{\"choice\": \" Login_Page \"}\n```", &c).as_deref(),
            Some("login_page")
        );
        // prose, an unlisted value, another key, a number: nothing
        assert_eq!(parse_choice("It is an error page.", &c), None);
        assert_eq!(parse_choice("{\"choice\": \"error\"}", &c), None);
        assert_eq!(parse_choice("{\"answer\": \"error_page\"}", &c), None);
        assert_eq!(parse_choice("{\"choice\": 1}", &c), None);
        assert_eq!(parse_choice("", &c), None);
    }

    #[test]
    fn a_flag_is_read_only_as_a_json_bool() {
        assert_eq!(parse_flag("{\"on_item\": true}", "on_item"), Some(true));
        assert_eq!(
            parse_flag("```json\n{\"on_item\": false}\n```", "on_item"),
            Some(false)
        );
        // prose, a string, another key: nothing
        assert_eq!(parse_flag("YES, it is on the button", "on_item"), None);
        assert_eq!(parse_flag("{\"on_item\": \"yes\"}", "on_item"), None);
        assert_eq!(parse_flag("{\"found\": true}", "on_item"), None);
    }

    #[test]
    fn a_multi_select_needs_every_entry_listed() {
        let c = ["pdf", "xlsx", "docx", "md", "none"];
        assert_eq!(
            parse_choices("{\"choices\": [\"PDF\", \"docx\", \"pdf\"]}", &c),
            Some(vec!["pdf".to_string(), "docx".to_string()])
        );
        assert_eq!(
            parse_choices("{\"choices\": [\"pdf\", \"pptx\"]}", &c),
            None
        );
        assert_eq!(parse_choices("{\"choices\": []}", &c), None);
        assert_eq!(parse_choices("{\"choice\": \"pdf\"}", &c), None);
        assert_eq!(parse_choices("pdf and docx", &c), None);
    }
}
