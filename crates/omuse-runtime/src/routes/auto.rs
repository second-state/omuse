//! Schedules (automations, event triggers) and goals.

use axum::body::Bytes;
use axum::extract::Path as UrlPath;
use axum::Json;
use omuse_common::util::{get_str, now_ts, py_str, take_chars, truthy};
use omuse_common::{params, pyjson};
use serde_json::{json, Map, Value};

use super::{body_obj, getv, http, not_found, pstr, Fail, St, R};
use crate::agent::is_terminal;
use crate::goals as g;
use crate::scheduler::{self as sc, ValueError};

/// `str(e)` of a Python `ValueError` → 400; anything else is unhandled.
fn value_error(e: anyhow::Error) -> Fail {
    match e.downcast_ref::<ValueError>() {
        Some(v) => http(400, v.0.clone()),
        None => e.into(),
    }
}

/// A string as is, anything else as JSON text (`x if isinstance(x, str) else json.dumps(x)`).
fn as_spec(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => pyjson::dumps(other),
    }
}

pub async fn schedules(st: St) -> R {
    let store = &st.rt.store;
    let mut out = Vec::new();
    for mut x in store.schedules()? {
        if x.get("goal_id").is_some_and(truthy) {
            continue; // shown on the goal card
        }
        let stv = x
            .get("state")
            .filter(|v| truthy(v))
            .cloned()
            .unwrap_or_else(|| json!({}));
        x["describe"] = json!(sc::describe(&x));
        x["state"] = super::filter_obj(&stv, |k| !k.starts_with('_'));
        let mut trig = Map::new();
        for k in ["_error", "_error_at", "_checked", "_seen", "_last_events"] {
            if let Some(v) = stv.get(k).filter(|v| !v.is_null()) {
                trig.insert(k.into(), v.clone());
            }
        }
        x["trigger"] = Value::Object(trig);
        let lt = get_str(&x, "last_task");
        let last = if lt.is_empty() {
            None
        } else {
            store.task(&lt)?
        };
        x["last_status"] = json!(last.map(|t| get_str(&t, "status")).unwrap_or_default());
        let mut blocked = Map::new();
        for k in ["_skipped", "_superseded"] {
            if let Some(v) = stv.get(k).filter(|v| truthy(v)) {
                blocked.insert(k[1..].to_string(), v.clone());
            }
        }
        x["blocked"] = Value::Object(blocked);
        out.push(x);
    }
    let sources: Map<String, Value> = sc::EVENT_SOURCES
        .iter()
        .map(|(k, v)| (k.to_string(), json!(v)))
        .collect();
    Ok(Json(json!({"schedules": out, "sources": sources})))
}

pub async fn goals(st: St) -> R {
    let store = &st.rt.store;
    let list = store
        .goals()?
        .iter()
        .map(|x| g::brief(store, x))
        .collect::<anyhow::Result<Vec<_>>>()?;
    Ok(Json(json!({"goals": list})))
}

pub async fn add_goal(st: St, body: Bytes) -> R {
    let b = body_obj(&body)?;
    let rt = &st.rt;
    let tz = get_str(&rt.store.settings()?, "timezone");
    let deadline = b
        .get("deadline")
        .filter(|v| truthy(v))
        .cloned()
        .unwrap_or(Value::Null);
    let spec = b.get("spec").cloned().unwrap_or_else(|| json!("60"));
    let goal = g::create_goal(
        &rt.store,
        &pstr(&b, "title", ""),
        &pstr(&b, "objective", ""),
        &pstr(&b, "criteria", ""),
        &pstr(&b, "kind", "interval"),
        &spec,
        &tz,
        &deadline,
    )
    .map_err(value_error)?;
    let gid = get_str(&goal, "id");
    rt.audit(
        "user",
        "goal.create",
        "",
        json!({"resource": gid, "detail": {"title": goal["title"]}}),
    )
    .await;
    if b.get("run_now").is_some_and(truthy) {
        st.sched
            .run_now(&get_str(&goal, "schedule_id"), None, None, "")
            .await?;
    }
    let cur = rt.store.goal(&gid)?.unwrap_or(goal);
    Ok(Json(g::brief(&rt.store, &cur)?))
}

