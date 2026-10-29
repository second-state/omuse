//! Agent Runtime: task state machine, planner, executor loop, sub-agents, local tools (`app/runtime/agent.py`).
//!
//! The runtime holds no credentials and has no raw network tool: every external action is
//! proposed to Sentinel via /internal/act, which decides ALLOW / DENY / ASK_USER.
//!
//! The Python module is one 3600-line file; here it is split by concern, all under `agent/`:
//!
//! * [`guards`] — the loop guards (repeat / re-read / invented ids / dead-end hosts), the page hints and the
//!   answer checks (ungrounded numbers, cut-off answers, …): pure functions over the transcript.
//! * [`tools`] — the local tool schema (`LOCAL_TOOLS`) and the tool-name sets.
//! * [`lang`] — the agent's language (`agent_lang`) with the per-task overrides the Python kept in contextvars.
//! * [`runtime`] — the [`Runtime`] itself: Sentinel client, events, task lifecycle and the planner/executor loop.
//! * [`bots`] — the bot of a task (its skills and prompt) and the hand-off from the main bot (`ask_bot`).
//! * [`exec`] — one tool call: guards, Sentinel `/internal/act`, result formatting, vision helpers, evidence.
//! * [`local`] — the local tools (files, documents, data, memory, schedules, goals, choices, …).
//! * [`images`] — `make_image` / `edit_image` / `vary_image` / `caption_image` / `upscale_image`.
//! * [`videos`] — `make_video` (a short clip from an AI video model through `videogen`).
//! * [`market_text`] — the text the agent gets for `market_data` / `stock_fundamentals` (Sentinel's `market.describe*`).
//! * [`ext`] — thin adapters to the sibling runtime modules (context, library, skills, imagegen, attachments, goals,
//!   scheduler, outcome), kept in one place.
//!
//! Every user-facing and model-facing string is byte-identical to the Python source: the scripted model of the
//! end-to-end suites (`tests/fake_llm.py`) matches on them.

pub mod bots;
pub mod exec;
pub mod ext;
pub mod guards;
pub mod images;
pub mod lang;
pub mod local;
pub mod market_text;
pub mod runtime;
pub mod tools;
pub(crate) mod util;
pub mod videos;

#[cfg(test)]
mod tests;

pub use guards::*;
pub use lang::{agent_lang, request_lang};
pub use runtime::{Publish, Runtime, Suspend};
pub use tools::*;

/// Task states that never change again.
pub const TERMINAL: [&str; 3] = ["COMPLETED", "FAILED", "CANCELLED"];
/// Task states in which the task waits for the user.
pub const WAITING: [&str; 3] = ["WAITING_APPROVAL", "WAITING_EXTERNAL", "PAUSED"];
pub const RESULT_LIMIT: usize = 9000;
/// gmail_search hits are compact (~250 chars); 100+ of them must fit for counts and totals.
pub const SEARCH_RESULT_LIMIT: usize = 30000;
/// chat download; Telegram's own bot limit (50 MB) is checked by Sentinel
pub const SEND_FILE_MAX: u64 = 200 * 1024 * 1024;

/// `status in TERMINAL`.
pub fn is_terminal(status: &str) -> bool {
    TERMINAL.contains(&status)
}
