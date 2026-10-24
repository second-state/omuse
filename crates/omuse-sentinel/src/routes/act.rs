//! `/internal/act` (the policy gate for every tool call), the approval summaries, running an action, and
//! resolving approvals (web UI and Telegram).

use std::sync::LazyLock;
use std::time::Instant;

use crate::actions::ExecError;
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::Json;
use chrono::TimeZone;
use omuse_common::api::ApiError;
use omuse_common::guard;
use omuse_common::pyjson;
use omuse_common::util::{get_str, py_str, take_chars, truncate, truthy};
use omuse_mail::mailboxes;
use omuse_phone::phone::MAX_MINUTES_CAP;
use omuse_sentinel_core::errors::ActionError;
use omuse_sentinel_core::policy::{Decision, PER_USE_TOOLS};
use omuse_sentinel_core::store::Audit;
use omuse_sentinel_core::{decide, ledger, suggest, vault, ALLOW, ASK, DENY};
use regex::Regex;
use serde_json::{json, Map, Value};

use super::deps::{self, BotExt, BrokerOut};
use super::{
    body_obj, g, g_or, internal as ierr, merged, notify_phone, py_float, py_int, s_opt, s_or,
    safe_tg, spawn_notify_runtime, tool, App, Ctx, JsonResult,
};

/// Fields the user may edit on the approval card, per tool (Python `EDITABLE`).
pub fn editable(tool: &str) -> &'static [&'static str] {
    match tool {
        "phone_call" => &["purpose"],
        "gmail_send" | "gmail_reply" | "gmail_create_draft" => &["to", "cc", "subject", "body"],
        "gmail_forward" => &["to", "note"],
        "browser_type" | "browser_click_at" => &["text"],
        "gmail_unsubscribe" => &["message_ids"],
        "slack_send_message" => &["text"],
        "notion_create_page" => &["title", "content"],
        "notion_append" => &["content"],
        "calendar_create_event" | "calendar_update_event" => {
            &["title", "start", "end", "location", "description"]
        }
        _ => &[],
    }
}

/// `DRY_RUN_CAPS` / `DRY_RUN_TOOLS`.
const DRY_RUN_CAPS: [&str; 7] = [
    "send", "write", "draft", "organize", "upload", "notify", "call",
];
const DRY_RUN_TOOLS: [&str; 3] = [
    "purchase_confirm",
    "browser_fill_secret",
    "browser_request_takeover",
];

/// A call a golden (test) run must not make even when policy would allow it.
pub fn dry_run_side_effect(tool: &str, t: &Value) -> bool {
    if DRY_RUN_TOOLS.contains(&tool) {
        return true;
    }
    if tool == "phone_call_status" || tool == "phone_hangup" {
        return false;
    }
    let cap = get_str(t, "capability");
    if get_str(t, "connector").starts_with("mcp:") {
        return cap != "read";
    }
    DRY_RUN_CAPS.contains(&cap.as_str())
}

/// `_safe_args`: long strings cut for the audit log.
pub fn safe_args(args: &Value) -> Value {
    let mut out = Map::new();
    if let Some(m) = args.as_object() {
        for (k, v) in m {
            out.insert(
                k.clone(),
                if let Value::String(s) = v {
                    json!(truncate(s, 2000))
                } else {
                    v.clone()
                },
            );
        }
    }
    Value::Object(out)
}

/// `_from_account`: which mailbox an outgoing email will be sent from.
fn from_account(ctx: &Ctx, tool: &str, args: &Value) -> String {
    let rid = match args.get("reply_to_message_id").filter(|v| truthy(v)) {
        Some(v) => Some(v.clone()),
        None if tool == "gmail_reply" || tool == "gmail_forward" => {
            args.get("message_id").filter(|v| truthy(v)).cloned()
        }
        None => None,
    };
    let acc = match rid {
        Some(rid) => {
            let (aid, _) = mailboxes::split_id(&py_str(&rid));
            mailboxes::find(&ctx.store, Some(&aid))
        }
        None => {
            let fa = args
                .get("from_account")
                .filter(|v| !v.is_null())
                .map(py_str);
            mailboxes::find(&ctx.store, fa.as_deref())
        }
    };
    match acc {
        Ok(Some(a)) => py_str(&a["email"]),
        Ok(None) => s_opt(args, "from_account"),
        Err(_) => String::new(),
    }
}

type Ctxs = (Option<Value>, Option<Value>);

/// `_context_for`: ask the broker what a ref points to so the policy can judge the click/type.
pub async fn context_for(
    ctx: &Ctx,
    tool: &str,
    args: &Value,
    task_id: &str,
) -> Result<Ctxs, ActionError> {
    let page_of = |info: &Value| json!({"url": g_or(info, "page_url", json!("")), "title": g_or(info, "page_title", json!(""))});
    if tool == "browser_save_media" && !args.get("ref").is_some_and(truthy) {
        let st = deps::broker(ctx, "GET", "/state", None, 10.0).await?.json();
        let url = st["tasks"]
            .as_array()
            .and_then(|a| a.iter().find(|x| get_str(x, "task_id") == task_id))
            .map(|x| g_or(x, "url", json!("")))
            .unwrap_or_else(|| g_or(&st, "url", json!("")));
        return Ok((None, Some(json!({"url": url, "title": ""}))));
    }
    if matches!(
        tool,
        "browser_click"
            | "browser_type"
            | "browser_select"
            | "browser_upload"
            | "browser_fill_secret"
            | "browser_save_media"
    ) {
        let body = json!({"task_id": task_id, "ref": g_or(args, "ref", json!(""))});
        let info = deps::broker(ctx, "POST", "/agent/describe", Some(&body), 20.0)
            .await?
            .json();
        let page = page_of(&info);
        return Ok((Some(info), Some(page)));
    }
    if tool == "browser_click_at" {
        let body = json!({"task_id": task_id, "x": g_or(args, "x", json!(0)), "y": g_or(args, "y", json!(0))});
        let info = deps::broker(ctx, "POST", "/agent/describe_at", Some(&body), 20.0)
            .await?
            .json();
        let page = page_of(&info);
        return Ok((Some(info), Some(page)));
    }
    if tool == "browser_press" {
        let info = deps::broker(
            ctx,
            "POST",
            "/agent/focused",
            Some(&json!({"task_id": task_id})),
            20.0,
        )
        .await?
        .json();
        let st = deps::broker(ctx, "GET", "/state", None, 10.0).await?.json();
        return Ok((
            Some(info),
            Some(
                json!({"url": g_or(&st, "url", json!("")), "title": g_or(&st, "title", json!(""))}),
            ),
        ));
    }
    Ok((None, None))
}

