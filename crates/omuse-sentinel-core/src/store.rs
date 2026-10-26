//! Sentinel persistent state: approvals, grants, taint, connections, secrets, audit (hash chained).

use std::collections::BTreeSet;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use parking_lot::Mutex;
use rusqlite::OptionalExtension;
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};

use omuse_common::fernet::Fernet;
use omuse_common::pyjson::{dumps, dumps_ascii};
use omuse_common::util::{loads, new_id, now_ts, take_chars, truthy};
use omuse_common::{params, Db};

use crate::guard::max_class;

/// The SQLite schema (identical to the Python version, so an existing `sentinel.db` keeps working).
pub const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS approvals (
  id TEXT PRIMARY KEY, task_id TEXT, call_id TEXT, tool TEXT, args TEXT, summary TEXT,
  risk TEXT, reason TEXT, status TEXT, scope TEXT, decided_by TEXT,
  result TEXT, created_at REAL, resolved_at REAL
);
CREATE INDEX IF NOT EXISTS ix_appr_status ON approvals(status);
CREATE TABLE IF NOT EXISTS grants (
  id TEXT PRIMARY KEY, tool TEXT, scope TEXT, task_id TEXT, match TEXT,
  expires_at REAL, created_at REAL, revoked INTEGER DEFAULT 0, note TEXT
);
CREATE TABLE IF NOT EXISTS task_ctx (
  task_id TEXT PRIMARY KEY, taint TEXT, injection TEXT, domains TEXT, updated_at REAL
);
CREATE TABLE IF NOT EXISTS purchases (
    id TEXT PRIMARY KEY, task_id TEXT, domain TEXT, card_item TEXT, max_total REAL, currency TEXT, summary TEXT,
    clicks_left INTEGER, fills_left INTEGER, created_at REAL, expires_at REAL, status TEXT);
CREATE TABLE IF NOT EXISTS ledger (
  id TEXT PRIMARY KEY, task_id TEXT, purchase_id TEXT, approval_id TEXT, merchant TEXT, order_number TEXT, items TEXT,
  total REAL, currency TEXT, card TEXT, delivery TEXT, status TEXT, evidence TEXT, history TEXT, created_at REAL,
  updated_at REAL);
CREATE TABLE IF NOT EXISTS user_requests (
  task_id TEXT PRIMARY KEY, text TEXT, created_at REAL
);
CREATE TABLE IF NOT EXISTS connections (
  name TEXT PRIMARY KEY, config TEXT, permissions TEXT, enabled INTEGER, updated_at REAL
);
CREATE TABLE IF NOT EXISTS secrets (
  handle TEXT PRIMARY KEY, connector TEXT, blob BLOB, created_at REAL
);
CREATE TABLE IF NOT EXISTS audit (
  seq INTEGER PRIMARY KEY AUTOINCREMENT, ts REAL, actor TEXT, task_id TEXT, action TEXT,
  resource TEXT, risk TEXT, decision TEXT, result TEXT, detail TEXT, prev_hash TEXT, hash TEXT
);
CREATE INDEX IF NOT EXISTS ix_audit_task ON audit(task_id);
CREATE TABLE IF NOT EXISTS kv (k TEXT PRIMARY KEY, v TEXT, updated_at REAL);
CREATE TRIGGER IF NOT EXISTS audit_no_update BEFORE UPDATE ON audit BEGIN SELECT RAISE(ABORT, 'audit is append-only'); END;
CREATE TRIGGER IF NOT EXISTS audit_no_delete BEFORE DELETE ON audit BEGIN SELECT RAISE(ABORT, 'audit is append-only'); END;
";

/// `DEFAULT_CONNECTIONS`: `{name: {"config": {..}, "permissions": {..}, "enabled": 0|1}}`, inserted once.
pub fn default_connections() -> Value {
    json!({
        "gmail": {
            "config": {"email": "", "imap_host": "imap.gmail.com", "smtp_host": "smtp.gmail.com", "display_name": ""},
            "permissions": {"read": true, "organize": true, "draft": true, "send": true},
            "enabled": 1,
        },
        "browser": {
            "config": {"blocked_domains": [], "allowed_domains": []},
            "permissions": {"browse": true, "interact": true, "upload": true, "download": true},
            "enabled": 1,
        },
        "telegram": {
            "config": {"chat_id": ""},
            "permissions": {"notify": true, "control": true},
            "enabled": 0,
        },
        "notion": {
            "config": {"workspace": "", "bot_id": ""},
            "permissions": {"read": true, "write": true},
            "enabled": 0,
        },
        "slack": {
            "config": {"team": "", "user": "", "user_id": "", "bot_id": "", "token_type": ""},
            "permissions": {"read": true, "send": true},
            "enabled": 0,
        },
        "calendar": {
            "config": {"email": "", "time_zone": "", "client_id": ""},
            "permissions": {"read": true, "write": true},
            "enabled": 0,
        },
        "drive": {
            "config": {"email": "", "client_id": ""},
            "permissions": {"read": true, "upload": true, "write": true},
            "enabled": 0,
        },
        "phone": {
            "config": {"from_number": "", "connection_id": "", "public_url": "", "owner_name": "",
                       "allowed_prefixes": ["+65", "+1"], "max_minutes": 10, "daily_limit": 10, "voice": "marin",
                       "model": "gpt-realtime"},
            "permissions": {"call": true},
            "enabled": 0,
        },
    })
}

