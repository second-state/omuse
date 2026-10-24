//! The agent's language (`agent_lang`, `request_lang`) and the per-task overrides the Python kept in
//! `contextvars` (`_TASK_LANG`, `_FORCE_LANG`): here they are tokio task-locals, scoped around a task's run
//! with [`with_langs`] (and re-applied to anything that run spawns).

use std::future::Future;
use std::sync::LazyLock;

use omuse_common::util::take_chars;
use regex::Regex;
use serde_json::Value;

use crate::prompts;

tokio::task_local! {
    /// `_TASK_LANG`: the language the request was written in (Settings → Reply language = match).
    pub static TASK_LANG: String;
    /// `_FORCE_LANG`: "zh" when an English app must still write Chinese text the user asked for.
    pub static FORCE_LANG: String;
}

/// 'zh' or 'en' for the language a request is written in ('' when it can't tell, e.g. only numbers or a URL).
pub fn request_lang(text: &str) -> &'static str {
    static STRIP: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"https?://\S+|\[[^\]]*\]|\([^)]*attached[^)]*\)").unwrap());
    static LETTER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[A-Za-z一-鿿]").unwrap());
    let t = STRIP.replace_all(text, " ");
    let t = take_chars(&t, 2000);
    if !LETTER.is_match(t) {
        return "";
    }
    if prompts::cjk_share(t) >= 0.3 {
        "zh"
    } else {
        "en"
    }
}

/// The current task-local overrides `(task_lang, force_lang)` ("" outside a scoped run).
pub fn current_langs() -> (String, String) {
    (
        TASK_LANG.try_with(|v| v.clone()).unwrap_or_default(),
        FORCE_LANG.try_with(|v| v.clone()).unwrap_or_default(),
    )
}

/// Run `fut` with the given per-task language overrides in scope (Python: `ContextVar.set` inside the asyncio task).
pub async fn with_langs<F: Future>(task_lang: String, force_lang: String, fut: F) -> F::Output {
    TASK_LANG
        .scope(task_lang, FORCE_LANG.scope(force_lang, fut))
        .await
}

/// The agent's language: 'en' or 'zh'. Settings → Language (unset = Chinese, as before), unless Settings → Reply
/// language is "match": then each task answers in the language its request was written in (set per run). A request
/// written in Chinese for Chinese text to use as written (a story, a post …) runs in Chinese whatever the setting.
pub fn agent_lang(settings: &Value) -> &'static str {
    let (task, forced) = current_langs();
    match forced.as_str() {
        "en" => return "en",
        "zh" => return "zh",
        _ => {}
    }
    let reply_match = settings.get("reply_language").and_then(Value::as_str) == Some("match");
    if reply_match {
        match task.as_str() {
            "en" => return "en",
            "zh" => return "zh",
            _ => {}
        }
    }
    if settings.get("language").and_then(Value::as_str) == Some("en") {
        "en"
    } else {
        "zh"
    }
}
