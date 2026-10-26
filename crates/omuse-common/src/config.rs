//! `config.toml` — every setting the Python services took from environment variables.
//!
//! Loaded once at start (`omuse --config <path>`, default `./config.toml`, or `OMUSE_CONFIG`),
//! shared as `Arc<Config>`. Each field documents the environment variable it replaces.
//! Missing keys take the Python defaults; unknown keys are ignored.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use anyhow::{Context, Result};
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct Config {
    pub server: ServerConfig,
    pub data: DataConfig,
    pub auth: AuthConfig,
    pub model: ModelConfig,
    pub skills: SkillsConfig,
    pub browser: BrowserConfig,
    pub telegram: TelegramConfig,
    pub voice: VoiceConfig,
    pub stripe: StripeConfig,
    pub google: GoogleConfig,
    pub endpoints: EndpointsConfig,
    pub internal: InternalConfig,
    pub web: WebConfig,
    /// Where the file was loaded from (not a TOML key).
    #[serde(skip)]
    pub path: PathBuf,
}

/// `[server]` — listening addresses.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct ServerConfig {
    /// Sentinel (web UI + API) bind address. Python: uvicorn `--host 0.0.0.0`.
    pub host: String,
    /// Sentinel port (8080).
    pub port: u16,
    /// Runtime bind address + port (loopback only in the Python stack: 127.0.0.1:8081).
    pub runtime_host: String,
    pub runtime_port: u16,
    /// Browser broker bind address + port (127.0.0.1:8082).
    pub browser_host: String,
    pub browser_port: u16,
    /// Honour `X-Forwarded-*` from any proxy (uvicorn `--proxy-headers --forwarded-allow-ips='*'`).
    pub proxy_headers: bool,
    /// Timezone of the install (`TZ`): the default for Settings → Timezone.
    pub timezone: String,
    /// Olares app id (`APP_ID`), used in file paths shown to the user.
    pub app_id: String,
    /// Event-loop stall watchdog threshold in seconds (`LOOP_STALL_SECONDS`).
    pub loop_stall_seconds: f64,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            host: "0.0.0.0".into(),
            port: 8080,
            runtime_host: "127.0.0.1".into(),
            runtime_port: 8081,
            browser_host: "127.0.0.1".into(),
            browser_port: 8082,
            proxy_headers: true,
            timezone: "UTC".into(),
            app_id: "omuse".into(),
            loop_stall_seconds: 3.0,
        }
    }
}

/// `[data]` — where state lives (`OMUSE_DATA`, `SENTINEL_DATA`, `RUNTIME_DATA`, `WORKSPACE`, `BROWSER_PROFILE`).
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct DataConfig {
    /// One directory for everything (`OMUSE_DATA`, default `/omuse`): `sentinel/`, `runtime/`, `workspace/`,
    /// `browser-profile/` are created under it unless the explicit paths below are set.
    pub dir: String,
    pub sentinel: String,
    pub runtime: String,
    pub workspace: String,
    pub browser_profile: String,
}

impl Default for DataConfig {
    fn default() -> Self {
        Self {
            dir: "/omuse".into(),
            sentinel: String::new(),
            runtime: String::new(),
            workspace: String::new(),
            browser_profile: String::new(),
        }
    }
}

/// `[auth]` — the login of a standalone install (`OMUSE_PASSWORD`, `OMUSE_USER`, `OMUSE_AUTH`).
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct AuthConfig {
    /// `false` = `OMUSE_AUTH=off`: no login page — only behind another proxy that already authenticates.
    pub enabled: bool,
    /// Login name (`OMUSE_USER`, default `omuse`).
    pub user: String,
    /// The initial password (`OMUSE_PASSWORD`); the one set in Settings → Password wins afterwards.
    /// Required when `enabled` (the service refuses to start without it, exit code 64).
    pub password: String,
    /// Session cookie lifetime in days (gate.py `SESSION_DAYS`).
    pub session_days: u32,
    /// Put the login gate (`deploy/docker/gate.py`) in front of Sentinel. `false` runs Sentinel bare with the password
    /// feature still on, the way the Python test stack ran `app.sentinel.main:app` directly (tests/run_local.sh), or
    /// when another proxy in front does the login but Settings → Password should still exist.
    pub gate: bool,
}