/// Keyword arguments of `Store::audit` (all optional, like the Python keyword-only parameters).
#[derive(Debug, Clone, Default)]
pub struct Audit {
    pub task_id: String,
    pub resource: String,
    pub risk: String,
    pub decision: String,
    pub result: String,
    pub detail: Option<Value>,
}

/// Sentinel's state store (`sentinel.db`, `audit.jsonl`, `vault.key`). `Send + Sync`; share it behind an `Arc`.
pub struct Store {
    /// The data directory (`<data>/sentinel`).
    pub dir: PathBuf,
    /// The SQLite connection (modules like `vault`, `ledger`, `suggest` query it directly, as in Python).
    pub db: Db,
    fernet: Fernet,
    audit_lock: Mutex<()>,
    jsonl: PathBuf,
    /// Compiled vault scrub patterns (`vault._patterns`), per store so that several stores never mix.
    pub(crate) vault_patterns: Mutex<Option<Arc<Vec<fancy_regex::Regex>>>>,
}

/// `loads(row[key], default)` for a JSON row.
pub(crate) fn loads_field(row: &Value, key: &str, default: Value) -> Value {
    loads(row.get(key).and_then(Value::as_str), default)
}

/// `row[key]` as a string (`""` for missing / null).
pub(crate) fn field_str(row: &Value, key: &str) -> String {
    omuse_common::util::get_str(row, key)
}

/// `row[key]` as a float (`0.0` for missing / null).
pub(crate) fn field_f64(row: &Value, key: &str) -> f64 {
    row.get(key)
        .and_then(omuse_common::util::as_f64)
        .unwrap_or(0.0)
}

/// `row[key]` as an integer (`0` for missing / null).
pub(crate) fn field_i64(row: &Value, key: &str) -> i64 {
    row.get(key)
        .and_then(omuse_common::util::as_i64)
        .unwrap_or(0)
}

fn set(row: &mut Value, key: &str, v: Value) {
    if let Some(o) = row.as_object_mut() {
        o.insert(key.to_string(), v);
    }
}

impl Store {
    /// Open (creating) the store under `data_dir`.
    pub fn new(data_dir: impl AsRef<Path>) -> Result<Store> {
        let dir = data_dir.as_ref().to_path_buf();
        std::fs::create_dir_all(&dir).with_context(|| format!("mkdir {}", dir.display()))?;
        let db = Db::open(dir.join("sentinel.db"))?;
        db.script(SCHEMA)?;
        let fernet = Fernet::new(&Self::load_key(&dir)?)?;
        let store = Store {
            jsonl: dir.join("audit.jsonl"),
            dir,
            db,
            fernet,
            audit_lock: Mutex::new(()),
            vault_patterns: Mutex::new(None),
        };
        if let Value::Object(defaults) = default_connections() {
            for (name, d) in defaults {
                if store
                    .db
                    .one("SELECT name FROM connections WHERE name=?", params![&name])?
                    .is_none()
                {
                    store.db.insert(
                        "connections",
                        &json!({
                            "name": name, "config": dumps(&d["config"]), "permissions": dumps(&d["permissions"]),
                            "enabled": d["enabled"], "updated_at": now_ts(),
                        }),
                    )?;
                }
            }
        }
        crate::vault::init(&store)?;
        Ok(store)
    }

    // ------------------------------------------------------------ vault
    fn load_key(dir: &Path) -> Result<String> {
        let path = dir.join("vault.key");
        if path.exists() {
            let raw = std::fs::read(&path).with_context(|| format!("read {}", path.display()))?;
            return Ok(String::from_utf8_lossy(&raw).trim().to_string());
        }
        let key = Fernet::generate_key();
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let mut f = opts
            .open(&path)
            .with_context(|| format!("create {}", path.display()))?;
        f.write_all(key.as_bytes())?;
        Ok(key)
    }

    /// Encrypt `payload` under `handle` (default `cred_<connector>_1`); returns the handle.
    pub fn put_secret(
        &self,
        connector: &str,
        payload: &Value,
        handle: Option<&str>,
    ) -> Result<String> {
        let handle = match handle {
            Some(h) if !h.is_empty() => h.to_string(),
            _ => format!("cred_{}_1", connector),
        };
        let blob = self
            .fernet
            .encrypt(dumps_ascii(payload).as_bytes())
            .into_bytes();
        self.db.execute(
            "INSERT OR REPLACE INTO secrets(handle, connector, blob, created_at) VALUES (?,?,?,?)",
            params![&handle, connector, blob, now_ts()],
        )?;
        Ok(handle)
    }