fn field(label: &str, v: impl Into<Value>) -> Value {
    json!([label, v.into()])
}

fn fmt2(v: f64) -> String {
    format!("{v:.2}")
}

/// `_summary`: the approval card (`args` may gain reply defaults, as the Python `setdefault` did).
pub async fn summary(
    ctx: &Ctx,
    tool: &str,
    args: &mut Value,
    elem: Option<&Value>,
    page: Option<&Value>,
) -> Value {
    let t = tool_def(tool);
    let connector = get_str(&t, "connector");
    let mut s = Map::new();
    s.insert("tool".into(), json!(tool));
    s.insert("connector".into(), t["connector"].clone());
    s.insert("fields".into(), json!([]));
    s.insert("editable".into(), json!(editable(tool)));
    let a = |args: &Value, k: &str| g_or(args, k, json!(""));
    let mut fields: Vec<Value> = Vec::new();
    if tool == "gmail_send" || tool == "gmail_create_draft" {
        s.insert(
            "title".into(),
            json!(if tool == "gmail_send" {
                "发送邮件 Send email"
            } else {
                "创建草稿 Create draft"
            }),
        );
        fields = vec![
            field("发件账号 From", from_account(ctx, tool, args)),
            field("收件人 To", a(args, "to")),
            field("抄送 Cc", a(args, "cc")),
            field("主题 Subject", a(args, "subject")),
        ];
        s.insert("body".into(), a(args, "body"));
    } else if tool == "gmail_reply" {
        s.insert("title".into(), json!("回复并发送邮件 Reply & send"));
        match deps::reply_defaults(
            ctx,
            &s_or(args, "message_id", ""),
            args.get("reply_all").is_some_and(truthy),
        )
        .await
        {
            Ok(d) => {
                if let Some(m) = args.as_object_mut() {
                    for k in ["to", "cc", "subject"] {
                        if !m.contains_key(k) {
                            m.insert(k.into(), g(&d, k));
                        }
                    }
                }
            }
            Err(e) => {
                s.insert("warning".into(), json!(format!("无法读取原邮件: {e:#}")));
            }
        }
        fields = vec![
            field("发件账号 From", from_account(ctx, tool, args)),
            field("收件人 To", a(args, "to")),
            field("抄送 Cc", a(args, "cc")),
            field("主题 Subject", a(args, "subject")),
        ];
        s.insert("body".into(), a(args, "body"));
    } else if tool == "phone_call" {
        let cfg = ctx.phone.config();
        let (n, line, why) = ctx.phone.check_call(&s_or(args, "to", ""), Some(&cfg));
        let want = args
            .get("max_minutes")
            .filter(|v| truthy(v))
            .and_then(py_int)
            .unwrap_or(cfg.max_minutes);
        let cap = if line == "dialmcp" {
            10
        } else {
            MAX_MINUTES_CAP
        };
        let mins = want.min(cfg.max_minutes).min(cap).max(1);
        s.insert("title".into(), json!("打电话 Phone call"));
        let line_label = match line.as_str() {
            "dialmcp" => "DialMCP（美国/加拿大 US/CA，会录音 recorded）",
            "telnyx" => "Telnyx + OpenAI Realtime",
            _ => "—",
        };
        fields = vec![
            field(
                "拨打 To",
                if n.is_empty() {
                    s_or(args, "to", "")
                } else {
                    n
                },
            ),
            field(
                "来电显示 From",
                if line.is_empty() {
                    String::new()
                } else {
                    ctx.phone.caller_id(&line, Some(&cfg))
                },
            ),
            field("线路 Line", line_label),
            field(
                "可以告诉对方 May share",
                super::g_truthy_or(args, "may_share", json!("（无 nothing）")),
            ),
            field(
                "语言 Language",
                super::g_truthy_or(args, "language", json!("跟随对方 match them")),
            ),
            field("最长 Max", format!("{mins} 分钟 min")),
            field(
                "今天已拨 Calls today",
                format!("{} / {}", ctx.phone.calls_today(), cfg.daily_limit),
            ),
        ];
        s.insert("body".into(), a(args, "purpose"));
        if !why.is_empty() {
            s.insert("warning".into(), json!(why));
        }
    } else if tool == "gmail_forward" {
        s.insert("title".into(), json!("转发邮件 Forward email"));
        fields = vec![
            field("发件账号 From", from_account(ctx, tool, args)),
            field("转发给 To", a(args, "to")),
            field("原邮件 ID", a(args, "message_id")),
        ];
        s.insert("body".into(), a(args, "note"));
    } else if tool == "gmail_unsubscribe" {
        s.insert("title".into(), json!("退订邮件 Unsubscribe"));
        let ids: Vec<String> = match args.get("message_ids") {
            Some(Value::Array(v)) => v.iter().take(30).map(py_str).collect(),
            Some(Value::String(x)) => x.chars().take(30).map(|c| c.to_string()).collect(),
            _ => vec![],
        };
        match deps::unsubscribe_targets(ctx, &ids).await {
            Ok(targets) => {
                let items: Vec<Value> = targets
                    .iter()
                    .map(|t| {
                        json!({"id": t["id"], "from": g_or(t, "from", json!("")), "subject": g_or(t, "subject", json!("")),
                               "account": g_or(t, "account", json!("")), "date": g_or(t, "date", json!("")),
                               "method": g_or(t, "method", json!("")), "error": g_or(t, "error", json!(""))})
                    })
                    .collect();
                s.insert("items".into(), json!(items));
            }
            Err(e) => {
                s.insert("warning".into(), json!(format!("无法读取邮件信息: {e:#}")));
                let items: Vec<Value> = ids
                    .iter()
                    .map(|i| json!({"id": i, "from": "", "subject": "", "method": ""}))
                    .collect();
                s.insert("items".into(), json!(items));
            }
        }
        fields = vec![
            field("邮件数 Emails", ids.len().to_string()),
            field(
                "同时归档 Also archive",
                if args.get("archive").is_some_and(truthy) {
                    "是 Yes"
                } else {
                    "否 No"
                },
            ),
        ];
    } else if tool == "purchase_confirm" {
        s.insert("title".into(), json!("确认购买 Confirm purchase"));
        let cur = s_opt(args, "currency");
        let items = super::g_truthy_or(args, "items", json!([]));
        let mut lines = Vec::new();
        let list: Vec<Value> = match &items {
            Value::Array(v) => v.iter().take(10).cloned().collect(),
            Value::String(x) => x.chars().take(10).map(|c| json!(c.to_string())).collect(),
            Value::Object(m) => m.keys().take(10).map(|k| json!(k)).collect(),
            _ => vec![],
        };
        for it in list {
            let line = if it.is_object() {
                py_float(&g_or(&it, "price", json!(0))).map(|p| {
                    format!(
                        "{} × {} — {} {}",
                        py_str(&g_or(&it, "name", json!(""))),
                        py_str(&g_or(&it, "qty", json!(1))),
                        cur,
                        fmt2(p)
                    )
                })
            } else {
                None
            };
            lines.push(line.unwrap_or_else(|| {
                let txt = if it.is_object() {
                    pyjson::py_repr(&it)
                } else {
                    py_str(&it)
                };
                take_chars(&txt, 120).to_string()
            }));
        }
        let mut card = "网站上已保存的卡 card saved on the site".to_string();
        let cid = s_opt(args, "card_item_id");
        if !cid.is_empty() {
            card = match vault::item(&ctx.store, &cid).ok().flatten() {
                Some(it) => vault::summary_text(&it, "number"),
                None => cid,
            };
        }
        let ship_v = super::g_truthy_or(args, "shipping", json!(0));
        let (ship, total) = match (py_float(&ship_v), py_float(&g(args, "total"))) {
            (Some(sv), Some(tv)) => (format!("{cur} {}", fmt2(sv)), format!("{cur} {}", fmt2(tv))),
            _ => (py_str(&a(args, "shipping")), py_str(&a(args, "total"))),
        };
        fields = vec![
            field("网站 Site", s_or(args, "site", "")),
            field("商品 Items", lines.join("\n")),
            field("运费 Shipping", ship),
            field("总价 Total", total),
            field("送货方式 Delivery", s_or(args, "delivery", "")),
            field("付款卡 Card", card),
            field(
                "批准后 After approval",
                "30 分钟内、仅这个网站：结账、填卡、下单不再逐个审批；页面总价超过上面的金额会再问你 \
                 (30 min, this site only; asks again if the page total is higher)",
            ),
        ];
        s.insert("body".into(), json!(s_or(args, "note", "")));
        s.insert("screenshot".into(), json!(true));
    } else if tool.starts_with("browser_") {
        let verb = match tool {
            "browser_click" => "点击 Click",
            "browser_click_at" => "按位置点击 Click at position",
            "browser_type" => "输入 Type",
            "browser_select" => "选择 Select",
            "browser_press" => "按键 Press key",
            "browser_upload" => "上传文件 Upload",
            "browser_navigate" => "打开网页 Open page",
            "browser_fill_secret" => "从保险箱填写 Fill from vault",
            other => other,
        };
        s.insert("title".into(), json!(format!("浏览器操作：{verb}")));
        let empty = json!({});
        let pg = page.unwrap_or(&empty);
        let site = match pg.get("url").filter(|v| truthy(v)) {
            Some(u) => u.clone(),
            None => a(args, "url"),
        };
        fields = vec![
            field("网站 Site", site),
            field("页面 Page", g_or(pg, "title", json!(""))),
        ];
        if let Some(e) = elem.filter(|e| truthy(e)) {
            fields.push(field(
                "元素 Element",
                format!(
                    "{} 「{}」",
                    py_str(&g_or(e, "tag", json!(""))),
                    py_str(&g_or(e, "name", json!("")))
                ),
            ));
        }
        if tool == "browser_click_at" {
            fields.push(field(
                "位置 Position",
                format!("x={}, y={}", py_str(&g(args, "x")), py_str(&g(args, "y"))),
            ));
            if let Some(frames) = elem.and_then(|e| e.get("frames")).filter(|f| truthy(f)) {
                let parts: Vec<String> = frames
                    .as_array()
                    .map(|a| a.iter().map(py_str).collect())
                    .unwrap_or_default();
                fields.push(field("所在框架 Inside iframe", parts.join(" › ")));
            }
            if args.get("text").is_some_and(truthy) {
                s.insert("body".into(), a(args, "text"));
                if args.get("submit").is_some_and(truthy) {
                    fields.push(field("提交 Submit", "输入后按回车 Enter"));
                }
            }
        }
        if tool == "browser_fill_secret" {
            s.insert("title".into(), json!("从保险箱填写 Fill from vault"));
            if let Some(it) = vault::item(&ctx.store, &s_or(args, "item_id", ""))
                .ok()
                .flatten()
            {
                fields.push(field(
                    "填写内容 Value",
                    vault::summary_text(&it, &s_or(args, "field", "")),
                ));
            }
            fields.push(field(
                "授权 Approval",
                "仅这一次（每次使用都要批准）Once only",
            ));
        }
        if tool == "browser_type" {
            s.insert("body".into(), a(args, "text"));
            if args.get("submit").is_some_and(truthy) {
                fields.push(field("提交 Submit", "输入后按回车 Enter"));
            }
        }
        if tool == "browser_select" {
            fields.push(field("选项 Option", a(args, "value")));
        }
        if tool == "browser_press" {
            fields.push(field("按键 Key", a(args, "key")));
        }
        if tool == "browser_upload" {
            fields.push(field("文件 File", a(args, "path")));
        }
        s.insert("screenshot".into(), json!(true));
    } else if tool == "slack_send_message" {
        s.insert("title".into(), json!("发送 Slack 消息 Send Slack message"));
        let conf = ctx
            .store
            .connection("slack")
            .map(|c| c["config"].clone())
            .unwrap_or(json!({}));
        let who = if get_str(&conf, "token_type") == "user" {
            "user"
        } else {
            "bot"
        };
        fields = vec![
            field("工作区 Workspace", g_or(&conf, "team", json!(""))),
            field("频道 Channel", a(args, "channel")),
            field(
                "身份 As",
                format!("{who} · {}", py_str(&g_or(&conf, "user", json!("")))),
            ),
        ];
        if args.get("thread_ts").is_some_and(truthy) {
            fields.push(field("回复讨论串 Thread", a(args, "thread_ts")));
        }
        s.insert("body".into(), a(args, "text"));
    } else if tool.starts_with("notion_") {
        let title = match tool {
            "notion_create_page" => "新建 Notion 页面 Create page",
            "notion_append" => "追加到 Notion 页面 Append",
            "notion_update_page" => "修改 Notion 页面 Update page",
            other => other,
        };
        s.insert("title".into(), json!(title));
        if args.get("archived").is_some_and(truthy) {
            s.insert(
                "title".into(),
                json!("归档（删除）Notion 页面 Archive page"),
            );
        }
        let conf = ctx
            .store
            .connection("notion")
            .map(|c| c["config"].clone())
            .unwrap_or(json!({}));
        let pg = match args.get("parent_id").filter(|v| truthy(v)) {
            Some(v) => v.clone(),
            None => a(args, "page_id"),
        };
        fields = vec![
            field("工作区 Workspace", g_or(&conf, "workspace", json!(""))),
            field("页面 Page", pg),
        ];
        if args.get("title").is_some_and(truthy) {
            fields.push(field("标题 Title", g(args, "title")));
        }
        if let Some(props) = args.get("properties").and_then(Value::as_object) {
            for (k, v) in props {
                let txt = if v.is_string() {
                    py_str(v)
                } else {
                    pyjson::py_repr(v)
                };
                fields.push(field(
                    &format!("属性 {k}"),
                    take_chars(&txt, 200).to_string(),
                ));
            }
        }
        s.insert("body".into(), a(args, "content"));
    } else if tool.starts_with("calendar_") {
        let conf = ctx
            .store
            .connection("calendar")
            .map(|c| c["config"].clone())
            .unwrap_or(json!({}));
        let title = match tool {
            "calendar_create_event" => "新建日程 Create event",
            "calendar_update_event" => "修改日程 Update event",
            "calendar_delete_event" => "删除日程 Delete event",
            other => other,
        };
        s.insert("title".into(), json!(title));
        let cal = {
            let c = s_opt(args, "calendar_id");
            if c.is_empty() {
                "primary".to_string()
            } else {
                c
            }
        };
        fields = vec![field(
            "日历 Calendar",
            format!("{} ({cal})", py_str(&g_or(&conf, "email", json!("")))),
        )];
        if tool != "calendar_create_event" {
            let cid = args.get("calendar_id").filter(|v| truthy(v)).map(py_str);
            match deps::calendar_event(ctx, &s_or(args, "event_id", ""), cid.as_deref()).await {
                Ok(ev) => {
                    fields.push(field(
                        "原日程 Event",
                        format!(
                            "{} · {} → {}",
                            py_str(&g(&ev, "title")),
                            py_str(&g(&ev, "start")),
                            py_str(&g(&ev, "end"))
                        ),
                    ));
                    if let Some(att) = ev
                        .get("attendees")
                        .and_then(Value::as_array)
                        .filter(|a| !a.is_empty())
                    {
                        let names: Vec<String> = att.iter().take(10).map(py_str).collect();
                        fields.push(field("参会人 Attendees", names.join(", ")));
                    }
                }
                Err(e) => {
                    s.insert("warning".into(), json!(format!("无法读取原日程: {e}")));
                }
            }
        }
        for (label, k) in [
            ("标题 Title", "title"),
            ("开始 Start", "start"),
            ("结束 End", "end"),
            ("地点 Location", "location"),
        ] {
            if args.get(k).is_some_and(truthy)
                || (tool == "calendar_create_event" && (k == "title" || k == "start"))
            {
                fields.push(field(label, s_or(args, k, "")));
            }
        }
        if args.get("all_day").is_some_and(truthy) {
            fields.push(field("全天 All day", "是 Yes"));
        }
        if args.get("attendees").is_some_and(truthy) {
            fields.push(field(
                "邀请 Invite (会发邮件 sends email)",
                omuse_apps::gcal::attendees(args.get("attendees")).join(", "),
            ));
        }
        let tz = match args.get("time_zone").filter(|v| truthy(v)) {
            Some(v) => v.clone(),
            None => g_or(&conf, "time_zone", json!("")),
        };
        fields.push(field("时区 Time zone", tz));
        if tool != "calendar_delete_event" {
            s.insert("body".into(), a(args, "description"));
        }
    } else if connector.starts_with("mcp:") {
        let info = omuse_mcp::mcp_hub::tool_info(&ctx.store, tool)
            .ok()
            .flatten();
        let (srv, rec) = match &info {
            Some((s, r)) => (Some(s), Some(r)),
            None => (None, None),
        };
        let sname = match srv {
            Some(s) => py_str(&s["name"]),
            None => connector.chars().skip(4).collect(),
        };
        let empty = json!({});
        let r = rec.unwrap_or(&empty);
        let tname = [r.get("title"), r.get("name")]
            .into_iter()
            .flatten()
            .find(|v| truthy(v))
            .map(py_str)
            .unwrap_or_else(|| tool.to_string());
        s.insert("title".into(), json!(format!("MCP · {sname}：{tname}")));
        let kind = match r.get("kind").and_then(Value::as_str) {
            Some("read") => "只读 read",
            Some("write") => "写入 write",
            Some("destructive") => "可能删除/覆盖 destructive",
            _ => "",
        };
        fields = vec![
            field("服务 Server", sname.clone()),
            field("工具 Tool", g_or(r, "name", json!(tool))),
            field("类型 Kind", kind),
        ];
        let am = args.as_object().cloned().unwrap_or_default();
        for (k, v) in am.iter().take(12) {
            let txt = match v {
                Value::String(x) => x.clone(),
                other => pyjson::dumps(other),
            };
            fields.push(field(&format!("参数 {k}"), truncate(&txt, 300)));
        }
        let big: Vec<String> = am
            .iter()
            .filter_map(|(k, v)| {
                v.as_str()
                    .filter(|x| x.chars().count() > 300)
                    .map(|x| format!("【{k}】\n{x}"))
            })
            .collect();
        if !big.is_empty() {
            s.insert("body".into(), json!(big.join("\n\n")));
        }
    } else {
        s.insert("title".into(), json!(tool));
        if let Some(m) = args.as_object() {
            fields = m.iter().map(|(k, v)| json!([k, py_str(v)])).collect();
        }
    }
    if matches!(tool, "gmail_send" | "gmail_reply" | "gmail_create_draft")
        && args.get("attachments").is_some_and(truthy)
    {
        let names: Vec<String> = match &args["attachments"] {
            Value::Array(a) => a
                .iter()
                .map(|x| {
                    let p = py_str(x);
                    p.rsplit('/').next().unwrap_or("").to_string()
                })
                .collect(),
            other => vec![py_str(other)],
        };
        fields.push(field("附件 Attachments", names.join(", ")));
    }
    s.insert("fields".into(), Value::Array(fields));
    Value::Object(s)
}

