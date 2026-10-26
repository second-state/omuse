//! Google Calendar connector (official REST API v3, OAuth 2.0 with the user's own Google Cloud client).
//!
//! Setup (Connections page): the user creates an OAuth client (type "Web application") in their own Google Cloud
//! project, enables the Calendar API, adds OMuse's callback URL as a redirect URI and pastes the client id/secret.
//! "Connect with Google" runs the consent flow; the refresh token is stored in Sentinel's vault and never leaves it.
//! The agent only ever sees event data, never tokens.
//!
//! All Google URLs come from `[google]` in config.toml ([`GoogleSettings::from_config`]).

use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock};
use std::time::Duration as StdDuration;

use chrono::{Duration, NaiveDateTime, NaiveTime};
use omuse_common::config::Config;
use omuse_common::pyjson::py_repr;
use omuse_common::util::{get_str, get_truthy, now_ts, py_str, take_chars, truthy};
use omuse_sentinel_core::Store;
use regex::Regex;
use serde_json::{json, Map, Value};

use crate::py::{httpx_name, py_int, quote, str_or_empty, urlencode};
use crate::pydt::{py_max, py_min, PyDt, PyTz};

/// OAuth scopes requested at consent for the calendar.
pub const SCOPES: &str =
    "openid email https://www.googleapis.com/auth/calendar.events https://www.googleapis.com/auth/calendar.readonly";
/// The Drive scope (full access: the agent lists, downloads, uploads and trashes the user's own files; `drive.file`
/// would only see files the app created). Google classes it as restricted: fine for the user's own OAuth client.
pub const DRIVE_SCOPE: &str = "https://www.googleapis.com/auth/drive";

/// The scope string for one Google sign-in covering `services` (any of "calendar", "drive"; both when empty).
pub fn scopes_for(services: &[String]) -> String {
    let want = |s: &str| services.is_empty() || services.iter().any(|x| x == s);
    let mut out = vec!["openid", "email"];
    if want("calendar") {
        out.extend(
            SCOPES
                .split_whitespace()
                .filter(|s| !matches!(*s, "openid" | "email")),
        );
    }
    if want("drive") {
        out.push(DRIVE_SCOPE);
    }
    out.join(" ")
}

/// Which services a token's granted `scope` string covers (Google lets the user untick scopes at consent).
pub fn granted_services(scope: &str) -> Vec<String> {
    let mut out = Vec::new();
    if scope.contains("auth/calendar") {
        out.push("calendar".to_string());
    }
    if scope.contains("auth/drive") {
        out.push("drive".to_string());
    }
    out
}
/// HTTP timeout (seconds) of every Google call.
pub const TIMEOUT: f64 = 30.0;
/// The relay page Google redirects to when no other is configured.
pub const DEFAULT_RELAY: &str = "https://drlucaslu.github.io/omuse-oauth/";
/// Vault handle of a Google client the user pasted on this box (redirect URI = the relay).
pub const BOX_CLIENT: &str = "cred_google_oauth_client";

/// Python `GCalError` (also raised by the iCal feed).
#[derive(Debug, Clone, thiserror::Error)]
#[error("{0}")]
pub struct GCalError(pub String);

fn arr(v: Option<&Value>) -> &[Value] {
    v.and_then(Value::as_array)
        .map(|a| a.as_slice())
        .unwrap_or(&[])
}

fn obj_or_empty(v: Option<&Value>) -> Value {
    v.filter(|x| truthy(x)).cloned().unwrap_or(json!({}))
}

// ------------------------------------------------------------------ settings

/// The Google endpoints and the OMuse-managed client settings (`[google]` in config.toml).
#[derive(Debug, Clone)]
pub struct GoogleSettings {
    /// `GOOGLE_AUTH_URL`.
    pub auth_url: String,
    /// `GOOGLE_TOKEN_URL`.
    pub token_url: String,
    /// `GCAL_API`.
    pub gcal_api: String,
    /// `GDRIVE_API` / `GDRIVE_UPLOAD_API`.
    pub gdrive_api: String,
    pub gdrive_upload_api: String,
    /// `GOOGLE_USERINFO_URL`.
    pub userinfo_url: String,
    /// `GOOGLE_RELAY_URL` or [`DEFAULT_RELAY`] (`Config::google_relay_url()`).
    pub relay_url: String,
    /// `GOOGLE_RELAY_URL` exactly as configured ("" = unset): overrides the managed file's `relay`.
    pub relay_override: String,
    /// `GOOGLE_MANAGED_FILE`: `google_managed.json` (`cfg.google.managed_file`, else `<data dir>/google_managed.json`).
    pub managed_file: PathBuf,
    /// `GOOGLE_MANAGED_CLIENT_ID`.
    pub managed_client_id: String,
    /// `GOOGLE_MANAGED_CLIENT_SECRET`.
    pub managed_client_secret: String,
    /// `GOOGLE_BROKER_URL`.
    pub broker_url: String,
}

impl GoogleSettings {
    pub fn from_config(cfg: &Config) -> Self {
        let g = &cfg.google;
        let managed_file = if g.managed_file.is_empty() {
            Path::new(&cfg.data.dir).join("google_managed.json")
        } else {
            PathBuf::from(&g.managed_file)
        };
        Self {
            auth_url: g.auth_url.clone(),
            token_url: g.token_url.clone(),
            gcal_api: g.gcal_api.clone(),
            gdrive_api: g.gdrive_api.clone(),
            gdrive_upload_api: g.gdrive_upload_api.clone(),
            userinfo_url: g.userinfo_url.clone(),
            relay_url: cfg.google_relay_url(),
            relay_override: g.relay_url.clone(),
            managed_file,
            managed_client_id: g.managed_client_id.clone(),
            managed_client_secret: g.managed_client_secret.clone(),
            broker_url: g.broker_url.clone(),
        }
    }
}

// ------------------------------------------------------------------ time parsing

fn zone_of(name: &str) -> PyTz {
    PyTz::zone_or(if name.is_empty() { "UTC" } else { name }, PyTz::UTC)
}

fn bad_time(value: &Value) -> GCalError {
    GCalError(format!(
        "看不懂的时间格式 (bad time): {}；请用 2026-10-03 19:00 这种格式",
        py_repr(value)
    ))
}

/// `'2026-10-03 19:00'`, `'2026-10-03T19:00'`, ISO with offset/Z, or a date → aware datetime in `zone`
/// (Python `GCal.parse_time`; "" → "缺少时间").
pub fn parse_time_in(value: &Value, zone: PyTz) -> Result<PyDt, GCalError> {
    static DATE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\d{4}-\d{2}-\d{2}$").unwrap());
    let mut s = str_or_empty(value).trim().to_string();
    if s.is_empty() {
        return Err(GCalError("缺少时间 (missing time)".into()));
    }
    s = s.replace('Z', "+00:00");
    if DATE.is_match(&s) {
        s.push_str("T00:00");
    }
    let s = s.replacen(' ', "T", 1);
    PyDt::fromisoformat(&s, zone).ok_or_else(|| bad_time(value))
}

/// Standalone time parser (no connected client needed) for the add-to-calendar links.
fn parse_dt(value: &Value, tz: &str) -> Result<PyDt, GCalError> {
    static DATE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\d{4}-\d{2}-\d{2}$").unwrap());
    let mut s = str_or_empty(value).trim().replace('Z', "+00:00");
    if DATE.is_match(&s) {
        s.push_str("T00:00");
    }
    let s = s.replacen(' ', "T", 1);
    PyDt::fromisoformat(&s, zone_of(tz)).ok_or_else(|| bad_time(value))
}