    fn secret_blob(&self, handle: &str) -> Result<Option<Vec<u8>>> {
        self.db.with(|c| {
            let v = c
                .query_row("SELECT blob FROM secrets WHERE handle=?", [handle], |r| {
                    r.get::<_, rusqlite::types::Value>(0)
                })
                .optional()?;
            Ok(v.and_then(|v| match v {
                rusqlite::types::Value::Blob(b) => Some(b),
                rusqlite::types::Value::Text(t) => Some(t.into_bytes()),
                _ => None,
            }))
        })
    }

    /// Decrypt a secret; `None` when missing or undecryptable.
    pub fn get_secret(&self, handle: &str) -> Result<Option<Value>> {
        let Some(blob) = self.secret_blob(handle)? else {
            return Ok(None);
        };
        let token = String::from_utf8_lossy(&blob).into_owned();
        Ok(self
            .fernet
            .decrypt(&token)
            .ok()
            .and_then(|pt| serde_json::from_slice::<Value>(&pt).ok()))
    }

    /// Is there a secret under `handle`?
    pub fn has_secret(&self, handle: &str) -> Result<bool> {
        Ok(self
            .db
            .one("SELECT handle FROM secrets WHERE handle=?", params![handle])?
            .is_some())
    }

    /// Remove a secret.
    pub fn delete_secret(&self, handle: &str) -> Result<()> {
        self.db
            .execute("DELETE FROM secrets WHERE handle=?", params![handle])?;
        Ok(())
    }

    // ------------------------------------------------------------ small key/value state (bot offsets, public URL, ...)
    /// `kv_get(k)`: `None` when the key is missing (invalid JSON → `Null`).
    pub fn kv_get(&self, k: &str) -> Result<Option<Value>> {
        Ok(self
            .db
            .one("SELECT v FROM kv WHERE k=?", params![k])?
            .map(|row| loads_field(&row, "v", Value::Null)))
    }

    /// `kv_set(k, v)`.
    pub fn kv_set(&self, k: &str, v: &Value) -> Result<()> {
        self.db.execute(
            "INSERT OR REPLACE INTO kv(k, v, updated_at) VALUES (?,?,?)",
            params![k, dumps(v), now_ts()],
        )?;
        Ok(())
    }

    // ------------------------------------------------------------ connections
    /// `{"name", "config", "permissions", "enabled"[, "updated_at"]}`.
    pub fn connection(&self, name: &str) -> Result<Value> {
        let Some(row) = self
            .db
            .one("SELECT * FROM connections WHERE name=?", params![name])?
        else {
            return Ok(json!({"name": name, "config": {}, "permissions": {}, "enabled": false}));
        };
        Ok(json!({
            "name": name,
            "config": loads_field(&row, "config", json!({})),
            "permissions": loads_field(&row, "permissions", json!({})),
            "enabled": truthy(&row["enabled"]),
            "updated_at": row["updated_at"],
        }))
    }

    /// Merge `config` / `permissions` (values coerced to bool) / `enabled` into the stored connection.
    pub fn save_connection(
        &self,
        name: &str,
        config: Option<&Value>,
        permissions: Option<&Value>,
        enabled: Option<bool>,
    ) -> Result<Value> {
        let mut cur = self.connection(name)?;
        if let Some(Value::Object(cfg)) = config {
            if !cur["config"].is_object() {
                cur["config"] = json!({});
            }
            let dst = cur["config"].as_object_mut().expect("object");
            for (k, v) in cfg {
                dst.insert(k.clone(), v.clone());
            }
        }
        if let Some(Value::Object(perms)) = permissions {
            if !cur["permissions"].is_object() {
                cur["permissions"] = json!({});
            }
            let dst = cur["permissions"].as_object_mut().expect("object");
            for (k, v) in perms {
                dst.insert(k.clone(), Value::Bool(truthy(v)));
            }
        }
        if let Some(e) = enabled {
            cur["enabled"] = Value::Bool(e);
        }
        self.db.execute(
            "INSERT OR REPLACE INTO connections(name, config, permissions, enabled, updated_at) VALUES (?,?,?,?,?)",
            params![name, dumps(&cur["config"]), dumps(&cur["permissions"]), truthy(&cur["enabled"]), now_ts()],
        )?;
        self.connection(name)
    }

    // ------------------------------------------------------------ task context (taint / injection)
    /// `{"taint", "injection": [..], "domains": [..]}` (defaults: PUBLIC, empty).
    pub fn task_ctx(&self, task_id: &str) -> Result<Value> {
        let Some(row) = self
            .db
            .one("SELECT * FROM task_ctx WHERE task_id=?", params![task_id])?
        else {
            return Ok(json!({"taint": "PUBLIC", "injection": [], "domains": []}));
        };
        Ok(json!({
            "taint": row["taint"],
            "injection": loads_field(&row, "injection", json!([])),
            "domains": loads_field(&row, "domains", json!([])),
        }))
    }

