//! Skills, the research library, memory (facts, entities, profile, consolidation) and the context preview.

use std::collections::BTreeSet;
use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::Path as UrlPath;
use axum::Json;
use omuse_common::util::{get_str, py_str, take_chars, truthy};
use serde_json::{json, Map, Value};

use super::{body_obj, getv, http, int_error, not_found, parse_int, pstr, pstr_or, Fail, St, Q, R};
use crate::agent::is_terminal;
use crate::library as lib;
use crate::memory_tidy as mt;
use crate::state::State;
use crate::store::{looks_sensitive, profile_key, FactOpts, DOMAINS, PROFILE_FIELDS};

const VAULT_MSG: &str =
    "证件号、卡号和密码请放进保险箱 (ID / card numbers and passwords belong in the vault)";

fn without_path(v: &Value) -> Value {
    super::filter_obj(v, |k| k != "path")
}

// ------------------------------------------------------------------ skills
pub async fn skills_list(st: St) -> R {
    let sk = &st.rt.skills_dirs;
    let mut out = Vec::new();
    for x in sk.listing(&st.rt.store.settings()?) {
        let mut x = without_path(&x);
        if get_str(&x, "source") == "imported" {
            x["url"] = json!(sk.source_link(&get_str(&x, "name")));
        }
        out.push(x);
    }
    // env: informational only (seed at first start)
    let env = st
        .cfg
        .skills_initial()
        .map(|v| v.join(","))
        .unwrap_or_default();
    Ok(Json(json!({"skills": out, "env": env.trim()})))
}

/// Python `for x in (v or [])`: a list's items, a string's characters, an object's keys.
fn py_iter(v: Option<&Value>) -> Vec<String> {
    match v {
        Some(Value::Array(a)) => a.iter().map(py_str).collect(),
        Some(Value::String(s)) => s.chars().map(String::from).collect(),
        Some(Value::Object(o)) => o.keys().cloned().collect(),
        _ => vec![],
    }
}

/// Which skills are on: the body lists the names that are off (an empty list = everything on).
pub async fn skills_select(st: St, body: Bytes) -> R {
    let b = body_obj(&body)?;
    let sk = &st.rt.skills_dirs;
    let known: BTreeSet<String> = sk.all_skills().iter().map(|s| get_str(s, "name")).collect();
    let off: BTreeSet<String> = py_iter(b.get("disabled"))
        .into_iter()
        .filter(|x| known.contains(x))
        .collect();
    let off: Vec<String> = off.into_iter().collect();
    st.rt.store.set_settings(&json!({"skills_disabled": off}))?;
    st.rt
        .audit(
            "user",
            "skills.select",
            "",
            json!({"detail": {"disabled": off}}),
        )
        .await;
    let list: Vec<Value> = sk
        .listing(&st.rt.store.settings()?)
        .iter()
        .map(without_path)
        .collect();
    Ok(Json(json!({"skills": list})))
}

pub async fn skills_import(st: St, body: Bytes) -> R {
    let b = body_obj(&body)?;
    let info = st
        .rt
        .skills_dirs
        .import_from_github(&pstr_or(&b, "url", ""))
        .await
        .map_err(|e| http(400, e.0))?;
    st.rt
        .audit(
            "user",
            "skills.import",
            "",
            json!({"resource": info["name"], "detail": {"url": info["url"]}}),
        )
        .await;
    Ok(Json(json!({"skill": info})))
}

async fn delete_skill(st: &State, name: &str) -> R {
    if !st.rt.skills_dirs.delete_imported(name) {
        return Err(http(
            404,
            "只能删除导入的技能 (only imported skills can be removed)",
        ));
    }
    if let Some(Value::Array(cur)) = st.rt.store.settings()?.get("skills_disabled") {
        let keep: Vec<Value> = cur
            .iter()
            .filter(|x| x.as_str() != Some(name))
            .cloned()
            .collect();
        st.rt
            .store
            .set_settings(&json!({"skills_disabled": keep}))?;
    }
    st.rt
        .audit("user", "skills.delete", "", json!({"resource": name}))
        .await;
    Ok(Json(Value::Null))
}