/// Build one-click "add to calendar" links from the same args calendar_create_event takes.
///
/// Works with NO calendar connection — used as the reliable fallback when the Calendar API is not connected
/// or its OAuth has expired. The user taps the link and the event opens pre-filled in their calendar.
pub fn add_to_calendar_links(a: &Value, tz: &str) -> Result<Value, GCalError> {
    let mut title = str_or_empty(&a["title"]);
    if title.is_empty() {
        title = str_or_empty(&a["summary"]);
    }
    let title = title.trim();
    let title = take_chars(
        if title.is_empty() {
            "(无标题 no title)"
        } else {
            title
        },
        300,
    )
    .to_string();
    let all_day = get_truthy(a, "all_day");
    let s = parse_dt(&a["start"], tz)?;
    let mut e = if get_truthy(a, "end") {
        parse_dt(&a["end"], tz)?
    } else {
        s.add(if all_day {
            Duration::days(1)
        } else {
            Duration::hours(1)
        })
    };
    if e <= s {
        e = s.add(if all_day {
            Duration::days(1)
        } else {
            Duration::hours(1)
        });
    }
    let (su, eu) = (s.astimezone(PyTz::UTC), e.astimezone(PyTz::UTC));
    let loc = take_chars(&str_or_empty(&a["location"]), 500).to_string();
    let desc = take_chars(&str_or_empty(&a["description"]), 1500).to_string();
    let g_dates = if all_day {
        format!("{}/{}", su.strftime("%Y%m%d"), eu.strftime("%Y%m%d"))
    } else {
        format!(
            "{}/{}",
            su.strftime("%Y%m%dT%H%M%SZ"),
            eu.strftime("%Y%m%dT%H%M%SZ")
        )
    };
    let google = format!(
        "https://calendar.google.com/calendar/render?{}",
        urlencode(&[
            ("action", "TEMPLATE"),
            ("text", &title),
            ("dates", &g_dates),
            ("details", &desc),
            ("location", &loc)
        ])
    );
    let outlook = format!(
        "https://outlook.live.com/calendar/0/deeplink/compose?{}",
        urlencode(&[
            ("subject", &title),
            ("body", &desc),
            ("location", &loc),
            ("path", "/calendar/action/compose"),
            ("rru", "addevent"),
            ("startdt", &su.strftime("%Y-%m-%dT%H:%M:%SZ")),
            ("enddt", &eu.strftime("%Y-%m-%dT%H:%M:%SZ")),
        ])
    );
    Ok(
        json!({"google": google, "outlook": outlook, "title": title, "all_day": all_day,
              "start": s.strftime("%Y-%m-%d %H:%M"), "end": e.strftime("%Y-%m-%d %H:%M")}),
    )
}

// ------------------------------------------------------------------ OMuse's own ("managed") Google client
// One OAuth client registered by the OMuse publisher, so users just "Sign in with Google" — no Google Cloud project.
// Every Olares box has its own domain and Google only accepts pre-registered redirect URIs, so Google redirects to one
// fixed relay page (relay) that forwards the code to the box named in `state`; the box checks the state it issued and
// exchanges the code (PKCE). The client secret is either built in, or kept by a tiny token broker (broker) that adds
// it to token requests, so it never ships in the app. Tokens are stored only in the box's vault.

/// `{client_id, client_secret, relay, broker, source}`: the build's OMuse client (`source == "build"`) or one
/// saved on this box (`source == "box"`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Managed {
    pub client_id: String,
    pub client_secret: String,
    pub relay: String,
    pub broker: String,
    pub source: String,
}

impl Managed {
    pub fn to_value(&self) -> Value {
        json!({"client_id": self.client_id, "client_secret": self.client_secret, "relay": self.relay, "broker": self.broker, "source": self.source})
    }
}

fn built_in(settings: &GoogleSettings) -> Option<Managed> {
    let file: Value = std::fs::read_to_string(&settings.managed_file)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .filter(|v: &Value| truthy(v))
        .unwrap_or(json!({}));
    let pick = |k: &str, over: &str| -> String {
        if !over.is_empty() {
            over.trim().to_string()
        } else {
            file.get(k)
                .filter(|v| truthy(v))
                .map(py_str)
                .unwrap_or_default()
                .trim()
                .to_string()
        }
    };
    let m = Managed {
        client_id: pick("client_id", &settings.managed_client_id),
        client_secret: pick("client_secret", &settings.managed_client_secret),
        relay: pick(
            "relay",
            if settings.relay_url == DEFAULT_RELAY {
                ""
            } else {
                &settings.relay_url
            },
        ),
        broker: pick("broker", &settings.broker_url),
        source: String::new(),
    };
    if m.client_id.is_empty()
        || m.relay.is_empty()
        || (m.client_secret.is_empty() && m.broker.is_empty())
    {
        return None;
    }
    Some(m)
}

/// The build's OMuse client, else one saved on this box (`BOX_CLIENT` secret), else `None`.
pub fn managed(settings: &GoogleSettings, store: Option<&Store>) -> Option<Managed> {
    if let Some(mut m) = built_in(settings) {
        m.source = "build".into();
        return Some(m);
    }
    let rec = store.and_then(|s| s.get_secret(BOX_CLIENT).ok().flatten())?;
    let (cid, sec) = (get_str(&rec, "client_id"), get_str(&rec, "client_secret"));
    if cid.is_empty() || sec.is_empty() {
        return None;
    }
    let relay = get_str(&rec, "relay");
    Some(Managed {
        client_id: cid,
        client_secret: sec,
        relay: if relay.is_empty() {
            settings.relay_url.clone()
        } else {
            relay
        },
        broker: String::new(),
        source: "box".into(),
    })
}

/// The token endpoint: the broker's `/token` for a managed client with a broker, else Google's.
pub fn token_url(settings: &GoogleSettings, m: Option<&Managed>) -> String {
    match m {
        Some(m) if !m.broker.is_empty() => format!("{}/token", m.broker.trim_end_matches('/')),
        _ => settings.token_url.clone(),
    }
}

/// Google's consent URL (offline access, forced consent, optional PKCE S256 challenge).
pub fn auth_url(
    settings: &GoogleSettings,
    client_id: &str,
    redirect_uri: &str,
    state: &str,
    code_challenge: &str,
) -> String {
    auth_url_for(
        settings,
        client_id,
        redirect_uri,
        state,
        code_challenge,
        SCOPES,
    )
}

/// [`auth_url`] with an explicit scope string (`scopes_for`).
pub fn auth_url_for(
    settings: &GoogleSettings,
    client_id: &str,
    redirect_uri: &str,
    state: &str,
    code_challenge: &str,
    scope: &str,
) -> String {
    let mut q: Vec<(&str, &str)> = vec![
        ("client_id", client_id),
        ("redirect_uri", redirect_uri),
        ("response_type", "code"),
        ("scope", scope),
        ("access_type", "offline"),
        ("prompt", "consent"),
        ("include_granted_scopes", "true"),
        ("state", state),
    ];
    if !code_challenge.is_empty() {
        q.push(("code_challenge", code_challenge));
        q.push(("code_challenge_method", "S256"));
    }
    format!("{}?{}", settings.auth_url, urlencode(&q))
}

async fn json_or_empty(r: reqwest::Response) -> (u16, Value) {
    let status = r.status().as_u16();
    let body = r.bytes().await.unwrap_or_default();
    let d = if body.is_empty() {
        json!({})
    } else {
        serde_json::from_slice(&body).unwrap_or(json!({}))
    };
    (status, d)
}