    /// How many of these actions already ran in this task (from the audit log).
    pub fn count_done(&self, task_id: &str, actions: &[&str]) -> Result<i64> {
        if task_id.is_empty() {
            return Ok(0);
        }
        let q = format!(
            "SELECT COUNT(*) AS n FROM audit WHERE task_id=? AND action IN ({}) AND result='success'",
            vec!["?"; actions.len()].join(",")
        );
        let mut p = params![task_id];
        p.extend(actions.iter().map(|a| omuse_common::db::Param::from(*a)));
        Ok(self.db.one(&q, p)?.map(|r| field_i64(&r, "n")).unwrap_or(0))
    }

    /// The user's own words that started a task, recorded by Sentinel at the front door (the agent can't change it).
    pub fn set_user_request(&self, task_id: &str, text: &str) -> Result<()> {
        if !task_id.is_empty() && !text.is_empty() {
            self.db.execute(
                "INSERT OR REPLACE INTO user_requests(task_id, text, created_at) VALUES (?,?,?)",
                params![task_id, take_chars(text, 4000), now_ts()],
            )?;
        }
        Ok(())
    }

    /// The recorded user request (`""` when none).
    pub fn user_request(&self, task_id: &str) -> Result<String> {
        Ok(self
            .db
            .one(
                "SELECT text FROM user_requests WHERE task_id=?",
                params![task_id],
            )?
            .map(|r| field_str(&r, "text"))
            .unwrap_or_default())
    }

    /// Raise the taint, add injection flags and/or a visited domain; returns the new context.
    pub fn update_task_ctx(
        &self,
        task_id: &str,
        taint: Option<&str>,
        injection: Option<&[String]>,
        domain: Option<&str>,
    ) -> Result<Value> {
        let mut ctx = self.task_ctx(task_id)?;
        if let Some(t) = taint.filter(|t| !t.is_empty()) {
            let cur = field_str(&ctx, "taint");
            set(&mut ctx, "taint", Value::String(max_class(&cur, t)));
        }
        if let Some(inj) = injection.filter(|i| !i.is_empty()) {
            let mut all: BTreeSet<String> = ctx["injection"]
                .as_array()
                .map(|a| a.iter().map(omuse_common::util::py_str).collect())
                .unwrap_or_default();
            all.extend(inj.iter().cloned());
            set(
                &mut ctx,
                "injection",
                Value::Array(all.into_iter().map(Value::String).collect()),
            );
        }
        if let Some(d) = domain.filter(|d| !d.is_empty()) {
            let present = ctx["domains"]
                .as_array()
                .is_some_and(|a| a.iter().any(|x| x.as_str() == Some(d)));
            if !present {
                if let Some(a) = ctx["domains"].as_array_mut() {
                    a.push(Value::String(d.to_string()));
                }
            }
        }
        self.db.execute(
            "INSERT OR REPLACE INTO task_ctx(task_id, taint, injection, domains, updated_at) VALUES (?,?,?,?,?)",
            params![task_id, field_str(&ctx, "taint"), dumps(&ctx["injection"]), dumps(&ctx["domains"]), now_ts()],
        )?;
        Ok(ctx)
    }

    // ------------------------------------------------------------ one-card purchase confirmations
    /// `add_purchase` with the Python defaults (`ttl=1800`, `clicks=10`, `fills=12`).
    pub fn add_purchase(
        &self,
        task_id: &str,
        domain: &str,
        card_item: &str,
        max_total: f64,
        currency: &str,
        summary: &Value,
    ) -> Result<String> {
        self.add_purchase_with(
            task_id, domain, card_item, max_total, currency, summary, 1800.0, 10, 12,
        )
    }

    /// Record a confirmed purchase (one approval covering checkout clicks / card fills on `domain`).
    #[allow(clippy::too_many_arguments)]
    pub fn add_purchase_with(
        &self,
        task_id: &str,
        domain: &str,
        card_item: &str,
        max_total: f64,
        currency: &str,
        summary: &Value,
        ttl: f64,
        clicks: i64,
        fills: i64,
    ) -> Result<String> {
        let pid = new_id("buy");
        let summary = if summary.is_null() {
            json!({})
        } else {
            summary.clone()
        };
        self.db.insert(
            "purchases",
            &json!({
                "id": pid, "task_id": task_id, "domain": domain, "card_item": card_item, "max_total": max_total,
                "currency": currency, "summary": dumps(&summary), "clicks_left": clicks, "fills_left": fills,
                "created_at": now_ts(), "expires_at": now_ts() + ttl, "status": "active",
            }),
        )?;
        Ok(pid)
    }