fn tool_def(name: &str) -> Value {
    tool(name).unwrap_or_else(|| json!({}))
}

fn date_local(ts: f64) -> String {
    chrono::Local
        .timestamp_opt(ts as i64, 0)
        .single()
        .map(|d| d.format("%Y-%m-%d").to_string())
        .unwrap_or_default()
}

/// `_ledger_summary`: cancel / return approvals name the ledger order they act on (or warn).
pub fn ledger_summary(summary: &mut Value, hits: &[Value], elem: Option<&Value>) {
    let name = elem.map(|e| s_opt(e, "name")).unwrap_or_default();
    summary["ledger_click"] = json!(take_chars(&name, 80));
    let keys = [
        "id",
        "merchant",
        "order_number",
        "total",
        "currency",
        "status_label",
        "created_at",
    ];
    let rows: Vec<Value> = hits
        .iter()
        .take(3)
        .map(|h| super::obj(keys.iter().map(|k| (*k, g(h, k))).collect()))
        .collect();
    summary["ledger"] = json!(rows);
    if !summary["fields"].is_array() {
        summary["fields"] = json!([]);
    }
    if !hits.is_empty() {
        for h in hits.iter().take(3) {
            let total = py_float(&g(h, "total")).unwrap_or(0.0);
            let created = py_float(&g(h, "created_at")).unwrap_or(0.0);
            let line = format!(
                "{} · {} · {} {} · {} · {}",
                py_str(&g(h, "merchant")),
                py_str(&g(h, "order_number")),
                py_str(&g(h, "currency")),
                fmt2(total),
                date_local(created),
                py_str(&g(h, "status_label"))
            );
            if let Some(f) = summary["fields"].as_array_mut() {
                f.push(json!(["账本订单 Ledger order", line]));
            }
        }
    } else {
        let w = format!(
            "{} 这个页面上的订单号不在 OMuse 的交易账本里：可能不是 OMuse 替你下的单，请核对订单号再批准 (the order on this page is not in OMuse's ledger — check it)",
            s_opt(summary, "warning")
        );
        summary["warning"] = json!(w.trim());
    }
}