/// Swap the one-time authorization code for tokens (`url` = "" → Google's token endpoint).
#[allow(clippy::too_many_arguments)]
pub async fn exchange_code(
    client: &reqwest::Client,
    settings: &GoogleSettings,
    client_id: &str,
    client_secret: &str,
    code: &str,
    redirect_uri: &str,
    code_verifier: &str,
    url: &str,
) -> Result<Value, GCalError> {
    let mut data = vec![
        ("code", code),
        ("client_id", client_id),
        ("redirect_uri", redirect_uri),
        ("grant_type", "authorization_code"),
    ];
    if !client_secret.is_empty() {
        data.push(("client_secret", client_secret));
    }
    if !code_verifier.is_empty() {
        data.push(("code_verifier", code_verifier));
    }
    let url = if url.is_empty() {
        settings.token_url.as_str()
    } else {
        url
    };
    let r = client
        .post(url)
        .timeout(StdDuration::from_secs_f64(TIMEOUT))
        .form(&data)
        .send()
        .await
        .map_err(|e| {
            GCalError(format!(
                "连不上 Google 授权服务器 (cannot reach token endpoint): {}",
                httpx_name(&e)
            ))
        })?;
    let (status, d) = json_or_empty(r).await;
    if status != 200 || !get_truthy(&d, "access_token") {
        let why = [get_str(&d, "error_description"), get_str(&d, "error")]
            .into_iter()
            .find(|s| !s.is_empty())
            .unwrap_or_else(|| status.to_string());
        return Err(GCalError(format!(
            "Google 授权失败 (token exchange failed): {}",
            why
        )));
    }
    if !get_truthy(&d, "refresh_token") {
        return Err(GCalError("Google 没有返回 refresh token：请在 myaccount.google.com/permissions 移除 OMuse 的旧授权后再连接一次".into()));
    }
    Ok(d)
}

fn fmt_err(status: u16, body: &[u8]) -> String {
    let text = String::from_utf8_lossy(body);
    let msg = match serde_json::from_slice::<Value>(body) {
        Ok(j) if j.is_object() => match j.get("error") {
            Some(Value::Object(e)) => e
                .get("message")
                .map(py_str)
                .unwrap_or_else(|| "None".into()),
            Some(Value::Null) | None => "None".into(),
            Some(e) => py_str(e),
        },
        _ => take_chars(&text, 200).to_string(),
    };
    let hint = match status {
        401 => "授权已失效，请在「连接」页重新连接 Google 日历 (re-connect)",
        403 => "没有权限：确认已启用 Google Calendar API、授权时勾选了日历权限 (API disabled or scope missing)",
        404 => "找不到这个日历或日程 (not found)",
        _ => "",
    };
    format!(
        "Google Calendar HTTP {}: {}{}",
        status,
        msg,
        if hint.is_empty() {
            String::new()
        } else {
            format!(" — {}", hint)
        }
    )
}

// ------------------------------------------------------------------ helpers shared with the iCal feed

/// Free windows of at least `dur` between `start` and `end` within the working hours, around `busy` (sorted).
pub(crate) fn free_slot_pairs(
    start: PyDt,
    end: PyDt,
    busy: &[(PyDt, PyDt)],
    dur: Duration,
    day_start: &str,
    day_end: &str,
    z: PyTz,
) -> Result<Vec<(PyDt, PyDt)>, GCalError> {
    let hm = |s: &str| -> Result<NaiveTime, GCalError> {
        let parts: Vec<&str> = s.split(':').collect();
        let bad = || {
            GCalError(format!(
                "看不懂的时间格式 (bad time): {}；请用 09:00 这种格式",
                py_repr(&json!(s))
            ))
        };
        if parts.len() != 2 {
            return Err(bad());
        }
        let h: u32 = parts[0].trim().parse().map_err(|_| bad())?;
        let m: u32 = parts[1].trim().parse().map_err(|_| bad())?;
        NaiveTime::from_hms_opt(h, m, 0).ok_or_else(bad)
    };
    let t1 = hm(if day_start.is_empty() {
        "09:00"
    } else {
        day_start
    })?;
    let t2 = hm(if day_end.is_empty() { "18:00" } else { day_end })?;
    let mut slots = Vec::new();
    let mut day = start.astimezone(z).date();
    let last = end.astimezone(z).date();
    while day <= last && slots.len() < 60 {
        let ws = py_max(PyDt::new(NaiveDateTime::new(day, t1), z), start);
        let we = py_min(PyDt::new(NaiveDateTime::new(day, t2), z), end);
        let mut cur = ws;
        for (b0, b1) in busy {
            if *b1 <= cur || *b0 >= we {
                continue;
            }
            if b0.diff(&cur) >= dur {
                slots.push((cur, *b0));
            }
            cur = py_max(cur, *b1);
        }
        if we.diff(&cur) >= dur {
            slots.push((cur, we));
        }
        day += Duration::days(1);
    }
    Ok(slots)
}

/// The `free_slots` result dict shared by Google and iCal.
pub(crate) fn free_slots_value(
    tz: &str,
    dur: Duration,
    day_start: &str,
    day_end: &str,
    busy_count: usize,
    slots: &[(PyDt, PyDt)],
    z: PyTz,
) -> Value {
    json!({
        "time_zone": tz, "duration_minutes": dur.num_seconds().div_euclid(60), "working_hours": format!("{}-{}", day_start, day_end),
        "busy_count": busy_count,
        "free": slots.iter().map(|(a, b)| format!("{}–{}", a.astimezone(z).strftime("%Y-%m-%d %a %H:%M"), b.astimezone(z).strftime("%H:%M"))).collect::<Vec<_>>(),
    })
}

/// `max(5, int(duration_minutes or 30))` as a duration.
pub(crate) fn slot_duration(duration_minutes: i64) -> Duration {
    Duration::minutes(
        (if duration_minutes == 0 {
            30
        } else {
            duration_minutes
        })
        .max(5),
    )
}

/// `(end - start).days > 31`.
pub(crate) fn range_too_long(start: &PyDt, end: &PyDt) -> bool {
    end.diff(start).num_seconds().div_euclid(86400) > 31
}

/// `max(1, min(int(max_results or 25), 100))`.
pub(crate) fn clamp_results(max_results: i64) -> usize {
    (if max_results == 0 { 25 } else { max_results }).clamp(1, 100) as usize
}

/// `f"{start:%Y-%m-%d %H:%M} → {end:%Y-%m-%d %H:%M}"` in the calendar zone.
pub(crate) fn range_label(start: &PyDt, end: &PyDt, z: PyTz) -> String {
    format!(
        "{} → {}",
        start.astimezone(z).strftime("%Y-%m-%d %H:%M"),
        end.astimezone(z).strftime("%Y-%m-%d %H:%M")
    )
}

/// Called with every refreshed access token: `(access_token, expires_at)`.
pub type TokenHook = Arc<dyn Fn(&str, f64) + Send + Sync>;

/// A Google Calendar client for one connected account.
pub struct GCal {
    client: reqwest::Client,
    settings: GoogleSettings,
    client_id: String,
    client_secret: String,
    refresh_token: String,
    token_url: String,
    tz: String,
    on_token: Option<TokenHook>,
    token: tokio::sync::Mutex<(String, f64)>,
}

impl GCal {
    /// `client_secret` is "" for a managed client behind a broker; `tz` "" → UTC.
    pub fn new(
        client: reqwest::Client,
        settings: &GoogleSettings,
        client_id: &str,
        client_secret: &str,
        refresh_token: &str,
        tz: &str,
    ) -> Self {
        Self {
            client,
            settings: settings.clone(),
            client_id: client_id.to_string(),
            client_secret: client_secret.to_string(),
            refresh_token: refresh_token.to_string(),
            token_url: settings.token_url.clone(),
            tz: if tz.is_empty() {
                "UTC".into()
            } else {
                tz.to_string()
            },
            on_token: None,
            token: tokio::sync::Mutex::new((String::new(), 0.0)),
        }
    }

