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

use omuse_common::util::{get_str, get_truthy, last_chars, py_str, truthy};
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

/// "I did it" in an answer. Kept to past-tense / result wording so "I can book it for you" is not a claim.
static CLAIM: LazyLock<Vec<(&'static str, Regex)>> = LazyLock::new(|| {
    vec![
        (
            "purchase",
            rx(
                r"(?i)下单成功|已下单|已经下单|订单已提交|已完成(购买|支付|付款)|支付成功|付款成功|购买成功|order (has been |was )?(placed|confirmed|submitted)|purchase (is )?complete|payment (was )?successful",
            ),
        ),
        (
            "booking",
            rx(
                r"(?i)预订成功|已预订|已经预订|订好了|已订好|预约成功|已预约|\b(booked|reserved)\b|booking (is )?confirmed|reservation (is )?confirmed",
            ),
        ),
        (
            "send",
            rx(
                r"(?i)已发送|已经发送|已发出|已回复|已转发|邮件已发|\b(sent|replied|forwarded) (the |your |an? )?(e-?mail|message|reply)",
            ),
        ),
        (
            "calendar",
            rx(
                r"(?i)已(添加|加入|创建|建立|更新|修改|删除|取消)[^。\n]{0,12}(日历|日程|事件|会议)|(日历|日程)[^。\n]{0,8}已(添加|创建|更新|删除)|(added|created|updated|deleted) (it |the event )?(to|on|in|from) (your )?calendar",
            ),
        ),
        (
            "call",
            rx(r"(?i)已(经)?(拨打|打了|打过)(电话)?|电话已(打|拨)|\bI (called|phoned)\b"),
        ),
    ]
});
/// Not a claim when the sentence says it did NOT happen.
static NEGATED: LazyLock<Regex> = LazyLock::new(|| {
    rx(r"(?i)(没有|未|尚未|还没|无法|不能|并未|没能|失败|not|n't|unable|couldn't|failed)")
});

/// An order / booking / confirmation number as people write it next to its label.
static REF: LazyLock<Regex> = LazyLock::new(|| {
    rx(
        r"(?i)(?:订单号|订单编号|订单|预订号|确认号|确认码|预约号|参考号|order\s*(?:no\.?|number|#|id)?|booking\s*(?:ref(?:erence)?|no\.?|number|id|code)?|confirmation\s*(?:no\.?|number|code|#)?|reference\s*(?:no\.?|number)?|ref\.?)[\s*]*[:：#]?[\s*]*([A-Z0-9][A-Z0-9\-]{4,24})",
    )
});

/// What the request asks to have done: an answer's claim only counts for these (a summary of emails saying "Jennifer has
/// replied" is not OMuse claiming it sent something).
static INTENT: LazyLock<Vec<(&'static str, Regex)>> = LazyLock::new(|| {
    vec![
        (
            "purchase",
            rx(r"(?i)买|购买|下单|订购|结账|付款|\b(buy|order|purchase|checkout|pay)\b"),
        ),
        (
            "booking",
            rx(
                r"(?i)预订|订位|订座|订餐|订票|订房|预约|订[^，。,.]{0,10}(餐厅|位子|座位|酒店|机票|门票|桌)|\b(book|reserve|reservation)\b",
            ),
        ),
        (
            "send",
            rx(
                r"(?i)发(送|给|一封|封|邮件|消息|信)|回复|回信|转发|\b(send|reply|forward|e-?mail)\b",
            ),
        ),
        (
            "calendar",
            rx(r"(?i)日历|日程|会议|提醒我|\b(calendar|schedule|meeting|event)\b"),
        ),
        ("call", rx(r"(?i)打(个)?电话|致电|拨打|\b(call|phone)\b")),
    ]
});

/// The consequential things a request asks for.
pub fn intents(goal: &str) -> BTreeSet<String> {
    INTENT
        .iter()
        .filter(|(_, r)| r.is_match(goal))
        .map(|(k, _)| k.to_string())
        .collect()
}

fn claims(final_: &str) -> BTreeSet<String> {
    static SENT: LazyLock<Regex> = LazyLock::new(|| rx(r"[\n。！!？?]"));
    let mut out = BTreeSet::new();
    for line in SENT.split(final_) {
        for (kind, r) in CLAIM.iter() {
            if let Some(m) = r.find(line) {
                if !NEGATED.is_match(last_chars(&line[..m.start()], 12)) {
                    out.insert(kind.to_string());
                }
            }
        }
    }
    out
}

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
        } else if (name == "browser_click" || name == "browser_click_at") && purchase_at.is_some() {
            // a click after the user confirmed the purchase: the place-order / pay click (recorded once)
            if !out.iter().any(|a| get_str(a, "kind") == "purchase") {
                push(&mut out, "purchase");
            }
        }
    }
    out
}