    /// The unexpired active purchase of this task on `domain` (or a parent / sub domain), summary parsed.
    pub fn active_purchase(&self, task_id: &str, domain: &str) -> Result<Option<Value>> {
        if task_id.is_empty() || domain.is_empty() {
            return Ok(None);
        }
        let rows = self.db.all(
            "SELECT * FROM purchases WHERE task_id=? AND status='active' ORDER BY created_at DESC",
            params![task_id],
        )?;
        for mut r in rows {
            if field_f64(&r, "expires_at") < now_ts() {
                continue;
            }
            let d = field_str(&r, "domain");
            if domain == d
                || domain.ends_with(&format!(".{}", d))
                || d.ends_with(&format!(".{}", domain))
            {
                let summary = loads_field(&r, "summary", json!({}));
                set(&mut r, "summary", summary);
                return Ok(Some(r));
            }
        }
        Ok(None)
    }

    /// Consume one click (`kind != "fill"`) or one fill (`kind == "fill"`) of a purchase.
    pub fn use_purchase(&self, pid: &str, kind: &str) -> Result<()> {
        let col = if kind == "fill" {
            "fills_left"
        } else {
            "clicks_left"
        };
        self.db.execute(
            &format!("UPDATE purchases SET {col}={col}-1 WHERE id=?"),
            params![pid],
        )?;
        Ok(())
    }

    /// Close a purchase (`status` default `"used"` in Python).
    pub fn end_purchase(&self, pid: &str, status: &str) -> Result<()> {
        self.db.execute(
            "UPDATE purchases SET status=? WHERE id=?",
            params![status, pid],
        )?;
        Ok(())
    }

    // ------------------------------------------------------------ grants
    /// Add a standing approval; `ttl` seconds (`None`/`0` = no expiry). Returns the grant id.
    pub fn add_grant(
        &self,
        tool: &str,
        scope: &str,
        task_id: Option<&str>,
        match_: Option<&Value>,
        ttl: Option<f64>,
        note: &str,
    ) -> Result<String> {
        let gid = new_id("grant");
        let m = match match_ {
            Some(v) if !v.is_null() => v.clone(),
            _ => json!({}),
        };
        let expires_at = ttl.filter(|t| *t != 0.0).map(|t| now_ts() + t);
        self.db.insert(
            "grants",
            &json!({
                "id": gid, "tool": tool, "scope": scope, "task_id": task_id.unwrap_or(""), "match": dumps(&m),
                "expires_at": expires_at, "created_at": now_ts(), "revoked": 0, "note": note,
            }),
        )?;
        Ok(gid)
    }

    /// Unrevoked, unexpired grants (newest first), `match` parsed.
    pub fn active_grants(&self) -> Result<Vec<Value>> {
        let rows = self.db.all(
            "SELECT * FROM grants WHERE revoked=0 ORDER BY created_at DESC",
            params![],
        )?;
        let mut out = Vec::new();
        for mut r in rows {
            let exp = field_f64(&r, "expires_at");
            if exp != 0.0 && exp < now_ts() {
                continue;
            }
            let m = loads_field(&r, "match", json!({}));
            set(&mut r, "match", m);
            out.push(r);
        }
        Ok(out)
    }

    /// Revoke a grant.
    pub fn revoke_grant(&self, gid: &str) -> Result<()> {
        self.db
            .execute("UPDATE grants SET revoked=1 WHERE id=?", params![gid])?;
        Ok(())
    }

    // ------------------------------------------------------------ approvals
    /// Create a pending approval; returns it (as `approval(id)`).
    #[allow(clippy::too_many_arguments)]
    pub fn create_approval(
        &self,
        task_id: &str,
        call_id: &str,
        tool: &str,
        args: &Value,
        summary: &Value,
        risk: &str,
        reason: &str,
    ) -> Result<Value> {
        let aid = new_id("appr");
        self.db.insert(
            "approvals",
            &json!({
                "id": aid, "task_id": task_id, "call_id": call_id, "tool": tool, "args": dumps(args),
                "summary": dumps(summary), "risk": risk, "reason": reason, "status": "pending", "scope": "",
                "decided_by": "", "result": "", "created_at": now_ts(), "resolved_at": null,
            }),
        )?;
        Ok(self.approval(&aid)?.unwrap_or(Value::Null))
    }

    /// One approval with `args` / `summary` / `result` parsed, or `None`.
    pub fn approval(&self, aid: &str) -> Result<Option<Value>> {
        let Some(mut r) = self
            .db
            .one("SELECT * FROM approvals WHERE id=?", params![aid])?
        else {
            return Ok(None);
        };
        let args = loads_field(&r, "args", json!({}));
        let summary = loads_field(&r, "summary", json!({}));
        let result = loads_field(&r, "result", Value::Null);
        set(&mut r, "args", args);
        set(&mut r, "summary", summary);
        set(&mut r, "result", result);
        Ok(Some(r))
    }

