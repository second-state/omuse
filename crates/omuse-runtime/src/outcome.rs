//! Outcome evidence: a task that changed something in the world (an order, a booking, an email, a calendar event, a call)
//! is only "done" when there is proof — an order number on the confirmation page, a message id, an event id.
//!
//! [`assess`] reads what the task actually did (its events) and what the tools returned, and compares that with what the
//! final answer claims. It is pure (no I/O) so it can be tested and re-run on old tasks.
//!
//! ```text
//!     verified    every consequential action has evidence
//!     unverified  the answer says something was done, but the proof is missing (or nothing was done at all)
//!     none        the task changed nothing and claims nothing (a question, a search, a summary)
//! ```

use std::collections::BTreeSet;
use std::sync::LazyLock;

use omuse_common::util::{get_str, get_truthy, py_str, truthy};
use regex::Regex;
use serde_json::{json, Value};

pub const SEND_TOOLS: [&str; 4] = [
    "gmail_send",
    "gmail_reply",
    "gmail_forward",
    "slack_send_message",
];
pub const CALENDAR_TOOLS: [&str; 3] = [
    "calendar_create_event",
    "calendar_update_event",
    "calendar_delete_event",
];
pub const NOTION_TOOLS: [&str; 3] = ["notion_create_page", "notion_update_page", "notion_append"];
pub const CALL_TOOLS: [&str; 1] = ["phone_call"];

fn rx(p: &str) -> Regex {
    Regex::new(p).unwrap()
}

/// An order / booking / confirmation number as people write it next to its label.
static REF: LazyLock<Regex> = LazyLock::new(|| {
    rx(
        r"(?i)(?:订单号|订单编号|订单|预订号|确认号|确认码|预约号|参考号|order\s*(?:ref(?:erence)?|no\.?|number|#|id)?|booking\s*(?:ref(?:erence)?|no\.?|number|id|code)?|confirmation\s*(?:no\.?|number|code|#)?|reference\s*(?:no\.?|number)?|ref\.?)[\s*]*[:：#]?[\s*]*([A-Z0-9][A-Z0-9\-]{4,24})",
    )
});

/// Order / booking / confirmation numbers written next to their label. Markdown bold may wrap the label, the number or
/// both (`**Booking reference:** **TRV-1234**`); the Python version missed the last form and flagged confirmed orders.
pub fn refs_in(text: &str) -> Vec<String> {
    static DIGIT: LazyLock<Regex> = LazyLock::new(|| rx(r"\d"));
    let mut out: Vec<String> = Vec::new();
    for m in REF.captures_iter(text) {
        let r = m[1].trim_matches('-').to_string();
        if DIGIT.is_match(&r) && !out.iter().any(|x| x.to_uppercase() == r.to_uppercase()) {
            out.push(r);
        }
    }
    out
}

/// The reference a page showed after the action: the newest labelled reference in the tool results (`texts`, newest
/// last) — the page is the proof; the answer's words are never read for it.
fn confirmed_ref(texts: &[String]) -> Option<String> {
    texts
        .iter()
        .rev()
        .find_map(|t| refs_in(t).into_iter().next())
}

fn data_of(e: &Value) -> Value {
    e.get("data")
        .filter(|v| truthy(v))
        .cloned()
        .unwrap_or_else(|| json!({}))
}

/// A tool call that waited for the user's approval and got it (purchase_confirm, a booking click).
fn approved(events: &[Value], tool: &str) -> bool {
    let mut waiting: Vec<(String, Option<String>)> = Vec::new();
    for e in events {
        let d = data_of(e);
        let typ = get_str(e, "type");
        let aid = py_str(d.get("approval_id").unwrap_or(&Value::Null));
        if typ == "waiting" && get_str(&d, "type") == "approval" {
            let t = d
                .get("summary")
                .filter(|v| truthy(v))
                .map(|s| get_str(s, "tool"))
                .filter(|t| !t.is_empty())
                .or_else(|| Some(get_str(&d, "tool")).filter(|t| !t.is_empty()));
            match waiting.iter_mut().find(|(k, _)| *k == aid) {
                Some((_, v)) => *v = t,
                None => waiting.push((aid.clone(), t)),
            }
        }
        if typ == "approval_resolved"
            && get_str(&d, "decision") == "approved"
            && waiting
                .iter()
                .find(|(k, _)| *k == aid)
                .map(|(_, v)| v.as_deref() == Some(tool))
                .unwrap_or(false)
        {
            return true;
        }
    }
    false
}

