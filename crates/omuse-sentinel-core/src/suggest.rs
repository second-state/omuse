//! Fewer approvals over time (roadmap batch 3): when the user has approved the same low-risk action (same tool, same
//! destination) 3 times in 30 days, OMuse proposes a standing approval for it. Nothing changes until the user turns it on;
//! payments, card fills, phone calls, order cancels / returns and browser clicks are never proposed — those stay one by one.

use std::collections::HashSet;

use anyhow::{anyhow, Result};
use indexmap::IndexMap;
use serde_json::{json, Value};

use omuse_common::params;
use omuse_common::util::{get_str, now_ts};

use crate::store::{field_f64, field_i64, field_str, Store};

/// Approvals needed before a suggestion appears.
pub const NEED: i64 = 3;
/// Window (seconds) in which they must have happened.
pub const WINDOW: f64 = 30.0 * 86400.0;
/// Tools never proposed for a standing approval.
pub const NEVER: &[&str] = &[
    "purchase_confirm",
    "browser_fill_secret",
    "phone_call",
    "gmail_forward",
];

/// May this approval count towards a suggestion?
pub fn eligible(ap: &Value) -> bool {
    let tool = get_str(ap, "tool");
    let reason = get_str(ap, "reason");
    if NEVER.contains(&tool.as_str()) || tool.starts_with("browser_") || tool.starts_with("mcp:") {
        return false;
    }
    !(reason.contains("spends money")
        || reason.contains("changes an order")
        || reason.contains("injection"))
}

fn init(store: &Store) -> Result<()> {
    store.db.script("CREATE TABLE IF NOT EXISTS grant_suggestions (key TEXT PRIMARY KEY, status TEXT, updated_at REAL);")
}

/// `"<tool>|<destination>"`.
pub fn key(tool: &str, dest: &str) -> String {
    format!("{tool}|{dest}")
}

#[derive(Default)]
struct Seen {
    count: i64,
    last: f64,
    titles: Vec<String>,
    tool: String,
    destination: String,
}

/// Open suggestions, most approvals first: `{key, count, last, titles, tool, destination, title}`.
pub fn suggestions(store: &Store, now: Option<f64>) -> Result<Vec<Value>> {
    init(store)?;
    let now = now.filter(|n| *n != 0.0).unwrap_or_else(now_ts);
    let mut seen: IndexMap<String, Seen> = IndexMap::new();
    for ap in store.approvals(Some("resolved"), 500)? {
        let resolved_at = field_f64(&ap, "resolved_at");
        if field_str(&ap, "status") != "approved" || resolved_at < now - WINDOW || !eligible(&ap) {
            continue;
        }
        let dest = get_str(&ap["summary"], "destination");
        let tool = field_str(&ap, "tool");
        let k = key(&tool, &dest);
        let s = seen.entry(k).or_default();
        s.tool = tool;
        s.destination = dest;
        s.count += 1;
        s.last = s.last.max(resolved_at);
        let t = get_str(&ap["summary"], "title");
        if !t.is_empty() && !s.titles.contains(&t) {
            s.titles.push(t);
        }
    }
    let closed: HashSet<String> = store
        .db
        .all(
            "SELECT key FROM grant_suggestions WHERE status IN ('dismissed','accepted')",
            params![],
        )?
        .iter()
        .map(|r| field_str(r, "key"))
        .collect();
    let grants = store.active_grants()?;
    let granted: HashSet<String> = grants
        .iter()
        .map(|g| key(&field_str(g, "tool"), &get_str(&g["match"], "destination")))
        .collect();
    let granted_any: HashSet<String> = grants
        .iter()
        .filter(|g| get_str(&g["match"], "destination").is_empty())
        .map(|g| field_str(g, "tool"))
        .collect();
    let mut out: Vec<Value> = seen
        .iter()
        .filter(|(k, v)| v.count >= NEED && !closed.contains(*k) && !granted.contains(*k) && !granted_any.contains(&v.tool))
        .map(|(k, v)| {
            json!({
                "key": k, "count": v.count, "last": v.last, "titles": v.titles, "tool": v.tool,
                "destination": v.destination, "title": v.titles.first().cloned().unwrap_or_else(|| v.tool.clone()),
            })
        })
        .collect();
    out.sort_by_key(|x| -field_i64(x, "count"));
    Ok(out)
}

/// Turn a suggestion into a grant (`days` = `None`/`0` → PERMANENT, else TIME_BOUND); returns the grant id.
pub fn accept(store: &Store, k: &str, days: Option<f64>) -> Result<String> {
    init(store)?;
    let (tool, dest) = k
        .split_once('|')
        .ok_or_else(|| anyhow!("not enough values to unpack"))?;
    let days = days.filter(|d| *d != 0.0);
    let gid = store.add_grant(
        tool,
        if days.is_none() {
            "PERMANENT"
        } else {
            "TIME_BOUND"
        },
        None,
        Some(&if dest.is_empty() {
            json!({})
        } else {
            json!({"destination": dest})
        }),
        days.map(|d| d * 86400.0),
        "你批准过 3 次以上后开启 (turned on from a suggestion)",
    )?;
    store.db.execute(
        "INSERT OR REPLACE INTO grant_suggestions(key, status, updated_at) VALUES (?,?,?)",
        params![k, "accepted", now_ts()],
    )?;
    Ok(gid)
}