/// `{status, actions: [...], claims: [...], evidence: [...], missing: [...]}`. `tool_texts` = the full tool results of
/// the task (newest last); without them the event previews are used.
pub fn assess(
    events: &[Value],
    final_: &str,
    tool_texts: Option<&[String]>,
    goal: Option<&str>,
) -> Value {
    static MID: LazyLock<Regex> =
        LazyLock::new(|| rx(r#"(?:message_?id|"id")\W{0,4}([A-Za-z0-9_\-]{8,})"#));
    static EID: LazyLock<Regex> =
        LazyLock::new(|| rx(r#"(?:event_?id|"id")\W{0,4}([A-Za-z0-9_\-@.]{6,})"#));
    static CID: LazyLock<Regex> = LazyLock::new(|| rx(r"(?:call_?id)\W{0,4}([A-Za-z0-9_\-]{6,})"));
    let acts = actions_done(events);
    let mut claims = claims(final_);
    if let Some(goal) = goal {
        // only what the request asked for, or what the task actually did
        let mut allowed = intents(goal);
        allowed.extend(acts.iter().map(|a| get_str(a, "kind")));
        claims.retain(|c| allowed.contains(c));
    }
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
                // proof = an order number the answer gives that a page after the order click actually shows
                let seen = texts.join("\n").to_uppercase();
                let nums: Vec<String> = refs_in(final_)
                    .into_iter()
                    .filter(|r| seen.contains(&r.to_uppercase()))
                    .collect();
                if let Some(n) = nums.first() {
                    evidence.push(json!({"kind": "purchase", "order_number": n}));
                } else {
                    missing.push("purchase".to_string());
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
    // a booking is done through an approved click on the booking site: accept it when the answer carries a reference
    // number that a page showed
    if claims.contains("booking") && !done_kinds.contains("booking") {
        let seen = texts.join("\n").to_uppercase();
        let nums: Vec<String> = refs_in(final_)
            .into_iter()
            .filter(|r| seen.contains(&r.to_uppercase()))
            .collect();
        if !nums.is_empty() && approved(events, "browser_click") {
            evidence.push(json!({"kind": "booking", "reference": nums[0]}));
            done_kinds.insert("booking".to_string());
        }
    }
    let claimed_not_done: Vec<&String> = claims
        .iter()
        .filter(|k| {
            !done_kinds.contains(*k) && !(*k == "booking" && done_kinds.contains("purchase"))
        })
        .collect();
    missing.extend(claimed_not_done.iter().map(|k| format!("{}:not_done", k)));
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
        "claims": claims.iter().collect::<Vec<_>>(),
        "evidence": evidence,
        "missing": missing,
    })
}

fn missing_of(result: &Value) -> Vec<String> {
    result
        .get("missing")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .map(|m| match m {
                    Value::String(s) => s.clone(),
                    o => py_str(o),
                })
                .collect()
        })
        .unwrap_or_default()
}

/// What to tell the model before it finishes, when the proof is missing (`""` = nothing to fix).
pub fn nudge(result: &Value, zh: bool) -> String {
    if get_str(result, "status") != "unverified" {
        return String::new();
    }
    let missing = missing_of(result);
    let mut parts: Vec<String> = Vec::new();
    if missing.iter().any(|m| m == "purchase") {
        parts.push(if zh {
            "下单之后没有拿到订单号：用 browser_snapshot 读一下现在的页面（确认页），把页面上的订单号原样写进回答；如果页面没有显示订单号或下单没有成功，就如实说「未能确认订单是否提交」，不要说下单成功。".to_string()
        } else {
            "After the order click there is no order number yet: browser_snapshot the page you are on (the confirmation page) and copy its order number into the answer as shown; if the page shows none or the order failed, say plainly that the order could not be confirmed — do not say it was placed.".to_string()
        });
    }
    let nd: Vec<String> = missing
        .iter()
        .filter(|m| m.ends_with(":not_done"))
        .map(|m| m.split(':').next().unwrap_or("").to_string())
        .collect();
    if !nd.is_empty() {
        let what = |k: &str| {
            match k {
                "purchase" => "下单",
                "booking" => "预订",
                "send" => "发送",
                "calendar" => "修改日历",
                "call" => "打电话",
                other => other,
            }
            .to_string()
        };
        let what_en = |k: &str| {
            match k {
                "purchase" => "placed an order",
                "booking" => "made a booking",
                "send" => "sent a message",
                "calendar" => "changed the calendar",
                "call" => "made a call",
                other => other,
            }
            .to_string()
        };
        parts.push(if zh {
            format!(
                "你的回答说已经{}，但这个任务里没有成功执行过对应的操作。改写回答：只写实际完成了什么、停在了哪一步。",
                nd.iter().map(|k| what(k)).collect::<Vec<_>>().join("、")
            )
        } else {
            format!(
                "Your answer says you {}, but no such action ran successfully in this task. Rewrite the answer: say only what was actually done and where it stopped.",
                nd.iter().map(|k| what_en(k)).collect::<Vec<_>>().join(", ")
            )
        });
    }
    format!(
        "{}{}",
        if zh { "（系统）" } else { "(System) " },
        parts.join(" ")
    )
}

/// A short line added under the answer when the proof is still missing after the nudge.
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
    fn outcome_needs_proof() {
        // roadmap batch 1: "done" needs evidence — an order number the confirmation page shows, a message id, an event id
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
        let good = "下单成功！\n- **订单号**：SG5X42VMKCAG\n- 总价 S$6.80";
        let r = assess(&buy, good, Some(&texts(&[page])), None);
        assert_eq!(r["status"], json!("verified"));
        assert_eq!(r["evidence"][0]["order_number"], json!("SG5X42VMKCAG"));
        assert_eq!(notice(&r, true), "");
        let made_up = "下单成功！订单号：SG77777777"; // a number no page showed
        let r = assess(&buy, made_up, Some(&texts(&[page])), None);
        assert_eq!(r["status"], json!("unverified"));
        assert!(missing_of(&r).contains(&"purchase".to_string()));
        assert!(nudge(&r, true).contains("browser_snapshot"));
        assert!(nudge(&r, true).starts_with("（系统）下单之后"));
        assert!(nudge(&r, false).starts_with("(System) After the order click"));
        assert!(notice(&r, true).contains("未经确认"));
        assert!(notice(&r, false).contains("Not confirmed"));
        let honest = "下单按钮点了，但页面没有显示订单号，未能确认订单是否提交。";
        assert_eq!(
            assess(&buy, honest, Some(&texts(&["Processing…"])), None)["status"],
            json!("unverified")
        ); // the purchase still lacks proof
           // claims with nothing done
        let r = assess(
            &[ev(
                "tool_result",
                json!({"name": "browser_read", "ok": true, "preview": "menu"}),
            )],
            "已预订周六 7 点 2 位。",
            Some(&[]),
            None,
        );
        assert_eq!(r["status"], json!("unverified"));
        assert_eq!(r["missing"], json!(["booking:not_done"]));
        assert!(nudge(&r, true).contains("没有成功执行"));
        assert!(nudge(&r, true).contains("你的回答说已经预订，"));
        assert!(nudge(&r, false).contains("Your answer says you made a booking, but"));
        assert_eq!(
            assess(&[], "我可以帮你预订，要我继续吗？", Some(&[]), None)["status"],
            json!("none")
        );
        assert_eq!(
            assess(&[], "没有发送邮件，草稿在下面。", Some(&[]), None)["status"],
            json!("none")
        );
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
            json!("unverified")
        );
        assert_eq!(
            refs_in("Booking reference: ABC123, order no. 98-7654"),
            vec!["ABC123", "98-7654"]
        );
        // only what the request asked for counts as a claim
        let mail = "这周重要邮件：1. Jennifer 已回复了合同修改；2. 银行已发送对账单。";
        assert_eq!(
            assess(&[], mail, Some(&[]), Some("看看这周有哪些重要邮件"))["status"],
            json!("none")
        );
        assert_eq!(
            assess(
                &[],
                "已预订周六 7 点。",
                Some(&[]),
                Some("帮我订周六晚上的餐厅")
            )["status"],
            json!("unverified")
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
        let r = assess(
            &book,
            "已预订成功，确认码 CHOPE8821。",
            None,
            Some("帮我订周六晚上的餐厅"),
        );
        assert_eq!(r["status"], json!("verified"));
        assert_eq!(r["evidence"][0]["reference"], json!("CHOPE8821"));
        assert_eq!(r["claims"], json!(["booking"]));
    }
}