/// Consequential tool calls that ran successfully, in order: `{kind, tool, idx, preview}`.
pub fn actions_done(events: &[Value]) -> Vec<Value> {
    let mut out: Vec<Value> = Vec::new();
    let mut purchase_at: Option<usize> = None;
    for (i, e) in events.iter().enumerate() {
        let d = data_of(e);
        let typ = get_str(e, "type");
        if typ == "tool_call" {
            continue;
        }
        // a call run after the user's approval reports status "ok" instead of ok=True
        if typ != "tool_result"
            || !(get_truthy(&d, "ok") || get_str(&d, "status") == "ok")
            || get_truthy(&d, "skipped")
        {
            continue;
        }
        let name = get_str(&d, "name");
        let prev = get_str(&d, "preview");
        let push = |out: &mut Vec<Value>, kind: &str| {
            out.push(json!({"kind": kind, "tool": name, "idx": i, "preview": prev}));
        };
        if SEND_TOOLS.contains(&name.as_str()) {
            push(&mut out, "send");
        } else if CALENDAR_TOOLS.contains(&name.as_str()) {
            push(&mut out, "calendar");
        } else if NOTION_TOOLS.contains(&name.as_str()) {
            push(&mut out, "notion");
        } else if CALL_TOOLS.contains(&name.as_str()) {
            push(&mut out, "call");
        } else if name == "purchase_confirm" && prev.contains("approved") {
            purchase_at = Some(i);
        } else if (name == "browser_click"
            || name == "browser_click_at"
            || name == "browser_request_takeover")
            && purchase_at.is_some()
        {
            // after the purchase was confirmed: the place-order / pay click — the agent's, or since that click is
            // always the user's, the takeover in which they press it (recorded once; the order number comes from the
            // pages read after the hand-back)
            if !out.iter().any(|a| get_str(a, "kind") == "purchase") {
                push(&mut out, "purchase");
            }
        }
    }
    out
}