impl Default for AuthConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            user: "omuse".into(),
            password: String::new(),
            session_days: 180,
            gate: true,
        }
    }
}

/// `[model]` — the system's OpenAI-compatible endpoint (`OMUSE_MODEL_URL`, `OMUSE_MODEL`, `OMUSE_MODEL_API_KEY`, …).
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct ModelConfig {
    /// `OMUSE_MODEL_URL` / `PERSONA_MODEL_URL`.
    pub url: String,
    /// `OMUSE_MODEL_API_KEY` / `PERSONA_MODEL_API_KEY` — never shown in Settings, never sent to a user's own endpoint.
    pub api_key: String,
    /// `OMUSE_MODEL` / `PERSONA_MODEL`.
    pub name: String,
    /// `OMUSE_PLANNER_MODEL` ("" = `name`).
    pub planner: String,
    /// `OMUSE_VISION_MODEL` ("" = `name`).
    pub vision: String,
    /// `OMUSE_STT_MODEL` ("" = a whisper-like model on the endpoint, if any).
    pub stt: String,
    /// `OMUSE_IMAGE_MODEL` ("" = auto-pick).
    pub image: String,
    /// `OMUSE_IMAGE_URL` ("" = the model endpoint).
    pub image_url: String,
    /// `OMUSE_IMAGE_API_KEY` ("" = `api_key`).
    pub image_api_key: String,
    /// `OMUSE_IMAGE_BUDGET`: generated pictures per task.
    pub image_budget: u32,
    /// `OMUSE_VIDEO_MODEL` ("" = auto-pick a video model the endpoint serves).
    pub video: String,
    /// `OMUSE_VIDEO_URL`: an OpenAI-Videos-compatible endpoint for `make_video` ("" = the model endpoint).
    pub video_url: String,
    /// `OMUSE_VIDEO_API_KEY` ("" = `api_key`).
    pub video_api_key: String,
    /// `OMUSE_VIDEO_BUDGET`: generated clips per task.
    pub video_budget: u32,
    /// `OMUSE_MODEL_FAST`: send `service_tier: "fast"` with every chat request to the system's endpoint (Settings
    /// may change it only for the user's own model).
    pub fast: bool,
    /// `OMUSE_MODEL_REASONING`: `reasoning_effort` sent with every chat request to the system's endpoint — `none`,
    /// `minimal`, `low`, `medium`, `high` or `xhigh`; "" = not sent (the model's default).
    pub reasoning_effort: String,
}

impl Default for ModelConfig {
    fn default() -> Self {
        Self {
            url: "https://api.openai.com/v1".into(),
            api_key: String::new(),
            name: "gpt-4.1".into(),
            planner: String::new(),
            vision: String::new(),
            stt: String::new(),
            image: String::new(),
            image_url: String::new(),
            image_api_key: String::new(),
            image_budget: 6,
            video: String::new(),
            video_url: String::new(),
            video_api_key: String::new(),
            video_budget: 2,
            fast: false,
            reasoning_effort: String::new(),
        }
    }
}

/// `[skills]` — `OMUSE_SKILLS` and `SKILLS_DIR`.
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct SkillsConfig {
    /// Skill names that are on when a fresh data volume is set up (`OMUSE_SKILLS`, comma list in Python).
    /// Empty = every skill. Only read at that first start.
    pub initial: Vec<String>,
    /// `None` in Python terms is "unset": `initial_set` tells whether the key was present at all.
    #[serde(skip)]
    pub initial_set: bool,
    /// Directory with `<name>/SKILL.md` folders (`SKILLS_DIR`); "" = the skills built into the binary.
    pub dir: String,
}

/// `[browser]` — the broker (`BROWSER_*`, `SEARCH_ENGINES`).
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct BrowserConfig {
    /// `BROWSER_HEADLESS`: "" auto (headed on the virtual display when one can be started), "1" force headless, "0" force headed.
    pub headless: String,
    /// `BROWSER_DISPLAY` (`:99`): the Xvfb display number to start.
    pub display: String,
    /// `BROWSER_UA`: user agent override ("" = the Python defaults).
    pub user_agent: String,
    /// `BROWSER_PACE_GAP`: minimum seconds between hits to one registered domain.
    pub pace_gap: f64,
    /// Default of "auto-approve browser actions" (Settings → Agent) until the user sets it: page loads, clicks,
    /// typing and form submits run without the approval dialog, recorded as approved by "auto". Vault fills,
    /// uploads, money and order-change clicks, anything after a suspected prompt injection, logins and takeovers
    /// still come to the user.
    pub auto_approve: bool,
    /// `BROWSER_IDLE_SHUTDOWN`: close Chromium after this many idle seconds (0 = never).
    pub idle_shutdown: f64,
    /// `SEARCH_ENGINES` (tests): `name=url-prefix,name=url-prefix`.
    pub search_engines: String,
    /// Chromium executable ("" = auto-detect: `chromium`, `chromium-browser`, `google-chrome`, Playwright's bundle).
    pub executable: String,
}

