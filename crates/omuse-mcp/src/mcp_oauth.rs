//! OAuth 2.1 for remote MCP servers (MCP authorization spec 2025-06-18), e.g. DialMCP (`app/sentinel/mcp_oauth.py`).
//!
//! Flow (all on Sentinel; tokens never reach the runtime or the LLM):
//! 1. [`discover`]: ask the MCP server (401 + WWW-Authenticate `resource_metadata`, or the well-known Protected Resource
//!    Metadata URL) which authorization server protects it, then read that server's metadata (RFC 8414 / OIDC).
//! 2. Dynamic Client Registration (RFC 7591) as a public client ("none" auth) with OMuse's own callback URL.
//! 3. Authorization Code + PKCE (S256) in the user's browser, with `resource` = the MCP server (RFC 8707) so the
//!    token is only good for that server. `state` is single-use and expires after 15 minutes.
//! 4. The callback exchanges the code; access + refresh tokens go into the vault under a handle. [`auth_headers`]
//!    refreshes them shortly before they expire (or when the server answers 401).
//!
//! Every URL taken from metadata goes through `check_server_url` (https only on the public internet).
//!
//! Errors: functions without a store return `Result<_, OAuthError>`; the store-backed ones ([`refresh`],
//! [`auth_headers`], [`save`]) return `anyhow::Result`, in which an `OAuthError` (the Python exception class the
//! callers branch on) is recoverable with `e.downcast_ref::<OAuthError>()`; anything else is a store failure.

use std::collections::HashMap;
use std::sync::{Arc, LazyLock};
use std::time::Duration;

use anyhow::Result;
use base64::Engine;
use omuse_common::guard::urlparse;
use omuse_common::util::{get_str, now_ts, py_str, take_chars, token_urlsafe, truthy, VERSION};
use omuse_sentinel_core::store::Store;
use parking_lot::Mutex;
use regex::Regex;
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};

use crate::mcp_client::{check_server_url, Headers, MCPError};

/// A pending browser login is valid this long (seconds).
pub const FLOW_TTL: f64 = 15.0 * 60.0;
/// `client_name` used for dynamic registration.
pub const CLIENT_NAME: &str = "OMuse";

static FLOWS: LazyLock<Mutex<HashMap<String, Flow>>> = LazyLock::new(|| Mutex::new(HashMap::new()));
static LOCKS: LazyLock<Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// `OAuthError`: the user-facing failure of a sign-in / refresh step.
#[derive(Debug, Clone, thiserror::Error)]
#[error("{0}")]
pub struct OAuthError(pub String);

impl OAuthError {
    pub fn new(msg: impl Into<String>) -> Self {
        Self(msg.into())
    }
}

/// `discover()` result: `{resource, issuer, authorization_endpoint, token_endpoint, registration_endpoint, scopes}`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Discovery {
    pub resource: String,
    pub issuer: String,
    pub authorization_endpoint: String,
    pub token_endpoint: String,
    pub registration_endpoint: String,
    pub scopes: Vec<String>,
}

/// `register()` result: `{client_id, client_secret, auth_method}`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ClientReg {
    pub client_id: String,
    pub client_secret: String,
    pub auth_method: String,
}

/// A pending login (`_FLOWS[state]`); `purpose` + `extra` come back from [`complete`].
#[derive(Debug, Clone)]
pub struct Flow {
    pub server_url: String,
    pub redirect_uri: String,
    pub meta: Discovery,
    pub client: ClientReg,
    pub verifier: String,
    pub purpose: String,
    pub extra: Value,
    pub expires_at: f64,
}

/// `begin()` result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Begin {
    pub auth_url: String,
    pub state: String,
}

fn safe(url: &str) -> Result<String, OAuthError> {
    check_server_url(url).map_err(|e: MCPError| {
        OAuthError(format!(
            "授权服务器地址不安全 (unsafe OAuth URL {}): {}",
            take_chars(url, 120),
            e
        ))
    })
}