/// `{status, actions: [...], evidence: [...], missing: [...]}` from the task's events and tool results alone — the
/// answer's words are never read (`_final`, `_goal` stay in the signature for the trust route). `tool_texts` = the full
/// tool results of the task (newest last); without them the event previews are used.
pub fn assess(
    events: &[Value],
    _final: &str,
    tool_texts: Option<&[String]>,
    _goal: Option<&str>,
) -> Value {
    static MID: LazyLock<Regex> =
        LazyLock::new(|| rx(r#"(?:message_?id|"id")\W{0,4}([A-Za-z0-9_\-]{8,})"#));
    static EID: LazyLock<Regex> =
        LazyLock::new(|| rx(r#"(?:event_?id|"id")\W{0,4}([A-Za-z0-9_\-@.]{6,})"#));
    static CID: LazyLock<Regex> = LazyLock::new(|| rx(r"(?:call_?id)\W{0,4}([A-Za-z0-9_\-]{6,})"));
    let acts = actions_done(events);
    let mut texts: Vec<String> = tool_texts.map(|t| t.to_vec()).unwrap_or_default();
    if texts.is_empty() {
        texts = events
            .iter()
            .filter(|e| get_str(e, "type") == "tool_result")
            .map(|e| get_str(&data_of(e), "preview"))
            .collect();
    }
    let mut evidence: Vec<Value> = Vec::new();
    let mut missing: Vec<String> = Vec::new();
    let first_cap =
        |r: &Regex, s: &str| r.captures(s).map(|c| c[1].to_string()).unwrap_or_default();
    for a in &acts {
        let kind = get_str(a, "kind");
        let tool = get_str(a, "tool");
        let preview = get_str(a, "preview");
        match kind.as_str() {
            "purchase" => {
                // proof = an order number a page read after the order click shows
                match confirmed_ref(&texts) {
                    Some(n) => evidence.push(json!({"kind": "purchase", "order_number": n})),
                    None => missing.push("purchase".to_string()),
                }
            }
            "send" => evidence
                .push(json!({"kind": "send", "tool": tool, "id": first_cap(&MID, &preview)})),
            "calendar" => evidence
                .push(json!({"kind": "calendar", "tool": tool, "id": first_cap(&EID, &preview)})),
            "call" => evidence.push(json!({"kind": "call", "id": first_cap(&CID, &preview)})),
            _ => evidence.push(json!({"kind": kind, "tool": tool})),
        }
    }
    let mut done_kinds: BTreeSet<String> = acts.iter().map(|a| get_str(a, "kind")).collect();
    // a booking made through an approved click on the booking site, or in a takeover the user handed back, with a
    // reference number on a page read afterwards
    if !done_kinds.contains("purchase") && !done_kinds.contains("booking") {
        let handed_back = events.iter().any(|e| {
            let d = data_of(e);
            get_str(e, "type") == "tool_result"
                && get_str(&d, "name") == "browser_request_takeover"
                && (get_truthy(&d, "ok") || get_str(&d, "status") == "ok")
        });
        if approved(events, "browser_click") || handed_back {
            if let Some(n) = confirmed_ref(&texts) {
                evidence.push(json!({"kind": "booking", "reference": n}));
                done_kinds.insert("booking".to_string());
            }
        }
    }
    let status = if !missing.is_empty() {
        "unverified"
    } else if !acts.is_empty() || done_kinds.contains("booking") {
        "verified"
    } else {
        "none"
    };
    json!({
        "status": status,
        "actions": acts.iter().map(|a| json!({"kind": get_str(a, "kind"), "tool": get_str(a, "tool")})).collect::<Vec<_>>(),
        "evidence": evidence,
        "missing": missing,
    })
}

/// A short line added under the answer when an action ran without its proof (a purchase with no order number on any
/// page read afterwards) — from the task's events and pages, never from words.
pub fn notice(result: &Value, zh: bool) -> String {
    if get_str(result, "status") != "unverified" {
        return String::new();
    }
    if zh {
        return "\n\n> ⚠️ 结果未经确认：没有找到能证明这一步已完成的订单号或确认信息，请到对应网站或邮箱核对。".to_string();
    }
    "\n\n> ⚠️ Not confirmed: no order number or confirmation was found that proves this step was completed — please check on the site or in your email.".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refs_with_markdown_bold_on_label_and_number() {
        for t in [
            "- **Booking reference / order number:** **TRV-FBB71D4**",
            "- **Booking reference:** **TRV-FBB71D4**",
            "Booking reference: **TRV-FBB71D4**",
            "**Order number**: TRV-FBB71D4",
            "Booking reference TRV-FBB71D4",
        ] {
            assert_eq!(refs_in(t), ["TRV-FBB71D4"], "{t}");
        }
        assert!(refs_in("**Booking reference:** **pending**").is_empty());
    }

    fn ev(typ: &str, d: Value) -> Value {
        json!({"type": typ, "data": d})
    }

    fn texts(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn outcome_needs_proof_from_the_pages_not_from_the_answer() {
        // "done" needs evidence — an order number a page shows after the order click, a message id, an event id;
        // the answer's words are never read
        let buy = vec![
            ev(
                "tool_call",
                json!({"call_id": "c1", "name": "purchase_confirm"}),
            ),
            ev(
                "waiting",
                json!({"type": "approval", "approval_id": "a1", "summary": {"tool": "purchase_confirm"}}),
            ),
            ev(
                "approval_resolved",
                json!({"approval_id": "a1", "decision": "approved"}),
            ),
            ev(
                "tool_result",
                json!({"call_id": "c1", "name": "purchase_confirm", "status": "ok", "preview": "{\"status\": \"approved\", \"purchase_id\": \"buy_1\"}"}),
            ),
            ev(
                "tool_call",
                json!({"call_id": "c2", "name": "browser_fill_secret"}),
            ),
            ev(
                "tool_result",
                json!({"call_id": "c2", "name": "browser_fill_secret", "ok": true, "preview": "filled"}),
            ),
            ev(
                "tool_call",
                json!({"call_id": "c3", "name": "browser_click"}),
            ),
            ev(
                "tool_result",
                json!({"call_id": "c3", "name": "browser_click", "ok": true, "preview": "web page https://www.decathlon.sg/order-confirmation"}),
            ),
        ];
        let page =
            "Thank you for your order! Order number SG5X42VMKCAG. Estimated delivery Wed 7 Oct";
        let r = assess(
            &buy,
            "whatever the model wrote",
            Some(&texts(&[page])),
            None,
        );
        assert_eq!(r["status"], json!("verified"));
        assert_eq!(r["evidence"][0]["order_number"], json!("SG5X42VMKCAG"));
        assert_eq!(notice(&r, true), "");
        // the final click is the user's: they press Place order in a takeover; the confirmation read after the
        // hand-back is the proof, and the order is recorded the same way
        let mut handed = buy[..6].to_vec();
        handed.push(ev(
            "tool_result",
            json!({"call_id": "c4", "name": "browser_request_takeover", "ok": true, "preview": "the user handed the browser back"}),
        ));
        let r = assess(&handed, "", Some(&texts(&[page])), None);
        assert_eq!(r["status"], json!("verified"));
        assert_eq!(r["evidence"][0]["order_number"], json!("SG5X42VMKCAG"));
        // no page shows a number: unverified, whatever the answer claims
        let r = assess(
            &buy,
            "下单成功！订单号：SG77777777",
            Some(&texts(&["Processing…"])),
            None,
        );
        assert_eq!(r["status"], json!("unverified"));
        assert_eq!(r["missing"], json!(["purchase"]));
        assert!(notice(&r, true).contains("未经确认"));
        assert!(notice(&r, false).contains("Not confirmed"));
        // nothing done: no claim is read, the status is none
        let r = assess(
            &[ev(
                "tool_result",
                json!({"name": "browser_read", "ok": true, "preview": "menu"}),
            )],
            "已预订周六 7 点 2 位。",
            Some(&[]),
            None,
        );
        assert_eq!(r["status"], json!("none"));
        // a sent email / calendar event carry their ids
        let sent = vec![ev(
            "tool_result",
            json!({"name": "gmail_send", "ok": true, "preview": "{\"id\": \"18c2f0a9b1d2e3f4\", \"status\": \"sent\"}"}),
        )];
        let r = assess(&sent, "邮件已发送给 Jennifer。", Some(&[]), None);
        assert_eq!(r["status"], json!("verified"));
        assert_eq!(r["evidence"][0]["id"], json!("18c2f0a9b1d2e3f4"));
        let cal = vec![ev(
            "tool_result",
            json!({"name": "calendar_create_event", "ok": true, "preview": "{\"event_id\": \"evt_abc123\", \"title\": \"x\"}"}),
        )];
        assert_eq!(
            assess(&cal, "已添加到日历。", Some(&[]), None)["evidence"][0]["id"],
            json!("evt_abc123")
        );
        // a refused send is not a send
        assert_eq!(
            assess(
                &[ev(
                    "tool_result",
                    json!({"name": "gmail_send", "ok": false, "preview": "denied"})
                )],
                "邮件已发送。",
                Some(&[]),
                None
            )["status"],
            json!("none")
        );
        assert_eq!(
            refs_in("Booking reference: ABC123, order no. 98-7654"),
            vec!["ABC123", "98-7654"]
        );
        // a booking through an approved click with a reference the page showed
        let book = vec![
            ev(
                "waiting",
                json!({"type": "approval", "approval_id": "b1", "tool": "browser_click"}),
            ),
            ev(
                "approval_resolved",
                json!({"approval_id": "b1", "decision": "approved"}),
            ),
            ev(
                "tool_result",
                json!({"name": "browser_click", "ok": true, "preview": "Reservation confirmed. Confirmation code: CHOPE8821"}),
            ),
        ];
        let r = assess(&book, "", None, Some("帮我订周六晚上的餐厅"));
        assert_eq!(r["status"], json!("verified"));
        assert_eq!(r["evidence"][0]["reference"], json!("CHOPE8821"));
    }

    #[test]
    fn the_reference_comes_from_the_newest_page_that_shows_one() {
        // shopping_real 2026-10-31: "Order reference: KM-50731" on the confirmation page
        let pages = texts(&[
            "Checkout — Order summary S$24.90",
            "THANK YOU! Order reference: KM-50731. DELIVERY 2026-11-03, S$24.90",
        ]);
        assert_eq!(confirmed_ref(&pages).as_deref(), Some("KM-50731"));
        assert_eq!(
            confirmed_ref(&texts(&["Processing… 2026-11-03 24.90"])),
            None
        );
        assert_eq!(refs_in("Order number: SG5X42VMKCAG"), vec!["SG5X42VMKCAG"]);
    }
}