static RETURN_RX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)return|refund|退货|退款|返品").unwrap());

/// `_ledger_after`: keep the ledger in step with what was just done.
fn ledger_after(
    ctx: &Ctx,
    tool: &str,
    args: &Value,
    task_id: &str,
    detail: &Value,
    result: &Value,
) {
    let r: anyhow::Result<()> = (|| {
        if tool == "purchase_confirm" {
            let mut card = String::new();
            let cid = s_opt(args, "card_item_id");
            if !cid.is_empty() {
                card = vault::item(&ctx.store, &cid)?
                    .map(|it| vault::summary_text(&it, "number"))
                    .unwrap_or_default();
            }
            let mut merchant = s_opt(result, "site");
            if merchant.is_empty() {
                let site = s_or(args, "site", "");
                let site = site.strip_prefix("https://").unwrap_or(&site);
                let site = site.strip_prefix("http://").unwrap_or(site);
                merchant = guard::domain_of(&format!("https://{site}"));
            }
            let total_v = super::g_truthy_or(args, "total", json!(0));
            let total = py_float(&total_v).ok_or_else(|| {
                anyhow::anyhow!(
                    "ValueError: could not convert string to float: {}",
                    pyjson::py_repr(&total_v)
                )
            })?;
            let items = super::g_truthy_or(args, "items", json!([]));
            ledger::add(
                &ctx.store,
                task_id,
                &merchant,
                total,
                &s_opt(args, "currency"),
                Some(&items),
                &card,
                &s_opt(args, "delivery"),
                &s_opt(result, "purchase_id"),
                &s_opt(detail, "approval_id"),
            )?;
        } else if (tool == "browser_click" || tool == "browser_click_at")
            && detail.get("approval_id").is_some_and(truthy)
        {
            let aid = py_str(&detail["approval_id"]);
            let ap = ctx.store.approval(&aid)?.unwrap_or(json!({}));
            let summ = ap.get("summary").cloned().unwrap_or(json!({}));
            let hits = summ
                .get("ledger")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            let name = s_opt(&summ, "ledger_click");
            let st = if RETURN_RX.is_match(&name) {
                "return_requested"
            } else {
                "cancel_requested"
            };
            let what = if name.is_empty() {
                "取消/退货".to_string()
            } else {
                name.clone()
            };
            for h in hits {
                ledger::set_status(
                    &ctx.store,
                    &get_str(&h, "id"),
                    st,
                    "user",
                    &format!("用户批准了「{what}」 approval {aid}"),
                    Some(&json!({"kind": "approval", "id": aid})),
                    false,
                )?;
            }
        }
        Ok(())
    })();
    if let Err(e) = r {
        ctx.audit(
            "sentinel",
            "ledger.update_failed",
            Audit {
                task_id: task_id.into(),
                result: "error".into(),
                detail: Some(json!({"error": take_chars(&format!("{e:#}"), 300)})),
                ..Default::default()
            },
        );
    }
}