    /// Start with a cached access token (`cred_calendar_access`).
    pub fn with_access_token(self, access_token: &str, expires_at: f64) -> Self {
        Self {
            token: tokio::sync::Mutex::new((access_token.to_string(), expires_at)),
            ..self
        }
    }

    /// Use another token endpoint (the broker); "" keeps Google's.
    pub fn with_token_url(mut self, url: &str) -> Self {
        if !url.is_empty() {
            self.token_url = url.to_string();
        }
        self
    }

    /// Persist refreshed access tokens.
    pub fn with_on_token(mut self, hook: TokenHook) -> Self {
        self.on_token = Some(hook);
        self
    }

    /// The calendar's time zone name.
    pub fn tz(&self) -> &str {
        &self.tz
    }

    /// A valid access token (refreshed when needed) — for sibling Google APIs such as Drive.
    pub async fn access_token(&self) -> Result<String, GCalError> {
        self.token().await
    }

    /// The HTTP client and Google settings, for sibling Google APIs.
    pub fn parts(&self) -> (&reqwest::Client, &GoogleSettings) {
        (&self.client, &self.settings)
    }

    // ------------------------------------------------------------ auth

    async fn token(&self) -> Result<String, GCalError> {
        let mut guard = self.token.lock().await;
        if !guard.0.is_empty() && now_ts() < guard.1 - 60.0 {
            return Ok(guard.0.clone());
        }
        let mut data = vec![
            ("client_id", self.client_id.as_str()),
            ("refresh_token", self.refresh_token.as_str()),
            ("grant_type", "refresh_token"),
        ];
        if !self.client_secret.is_empty() {
            data.push(("client_secret", self.client_secret.as_str()));
        }
        let r = self
            .client
            .post(&self.token_url)
            .timeout(StdDuration::from_secs_f64(TIMEOUT))
            .form(&data)
            .send()
            .await
            .map_err(|e| GCalError(format!("{}: {}", httpx_name(&e), e)))?;
        let (status, d) = json_or_empty(r).await;
        if status != 200 || !get_truthy(&d, "access_token") {
            let err = get_str(&d, "error");
            if err == "invalid_grant" {
                return Err(GCalError(
                    "Google 授权已过期或被撤销 (invalid_grant)。请在「连接」页重新连接 Google 日历。如果每 7 天就过期一次：把 Google Cloud 里 OAuth 同意屏幕的发布状态改成「正式版 In production」。".into(),
                ));
            }
            let why = [get_str(&d, "error_description"), err]
                .into_iter()
                .find(|s| !s.is_empty())
                .unwrap_or_else(|| status.to_string());
            return Err(GCalError(format!(
                "刷新 Google 令牌失败 (token refresh failed): {}",
                why
            )));
        }
        let tok = get_str(&d, "access_token");
        let exp = now_ts()
            + d.get("expires_in")
                .and_then(py_int)
                .filter(|x| *x != 0)
                .unwrap_or(3600) as f64;
        *guard = (tok.clone(), exp);
        if let Some(h) = &self.on_token {
            h(&tok, exp);
        }
        Ok(tok)
    }

    async fn req(
        &self,
        method: reqwest::Method,
        path: &str,
        params: &[(&str, String)],
        body: Option<&Value>,
    ) -> Result<Value, GCalError> {
        let tok = self.token().await?;
        let mut rb = self
            .client
            .request(method, format!("{}{}", self.settings.gcal_api, path))
            .timeout(StdDuration::from_secs_f64(TIMEOUT))
            .header("Authorization", format!("Bearer {}", tok));
        if !params.is_empty() {
            rb = rb.query(params);
        }
        if let Some(b) = body {
            rb = rb.json(b);
        }
        let r = rb
            .send()
            .await
            .map_err(|e| GCalError(format!("{}: {}", httpx_name(&e), e)))?;
        let status = r.status().as_u16();
        if status == 204 {
            return Ok(json!({}));
        }
        let bytes = r.bytes().await.unwrap_or_default();
        if status >= 400 {
            return Err(GCalError(fmt_err(status, &bytes)));
        }
        if bytes.is_empty() {
            return Ok(json!({}));
        }
        serde_json::from_slice(&bytes).map_err(|e| GCalError(format!("JSONDecodeError: {}", e)))
    }

    /// The signed-in Google account (`{}` when the userinfo call fails).
    pub async fn userinfo(&self) -> Result<Value, GCalError> {
        let tok = self.token().await?;
        let r = self
            .client
            .get(&self.settings.userinfo_url)
            .timeout(StdDuration::from_secs_f64(TIMEOUT))
            .header("Authorization", format!("Bearer {}", tok))
            .send()
            .await
            .map_err(|e| GCalError(format!("{}: {}", httpx_name(&e), e)))?;
        if r.status().as_u16() != 200 {
            return Ok(json!({}));
        }
        Ok(r.json().await.unwrap_or(json!({})))
    }

    /// The account's calendar time zone (falls back to the configured one).
    pub async fn settings_tz(&self) -> String {
        match self
            .req(
                reqwest::Method::GET,
                "/users/me/settings/timezone",
                &[],
                None,
            )
            .await
        {
            Ok(d) => {
                let v = get_str(&d, "value");
                if v.is_empty() {
                    self.tz.clone()
                } else {
                    v
                }
            }
            Err(_) => self.tz.clone(),
        }
    }

    /// The calendar list: `[{id, name, primary, access}]`.
    pub async fn calendars(&self) -> Result<Vec<Value>, GCalError> {
        let d = self
            .req(
                reqwest::Method::GET,
                "/users/me/calendarList",
                &[("maxResults", "50".into())],
                None,
            )
            .await?;
        Ok(arr(d.get("items"))
            .iter()
            .map(|c| {
                let mut name = get_str(c, "summaryOverride");
                if name.is_empty() {
                    name = c.get("summary").map(py_str).unwrap_or_default();
                }
                json!({"id": c.get("id").cloned().unwrap_or(Value::Null), "name": name, "primary": get_truthy(c, "primary"),
                       "access": c.get("accessRole").cloned().unwrap_or(json!(""))})
            })
            .collect())
    }

    // ------------------------------------------------------------ time helpers

    fn zone(&self, tz: Option<&str>) -> PyTz {
        zone_of(tz.unwrap_or(&self.tz))
    }

    /// `'2026-10-03 19:00'`, `'2026-10-03T19:00'`, ISO with offset/Z, or a date → aware datetime in the calendar tz.
    pub fn parse_time(&self, value: &Value, tz: Option<&str>) -> Result<PyDt, GCalError> {
        parse_time_in(value, self.zone(tz))
    }

    fn when(&self, ev_time: &Value) -> Value {
        if let Some(d) = ev_time.get("date") {
            return d.clone();
        }
        let s = match ev_time.get("dateTime") {
            Some(v) => py_str(v),
            None => return json!(""),
        };
        let iso = s.replace('Z', "+00:00");
        let naive_tz = PyDt::fromisoformat(&iso, PyTz::UTC)
            .map(|d| PyDt::local_tz_for(&d.local()))
            .unwrap_or(PyTz::UTC);
        match PyDt::fromisoformat(&iso, naive_tz) {
            Some(d) => json!(d.astimezone(self.zone(None)).strftime("%Y-%m-%d %H:%M")),
            None => ev_time.get("dateTime").cloned().unwrap_or(json!("")),
        }
    }

