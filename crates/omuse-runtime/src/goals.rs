//! Goals (场景目标): long-running objectives that OMuse keeps working on until they are achieved.
//!
//! A goal = objective + success criteria + how often to check (cron / interval / event trigger) + optional deadline.
//! Each check is a normal agent task (same tools, same Sentinel approvals). The agent must end every check with
//! goal_update(status, progress): progress notes build a history the next check sees; "achieved"/"failed" close
//! the goal, "blocked" asks the user for help. When the deadline passes, one final check decides the outcome.

use anyhow::Result;
use chrono::{Local, NaiveDate, NaiveDateTime, TimeZone};
use omuse_common::params;
use omuse_common::util::{
    as_f64, dumps, get_str, get_truthy, new_id, now_ts, take_chars, truncate, truthy,
};
use serde_json::{json, Map, Value};

use crate::rt_api::RuntimeApi;
use crate::scheduler::{create_schedule, describe, ValueError};
use crate::store::Store;

/// Python's `KeyError`: no such goal (the HTTP layer answers 404). Wrapped in `anyhow::Error`.
#[derive(Debug, Clone, thiserror::Error)]
#[error("{0}")]
pub struct KeyError(pub String);

pub const STATUSES: &[&str] = &[
    "active",
    "paused",
    "achieved",
    "failed",
    "expired",
    "cancelled",
];
pub const UPDATE_STATUSES: &[&str] = &["active", "blocked", "achieved", "failed"];
pub const STATUS_ZH: &[(&str, &str)] = &[
    ("active", "进行中"),
    ("paused", "已暂停"),
    ("achieved", "已达成"),
    ("failed", "未达成"),
    ("expired", "已过期"),
    ("cancelled", "已取消"),
    ("blocked", "需要你帮忙"),
];

/// `STATUS_ZH.get(status, status)`.
pub fn status_zh(status: &str) -> String {
    STATUS_ZH
        .iter()
        .find(|(k, _)| *k == status)
        .map(|(_, v)| (*v).to_string())
        .unwrap_or_else(|| status.to_string())
}

fn fmt(ts: Option<f64>) -> String {
    match ts {
        Some(t) if t != 0.0 => Local
            .timestamp_opt(t.floor() as i64, 0)
            .single()
            .map(|d| d.format("%Y-%m-%d %H:%M").to_string())
            .unwrap_or_default(),
        _ => String::new(),
    }
}

fn local_ts(dt: NaiveDateTime) -> Option<f64> {
    Local
        .from_local_datetime(&dt)
        .earliest()
        .map(|d| d.timestamp() as f64)
}

/// A deadline as Unix seconds: a number as is, `YYYY-MM-DD HH:MM` / `YYYY-MM-DDTHH:MM` in local time, a bare
/// `YYYY-MM-DD` = the end of that day; `None`/`""`/`0` = no deadline.
pub fn parse_deadline(v: &Value) -> std::result::Result<Option<f64>, ValueError> {
    match v {
        Value::Null => return Ok(None),
        Value::String(s) if s.is_empty() => return Ok(None),
        Value::Number(n) => {
            let f = n.as_f64().unwrap_or(0.0);
            return Ok(if f == 0.0 { None } else { Some(f) });
        }
        Value::Bool(false) => return Ok(None),
        _ => {}
    }
    let s = omuse_common::util::py_str(v).trim().to_string();
    for fmt in ["%Y-%m-%d %H:%M", "%Y-%m-%dT%H:%M", "%Y-%m-%d"] {
        if fmt.contains('H') {
            if let Ok(dt) = NaiveDateTime::parse_from_str(take_chars(&s, 16), fmt) {
                if let Some(ts) = local_ts(dt) {
                    return Ok(Some(ts));
                }
            }
        } else if let Ok(d) = NaiveDate::parse_from_str(take_chars(&s, 10), fmt) {
            if let Some(ts) = local_ts(d.and_hms_opt(0, 0, 0).unwrap()) {
                return Ok(Some(ts + 86399.0)); // a bare date means end of that day
            }
        }
    }
    Err(ValueError(
        "截止时间格式应为 YYYY-MM-DD 或 YYYY-MM-DD HH:MM (deadline format)".into(),
    ))
}

