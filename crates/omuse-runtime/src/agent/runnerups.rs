//! A flight shown alone gets its runner-ups AFTER the reply (the user, 2026-11-02): when `travel_pick` shows one flight,
//! a second model call — a JSON choice over the search's other flights ([`super::booking::runner_ups`]) — starts at once
//! and runs while the reply streams. Its card is held here per task and posted to the chat only after the reply has
//! been stored, so the user reads the streamed text first and the recommendation card comes after it.

use std::collections::HashMap;
use std::sync::{Arc, LazyLock};
use std::time::Duration;

use parking_lot::Mutex;
use serde_json::Value;
use tokio::task::JoinHandle;

use super::booking::RunnerUps;
use super::runtime::Runtime;

/// The pending runner-ups card per task: the call's handle, whose output is the card's `present_choices` arguments.
static PENDING: LazyLock<Mutex<HashMap<String, JoinHandle<Option<Value>>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// How long the posted reply waits for the runner-ups call before the card is dropped.
const WAIT: Duration = Duration::from_secs(45);

/// Starts the runner-ups call for `task_id` (a later one for the same task replaces it).
pub fn start(rt: Arc<Runtime>, task_id: &str, ru: RunnerUps) {
    let tid = task_id.to_string();
    let h = tokio::spawn(async move {
        // "none" is a listed choice: a decision answers with at least one (an empty list is not an answer)
        let mut choices: Vec<&str> = ru.choices.iter().map(String::as_str).collect();
        choices.push(super::booking::NO_RUNNER_UP);
        let picked = rt
            .decide_many(
                &tid,
                &ru.question,
                &ru.context,
                &choices,
                &[super::booking::NO_RUNNER_UP],
            )
            .await;
        ru.card(&picked)
    });
    if let Some(old) = PENDING.lock().insert(task_id.to_string(), h) {
        old.abort();
    }
}

/// The card to post after the reply: waits for the call (up to [`WAIT`]); `None` when there is no call, it chose no
/// runner-up or did not finish in time.
pub async fn take(task_id: &str) -> Option<Value> {
    let h = PENDING.lock().remove(task_id)?;
    match tokio::time::timeout(WAIT, h).await {
        Ok(Ok(card)) => card,
        _ => None,
    }
}

/// Drops a pending call (the task stopped or asked the user instead).
pub fn drop_for(task_id: &str) {
    if let Some(h) = PENDING.lock().remove(task_id) {
        h.abort();
    }
}
