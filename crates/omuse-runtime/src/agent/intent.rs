//! What the user's latest request asks for, as ONE JSON choice the model makes (`Runtime::decide`), recorded in the
//! transcript for the code machines to read. A machine starts from this choice (or the skill the run loaded), never
//! from patterns over the user's wording; the request's details are then parsed by the machine's own JSON extraction
//! (`Step::Extract`) and validated in code.

use serde_json::{json, Value};

use super::util::{content_of, sget, stored_call, tool_calls};
use omuse_common::util::get_str;

/// The tool name of the recorded choice (a code call, never in the model's catalog).
pub const TOOL: &str = "request_intent";
/// The id prefix of the recorded choice.
pub const PREFIX: &str = "intent_";

pub const FLIGHT: &str = "search_or_book_a_flight";
pub const HOTEL: &str = "search_or_book_a_hotel";
pub const MICHELIN: &str = "find_or_book_a_michelin_restaurant";
pub const RESTAURANT: &str = "find_or_book_another_restaurant";
pub const SHOP: &str = "buy_or_compare_items_on_a_shop_link";
pub const ARRIVAL: &str = "fill_an_arrival_card_or_entry_form";
pub const CLAIM: &str = "file_an_insurance_or_card_claim";
pub const INSURANCE: &str = "buy_travel_insurance";
pub const TAX_REFUND: &str = "hong_kong_departure_tax_refund";
pub const NEWS: &str = "research_news_into_a_report";
pub const BAGGAGE: &str = "check_a_delayed_or_lost_bag";
pub const OTHER: &str = "something_else";

/// Every choice, in the order the model sees them; `OTHER` is the default.
pub const CHOICES: &[&str] = &[
    FLIGHT, HOTEL, MICHELIN, RESTAURANT, SHOP, ARRIVAL, CLAIM, INSURANCE, TAX_REFUND, NEWS,
    BAGGAGE, OTHER,
];

/// The question the choice answers.
pub const QUESTION: &str = "Which task does the user's latest message ask the assistant to do now? A question about \
a topic, advice, an email or a plan is something_else; a reply that accepts an offer in the assistant's last answer \
(\"yes, the hotel please\") asks for that offer.";

/// What the model reads for the choice: the assistant's last answer (for a reply to its offer) and the user's latest
/// message.
pub fn context(transcript: &[Value]) -> Option<String> {
    let last = transcript
        .iter()
        .rposition(|m| get_str(m, "role") == "user")?;
    let prev = transcript[..last]
        .iter()
        .rev()
        .find(|m| get_str(m, "role") == "assistant" && !content_of(m).trim().is_empty())
        .map(content_of)
        .unwrap_or_default();
    let prev: String = prev
        .chars()
        .rev()
        .take(600)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    Some(format!(
        "The assistant's last answer (its end):\n{}\n\nThe user's latest message:\n{}",
        if prev.is_empty() { "(none)" } else { &prev },
        content_of(&transcript[last])
    ))
}

/// Whether the latest user message still needs its choice (none recorded after it).
pub fn needed(transcript: &[Value]) -> bool {
    let Some(last) = transcript
        .iter()
        .rposition(|m| get_str(m, "role") == "user")
    else {
        return false;
    };
    !transcript[last..]
        .iter()
        .flat_map(tool_calls)
        .any(|tc| stored_call(tc).0 == TOOL)
}

/// The exchange that records the choice: a code call and its result.
pub fn exchange(task: &str) -> Vec<Value> {
    let id = format!("{PREFIX}1");
    vec![
        json!({"role": "assistant", "content": "", "tool_calls": [{"id": id, "type": "function",
               "function": {"name": TOOL, "arguments": "{}"}}]}),
        json!({"role": "tool", "tool_call_id": id, "content": json!({"task": task}).to_string()}),
    ]
}

/// The choice recorded for the latest user message, if any.
pub fn recorded(transcript: &[Value]) -> Option<String> {
    let last = transcript
        .iter()
        .rposition(|m| get_str(m, "role") == "user")?;
    let tail = &transcript[last..];
    let id = tail
        .iter()
        .flat_map(tool_calls)
        .filter(|tc| stored_call(tc).0 == TOOL)
        .map(|tc| get_str(tc, "id"))
        .last()?;
    tail.iter()
        .find(|m| get_str(m, "role") == "tool" && get_str(m, "tool_call_id") == id)
        .and_then(|m| serde_json::from_str::<Value>(&content_of(m)).ok())
        .map(|v| sget(&v, "task"))
        .filter(|t| CHOICES.contains(&t.as_str()))
}

/// Whether the latest request was recorded as `task`.
pub fn is(transcript: &[Value], task: &str) -> bool {
    recorded(transcript).as_deref() == Some(task)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_choice_is_recorded_for_the_latest_message_only() {
        let mut t =
            vec![json!({"role": "user", "content": "Find me a flight to Tokyo on 1 December"})];
        assert!(needed(&t));
        t.extend(exchange(FLIGHT));
        assert!(!needed(&t));
        assert!(is(&t, FLIGHT));
        t.push(json!({"role": "assistant", "content": "Flights … Shall I search the hotel near Ginza now?"}));
        t.push(json!({"role": "user", "content": "yes please"}));
        assert!(needed(&t));
        assert_eq!(recorded(&t), None);
        assert!(context(&t).unwrap().contains("Shall I search the hotel"));
        t.extend(exchange(HOTEL));
        assert!(is(&t, HOTEL));
        // a choice outside the list reads as none
        let mut bad = vec![json!({"role": "user", "content": "x"})];
        bad.extend(exchange("delete_everything"));
        assert_eq!(recorded(&bad), None);
    }
}
