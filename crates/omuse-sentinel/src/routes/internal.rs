//! `/internal/*`: the runtime's API (runtime token required; checked by the router layer).

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use axum::body::Bytes;
use axum::extract::{Query, State};
use axum::Json;
use omuse_common::api::ApiError;
use omuse_common::util::{get_str, py_str, take_chars, truthy};
use omuse_mail::{mailboxes, mailproviders};
use omuse_mcp::mcp_hub;
use omuse_sentinel_core::store::Audit;
use omuse_sentinel_core::{catalog, ledger, vault, ALLOW, DENY};
use serde_json::{json, Map, Value};

use super::deps::{self, BotExt, BrokerOut};
use super::{
    body_obj, g, internal as ierr, obj, s_cut, s_opt, s_or, App, JsonResult, CONNECTORS,
    TG_FILE_MAX,
};

fn audit_task(task_id: String, resource: String, result: &str, detail: Value) -> Audit {
    Audit {
        task_id,
        resource,
        result: result.into(),
        detail: Some(detail),
        ..Default::default()
    }
}

/// `GET /internal/catalog`: the tools the agent may use right now, connection status, risk per tool.
pub async fn catalog(State(ctx): State<App>) -> JsonResult {
    let store = &ctx.store;
    let mut enabled: HashSet<String> = HashSet::new();
    let mut status = Map::new();
    for name in CONNECTORS {
        let c = store.connection(name).map_err(ierr)?;
        let conf = &c["config"];
        let mut ready = truthy(&c["enabled"]);
        if matches!(name, "notion" | "slack" | "calendar" | "drive") {
            ready = ready && store.has_secret(&format!("cred_{name}_1")).unwrap_or(false);
        }
        if name == "travel" {
            ready = true; // flights_search always works (Kiwi.com data); the hotel tools say when they need the key
        }
        if name == "search" {
            ready = true; // web_search is always offered: the browser's engines answer when no service does
        }
        if name == "gmail" {
            ready = ready && ctx.gmail_ready();
        }
        if name == "telegram" {
            ready = ready
                && store.has_secret("cred_telegram_1").unwrap_or(false)
                && conf.get("chat_id").is_some_and(truthy);
        }
        if name == "phone" {
            ready = ready && ctx.phone.ready();
        }
        if ready {
            enabled.insert(name.to_string());
        }
        // Python precedence: `workspace or team or ("" if notion/slack else "")`
        let mut workspace = [conf.get("workspace"), conf.get("team")]
            .into_iter()
            .flatten()
            .find(|v| truthy(v))
            .cloned()
            .unwrap_or(json!(""));
        if name == "search" {
            // the providers with a key, in the order they are tried, for the agent's prompt
            let names: Vec<String> = crate::actions::search_keys(&ctx)
                .into_iter()
                .map(|(p, _, _)| p.id().to_string())
                .collect();
            let last = get_str(conf, "last_ok");
            workspace = json!(if last.is_empty() {
                names.join(", ")
            } else {
                format!("{} (last used: {last})", names.join(", "))
            });
        }
        if name == "travel" {
            let keyed = store.has_secret("cred_travel_1").unwrap_or(false)
                || !ctx.cfg.search.serpapi_key.trim().is_empty();
            workspace = json!(if keyed {
                "google flights + google hotels + google maps places (key)"
            } else {
                "flights: kiwi.com data (free, information only); hotels and restaurants: need the key"
            });
        }
        let accounts: Value = if name == "gmail" {
            let mut accs = mailboxes::ready_accounts(store).map_err(ierr)?;
            let def = mailboxes::default_id(store).map_err(ierr)?;
            accs.sort_by_key(|a| get_str(a, "id") != def);
            json!(accs.iter().map(|a| a["email"].clone()).collect::<Vec<_>>())
        } else {
            json!([])
        };
        let providers: Value = if name == "gmail" {
            let mut m = Map::new();
            for a in mailboxes::ready_accounts(store).map_err(ierr)? {
                m.insert(
                    get_str(&a, "email"),
                    json!(mailproviders::label(&get_str(&a, "provider"))),
                );
            }
            Value::Object(m)
        } else {
            json!({})
        };
        status.insert(
            name.to_string(),
            obj(vec![
                ("ready", json!(ready)),
                ("permissions", c["permissions"].clone()),
                ("workspace", workspace),
                (
                    "account",
                    if matches!(name, "gmail" | "calendar" | "drive") {
                        conf.get("email").cloned().unwrap_or(json!(""))
                    } else {
                        json!("")
                    },
                ),
                (
                    "time_zone",
                    if name == "calendar" {
                        conf.get("time_zone").cloned().unwrap_or(json!(""))
                    } else {
                        json!("")
                    },
                ),
                (
                    "read_only",
                    json!(name == "calendar" && deps::calendar_read_only(store)),
                ),
                (
                    "lines",
                    json!(if name == "phone" && ready {
                        ctx.phone.lines_summary()
                    } else {
                        String::new()
                    }),
                ),
                ("accounts", accounts),
                ("providers", providers),
            ]),
        );
    }
    let mcp = mcp_hub::status(store).map_err(ierr)?;
    for x in mcp["servers"].as_array().cloned().unwrap_or_default() {
        if truthy(&x["enabled"]) {
            enabled.insert(format!("mcp:{}", get_str(&x, "id")));
        }
    }
    status.insert("mcp".into(), mcp);
    let read_only = deps::calendar_read_only(store);
    let tools = catalog::snapshot();
    let allowed = |name: &str| -> bool {
        let Some(t) = tools.get(name) else {
            return false;
        };
        let connector = get_str(t, "connector");
        if connector.starts_with("mcp:") {
            return true; // sync_catalog only publishes enabled, reviewed, non-"off" tools
        }
        if connector == "calendar" && get_str(t, "capability") == "write" && read_only {
            return false; // read-only iCal feed: don't offer writes the agent can't do
        }
        store
            .connection(&connector)
            .map(|c| {
                c["permissions"]
                    .get(get_str(t, "capability"))
                    .is_some_and(truthy)
            })
            .unwrap_or(false)
    };
    let mut schemas: Vec<Value> = catalog::llm_schemas(Some(&enabled))
        .into_iter()
        .filter(|s| allowed(&get_str(&s["function"], "name")))
        .collect();
    if vault::list_items(store).map_err(ierr)?.is_empty() {
        schemas.retain(|s| get_str(&s["function"], "name") != "browser_fill_secret");
    }
    let mut risk = Map::new();
    for (k, v) in tools.iter() {
        risk.insert(k.clone(), v["risk"].clone());
    }
    Ok(Json(
        json!({"tools": schemas, "connections": status, "risk": risk}),
    ))
}