fn origin(url: &str) -> String {
    match urlparse(url) {
        Ok(p) => format!("{}://{}", p.scheme, p.netloc),
        Err(()) => String::new(),
    }
}

/// `_www_auth_param(header, key)`: a quoted or bare parameter of a `WWW-Authenticate` header.
pub fn www_auth_param(header: &str, key: &str) -> String {
    let quoted =
        Regex::new(&format!(r#"{}\s*=\s*"([^"]+)""#, regex::escape(key))).expect("www_auth quoted");
    let bare =
        Regex::new(&format!(r"{}\s*=\s*([^,\s]+)", regex::escape(key))).expect("www_auth bare");
    quoted
        .captures(header)
        .or_else(|| bare.captures(header))
        .map(|c| c[1].to_string())
        .unwrap_or_default()
}

fn http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(20))
        .connect_timeout(Duration::from_secs(20))
        .redirect(reqwest::redirect::Policy::none())
        .no_proxy()
        .build()
        .expect("reqwest client")
}

async fn get_json(
    c: &reqwest::Client,
    url: &str,
) -> Result<Option<Map<String, Value>>, OAuthError> {
    let url = safe(url)?;
    let Ok(r) = c
        .get(&url)
        .header("Accept", "application/json")
        .send()
        .await
    else {
        return Ok(None);
    };
    if r.status().as_u16() != 200 {
        return Ok(None);
    }
    match r.json::<Value>().await {
        Ok(Value::Object(m)) => Ok(Some(m)),
        _ => Ok(None),
    }
}

/// Python `urllib.parse.quote_plus` (safe = alphanumerics and `_.-~`, space → `+`).
fn quote_plus(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'_' | b'.' | b'-' | b'~' => {
                out.push(b as char)
            }
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}

/// Python `urllib.parse.urlencode(dict)`.
pub fn urlencode(pairs: &[(&str, &str)]) -> String {
    pairs
        .iter()
        .map(|(k, v)| format!("{}={}", quote_plus(k), quote_plus(v)))
        .collect::<Vec<_>>()
        .join("&")
}

