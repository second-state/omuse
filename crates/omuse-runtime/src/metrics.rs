//! Trust & outcome metrics (roadmap batch 1): how often OMuse finishes what it is asked, how often it needs the user,
//! and whether "done" is backed by proof. Computed from the task records and their events; nothing extra is logged.
//!
//! Definitions (finished = COMPLETED + FAILED + CANCELLED; golden test runs are left out):
//! ```text
//!   completion_rate      COMPLETED / finished
//!   autonomous_rate      COMPLETED with no approval, no takeover and no question back to the user / COMPLETED
//!   intervention_rate    finished tasks that needed an approval, a takeover or an answer from the user / finished
//!   approval_burden      approvals asked / tasks that changed something (an order, an email, a calendar event …)
//!   denied               approvals the user refused (an action OMuse wanted that the user stopped)
//!   verified / unverified  tasks that changed something, with / without proof (order number, message id …)
//! ```

use anyhow::Result;
use omuse_common::util::{as_f64, get_str, get_truthy, now_ts, py_str, take_chars, truthy};
use serde_json::{json, Value};

use crate::health::{failure_cause, CAUSES};

pub const FINISHED: [&str; 3] = ["COMPLETED", "FAILED", "CANCELLED"];

/// `collections.Counter` with insertion order (so `most_common` breaks ties the way Python does).
#[derive(Default, Clone, Debug)]
struct Counter(Vec<(String, i64)>);

impl Counter {
    fn add(&mut self, k: &str, n: i64) {
        match self.0.iter_mut().find(|(kk, _)| kk == k) {
            Some((_, v)) => *v += n,
            None => self.0.push((k.to_string(), n)),
        }
    }
    fn get(&self, k: &str) -> i64 {
        self.0
            .iter()
            .find(|(kk, _)| kk == k)
            .map(|(_, v)| *v)
            .unwrap_or(0)
    }
    fn most_common(&self, n: Option<usize>) -> Vec<(String, i64)> {
        let mut v = self.0.clone();
        v.sort_by_key(|x| std::cmp::Reverse(x.1));
        if let Some(n) = n {
            v.truncate(n);
        }
        v
    }
}

/// `collections.defaultdict(Counter)` keyed by a string, insertion ordered.
#[derive(Default)]
struct Groups(Vec<(String, Counter)>);

impl Groups {
    fn entry(&mut self, k: &str) -> &mut Counter {
        if let Some(i) = self.0.iter().position(|(kk, _)| kk == k) {
            return &mut self.0[i].1;
        }
        self.0.push((k.to_string(), Counter::default()));
        &mut self.0.last_mut().unwrap().1
    }
}

fn cause_label(k: &str) -> String {
    CAUSES
        .iter()
        .find(|(c, _)| *c == k)
        .map(|(_, l)| l.to_string())
        .unwrap_or_else(|| k.to_string())
}

/// What a task's events say: approvals asked / approved / denied, takeovers, a question back to the user, its
/// outcome status, whether it changed something, and (for FAILED) the failure cause.
pub fn task_facts(t: &Value, events: &[Value]) -> Value {
    let (mut approvals, mut takeovers, mut approved, mut denied) = (0i64, 0i64, 0i64, 0i64);
    let mut needs_user = false;
    let empty = json!({});
    for e in events {
        let d = e.get("data").filter(|v| truthy(v)).unwrap_or(&empty);
        let typ = get_str(e, "type");
        if typ == "waiting" {
            let k = get_str(d, "type");
            if k == "approval" {
                approvals += 1;
            } else if k == "takeover_requested" || k == "takeover" {
                takeovers += 1;
            }
        } else if typ == "approval_resolved" {
            let dec = get_str(d, "decision");
            if dec == "approved" {
                approved += 1;
            } else if dec == "denied" || dec == "rejected" {
                denied += 1;
            }
        } else if typ == "needs_user" {
            needs_user = true;
        }
    }
    let oc = t.get("outcome").filter(|v| truthy(v)).unwrap_or(&empty);
    json!({
        "approvals": approvals, "approved": approved, "denied": denied, "takeovers": takeovers, "needs_user": needs_user,
        "outcome": get_str(oc, "status"), "acted": get_truthy(oc, "actions"),
        "cause": if get_str(t, "status") == "FAILED" { failure_cause(&get_str(t, "error")).to_string() } else { String::new() },
    })
}