impl Default for BrowserConfig {
    fn default() -> Self {
        Self {
            headless: String::new(),
            display: ":99".into(),
            user_agent: String::new(),
            pace_gap: 1.3,
            auto_approve: true,
            idle_shutdown: 600.0,
            search_engines: String::new(),
            executable: String::new(),
        }
    }
}

/// `[telegram]` — `TELEGRAM_BOT`, `TELEGRAM_API`.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct TelegramConfig {
    /// `false` = `TELEGRAM_BOT=0`.
    pub enabled: bool,
    pub api: String,
}

impl Default for TelegramConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            api: "https://api.telegram.org".into(),
        }
    }
}

/// `[voice]` — the phone line (`VOICE_PORT`, `VOICE_ALLOW_HTTP`, `TELNYX_API`, `OPENAI_API`, `OPENAI_REALTIME_URL`, `DIALMCP_URL`).
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct VoiceConfig {
    /// `VOICE_PORT`: 0 turns the phone port off.
    pub port: u16,
    /// `VOICE_ALLOW_HTTP=1`: accept an `http://` public URL (tests).
    pub allow_http: bool,
    pub telnyx_api: String,
    pub openai_api: String,
    pub openai_realtime_url: String,
    pub dialmcp_url: String,
}

impl Default for VoiceConfig {
    fn default() -> Self {
        Self {
            port: 8083,
            allow_http: false,
            telnyx_api: "https://api.telnyx.com".into(),
            openai_api: "https://api.openai.com".into(),
            openai_realtime_url: "wss://api.openai.com/v1/realtime".into(),
            dialmcp_url: "https://mcp.dialmcp.com/mcp".into(),
        }
    }
}

/// `[stripe]` — `OMUSE_STRIPE_API_KEY`, `OMUSE_STRIPE_CUSTOMER_ID`, `OMUSE_STRIPE_SUBSCRIPTION_ID`, `STRIPE_API`.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct StripeConfig {
    pub api_key: String,
    pub customer_id: String,
    pub subscription_id: String,
    pub api: String,
}

impl Default for StripeConfig {
    fn default() -> Self {
        Self {
            api_key: String::new(),
            customer_id: String::new(),
            subscription_id: String::new(),
            api: "https://api.stripe.com".into(),
        }
    }
}

/// `[google]` — Google OAuth (`GOOGLE_*`).
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct GoogleConfig {
    pub auth_url: String,
    pub token_url: String,
    pub userinfo_url: String,
    pub gcal_api: String,
    /// `GOOGLE_RELAY_URL` ("" = the default relay page).
    pub relay_url: String,
    /// `GOOGLE_MANAGED_FILE` ("" = `google_managed.json` next to the binary's data).
    pub managed_file: String,
    /// `GOOGLE_MANAGED_CLIENT_ID` / `GOOGLE_MANAGED_CLIENT_SECRET` / `GOOGLE_BROKER_URL`.
    pub managed_client_id: String,
    pub managed_client_secret: String,
    pub broker_url: String,
}

impl Default for GoogleConfig {
    fn default() -> Self {
        Self {
            auth_url: "https://accounts.google.com/o/oauth2/v2/auth".into(),
            token_url: "https://oauth2.googleapis.com/token".into(),
            userinfo_url: "https://www.googleapis.com/oauth2/v3/userinfo".into(),
            gcal_api: "https://www.googleapis.com/calendar/v3".into(),
            relay_url: String::new(),
            managed_file: String::new(),
            managed_client_id: String::new(),
            managed_client_secret: String::new(),
            broker_url: String::new(),
        }
    }
}

