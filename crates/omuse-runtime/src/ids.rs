//! Values the model copies into a tool call, repaired in code like its links ([`crate::links`]). Many arguments are
//! not the model's own words but something a result, the user or the system prompt showed it, to be repeated exactly:
//! record ids (`item_id`, `message_id`, `thread_id`, `event_id`, `goal_id` …, lists such as `message_ids`), a page's
//! element `ref`, a mail `account` or `calendar`, a bot, a skill's `name`, the workspace file a tool reads (`path`,
//! `paths`, `audio_path`). On 2026-10-31 a real model wrote `vlt_752694d1f8` for `vlt_752694d1f1f8`: Sentinel refused
//! the unknown item and the model told the user the Vault had no passport expiry.
//!
//! Wherever a tool call's arguments are read, each such top-level value that was never shown becomes the one shown
//! value it is a slip of (`util::nearest`: one character off for short values, two for long ones, or cut short);
//! anything else stays as written. Free text, titles and the files a tool writes are never touched, and the Vault's
//! ids and field names are resolved again by Sentinel against the Vault itself (`vault::resolve_args`).

use std::collections::BTreeSet;
use std::sync::LazyLock;

use regex::Regex;
use serde_json::Value;

/// The words a value can be copied from: ids, refs, emails, file names and paths.
static TOKEN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[A-Za-z0-9][A-Za-z0-9_./@-]*[A-Za-z0-9]").unwrap());

/// Whether argument `key` of `tool` holds a value copied from what the run showed.
fn copied(tool: &str, key: &str) -> bool {
    match key {
        "id" | "ids" | "ref" | "refs" | "thread" | "account" | "from_account" | "calendar"
        | "bot" => true,
        "path" | "paths" | "audio_path" => tool != "files_write" && tool != "files_list",
        "name" | "file" => tool == "load_skill",
        k => k.ends_with("_id") || k.ends_with("_ids"),
    }
}

/// The values the run showed the model: tokens of tool results, the user's messages and the system prompt (never
/// the model's own messages).
pub fn seen(transcript: &[Value]) -> Vec<String> {
    let mut have = BTreeSet::new();
    let mut out = Vec::new();
    for m in transcript {
        if !matches!(
            m.get("role").and_then(Value::as_str),
            Some("tool" | "user" | "system")
        ) {
            continue;
        }
        let text = match m.get("content") {
            Some(Value::String(s)) => s.clone(),
            Some(other) => other.to_string(),
            None => continue,
        };
        for t in TOKEN.find_iter(&text) {
            let t = t.as_str();
            if t.len() >= 4 && have.insert(t.to_string()) {
                out.push(t.to_string());
            }
        }
    }
    out
}

fn mend(s: &mut String, seen: &[String]) {
    if s.contains("http") || s.trim().is_empty() {
        return; // links are crate::links' business
    }
    if let Some(fixed) = omuse_common::util::nearest(s.trim(), seen.iter().map(String::as_str)) {
        if fixed != s {
            *s = fixed.to_string();
        }
    }
}