/// `discover(server_url)`: the authorization server and its endpoints for an MCP server.
pub async fn discover(server_url: &str) -> Result<Discovery, OAuthError> {
    let server_url = safe(server_url)?;
    let p = urlparse(&server_url)
        .map_err(|_| OAuthError::new("授权服务器地址不安全 (unsafe OAuth URL)"))?;
    let c = http_client();
    let mut prm_urls: Vec<String> = Vec::new();
    // an unauthenticated MCP request should answer 401 and point at its resource metadata
    let init = json!({"jsonrpc": "2.0", "id": 0, "method": "initialize", "params": {
        "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": CLIENT_NAME, "version": *VERSION}}});
    if let Ok(r) = c
        .post(&server_url)
        .json(&init)
        .header("Accept", "application/json, text/event-stream")
        .header("MCP-Protocol-Version", "2025-06-18")
        .send()
        .await
    {
        let hdr = r
            .headers()
            .get("www-authenticate")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        let u = www_auth_param(hdr, "resource_metadata");
        if !u.is_empty() {
            prm_urls.push(u);
        }
    }
    let path = p.path.trim_end_matches('/');
    if !path.is_empty() {
        prm_urls.push(format!(
            "{}/.well-known/oauth-protected-resource{}",
            origin(&server_url),
            path
        ));
    }
    prm_urls.push(format!(
        "{}/.well-known/oauth-protected-resource",
        origin(&server_url)
    ));
    let mut prm: Option<Map<String, Value>> = None;
    for u in &prm_urls {
        prm = get_json(&c, u).await?;
        if prm
            .as_ref()
            .is_some_and(|m| m.get("authorization_servers").is_some_and(truthy))
        {
            break;
        }
        prm = None;
    }
    let issuer = prm
        .as_ref()
        .and_then(|m| m.get("authorization_servers"))
        .and_then(Value::as_array)
        .and_then(|a| a.first())
        .map(py_str)
        .unwrap_or_else(|| origin(&server_url))
        .trim_end_matches('/')
        .to_string();
    let ip = urlparse(&safe(&issuer)?)
        .map_err(|_| OAuthError::new("授权服务器地址不安全 (unsafe OAuth URL)"))?;
    let ipath = ip.path.trim_end_matches('/');
    let cands: Vec<String> = if !ipath.is_empty() {
        vec![
            format!(
                "{}/.well-known/oauth-authorization-server{}",
                origin(&issuer),
                ipath
            ),
            format!(
                "{}/.well-known/openid-configuration{}",
                origin(&issuer),
                ipath
            ),
            format!("{}/.well-known/openid-configuration", issuer),
        ]
    } else {
        vec![
            format!("{}/.well-known/oauth-authorization-server", issuer),
            format!("{}/.well-known/openid-configuration", issuer),
        ]
    };
    let mut meta: Option<Map<String, Value>> = None;
    for u in &cands {
        meta = get_json(&c, u).await?;
        if meta.as_ref().is_some_and(|m| {
            m.get("authorization_endpoint").is_some_and(truthy)
                && m.get("token_endpoint").is_some_and(truthy)
        }) {
            break;
        }
        meta = None;
    }
    let meta = meta.unwrap_or_else(|| {
        // 2025-03-26 servers without metadata: default endpoints at the issuer
        let mut m = Map::new();
        m.insert(
            "authorization_endpoint".into(),
            json!(format!("{}/authorize", issuer)),
        );
        m.insert("token_endpoint".into(), json!(format!("{}/token", issuer)));
        m.insert(
            "registration_endpoint".into(),
            json!(format!("{}/register", issuer)),
        );
        m
    });
    if let Some(methods) = meta
        .get("code_challenge_methods_supported")
        .filter(|m| truthy(m))
    {
        let has_s256 = match methods {
            Value::Array(a) => a.iter().any(|x| x.as_str() == Some("S256")),
            Value::String(s) => s.contains("S256"),
            _ => false,
        };
        if !has_s256 {
            return Err(OAuthError::new(
                "授权服务器不支持 PKCE S256 (server does not support PKCE S256)",
            ));
        }
    }
    let scopes: Vec<String> = prm
        .as_ref()
        .and_then(|m| m.get("scopes_supported"))
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|s| s.as_str().map(str::to_string))
                .take(20)
                .collect()
        })
        .unwrap_or_default();
    let resource = match prm.as_ref().and_then(|m| m.get("resource")) {
        Some(r) if truthy(r) => py_str(r),
        _ => server_url.clone(),
    };
    let reg = meta
        .get("registration_endpoint")
        .filter(|v| truthy(v))
        .map(py_str);
    Ok(Discovery {
        resource,
        issuer,
        authorization_endpoint: safe(
            &meta
                .get("authorization_endpoint")
                .map(py_str)
                .unwrap_or_default(),
        )?,
        token_endpoint: safe(&meta.get("token_endpoint").map(py_str).unwrap_or_default())?,
        registration_endpoint: match reg {
            Some(r) => safe(&r)?,
            None => String::new(),
        },
        scopes,
    })
}

