//! `omuse` — the OMuse services in one binary.
//!
//! ```text
//! omuse [--config config.toml] [--role all|sentinel|runtime|browser]
//! ```
//! `all` (the default) runs the three services in one process, the way the Docker image's
//! entrypoint ran three uvicorn processes; a single role is for split deployments (the Olares chart).

use std::sync::Arc;

use anyhow::Context;
use clap::{Parser, ValueEnum};
use omuse_common::Config;

#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
enum Role {
    All,
    Sentinel,
    Runtime,
    Browser,
}

#[derive(Parser, Debug)]
#[command(name = "omuse", version = omuse_common::util::VERSION.as_str(), about = "OMuse: your private AI agent")]
struct Args {
    /// Path to the TOML config file (default: $OMUSE_CONFIG or ./config.toml)
    #[arg(long)]
    config: Option<String>,
    /// Which service(s) to run in this process
    #[arg(long, value_enum, default_value_t = Role::All)]
    role: Role,
    /// Validate the config file and print the effective paths, then exit
    #[arg(long)]
    check: bool,
}

fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    let path = Config::resolve_path(args.config.as_deref());
    let cfg = Config::load(&path).with_context(|| {
        format!(
            "OMuse needs a config file ({}); copy config.example.toml and set [auth] password",
            path.display()
        )
    })?;
    if args.check {
        println!("config: {}", path.display());
        println!("sentinel data: {}", cfg.sentinel_data().display());
        println!("runtime data:  {}", cfg.runtime_data().display());
        println!("workspace:     {}", cfg.workspace().display());
        println!("browser profile: {}", cfg.browser_profile().display());
        println!("auth: {}", if cfg.auth_enabled() { "on" } else { "OFF" });
        return Ok(());
    }
    // Fail closed without a login password, like the Docker entrypoint (exit code 64).
    if matches!(args.role, Role::All | Role::Sentinel)
        && cfg.auth.enabled
        && cfg.auth.password.is_empty()
    {
        eprintln!("[auth] password is not set. OMuse reads your mail and drives a browser: set [auth] password in {}", path.display());
        eprintln!("(or [auth] enabled = false if another proxy in front already does the login).");
        std::process::exit(64);
    }
    let cfg = Arc::new(cfg);
    cfg.internal_tokens();
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .with_target(false)
        .init();

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_name("omuse")
        .build()?;
    rt.block_on(async move {
        for d in data_dirs(args.role, &cfg) {
            std::fs::create_dir_all(&d).with_context(|| format!("mkdir {}", d.display()))?;
        }
        #[cfg(unix)]
        if matches!(args.role, Role::All | Role::Sentinel) {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(
                cfg.sentinel_data(),
                std::fs::Permissions::from_mode(0o700),
            );
        }
        let mut set = tokio::task::JoinSet::new();
        if matches!(args.role, Role::All | Role::Browser) {
            let c = cfg.clone();
            set.spawn(async move { omuse_browser::serve(c).await.context("browser broker") });
        }
        if matches!(args.role, Role::All | Role::Runtime) {
            let c = cfg.clone();
            set.spawn(async move { omuse_runtime::serve(c).await.context("runtime") });
        }
        if matches!(args.role, Role::All | Role::Sentinel) {
            let c = cfg.clone();
            set.spawn(async move { omuse_sentinel::serve(c).await.context("sentinel") });
        }
        // one service gone = the whole process exits (the container restarts, like the pod)
        let first = tokio::select! {
            r = set.join_next() => r,
            _ = shutdown_signal() => { println!("[omuse] shutting down"); return Ok(()); }
        };
        match first {
            Some(Ok(Ok(()))) => Ok(()),
            Some(Ok(Err(e))) => Err(e),
            Some(Err(e)) => Err(anyhow::anyhow!("service task failed: {e}")),
            None => Ok(()),
        }
    })
}

async fn shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        let mut term = signal(SignalKind::terminate()).expect("SIGTERM handler");
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {},
            _ = term.recv() => {},
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}

/// The data directories a role works in; a role-split pod mounts only these into each container.
fn data_dirs(role: Role, cfg: &Config) -> Vec<std::path::PathBuf> {
    match role {
        Role::All => vec![
            cfg.sentinel_data(),
            cfg.runtime_data(),
            cfg.workspace(),
            cfg.browser_profile(),
        ],
        Role::Sentinel => vec![cfg.sentinel_data()],
        Role::Runtime => vec![cfg.runtime_data(), cfg.workspace()],
        Role::Browser => vec![cfg.browser_profile(), cfg.workspace()],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_role_creates_only_its_own_directories() {
        let mut cfg = Config::default();
        cfg.data.dir = "/x".into();
        let names = |role| -> Vec<String> {
            data_dirs(role, &cfg)
                .iter()
                .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
                .collect()
        };
        assert_eq!(names(Role::Sentinel), ["sentinel"]);
        assert_eq!(names(Role::Runtime), ["runtime", "workspace"]);
        assert_eq!(names(Role::Browser), ["browser-profile", "workspace"]);
        assert_eq!(names(Role::All).len(), 4);
    }
}
