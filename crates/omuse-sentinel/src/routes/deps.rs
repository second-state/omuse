//! The routes' view of the sibling modules (`actions`, `telegram_bot`, `watchers`, `state`): the few adapters
//! that turn their Rust signatures into the shapes the handlers use (Python call sites).

use std::sync::{Arc, Weak};
use std::time::Duration;

use omuse_sentinel_core::errors::ActionError;
use omuse_sentinel_core::store::Store;
use serde_json::Value;

use super::{App, Ctx};
use crate::actions::{self, BrokerBody, ExecError};
pub use crate::telegram_bot::TelegramBot as Bot;

fn to_action(e: ExecError) -> ActionError {
    match e {
        ExecError::Action(a) => a,
        ExecError::Internal(m) => ActionError::new(m),
    }
}

// ------------------------------------------------------------------ broker client (actions.broker)

/// What the browser broker answered: JSON, or an image's bytes.
#[derive(Debug, Clone)]
pub enum BrokerOut {
    Json(Value),
    Bytes(Vec<u8>),
}

impl BrokerOut {
    /// The JSON answer (`{}` for bytes).
    pub fn json(self) -> Value {
        match self {
            BrokerOut::Json(v) => v,
            BrokerOut::Bytes(_) => serde_json::json!({}),
        }
    }
}

/// `actions.broker(method, path, json, timeout)`.
pub async fn broker(
    ctx: &Ctx,
    method: &str,
    path: &str,
    body: Option<&Value>,
    timeout: f64,
) -> Result<BrokerOut, ActionError> {
    Ok(
        match actions::broker_raw(ctx, method, path, body, timeout).await? {
            BrokerBody::Json(v) => BrokerOut::Json(v),
            BrokerBody::Image { data, .. } => BrokerOut::Bytes(data),
        },
    )
}

// ------------------------------------------------------------------ actions

/// `actions.telegram_send(store, text)`.
pub async fn telegram_send(ctx: &Ctx, text: &str) -> Result<Value, ActionError> {
    actions::telegram_send(ctx, text).await.map_err(to_action)
}

/// `actions.calendar_read_only(store)`.
pub fn calendar_read_only(store: &Store) -> bool {
    actions::calendar_read_only(store)
}

/// `actions.execute(store, tool, args, task_id)`.
pub async fn execute(
    ctx: &Ctx,
    tool: &str,
    args: &Value,
    task_id: &str,
) -> Result<Value, ExecError> {
    actions::execute(ctx, tool, args, task_id).await
}

/// `actions.calendar_call(store, tool, args, task_id)`.
pub async fn calendar_call(
    ctx: &Ctx,
    tool: &str,
    args: &Value,
    task_id: &str,
) -> Result<Value, ActionError> {
    actions::calendar_call(ctx, tool, args, task_id)
        .await
        .map_err(to_action)
}

/// `actions.calendar_event(store, event_id, calendar_id)`.
pub async fn calendar_event(
    ctx: &Ctx,
    event_id: &str,
    calendar_id: Option<&str>,
) -> Result<Value, ActionError> {
    actions::calendar_event(ctx, event_id, calendar_id)
        .await
        .map_err(to_action)
}

/// `actions.unsubscribe_targets(store, ids)`.
pub async fn unsubscribe_targets(ctx: &Ctx, ids: &[String]) -> Result<Vec<Value>, ExecError> {
    actions::unsubscribe_targets(ctx, ids).await
}

/// `g, raw = actions.client_for_id(store, mid); g.reply_defaults(raw, reply_all)` → `{to, cc, subject}`.
pub async fn reply_defaults(
    ctx: &Ctx,
    message_id: &str,
    reply_all: bool,
) -> Result<Value, ExecError> {
    let (mut g, raw) = actions::client_for_id(ctx, message_id)?;
    Ok(g.reply_defaults(&raw, reply_all).await?)
}

/// A failed mailbox call: the Python exception class name and message.
#[derive(Debug, Clone)]
pub struct MailError {
    /// `type(e).__name__` (`GmailError`, `ActionError`, `TimeoutError`, ...).
    pub kind: String,
    pub msg: String,
}