/// Python `round(a / b, 3)`, `None` when `b` is 0.
fn rate(a: i64, b: i64) -> Value {
    if b == 0 {
        return Value::Null;
    }
    round3(a as f64 / b as f64)
}

fn round3(x: f64) -> Value {
    let r: f64 = format!("{:.3}", x).parse().unwrap_or(x);
    json!(r)
}

fn day_of(ts: f64) -> String {
    chrono::DateTime::from_timestamp(ts as i64, 0)
        .map(|d| d.format("%Y-%m-%d").to_string())
        .unwrap_or_default()
}

/// The metrics of the last `days` days (tasks created since then, golden runs left out).
pub fn compute(
    store: &crate::store::Store,
    days: i64,
    now: Option<f64>,
    tz_offset_h: f64,
) -> Result<Value> {
    let now = now.filter(|n| *n != 0.0).unwrap_or_else(now_ts);
    let since = now - days as f64 * 86400.0;
    let rows: Vec<Value> = store
        .tasks(None, 2000, None)?
        .into_iter()
        .filter(|t| {
            t.get("created_at").and_then(as_f64).unwrap_or(0.0) >= since
                && get_str(t, "source") != "golden"
        })
        .collect();
    let mut per_day = Groups::default();
    let mut causes = Counter::default();
    let mut errors = Counter::default();
    let mut agg = Counter::default();
    let mut sites = Groups::default(); // registered site -> {opens, blocked}
    let mut block_kinds = Counter::default();
    let empty = json!({});
    for t in &rows {
        let evs = store.events(&get_str(t, "id"), 0)?;
        for e in &evs {
            let d = e.get("data").filter(|v| truthy(v)).unwrap_or(&empty);
            let typ = get_str(e, "type");
            if typ == "tool_call"
                && (get_str(d, "name") == "browser_navigate"
                    || get_str(d, "name") == "browser_read")
            {
                let args = d.get("args").filter(|v| v.is_object()).unwrap_or(&empty);
                let mut urls: Vec<Value> = vec![args.get("url").cloned().unwrap_or(Value::Null)];
                if let Some(more) = args.get("urls").and_then(Value::as_array) {
                    urls.extend(more.iter().cloned());
                }
                for u in urls {
                    let text = if truthy(&u) {
                        py_str(&u)
                    } else {
                        String::new()
                    };
                    let s = site_key(&text);
                    if !s.is_empty() {
                        sites.entry(&s).add("opens", 1);
                    }
                }
            } else if typ == "site_blocked" {
                let s = get_str(d, "site");
                if !s.is_empty() {
                    sites.entry(&s).add("blocked", 1);
                    let kind = [d.get("detail"), d.get("kind")]
                        .into_iter()
                        .flatten()
                        .find(|v| truthy(v))
                        .map(py_str)
                        .unwrap_or_else(|| "?".to_string());
                    block_kinds.add(&kind, 1);
                }
            }
        }
        let f = task_facts(t, &evs);
        let st = get_str(t, "status");
        let created = t.get("created_at").and_then(as_f64).unwrap_or(now);
        let day = day_of(created + tz_offset_h * 3600.0);
        let pd = per_day.entry(&day);
        pd.add("total", 1);
        pd.add(&st, 1);
        agg.add("total", 1);
        agg.add(&st, 1);
        if !FINISHED.contains(&st.as_str()) {
            continue;
        }
        agg.add("finished", 1);
        let n = |k: &str| f.get(k).and_then(|v| v.as_i64()).unwrap_or(0);
        let helped = n("approvals") != 0 || n("takeovers") != 0 || get_truthy(&f, "needs_user");
        if helped {
            agg.add("intervened", 1);
        }
        if st == "COMPLETED" && !helped {
            agg.add("autonomous", 1);
        }
        agg.add("approvals", n("approvals"));
        agg.add("denied", n("denied"));
        agg.add("takeovers", n("takeovers"));
        if get_truthy(&f, "acted") {
            agg.add("acted", 1);
            agg.add("acted_approvals", n("approvals"));
        }
        let oc = get_str(&f, "outcome");
        if oc == "verified" || oc == "unverified" {
            agg.add(&oc, 1);
        }
        if st == "FAILED" {
            causes.add(&get_str(&f, "cause"), 1);
            errors.add(take_chars(&get_str(t, "error"), 90), 1);
        }
    }

    let mut per_day_rows: Vec<(String, Counter)> = per_day.0;
    per_day_rows.sort_by(|a, b| a.0.cmp(&b.0));

    Ok(json!({
        "days": days, "since": since, "until": now,
        "tasks": agg.get("total"), "finished": agg.get("finished"),
        "completed": agg.get("COMPLETED"), "failed": agg.get("FAILED"), "cancelled": agg.get("CANCELLED"),
        "completion_rate": rate(agg.get("COMPLETED"), agg.get("finished")),
        "autonomous_rate": rate(agg.get("autonomous"), agg.get("COMPLETED")),
        "intervention_rate": rate(agg.get("intervened"), agg.get("finished")),
        "approvals": agg.get("approvals"), "denied": agg.get("denied"), "takeovers": agg.get("takeovers"),
        "acted": agg.get("acted"), "approval_burden": rate(agg.get("acted_approvals"), agg.get("acted")),
        "verified": agg.get("verified"), "unverified": agg.get("unverified"),
        "failure_causes": causes.most_common(None).iter()
            .map(|(k, v)| json!({"cause": k, "label": cause_label(k), "count": v})).collect::<Vec<_>>(),
        "top_errors": errors.most_common(Some(5)).iter()
            .map(|(k, v)| json!({"error": k, "count": v})).collect::<Vec<_>>(),
        "per_day": per_day_rows.iter().map(|(d, c)| json!({"day": d, "total": c.get("total"), "completed": c.get("COMPLETED"),
                     "failed": c.get("FAILED"), "cancelled": c.get("CANCELLED")})).collect::<Vec<_>>(),
        "browser": browser_reliability(&sites, &block_kinds),
    }))
}