/// Create a goal with its schedule and conversation; returns the goal record.
#[allow(clippy::too_many_arguments)]
pub fn create_goal(
    store: &Store,
    title: &str,
    objective: &str,
    criteria: &str,
    kind: &str,
    spec: &Value,
    tz: &str,
    deadline: &Value,
) -> Result<Value> {
    let title = take_chars(title.trim(), 80).to_string();
    let objective = take_chars(objective.trim(), 3000).to_string();
    if title.is_empty() || objective.is_empty() {
        return Err(ValueError("目标需要标题和描述 (title and objective required)".into()).into());
    }
    let dl = parse_deadline(deadline)?;
    if dl.is_some_and(|d| d < now_ts()) {
        return Err(ValueError("截止时间已经过去了 (deadline is in the past)".into()).into());
    }
    let gid = new_id("goal");
    let conv = store.create_conv(
        &format!("🎯 {}", title),
        "schedule",
        Some(&format!("conv_{}", gid)),
    )?;
    let sch = create_schedule(
        store,
        &format!("🎯 {}", title),
        &format!("(goal {})", gid),
        kind,
        spec,
        tz,
        &gid,
        &conv,
    )?;
    store.db.insert(
        "goals",
        &json!({"id": gid, "title": title, "objective": objective, "criteria": take_chars(criteria.trim(), 2000),
                "status": "active", "deadline": dl, "schedule_id": sch["id"], "conv_id": conv,
                "progress": dumps(&json!([])), "result": "", "created_at": now_ts(), "updated_at": now_ts(),
                "finished_at": null}),
    )?;
    store
        .goal(&gid)?
        .ok_or_else(|| anyhow::anyhow!("goal vanished"))
}

/// The task text of one goal check (`final` = the deadline has passed).
pub fn goal_prompt(g: &Value, final_: bool, lang: &str) -> String {
    let prog: Vec<Value> = g
        .get("progress")
        .and_then(|p| p.as_array())
        .cloned()
        .unwrap_or_default();
    let start = prog.len().saturating_sub(12);
    let hist: Vec<String> = prog[start..]
        .iter()
        .map(|p| {
            format!(
                "- {} [{}] {}",
                get_str(p, "at"),
                p.get("status")
                    .map(omuse_common::util::py_str)
                    .unwrap_or_else(|| "None".into()),
                get_str(p, "note")
            )
        })
        .collect();
    let hist = if hist.is_empty() {
        if lang == "en" {
            "(no progress yet — this is the first check)".to_string()
        } else {
            "（还没有进展记录 no progress yet — this is the first check）".to_string()
        }
    } else {
        hist.join("\n")
    };
    let dl = if get_truthy(g, "deadline") {
        fmt(as_f64(&g["deadline"]))
    } else {
        "none".to_string()
    };
    let title = get_str(g, "title");
    let objective = get_str(g, "objective");
    let criteria = get_str(g, "criteria");
    if lang == "en" {
        let task = if final_ {
            "⏰ The deadline has passed. This is the final check: decide whether the goal was achieved, then call goal_update with status achieved or failed."
        } else {
            "This run: check the latest progress, take the next useful step toward the goal (sending/submitting still needs the user's approval as usual), then ALWAYS call goal_update: active / blocked (need the user's help) / achieved / failed."
        };
        return format!(
            "🎯 Goal: {}\nObjective: {}\nSuccess criteria: {}\nDeadline: {}\nProgress so far (oldest first):\n{}\n\n{}",
            title,
            objective,
            if criteria.is_empty() { "(use your judgement)" } else { criteria.as_str() },
            dl,
            hist,
            task
        );
    }
    let task = if final_ {
        "⏰ 截止时间已到。这是最后一次检查：判断目标是否达成，然后调用 goal_update，status 只能是 achieved 或 failed。\nDeadline reached — final check: decide and call goal_update with status achieved or failed."
    } else {
        "本次任务：检查目标的最新进展，做下一步能推进目标的事情（发送/提交等操作照常需要用户审批），最后必须调用 goal_update 记录进展：仍在进行 active / 需要用户帮忙 blocked / 已达成 achieved / 无法达成 failed。\nThis run: check progress, take the next useful step toward the goal, then ALWAYS call goal_update."
    };
    format!(
        "🎯 长期目标 Goal: {}\n目标描述 Objective: {}\n完成标准 Success criteria: {}\n截止时间 Deadline: {}\n以往进展 Progress so far (oldest first):\n{}\n\n{}",
        title,
        objective,
        if criteria.is_empty() { "（由你根据目标判断 use judgement）" } else { criteria.as_str() },
        dl,
        hist,
        task
    )
}