pub async fn skills_delete(st: St, UrlPath(name): UrlPath<String>) -> R {
    delete_skill(&st, &name).await
}

/// `DELETE /api/skills/import` reaches `/api/skills/{name}` in FastAPI.
pub async fn skills_delete_import(st: St) -> R {
    delete_skill(&st, "import").await
}

// ------------------------------------------------------------------ research library
async fn start_research(st: &State, sub: &Value, refresh: bool) -> anyhow::Result<Value> {
    let rt = &st.rt;
    let lang = pstr_or(&rt.store.settings()?, "language", "zh");
    let sid = get_str(sub, "id");
    rt.store
        .update_subject(&sid, &json!({"status": "researching", "error": ""}))?;
    let conv = get_str(sub, "conv_id");
    let t = rt
        .submit(
            &conv,
            &lib::research_goal(sub, refresh, &lang),
            "library",
            "",
            None,
        )
        .await?;
    rt.store
        .update_subject(&sid, &json!({"last_task": t["id"]}))?;
    st.publish(json!({"kind": "conv_update", "conv_id": conv}));
    st.publish(json!({"kind": "library_update", "subject_id": sid}));
    Ok(t)
}

pub async fn library_list(st: St) -> R {
    Ok(Json(json!({"subjects": st.rt.store.subjects()?})))
}

/// A new research subject: its report file, its own conversation, and the first research task.
pub async fn library_create(st: St, body: Bytes) -> R {
    let b = body_obj(&body)?;
    let rt = &st.rt;
    let title = pstr_or(&b, "title", "").trim().to_string();
    if title.is_empty() {
        return Err(http(400, "title required"));
    }
    let mut rec = lib::new_subject(&title, &pstr_or(&b, "brief", ""));
    rec["conv_id"] = json!(rt.store.create_conv(&title, "research", None)?);
    let sub = rt.store.create_subject(&rec)?;
    let _ = std::fs::create_dir_all(rt.workspace.join("library").join(get_str(&sub, "slug")));
    rt.audit(
        "user",
        "library.create",
        "",
        json!({"detail": {"title": title, "slug": sub["slug"]}}),
    )
    .await;
    let t = start_research(&st, &sub, false).await?;
    Ok(Json(
        json!({"subject": rt.store.subject(&get_str(&sub, "id"))?, "task_id": t["id"]}),
    ))
}

pub async fn library_search(st: St, q: Q) -> R {
    let query = q.get("q").cloned().unwrap_or_default();
    Ok(Json(
        json!({"hits": st.rt.store.library_search(&query, 10)?}),
    ))
}

pub async fn library_get(st: St, UrlPath(sid): UrlPath<String>) -> R {
    let Some(sub) = st.rt.store.subject(&sid)? else {
        return Err(not_found());
    };
    let slug = get_str(&sub, "slug");
    let report = lib::read_report(&st.rt.workspace, &slug);
    Ok(Json(
        json!({"subject": sub, "report": report, "path": lib::report_path(&slug)}),
    ))
}

pub async fn library_refresh(st: St, UrlPath(sid): UrlPath<String>) -> R {
    let rt = &st.rt;
    let Some(sub) = rt.store.subject(&sid)? else {
        return Err(not_found());
    };
    let lt = get_str(&sub, "last_task");
    if get_str(&sub, "status") == "researching" && !lt.is_empty() {
        let status = rt
            .store
            .task(&lt)?
            .map(|t| get_str(&t, "status"))
            .unwrap_or_default();
        if !is_terminal(&status) {
            return Err(http(409, "这个课题正在研究中 (research already running)"));
        }
    }
    rt.audit(
        "user",
        "library.refresh",
        "",
        json!({"detail": {"slug": sub["slug"]}}),
    )
    .await;
    let t = start_research(&st, &sub, true).await?;
    Ok(Json(
        json!({"subject": rt.store.subject(&sid)?, "task_id": t["id"]}),
    ))
}