fn site_key(url: &str) -> String {
    crate::context::site_name(url)
}

fn browser_reliability(sites: &Groups, block_kinds: &Counter) -> Value {
    let opens: i64 = sites.0.iter().map(|(_, c)| c.get("opens")).sum();
    let blocked: i64 = sites.0.iter().map(|(_, c)| c.get("blocked")).sum();
    let mut rows: Vec<(i64, Value)> = sites
        .0
        .iter()
        .filter(|(_, c)| c.get("blocked") != 0)
        .map(|(s, c)| {
            (
                c.get("blocked"),
                json!({"site": s, "opens": c.get("opens"), "blocked": c.get("blocked"),
                       "block_rate": rate(c.get("blocked"), c.get("opens"))}),
            )
        })
        .collect();
    rows.sort_by_key(|x| std::cmp::Reverse(x.0));
    rows.truncate(12);
    json!({
        "opens": opens, "blocked": blocked,
        "block_rate": rate(blocked, opens),
        "sites": rows.into_iter().map(|(_, v)| v).collect::<Vec<_>>(),
        "kinds": block_kinds.most_common(Some(8)).iter().map(|(k, v)| json!({"detail": k, "count": v})).collect::<Vec<_>>(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Store;
    use omuse_common::Config;
    use std::sync::Arc;

    fn store() -> (tempfile::TempDir, Store) {
        let d = tempfile::tempdir().unwrap();
        let st = Store::new(d.path(), Arc::new(Config::parse("").unwrap())).unwrap();
        (d, st)
    }

    fn task(
        st: &Store,
        cid: &str,
        status: &str,
        error: &str,
        events: &[(&str, Value)],
        outcome: Option<Value>,
        source: &str,
    ) -> String {
        let t = st.create_task("x", cid, source, "", "", None).unwrap();
        let tid = get_str(&t, "id");
        for (typ, d) in events {
            st.add_event(&tid, typ, d).unwrap();
        }
        st.update_task(
            &tid,
            &json!({"status": status, "error": error, "outcome": outcome}),
        )
        .unwrap();
        tid
    }

    #[test]
    fn trust_metrics() {
        let (_d, st) = store();
        let cid = st.create_conv("t", "chat", None).unwrap();
        task(&st, &cid, "COMPLETED", "", &[], None, "chat"); // autonomous
        task(
            &st,
            &cid,
            "COMPLETED",
            "",
            &[
                ("waiting", json!({"type": "approval", "approval_id": "a"})),
                (
                    "approval_resolved",
                    json!({"approval_id": "a", "decision": "approved"}),
                ),
            ],
            Some(json!({"status": "verified", "actions": [{"kind": "purchase"}]})),
            "chat",
        );
        task(
            &st,
            &cid,
            "FAILED",
            "LLMError: 模型接口错误 HTTP 422",
            &[],
            None,
            "chat",
        );
        task(
            &st,
            &cid,
            "CANCELLED",
            "",
            &[
                ("waiting", json!({"type": "approval", "approval_id": "b"})),
                (
                    "approval_resolved",
                    json!({"approval_id": "b", "decision": "denied"}),
                ),
            ],
            None,
            "chat",
        );
        task(&st, &cid, "COMPLETED", "", &[], None, "golden"); // not counted
        let m = compute(&st, 7, None, 8.0).unwrap();
        assert_eq!(m["tasks"], json!(4));
        assert_eq!(m["completed"], json!(2));
        assert_eq!(m["failed"], json!(1));
        assert_eq!(m["cancelled"], json!(1));
        assert_eq!(m["completion_rate"], json!(0.5));
        assert_eq!(m["autonomous_rate"], json!(0.5));
        assert_eq!(m["intervention_rate"], json!(0.5));
        assert_eq!(m["approvals"], json!(2));
        assert_eq!(m["denied"], json!(1));
        assert_eq!(m["acted"], json!(1));
        assert_eq!(m["approval_burden"], json!(1.0));
        assert_eq!(m["verified"], json!(1));
        assert_eq!(m["failure_causes"][0]["cause"], json!("model"));
        assert_eq!(m["failure_causes"][0]["label"], json!("模型"));
        assert_eq!(
            m["top_errors"][0]["error"],
            json!("LLMError: 模型接口错误 HTTP 422")
        );
        assert_eq!(m["per_day"].as_array().unwrap().len(), 1);
        assert_eq!(m["per_day"][0]["total"], json!(4));
    }

    #[test]
    fn browser_reliability_metric() {
        let (_d, st) = store();
        let cid = st.create_conv("c", "chat", None).unwrap();
        let t = st.create_task("shop", &cid, "chat", "", "", None).unwrap();
        let tid = get_str(&t, "id");
        st.update_task(&tid, &json!({"status": "COMPLETED"}))
            .unwrap();
        for u in [
            "https://www.decathlon.sg/a",
            "https://www.decathlon.sg/b",
            "https://www.amazon.sg/x",
        ] {
            st.add_event(
                &tid,
                "tool_call",
                &json!({"name": "browser_navigate", "args": {"url": u}}),
            )
            .unwrap();
        }
        st.add_event(
            &tid,
            "site_blocked",
            &json!({"site": "decathlon", "kind": "cloudflare", "detail": "Cloudflare bot check"}),
        )
        .unwrap();
        let m = compute(&st, 7, None, 8.0).unwrap();
        let m = &m["browser"];
        assert_eq!(m["opens"], json!(3));
        assert_eq!(m["blocked"], json!(1));
        let d = m["sites"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["site"] == json!("decathlon"))
            .unwrap();
        assert_eq!(d["opens"], json!(2));
        assert_eq!(d["blocked"], json!(1));
        assert_eq!(d["block_rate"], json!(0.5));
        assert_eq!(m["kinds"][0]["detail"], json!("Cloudflare bot check"));
        assert_eq!(rate(1, 3), json!(0.333));
        assert_eq!(rate(0, 0), Value::Null);
    }
}