/// The agent's `goal_update(status, note)`: append to the progress log; achieved/failed close the goal and its
/// schedule; blocked/closed notify the user. Returns the tool result text.
pub async fn apply_update(
    rt: &dyn RuntimeApi,
    g: &Value,
    status: &str,
    note: &str,
    task_id: &str,
) -> Result<String> {
    let store = rt.store();
    let status = if UPDATE_STATUSES.contains(&status) {
        status
    } else {
        "active"
    };
    let mut prog: Vec<Value> = g
        .get("progress")
        .and_then(|p| p.as_array())
        .cloned()
        .unwrap_or_default();
    prog.push(json!({"at": fmt(Some(now_ts())), "status": status, "note": truncate(note, 800), "task_id": task_id}));
    let start = prog.len().saturating_sub(50);
    let mut data = Map::new();
    data.insert(
        "progress".into(),
        Value::String(dumps(&Value::Array(prog[start..].to_vec()))),
    );
    data.insert("updated_at".into(), json!(now_ts()));
    let final_ = matches!(status, "achieved" | "failed");
    let gid = get_str(g, "id");
    if final_ {
        data.insert("status".into(), json!(status));
        data.insert("result".into(), json!(truncate(note, 2000)));
        data.insert("finished_at".into(), json!(now_ts()));
        store.db.execute(
            "UPDATE schedules SET enabled=0 WHERE id=?",
            params![get_str(g, "schedule_id")],
        )?;
    }
    store
        .db
        .update("goals", "id", gid.as_str(), &Value::Object(data))?;
    if final_ || status == "blocked" {
        let icon = match status {
            "achieved" => "🎉",
            "failed" => "⚠️",
            _ => "🙋",
        };
        let title = if get_str(&store.settings()?, "language") == "en" {
            let word = match status {
                "achieved" => "achieved",
                "failed" => "failed",
                _ => "needs your help",
            };
            format!("{} Goal {}: {}", icon, word, get_str(g, "title"))
        } else {
            format!(
                "{} 目标{}：{}",
                icon,
                status_zh(status),
                get_str(g, "title")
            )
        };
        let n = store.notify(
            &title,
            &truncate(note, 1500),
            task_id,
            if status == "achieved" {
                "info"
            } else {
                "warning"
            },
        )?;
        rt.publish(json!({"kind": "notification", "notification": n}))
            .await;
        let _ = rt
            .sentinel(
                "POST",
                "/internal/notify",
                Some(json!({"task_id": task_id, "text": format!("{}\n{}", title, truncate(note, 1500))})),
                20.0,
            )
            .await;
    }
    rt.publish(json!({"kind": "goal_update", "goal_id": gid}))
        .await;
    rt.audit(
        "executor",
        "goal.update",
        task_id,
        json!({"resource": gid, "result": status, "detail": {"note": truncate(note, 300)}}),
    )
    .await;
    Ok(format!("已记录 recorded: {}", status_zh(status))
        + if final_ {
            "（目标已结束 goal closed）"
        } else {
            ""
        })
}