/// Repair the copied values among `tool`'s arguments (see the module docs). `seen` is only built when one is there.
pub fn repair_args(tool: &str, args: &mut Value, transcript: &[Value]) {
    let Some(o) = args.as_object_mut() else {
        return;
    };
    if !o.keys().any(|k| copied(tool, k)) {
        return;
    }
    let shown = seen(transcript);
    for (_, v) in o.iter_mut().filter(|(k, _)| copied(tool, k)) {
        match v {
            Value::String(s) => mend(s, &shown),
            Value::Array(xs) => xs.iter_mut().for_each(|x| {
                if let Value::String(s) = x {
                    mend(s, &shown);
                }
            }),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn run() -> Vec<Value> {
        vec![
            json!({"role": "system", "content": "Skills: arrival-card-tw, hk-tax-refund"}),
            json!({"role": "user", "content": "Attached: workspace/uploads/e-ticket_CI52.pdf"}),
            json!({"role": "tool", "content": "id=vlt_752694d1f1f8 · Passport Jordan · fields: number, expiry\nid=vlt_0a1b2c3d4e5f · Visa card"}),
            json!({"role": "tool", "content": "[{\"id\": \"18c2f0a9b3d4e5f6\", \"thread_id\": \"18c2f0a9b3d4e000\", \"account\": \"lucas@example.com\"}]"}),
            json!({"role": "assistant", "content": "vlt_ffffffffffff"}),
        ]
    }

    #[test]
    fn the_values_the_run_showed_are_collected_but_not_the_models() {
        let s = seen(&run());
        for v in [
            "vlt_752694d1f1f8",
            "lucas@example.com",
            "workspace/uploads/e-ticket_CI52.pdf",
            "arrival-card-tw",
        ] {
            assert!(s.contains(&v.to_string()), "{v}: {s:?}");
        }
        assert!(!s.contains(&"vlt_ffffffffffff".to_string()));
    }

    #[test]
    fn a_mistyped_copy_becomes_the_shown_value() {
        // arrival_cards2_real 2026-10-31: "1f" dropped from the middle of the Vault id
        let mut a = json!({"ref": "e9", "item_id": "vlt_752694d1f8", "field": "expiry"});
        repair_args("browser_fill_secret", &mut a, &run());
        assert_eq!(
            a,
            json!({"ref": "e9", "item_id": "vlt_752694d1f1f8", "field": "expiry"})
        );
        let mut m = json!({"message_ids": ["18c2f0a9b3d4e5f", "18c2f0a9b3d4e000"], "account": "lucas@exmple.com"});
        repair_args("gmail_archive", &mut m, &run());
        assert_eq!(
            m,
            json!({"message_ids": ["18c2f0a9b3d4e5f6", "18c2f0a9b3d4e000"], "account": "lucas@example.com"})
        );
        let mut f =
            json!({"path": "workspace/uploads/e-ticket-CI52.pdf", "question": "flight number?"});
        repair_args("file_look", &mut f, &run());
        assert_eq!(f["path"], "workspace/uploads/e-ticket_CI52.pdf");
        let mut k = json!({"name": "arrival-card-twn"});
        repair_args("load_skill", &mut k, &run());
        assert_eq!(k["name"], "arrival-card-tw");
    }

    #[test]
    fn free_text_new_files_and_far_values_stay_as_written() {
        // a file the tool writes, a schedule's name, a value three characters off, two equally close values
        let mut w = json!({"path": "workspace/uploads/e-ticket_CI53.pdf", "content": "x"});
        repair_args("files_write", &mut w, &run());
        assert_eq!(w["path"], "workspace/uploads/e-ticket_CI53.pdf");
        let mut n = json!({"name": "arrival-card-twn", "goal": "vlt_752694d1f8"});
        repair_args("schedule_create", &mut n, &run());
        assert_eq!(
            n,
            json!({"name": "arrival-card-twn", "goal": "vlt_752694d1f8"})
        );
        let mut far = json!({"item_id": "vlt_752694d8"});
        repair_args("browser_fill_secret", &mut far, &run());
        assert_eq!(far["item_id"], "vlt_752694d8");
        let two = vec![json!({"role": "tool", "content": "vlt_aaaaaaaaaaa1 vlt_aaaaaaaaaaa2"})];
        let mut t = json!({"item_id": "vlt_aaaaaaaaaaa3"});
        repair_args("browser_fill_secret", &mut t, &two);
        assert_eq!(t["item_id"], "vlt_aaaaaaaaaaa3");
        // nested objects (a diagram's node ids) are the model's own
        let mut d = json!({"nodes": [{"id": "vlt_752694d1f8"}]});
        repair_args("make_diagram", &mut d, &run());
        assert_eq!(d["nodes"][0]["id"], "vlt_752694d1f8");
    }
}
