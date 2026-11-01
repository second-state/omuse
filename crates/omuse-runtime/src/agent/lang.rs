//! The agent's language (`agent_lang`, the model's `request_lang` decision) and the per-task overrides the Python kept in
//! `contextvars` (`_TASK_LANG`, `_FORCE_LANG`): here they are tokio task-locals, scoped around a task's run
//! with [`with_langs`] (and re-applied to anything that run spawns).

use std::future::Future;

use omuse_common::util::take_chars;
use serde_json::Value;

use super::Runtime;

tokio::task_local! {
    /// `_TASK_LANG`: the language the request was written in (Settings → Reply language = match).
    pub static TASK_LANG: String;
    /// `_FORCE_LANG`: "zh" when an English app must still write Chinese text the user asked for.
    pub static FORCE_LANG: String;
}

/// The choices of the request-language decision: the model reads the request, code maps its choice.
pub const REQUEST_LANG_CHOICES: [&str; 3] = ["en", "zh", "other"];

/// The question of the request-language decision (Settings → Reply language = match).
pub const REQUEST_LANG_QUESTION: &str = "Which language is the user's request written in? \"en\": English; \"zh\": \
Chinese; \"other\": another language, or it cannot be told (only numbers, a link, a name). Names, places, links and \
terms quoted from another language inside a sentence do not change the sentence's language.";

/// The task language for a choice of the request-language decision: "en" / "zh", or "" (the setting decides).
pub fn task_lang_of(choice: &str) -> &'static str {
    match choice {
        "en" => "en",
        "zh" => "zh",
        _ => "",
    }
}

impl Runtime {
    /// Settings → Reply language = match: the language the request is written in, as the model reads it — a
    /// [`Runtime::decide`] over [`REQUEST_LANG_CHOICES`] (no character counting in code). "" when it is another
    /// language or the model gave no listed choice: then Settings → Language decides.
    pub async fn request_lang(&self, task_id: &str, request: &str) -> &'static str {
        if request.trim().is_empty() {
            return "";
        }
        let choice = self
            .decide(
                task_id,
                REQUEST_LANG_QUESTION,
                take_chars(request, 2000),
                &REQUEST_LANG_CHOICES,
                "other",
            )
            .await;
        task_lang_of(&choice)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_request_language_is_a_choice_code_maps() {
        assert_eq!(task_lang_of("en"), "en");
        assert_eq!(task_lang_of("zh"), "zh");
        // another language or no answer: the setting decides
        assert_eq!(task_lang_of("other"), "");
        assert_eq!(task_lang_of(""), "");
        assert!(REQUEST_LANG_CHOICES
            .iter()
            .all(|c| REQUEST_LANG_QUESTION.contains(&format!("\"{c}\""))));
    }
}