/// `_run`: execute an authorized action and audit it.
pub async fn run(
    ctx: &App,
    tool: &str,
    args: &Value,
    task_id: &str,
    risk: &str,
    detail: &Value,
    decision: &str,
) -> Value {
    let t = tool_def(tool);
    let connector = get_str(&t, "connector");
    let started = Instant::now();
    let base = |result: &str, extra: Vec<(&str, Value)>| Audit {
        task_id: task_id.into(),
        resource: connector.clone(),
        risk: risk.into(),
        decision: decision.into(),
        result: result.into(),
        detail: Some(merged(detail, extra)),
    };
    match deps::execute(ctx, tool, args, task_id).await {
        Ok(result) => {
            ledger_after(ctx, tool, args, task_id, detail, &result);
            let ms = started.elapsed().as_millis() as i64;
            let preview = truncate(&pyjson::py_repr(&result), 600);
            ctx.audit(
                "sentinel",
                tool,
                base(
                    "success",
                    vec![("ms", json!(ms)), ("result_preview", json!(preview))],
                ),
            );
            json!({"status": "ok", "result": result})
        }
        Err(ExecError::Action(ae)) => {
            ctx.audit(
                "sentinel",
                tool,
                base(&ae.status, vec![("error", json!(ae.to_string()))]),
            );
            if ae.status == "waiting_user" {
                let reason = s_or(args, "reason", "");
                let c = ctx.clone();
                let tid = task_id.to_string();
                match ctx.bot().filter(|b| b.configured()) {
                    Some(bot) => {
                        tokio::spawn(
                            async move { safe_tg(&c, bot.send_takeover(&reason, &tid)).await },
                        );
                    }
                    None => {
                        let link = format!("{}/#browser/{tid}", ctx.kv_str("public_base"));
                        let text =
                            format!("OMuse 请求你接管浏览器 Takeover requested：{reason}\n{link}");
                        tokio::spawn(async move { notify_phone(&c, &text).await });
                    }
                }
            }
            json!({"status": ae.status, "error": ae.to_string()})
        }
        Err(ExecError::Internal(msg)) => {
            ctx.audit(
                "sentinel",
                tool,
                base("error", vec![("error", json!(take_chars(&msg, 500)))]),
            );
            json!({"status": "error", "error": format!("内部错误 internal error: {}", take_chars(&msg, 300))})
        }
    }
}