pub async fn upd_goal(st: St, UrlPath(gid): UrlPath<String>, body: Bytes) -> R {
    let rt = &st.rt;
    let store = &rt.store;
    let Some(goal) = store.goal(&gid)? else {
        return Err(not_found());
    };
    let b = body_obj(&body)?;
    let mut data = Map::new();
    for k in ["title", "objective", "criteria"] {
        if let Some(v) = b.get(k) {
            data.insert(k.into(), json!(take_chars(&py_str(v), 3000)));
        }
    }
    if let Some(v) = b.get("deadline") {
        let dl = g::parse_deadline(v).map_err(|e| http(400, e.0))?;
        data.insert("deadline".into(), json!(dl));
    }
    if !data.is_empty() {
        data.insert("updated_at".into(), json!(now_ts()));
        store
            .db
            .update("goals", "id", gid.as_str(), &Value::Object(data))?;
    }
    if b.get("kind").is_some() || b.get("spec").is_some() {
        let sch = store
            .schedule(&get_str(&goal, "schedule_id"))?
            .ok_or_else(|| Fail::Internal(anyhow::anyhow!("TypeError: schedule missing")))?;
        let kind = b
            .get("kind")
            .map(py_str)
            .unwrap_or_else(|| get_str(&sch, "kind"));
        let spec = as_spec(b.get("spec").unwrap_or(&sch["spec"]));
        sc::validate(&kind, &spec).map_err(|e| http(400, e.0))?;
        let nxt =
            sc::next_run(&kind, &spec, &get_str(&sch, "tz"), None).map_err(|e| http(400, e.0))?;
        store.db.update(
            "schedules",
            "id",
            get_str(&sch, "id"),
            &json!({"kind": kind, "spec": spec, "next_run": nxt}),
        )?;
    }
    if let Some(s) = b.get("status") {
        g::set_status(store, &gid, &py_str(s)).map_err(value_error)?;
    }
    let detail = super::filter_obj(&b, |k| k != "objective");
    rt.audit(
        "user",
        "goal.update",
        "",
        json!({"resource": gid, "detail": detail}),
    )
    .await;
    let cur = store.goal(&gid)?.ok_or_else(not_found)?;
    Ok(Json(g::brief(store, &cur)?))
}

pub async fn run_goal(st: St, UrlPath(gid): UrlPath<String>) -> R {
    let Some(goal) = st.rt.store.goal(&gid)? else {
        return Err(not_found());
    };
    if get_str(&goal, "status") != "active" {
        return Err(http(400, "目标不在进行中 (goal is not active)"));
    }
    let t = st
        .sched
        .run_now(&get_str(&goal, "schedule_id"), None, None, "")
        .await?;
    Ok(Json(json!({"task_id": t["id"]})))
}

pub async fn del_goal(st: St, UrlPath(gid): UrlPath<String>) -> R {
    let store = &st.rt.store;
    if let Some(goal) = store.goal(&gid)? {
        store.db.execute(
            "DELETE FROM schedules WHERE id=?",
            params![get_str(&goal, "schedule_id")],
        )?;
        store
            .db
            .execute("DELETE FROM goals WHERE id=?", params![gid.as_str()])?;
    }
    st.rt
        .audit("user", "goal.delete", "", json!({"resource": gid}))
        .await;
    Ok(Json(json!({"ok": true})))
}

/// Python `str(KeyError(k))`.
fn key_error(k: &str) -> Fail {
    http(400, format!("'{}'", k))
}

pub async fn add_schedule(st: St, body: Bytes) -> R {
    let b = body_obj(&body)?;
    let rt = &st.rt;
    let spec = as_spec(b.get("spec").ok_or_else(|| key_error("spec"))?);
    let name = py_str(b.get("name").ok_or_else(|| key_error("name"))?);
    let goal = py_str(b.get("goal").ok_or_else(|| key_error("goal"))?);
    let tz = match b.get("tz").filter(|v| truthy(v)) {
        Some(v) => py_str(v),
        None => get_str(&rt.store.settings()?, "timezone"),
    };
    let s = sc::create_schedule(
        &rt.store,
        &name,
        &goal,
        &pstr(&b, "kind", "cron"),
        &json!(spec),
        &tz,
        "",
        "",
    )
    .map_err(value_error)?;
    rt.audit(
        "user",
        "schedule.create",
        "",
        json!({"resource": s["id"], "detail": {"name": s["name"], "spec": s["spec"]}}),
    )
    .await;
    Ok(Json(s))
}