/// `POST /internal/audit`: the runtime writes to the audit log.
pub async fn audit(State(ctx): State<App>, body: Bytes) -> JsonResult {
    let b = body_obj(&body)?;
    let mut actor = s_or(&b, "actor", "runtime");
    if ![
        "runtime",
        "planner",
        "executor",
        "llm",
        "memory",
        "scheduler",
        "subagent",
        "user",
    ]
    .contains(&actor.as_str())
    {
        actor = "runtime".into();
    }
    let detail = match b.get("detail") {
        Some(d) if truthy(d) => d.clone(),
        _ => json!({}),
    };
    let rec = ctx
        .store
        .audit(
            &actor,
            &s_cut(&b, "action", "", 100),
            Audit {
                task_id: s_or(&b, "task_id", ""),
                resource: s_cut(&b, "resource", "", 200),
                risk: s_or(&b, "risk", ""),
                decision: s_or(&b, "decision", ""),
                result: s_cut(&b, "result", "", 50),
                detail: Some(detail),
            },
        )
        .map_err(ierr)?;
    Ok(Json(json!({"seq": rec["seq"]})))
}

/// `POST /internal/notify`: a Telegram message to the owner.
pub async fn notify(State(ctx): State<App>, body: Bytes) -> JsonResult {
    let b = body_obj(&body)?;
    let conn = ctx.store.connection("telegram").map_err(ierr)?;
    if !(truthy(&conn["enabled"]) && ctx.store.has_secret("cred_telegram_1").unwrap_or(false)) {
        return Ok(Json(json!({"sent": false})));
    }
    match deps::telegram_send(&ctx, &s_or(&b, "text", "")).await {
        Ok(_) => {
            ctx.audit(
                "sentinel",
                "notify.telegram",
                Audit {
                    task_id: s_or(&b, "task_id", ""),
                    decision: ALLOW.into(),
                    result: "success".into(),
                    ..Default::default()
                },
            );
            Ok(Json(json!({"sent": true})))
        }
        Err(e) => Ok(Json(json!({"sent": false, "error": e.to_string()}))),
    }
}