/// `g = actions.gmail_client(store, account); g.test()` → `(g.email, info)`; `timeout` like `asyncio.wait_for`.
pub async fn gmail_test(
    ctx: &Ctx,
    account: Option<&str>,
    timeout: Option<f64>,
) -> Result<(String, Value), MailError> {
    let mut g = actions::gmail_client(ctx, account).map_err(|e| MailError {
        kind: "ActionError".into(),
        msg: e.to_string(),
    })?;
    let r = match timeout {
        Some(t) => match tokio::time::timeout(Duration::from_secs_f64(t), g.test()).await {
            Ok(r) => r,
            Err(_) => {
                return Err(MailError {
                    kind: "TimeoutError".into(),
                    msg: String::new(),
                })
            }
        },
        None => g.test().await,
    };
    r.map(|info| (g.email.clone(), info))
        .map_err(|e| MailError {
            kind: "GmailError".into(),
            msg: e.to_string(),
        })
}

/// `actions.gmail_client(store, aid).search(query, n)`.
pub async fn gmail_search(
    ctx: &Ctx,
    aid: &str,
    query: &str,
    n: i64,
) -> Result<Vec<Value>, ExecError> {
    let mut g = actions::gmail_client(ctx, Some(aid))?;
    Ok(g.search(query, n).await?)
}

/// `_mail_connect`'s test: `g = actions.make_client(acc, secret, store)` (using a fresh OAuth access token when
/// given), then `g.test()`. The error is the `GmailError` text.
pub async fn mail_connect_test(
    ctx: &Ctx,
    acc: &Value,
    secret: &Value,
    access_token: &str,
) -> Result<Value, String> {
    let mut g = actions::make_client(acc, secret, Some(ctx.store.clone()), &ctx.ms_login);
    if !access_token.is_empty() {
        let tok = access_token.to_string();
        let f: omuse_mail::gmail::TokenFn = Arc::new(move || {
            let t = tok.clone();
            Box::pin(async move { Ok(t) })
        });
        g.token_fn = Some(f);
    }
    g.test().await.map_err(|e| e.to_string())
}

/// `actions._MS_TOKENS.pop(aid, None)`.
pub async fn forget_ms_token(aid: &str) {
    omuse_mail::gmail::forget_ms_token(aid).await;
}

// ------------------------------------------------------------------ watchers

/// `watchers.SOURCES`.
pub fn watch_sources() -> Value {
    crate::watchers::SOURCES.clone()
}

/// `watchers.poll(store, src, params, cursor)`, or `watchers.poll_web(...)` for `web.page`. `Err` is a `WatchError`.
pub async fn watch_poll(
    ctx: &Ctx,
    src: &str,
    params: &Value,
    cursor: &Value,
) -> Result<Value, String> {
    let cur = if cursor.is_null() { None } else { Some(cursor) };
    let r = if src == "web.page" {
        crate::watchers::poll_web(ctx, params, cur).await
    } else {
        crate::watchers::poll(ctx, src, params, cur).await
    };
    r.map_err(|e| e.0)
}

// ------------------------------------------------------------------ the Telegram bot

/// The bot calls as the routes use them.
pub trait BotExt {
    /// `bot.status` (a copy).
    fn status_value(&self) -> Value;
    /// `bot.status["running"]`.
    fn running(&self) -> bool;
    /// `bot.status["username"] = ""` (a new bot token was saved).
    fn clear_username(&self);
    /// `bool(bot.config())`.
    fn configured(&self) -> bool;
}

impl BotExt for Bot {
    fn status_value(&self) -> Value {
        self.status.lock().to_value()
    }
    fn running(&self) -> bool {
        self.status.lock().running
    }
    fn clear_username(&self) {
        self.status.lock().username.clear();
    }
    fn configured(&self) -> bool {
        self.config().is_some()
    }
}

/// Install `resolve_core` as the bot's approval resolver (`TelegramBot(store, RUNTIME_URL, resolve_core)`).
pub fn install_resolver(ctx: &App) {
    let weak: Weak<Ctx> = Arc::downgrade(ctx);
    ctx.bot
        .set_resolver(Arc::new(move |aid: String, body: Value, via: String| {
            let weak = weak.clone();
            Box::pin(async move {
                let Some(ctx) = weak.upgrade() else {
                    return Err(omuse_common::api::ApiError::new(
                        503,
                        "sentinel is shutting down",
                    ));
                };
                super::resolve_core(&ctx, &aid, &body, &via).await
            })
        }));
}
