//! Chat & conversations, tasks, and the live event stream (SSE).

use std::collections::BTreeSet;
use std::convert::Infallible;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use axum::body::{Body, Bytes};
use axum::extract::Path as UrlPath;
use axum::http::{header, HeaderMap};
use axum::response::{IntoResponse, Response};
use axum::Json;
use omuse_common::util::{get_str, py_str, take_chars, truthy};
use omuse_common::{params, pyjson};
use serde_json::{json, Map, Value};

use super::{body_obj, http, int_param, not_found, Fail, St, Q, R};
use crate::agent::util::realpath;
use crate::agent::{is_terminal, Runtime};
use crate::attachments::{self as at, AttachmentError, UPLOAD_MAX, UPLOAD_MAX_FILES};
use crate::state::Hub;

fn too_large() -> Fail {
    http(
        413,
        format!("文件超过 {} MB (file too large)", UPLOAD_MAX / 1024 / 1024),
    )
}

/// The chat's ＋ button: the raw file is the request body; it is stored under workspace/uploads/<YYYY-MM>/.
pub async fn upload(st: St, q: Q, headers: HeaderMap, body: Body) -> R {
    let declared = headers
        .get(header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.trim().parse::<u64>().ok())
        .unwrap_or(0);
    if declared > UPLOAD_MAX as u64 {
        return Err(too_large());
    }
    // no Content-Length (chunked): read at most one byte more than allowed, save_upload refuses the rest
    let data = axum::body::to_bytes(body, UPLOAD_MAX + 1)
        .await
        .map_err(|_| too_large())?;
    let name = q.get("name").cloned().unwrap_or_default();
    let ws = st.rt.workspace.clone();
    let res = tokio::task::spawn_blocking(move || at::save_upload(&ws, &name, &data))
        .await
        .map_err(|e| Fail::Internal(e.into()))?;
    let info = match res {
        Ok(i) => i,
        Err(e) => match e.downcast_ref::<AttachmentError>() {
            Some(a) => return Err(http(400, a.0.clone())),
            None => return Err(e.into()),
        },
    };
    st.rt
        .audit(
            "user",
            "file.upload",
            "",
            json!({"resource": info["path"], "detail": {"size": info["size"], "mime": info["mime"]}}),
        )
        .await;
    Ok(Json(info))
}

/// The files a chat message refers to: each must be an existing file under workspace/uploads/.
fn attachments(ws: &Path, raw: Option<&Value>) -> Result<Vec<Value>, Fail> {
    let list: Vec<Value> = match raw {
        None => vec![],
        Some(v) if !truthy(v) => vec![],
        Some(Value::Array(a)) => a.iter().take(UPLOAD_MAX_FILES).cloned().collect(),
        Some(_) => {
            return Err(Fail::Internal(anyhow::anyhow!(
                "TypeError: attachments must be a list"
            )))
        }
    };
    let wss = ws.to_string_lossy().to_string();
    let uploads = format!("{}/uploads/", wss);
    let mut out = Vec::new();
    for x in list {
        let s = match &x {
            Value::Object(o) => o.get("path").map(py_str).unwrap_or_else(|| "None".into()),
            v if truthy(v) => py_str(v),
            _ => String::new(),
        };
        let rel = s.trim().trim_start_matches('/').to_string();
        let fp = realpath(&ws.join(&rel));
        let fps = fp.to_string_lossy();
        if !rel.starts_with("uploads/") || !fps.starts_with(&uploads) || !fp.is_file() {
            return Err(http(
                400,
                format!("附件不存在 attachment not found: {}", rel),
            ));
        }
        out.push(at::info(ws, &fp)?);
    }
    Ok(out)
}

pub async fn chat(st: St, body: Bytes) -> R {
    let b = body_obj(&body)?;
    let rt = &st.rt;
    let mut text = super::pstr(&b, "message", "").trim().to_string();
    let atts = attachments(&rt.workspace, b.get("attachments"))?;
    if text.is_empty() && !atts.is_empty() {
        let en = get_str(&rt.store.settings()?, "language") == "en";
        text = if en {
            "Please look at the attached file(s)."
        } else {
            "请看一下附件。"
        }
        .to_string();
    }
    if text.is_empty() {
        return Err(http(400, "message is empty"));
    }
    let mut cid = b
        .get("conversation_id")
        .filter(|v| truthy(v))
        .map(py_str)
        .unwrap_or_default();
    if cid.is_empty() || rt.store.conv(&cid)?.is_none() {
        cid = rt.store.create_conv(take_chars(&text, 40), "chat", None)?;
        // a new conversation in a bot's view belongs to that bot (crate::bots); unknown ids stay with the main bot
        let bot = super::pstr(&b, "bot", "");
        if let Some(bot) = crate::bots::get(bot.trim()) {
            rt.store.set_conv_bot(&cid, &bot.id, "")?;
        }
    }
    let atts_v = Value::Array(atts.clone());
    let meta = if atts.is_empty() {
        None
    } else {
        Some(json!({"attachments": atts_v}))
    };
    rt.store.add_msg(&cid, "user", &text, "", meta.as_ref())?;
    let t = rt
        .submit(
            &cid,
            &text,
            "chat",
            "",
            if atts.is_empty() { None } else { Some(&atts_v) },
        )
        .await?;
    let tid = get_str(&t, "id");
    rt.store.db.execute(
        "UPDATE messages SET task_id=? WHERE id=(SELECT MAX(id) FROM messages WHERE conv_id=? AND role='user')",
        params![tid.as_str(), cid.as_str()],
    )?;
    st.publish(json!({"kind": "conv_update", "conv_id": cid}));
    Ok(Json(json!({"conversation_id": cid, "task_id": tid})))
}

