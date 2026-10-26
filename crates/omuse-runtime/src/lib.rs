//! Agent Runtime (`app/runtime/*`): planner / executor, scheduler, goals, memory, library, skills.
//! Reached only through Sentinel's reverse proxy.

use std::sync::Arc;

use omuse_common::Config;

pub mod agent;
pub mod attachments;
pub mod context;
pub mod goals;
pub mod golden;
pub mod health;
pub mod imagegen;
pub mod library;
pub mod llm;
pub mod memory_tidy;
pub mod metrics;
pub mod outcome;
pub mod pdfforms;
pub mod prompts;
pub mod routes;
pub mod rt_api;
pub mod scheduler;
pub mod skills;
pub mod state;
pub mod store;
pub mod videogen;

/// Start the runtime HTTP API on `[server] runtime_host:runtime_port` and serve until the process ends.
pub async fn serve(cfg: Arc<Config>) -> anyhow::Result<()> {
    routes::serve(cfg).await
}