/// A broker call for the render endpoints: the JSON answer or the `ActionError` text.
async fn render(ctx: &App, path: &str, body: &Value, timeout: f64) -> Result<Value, String> {
    deps::broker(ctx, "POST", path, Some(body), timeout)
        .await
        .map(BrokerOut::json)
        .map_err(|e| e.to_string())
}

/// `POST /internal/render_pdf`: print a workspace document to PDF on this machine — no online converters.
pub async fn render_pdf(State(ctx): State<App>, body: Bytes) -> JsonResult {
    let b = body_obj(&body)?;
    let req = obj(["source", "markdown", "output", "title"]
        .iter()
        .map(|k| (*k, json!(s_opt(&b, k))))
        .collect());
    let source = get_str(&req, "source");
    let tid = s_or(&b, "task_id", "");
    match render(&ctx, "/pdf", &req, 150.0).await {
        Err(e) => {
            ctx.audit(
                "sentinel",
                "pdf.render",
                audit_task(
                    tid,
                    if source.is_empty() {
                        "text".into()
                    } else {
                        source
                    },
                    "error",
                    json!({"error": take_chars(&e, 200)}),
                ),
            );
            Ok(Json(json!({"error": e})))
        }
        Ok(r) => {
            let src = if source.is_empty() {
                "(text)".to_string()
            } else {
                source
            };
            ctx.audit(
                "sentinel",
                "pdf.render",
                audit_task(
                    tid,
                    get_str(&r, "path"),
                    "success",
                    json!({"source": src, "size": g(&r, "size")}),
                ),
            );
            Ok(Json(r))
        }
    }
}

/// `[str(s) for s in (b.get("symbols") or []) if str(s).strip()][:n]`.
fn symbols(b: &Value, n: usize) -> Vec<String> {
    let items: Vec<String> = match b.get("symbols") {
        Some(Value::Array(a)) => a.iter().map(py_str).collect(),
        Some(Value::String(s)) => s.chars().map(|c| c.to_string()).collect(),
        _ => vec![],
    };
    items
        .into_iter()
        .filter(|s| !s.trim().is_empty())
        .take(n)
        .collect()
}

fn first_errors(r: &Value) -> Value {
    json!(r["errors"]
        .as_array()
        .map(|a| a.iter().take(5).cloned().collect::<Vec<_>>())
        .unwrap_or_default())
}

/// `POST /internal/market_data`: quotes and price history (Yahoo Finance public chart API, read-only).
pub async fn market_data(State(ctx): State<App>, body: Bytes) -> JsonResult {
    let b = body_obj(&body)?;
    let syms = symbols(&b, 8);
    if syms.is_empty() {
        return Ok(Json(
            json!({"error": "需要 symbols (give one or more tickers)"}),
        ));
    }
    let rng = {
        let r = s_opt(&b, "range");
        if r.is_empty() {
            "1y".to_string()
        } else {
            r
        }
    };
    let r = ctx.market.fetch(&syms, &rng).await;
    let ok = r["results"].as_array().is_some_and(|a| !a.is_empty());
    ctx.audit(
        "sentinel",
        "market.data",
        audit_task(
            s_or(&b, "task_id", ""),
            take_chars(&syms.join(","), 200).into(),
            if ok { "success" } else { "error" },
            json!({"range": g(&b, "range"), "errors": first_errors(&r)}),
        ),
    );
    Ok(Json(r))
}