/// `register(meta, redirect_uri)`: dynamic client registration (RFC 7591) as a public client.
pub async fn register(meta: &Discovery, redirect_uri: &str) -> Result<ClientReg, OAuthError> {
    if meta.registration_endpoint.is_empty() {
        return Err(OAuthError::new(
            "这个服务器不支持自动注册客户端 (no dynamic client registration)",
        ));
    }
    let mut body = json!({"client_name": CLIENT_NAME, "redirect_uris": [redirect_uri],
        "grant_types": ["authorization_code", "refresh_token"], "response_types": ["code"],
        "token_endpoint_auth_method": "none", "software_id": "omuse", "software_version": *VERSION});
    if !meta.scopes.is_empty() {
        body["scope"] = json!(meta.scopes.join(" "));
    }
    let r = http_client()
        .post(&meta.registration_endpoint)
        .json(&body)
        .send()
        .await
        .map_err(|e| OAuthError(format!("注册客户端失败 (client registration failed): {e}")))?;
    let status = r.status().as_u16();
    let text = r.text().await.unwrap_or_default();
    if status != 200 && status != 201 {
        return Err(OAuthError(format!(
            "注册客户端失败 (client registration failed): HTTP {} {}",
            status,
            take_chars(&text, 200)
        )));
    }
    let d: Value = serde_json::from_str(&text)
        .map_err(|e| OAuthError(format!("注册客户端失败 (client registration failed): {e}")))?;
    if !d.get("client_id").is_some_and(truthy) {
        return Err(OAuthError::new(
            "注册客户端失败：没有 client_id (no client_id)",
        ));
    }
    let secret = match d.get("client_secret") {
        Some(s) if truthy(s) => py_str(s),
        _ => String::new(),
    };
    let auth_method = match d.get("token_endpoint_auth_method") {
        Some(m) if truthy(m) => py_str(m),
        _ if !secret.is_empty() => "client_secret_post".to_string(),
        _ => "none".to_string(),
    };
    Ok(ClientReg {
        client_id: py_str(&d["client_id"]),
        client_secret: secret,
        auth_method,
    })
}

/// `_pkce() -> (verifier, challenge)`: S256.
pub fn pkce() -> (String, String) {
    let verifier = token_urlsafe(48);
    let challenge = pkce_challenge(&verifier);
    (verifier, challenge)
}

/// The S256 challenge of a verifier.
pub fn pkce_challenge(verifier: &str) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

/// Start the browser login. `purpose` + `extra` come back from [`complete`].
pub async fn begin(
    server_url: &str,
    redirect_uri: &str,
    purpose: &str,
    extra: Option<Value>,
) -> Result<Begin, OAuthError> {
    let meta = discover(server_url).await?;
    let client = register(&meta, redirect_uri).await?;
    let (verifier, challenge) = pkce();
    let state = token_urlsafe(24);
    let now = now_ts();
    {
        let mut flows = FLOWS.lock();
        flows.retain(|_, f| f.expires_at >= now);
        flows.insert(
            state.clone(),
            Flow {
                server_url: server_url.to_string(),
                redirect_uri: redirect_uri.to_string(),
                meta: meta.clone(),
                client: client.clone(),
                verifier,
                purpose: purpose.to_string(),
                extra: extra.filter(truthy).unwrap_or_else(|| json!({})),
                expires_at: now + FLOW_TTL,
            },
        );
    }
    let scope = meta.scopes.join(" ");
    let mut q: Vec<(&str, &str)> = vec![
        ("response_type", "code"),
        ("client_id", &client.client_id),
        ("redirect_uri", redirect_uri),
        ("state", &state),
        ("code_challenge", &challenge),
        ("code_challenge_method", "S256"),
        ("resource", &meta.resource),
    ];
    if !meta.scopes.is_empty() {
        q.push(("scope", &scope));
    }
    let sep = if meta.authorization_endpoint.contains('?') {
        "&"
    } else {
        "?"
    };
    Ok(Begin {
        auth_url: format!("{}{}{}", meta.authorization_endpoint, sep, urlencode(&q)),
        state,
    })
}

/// `_client_auth`: Basic header for `client_secret_basic`, otherwise the secret goes into the form.
fn client_auth(client: &ClientReg, form: &mut Vec<(String, String)>) -> Vec<(String, String)> {
    let mut headers = vec![("Accept".to_string(), "application/json".to_string())];
    if !client.client_secret.is_empty() && client.auth_method == "client_secret_basic" {
        let tok = base64::engine::general_purpose::STANDARD
            .encode(format!("{}:{}", client.client_id, client.client_secret).as_bytes());
        headers.push(("Authorization".to_string(), format!("Basic {tok}")));
    } else if !client.client_secret.is_empty() {
        form.push(("client_secret".to_string(), client.client_secret.clone()));
    }
    headers
}

