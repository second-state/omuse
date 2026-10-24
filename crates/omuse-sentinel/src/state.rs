//! The shared state of the Sentinel service (the module-level globals of `app/sentinel/main.py`):
//! configuration, the store, outbound HTTP clients, the phone line, the Telegram bot, the set of approvals
//! being resolved right now, and the web assets / templates. The routes layer wraps it in an `Arc`.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use omuse_apps::gcal::GoogleSettings;
use omuse_common::Config;
use omuse_mail::mailproviders::MsLogin;
use omuse_phone::Phone;
use omuse_sentinel_core::Store;
use omuse_web::assets::Assets;
use omuse_web::templates::Templates;

use crate::telegram_bot::TelegramBot;

/// Everything a Sentinel handler needs. Plain struct with public fields; build it with [`AppState::new`].
pub struct AppState {
    pub cfg: Arc<Config>,
    pub store: Arc<Store>,
    /// Outbound HTTP (broker, connectors, Telegram notifications): no total timeout, every call sets its own.
    pub http: reqwest::Client,
    /// The `/api/*` reverse proxy to the runtime: 600 s total, 10 s connect, keep-alive 3 s (shorter than the
    /// runtime server's, so a connection the runtime is closing is never reused).
    pub proxy_client: reqwest::Client,
    /// The phone line (Telnyx / DialMCP).
    pub phone: Arc<Phone>,
    /// The two-way Telegram bot (started by the routes layer unless `[telegram] enabled = false`).
    pub bot: Arc<TelegramBot>,
    /// Approvals being resolved right now (`_resolving`; web + Telegram may race).
    pub resolving: parking_lot::Mutex<HashSet<String>>,
    /// The web UI files (`WEB_DIR` or the files built into the binary).
    pub assets: Arc<Assets>,
    pub templates: Arc<Templates>,
    /// Microsoft login endpoint client (Outlook mailboxes).
    pub ms_login: MsLogin,
    /// Google OAuth / Calendar endpoints.
    pub google: GoogleSettings,
}

impl AppState {
    /// Build the state. The Telegram bot is created but not started (`state.bot.start()`), and has no
    /// approval resolver until the routes layer calls `state.bot.set_resolver(..)`.
    pub fn new(cfg: Arc<Config>, store: Arc<Store>) -> AppState {
        let proxy_client = reqwest::Client::builder()
            .timeout(Duration::from_secs(600))
            .connect_timeout(Duration::from_secs(10))
            .pool_idle_timeout(Duration::from_secs(3))
            .no_proxy()
            .build()
            .expect("reqwest client");
        let assets = if cfg.web.dir.is_empty() {
            Assets::embedded()
        } else {
            Assets::from_dir(cfg.web.dir.clone())
        };
        AppState {
            http: omuse_common::http::client_no_timeout(),
            proxy_client,
            phone: Arc::new(Phone::new(store.clone(), cfg.voice.clone())),
            bot: Arc::new(TelegramBot::new(
                cfg.clone(),
                store.clone(),
                cfg.runtime_url(),
                None,
            )),
            resolving: parking_lot::Mutex::new(HashSet::new()),
            assets: Arc::new(assets),
            templates: Arc::new(Templates::new()),
            ms_login: MsLogin::new(cfg.endpoints.ms_login.clone()),
            google: GoogleSettings::from_config(&cfg),
            cfg,
            store,
        }
    }
}