/// Goals past their deadline get one final check; if that run doesn't decide, they expire.
pub async fn check_deadlines(rt: &dyn RuntimeApi) -> Result<()> {
    let store = rt.store();
    let now = now_ts();
    for g in store.goals()? {
        let deadline = as_f64(&g["deadline"]).filter(|d| *d != 0.0);
        if get_str(&g, "status") != "active" || deadline.is_none_or(|d| d > now) {
            continue;
        }
        let sid = get_str(&g, "schedule_id");
        let sch = if sid.is_empty() {
            None
        } else {
            store.schedule(&sid)?
        };
        let mut st: Map<String, Value> = sch
            .as_ref()
            .and_then(|s| s.get("state"))
            .and_then(|s| s.as_object())
            .cloned()
            .unwrap_or_default();
        let final_check = st
            .get("_final_check")
            .filter(|v| truthy(v))
            .and_then(as_f64);
        let gid = get_str(&g, "id");
        let title = get_str(&g, "title");
        match final_check {
            None => {
                if let Some(sch) = &sch {
                    let last_id = get_str(sch, "last_task");
                    let last = if last_id.is_empty() {
                        None
                    } else {
                        store.task(&last_id)?
                    };
                    if last.as_ref().is_some_and(|l| {
                        !matches!(
                            get_str(l, "status").as_str(),
                            "COMPLETED" | "FAILED" | "CANCELLED"
                        )
                    }) {
                        continue;
                    }
                    st.insert("_final_check".into(), json!(now_ts()));
                    let sch_id = get_str(sch, "id");
                    store.db.execute(
                        "UPDATE schedules SET state=?, enabled=0 WHERE id=?",
                        params![dumps(&Value::Object(st)), sch_id.as_str()],
                    )?;
                    let en = get_str(&store.settings()?, "language") == "en";
                    let conv_id = get_str(&g, "conv_id");
                    store.add_msg(
                        &conv_id,
                        "user",
                        &format!(
                            "⏰ {}: {}",
                            if en {
                                "Deadline reached — final check"
                            } else {
                                "截止时间已到，最后检查 Final check"
                            },
                            title
                        ),
                        "",
                        None,
                    )?;
                    let t = rt
                        .submit(
                            &conv_id,
                            &goal_prompt(&g, true, if en { "en" } else { "zh" }),
                            "schedule",
                            &sch_id,
                        )
                        .await?;
                    store.db.execute(
                        "UPDATE schedules SET last_task=? WHERE id=?",
                        params![get_str(&t, "id"), sch_id.as_str()],
                    )?;
                    continue;
                }
            }
            Some(fc) => {
                if now - fc < 3600.0 {
                    continue;
                }
            }
        }
        store.db.update(
            "goals",
            "id",
            gid.as_str(),
            &json!({"status": "expired", "finished_at": now_ts(), "updated_at": now_ts()}),
        )?;
        let n = if get_str(&store.settings()?, "language") == "en" {
            store.notify(
                &format!("⌛ Goal expired: {}", title),
                "The deadline passed and the final check reached no conclusion.",
                "",
                "warning",
            )?
        } else {
            store.notify(
                &format!("⌛ 目标已过期：{}", title),
                "截止时间已过，最后一次检查没有给出结论。",
                "",
                "warning",
            )?
        };
        rt.publish(json!({"kind": "notification", "notification": n}))
            .await;
    }
    Ok(())
}

/// Pause / resume / cancel a goal (and its schedule). `KeyError` when the goal does not exist.
pub fn set_status(store: &Store, gid: &str, status: &str) -> Result<Value> {
    let g = store
        .goal(gid)?
        .ok_or_else(|| anyhow::Error::new(KeyError(gid.to_string())))?;
    if !matches!(status, "active" | "paused" | "cancelled") {
        return Err(ValueError("只能设置为 active / paused / cancelled".into()).into());
    }
    if matches!(
        get_str(&g, "status").as_str(),
        "achieved" | "failed" | "expired" | "cancelled"
    ) && status == "active"
    {
        if let Some(dl) = as_f64(&g["deadline"]).filter(|d| *d != 0.0) {
            if dl < now_ts() {
                return Err(
                    ValueError("截止时间已过，请先修改截止时间 (deadline passed)".into()).into(),
                );
            }
        }
    }
    store.db.update(
        "goals",
        "id",
        gid,
        &json!({"status": status, "updated_at": now_ts(),
                "finished_at": if status == "cancelled" { json!(now_ts()) } else { Value::Null }}),
    )?;
    store.db.execute(
        "UPDATE schedules SET enabled=? WHERE id=?",
        params![
            if status == "active" { 1 } else { 0 },
            get_str(&g, "schedule_id")
        ],
    )?;
    store
        .goal(gid)?
        .ok_or_else(|| anyhow::Error::new(KeyError(gid.to_string())))
}

/// The goal a (schedule) task belongs to, if any.
pub fn goal_for_task(store: &Store, t: &Value) -> Result<Option<Value>> {
    let sid = get_str(t, "schedule_id");
    if sid.is_empty() {
        return Ok(None);
    }
    let Some(sch) = store.schedule(&sid)? else {
        return Ok(None);
    };
    let gid = get_str(&sch, "goal_id");
    if gid.is_empty() {
        return Ok(None);
    }
    store.goal(&gid)
}

