//! Transaction ledger (roadmap batch 3): every purchase OMuse made for the user — merchant, order number, amount, card
//! (masked), status and the proof behind each status — so "cancel / return / where is my order" always points at a real
//! order (2026-10-04: a follow-up about the Decathlon order went to an Amazon order instead).
//!
//! Life of an entry:
//! ```text
//!   approved          the user approved the purchase card (purchase_confirm) — nothing placed yet
//!   placed            the task ended with an order number the confirmation page showed
//!   unconfirmed       the task ended without an order number (check the merchant's site / email)
//!   shipped / delivered / cancelled / refunded     from the merchant's emails (reconcile) or the merchant's page
//!   cancel_requested / return_requested            OMuse clicked cancel / return for it, approved by the user
//! ```

use std::sync::LazyLock;

use anyhow::Result;
use regex::Regex;
use serde_json::{json, Map, Value};

use omuse_common::params;
use omuse_common::pyjson::dumps;
use omuse_common::util::{get_str, loads, new_id, now_ts, take_chars, truthy};

use crate::store::{field_f64, field_str, loads_field, Store};

/// Statuses of orders that are still "in flight".
pub const OPEN: &[&str] = &[
    "approved",
    "placed",
    "shipped",
    "unconfirmed",
    "cancel_requested",
    "return_requested",
];

/// Bilingual status label (`status` itself when unknown).
pub fn label(status: &str) -> String {
    match status {
        "approved" => "已批准，未下单 approved",
        "placed" => "已下单 placed",
        "unconfirmed" => "未确认 unconfirmed",
        "shipped" => "已发货 shipped",
        "delivered" => "已送达 delivered",
        "cancel_requested" => "已申请取消 cancel requested",
        "return_requested" => "已申请退货 return requested",
        "cancelled" => "已取消 cancelled",
        "refunded" => "已退款 refunded",
        other => other,
    }
    .to_string()
}

// what a merchant's email says, most final first
static MAIL_STATUS: LazyLock<Vec<(&'static str, Regex)>> = LazyLock::new(|| {
    vec![
        ("refunded", Regex::new(r"(?i)refund(ed| (has been |was )?(issued|processed|completed))|已退款|退款(已|成功|完成)").unwrap()),
        (
            "cancelled",
            Regex::new(r"(?i)(order|booking)\b.{0,40}\b(has been |was |is )?cancel(l)?ed|cancellation confirmed|订单已取消|已取消")
                .unwrap(),
        ),
        ("delivered", Regex::new(r"(?i)\bdelivered\b|has arrived|已送达|已签收|派送完成").unwrap()),
        (
            "shipped",
            Regex::new(r"(?i)\b(shipped|dispatched|on (its|the) way|out for delivery|in transit)\b|已发货|已出库|派送中|运输中").unwrap(),
        ),
        (
            "placed",
            Regex::new(r"(?i)order (confirmation|confirmed|received)|thank(s| you) for your (order|purchase)|订单确认|已收到(你|您)的订单")
                .unwrap(),
        ),
    ]
});

fn rank_or(status: &str, default: i64) -> i64 {
    match status {
        "approved" => 0,
        "unconfirmed" => 1,
        "placed" => 2,
        "cancel_requested" | "return_requested" => 3,
        "shipped" => 4,
        "delivered" => 5,
        "cancelled" => 6,
        "refunded" => 7,
        _ => default,
    }
}

fn rank(status: &str) -> i64 {
    rank_or(status, 0)
}

fn row(r: Option<Value>) -> Option<Value> {
    let mut r = r?;
    for k in ["items", "evidence", "history"] {
        let v = loads_field(&r, k, json!([]));
        let v = if truthy(&v) { v } else { json!([]) };
        r[k] = v;
    }
    let status = field_str(&r, "status");
    r["status_label"] = json!(label(&status));
    Some(r)
}