async fn token(
    token_endpoint: &str,
    mut form: Vec<(String, String)>,
    client: &ClientReg,
) -> Result<Map<String, Value>, OAuthError> {
    let headers = client_auth(client, &mut form);
    let mut rb = http_client().post(token_endpoint).form(&form);
    for (k, v) in headers {
        rb = rb.header(k, v);
    }
    let r = rb
        .send()
        .await
        .map_err(|e| OAuthError(format!("换取令牌失败 (token request failed): {e}")))?;
    let status = r.status().as_u16();
    let text = r.text().await.unwrap_or_default();
    let d: Map<String, Value> = match serde_json::from_str::<Value>(&text) {
        Ok(Value::Object(m)) => m,
        _ => Map::new(),
    };
    if status != 200 || !d.get("access_token").is_some_and(truthy) {
        let err = d.get("error").map(py_str).unwrap_or_default();
        let desc = match d.get("error_description") {
            Some(v) if truthy(v) => py_str(v),
            _ => take_chars(&text, 200).to_string(),
        };
        return Err(OAuthError(
            format!("换取令牌失败 (token request failed): HTTP {status} {err} {desc}")
                .trim()
                .to_string(),
        ));
    }
    Ok(d)
}

/// `_record(meta, client, tok, old)`: the oauth record kept in the vault.
fn record(
    meta: &Discovery,
    client: &ClientReg,
    tok: &Map<String, Value>,
    old: Option<&Value>,
) -> Value {
    let exp = tok
        .get("expires_in")
        .filter(|v| truthy(v))
        .and_then(omuse_common::util::as_f64);
    let refresh_token = match tok.get("refresh_token") {
        Some(r) if truthy(r) => r.clone(),
        _ => old
            .and_then(|o| o.get("refresh_token"))
            .cloned()
            .unwrap_or_else(|| json!("")),
    };
    json!({
        "client_id": client.client_id, "client_secret": client.client_secret,
        "auth_method": if client.auth_method.is_empty() { "none" } else { client.auth_method.as_str() },
        "token_endpoint": meta.token_endpoint,
        "resource": meta.resource, "issuer": meta.issuer,
        "access_token": tok.get("access_token").cloned().unwrap_or(Value::Null),
        "refresh_token": refresh_token,
        "expires_at": match exp { Some(e) => json!(now_ts() + e), None => json!(0) },
        "scope": tok.get("scope").cloned().unwrap_or_else(|| json!("")),
    })
}

/// Finish the login from the callback. -> `(flow, oauth record to store in the vault)`.
pub async fn complete(state: &str, code: &str, error: &str) -> Result<(Flow, Value), OAuthError> {
    let f = FLOWS.lock().remove(state);
    let Some(f) = f.filter(|f| f.expires_at >= now_ts()) else {
        return Err(OAuthError::new(
            "登录已过期或无效，请重新开始 (login expired or invalid — start again)",
        ));
    };
    if !error.is_empty() {
        return Err(OAuthError(format!(
            "授权被拒绝或失败 (authorization failed): {error}"
        )));
    }
    if code.is_empty() {
        return Err(OAuthError::new("缺少授权码 (missing code)"));
    }
    let form = vec![
        ("grant_type".to_string(), "authorization_code".to_string()),
        ("code".to_string(), code.to_string()),
        ("redirect_uri".to_string(), f.redirect_uri.clone()),
        ("client_id".to_string(), f.client.client_id.clone()),
        ("code_verifier".to_string(), f.verifier.clone()),
        ("resource".to_string(), f.meta.resource.clone()),
    ];
    let tok = token(&f.meta.token_endpoint, form, &f.client).await?;
    let rec = record(&f.meta, &f.client, &tok, None);
    Ok((f, rec))
}