/// The goal record plus its schedule's summary fields, for the UI.
pub fn brief(store: &Store, g: &Value) -> Result<Value> {
    let sid = get_str(g, "schedule_id");
    let sch = if sid.is_empty() {
        None
    } else {
        store.schedule(&sid)?
    };
    let mut out = g.as_object().cloned().unwrap_or_default();
    let pick = |k: &str| {
        sch.as_ref()
            .and_then(|s| s.get(k))
            .cloned()
            .unwrap_or(Value::Null)
    };
    out.insert(
        "check".into(),
        Value::String(sch.as_ref().map(describe).unwrap_or_default()),
    );
    out.insert("next_run".into(), pick("next_run"));
    out.insert("last_run".into(), pick("last_run"));
    out.insert("last_task".into(), pick("last_task"));
    out.insert("kind".into(), pick("kind"));
    out.insert("spec".into(), pick("spec"));
    let err = sch
        .as_ref()
        .and_then(|s| s.get("state"))
        .and_then(|s| s.get("_error"))
        .cloned()
        .unwrap_or_else(|| json!(""));
    out.insert("state_error".into(), err);
    Ok(Value::Object(out))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rt_api::testing::FakeRt;
    use chrono::Timelike;

    #[test]
    fn deadline_parse() {
        let d = parse_deadline(&json!("2030-01-01")).unwrap().unwrap();
        let lt = Local.timestamp_opt(d as i64, 0).unwrap();
        assert_eq!(lt.hour(), 23);
        let d2 = parse_deadline(&json!("2030-01-01 09:30")).unwrap().unwrap();
        assert_eq!(Local.timestamp_opt(d2 as i64, 0).unwrap().minute(), 30);
        let d3 = parse_deadline(&json!("2030-01-01T09:30")).unwrap().unwrap();
        assert_eq!(d2, d3);
        assert!(parse_deadline(&json!("next friday")).is_err());
        assert_eq!(parse_deadline(&Value::Null).unwrap(), None);
        assert_eq!(parse_deadline(&json!("")).unwrap(), None);
        assert_eq!(parse_deadline(&json!(0)).unwrap(), None);
        assert_eq!(
            parse_deadline(&json!(1700000000)).unwrap(),
            Some(1700000000.0)
        );
    }

    #[tokio::test]
    async fn create_update_and_brief() {
        let rt = FakeRt::new();
        let g = create_goal(
            &rt.store,
            "Inbox GOALSILENT",
            "keep inbox small",
            "< 20 unread",
            "interval",
            &json!("30"),
            "UTC",
            &Value::Null,
        )
        .unwrap();
        assert_eq!(g["status"], "active");
        assert_eq!(g["progress"], json!([]));
        let b = brief(&rt.store, &g).unwrap();
        assert_eq!(b["check"], "interval 30");
        assert_eq!(b["kind"], "interval");
        assert_eq!(b["state_error"], "");
        let p = goal_prompt(&g, false, "zh");
        assert!(p.starts_with("🎯 长期目标 Goal: Inbox GOALSILENT\n目标描述 Objective: keep inbox small\n完成标准 Success criteria: < 20 unread\n截止时间 Deadline: none\n"));
        assert!(p.contains("（还没有进展记录 no progress yet — this is the first check）"));
        assert!(goal_prompt(&g, true, "en").contains("This is the final check"));
        // schedule hidden behind the goal
        let sch = rt
            .store
            .schedule(g["schedule_id"].as_str().unwrap())
            .unwrap()
            .unwrap();
        assert_eq!(sch["goal_id"], g["id"]);
        assert_eq!(sch["name"], "🎯 Inbox GOALSILENT");
        let r = apply_update(rt.as_ref(), &g, "achieved", "done", "t1")
            .await
            .unwrap();
        assert_eq!(r, "已记录 recorded: 已达成（目标已结束 goal closed）");
        let g2 = rt.store.goal(g["id"].as_str().unwrap()).unwrap().unwrap();
        assert_eq!(g2["status"], "achieved");
        assert_eq!(g2["progress"][0]["note"], "done");
        let sch = rt
            .store
            .schedule(g["schedule_id"].as_str().unwrap())
            .unwrap()
            .unwrap();
        assert_eq!(sch["enabled"], 0);
        assert!(rt.calls.lock().iter().any(|(p, b)| p == "/internal/notify"
            && b["text"]
                .as_str()
                .unwrap()
                .starts_with("🎉 目标已达成：Inbox GOALSILENT")));
        // past deadline / bad input
        let e = create_goal(
            &rt.store,
            "x",
            "y",
            "",
            "interval",
            &json!("30"),
            "UTC",
            &json!("2020-01-01"),
        )
        .unwrap_err();
        assert!(e.downcast_ref::<ValueError>().is_some());
        let e = set_status(&rt.store, "goal_nope", "paused").unwrap_err();
        assert!(e.downcast_ref::<KeyError>().is_some());
        let paused = set_status(&rt.store, g["id"].as_str().unwrap(), "paused").unwrap();
        assert_eq!(paused["status"], "paused");
    }
}