pub async fn upd_schedule(st: St, UrlPath(sid): UrlPath<String>, body: Bytes) -> R {
    let store = &st.rt.store;
    let Some(s) = store.schedule(&sid)? else {
        return Err(not_found());
    };
    let b = body_obj(&body)?;
    let mut data = Map::new();
    for k in ["name", "goal", "kind", "spec", "tz"] {
        if let Some(v) = b.get(k) {
            data.insert(k.into(), json!(as_spec(v)));
        }
    }
    if let Some(v) = b.get("enabled") {
        data.insert("enabled".into(), json!(if truthy(v) { 1 } else { 0 }));
    }
    let pick = |k: &str| data.get(k).map(py_str).unwrap_or_else(|| get_str(&s, k));
    let (kind, spec, tz) = (pick("kind"), pick("spec"), pick("tz"));
    sc::validate(&kind, &spec).map_err(|e| http(400, e.0))?;
    let nxt = sc::next_run(&kind, &spec, &tz, None).map_err(|e| Fail::Internal(e.into()))?;
    data.insert("next_run".into(), json!(nxt));
    store
        .db
        .update("schedules", "id", sid.as_str(), &Value::Object(data))?;
    Ok(Json(store.schedule(&sid)?.unwrap_or(Value::Null)))
}

pub async fn del_schedule(st: St, UrlPath(sid): UrlPath<String>) -> R {
    st.rt
        .store
        .db
        .execute("DELETE FROM schedules WHERE id=?", params![sid.as_str()])?;
    st.rt
        .audit("user", "schedule.delete", "", json!({"resource": sid}))
        .await;
    Ok(Json(json!({"ok": true})))
}

/// Check an event trigger right now (instead of waiting for the next poll).
pub async fn poll_schedule(st: St, UrlPath(sid): UrlPath<String>) -> R {
    let store = &st.rt.store;
    let s = match store.schedule(&sid)? {
        Some(s) if get_str(&s, "kind") == "event" => s,
        _ => return Err(http(400, "不是事件触发器 (not an event trigger)")),
    };
    let lt = get_str(&s, "last_task");
    let last = if lt.is_empty() {
        None
    } else {
        store.task(&lt)?
    };
    if last.is_some_and(|l| !is_terminal(&get_str(&l, "status"))) {
        return Err(http(409, "上一次运行还没结束 (previous run still active)"));
    }
    let n = st.sched.poll_event(&s).await?;
    let s2 = store.schedule(&sid)?.unwrap_or_else(|| json!({}));
    let stv = s2
        .get("state")
        .filter(|v| truthy(v))
        .cloned()
        .unwrap_or_else(|| json!({}));
    let changed = getv(&s2, "last_task") != getv(&s, "last_task");
    Ok(Json(json!({
        "ok": !stv.get("_error").is_some_and(truthy),
        "error": stv.get("_error").cloned().unwrap_or_else(|| json!("")),
        "fired": n > 0 || changed,
        "task_id": if changed { getv(&s2, "last_task") } else { json!("") },
    })))
}

pub async fn run_schedule(st: St, UrlPath(sid): UrlPath<String>) -> R {
    let store = &st.rt.store;
    let s = store.schedule(&sid)?;
    if let Some(s) = &s {
        let lt = get_str(s, "last_task");
        if !lt.is_empty() {
            if let Some(last) = store.task(&lt)? {
                if sc::WAITING.contains(&get_str(&last, "status").as_str()) {
                    // don't leave the old run (and its approval) dangling
                    st.sched.supersede(s, &last, true).await?;
                }
            }
        }
    }
    let t = st.sched.run_now(&sid, None, None, "").await?;
    Ok(Json(json!({"task_id": t["id"]})))
}