fn oauth_of(sec: &Value) -> Value {
    match sec.get("oauth") {
        Some(o) if truthy(o) => o.clone(),
        _ => json!({}),
    }
}

/// `refresh(store, handle)`: exchange the refresh token and store the new record; returns it.
pub async fn refresh(store: &Store, handle: &str) -> Result<Value> {
    let sec = store
        .get_secret(handle)?
        .filter(truthy)
        .unwrap_or_else(|| json!({}));
    let o = oauth_of(&sec);
    if !o.get("refresh_token").is_some_and(truthy) {
        return Err(
            OAuthError::new("授权已失效，请重新登录 (session expired — sign in again)").into(),
        );
    }
    let client = ClientReg {
        client_id: get_str(&o, "client_id"),
        client_secret: get_str(&o, "client_secret"),
        auth_method: match o.get("auth_method") {
            Some(m) if !m.is_null() => py_str(m),
            _ => "none".to_string(),
        },
    };
    let form = vec![
        ("grant_type".to_string(), "refresh_token".to_string()),
        ("refresh_token".to_string(), get_str(&o, "refresh_token")),
        ("client_id".to_string(), client.client_id.clone()),
        ("resource".to_string(), get_str(&o, "resource")),
    ];
    let tok = token(&get_str(&o, "token_endpoint"), form, &client).await?;
    let meta = Discovery {
        token_endpoint: get_str(&o, "token_endpoint"),
        resource: get_str(&o, "resource"),
        issuer: get_str(&o, "issuer"),
        ..Discovery::default()
    };
    let rec = record(&meta, &client, &tok, Some(&o));
    let connector = match sec.get("connector") {
        Some(c) if !c.is_null() => py_str(c),
        _ => "oauth".to_string(),
    };
    let mut payload = match sec {
        Value::Object(m) => m,
        _ => Map::new(),
    };
    payload.insert("oauth".into(), rec.clone());
    store.put_secret(&connector, &Value::Object(payload), Some(handle))?;
    Ok(rec)
}

fn lock_for(handle: &str) -> Arc<tokio::sync::Mutex<()>> {
    LOCKS
        .lock()
        .entry(handle.to_string())
        .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
        .clone()
}

/// Bearer header for the server behind `handle`, refreshing the token when it is (nearly) expired.
pub async fn auth_headers(store: &Store, handle: &str, force_refresh: bool) -> Result<Headers> {
    let lock = lock_for(handle);
    let _g = lock.lock().await;
    let mut o = oauth_of(&store.get_secret(handle)?.unwrap_or_else(|| json!({})));
    if !o.get("access_token").is_some_and(truthy) {
        return Err(OAuthError::new("还没有登录 (not signed in)").into());
    }
    let expires_at = o
        .get("expires_at")
        .filter(|v| truthy(v))
        .and_then(omuse_common::util::as_f64);
    if force_refresh || expires_at.is_some_and(|e| e - now_ts() < 60.0) {
        o = refresh(store, handle).await?;
    }
    Ok(vec![(
        "Authorization".to_string(),
        format!("Bearer {}", get_str(&o, "access_token")),
    )])
}

/// `save(store, handle, connector, rec, **extra)`: `{"connector": connector, "oauth": rec, **extra}` into the vault.
pub fn save(
    store: &Store,
    handle: &str,
    connector: &str,
    rec: &Value,
    extra: Option<&Map<String, Value>>,
) -> Result<()> {
    let mut payload = Map::new();
    payload.insert("connector".into(), json!(connector));
    payload.insert("oauth".into(), rec.clone());
    if let Some(x) = extra {
        for (k, v) in x {
            payload.insert(k.clone(), v.clone());
        }
    }
    store.put_secret(connector, &Value::Object(payload), Some(handle))?;
    Ok(())
}