/// `POST /internal/fundamentals`: valuations and reported financials — Yahoo Finance, read-only.
pub async fn fundamentals(State(ctx): State<App>, body: Bytes) -> JsonResult {
    let b = body_obj(&body)?;
    let syms = symbols(&b, 6);
    if syms.is_empty() {
        return Ok(Json(
            json!({"error": "需要 symbols (give one or more tickers)"}),
        ));
    }
    let r = ctx.market.fundamentals(&syms).await;
    let ok = r["results"].as_array().is_some_and(|a| !a.is_empty());
    ctx.audit(
        "sentinel",
        "market.fundamentals",
        audit_task(
            s_or(&b, "task_id", ""),
            take_chars(&syms.join(","), 200).into(),
            if ok { "success" } else { "error" },
            json!({"errors": first_errors(&r)}),
        ),
    );
    Ok(Json(r))
}

/// `POST /internal/browser_release`: a task ended; the browser may recycle its page later.
pub async fn browser_release(State(ctx): State<App>, body: Bytes) -> JsonResult {
    let b = body_obj(&body)?;
    let _ = deps::broker(
        &ctx,
        "POST",
        "/agent/release",
        Some(&json!({"task_id": s_opt(&b, "task_id")})),
        10.0,
    )
    .await;
    Ok(Json(json!({"ok": true})))
}

/// `POST /internal/render_png`: an SVG chart → PNG in the workspace, rendered on this machine.
pub async fn render_png(State(ctx): State<App>, body: Bytes) -> JsonResult {
    let b = body_obj(&body)?;
    let req = json!({"svg": s_opt(&b, "svg"), "output": s_opt(&b, "output")});
    let tid = s_or(&b, "task_id", "");
    match render(&ctx, "/png", &req, 90.0).await {
        Err(e) => {
            ctx.audit(
                "sentinel",
                "chart.render",
                audit_task(
                    tid,
                    get_str(&req, "output"),
                    "error",
                    json!({"error": take_chars(&e, 200)}),
                ),
            );
            Ok(Json(json!({"error": e})))
        }
        Ok(r) => {
            ctx.audit(
                "sentinel",
                "chart.render",
                audit_task(
                    tid,
                    get_str(&r, "path"),
                    "success",
                    json!({"size": g(&r, "size")}),
                ),
            );
            Ok(Json(r))
        }
    }
}

/// `POST /internal/compose_image`: exact text on a workspace image, rendered by the browser container offline.
pub async fn compose_image(State(ctx): State<App>, body: Bytes) -> JsonResult {
    let b = body_obj(&body)?;
    let keys = [
        "image", "text", "position", "size", "color", "band", "weight", "align", "font", "output",
        "padding",
    ];
    let req = obj(keys.iter().map(|k| (*k, g(&b, k))).collect());
    let tid = s_or(&b, "task_id", "");
    match render(&ctx, "/compose", &req, 90.0).await {
        Err(e) => {
            ctx.audit(
                "sentinel",
                "image.compose",
                audit_task(
                    tid,
                    s_opt(&b, "image"),
                    "error",
                    json!({"error": take_chars(&e, 200)}),
                ),
            );
            Ok(Json(json!({"error": e})))
        }
        Ok(r) => {
            ctx.audit(
                "sentinel",
                "image.compose",
                audit_task(
                    tid,
                    get_str(&r, "path"),
                    "success",
                    json!({"size": g(&r, "size")}),
                ),
            );
            Ok(Json(r))
        }
    }
}