/// A new `approved` entry (the user just approved the purchase card).
#[allow(clippy::too_many_arguments)]
pub fn add(
    store: &Store,
    task_id: &str,
    merchant: &str,
    total: f64,
    currency: &str,
    items: Option<&Value>,
    card: &str,
    delivery: &str,
    purchase_id: &str,
    approval_id: &str,
) -> Result<Value> {
    let lid = new_id("ord");
    let now = now_ts();
    let items = match items {
        Some(v) if truthy(v) => v.clone(),
        _ => json!([]),
    };
    store.db.insert(
        "ledger",
        &json!({
            "id": lid, "task_id": task_id, "purchase_id": purchase_id, "approval_id": approval_id,
            "merchant": merchant, "order_number": "", "items": dumps(&items),
            "total": total, "currency": currency, "card": card,
            "delivery": delivery, "status": "approved", "evidence": "[]",
            "history": dumps(&json!([{"ts": now, "status": "approved", "by": "user",
                                      "note": format!("批准购买 approval {approval_id}").trim()}])),
            "created_at": now, "updated_at": now,
        }),
    )?;
    Ok(get(store, &lid)?.unwrap_or(Value::Null))
}

/// One entry (lists parsed, `status_label` added) or `None`.
pub fn get(store: &Store, lid: &str) -> Result<Option<Value>> {
    Ok(row(store
        .db
        .one("SELECT * FROM ledger WHERE id=?", params![lid])?))
}

/// Entries, newest first; `status == Some("open")` keeps only [`OPEN`] ones.
pub fn entries(store: &Store, limit: i64, status: Option<&str>) -> Result<Vec<Value>> {
    let rows = if status == Some("open") {
        let q = format!(
            "SELECT * FROM ledger WHERE status IN ({}) ORDER BY created_at DESC LIMIT ?",
            vec!["?"; OPEN.len()].join(",")
        );
        let mut p: Vec<omuse_common::db::Param> = OPEN.iter().map(|s| (*s).into()).collect();
        p.push(limit.into());
        store.db.all(&q, p)?
    } else {
        store.db.all(
            "SELECT * FROM ledger ORDER BY created_at DESC LIMIT ?",
            params![limit],
        )?
    };
    Ok(rows.into_iter().filter_map(|r| row(Some(r))).collect())
}

/// Entries of one task, oldest first.
pub fn for_task(store: &Store, task_id: &str) -> Result<Vec<Value>> {
    let rows = store.db.all(
        "SELECT * FROM ledger WHERE task_id=? ORDER BY created_at",
        params![task_id],
    )?;
    Ok(rows.into_iter().filter_map(|r| row(Some(r))).collect())
}

static NON_WORD: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\W").unwrap());
static SPACE_DASH: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[\s\-]").unwrap());

/// The entry with this order number (punctuation / case insensitive), or `None`.
pub fn by_number(store: &Store, number: &str) -> Result<Option<Value>> {
    let n = NON_WORD.replace_all(number, "").to_uppercase();
    if n.chars().count() < 5 {
        return Ok(None);
    }
    for r in store
        .db
        .all("SELECT * FROM ledger WHERE order_number!=''", params![])?
    {
        if NON_WORD
            .replace_all(&field_str(&r, "order_number"), "")
            .to_uppercase()
            == n
        {
            return Ok(row(Some(r)));
        }
    }
    Ok(None)
}

/// Ledger orders whose number appears on a page / in an email.
pub fn in_text(store: &Store, text: &str) -> Result<Vec<Value>> {
    let t = SPACE_DASH.replace_all(text, "").to_uppercase();
    let mut out = Vec::new();
    for r in store
        .db
        .all("SELECT * FROM ledger WHERE order_number!=''", params![])?
    {
        let n = SPACE_DASH
            .replace_all(&field_str(&r, "order_number"), "")
            .to_uppercase();
        if n.chars().count() >= 5 && t.contains(&n) {
            if let Some(v) = row(Some(r)) {
                out.push(v);
            }
        }
    }
    Ok(out)
}

