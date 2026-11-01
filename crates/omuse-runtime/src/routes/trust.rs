//! Trust: metrics, health, the purchase ledger, outcomes — and notifications.

use axum::extract::Path as UrlPath;
use axum::Json;
use omuse_common::params;
use omuse_common::util::{as_f64, get_str, py_str, truthy};
use serde_json::{json, Value};

use super::{http, int_param, Fail, St, Q, R};
use crate::outcome;

pub async fn api_metrics(st: St, q: Q) -> R {
    let days = int_param(&q, "query", "days", Some(7))?.clamp(1, 90);
    let store = st.rt.store.clone();
    let off = store
        .settings()?
        .get("tz_offset_hours")
        .filter(|v| truthy(v))
        .and_then(as_f64)
        .unwrap_or(8.0);
    let m = tokio::task::spawn_blocking(move || crate::metrics::compute(&store, days, None, off))
        .await
        .map_err(|e| Fail::Internal(e.into()))??;
    Ok(Json(
        json!({"metrics": m, "health": st.rt.health().status()}),
    ))
}

pub async fn api_health(st: St) -> R {
    Ok(Json(st.rt.health().status()))
}

pub async fn api_health_check(st: St) -> R {
    let h = st.rt.health();
    let res = h.run_checks().await;
    for (comp, (ok, detail)) in res {
        h.observe(&comp, ok, &detail, false).await;
    }
    Ok(Json(h.status()))
}

/// The tool results kept in a task's transcript.
fn tool_texts(t: &Value) -> Vec<String> {
    t.get("transcript")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter(|m| get_str(m, "role") == "tool")
                .map(|m| {
                    m.get("content")
                        .filter(|v| truthy(v))
                        .map(py_str)
                        .unwrap_or_default()
                })
                .collect()
        })
        .unwrap_or_default()
}

fn assess(st: &crate::state::State, tid: &str, t: &Value) -> anyhow::Result<Value> {
    let texts = tool_texts(t);
    let events = st.rt.store.events(tid, 0)?;
    Ok(outcome::assess(
        &events,
        &get_str(t, "result"),
        Some(&texts),
        Some(&get_str(t, "goal")),
    ))
}

/// Ledger entries for purchases made before the ledger existed, with the order number their task got.
pub async fn ledger_backfill(st: St) -> R {
    let rt = &st.rt;
    let res = rt
        .sentinel("POST", "/internal/ledger/backfill", Some(json!({})), 60.0)
        .await?;
    let mut done = Vec::new();
    let tids: Vec<String> = res
        .get("tasks")
        .and_then(Value::as_array)
        .map(|a| a.iter().map(py_str).collect())
        .unwrap_or_default();
    for tid in tids {
        let Some(t) = rt.store.task(&tid)? else {
            continue;
        };
        let mut oc = t
            .get("outcome")
            .filter(|v| truthy(v))
            .cloned()
            .unwrap_or_else(|| json!({}));
        if !truthy(&oc) {
            oc = assess(&st, &tid, &t)?;
        }
        rt.ledger_confirm(&tid, &oc, &get_str(&t, "result")).await;
        done.push(tid);
    }
    let created = res
        .get("created")
        .filter(|v| truthy(v))
        .cloned()
        .unwrap_or_else(|| json!([]));
    Ok(Json(json!({"created": created, "confirmed": done})))
}

pub async fn ledger_reconcile(st: St) -> R {
    Ok(Json(st.rt.health().reconcile_ledger().await?))
}

/// Re-assess an older task's outcome from what it did (the tool results kept in its transcript).
pub async fn task_outcome(st: St, UrlPath(tid): UrlPath<String>) -> R {
    let Some(t) = st.rt.store.task(&tid)? else {
        return Err(http(404, "no such task"));
    };
    let oc = assess(&st, &tid, &t)?;
    st.rt.store.update_task(&tid, &json!({"outcome": oc}))?;
    Ok(Json(json!({"task_id": tid, "outcome": oc})))
}

pub async fn notifications(st: St) -> R {
    Ok(Json(
        json!({"notifications": st.rt.store.notifications(50)?}),
    ))
}

pub async fn notifications_read(st: St) -> R {
    st.rt
        .store
        .db
        .execute("UPDATE notifications SET read=1", params![])?;
    Ok(Json(json!({"ok": true})))
}