/// `POST /internal/send_file`: deliver a file the agent sent to the Telegram conversation (owner chat only).
pub async fn send_file(State(ctx): State<App>, body: Bytes) -> JsonResult {
    let b = body_obj(&body)?;
    let conv = s_or(&b, "conv_id", "");
    let path = s_or(&b, "path", "");
    let name = {
        let n = s_or(&b, "name", "");
        if n.is_empty() {
            "file".to_string()
        } else {
            n
        }
    };
    let bot = ctx.bot();
    let Some(bot) =
        bot.filter(|bt| bt.running() && !conv.is_empty() && conv == ctx.kv_str("tg_conv"))
    else {
        return Ok(Json(json!({"sent": false})));
    };
    let r = ctx
        .http
        .get(format!("{}/api/files/raw", ctx.cfg.runtime_url()))
        .query(&[("path", path.as_str()), ("download", "1")])
        .timeout(Duration::from_secs(120))
        .send()
        .await;
    let r = match r {
        Ok(r) => r,
        Err(e) => {
            return Ok(Json(
                json!({"sent": false, "error": take_chars(&e.to_string(), 200)}),
            ))
        }
    };
    let code = r.status().as_u16();
    let content = match r.bytes().await {
        Ok(c) => c,
        Err(e) => {
            return Ok(Json(
                json!({"sent": false, "error": take_chars(&e.to_string(), 200)}),
            ))
        }
    };
    if code != 200 {
        return Ok(Json(
            json!({"sent": false, "error": format!("读取文件失败 (HTTP {code})")}),
        ));
    }
    if content.len() > TG_FILE_MAX {
        return Ok(Json(
            json!({"sent": false, "error": "文件超过 Telegram 的 50 MB 上限 (over Telegram's 50 MB limit)"}),
        ));
    }
    let size = content.len();
    if let Err(e) = bot
        .send_document(
            &name,
            &content,
            &s_cut(&b, "caption", "", 900),
            &s_or(&b, "mime", ""),
        )
        .await
    {
        return Ok(Json(
            json!({"sent": false, "error": take_chars(&format!("{e:#}"), 200)}),
        ));
    }
    ctx.audit(
        "sentinel",
        "telegram.send_file",
        audit_task(
            s_or(&b, "task_id", ""),
            name,
            "success",
            json!({"size": size, "path": path}),
        ),
    );
    Ok(Json(json!({"sent": true})))
}

/// `GET /internal/public_base`: the address users reach this OMuse at (learned from the first page load).
pub async fn public_base(State(ctx): State<App>) -> JsonResult {
    Ok(Json(json!({"base": ctx.kv_str("public_base")})))
}

/// `POST /internal/expire_approvals`: a task ended without the user deciding; its pending approvals expire.
pub async fn expire_approvals(State(ctx): State<App>, body: Bytes) -> JsonResult {
    let b = body_obj(&body)?;
    let tid = s_or(&b, "task_id", "");
    let reason = s_cut(&b, "reason", "", 300);
    if tid.is_empty() {
        return Ok(Json(json!({"expired": 0})));
    }
    let mut n = 0;
    for ap in ctx.store.approvals(Some("pending"), 500).map_err(ierr)? {
        let aid = get_str(&ap, "id");
        if get_str(&ap, "task_id") != tid || ctx.resolving.lock().contains(&aid) {
            continue;
        }
        let scope = {
            let s = s_opt(&ap, "scope");
            if s.is_empty() {
                "ONCE".to_string()
            } else {
                s
            }
        };
        ctx.store
            .resolve_approval(
                &aid,
                "expired",
                &scope,
                Some(&json!({"status": "expired", "reason": reason})),
                None,
                "system",
            )
            .map_err(ierr)?;
        ctx.audit(
            "sentinel",
            "approval.expire",
            Audit {
                task_id: tid.clone(),
                resource: get_str(&ap, "tool"),
                risk: get_str(&ap, "risk"),
                decision: DENY.into(),
                result: "expired".into(),
                detail: Some(json!({"approval_id": aid, "reason": reason})),
            },
        );
        n += 1;
    }
    // a cancelled task can't use the browser any more: end a takeover of its page
    if let Ok(BrokerOut::Json(bst)) = deps::broker(&ctx, "GET", "/state", None, 5.0).await {
        if get_str(&bst, "mode") == "user"
            && get_str(&bst, "takeover_task") == tid
            && deps::broker(&ctx, "POST", "/user/release", Some(&json!({})), 10.0)
                .await
                .is_ok()
        {
            ctx.audit(
                "sentinel",
                "browser.release",
                audit_task(
                    tid.clone(),
                    String::new(),
                    "success",
                    json!({"reason": "task ended"}),
                ),
            );
        }
    }
    Ok(Json(json!({"expired": n})))
}

/// Query parameter as an int (FastAPI 422 when it is not one).
pub fn q_int(q: &HashMap<String, String>, k: &str, default: i64) -> Result<i64, ApiError> {
    match q.get(k) {
        None => Ok(default),
        Some(v) => v.trim().parse::<i64>().map_err(|_| {
            ApiError::with_json(
                422,
                json!([{"type": "int_parsing", "loc": ["query", k], "msg": "Input should be a valid integer, unable to parse string as an integer", "input": v}]),
            )
        }),
    }
}