/// Change the status (with history + optional evidence); `only_forward` never moves it backwards.
pub fn set_status(
    store: &Store,
    lid: &str,
    status: &str,
    by: &str,
    note: &str,
    evidence: Option<&Value>,
    only_forward: bool,
) -> Result<Option<Value>> {
    let Some(r) = get(store, lid)? else {
        return Ok(None);
    };
    let cur = field_str(&r, "status");
    let status = if only_forward && rank(status) <= rank(&cur) {
        cur.as_str()
    } else {
        status
    };
    let now = now_ts();
    let mut hist = r["history"].as_array().cloned().unwrap_or_default();
    hist.push(json!({"ts": now, "status": status, "by": by, "note": take_chars(note, 300)}));
    let mut ev = r["evidence"].as_array().cloned().unwrap_or_default();
    if let Some(e) = evidence.filter(|e| truthy(e)) {
        let mut m: Map<String, Value> = e.as_object().cloned().unwrap_or_default();
        m.insert("ts".into(), json!(now));
        ev.push(Value::Object(m));
    }
    let hist: Vec<Value> = hist.into_iter().rev().take(40).rev().collect();
    let ev: Vec<Value> = ev.into_iter().rev().take(40).rev().collect();
    store.db.update(
        "ledger",
        "id",
        lid,
        &json!({"status": status, "history": dumps(&json!(hist)), "evidence": dumps(&json!(ev)), "updated_at": now}),
    )?;
    get(store, lid)
}

/// The purchase task finished: record the order number it got (or that it got none).
pub fn confirm(
    store: &Store,
    task_id: &str,
    order_number: &str,
    total: Option<f64>,
    evidence: Option<&Value>,
) -> Result<Vec<Value>> {
    let mut out = Vec::new();
    for r in for_task(store, task_id)? {
        if field_str(&r, "status") != "approved" {
            continue;
        }
        let mut upd = Map::new();
        if !order_number.is_empty() {
            upd.insert(
                "order_number".into(),
                json!(take_chars(order_number.trim(), 40)),
            );
        }
        if let Some(t) = total {
            upd.insert("total".into(), json!(t));
        }
        let lid = field_str(&r, "id");
        if !upd.is_empty() {
            store
                .db
                .update("ledger", "id", lid.as_str(), &Value::Object(upd))?;
        }
        let (status, note) = if order_number.is_empty() {
            (
                "unconfirmed",
                "任务结束时没有拿到订单号 no order number".to_string(),
            )
        } else {
            ("placed", format!("订单号 {order_number}"))
        };
        if let Some(v) = set_status(store, &lid, status, "omuse", &note, evidence, false)? {
            out.push(v);
        }
    }
    Ok(out)
}

/// The order status a merchant's email announces (`""` when it is not a status mail).
pub fn status_from_mail(subject: &str, snippet: &str) -> String {
    let text = format!("{subject}\n{snippet}");
    for (st, rx) in MAIL_STATUS.iter() {
        if rx.is_match(&text) {
            return st.to_string();
        }
    }
    String::new()
}

/// Match merchants' emails to open orders by order number. `search(query) -> [{id, subject, snippet, from, date}]`
/// (a failing search skips that order). Returns `{"checked": n, "changed": [{id, order_number, from, to}]}`.
pub fn reconcile(
    store: &Store,
    search: &mut dyn FnMut(&str) -> Result<Vec<Value>>,
) -> Result<Value> {
    let mut checked = 0i64;
    let mut changed = Vec::new();
    for r in entries(store, 200, Some("open"))? {
        let number = field_str(&r, "order_number");
        if number.is_empty() {
            continue;
        }
        checked += 1;
        let msgs = match search(&format!("\"{number}\" newer_than:90d")) {
            Ok(m) => m,
            Err(_) => continue,
        };
        let mut best = String::new();
        let mut best_msg: Option<&Value> = None;
        for m in &msgs {
            let st = status_from_mail(&get_str(m, "subject"), &get_str(m, "snippet"));
            if !st.is_empty() && rank(&st) > rank_or(&best, -1) {
                best = st;
                best_msg = Some(m);
            }
        }
        let cur = field_str(&r, "status");
        if !best.is_empty() && rank(&best) > rank(&cur) {
            let m = best_msg.expect("message");
            let ev = json!({
                "kind": "email", "id": get_str(m, "id"), "subject": take_chars(&get_str(m, "subject"), 200),
                "from": take_chars(&get_str(m, "from"), 120), "date": get_str(m, "date"),
            });
            let note = format!("邮件：{}", get_str(&ev, "subject"));
            set_status(
                store,
                &field_str(&r, "id"),
                &best,
                "email",
                &note,
                Some(&ev),
                false,
            )?;
            changed.push(json!({"id": r["id"], "order_number": number, "from": cur, "to": best}));
        }
    }
    Ok(json!({"checked": checked, "changed": changed}))
}

