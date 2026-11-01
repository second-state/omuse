//! The catalog id of a Vault card that has none yet (a card the user typed on the Vault card, a card the model added
//! without its `card_id`): in a card run, the model picks the catalog row for it out of the catalog's ids as a JSON
//! choice (`Runtime::decide`, `none` when the catalog has no row); code keeps the id with the card in the Vault and the
//! run's card facts read it. Code never matches a card's name to a row itself.

use omuse_common::util::get_str;
use serde_json::{json, Value};

use super::guards::{card_run, catalog_cards, vault_cards};
use super::Runtime;

/// The catalog rows as the decision's context: `id — issuer product (aliases) — market`, one per line.
pub fn catalog_menu() -> String {
    catalog_cards()
        .iter()
        .map(|c| {
            let aliases: Vec<String> = c
                .get("aliases")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(Value::as_str)
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default();
            format!(
                "{} — {} {}{} — {}",
                get_str(c, "id"),
                get_str(c, "issuer"),
                get_str(c, "product"),
                if aliases.is_empty() {
                    String::new()
                } else {
                    format!(" ({})", aliases.join(", "))
                },
                get_str(c, "region")
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// A code exchange that records a card's id for this run's facts (the `vault_add_card` call code made, and its result).
pub fn id_exchange(n: usize, label: &str, country: &str, card_id: &str) -> [Value; 2] {
    let id = format!("cid_{n}");
    [
        json!({"role": "assistant", "content": "", "tool_calls": [{"id": id, "type": "function",
               "function": {"name": "vault_add_card",
                            "arguments": json!({"name": label, "country": country, "card_id": card_id}).to_string()}}]}),
        json!({"role": "tool", "tool_call_id": id, "content": format!("card_id={card_id}")}),
    ]
}

impl Runtime {
    /// In a card run, the Vault cards this run saw without a catalog id: the model picks each one's row (or `none`),
    /// the Vault keeps it, and the exchanges returned let this run's facts read it.
    pub(crate) async fn attach_card_ids(&self, task_id: &str, transcript: &[Value]) -> Vec<Value> {
        if !card_run(transcript) {
            return Vec::new();
        }
        let todo: Vec<_> = vault_cards(transcript)
            .into_iter()
            .filter(|c| c.card_id.is_empty())
            .collect();
        if todo.is_empty() {
            return Vec::new();
        }
        let menu = catalog_menu();
        let ids: Vec<String> = catalog_cards()
            .iter()
            .map(|c| get_str(c, "id"))
            .filter(|i| !i.is_empty())
            .chain(std::iter::once("none".to_string()))
            .collect();
        let choices: Vec<&str> = ids.iter().map(String::as_str).collect();
        let mut out = Vec::new();
        for (n, card) in todo.iter().enumerate() {
            let question = format!(
                "Which catalog row is the user's card \"{}\"{}? Pick its id, or none when the catalog has no row for this card.",
                card.label,
                if card.country.is_empty() {
                    String::new()
                } else {
                    format!(" (issued in {})", card.country)
                }
            );
            let picked = self.decide(task_id, &question, &menu, &choices, "").await;
            if picked.is_empty() {
                continue;
            }
            let _ = self
                .sentinel(
                    "POST",
                    "/internal/vault/card",
                    Some(json!({"name": card.label, "country": card.country, "card_id": picked, "task_id": task_id})),
                    20.0,
                )
                .await;
            out.extend(id_exchange(n, &card.label, &card.country, &picked));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_menu_lists_every_row_by_id_and_the_exchange_feeds_the_facts() {
        let m = catalog_menu();
        assert!(m
            .lines()
            .any(|l| l.starts_with("hk-hang-seng-travel-visa-signature-card — Hang Seng")));
        // a recorded id is read by the run's card facts
        let mut t = vec![json!({"role": "tool", "tool_call_id": "x",
                                "content": "- id=v1 · Hang Seng Travel+ Visa Signature · card · card_id=unchecked · UNKNOWN"})];
        assert_eq!(vault_cards(&t)[0].card_id, "");
        t.extend(id_exchange(
            0,
            "Hang Seng Travel+ Visa Signature",
            "",
            "hk-hang-seng-travel-visa-signature-card",
        ));
        assert_eq!(
            vault_cards(&t)[0].card_id,
            "hk-hang-seng-travel-visa-signature-card"
        );
    }
}