/// `POST /internal/act`: the policy gate for every tool call the agent makes.
pub async fn act(State(ctx): State<App>, body: Bytes) -> JsonResult {
    let b = body_obj(&body)?;
    let tool = s_or(&b, "tool", "");
    let mut args = match b.get("args") {
        Some(Value::Object(m)) => Value::Object(m.clone()),
        _ => json!({}),
    };
    let task_id = s_or(&b, "task_id", "");
    let call_id = s_or(&b, "call_id", "");
    let Some(t) = tool_opt(&tool) else {
        ctx.audit(
            "sentinel",
            if tool.is_empty() { "unknown" } else { &tool },
            Audit {
                task_id,
                decision: DENY.into(),
                result: "denied".into(),
                detail: Some(json!({"reason": "unknown tool"})),
                ..Default::default()
            },
        );
        return Ok(Json(
            json!({"status": "denied", "reason": format!("未知工具 unknown tool {tool}")}),
        ));
    };
    let connector = get_str(&t, "connector");
    let (mut elem, mut page) = (None, None);
    if connector == "browser" && tool != "browser_request_takeover" && tool != "browser_downloads" {
        match context_for(&ctx, &tool, &args, &task_id).await {
            Ok((e, p)) => {
                elem = e;
                page = p;
            }
            Err(e) => {
                ctx.audit(
                    "sentinel",
                    &tool,
                    Audit {
                        task_id,
                        resource: "browser".into(),
                        decision: "ERROR".into(),
                        result: e.status.clone(),
                        detail: Some(json!({"args": safe_args(&args), "error": e.to_string()})),
                        ..Default::default()
                    },
                );
                return Ok(Json(json!({"status": e.status, "error": e.to_string()})));
            }
        }
    }
    let is_click = tool == "browser_click" || tool == "browser_click_at";
    if is_click {
        if let Some(p) = page.as_ref().filter(|p| truthy(p)) {
            let dom = guard::domain_of(&s_opt(p, "url"));
            if ctx
                .store
                .active_purchase(&task_id, &dom)
                .ok()
                .flatten()
                .is_some()
            {
                // a confirmed purchase: check the total the page shows before letting a pay click through
                if let Ok(pt) = deps::broker(
                    &ctx,
                    "POST",
                    "/agent/page_text",
                    Some(&json!({"task_id": task_id})),
                    15.0,
                )
                .await
                {
                    let txt = s_opt(&pt.json(), "text");
                    page = Some(merged(p, vec![("text", json!(txt))]));
                }
            }
        }
    }
    let mut order_hits: Option<Vec<Value>> = None;
    if is_click {
        if let Some(e) = elem.as_ref().filter(|e| truthy(e)) {
            if guard::order_change_click(&s_or(e, "name", "")) {
                // cancel / return: which order in the ledger is this page about?
                let txt = match deps::broker(
                    &ctx,
                    "POST",
                    "/agent/page_text",
                    Some(&json!({"task_id": task_id})),
                    15.0,
                )
                .await
                {
                    Ok(BrokerOut::Json(pt)) => s_opt(&pt, "text"),
                    _ => String::new(),
                };
                let url = page.as_ref().map(|p| s_opt(p, "url")).unwrap_or_default();
                order_hits =
                    Some(ledger::in_text(&ctx.store, &format!("{txt} {url}")).map_err(ierr)?);
            }
        }
    }
    let d: Decision = decide(
        &ctx.store,
        &tool,
        &args,
        &task_id,
        elem.as_ref(),
        page.as_ref(),
        ctx.gmail_ready(),
    )
    .map_err(ierr)?;
    let mut detail =
        json!({"args": safe_args(&args), "reason": d.reason, "destination": d.destination});
    if let Some(e) = elem.as_ref().filter(|e| truthy(e)) {
        detail["element"] =
            json!({"tag": g(e, "tag"), "name": g(e, "name"), "input_type": g(e, "input_type")});
    }
    let au = |result: &str, decision: &str, det: Value| Audit {
        task_id: task_id.clone(),
        resource: connector.clone(),
        risk: d.risk.clone(),
        decision: decision.into(),
        result: result.into(),
        detail: Some(det),
    };
    if d.decision == DENY {
        ctx.audit("sentinel", &tool, au("denied", DENY, detail));
        return Ok(Json(json!({"status": "denied", "reason": d.reason})));
    }
    if b.get("dry_run").is_some_and(truthy) && (d.decision == ASK || dry_run_side_effect(&tool, &t))
    {
        // golden (test) run: stop where a real run would act or ask — nothing is sent, bought or written
        ctx.audit(
            "sentinel",
            &tool,
            au(
                "dry_run",
                DENY,
                merged(&detail, vec![("would", json!(d.decision))]),
            ),
        );
        let what = if d.decision == ASK {
            "需要你批准"
        } else {
            "会真正执行"
        };
        let title = {
            let x = s_opt(&t, "title");
            if x.is_empty() {
                tool.clone()
            } else {
                x
            }
        };
        return Ok(Json(json!({
            "status": "denied", "dry_run": true,
            "reason": format!("{title}：{what}（{}）", take_chars(&d.reason, 120)),
        })));
    }
    if d.decision == ASK && b.get("no_ask").is_some_and(truthy) {
        ctx.audit(
            "sentinel",
            &tool,
            au(
                "denied",
                DENY,
                merged(
                    &detail,
                    vec![("why", json!("needs approval but caller is a sub-agent"))],
                ),
            ),
        );
        return Ok(Json(
            json!({"status": "denied", "reason": "此操作需要用户审批，子 Agent 不能执行 (needs approval; sub-agents cannot request it)"}),
        ));
    }
    if d.decision == ASK {
        let mut summ = summary(&ctx, &tool, &mut args, elem.as_ref(), page.as_ref()).await;
        summ["reason"] = json!(d.reason);
        summ["destination"] = json!(d.destination);
        if let Some(hits) = &order_hits {
            ledger_summary(&mut summ, hits, elem.as_ref());
        }
        let ap = ctx
            .store
            .create_approval(&task_id, &call_id, &tool, &args, &summ, &d.risk, &d.reason)
            .map_err(ierr)?;
        let aid = get_str(&ap, "id");
        if summ.get("screenshot").is_some_and(truthy) {
            if let Ok(BrokerOut::Bytes(img)) = deps::broker(
                &ctx,
                "GET",
                &format!("/screenshot?task_id={task_id}"),
                None,
                20.0,
            )
            .await
            {
                let _ = std::fs::write(ctx.shots.join(format!("{aid}.jpg")), img);
            }
        }
        ctx.audit(
            "sentinel",
            &tool,
            au(
                "pending",
                ASK,
                merged(&detail, vec![("approval_id", json!(aid))]),
            ),
        );
        let c = ctx.clone();
        match ctx.bot().filter(|bt| bt.configured()) {
            Some(bot) => {
                let ap2 = ap.clone();
                tokio::spawn(async move { safe_tg(&c, bot.send_approval(&ap2)).await });
            }
            None => {
                let text = format!(
                    "OMuse 需要你审批 Approval needed：{}\n{}\n{}",
                    py_str(&g(&summ, "title")),
                    d.destination,
                    d.reason
                );
                tokio::spawn(async move { notify_phone(&c, &text).await });
            }
        }
        return Ok(Json(
            json!({"status": "approval_required", "approval_id": aid, "summary": summ, "reason": d.reason}),
        ));
    }
    Ok(Json(
        run(&ctx, &tool, &args, &task_id, &d.risk, &detail, ALLOW).await,
    ))
}