pub async fn conversations(st: St, q: Q) -> R {
    let mut rows = st.rt.store.convs(200)?;
    if let Some(kind) = q.get("kind").filter(|k| !k.is_empty()) {
        rows.retain(|r| get_str(r, "kind") == *kind);
    }
    if let Some(bot) = q.get("bot").filter(|k| !k.is_empty()) {
        // conversations saved before bots existed have no bot: they are the main bot's
        rows.retain(|r| {
            let b = get_str(r, "bot_id");
            b == *bot || (b.is_empty() && bot == crate::bots::MAIN)
        });
    }
    Ok(Json(json!({"conversations": rows})))
}

/// `GET /api/bots`: the bots, the main one first, each with the skills it may use now (enabled ones only).
pub async fn bots(st: St) -> R {
    let list: Vec<Value> = crate::bots::all()
        .iter()
        .map(|b| {
            let mut v = b.to_json();
            v["skills"] = json!(st
                .rt
                .bot_skills(b)
                .iter()
                .map(|s| get_str(s, "name"))
                .collect::<Vec<_>>());
            v
        })
        .collect();
    Ok(Json(json!({"bots": list})))
}

pub async fn conversation(st: St, UrlPath(cid): UrlPath<String>) -> R {
    let store = &st.rt.store;
    let Some(c) = store.conv(&cid)? else {
        return Err(not_found());
    };
    let msgs = store.msgs(&cid, 300)?;
    let tids: BTreeSet<String> = msgs
        .iter()
        .filter(|m| m.get("task_id").is_some_and(truthy))
        .map(|m| get_str(m, "task_id"))
        .collect();
    let mut tasks = Map::new();
    for tid in tids {
        if let Some(t) = store.task(&tid)? {
            tasks.insert(tid, Runtime::task_brief(&t));
        }
    }
    Ok(Json(
        json!({"conversation": c, "messages": msgs, "tasks": tasks}),
    ))
}

pub async fn del_conv(st: St, UrlPath(cid): UrlPath<String>) -> R {
    st.rt.store.delete_conv(&cid)?;
    Ok(Json(json!({"ok": true})))
}

pub async fn tasks(st: St, q: Q) -> R {
    let limit = int_param(&q, "query", "limit", Some(100))?;
    let status = q.get("status").map(String::as_str);
    Ok(Json(
        json!({"tasks": st.rt.store.tasks(status, limit.min(500), None)?}),
    ))
}

pub async fn task(st: St, UrlPath(tid): UrlPath<String>) -> R {
    let Some(t) = st.rt.store.task(&tid)? else {
        return Err(not_found());
    };
    let mut brief = Runtime::task_brief(&t);
    brief["events"] = Value::Array(st.rt.store.events(&tid, 0)?);
    Ok(Json(brief))
}

pub async fn task_action(st: St, UrlPath((tid, action)): UrlPath<(String, String)>) -> R {
    let rt = &st.rt;
    let Some(t) = rt.store.task(&tid)? else {
        return Err(not_found());
    };
    match action.as_str() {
        "cancel" => rt.cancel(&tid, "").await,
        "pause" => rt.pause(&tid).await,
        "resume" => rt.resume(&tid).await,
        "retry" => {
            if !is_terminal(&get_str(&t, "status")) {
                return Err(http(409, "task still active"));
            }
            let att = t.get("attachments").filter(|a| truthy(a));
            let nt = rt
                .submit(
                    &get_str(&t, "conv_id"),
                    &get_str(&t, "goal"),
                    &get_str(&t, "source"),
                    &get_str(&t, "schedule_id"),
                    att,
                )
                .await?;
            return Ok(Json(json!({"ok": true, "task_id": nt["id"]})));
        }
        _ => return Err(http(404, "unknown action")),
    }
    Ok(Json(json!({"ok": true})))
}

/// Removes the subscriber when the response stream is dropped (client gone).
struct Unsubscribe(Arc<Hub>, u64);

impl Drop for Unsubscribe {
    fn drop(&mut self) {
        self.0.unsubscribe(self.1);
    }
}

/// Seconds without an event before a ping is sent.
pub const PING_SECS: u64 = 15;

/// Live events (SSE): `retry: 3000`, then one `data: <json>` frame per event; a `{"kind": "ping"}` event after 15 s
/// of silence so the page can tell a live stream from a silently dead one.
pub async fn stream(st: St) -> Response {
    let (id, mut rx) = st.hub.subscribe();
    let guard = Unsubscribe(st.hub.clone(), id);
    let s = async_stream::stream! {
        let _guard = guard;
        yield Ok::<Bytes, Infallible>(Bytes::from_static(b"retry: 3000\n\n"));
        loop {
            match tokio::time::timeout(Duration::from_secs(PING_SECS), rx.recv()).await {
                Ok(Some(ev)) => yield Ok(Bytes::from(format!("data: {}\n\n", pyjson::dumps(&ev)))),
                Ok(None) => break,
                Err(_) => yield Ok(Bytes::from_static(b"data: {\"kind\": \"ping\"}\n\n")),
            }
        }
    };
    (
        [
            (header::CONTENT_TYPE, "text/event-stream; charset=utf-8"),
            (header::CACHE_CONTROL, "no-cache"),
            (header::HeaderName::from_static("x-accel-buffering"), "no"),
        ],
        Body::from_stream(s),
    )
        .into_response()
}