    /// Approvals by status (`"resolved"` = everything not pending, newest decided first), newest first.
    pub fn approvals(&self, status: Option<&str>, limit: i64) -> Result<Vec<Value>> {
        let rows = match status {
            Some("resolved") => self.db.all(
                "SELECT id FROM approvals WHERE status!='pending' ORDER BY COALESCE(resolved_at, created_at) DESC LIMIT ?",
                params![limit],
            )?,
            Some(s) if !s.is_empty() => self.db.all(
                "SELECT id FROM approvals WHERE status=? ORDER BY created_at DESC LIMIT ?",
                params![s, limit],
            )?,
            _ => self.db.all("SELECT id FROM approvals ORDER BY created_at DESC LIMIT ?", params![limit])?,
        };
        let mut out = Vec::with_capacity(rows.len());
        for r in rows {
            if let Some(a) = self.approval(&field_str(&r, "id"))? {
                out.push(a);
            }
        }
        Ok(out)
    }

    /// Decide an approval (`decided_by` is `"user"` in the Python default).
    pub fn resolve_approval(
        &self,
        aid: &str,
        status: &str,
        scope: &str,
        result: Option<&Value>,
        args: Option<&Value>,
        decided_by: &str,
    ) -> Result<()> {
        let mut data = Map::new();
        data.insert("status".into(), json!(status));
        data.insert("scope".into(), json!(scope));
        data.insert("resolved_at".into(), json!(now_ts()));
        data.insert("decided_by".into(), json!(decided_by));
        data.insert(
            "result".into(),
            json!(result.map(dumps).unwrap_or_default()),
        );
        if let Some(a) = args {
            data.insert("args".into(), json!(dumps(a)));
        }
        self.db
            .update("approvals", "id", aid, &Value::Object(data))?;
        Ok(())
    }

    // ------------------------------------------------------------ audit (append-only, hash chained)
    /// Append an audit record (hash-chained, mirrored to `audit.jsonl`); returns the record.
    pub fn audit(&self, actor: &str, action: &str, a: Audit) -> Result<Value> {
        let _g = self.audit_lock.lock();
        let prev = self
            .db
            .one(
                "SELECT hash FROM audit ORDER BY seq DESC LIMIT 1",
                params![],
            )?
            .map(|r| field_str(&r, "hash"))
            .unwrap_or_else(|| "GENESIS".to_string());
        let ts = now_ts();
        let detail = a.detail.unwrap_or_else(|| json!({}));
        let mut detail_s = dumps(&detail);
        if detail_s.chars().count() > 20000 {
            detail_s = dumps(&json!({"truncated": true, "preview": take_chars(&detail_s, 20000)}));
        }
        let body = dumps(&json!([
            ts, actor, a.task_id, action, a.resource, a.risk, a.decision, a.result, detail_s, prev
        ]));
        let h = hex::encode(Sha256::digest(body.as_bytes()));
        let seq = self.db.insert(
            "audit",
            &json!({
                "ts": ts, "actor": actor, "task_id": a.task_id, "action": action, "resource": a.resource, "risk": a.risk,
                "decision": a.decision, "result": a.result, "detail": detail_s, "prev_hash": prev, "hash": h,
            }),
        )?;
        let rec = json!({
            "seq": seq, "ts": ts, "actor": actor, "task_id": a.task_id, "action": action, "resource": a.resource,
            "risk": a.risk, "decision": a.decision, "result": a.result, "detail": loads(Some(&detail_s), json!({})),
            "prev_hash": prev, "hash": h,
        });
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .append(true)
            .create(true)
            .open(&self.jsonl)
        {
            let _ = f.write_all(format!("{}\n", dumps(&rec)).as_bytes());
        }
        Ok(rec)
    }

    /// Audit records, newest first, optionally filtered by task / actor / `seq < before`.
    pub fn audit_list(
        &self,
        task_id: Option<&str>,
        limit: i64,
        before: Option<i64>,
        actor: Option<&str>,
    ) -> Result<Vec<Value>> {
        let mut q = String::from("SELECT * FROM audit WHERE 1=1");
        let mut p = params![];
        if let Some(t) = task_id.filter(|t| !t.is_empty()) {
            q.push_str(" AND task_id=?");
            p.push(t.into());
        }
        if let Some(a) = actor.filter(|a| !a.is_empty()) {
            q.push_str(" AND actor=?");
            p.push(a.into());
        }
        if let Some(b) = before.filter(|b| *b != 0) {
            q.push_str(" AND seq<?");
            p.push(b.into());
        }
        q.push_str(" ORDER BY seq DESC LIMIT ?");
        p.push(limit.into());
        let mut rows = self.db.all(&q, p)?;
        for r in rows.iter_mut() {
            let d = loads_field(r, "detail", json!({}));
            set(r, "detail", d);
        }
        Ok(rows)
    }

