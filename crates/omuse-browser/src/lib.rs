//! Browser Broker (`app/browser/main.py`): a restricted API over a persistent Chromium driven through the
//! DevTools protocol. The agent never gets raw CDP or JS execution: it can only navigate, read an
//! accessibility-style snapshot and act on element refs. Sentinel is the only caller (shared browser token).
//! The user can take over at any time; while the user has control every agent call is rejected with 423.

use std::sync::Arc;

use omuse_common::Config;

pub mod actions;
pub mod broker;
pub mod downloads;
pub mod keys;
pub mod launch;
pub mod pages;
pub mod render;
pub mod routes;
pub mod search;
pub mod snapshot;
pub mod textfmt;

/// Start the broker on `[server] browser_host:browser_port` and serve until the process ends.
pub async fn serve(cfg: Arc<Config>) -> anyhow::Result<()> {
    routes::serve(cfg).await
}