/// `[endpoints]` — third-party API bases (tests point them at fakes).
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct EndpointsConfig {
    /// `NOTION_API`.
    pub notion: String,
    /// `SLACK_API`.
    pub slack: String,
    /// `MS_LOGIN_URL` (Outlook OAuth).
    pub ms_login: String,
    /// `GITHUB_RAW_URL` (skill import).
    pub github_raw: String,
    /// `MARKET_DATA_HOSTS` (Yahoo Finance hosts; empty = the two query hosts).
    pub market_data_hosts: Vec<String>,
    /// `ICAL_ALLOW_HTTP=1`.
    pub ical_allow_http: bool,
    /// Outlook's IMAP server as `host:port:security` instead of `outlook.office365.com:993:ssl` (tests: a local
    /// Dovecot); empty = Microsoft's server.
    pub outlook_imap: String,
    /// Outlook's SMTP server as `host:port:security` instead of `smtp-mail.outlook.com:587:starttls`; empty =
    /// Microsoft's server.
    pub outlook_smtp: String,
}

impl Default for EndpointsConfig {
    fn default() -> Self {
        Self {
            notion: "https://api.notion.com".into(),
            slack: "https://slack.com/api".into(),
            ms_login: "https://login.microsoftonline.com".into(),
            github_raw: "https://raw.githubusercontent.com".into(),
            market_data_hosts: vec![],
            ical_allow_http: false,
            outlook_imap: String::new(),
            outlook_smtp: String::new(),
        }
    }
}

/// `[internal]` — how the three services reach each other (`SENTINEL_URL`, `RUNTIME_URL`, `BROWSER_URL`,
/// `RUNTIME_TOKEN`, `BROWSER_TOKEN`). Blank URLs are derived from `[server]`; blank tokens are generated per start.
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct InternalConfig {
    pub sentinel_url: String,
    pub runtime_url: String,
    pub browser_url: String,
    pub runtime_token: String,
    pub browser_token: String,
}

/// `[web]` — `WEB_DIR`: serve the UI from a directory (hot reload) instead of the files built into the binary.
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct WebConfig {
    pub dir: String,
}

static TOKENS: OnceLock<(String, String)> = OnceLock::new();