    /// Walk the chain: `{"ok": true, "checked": n, "head": hash}` or `{"ok": false, "broken_at": seq, "checked": n}`.
    pub fn audit_verify(&self) -> Result<Value> {
        let mut prev = "GENESIS".to_string();
        let mut n = 0i64;
        for r in self
            .db
            .all("SELECT * FROM audit ORDER BY seq ASC", params![])?
        {
            let body = dumps(&json!([
                r["ts"],
                r["actor"],
                r["task_id"],
                r["action"],
                r["resource"],
                r["risk"],
                r["decision"],
                r["result"],
                r["detail"],
                prev
            ]));
            let h = hex::encode(Sha256::digest(body.as_bytes()));
            if field_str(&r, "prev_hash") != prev || h != field_str(&r, "hash") {
                return Ok(json!({"ok": false, "broken_at": r["seq"], "checked": n}));
            }
            prev = field_str(&r, "hash");
            n += 1;
        }
        Ok(json!({"ok": true, "checked": n, "head": prev}))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) fn temp_store() -> (tempfile::TempDir, Store) {
        let d = tempfile::tempdir().unwrap();
        let s = Store::new(d.path()).unwrap();
        (d, s)
    }

    #[test]
    fn audit_chain_and_tamper() {
        let (_d, store) = temp_store();
        for i in 0..5 {
            store
                .audit(
                    "sentinel",
                    &format!("a{i}"),
                    Audit {
                        detail: Some(json!({"i": i})),
                        ..Default::default()
                    },
                )
                .unwrap();
        }
        assert_eq!(store.audit_verify().unwrap()["ok"], true);
        assert!(store
            .db
            .execute("UPDATE audit SET action='x' WHERE seq=2", params![])
            .is_err());
        assert!(store
            .db
            .execute("DELETE FROM audit WHERE seq=2", params![])
            .is_err());
        // tamper by bypassing triggers
        store
            .db
            .execute("DROP TRIGGER audit_no_update", params![])
            .unwrap();
        store
            .db
            .execute("UPDATE audit SET action='evil' WHERE seq=3", params![])
            .unwrap();
        let v = store.audit_verify().unwrap();
        assert_eq!(v["ok"], false);
        assert_eq!(v["broken_at"], 3);
        assert!(
            std::fs::read_to_string(store.dir.join("audit.jsonl"))
                .unwrap()
                .lines()
                .count()
                == 5
        );
    }

    #[test]
    fn audit_hash_matches_python() {
        // body = json.dumps([ts, actor, task_id, action, resource, risk, decision, result, detail_s, prev])
        let body = dumps(&json!([
            1700000000.5,
            "sentinel",
            "",
            "a0",
            "",
            "",
            "",
            "",
            "{\"i\": 0}",
            "GENESIS"
        ]));
        assert_eq!(
            body,
            r#"[1700000000.5, "sentinel", "", "a0", "", "", "", "", "{\"i\": 0}", "GENESIS"]"#
        );
        // hashlib.sha256(body.encode()).hexdigest() in Python
        assert_eq!(
            hex::encode(Sha256::digest(body.as_bytes())),
            "fc170df1be57bfd12e5dff1f354e2bcdbcb9c0da10073d4a0a3d48b1a7a4748c"
        );
    }