/// Hide a suggestion for good.
pub fn dismiss(store: &Store, k: &str) -> Result<()> {
    init(store)?;
    store.db.execute(
        "INSERT OR REPLACE INTO grant_suggestions(key, status, updated_at) VALUES (?,?,?)",
        params![k, "dismissed", now_ts()],
    )?;
    Ok(())
}

/// After an approval: the suggestion it just completed (to tell the user once).
pub fn just_reached(store: &Store, ap: &Value) -> Result<Option<Value>> {
    if !eligible(ap) {
        return Ok(None);
    }
    let dest = get_str(&ap["summary"], "destination");
    let k = key(&field_str(ap, "tool"), &dest);
    Ok(suggestions(store, None)?
        .into_iter()
        .find(|s| field_str(s, "key") == k && field_i64(s, "count") == NEED))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::policy::{decide, ALLOW, ASK};

    #[test]
    fn suggested_not_switched_on() {
        let d = tempfile::tempdir().unwrap();
        let store = Store::new(d.path()).unwrap();
        let mut last = Value::Null;
        for i in 0..3 {
            let a = store
                .create_approval(
                    &format!("t{i}"),
                    &format!("c{i}"),
                    "gmail_send",
                    &json!({"to": "john@x.com"}),
                    &json!({"title": "发送邮件 Send email", "destination": "john@x.com"}),
                    "high",
                    "sends an email",
                )
                .unwrap();
            store
                .resolve_approval(
                    a["id"].as_str().unwrap(),
                    "approved",
                    "ONCE",
                    None,
                    None,
                    "user",
                )
                .unwrap();
            let b = store
                .create_approval(
                    &format!("t{i}"),
                    &format!("p{i}"),
                    "purchase_confirm",
                    &json!({"total": 5}),
                    &json!({"title": "确认购买", "destination": "decathlon.sg"}),
                    "high",
                    "确认这次购买 (spends money)",
                )
                .unwrap();
            store
                .resolve_approval(
                    b["id"].as_str().unwrap(),
                    "approved",
                    "ONCE",
                    None,
                    None,
                    "user",
                )
                .unwrap();
            if i < 2 {
                assert!(suggestions(&store, None).unwrap().is_empty());
            }
            last = a;
        }
        let sg = suggestions(&store, None).unwrap();
        assert_eq!(sg.len(), 1);
        assert_eq!(sg[0]["tool"], "gmail_send"); // never the purchase
        assert_eq!(sg[0]["count"], 3);
        assert_eq!(sg[0]["title"], "发送邮件 Send email");
        let ap = store
            .approval(last["id"].as_str().unwrap())
            .unwrap()
            .unwrap();
        assert_eq!(
            just_reached(&store, &ap).unwrap().unwrap()["destination"],
            "john@x.com"
        );
        let d0 = decide(
            &store,
            "gmail_send",
            &json!({"to": "john@x.com", "body": "hi"}),
            "t9",
            None,
            None,
            true,
        )
        .unwrap();
        accept(&store, sg[0]["key"].as_str().unwrap(), Some(30.0)).unwrap();
        assert!(suggestions(&store, None).unwrap().is_empty());
        let g = &store.active_grants().unwrap()[0];
        assert_eq!(g["tool"], "gmail_send");
        assert_eq!(g["scope"], "TIME_BOUND");
        let d1 = decide(
            &store,
            "gmail_send",
            &json!({"to": "john@x.com", "body": "hi"}),
            "t9",
            None,
            None,
            true,
        )
        .unwrap();
        let d2 = decide(
            &store,
            "gmail_send",
            &json!({"to": "eve@x.com", "body": "hi"}),
            "t9",
            None,
            None,
            true,
        )
        .unwrap();
        assert_eq!(
            (
                d0.decision.as_str(),
                d1.decision.as_str(),
                d2.decision.as_str()
            ),
            (ASK, ALLOW, ASK)
        );
        // dismissed stays dismissed
        for i in 0..3 {
            let a = store
                .create_approval(
                    &format!("u{i}"),
                    &format!("c{i}"),
                    "notion_create_page",
                    &json!({}),
                    &json!({"title": "Notion", "destination": "ws"}),
                    "high",
                    "",
                )
                .unwrap();
            store
                .resolve_approval(
                    a["id"].as_str().unwrap(),
                    "approved",
                    "ONCE",
                    None,
                    None,
                    "user",
                )
                .unwrap();
        }
        let k = suggestions(&store, None).unwrap()[0]["key"]
            .as_str()
            .unwrap()
            .to_string();
        assert_eq!(k, "notion_create_page|ws");
        dismiss(&store, &k).unwrap();
        assert!(suggestions(&store, None).unwrap().is_empty());
        assert!(!eligible(&json!({"tool": "browser_click"})));
        assert!(!eligible(
            &json!({"tool": "gmail_send", "reason": "x (changes an order)"})
        ));
    }
}