    fn event(&self, e: &Value) -> Value {
        let mut conf = String::new();
        for ep in arr(e.get("conferenceData").and_then(|c| c.get("entryPoints"))) {
            if get_str(ep, "entryPointType") == "video" {
                conf = get_str(ep, "uri");
            }
        }
        let start = obj_or_empty(e.get("start"));
        let end = obj_or_empty(e.get("end"));
        let attendees: Vec<String> = arr(e.get("attendees"))
            .iter()
            .take(30)
            .map(|a| {
                let st = get_str(a, "responseStatus");
                format!(
                    "{}{}",
                    get_str(a, "email"),
                    if st.is_empty() {
                        String::new()
                    } else {
                        format!(" ({})", st)
                    }
                )
            })
            .collect();
        let meeting = if conf.is_empty() {
            e.get("hangoutLink").cloned().unwrap_or(json!(""))
        } else {
            json!(conf)
        };
        json!({
            "id": e.get("id").cloned().unwrap_or(Value::Null),
            "title": e.get("summary").cloned().unwrap_or(json!("(无标题 no title)")),
            "start": self.when(&start), "end": self.when(&end), "all_day": start.get("date").is_some(),
            "location": e.get("location").cloned().unwrap_or(json!("")), "status": e.get("status").cloned().unwrap_or(json!("")),
            "organizer": e.get("organizer").filter(|o| truthy(o)).map(|o| o.get("email").cloned().unwrap_or(json!(""))).unwrap_or(json!("")),
            "attendees": attendees,
            "meeting_link": meeting, "description": take_chars(&str_or_empty(&e["description"]), 1500),
            "link": e.get("htmlLink").cloned().unwrap_or(json!("")),
        })
    }

    // ------------------------------------------------------------ read

    /// Events between `time_min` (default now) and `time_max` (default +7 days), cancelled ones dropped.
    pub async fn list_events(
        &self,
        time_min: &Value,
        time_max: &Value,
        query: &str,
        calendar_id: &str,
        max_results: i64,
    ) -> Result<Value, GCalError> {
        let z = self.zone(None);
        let start = if truthy(time_min) {
            self.parse_time(time_min, None)?
        } else {
            PyDt::now(z)
        };
        let mut end = if truthy(time_max) {
            self.parse_time(time_max, None)?
        } else {
            start.add(Duration::days(7))
        };
        if end <= start {
            end = start.add(Duration::days(1));
        }
        let mut params = vec![
            ("timeMin", start.isoformat()),
            ("timeMax", end.isoformat()),
            ("singleEvents", "true".to_string()),
            ("orderBy", "startTime".to_string()),
            ("maxResults", clamp_results(max_results).to_string()),
            ("timeZone", self.tz.clone()),
        ];
        if !query.is_empty() {
            params.push(("q", query.to_string()));
        }
        let d = self
            .req(
                reqwest::Method::GET,
                &format!("/calendars/{}/events", cid(&json!(calendar_id))),
                &params,
                None,
            )
            .await?;
        Ok(json!({
            "calendar": if calendar_id.is_empty() { "primary" } else { calendar_id }, "time_zone": self.tz,
            "range": range_label(&start, &end, z),
            "events": arr(d.get("items")).iter().filter(|e| e.get("status").map(py_str).as_deref() != Some("cancelled")).map(|e| self.event(e)).collect::<Vec<_>>(),
        }))
    }

    /// Free windows from freeBusy within working hours.
    pub async fn free_slots(
        &self,
        time_min: &Value,
        time_max: &Value,
        duration_minutes: i64,
        day_start: &str,
        day_end: &str,
        calendar_id: &str,
    ) -> Result<Value, GCalError> {
        let z = self.zone(None);
        let (start, end) = (
            self.parse_time(time_min, None)?,
            self.parse_time(time_max, None)?,
        );
        if range_too_long(&start, &end) {
            return Err(GCalError(
                "时间范围太长，最多 31 天 (range too long)".into(),
            ));
        }
        let cal_id = if calendar_id.is_empty() {
            "primary"
        } else {
            calendar_id
        };
        let d = self
            .req(
                reqwest::Method::POST,
                "/freeBusy",
                &[],
                Some(&json!({"timeMin": start.isoformat(), "timeMax": end.isoformat(), "timeZone": self.tz, "items": [{"id": cal_id}]})),
            )
            .await?;
        let cal = obj_or_empty(d.get("calendars").and_then(|c| c.get(cal_id)));
        if let Some(errs) = cal.get("errors").filter(|e| truthy(e)) {
            return Err(GCalError(format!(
                "无法读取忙闲 (freeBusy error): {}",
                py_str(errs)
            )));
        }
        let mut busy: Vec<(PyDt, PyDt)> = Vec::new();
        for b in arr(cal.get("busy")) {
            let p = |k: &str| {
                PyDt::fromisoformat(&get_str(b, k).replace('Z', "+00:00"), PyTz::UTC)
                    .ok_or_else(|| bad_time(&b[k]))
            };
            busy.push((p("start")?, p("end")?));
        }
        busy.sort();
        let dur = slot_duration(duration_minutes);
        let slots = free_slot_pairs(start, end, &busy, dur, day_start, day_end, z)?;
        Ok(free_slots_value(
            &self.tz,
            dur,
            day_start,
            day_end,
            busy.len(),
            &slots,
            z,
        ))
    }

    /// One event.
    pub async fn get_event(&self, event_id: &str, calendar_id: &str) -> Result<Value, GCalError> {
        let e = self
            .req(
                reqwest::Method::GET,
                &format!(
                    "/calendars/{}/events/{}",
                    cid(&json!(calendar_id)),
                    eid(event_id)?
                ),
                &[],
                None,
            )
            .await?;
        Ok(self.event(&e))
    }

    // ------------------------------------------------------------ write

    fn times(
        &self,
        start: &Value,
        end: &Value,
        all_day: bool,
        tz: Option<&str>,
    ) -> Result<(Value, Value), GCalError> {
        if all_day {
            let d0 = self.parse_time(start, tz)?.date();
            let mut d1 = if truthy(end) {
                self.parse_time(end, tz)?.date()
            } else {
                d0
            };
            if d1 <= d0 {
                d1 = d0 + Duration::days(1);
            }
            return Ok((
                json!({"date": d0.format("%Y-%m-%d").to_string()}),
                json!({"date": d1.format("%Y-%m-%d").to_string()}),
            ));
        }
        let s = self.parse_time(start, tz)?;
        let e = if truthy(end) {
            self.parse_time(end, tz)?
        } else {
            s.add(Duration::hours(1))
        };
        if e <= s {
            return Err(GCalError(
                "结束时间必须晚于开始时间 (end must be after start)".into(),
            ));
        }
        let zone = tz.unwrap_or(&self.tz);
        Ok((
            json!({"dateTime": s.isoformat(), "timeZone": zone}),
            json!({"dateTime": e.isoformat(), "timeZone": zone}),
        ))
    }

