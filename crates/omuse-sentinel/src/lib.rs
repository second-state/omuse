//! Sentinel: the front door and the sole permission authority (`app/sentinel/*` + `deploy/docker/gate.py`).
//!
//! * Serves the web UI and reverse-proxies `/api/*` to the Agent Runtime.
//! * `/internal/*` is for the runtime only (runtime token): tool catalog, act, audit, notify.
//! * `/sentinel/api/*` is for the user (via the login gate / Olares entrance): approvals, connections,
//!   credentials, audit, browser live view / takeover.

use std::sync::Arc;

use omuse_common::Config;

pub use omuse_sentinel_core as core;

pub mod actions;
pub mod routes;
pub mod state;
pub mod telegram_bot;
pub mod watchers;

/// Start Sentinel (and the voice app when `[voice] port` is not 0) and serve until the process ends.
pub async fn serve(cfg: Arc<Config>) -> anyhow::Result<()> {
    routes::serve(cfg).await
}