// ================================================================== resolving approvals

/// `POST /sentinel/api/approvals/{aid}/resolve`.
pub async fn resolve(State(ctx): State<App>, Path(aid): Path<String>, body: Bytes) -> JsonResult {
    let b = body_obj(&body)?;
    resolve_core(&ctx, &aid, &b, "web").await.map(Json)
}

struct Resolving<'a> {
    ctx: &'a Ctx,
    aid: String,
}

impl Drop for Resolving<'_> {
    fn drop(&mut self) {
        self.ctx.resolving.lock().remove(&self.aid);
    }
}

/// `resolve_core(aid, body, via)`: approve/deny a pending approval. Used by the web UI and the Telegram bot.
pub async fn resolve_core(ctx: &App, aid: &str, b: &Value, via: &str) -> Result<Value, ApiError> {
    {
        let mut r = ctx.resolving.lock();
        if r.contains(aid) {
            return Err(ApiError::new(409, "这个审批正在处理中 (being resolved)"));
        }
        r.insert(aid.to_string());
    }
    let _guard = Resolving {
        ctx,
        aid: aid.to_string(),
    };
    resolve_inner(ctx, aid, b, via).await
}

fn denied_notify(ctx: &App, aid: &str, task_id: &str, call_id: &Value, reason: Value) {
    spawn_notify_runtime(
        ctx,
        "/internal/approval_resolved",
        json!({"approval_id": aid, "task_id": task_id, "call_id": call_id, "decision": "denied",
               "result": {"status": "denied", "reason": reason}}),
    );
}