    /// Create an event from the `calendar_create_event` args (invites sent when attendees are given).
    pub async fn create_event(&self, a: &Value) -> Result<Value, GCalError> {
        let mut title = str_or_empty(&a["title"]);
        if title.is_empty() {
            title = str_or_empty(&a["summary"]);
        }
        let title = title.trim().to_string();
        if title.is_empty() {
            return Err(GCalError("缺少标题 (title required)".into()));
        }
        let tz_arg = a.get("time_zone").filter(|t| truthy(t)).map(py_str);
        let (st, en) = self.times(
            &a["start"],
            &a["end"],
            get_truthy(a, "all_day"),
            tz_arg.as_deref(),
        )?;
        let mut body = json!({"summary": take_chars(&title, 300), "start": st, "end": en});
        if get_truthy(a, "location") {
            body["location"] = json!(take_chars(&py_str(&a["location"]), 500));
        }
        if get_truthy(a, "description") {
            body["description"] = json!(take_chars(&py_str(&a["description"]), 8000));
        }
        let att = attendees(a.get("attendees"));
        if !att.is_empty() {
            body["attendees"] = json!(att.iter().map(|x| json!({"email": x})).collect::<Vec<_>>());
        }
        if let Some(rm) = a
            .get("reminder_minutes")
            .filter(|v| !v.is_null() && v.as_str() != Some(""))
        {
            let minutes = py_int(rm).ok_or_else(|| {
                GCalError(format!(
                    "invalid literal for int() with base 10: {}",
                    py_repr(rm)
                ))
            })?;
            body["reminders"] = json!({"useDefault": false, "overrides": [{"method": "popup", "minutes": minutes}]});
        }
        let e = self
            .req(
                reqwest::Method::POST,
                &format!("/calendars/{}/events", cid(&a["calendar_id"])),
                &[(
                    "sendUpdates",
                    if att.is_empty() {
                        "none".into()
                    } else {
                        "all".into()
                    },
                )],
                Some(&body),
            )
            .await?;
        Ok(json!({"created": true, "event": self.event(&e), "invites_sent_to": att}))
    }

    /// Patch an event; a new `start` without `end` keeps the duration.
    pub async fn update_event(&self, a: &Value) -> Result<Value, GCalError> {
        let eid_s = str_or_empty(&a["event_id"]);
        if eid_s.is_empty() {
            return Err(GCalError(
                "缺少 event_id (use calendar_list_events to find it)".into(),
            ));
        }
        let mut body = Map::new();
        if get_truthy(a, "title") {
            body.insert(
                "summary".into(),
                json!(take_chars(&py_str(&a["title"]), 300)),
            );
        }
        let tz_arg = a.get("time_zone").filter(|t| truthy(t)).map(py_str);
        if get_truthy(a, "start") || get_truthy(a, "end") {
            let cur = self
                .req(
                    reqwest::Method::GET,
                    &format!(
                        "/calendars/{}/events/{}",
                        cid(&a["calendar_id"]),
                        eid(&eid_s)?
                    ),
                    &[],
                    None,
                )
                .await?;
            let cs = obj_or_empty(cur.get("start"));
            let ce = obj_or_empty(cur.get("end"));
            let all_day = cs.get("date").is_some();
            let pick = |t: &Value| -> Value {
                match t.get("dateTime").filter(|v| truthy(v)) {
                    Some(v) => v.clone(),
                    None => t.get("date").cloned().unwrap_or(Value::Null),
                }
            };
            let start = if get_truthy(a, "start") {
                a["start"].clone()
            } else {
                pick(&cs)
            };
            let end = if get_truthy(a, "end") {
                a["end"].clone()
            } else {
                let c0 = self.parse_time(&pick(&cs), None)?;
                let c1 = self.parse_time(&pick(&ce), None)?;
                json!(self.parse_time(&start, None)?.add(c1.diff(&c0)).isoformat())
            };
            let (st, en) = self.times(&start, &end, all_day, tz_arg.as_deref())?;
            body.insert("start".into(), st);
            body.insert("end".into(), en);
        }
        for k in ["location", "description"] {
            if let Some(v) = a.get(k).filter(|v| !v.is_null() && v.as_str() != Some("")) {
                body.insert(k.into(), json!(take_chars(&py_str(v), 8000)));
            }
        }
        if let Some(att) = a.get("attendees").filter(|v| !v.is_null()) {
            let att = attendees(Some(att));
            body.insert(
                "attendees".into(),
                json!(att.iter().map(|x| json!({"email": x})).collect::<Vec<_>>()),
            );
        }
        if body.is_empty() {
            return Err(GCalError("没有要修改的内容 (nothing to change)".into()));
        }
        let notify = a.get("notify_attendees").map(truthy).unwrap_or(true);
        let e = self
            .req(
                reqwest::Method::PATCH,
                &format!(
                    "/calendars/{}/events/{}",
                    cid(&a["calendar_id"]),
                    eid(&eid_s)?
                ),
                &[(
                    "sendUpdates",
                    if notify { "all".into() } else { "none".into() },
                )],
                Some(&Value::Object(body)),
            )
            .await?;
        Ok(json!({"updated": true, "event": self.event(&e)}))
    }

    /// Delete an event.
    pub async fn delete_event(&self, a: &Value) -> Result<Value, GCalError> {
        let eid_s = str_or_empty(&a["event_id"]);
        if eid_s.is_empty() {
            return Err(GCalError("缺少 event_id".into()));
        }
        let notify = a.get("notify_attendees").map(truthy).unwrap_or(true);
        self.req(
            reqwest::Method::DELETE,
            &format!(
                "/calendars/{}/events/{}",
                cid(&a["calendar_id"]),
                eid(&eid_s)?
            ),
            &[(
                "sendUpdates",
                if notify { "all".into() } else { "none".into() },
            )],
            None,
        )
        .await?;
        Ok(json!({"deleted": true, "event_id": eid_s}))
    }
}

/// Attendee e-mails from a list or a `,;`/space separated string: lower-cased, validated, deduplicated, ≤ 50.
pub fn attendees(v: Option<&Value>) -> Vec<String> {
    static SPLIT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[,;\s]+").unwrap());
    static EMAIL: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"^[^@\s]+@[^@\s]+\.[^@\s]+$").unwrap());
    let Some(v) = v.filter(|v| truthy(v)) else {
        return vec![];
    };
    let items: Vec<String> = match v.as_array() {
        Some(a) => a.iter().map(py_str).collect(),
        None => SPLIT.split(&py_str(v)).map(String::from).collect(),
    };
    let mut out: Vec<String> = Vec::new();
    for x in items {
        let x = x
            .trim()
            .trim_matches(|c| c == '<' || c == '>')
            .to_lowercase();
        if EMAIL.is_match(&x) && !out.contains(&x) {
            out.push(x);
        }
    }
    out.truncate(50);
    out
}

/// `quote(str(calendar_id or "primary"), safe="")`.
pub fn cid(calendar_id: &Value) -> String {
    let s = str_or_empty(calendar_id);
    quote(if s.is_empty() { "primary" } else { &s }, "")
}

/// A validated event id (`[A-Za-z0-9_-]{3,1024}`).
pub fn eid(event_id: &str) -> Result<String, GCalError> {
    static RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[A-Za-z0-9_\-]{3,1024}$").unwrap());
    let e = event_id.trim();
    if !RE.is_match(e) {
        return Err(GCalError(format!("无效的 event_id: {}", take_chars(e, 60))));
    }
    Ok(e.to_string())
}