    #[test]
    fn vault_encrypts() {
        let (_d, store) = temp_store();
        store
            .put_secret(
                "gmail",
                &json!({"app_password": "abcd efgh ijkl mnop"}),
                None,
            )
            .unwrap();
        let raw = store.secret_blob("cred_gmail_1").unwrap().unwrap();
        assert!(!String::from_utf8_lossy(&raw).contains("abcd efgh"));
        assert_eq!(
            store.get_secret("cred_gmail_1").unwrap().unwrap()["app_password"],
            "abcd efgh ijkl mnop"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(store.dir.join("vault.key"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        assert!(store.has_secret("cred_gmail_1").unwrap());
        store.delete_secret("cred_gmail_1").unwrap();
        assert!(!store.has_secret("cred_gmail_1").unwrap());
        // reopening reads the same key back
        let again = Store::new(&store.dir).unwrap();
        store
            .put_secret("x", &json!({"k": "v"}), Some("h1"))
            .unwrap();
        assert_eq!(again.get_secret("h1").unwrap().unwrap()["k"], "v");
    }

    #[test]
    fn connections_ctx_kv() {
        let (_d, store) = temp_store();
        let g = store.connection("gmail").unwrap();
        assert_eq!(g["enabled"], true);
        assert_eq!(g["permissions"]["send"], true);
        let g = store
            .save_connection("gmail", None, Some(&json!({"send": 0})), None)
            .unwrap();
        assert_eq!(g["permissions"]["send"], false);
        let b = store
            .save_connection(
                "browser",
                Some(&json!({"blocked_domains": ["bad.com"]})),
                None,
                None,
            )
            .unwrap();
        assert_eq!(b["config"]["blocked_domains"], json!(["bad.com"]));
        assert_eq!(b["config"]["allowed_domains"], json!([]));
        assert_eq!(
            store.connection("nope").unwrap(),
            json!({"name": "nope", "config": {}, "permissions": {}, "enabled": false})
        );

        assert_eq!(
            store.task_ctx("t").unwrap(),
            json!({"taint": "PUBLIC", "injection": [], "domains": []})
        );
        store
            .update_task_ctx("t", Some("CONFIDENTIAL"), None, Some("a.com"))
            .unwrap();
        store
            .update_task_ctx(
                "t",
                Some("PERSONAL"),
                Some(&["x".to_string(), "a".to_string()]),
                Some("a.com"),
            )
            .unwrap();
        let c = store.task_ctx("t").unwrap();
        assert_eq!(
            c,
            json!({"taint": "CONFIDENTIAL", "injection": ["a", "x"], "domains": ["a.com"]})
        );

        assert!(store.kv_get("k").unwrap().is_none());
        store.kv_set("k", &json!({"a": [1, 2]})).unwrap();
        assert_eq!(store.kv_get("k").unwrap().unwrap(), json!({"a": [1, 2]}));

        store.set_user_request("t", "hello").unwrap();
        assert_eq!(store.user_request("t").unwrap(), "hello");
        assert_eq!(store.user_request("none").unwrap(), "");
    }

    #[test]
    fn approvals_grants_purchases() {
        let (_d, store) = temp_store();
        let a = store
            .create_approval(
                "t1",
                "c1",
                "gmail_send",
                &json!({"to": "a@b.c"}),
                &json!({"title": "x"}),
                "high",
                "r",
            )
            .unwrap();
        assert_eq!(a["status"], "pending");
        assert_eq!(a["args"]["to"], "a@b.c");
        assert!(a["result"].is_null());
        assert_eq!(store.approvals(Some("pending"), 10).unwrap().len(), 1);
        assert_eq!(store.approvals(Some("resolved"), 10).unwrap().len(), 0);
        store
            .resolve_approval(
                a["id"].as_str().unwrap(),
                "approved",
                "ONCE",
                Some(&json!({"ok": 1})),
                None,
                "user",
            )
            .unwrap();
        let r = store.approvals(Some("resolved"), 10).unwrap();
        assert_eq!(r.len(), 1);
        assert_eq!(r[0]["result"], json!({"ok": 1}));
        assert_eq!(r[0]["decided_by"], "user");
        assert_eq!(store.approvals(None, 10).unwrap().len(), 1);

        let gid = store
            .add_grant(
                "gmail_send",
                "TASK",
                Some("t1"),
                Some(&json!({"destination": "x"})),
                None,
                "",
            )
            .unwrap();
        let gs = store.active_grants().unwrap();
        assert_eq!(gs.len(), 1);
        assert_eq!(gs[0]["match"]["destination"], "x");
        assert!(gs[0]["expires_at"].is_null());
        store
            .add_grant("gmail_send", "TIME_BOUND", None, None, Some(-5.0), "")
            .unwrap();
        assert_eq!(store.active_grants().unwrap().len(), 1); // expired one skipped
        store.revoke_grant(&gid).unwrap();
        assert!(store.active_grants().unwrap().is_empty());

        let pid = store
            .add_purchase("tp", "decathlon.sg", "v1", 13.99, "SGD", &json!({}))
            .unwrap();
        assert!(store
            .active_purchase("tp", "www.decathlon.sg")
            .unwrap()
            .is_some()); // sub-domain matches
        assert!(store.active_purchase("tp", "lazada.sg").unwrap().is_none());
        assert!(store
            .active_purchase("tq", "decathlon.sg")
            .unwrap()
            .is_none());
        let p = store
            .active_purchase("tp", "decathlon.sg")
            .unwrap()
            .unwrap();
        assert_eq!(p["id"], pid);
        assert_eq!(p["clicks_left"], 10);
        store.use_purchase(&pid, "click").unwrap();
        store.use_purchase(&pid, "fill").unwrap();
        let p = store
            .active_purchase("tp", "shop.decathlon.sg")
            .unwrap()
            .unwrap();
        assert_eq!(p["clicks_left"], 9);
        assert_eq!(p["fills_left"], 11);
        store.end_purchase(&pid, "used").unwrap();
        assert!(store
            .active_purchase("tp", "decathlon.sg")
            .unwrap()
            .is_none());

        store
            .audit(
                "sentinel",
                "gmail_archive",
                Audit {
                    task_id: "t9".into(),
                    result: "success".into(),
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(
            store
                .count_done("t9", &["gmail_archive", "gmail_label"])
                .unwrap(),
            1
        );
        assert_eq!(store.count_done("", &["gmail_archive"]).unwrap(), 0);
        let l = store.audit_list(Some("t9"), 10, None, None).unwrap();
        assert_eq!(l.len(), 1);
        assert_eq!(l[0]["detail"], json!({}));
        assert!(store
            .audit_list(None, 10, Some(1), None)
            .unwrap()
            .is_empty());
    }
}