/// `GET /internal/ledger?limit=&task_id=`.
pub async fn ledger(
    State(ctx): State<App>,
    Query(q): Query<HashMap<String, String>>,
) -> JsonResult {
    let limit = q_int(&q, "limit", 30)?;
    let task_id = q.get("task_id").cloned().unwrap_or_default();
    let orders = if task_id.is_empty() {
        ledger::entries(&ctx.store, limit.min(200), None)
    } else {
        ledger::for_task(&ctx.store, &task_id)
    }
    .map_err(ierr)?;
    Ok(Json(json!({"orders": orders})))
}

/// `POST /internal/ledger/confirm`: how a purchase task ended (order number from the confirmation page, or none).
pub async fn ledger_confirm(State(ctx): State<App>, body: Bytes) -> JsonResult {
    let b = body_obj(&body)?;
    let total = match b.get("total") {
        Some(Value::Number(n)) => n.as_f64(),
        Some(Value::Bool(x)) => Some(if *x { 1.0 } else { 0.0 }),
        _ => None,
    };
    let evidence = b.get("evidence").filter(|e| truthy(e));
    let tid = s_opt(&b, "task_id");
    let out = ledger::confirm(
        &ctx.store,
        &tid,
        &s_opt(&b, "order_number"),
        total,
        evidence,
    )
    .map_err(ierr)?;
    if !out.is_empty() {
        let orders: Vec<Value> =
            out.iter().map(|o| json!({"id": o["id"], "order_number": o["order_number"], "status": o["status"]})).collect();
        ctx.audit(
            "sentinel",
            "ledger.confirm",
            audit_task(tid, String::new(), "success", json!({"orders": orders})),
        );
    }
    Ok(Json(json!({"orders": out})))
}

/// `_ledger_search(query)`: search every connected mailbox (errors skipped).
pub async fn ledger_search(ctx: &App, query: &str) -> Vec<Value> {
    let mut out = Vec::new();
    for acc in mailboxes::ready_accounts(&ctx.store).unwrap_or_default() {
        if let Ok(v) = deps::gmail_search(ctx, &get_str(&acc, "id"), query, 10).await {
            out.extend(v);
        }
    }
    out
}

/// `ledger.reconcile(store, _ledger_search)` off the event loop.
pub async fn run_reconcile(ctx: &App) -> Result<Value, ApiError> {
    let c = ctx.clone();
    let handle = tokio::runtime::Handle::current();
    tokio::task::spawn_blocking(move || {
        let mut search =
            |q: &str| -> anyhow::Result<Vec<Value>> { Ok(handle.block_on(ledger_search(&c, q))) };
        ledger::reconcile(&c.store, &mut search)
    })
    .await
    .map_err(ierr)?
    .map_err(ierr)
}

/// `POST /internal/ledger/backfill`.
pub async fn ledger_backfill(State(ctx): State<App>) -> JsonResult {
    let made = ledger::backfill(&ctx.store).map_err(ierr)?;
    let mut tasks = Vec::new();
    for i in &made {
        tasks.push(
            ledger::get(&ctx.store, i)
                .map_err(ierr)?
                .map(|r| r["task_id"].clone())
                .unwrap_or(Value::Null),
        );
    }
    Ok(Json(json!({"created": made, "tasks": tasks})))
}

/// `POST /internal/ledger/reconcile`.
pub async fn ledger_reconcile(State(ctx): State<App>) -> JsonResult {
    let res = run_reconcile(&ctx).await?;
    if res.get("changed").is_some_and(truthy) {
        ctx.audit(
            "sentinel",
            "ledger.reconcile",
            Audit {
                result: "success".into(),
                detail: Some(res.clone()),
                ..Default::default()
            },
        );
    }
    Ok(Json(res))
}

/// `GET /internal/mail_check`: sign in to each connected mailbox and list the inbox (runtime health watch).
pub async fn mail_check(State(ctx): State<App>) -> JsonResult {
    let mut out = Vec::new();
    for acc in mailboxes::ready_accounts(&ctx.store).map_err(ierr)? {
        let email = get_str(&acc, "email");
        match deps::gmail_test(&ctx, Some(&get_str(&acc, "id")), Some(40.0)).await {
            Ok(_) => out.push(json!({"email": email, "ok": true})),
            Err(e) => out.push(json!({"email": email, "ok": false, "error": format!("{}: {}", e.kind, take_chars(&e.msg, 160))})),
        }
    }
    Ok(Json(json!({"accounts": out})))
}