/// Today's date in `tz` (`YYYY-MM-DD`; the system date when the zone is unknown).
pub fn today_str(tz: &str) -> String {
    match PyTz::zone(tz) {
        Some(z) => PyDt::now(z).strftime("%Y-%m-%d"),
        None => chrono::Local::now().format("%Y-%m-%d").to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::extract::{Path as AxPath, Query, State};
    use axum::routing::{get, post};
    use axum::{Json, Router};
    use std::collections::HashMap;
    use std::sync::Mutex;

    fn settings(base: &str) -> GoogleSettings {
        GoogleSettings {
            auth_url: format!("{}/g/auth", base),
            token_url: format!("{}/g/token", base),
            gcal_api: format!("{}/cal", base),
            gdrive_api: format!("{}/drive", base),
            gdrive_upload_api: format!("{}/drive-upload", base),
            userinfo_url: format!("{}/g/userinfo", base),
            relay_url: DEFAULT_RELAY.into(),
            relay_override: String::new(),
            managed_file: PathBuf::from("/nonexistent/google_managed.json"),
            managed_client_id: String::new(),
            managed_client_secret: String::new(),
            broker_url: String::new(),
        }
    }

    #[test]
    fn add_links_match_python() {
        let r = add_to_calendar_links(
            &json!({"title": "[测试] A", "start": "2026-10-08 15:00", "end": "2026-10-08 15:30", "location": "X", "description": "d"}),
            "Asia/Singapore",
        )
        .unwrap();
        let g = r["google"].as_str().unwrap();
        assert!(g.contains("calendar.google.com/calendar/render") && g.contains("action=TEMPLATE"));
        assert!(g
            .replace("%2F", "/")
            .contains("20261008T070000Z/20261008T073000Z"));
        assert!(g.contains("%5B"));
        assert!(r["outlook"].as_str().unwrap().contains("outlook.live.com"));
        assert_eq!(r["start"], json!("2026-10-08 15:00"));
        let rd = add_to_calendar_links(
            &json!({"title": "休假", "start": "2026-12-25", "all_day": true}),
            "UTC",
        )
        .unwrap();
        assert!(rd["google"]
            .as_str()
            .unwrap()
            .replace("%2F", "/")
            .contains("dates=20261225/20261226"));
        assert!(add_to_calendar_links(&json!({"title": "x", "start": "soon"}), "UTC").is_err());
    }

    #[test]
    fn auth_url_and_helpers() {
        let s = settings("https://accounts.test");
        let u = auth_url(
            &s,
            "1-a.apps.googleusercontent.com",
            "https://box/sentinel/api/connections/calendar/callback",
            "st",
            "",
        );
        assert!(u.starts_with("https://accounts.test/g/auth?client_id=1-a.apps.googleusercontent.com&redirect_uri=https%3A%2F%2Fbox%2Fsentinel"));
        assert!(
            u.contains("access_type=offline")
                && u.contains("calendar.events")
                && u.contains("state=st")
        );
        assert!(!u.contains("code_challenge"));
        let u2 = auth_url(&s, "c", "r", "st", "ch");
        assert!(u2.ends_with("&state=st&code_challenge=ch&code_challenge_method=S256"));
        assert_eq!(
            attendees(Some(&json!(
                "eva@example.com, bad-address; <Bob@X.io> eva@example.com"
            ))),
            ["eva@example.com", "bob@x.io"]
        );
        assert_eq!(attendees(Some(&json!(["a@b.co", 5]))), ["a@b.co"]);
        assert!(attendees(None).is_empty());
        assert_eq!(cid(&Value::Null), "primary");
        assert_eq!(
            cid(&json!("a@group.calendar.google.com")),
            "a%40group.calendar.google.com"
        );
        assert!(eid("../../x").is_err() && eid("ev1").is_ok());
        assert_eq!(
            token_url(
                &s,
                Some(&Managed {
                    client_id: "c".into(),
                    client_secret: "".into(),
                    relay: "r".into(),
                    broker: "https://b/".into(),
                    source: "build".into()
                })
            ),
            "https://b/token"
        );
        assert_eq!(token_url(&s, None), "https://accounts.test/g/token");
        assert_eq!(today_str("Asia/Singapore").len(), 10);
    }

    #[test]
    fn managed_from_file_and_overrides() {
        let dir =
            std::env::temp_dir().join(format!("omuse-gcal-{}", omuse_common::util::uuid_hex()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut s = settings("https://x");
        s.managed_file = dir.join("google_managed.json");
        assert!(managed(&s, None).is_none());
        std::fs::write(&s.managed_file, r#"{"client_id": "omuse-test.apps.googleusercontent.com", "client_secret": "GOCSPX-test", "relay": "https://relay/cb"}"#).unwrap();
        let m = managed(&s, None).unwrap();
        assert_eq!(
            (m.source.as_str(), m.relay.as_str(), m.broker.as_str()),
            ("build", "https://relay/cb", "")
        );
        std::fs::write(
            &s.managed_file,
            r#"{"client_id": "c", "relay": "https://relay/cb", "broker": "https://b/broker"}"#,
        )
        .unwrap();
        assert_eq!(managed(&s, None).unwrap().broker, "https://b/broker");
        std::fs::write(
            &s.managed_file,
            r#"{"client_id": "c", "relay": "https://relay/cb"}"#,
        )
        .unwrap();
        assert!(managed(&s, None).is_none(), "secret or broker required");
        s.managed_client_secret = "sec".into();
        assert!(managed(&s, None).is_some());
        std::fs::write(&s.managed_file, "{}").unwrap();
        s.managed_client_secret.clear();
        let store = Store::new(&dir).unwrap();
        assert!(managed(&s, Some(&store)).is_none());
        store.put_secret("calendar", &json!({"client_id": "123-abc.apps.googleusercontent.com", "client_secret": "GOCSPX-x"}), Some(BOX_CLIENT)).unwrap();
        let m = managed(&s, Some(&store)).unwrap();
        assert_eq!(
            (m.source.as_str(), m.relay.as_str()),
            ("box", DEFAULT_RELAY)
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ------------------------------------------------------------ fake Google (tests/test_027.py FakeGoogle)
    /// (method, path, query, json body)
    type Call = (String, String, HashMap<String, String>, Value);

    #[derive(Default)]
    struct Fake {
        calls: Mutex<Vec<Call>>,
        tokens: Mutex<u32>,
    }

    fn events() -> Vec<Value> {
        vec![
            json!({"id": "ev1", "summary": "Standup", "start": {"dateTime": "2026-10-05T09:30:00+08:00"},
                   "end": {"dateTime": "2026-10-05T10:00:00+08:00"}, "attendees": [{"email": "a@x.com", "responseStatus": "accepted"}]}),
            json!({"id": "ev2", "summary": "Holiday", "start": {"date": "2026-10-06"}, "end": {"date": "2026-10-07"}}),
        ]
    }

    async fn serve() -> (String, Arc<Fake>) {
        let fake = Arc::new(Fake::default());
        async fn record(f: &Fake, m: &str, p: &str, q: HashMap<String, String>, b: Value) {
            f.calls.lock().unwrap().push((m.into(), p.into(), q, b));
        }
        let app = Router::new()
            .route(
                "/g/token",
                post(|State(f): State<Arc<Fake>>| async move {
                    *f.tokens.lock().unwrap() += 1;
                    Json(json!({"access_token": "at-1", "expires_in": 3600}))
                }),
            )
            .route(
                "/cal/freeBusy",
                post(|State(f): State<Arc<Fake>>, Json(b): Json<Value>| async move {
                    record(&f, "POST", "/freeBusy", HashMap::new(), b).await;
                    Json(json!({"calendars": {"primary": {"busy": [
                        {"start": "2026-10-05T01:30:00Z", "end": "2026-10-05T02:00:00Z"},
                        {"start": "2026-10-05T04:00:00Z", "end": "2026-10-05T06:00:00Z"}]}}}))
                }),
            )
            .route(
                "/cal/calendars/{cid}/events",
                get(|State(f): State<Arc<Fake>>, Query(q): Query<HashMap<String, String>>| async move {
                    record(&f, "GET", "/events", q, Value::Null).await;
                    Json(json!({"items": events()}))
                })
                .post(|State(f): State<Arc<Fake>>, Query(q): Query<HashMap<String, String>>, Json(b): Json<Value>| async move {
                    record(&f, "POST", "/events", q, b.clone()).await;
                    let mut e = json!({"id": "new1", "htmlLink": "https://calendar.google.com/x"});
                    for (k, v) in b.as_object().unwrap() {
                        e[k] = v.clone();
                    }
                    Json(e)
                }),
            )
            .route(
                "/cal/calendars/{cid}/events/{eid}",
                get(|State(f): State<Arc<Fake>>, AxPath((_, e)): AxPath<(String, String)>| async move {
                    record(&f, "GET", &format!("/events/{}", e), HashMap::new(), Value::Null).await;
                    Json(events()[0].clone())
                })
                .patch(|State(f): State<Arc<Fake>>, Query(q): Query<HashMap<String, String>>, Json(b): Json<Value>| async move {
                    record(&f, "PATCH", "/events/x", q, b.clone()).await;
                    let mut e = events()[0].clone();
                    for (k, v) in b.as_object().unwrap() {
                        e[k] = v.clone();
                    }
                    Json(e)
                })
                .delete(|State(f): State<Arc<Fake>>, Query(q): Query<HashMap<String, String>>| async move {
                    record(&f, "DELETE", "/events/x", q, Value::Null).await;
                    axum::http::StatusCode::NO_CONTENT
                }),
            )
            .with_state(fake.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (format!("http://{}", addr), fake)
    }

    async fn cal() -> (GCal, Arc<Fake>) {
        let (base, fake) = serve().await;
        (
            GCal::new(
                reqwest::Client::new(),
                &settings(&base),
                "cid",
                "sec",
                "rt",
                "Asia/Singapore",
            ),
            fake,
        )
    }

    #[tokio::test]
    async fn gcal_list_events() {
        let (cal, fake) = cal().await;
        let r = cal
            .list_events(
                &json!("2026-10-05"),
                &json!("2026-10-08"),
                "",
                "primary",
                25,
            )
            .await
            .unwrap();
        let titles: Vec<&str> = r["events"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["title"].as_str().unwrap())
            .collect();
        assert_eq!(titles, ["Standup", "Holiday"]);
        assert_eq!(r["events"][0]["start"], json!("2026-10-05 09:30"));
        assert_eq!(r["events"][1]["all_day"], json!(true));
        assert_eq!(r["events"][0]["attendees"], json!(["a@x.com (accepted)"]));
        assert_eq!(r["range"], json!("2026-10-05 00:00 → 2026-10-08 00:00"));
        let calls = fake.calls.lock().unwrap();
        let (_, _, params, _) = calls.last().unwrap();
        assert_eq!(params["singleEvents"], "true");
        assert!(params["timeMin"].starts_with("2026-10-05T00:00:00+08:00"));
        assert_eq!(params["maxResults"], "25");
    }

    #[tokio::test]
    async fn gcal_free_slots() {
        let (cal, _) = cal().await;
        let r = cal
            .free_slots(
                &json!("2026-10-05 09:00"),
                &json!("2026-10-05 18:00"),
                60,
                "09:00",
                "18:00",
                "primary",
            )
            .await
            .unwrap();
        // busy 09:30–10:00 and 12:00–14:00 (SGT) → free 10:00–12:00 and 14:00–18:00 (09:00–09:30 is too short)
        assert_eq!(
            r["free"],
            json!(["2026-10-05 Mon 10:00–12:00", "2026-10-05 Mon 14:00–18:00"])
        );
        assert_eq!(r["busy_count"], json!(2));
        assert_eq!(r["duration_minutes"], json!(60));
        assert_eq!(r["working_hours"], json!("09:00-18:00"));
        let e = cal
            .free_slots(
                &json!("2026-10-05"),
                &json!("2026-12-05"),
                60,
                "09:00",
                "18:00",
                "primary",
            )
            .await
            .unwrap_err();
        assert!(e.0.contains("range too long"));
    }

    #[tokio::test]
    async fn gcal_create_with_invites() {
        let (cal, fake) = cal().await;
        let r = cal
            .create_event(&json!({"title": "🍽 Kuriya", "start": "2026-10-03 19:00", "location": "Orchard",
                                  "attendees": "eva@example.com, bad-address", "reminder_minutes": 120}))
            .await
            .unwrap();
        assert_eq!(r["created"], json!(true));
        assert_eq!(r["invites_sent_to"], json!(["eva@example.com"]));
        let calls = fake.calls.lock().unwrap();
        let (_, _, params, b) = calls.last().unwrap();
        assert_eq!(params["sendUpdates"], "all");
        assert_eq!(b["start"]["dateTime"], json!("2026-10-03T19:00:00+08:00"));
        assert_eq!(b["end"]["dateTime"], json!("2026-10-03T20:00:00+08:00"));
        assert_eq!(b["reminders"]["overrides"][0]["minutes"], json!(120));
        assert_eq!(b["attendees"], json!([{"email": "eva@example.com"}]));
        assert_eq!(
            *fake.tokens.lock().unwrap(),
            1,
            "access token reused (no refresh per call)"
        );
    }

    #[tokio::test]
    async fn gcal_update_keeps_duration_and_delete() {
        let (cal, fake) = cal().await;
        let r = cal
            .update_event(&json!({"event_id": "ev1", "start": "2026-10-05 11:00"}))
            .await
            .unwrap();
        assert_eq!(r["updated"], json!(true));
        {
            let calls = fake.calls.lock().unwrap();
            let (_, _, params, body) = calls.last().unwrap();
            assert_eq!(
                body["start"]["dateTime"],
                json!("2026-10-05T11:00:00+08:00")
            );
            assert_eq!(body["end"]["dateTime"], json!("2026-10-05T11:30:00+08:00"));
            assert_eq!(params["sendUpdates"], "all");
        }
        assert_eq!(
            cal.delete_event(&json!({"event_id": "ev1", "notify_attendees": false}))
                .await
                .unwrap()["deleted"],
            json!(true)
        );
        assert_eq!(
            fake.calls.lock().unwrap().last().unwrap().2["sendUpdates"],
            "none"
        );
        assert!(cal
            .delete_event(&json!({"event_id": "../../x"}))
            .await
            .is_err());
        assert!(cal
            .update_event(&json!({"event_id": "ev1"}))
            .await
            .unwrap_err()
            .0
            .contains("nothing to change"));
    }

    #[tokio::test]
    async fn gcal_bad_time() {
        let (cal, _) = cal().await;
        let e = cal
            .create_event(&json!({"title": "x", "start": "next friday"}))
            .await
            .unwrap_err();
        assert_eq!(
            e.0,
            "看不懂的时间格式 (bad time): 'next friday'；请用 2026-10-03 19:00 这种格式"
        );
        let e = cal
            .create_event(
                &json!({"title": "x", "start": "2026-10-03 19:00", "end": "2026-10-03 18:00"}),
            )
            .await
            .unwrap_err();
        assert_eq!(e.0, "结束时间必须晚于开始时间 (end must be after start)");
        assert!(cal
            .create_event(&json!({"start": "2026-10-03 19:00"}))
            .await
            .unwrap_err()
            .0
            .contains("title required"));
        let all = cal
            .create_event(&json!({"title": "x", "start": "2026-10-03", "all_day": true}))
            .await
            .unwrap();
        assert_eq!(all["event"]["all_day"], json!(true));
    }

    #[test]
    fn fmt_err_variants() {
        assert_eq!(fmt_err(401, br#"{"error": {"message": "bad token"}}"#), "Google Calendar HTTP 401: bad token — 授权已失效，请在「连接」页重新连接 Google 日历 (re-connect)");
        assert_eq!(
            fmt_err(500, b"<html>oops"),
            "Google Calendar HTTP 500: <html>oops"
        );
        assert_eq!(
            fmt_err(404, br#"{"error": "gone"}"#),
            "Google Calendar HTTP 404: gone — 找不到这个日历或日程 (not found)"
        );
    }
}