async fn delete_subject(st: &State, sid: &str) -> R {
    let Some(sub) = st.rt.store.subject(sid)? else {
        return Err(not_found());
    };
    st.rt.store.delete_subject(sid)?;
    st.rt
        .audit(
            "user",
            "library.delete",
            "",
            json!({"detail": {"slug": sub["slug"]}}),
        )
        .await;
    st.publish(json!({"kind": "library_update", "subject_id": sid}));
    Ok(Json(json!({"ok": true})))
}

/// Removes the subject and its index; the report files stay in the workspace and the chat stays in the list.
pub async fn library_delete(st: St, UrlPath(sid): UrlPath<String>) -> R {
    delete_subject(&st, &sid).await
}

/// `DELETE /api/library/search` reaches `/api/library/{sid}` in FastAPI.
pub async fn library_delete_search(st: St) -> R {
    delete_subject(&st, "search").await
}

// ------------------------------------------------------------------ memory
pub async fn memory(st: St) -> R {
    let s = &st.rt.store;
    let runs: Vec<Value> = s
        .memory_runs(10)?
        .iter()
        .map(|r| {
            let rep = r
                .get("report")
                .filter(|v| truthy(v))
                .cloned()
                .unwrap_or_else(|| json!({}));
            let list = |k: &str| {
                rep.get(k)
                    .filter(|v| truthy(v))
                    .cloned()
                    .unwrap_or_else(|| json!([]))
            };
            json!({"id": r["id"], "ts": r["ts"], "kind": r["kind"], "lines": list("lines"),
                   "errors": list("errors"), "applied": getv(&rep, "applied")})
        })
        .collect();
    let settings = s.settings()?;
    let mut sub = Map::new();
    for k in [
        "memory_consolidation",
        "memory_consolidate_at",
        "memory_extraction",
    ] {
        sub.insert(k.into(), getv(&settings, k));
    }
    let triple = |list: &[(&str, &str, &str)]| -> Vec<Value> {
        list.iter()
            .map(|(k, zh, en)| json!({"key": k, "zh": zh, "en": en}))
            .collect()
    };
    Ok(Json(json!({
        "facts": s.facts(500, Some("long"))?, "recent": s.facts(1000, Some("recent"))?, "episodes": s.episodes(50)?,
        "domains": triple(&DOMAINS), "entities": s.entities(None)?,
        "profile": s.profile()?, "profile_fields": triple(&PROFILE_FIELDS),
        "pending": s.profile_pending("pending")?, "runs": runs, "job": mt::job_state(),
        "needs_first_review": mt::needs_first_review(s)?, "library": s.subjects()?,
        "settings": sub,
    })))
}

pub async fn add_memory(st: St, body: Bytes) -> R {
    let b = body_obj(&body)?;
    let fact = pstr(&b, "fact", "");
    if looks_sensitive(&fact) {
        return Err(http(400, VAULT_MSG));
    }
    let o = FactOpts {
        source: "user-ui".into(),
        confidence: 1.0,
        tier: if b.get("tier").and_then(Value::as_str) == Some("recent") {
            "recent"
        } else {
            "long"
        }
        .into(),
        domain: pstr_or(&b, "domain", ""),
        ..Default::default()
    };
    let r = st.rt.store.add_fact(
        &fact,
        &pstr_or(&b, "category", "preference"),
        &pstr_or(&b, "entity", ""),
        &o,
    )?;
    let Some(r) = r else {
        return Err(http(400, "empty fact"));
    };
    st.rt
        .audit(
            "user",
            "memory.add",
            "",
            json!({"detail": {"fact": r["fact"]}}),
        )
        .await;
    Ok(Json(r))
}