async fn resolve_inner(ctx: &App, aid: &str, b: &Value, via: &str) -> Result<Value, ApiError> {
    let store = &ctx.store;
    let Some(ap) = store.approval(aid).map_err(ierr)? else {
        return Err(ApiError::new(404, "approval not found"));
    };
    let status = get_str(&ap, "status");
    if status != "pending" {
        return Err(ApiError::new(409, format!("already {status}")));
    }
    let decision = g(b, "decision");
    let mut scope = s_or(b, "scope", "ONCE").to_uppercase();
    if !["ONCE", "TASK", "SESSION", "TIME_BOUND", "PERMANENT"].contains(&scope.as_str()) {
        scope = "ONCE".into();
    }
    let tool = get_str(&ap, "tool");
    let mut args = match &ap["args"] {
        Value::Object(m) => Value::Object(m.clone()),
        _ => json!({}),
    };
    let task_id = get_str(&ap, "task_id");
    let call_id = g(&ap, "call_id");
    let risk = get_str(&ap, "risk");
    let reason = s_opt(&ap, "reason");
    if PER_USE_TOOLS.contains(&tool.as_str())
        || reason.contains("spends money")
        || reason.contains("changes an order")
    {
        scope = "ONCE".into(); // payments are approved one by one: no 'always allow' for them
    }
    if decision.as_str() != Some("approve") {
        store
            .resolve_approval(
                aid,
                "denied",
                &scope,
                Some(&json!({"status": "denied"})),
                None,
                "user",
            )
            .map_err(ierr)?;
        ctx.audit(
            "user",
            "approval.deny",
            Audit {
                task_id: task_id.clone(),
                resource: tool.clone(),
                risk: risk.clone(),
                decision: DENY.into(),
                result: "denied".into(),
                detail: Some(
                    json!({"approval_id": aid, "note": g_or(b, "note", json!("")), "via": via}),
                ),
            },
        );
        let note = s_opt(b, "note");
        let why = format!(
            "用户拒绝了此操作 (user denied){}",
            if note.is_empty() {
                String::new()
            } else {
                format!("：{note}")
            }
        );
        denied_notify(ctx, aid, &task_id, &call_id, json!(why));
        return Ok(json!({"ok": true, "status": "denied"}));
    }

    let edited = match b.get("args") {
        Some(Value::Object(m)) => m.clone(),
        _ => Map::new(),
    };
    for k in editable(&tool) {
        match edited.get(*k) {
            Some(Value::String(v)) => {
                args[*k] = json!(v);
            }
            Some(Value::Array(list)) if *k == "message_ids" => {
                let keep: std::collections::HashSet<String> = list.iter().map(py_str).collect();
                let cur = args
                    .get(*k)
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                let narrowed: Vec<Value> = cur
                    .into_iter()
                    .filter(|x| keep.contains(&py_str(x)))
                    .collect(); // only narrow
                let empty = narrowed.is_empty();
                args[*k] = json!(narrowed);
                if empty {
                    store
                        .resolve_approval(
                            aid,
                            "denied",
                            &scope,
                            Some(&json!({"status": "denied"})),
                            None,
                            "user",
                        )
                        .map_err(ierr)?;
                    ctx.audit(
                        "user",
                        "approval.deny",
                        Audit {
                            task_id: task_id.clone(),
                            resource: tool.clone(),
                            risk: risk.clone(),
                            decision: DENY.into(),
                            result: "denied".into(),
                            detail: Some(json!({"approval_id": aid, "note": "no items selected"})),
                        },
                    );
                    denied_notify(
                        ctx,
                        aid,
                        &task_id,
                        &call_id,
                        json!("用户取消勾选了所有邮件，没有退订任何一封 (user deselected all)"),
                    );
                    return Ok(json!({"ok": true, "status": "denied"}));
                }
            }
            _ => {}
        }
    }
    // re-check hard rules (connector may have been disabled meanwhile); approval overrides ASK only
    let Some(t) = tool_opt(&tool) else {
        store
            .resolve_approval(
                aid,
                "denied",
                &scope,
                Some(&json!({"status": "denied", "reason": "tool no longer available"})),
                Some(&args),
                "user",
            )
            .map_err(ierr)?;
        denied_notify(
            ctx,
            aid,
            &task_id,
            &call_id,
            json!("这个工具已被移除或停用 (tool no longer available)"),
        );
        return Ok(json!({"ok": false, "status": "denied", "reason": "tool no longer available"}));
    };
    let (mut elem, mut page) = (None, None);
    if get_str(&t, "connector") == "browser" {
        if let Ok((e, p)) = context_for(ctx, &tool, &args, &task_id).await {
            elem = e;
            page = p;
        }
    }
    let d = decide(
        store,
        &tool,
        &args,
        &task_id,
        elem.as_ref(),
        page.as_ref(),
        ctx.gmail_ready(),
    )
    .map_err(ierr)?;
    if d.decision == DENY {
        store
            .resolve_approval(
                aid,
                "denied",
                &scope,
                Some(&json!({"status": "denied", "reason": d.reason})),
                Some(&args),
                "user",
            )
            .map_err(ierr)?;
        denied_notify(ctx, aid, &task_id, &call_id, json!(d.reason));
        return Ok(json!({"ok": false, "status": "denied", "reason": d.reason}));
    }
    if scope != "ONCE" {
        let ttl = match scope.as_str() {
            "SESSION" => Some(8.0 * 3600.0),
            "TIME_BOUND" => {
                let h = py_float(&g_or(b, "ttl_hours", json!(1))).unwrap_or(1.0);
                Some(h.clamp(0.25, 24.0 * 30.0) * 3600.0)
            }
            _ => None,
        };
        let title = ap["summary"].get("title").map(py_str).unwrap_or_default();
        store
            .add_grant(
                &tool,
                &scope,
                if scope == "TASK" {
                    Some(task_id.as_str())
                } else {
                    None
                },
                Some(&json!({"destination": d.destination})),
                ttl,
                &title,
            )
            .map_err(ierr)?;
    }
    let mut keys: Vec<String> = edited.keys().cloned().collect();
    keys.sort();
    ctx.audit(
        "user",
        "approval.approve",
        Audit {
            task_id: task_id.clone(),
            resource: tool.clone(),
            risk: risk.clone(),
            decision: ALLOW.into(),
            result: "approved".into(),
            detail: Some(json!({"approval_id": aid, "scope": scope, "edited": keys, "via": via})),
        },
    );
    let detail = json!({"args": safe_args(&args), "approval_id": aid});
    let result = run(ctx, &tool, &args, &task_id, &risk, &detail, "APPROVED").await;
    store
        .resolve_approval(aid, "approved", &scope, Some(&result), Some(&args), "user")
        .map_err(ierr)?;
    if scope == "ONCE" {
        if let Ok(Some(apv)) = store.approval(aid) {
            if let Ok(Some(sg)) = suggest::just_reached(store, &apv) {
                let dest = s_opt(&sg, "destination");
                let msg = format!(
                    "🔁 你已经第 {} 次批准「{}」{}。要不要以后自动允许？到「可信度 Trust」页可以开启（随时可撤销）。",
                    py_str(&g(&sg, "count")),
                    py_str(&g(&sg, "title")),
                    if dest.is_empty() { String::new() } else { format!("（{dest}）") }
                );
                if ctx.bot().is_some_and(|bt| bt.configured()) {
                    let c = ctx.clone();
                    let m = msg.clone();
                    tokio::spawn(async move {
                        safe_tg(&c, async {
                            deps::telegram_send(&c, &m)
                                .await
                                .map(|_| ())
                                .map_err(anyhow::Error::from)
                        })
                        .await
                    });
                }
                spawn_notify_runtime(
                    ctx,
                    "/internal/notify_user",
                    json!({"title": "🔁 可以少批一次 Fewer approvals", "body": msg}),
                );
            }
        }
    }
    spawn_notify_runtime(
        ctx,
        "/internal/approval_resolved",
        json!({"approval_id": aid, "task_id": task_id, "call_id": call_id, "decision": "approved", "result": result}),
    );
    Ok(json!({"ok": true, "status": "approved", "result": result}))
}

fn tool_opt(name: &str) -> Option<Value> {
    tool(name)
}