/// `connected(store, handle)`: an access token is stored under `handle`.
pub fn connected(store: &Store, handle: &str) -> bool {
    store
        .get_secret(handle)
        .ok()
        .flatten()
        .map(|s| oauth_of(&s).get("access_token").is_some_and(truthy))
        .unwrap_or(false)
}

/// The callback must be OMuse's own address (the host the browser is using) and the fixed callback path.
pub fn check_redirect(
    redirect_uri: &str,
    host: &str,
    path_suffix: &str,
) -> Result<String, OAuthError> {
    let p = urlparse(redirect_uri)
        .map_err(|_| OAuthError::new("回调地址无效 (invalid redirect URI)"))?;
    if (p.scheme != "http" && p.scheme != "https")
        || p.netloc.is_empty()
        || !p.query.is_empty()
        || !p.fragment.is_empty()
    {
        return Err(OAuthError::new("回调地址无效 (invalid redirect URI)"));
    }
    let want = host.split(',').next().unwrap_or("").trim().to_lowercase();
    if p.netloc.to_lowercase() != want {
        return Err(OAuthError::new(
            "回调地址必须是 OMuse 自己的网址 (redirect URI must be this OMuse address)",
        ));
    }
    if !p.path.ends_with(path_suffix) {
        return Err(OAuthError::new("回调地址路径不对 (wrong redirect path)"));
    }
    let hostname = p.hostname().unwrap_or_default();
    if p.scheme == "http" && hostname != "localhost" && hostname != "127.0.0.1" {
        check_server_url(redirect_uri).map_err(|e| OAuthError(e.msg))?;
    }
    Ok(format!("{}://{}{}", p.scheme, p.netloc, p.path))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn www_authenticate_param() {
        assert_eq!(
            www_auth_param(
                r#"Bearer realm="x", resource_metadata="https://h/.well-known/oauth-protected-resource""#,
                "resource_metadata"
            ),
            "https://h/.well-known/oauth-protected-resource"
        );
        assert_eq!(
            www_auth_param(
                "Bearer resource_metadata=https://h/prm, scope=x",
                "resource_metadata"
            ),
            "https://h/prm"
        );
        assert_eq!(www_auth_param("", "resource_metadata"), "");
    }

    #[test]
    fn pkce_s256() {
        // RFC 7636 appendix B
        assert_eq!(
            pkce_challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
        let (v, c) = pkce();
        assert_eq!(v.len(), 64);
        assert_eq!(pkce_challenge(&v), c);
    }

    #[test]
    fn python_urlencode() {
        assert_eq!(
            urlencode(&[
                ("redirect_uri", "https://h/cb?x=1"),
                ("scope", "a b"),
                ("state", "x_y-z~")
            ]),
            "redirect_uri=https%3A%2F%2Fh%2Fcb%3Fx%3D1&scope=a+b&state=x_y-z~"
        );
    }

    #[test]
    fn redirect_checks() {
        let cb = "/sentinel/api/oauth/callback";
        assert_eq!(
            check_redirect(
                "https://omuse.example.com/sentinel/api/oauth/callback",
                "omuse.example.com",
                cb
            )
            .unwrap(),
            "https://omuse.example.com/sentinel/api/oauth/callback"
        );
        assert_eq!(
            check_redirect(
                "http://localhost:8080/sentinel/api/oauth/callback",
                "localhost:8080, proxy",
                cb
            )
            .unwrap(),
            "http://localhost:8080/sentinel/api/oauth/callback"
        );
        assert!(check_redirect(
            "http://192.168.1.2:8080/sentinel/api/oauth/callback",
            "192.168.1.2:8080",
            cb
        )
        .is_ok());
        assert_eq!(
            check_redirect(
                "https://evil.com/sentinel/api/oauth/callback",
                "omuse.example.com",
                cb
            )
            .unwrap_err()
            .0,
            "回调地址必须是 OMuse 自己的网址 (redirect URI must be this OMuse address)"
        );
        assert_eq!(
            check_redirect("https://h/other", "h", cb).unwrap_err().0,
            "回调地址路径不对 (wrong redirect path)"
        );
        assert_eq!(
            check_redirect("https://h/sentinel/api/oauth/callback?x=1", "h", cb)
                .unwrap_err()
                .0,
            "回调地址无效 (invalid redirect URI)"
        );
        assert_eq!(
            check_redirect("ftp://h/sentinel/api/oauth/callback", "h", cb)
                .unwrap_err()
                .0,
            "回调地址无效 (invalid redirect URI)"
        );
        assert!(check_redirect(
            "http://example.com/sentinel/api/oauth/callback",
            "example.com",
            cb
        )
        .unwrap_err()
        .0
        .contains("https"));
    }

    #[test]
    fn record_shape() {
        let meta = Discovery {
            token_endpoint: "https://as/token".into(),
            resource: "https://mcp/x".into(),
            issuer: "https://as".into(),
            ..Default::default()
        };
        let client = ClientReg {
            client_id: "cid".into(),
            client_secret: String::new(),
            auth_method: "none".into(),
        };
        let mut tok = Map::new();
        tok.insert("access_token".into(), json!("at"));
        tok.insert("expires_in".into(), json!(3600));
        let rec = record(
            &meta,
            &client,
            &tok,
            Some(&json!({"refresh_token": "old-rt"})),
        );
        assert_eq!(rec["access_token"], "at");
        assert_eq!(rec["refresh_token"], "old-rt");
        assert!(rec["expires_at"].as_f64().unwrap() > now_ts() + 3000.0);
        let keys: Vec<&String> = rec.as_object().unwrap().keys().collect();
        assert_eq!(
            keys,
            [
                "client_id",
                "client_secret",
                "auth_method",
                "token_endpoint",
                "resource",
                "issuer",
                "access_token",
                "refresh_token",
                "expires_at",
                "scope"
            ]
        );
        tok.remove("expires_in");
        assert_eq!(record(&meta, &client, &tok, None)["expires_at"], json!(0));
        assert_eq!(record(&meta, &client, &tok, None)["refresh_token"], "");
    }

    #[test]
    fn basic_client_auth() {
        let mut form = vec![];
        let h = client_auth(
            &ClientReg {
                client_id: "a".into(),
                client_secret: "b".into(),
                auth_method: "client_secret_basic".into(),
            },
            &mut form,
        );
        assert_eq!(
            h[1],
            ("Authorization".to_string(), "Basic YTpi".to_string())
        );
        assert!(form.is_empty());
        let h = client_auth(
            &ClientReg {
                client_id: "a".into(),
                client_secret: "b".into(),
                auth_method: "client_secret_post".into(),
            },
            &mut form,
        );
        assert_eq!(h.len(), 1);
        assert_eq!(form, vec![("client_secret".to_string(), "b".to_string())]);
    }

    #[tokio::test]
    async fn complete_rejects_unknown_state() {
        let e = complete("nope", "code", "").await.unwrap_err();
        assert_eq!(
            e.0,
            "登录已过期或无效，请重新开始 (login expired or invalid — start again)"
        );
    }

    #[test]
    fn store_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path()).unwrap();
        assert!(!connected(&store, "cred_phone_dialmcp"));
        save(
            &store,
            "cred_phone_dialmcp",
            "phone",
            &json!({"access_token": "t", "refresh_token": "r"}),
            None,
        )
        .unwrap();
        assert!(connected(&store, "cred_phone_dialmcp"));
        let sec = store.get_secret("cred_phone_dialmcp").unwrap().unwrap();
        assert_eq!(sec["connector"], "phone");
        assert_eq!(sec["oauth"]["access_token"], "t");
    }
}