/// Ledger entries for purchases approved before the ledger existed (from the one-card purchase records).
pub fn backfill(store: &Store) -> Result<Vec<String>> {
    let mut made = Vec::new();
    for p in store
        .db
        .all("SELECT * FROM purchases ORDER BY created_at", params![])?
    {
        let pid = field_str(&p, "id");
        let task_id = field_str(&p, "task_id");
        if store
            .db
            .one(
                "SELECT id FROM ledger WHERE purchase_id=? OR (task_id=? AND task_id!='')",
                params![&pid, &task_id],
            )?
            .is_some()
        {
            continue;
        }
        let summ = loads(p.get("summary").and_then(Value::as_str), json!({}));
        let summ = if truthy(&summ) { summ } else { json!({}) };
        let ap = store.db.one(
            "SELECT id FROM approvals WHERE task_id=? AND tool='purchase_confirm' AND status='approved' ORDER BY resolved_at DESC",
            params![&task_id],
        )?;
        let items = summ
            .get("items")
            .filter(|v| truthy(v))
            .cloned()
            .unwrap_or_else(|| json!([]));
        let r = add(
            store,
            &task_id,
            &field_str(&p, "domain"),
            field_f64(&p, "max_total"),
            &field_str(&p, "currency"),
            Some(&items),
            "",
            &get_str(&summ, "delivery"),
            &pid,
            &ap.map(|a| field_str(&a, "id")).unwrap_or_default(),
        )?;
        let lid = field_str(&r, "id");
        store.db.update(
            "ledger",
            "id",
            lid.as_str(),
            &json!({"created_at": p["created_at"]}),
        )?;
        made.push(lid);
    }
    Ok(made)
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::anyhow;

    #[test]
    fn follows_the_order() {
        let d = tempfile::tempdir().unwrap();
        let store = Store::new(d.path()).unwrap();
        let r = add(
            &store,
            "t1",
            "decathlon.sg",
            9.9,
            "SGD",
            Some(&json!([{"name": "socks"}])),
            "Visa •6411",
            "Home delivery",
            "buy_1",
            "appr_1",
        )
        .unwrap();
        assert_eq!(r["status"], "approved");
        assert_eq!(r["history"][0]["by"], "user");
        assert_eq!(r["history"][0]["note"], "批准购买 approval appr_1");
        assert_eq!(r["status_label"], "已批准，未下单 approved");
        let rid = r["id"].as_str().unwrap().to_string();
        confirm(
            &store,
            "t1",
            "SG5X42VMKCAG",
            None,
            Some(&json!({"kind": "confirmation", "text": "Order SG5X42VMKCAG"})),
        )
        .unwrap();
        let r = get(&store, &rid).unwrap().unwrap();
        assert_eq!(r["status"], "placed");
        assert_eq!(r["order_number"], "SG5X42VMKCAG");
        assert_eq!(r["evidence"][0]["kind"], "confirmation");
        assert_eq!(
            by_number(&store, "sg5x-42vmkcag").unwrap().unwrap()["id"],
            rid
        );
        assert!(by_number(&store, "sg5").unwrap().is_none());
        let found: Vec<String> = in_text(&store, "Your order SG5X 42VMKCAG is on its way")
            .unwrap()
            .iter()
            .map(|x| x["id"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(found, vec![rid.clone()]);
        assert!(in_text(&store, "Order 113-555 from Amazon")
            .unwrap()
            .is_empty());
        assert_eq!(
            status_from_mail("Your Decathlon order SG5X42VMKCAG has been shipped", ""),
            "shipped"
        );
        assert_eq!(
            status_from_mail("订单 SG5X42VMKCAG 已签收", ""),
            "delivered"
        );
        assert_eq!(
            status_from_mail("Refund processed for order SG5X42VMKCAG", ""),
            "refunded"
        );
        assert_eq!(status_from_mail("Weekly deals just for you", ""), "");
        let mails = json!([{"id": "m1", "subject": "Thank you for your order SG5X42VMKCAG"},
                           {"id": "m2", "subject": "Your order SG5X42VMKCAG has been dispatched"}]);
        let mut search = |q: &str| -> Result<Vec<Value>> {
            Ok(if q.contains("SG5X42VMKCAG") {
                mails.as_array().unwrap().clone()
            } else {
                vec![]
            })
        };
        let res = reconcile(&store, &mut search).unwrap();
        assert_eq!(res["checked"], 1);
        assert_eq!(
            res["changed"],
            json!([{"id": rid, "order_number": "SG5X42VMKCAG", "from": "placed", "to": "shipped"}])
        );
        let r = get(&store, &rid).unwrap().unwrap();
        assert_eq!(r["status"], "shipped");
        assert_eq!(
            r["evidence"].as_array().unwrap().last().unwrap()["id"],
            "m2"
        );
        // an older "order confirmed" email never moves it backwards
        let mut older = |_q: &str| -> Result<Vec<Value>> {
            Ok(vec![
                json!({"id": "m1", "subject": "Order confirmed SG5X42VMKCAG"}),
            ])
        };
        assert_eq!(reconcile(&store, &mut older).unwrap()["changed"], json!([]));
        let mut failing = |_q: &str| -> Result<Vec<Value>> { Err(anyhow!("imap down")) };
        assert_eq!(
            reconcile(&store, &mut failing).unwrap(),
            json!({"checked": 1, "changed": []})
        );
        // a purchase task that ended without an order number
        let r2 = add(&store, "t2", "shop.test", 5.0, "SGD", None, "", "", "", "").unwrap();
        assert_eq!(r2["history"][0]["note"], "批准购买 approval");
        confirm(&store, "t2", "", None, None).unwrap();
        assert_eq!(
            get(&store, r2["id"].as_str().unwrap()).unwrap().unwrap()["status"],
            "unconfirmed"
        );
        assert_eq!(entries(&store, 10, Some("open")).unwrap().len(), 2);
        assert_eq!(for_task(&store, "t2").unwrap().len(), 1);
        // only_forward keeps the later status
        let r2 = set_status(
            &store,
            r2["id"].as_str().unwrap(),
            "approved",
            "x",
            "",
            None,
            true,
        )
        .unwrap()
        .unwrap();
        assert_eq!(r2["status"], "unconfirmed");
    }

    #[test]
    fn backfill_from_purchases() {
        let d = tempfile::tempdir().unwrap();
        let store = Store::new(d.path()).unwrap();
        let pid = store
            .add_purchase(
                "tb",
                "decathlon.sg",
                "v1",
                13.99,
                "SGD",
                &json!({"items": [{"name": "flask"}], "delivery": "Home"}),
            )
            .unwrap();
        let a = store
            .create_approval(
                "tb",
                "c1",
                "purchase_confirm",
                &json!({}),
                &json!({}),
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
        let made = backfill(&store).unwrap();
        assert_eq!(made.len(), 1);
        let r = get(&store, &made[0]).unwrap().unwrap();
        assert_eq!(r["purchase_id"], pid);
        assert_eq!(r["approval_id"], a["id"]);
        assert_eq!(r["items"], json!([{"name": "flask"}]));
        assert_eq!(r["delivery"], "Home");
        assert_eq!(r["total"], 13.99);
        assert!(backfill(&store).unwrap().is_empty());
    }
}