/// `GET /internal/browser_state`.
pub async fn browser_state(State(ctx): State<App>) -> JsonResult {
    match deps::broker(&ctx, "GET", "/state", None, 10.0).await {
        Ok(r) => Ok(Json(r.json())),
        Err(e) => Ok(Json(json!({"mode": "unknown", "error": e.to_string()}))),
    }
}

/// `POST /internal/user_request`: tasks the runtime starts itself record the request text.
pub async fn user_request(State(ctx): State<App>, body: Bytes) -> JsonResult {
    let b = body_obj(&body)?;
    if let (true, Some(Value::String(text))) = (b.get("task_id").is_some_and(truthy), b.get("text"))
    {
        ctx.store
            .set_user_request(&s_or(&b, "task_id", ""), take_chars(text, 4000))
            .map_err(ierr)?;
    }
    Ok(Json(json!({"ok": true})))
}

/// `GET /internal/watch/sources`.
pub async fn watch_sources() -> JsonResult {
    Ok(Json(json!({"sources": deps::watch_sources()})))
}

/// `POST /internal/watch`: poll one trigger source for new events.
pub async fn watch(State(ctx): State<App>, body: Bytes) -> JsonResult {
    let b = body_obj(&body)?;
    let src = s_or(&b, "source", "");
    let cursor = g(&b, "cursor");
    let sources = deps::watch_sources();
    let Some(info) = sources.get(&src).filter(|i| truthy(i)) else {
        return Ok(Json(
            json!({"error": format!("unknown source {src}"), "events": [], "cursor": cursor}),
        ));
    };
    let connector = get_str(info, "connector");
    let conn = ctx.store.connection(&connector).map_err(ierr)?;
    let perm = if connector == "browser" {
        "browse"
    } else {
        "read"
    };
    if !truthy(&conn["enabled"]) || !conn["permissions"].get(perm).is_some_and(truthy) {
        return Ok(Json(json!({
            "error": format!("{connector} 未启用或读取权限已关闭 (connector disabled / read permission off)"),
            "events": [], "cursor": cursor,
        })));
    }
    let params = match b.get("params") {
        Some(p) if p.is_object() => p.clone(),
        _ => json!({}),
    };
    let res = match deps::watch_poll(&ctx, &src, &params, &cursor).await {
        Ok(r) => r,
        Err(e) => return Ok(Json(json!({"error": e, "events": [], "cursor": cursor}))),
    };
    if res["events"].as_array().is_some_and(|a| !a.is_empty()) {
        ctx.audit(
            "sentinel",
            "trigger.events",
            Audit {
                resource: src,
                result: "success".into(),
                detail: Some(json!({"count": res["events"].as_array().map(|a| a.len()).unwrap_or(0), "injection": g(&res, "injection")})),
                ..Default::default()
            },
        );
    }
    Ok(Json(res))
}

/// `POST /internal/taint`: a task starts with external (trigger) data in its context.
pub async fn taint(State(ctx): State<App>, body: Bytes) -> JsonResult {
    let b = body_obj(&body)?;
    let lvl = s_or(&b, "taint", "CONFIDENTIAL");
    let lvl = if omuse_common::guard::is_level(&lvl) {
        lvl
    } else {
        "CONFIDENTIAL".into()
    };
    let inj: Vec<String> = match b.get("injection") {
        Some(Value::Array(a)) => a
            .iter()
            .take(10)
            .map(|x| take_chars(&py_str(x), 40).to_string())
            .collect(),
        Some(Value::String(s)) => s.chars().take(10).map(|c| c.to_string()).collect(),
        _ => vec![],
    };
    let ctx_v = ctx
        .store
        .update_task_ctx(
            &s_or(&b, "task_id", ""),
            Some(&lvl),
            if inj.is_empty() { None } else { Some(&inj) },
            None,
        )
        .map_err(ierr)?;
    Ok(Json(json!({"ok": true, "ctx": ctx_v})))
}