async fn edit_fact(st: &State, fid: &str, body: &Bytes) -> R {
    let b = body_obj(body)?;
    let mut kw = Map::new();
    if let Some(f) = b
        .get("fact")
        .and_then(Value::as_str)
        .filter(|f| !f.trim().is_empty())
    {
        if looks_sensitive(f) {
            return Err(http(400, VAULT_MSG));
        }
        kw.insert("fact".into(), json!(take_chars(f.trim(), 500)));
    }
    if let Some(t) = b
        .get("tier")
        .and_then(Value::as_str)
        .filter(|t| matches!(*t, "long" | "recent"))
    {
        kw.insert("tier".into(), json!(t));
    }
    if let Some(c) = b.get("category").and_then(Value::as_str) {
        kw.insert("category".into(), json!(take_chars(c, 30)));
    }
    if let Some(d) = b.get("domain").and_then(Value::as_str) {
        kw.insert("domain".into(), json!(d));
    }
    if let Some(s) = b
        .get("status")
        .and_then(Value::as_str)
        .filter(|s| matches!(*s, "active" | "pending"))
    {
        kw.insert("status".into(), json!(s)); // "active" = the user confirmed something OMuse learned
    }
    if let Some(e) = b.get("entity").and_then(Value::as_str) {
        kw.insert("entity".into(), json!(take_chars(e.trim(), 60)));
    }
    let kw = Value::Object(kw);
    let Some(r) = st.rt.store.update_fact(fid, &kw)? else {
        return Err(http(404, "not found"));
    };
    st.rt
        .audit(
            "user",
            "memory.edit",
            "",
            json!({"resource": fid, "detail": kw}),
        )
        .await;
    Ok(Json(r))
}

pub async fn edit_memory(st: St, UrlPath(fid): UrlPath<String>, body: Bytes) -> R {
    edit_fact(&st, &fid, &body).await
}

/// `PUT /api/memory/consolidate` reaches `/api/memory/{fid}` in FastAPI.
pub async fn edit_memory_consolidate(st: St, body: Bytes) -> R {
    edit_fact(&st, "consolidate", &body).await
}

/// What OMuse would know going into a request (Memory page → "试一试"; also used by the batch 2 tests).
pub async fn context_preview(st: St, q: Q) -> R {
    let query = q.get("q").cloned().unwrap_or_default();
    let mode = q.get("mode").cloned().unwrap_or_else(|| "v1".into());
    let rt = &st.rt;
    let facts = if mode == "legacy" {
        rt.facts_legacy(&query)
    } else {
        let ents = rt.store.entities(None)?;
        crate::context::select(
            &query,
            &rt.store.facts(500, Some("long"))?,
            Some(&ents),
            12,
            "",
        )
    };
    let list: Vec<Value> = facts
        .iter()
        .map(|f| {
            json!({"id": f["id"], "fact": f["fact"], "domain": getv(f, "domain"), "status": getv(f, "status"),
                   "why": f.get("why").cloned().unwrap_or_else(|| json!("")), "score": getv(f, "score")})
        })
        .collect();
    Ok(Json(json!({"mode": mode, "facts": list})))
}

pub async fn edit_entity(st: St, UrlPath(eid): UrlPath<String>, body: Bytes) -> R {
    static SEP: std::sync::LazyLock<regex::Regex> =
        std::sync::LazyLock::new(|| regex::Regex::new(r"[,，、;；]").unwrap());
    let b = body_obj(&body)?;
    let aliases: Option<Vec<String>> = match b.get("aliases") {
        Some(Value::String(s)) => Some(SEP.split(s).map(String::from).collect()),
        Some(Value::Array(a)) => Some(a.iter().map(py_str).collect()),
        _ => None,
    };
    let relation = b.get("relation").and_then(Value::as_str);
    let kind = b.get("type").and_then(Value::as_str);
    let Some(r) = st
        .rt
        .store
        .update_entity(&eid, aliases.as_deref(), relation, kind)?
    else {
        return Err(http(404, "not found"));
    };
    let mut detail = Map::new();
    for k in ["aliases", "relation", "type"] {
        detail.insert(k.into(), getv(&b, k));
    }
    st.rt
        .audit(
            "user",
            "memory.entity",
            "",
            json!({"resource": eid, "detail": detail}),
        )
        .await;
    st.rt.publish(json!({"kind": "memory_update"})).await;
    Ok(Json(r))
}