impl Config {
    /// Load from a TOML file. A missing file is an error (the operator must create one; see `config.example.toml`).
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("cannot read {}", path.display()))?;
        let mut cfg =
            Self::parse(&text).with_context(|| format!("bad config {}", path.display()))?;
        cfg.path = path.to_path_buf();
        Ok(cfg)
    }

    /// Parse TOML text.
    pub fn parse(text: &str) -> Result<Self> {
        let mut cfg: Config = toml::from_str(text)?;
        // `initial = [...]` present at all? (Python distinguished unset OMUSE_SKILLS from an empty one)
        let raw: toml::Value = toml::from_str(text)?;
        cfg.skills.initial_set = raw
            .get("skills")
            .and_then(|s| s.get("initial"))
            .is_some_and(|v| !v.as_array().map(|a| a.is_empty()).unwrap_or(true));
        Ok(cfg)
    }

    /// Resolve the file: `--config` argument, else `OMUSE_CONFIG`, else `./config.toml`.
    pub fn resolve_path(arg: Option<&str>) -> PathBuf {
        if let Some(p) = arg {
            return PathBuf::from(p);
        }
        if let Ok(p) = std::env::var("OMUSE_CONFIG") {
            if !p.is_empty() {
                return PathBuf::from(p);
            }
        }
        PathBuf::from("config.toml")
    }

    // ---------------------------------------------------------------- derived paths
    fn sub(&self, explicit: &str, name: &str) -> PathBuf {
        if explicit.is_empty() {
            Path::new(&self.data.dir).join(name)
        } else {
            PathBuf::from(explicit)
        }
    }
    /// `SENTINEL_DATA`.
    pub fn sentinel_data(&self) -> PathBuf {
        self.sub(&self.data.sentinel, "sentinel")
    }
    /// `RUNTIME_DATA`.
    pub fn runtime_data(&self) -> PathBuf {
        self.sub(&self.data.runtime, "runtime")
    }
    /// `WORKSPACE`.
    pub fn workspace(&self) -> PathBuf {
        self.sub(&self.data.workspace, "workspace")
    }
    /// `BROWSER_PROFILE`.
    pub fn browser_profile(&self) -> PathBuf {
        self.sub(&self.data.browser_profile, "browser-profile")
    }

    // ---------------------------------------------------------------- derived URLs
    pub fn sentinel_url(&self) -> String {
        if self.internal.sentinel_url.is_empty() {
            format!("http://127.0.0.1:{}", self.server.port)
        } else {
            self.internal.sentinel_url.trim_end_matches('/').to_string()
        }
    }
    pub fn runtime_url(&self) -> String {
        if self.internal.runtime_url.is_empty() {
            format!("http://127.0.0.1:{}", self.server.runtime_port)
        } else {
            self.internal.runtime_url.trim_end_matches('/').to_string()
        }
    }
    pub fn browser_url(&self) -> String {
        if self.internal.browser_url.is_empty() {
            format!("http://127.0.0.1:{}", self.server.browser_port)
        } else {
            self.internal.browser_url.trim_end_matches('/').to_string()
        }
    }

    // ---------------------------------------------------------------- tokens
    /// The internal tokens: the configured ones, else random ones generated once per process
    /// (the Python entrypoint made fresh ones on every start).
    pub fn internal_tokens(&self) -> (String, String) {
        TOKENS
            .get_or_init(|| {
                let rt = if self.internal.runtime_token.is_empty() {
                    crate::util::random_token()
                } else {
                    self.internal.runtime_token.clone()
                };
                let bt = if self.internal.browser_token.is_empty() {
                    crate::util::random_token()
                } else {
                    self.internal.browser_token.clone()
                };
                (rt, bt)
            })
            .clone()
    }
    pub fn runtime_token(&self) -> String {
        self.internal_tokens().0
    }
    pub fn browser_token(&self) -> String {
        self.internal_tokens().1
    }

    /// Python `passwd.enabled()`: a password is set and auth is not off.
    pub fn auth_enabled(&self) -> bool {
        self.auth.enabled && !self.auth.password.is_empty()
    }

    /// `MARKET_DATA_HOSTS` or the Yahoo defaults.
    pub fn market_data_hosts(&self) -> Vec<String> {
        if self.endpoints.market_data_hosts.is_empty() {
            vec![
                "https://query1.finance.yahoo.com".into(),
                "https://query2.finance.yahoo.com".into(),
            ]
        } else {
            self.endpoints.market_data_hosts.clone()
        }
    }

    /// `GOOGLE_RELAY_URL` or the default relay page.
    pub fn google_relay_url(&self) -> String {
        if self.google.relay_url.is_empty() {
            "https://drlucaslu.github.io/omuse-oauth/".into()
        } else {
            self.google.relay_url.clone()
        }
    }

    /// `OMUSE_IMAGE_API_KEY` or the model key.
    pub fn image_api_key(&self) -> String {
        if self.model.image_api_key.is_empty() {
            self.model.api_key.clone()
        } else {
            self.model.image_api_key.clone()
        }
    }

    /// `OMUSE_VIDEO_API_KEY` or the model key.
    pub fn video_api_key(&self) -> String {
        if self.model.video_api_key.is_empty() {
            self.model.api_key.clone()
        } else {
            self.model.video_api_key.clone()
        }
    }

    /// `OMUSE_SKILLS` as the Python saw it: `None` when unset, else the comma-joined list.
    pub fn skills_initial(&self) -> Option<Vec<String>> {
        if self.skills.initial_set {
            Some(self.skills.initial.clone())
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_and_paths() {
        let c = Config::parse("").unwrap();
        assert_eq!(c.server.port, 8080);
        assert_eq!(c.sentinel_data(), PathBuf::from("/omuse/sentinel"));
        assert_eq!(c.runtime_url(), "http://127.0.0.1:8081");
        assert!(!c.auth_enabled());
        assert!(c.skills_initial().is_none());
        let c = Config::parse(
            "[data]\ndir='/x'\nworkspace='/w'\n[skills]\ninitial=['a']\n[auth]\npassword='p'",
        )
        .unwrap();
        assert_eq!(c.workspace(), PathBuf::from("/w"));
        assert_eq!(c.browser_profile(), PathBuf::from("/x/browser-profile"));
        assert_eq!(c.skills_initial(), Some(vec!["a".to_string()]));
        assert!(c.auth_enabled());
    }
}