pub async fn del_entity(st: St, UrlPath(eid): UrlPath<String>) -> R {
    st.rt.store.delete_entity(&eid)?;
    Ok(Json(json!({"ok": true})))
}

async fn forget(st: &State, fid: &str) -> R {
    st.rt.store.delete_fact(fid)?;
    st.rt
        .audit("user", "memory.forget", "", json!({"resource": fid}))
        .await;
    Ok(Json(json!({"ok": true})))
}

pub async fn del_memory(st: St, UrlPath(fid): UrlPath<String>) -> R {
    forget(&st, &fid).await
}

/// `DELETE /api/memory/consolidate` reaches `/api/memory/{fid}` in FastAPI.
pub async fn del_memory_consolidate(st: St) -> R {
    forget(&st, "consolidate").await
}

/// The user's own edit on the Memory page (this is the confirmation).
pub async fn set_profile(st: St, body: Bytes) -> R {
    let b = body_obj(&body)?;
    let key = profile_key(&pstr(&b, "key", ""));
    let val = pstr_or(&b, "value", "");
    if key.is_empty() {
        return Err(http(400, "unknown field"));
    }
    if looks_sensitive(&val) {
        return Err(http(400, VAULT_MSG));
    }
    st.rt.store.set_profile(&key, &val, "user-ui")?;
    st.rt
        .audit("user", "profile.set", "", json!({"resource": key}))
        .await;
    Ok(Json(json!({"profile": st.rt.store.profile()?})))
}

pub async fn resolve_profile(st: St, UrlPath(pid): UrlPath<String>, body: Bytes) -> R {
    let b = body_obj(&body)?;
    let val = b
        .get("value")
        .and_then(Value::as_str)
        .filter(|v| !v.trim().is_empty());
    let accept = b.get("accept").is_some_and(truthy);
    let Some(r) = st.rt.store.resolve_profile(&pid, accept, val)? else {
        return Err(http(409, "already resolved"));
    };
    let action = if get_str(&r, "status") == "accepted" {
        "profile.accept"
    } else {
        "profile.reject"
    };
    st.rt
        .audit("user", action, "", json!({"resource": r["key"]}))
        .await;
    st.publish(json!({"kind": "memory_update"}));
    Ok(Json(r))
}

/// `{dry_run: true}` → preview; `{from_run: <id>}` → apply that preview exactly; `{}` → plan and apply now.
pub async fn consolidate(st: St, body: Bytes) -> R {
    let b = body_obj(&body)?;
    let fr = pstr_or(&b, "from_run", "");
    let from_run = if !fr.is_empty() && fr.chars().all(|c| c.is_ascii_digit()) {
        fr.parse::<i64>().ok()
    } else {
        None
    };
    let dry = b.get("dry_run").is_some_and(truthy) && from_run.unwrap_or(0) == 0;
    let rt: Arc<dyn mt::TidyRuntime> = st.rt.clone();
    if !mt::start(rt, dry, "manual", from_run) {
        return Err(http(409, "正在整理中 (a tidy run is already in progress)"));
    }
    Ok(Json(json!({"started": true})))
}

pub async fn memory_run(st: St, UrlPath(rid): UrlPath<String>) -> R {
    let rid = parse_int(&rid).ok_or_else(|| int_error("path", "rid", &rid))?;
    let r = st
        .rt
        .store
        .memory_runs(50)?
        .into_iter()
        .find(|x| x.get("id").and_then(Value::as_i64) == Some(rid));
    r.map(Json)
        .ok_or_else(|| -> Fail { http(404, "not found") })
}
