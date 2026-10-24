//! Multiple mailboxes (Gmail and other providers).
//!
//! Accounts live in the "gmail" connection config (the historical name of the email connector):
//! `{"accounts": [{id, email, display_name, provider, imap_host/port/security, smtp_host/port/security, username, auth}],
//! "default": "g1"}`; each account's secret is its own vault entry (`cred_gmail_<n>` for id `g<n>`): `{"app_password": …}`
//! or, for Outlook, `{"oauth": {client_id, tenant, refresh_token}}`.
//! Message / thread ids of the first account (g1) stay bare numbers (backwards compatible); ids from other
//! accounts are prefixed, e.g. `"g2:1877319910085372507"`, so the agent can pass them around without knowing
//! which mailbox they came from.
//!
//! Port of `app/sentinel/mailboxes.py`. Every function takes the sentinel [`Store`].

use std::sync::LazyLock;

use anyhow::Result;
use regex::Regex;
use serde_json::{json, Map, Value};

use omuse_common::util::{as_i64, py_str, truthy};
use omuse_sentinel_core::Store;

use crate::mailproviders::provider_or_custom;

static ID: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(g\d+):(\S{1,40})$").unwrap());

/// The account fields that are persisted.
pub const FIELDS: &[&str] = &[
    "id",
    "email",
    "display_name",
    "provider",
    "imap_host",
    "imap_port",
    "imap_security",
    "smtp_host",
    "smtp_port",
    "smtp_security",
    "username",
    "auth",
];

fn s(v: &Value, key: &str) -> String {
    match v.get(key) {
        None | Some(Value::Null) => String::new(),
        Some(Value::String(x)) => x.clone(),
        Some(x) => py_str(x),
    }
}

fn truthy_key(v: &Value, key: &str) -> bool {
    v.get(key).map(truthy).unwrap_or(false)
}

fn int_or(v: &Value, key: &str, default: i64) -> i64 {
    match v.get(key) {
        Some(x) if truthy(x) => as_i64(x)
            .or_else(|| x.as_str().and_then(|t| t.trim().parse::<i64>().ok()))
            .unwrap_or(default),
        _ => default,
    }
}

/// Fill in fields that older (Gmail-only) configs do not have.
pub fn defaults(a: &Value) -> Value {
    let mut a = a.clone();
    if !a.is_object() {
        a = json!({});
    }
    if !truthy_key(&a, "provider") && a.get("provider").is_none() {
        a["provider"] = json!("gmail");
    }
    let p = provider_or_custom(&s(&a, "provider"));
    let email = s(&a, "email");
    let dom = email
        .rsplit_once('@')
        .map(|(_, d)| d)
        .unwrap_or(&email)
        .to_lowercase();
    if !truthy_key(&a, "imap_host") {
        a["imap_host"] = json!(p.imap.0.replace("{domain}", &dom));
    }
    if !truthy_key(&a, "smtp_host") {
        a["smtp_host"] = json!(p.smtp.0.replace("{domain}", &dom));
    }
    a["imap_port"] = json!(int_or(&a, "imap_port", p.imap.1 as i64));
    a["smtp_port"] = json!(int_or(&a, "smtp_port", p.smtp.1 as i64));
    if !truthy_key(&a, "imap_security") {
        a["imap_security"] = json!(p.imap.2);
    }
    if !truthy_key(&a, "smtp_security") {
        a["smtp_security"] = json!(p.smtp.2);
    }
    if !truthy_key(&a, "username") {
        a["username"] = json!(email);
    }
    if !truthy_key(&a, "auth") {
        a["auth"] = json!(if p.auth == "oauth" {
            "oauth"
        } else {
            "password"
        });
    }
    a
}

/// Vault handle of an account id (`g2` → `cred_gmail_2`).
pub fn handle(aid: &str) -> String {
    format!("cred_gmail_{}", aid.get(1..).unwrap_or(""))
}

fn config(store: &Store) -> Result<Value> {
    Ok(store.connection("gmail")?["config"].take())
}

/// All configured accounts with defaults filled in and `"ready"` (= has a secret).
pub fn accounts(store: &Store) -> Result<Vec<Value>> {
    let cfg = config(store)?;
    let accs: Vec<Value> = match cfg.get("accounts") {
        Some(Value::Array(a)) => a.clone(),
        Some(Value::Null) | None => {
            // legacy single-account config
            if truthy_key(&cfg, "email") {
                let imap = s(&cfg, "imap_host");
                let smtp = s(&cfg, "smtp_host");
                vec![json!({
                    "id": "g1", "email": cfg["email"], "display_name": s(&cfg, "display_name"),
                    "imap_host": if imap.is_empty() { "imap.gmail.com".to_string() } else { imap },
                    "smtp_host": if smtp.is_empty() { "smtp.gmail.com".to_string() } else { smtp },
                })]
            } else {
                vec![]
            }
        }
        Some(_) => vec![],
    };
    let mut out = Vec::with_capacity(accs.len());
    for a in &accs {
        let mut a = defaults(a);
        a["ready"] = json!(store.has_secret(&handle(&s(&a, "id")))?);
        out.push(a);
    }
    Ok(out)
}

/// Accounts that have a credential.
pub fn ready_accounts(store: &Store) -> Result<Vec<Value>> {
    Ok(accounts(store)?
        .into_iter()
        .filter(|a| truthy_key(a, "ready"))
        .collect())
}

/// The default account id ("" when no mailbox is ready).
pub fn default_id(store: &Store) -> Result<String> {
    let accs = ready_accounts(store)?;
    let want = s(&config(store)?, "default");
    if !want.is_empty() && accs.iter().any(|a| s(a, "id") == want) {
        return Ok(want);
    }
    Ok(accs.first().map(|a| s(a, "id")).unwrap_or_default())
}

/// Account by id ("g2"), email address, or None/"" for the default account.
pub fn find(store: &Store, reference: Option<&str>) -> Result<Option<Value>> {
    let accs = ready_accounts(store)?;
    let mut r = reference.unwrap_or("").to_string();
    if r.is_empty() {
        r = default_id(store)?;
    }
    let r = r.trim().to_lowercase();
    Ok(accs
        .into_iter()
        .find(|a| s(a, "id") == r || s(a, "email").to_lowercase() == r))
}

/// `"g2:123"` → `("g2", "123")`; a bare id belongs to g1.
pub fn split_id(mid: &str) -> (String, String) {
    let mid = mid.trim();
    match ID.captures(mid) {
        Some(m) => (m[1].to_string(), m[2].to_string()),
        None => ("g1".to_string(), mid.to_string()),
    }
}

/// Prefix a raw id with the account (g1 stays bare).
pub fn make_id(aid: &str, raw: &str) -> String {
    if aid == "g1" {
        raw.to_string()
    } else {
        format!("{aid}:{raw}")
    }
}

fn write(store: &Store, accs: &[Value], default: Option<&str>) -> Result<()> {
    let clean: Vec<Value> = accs
        .iter()
        .map(|a| {
            let mut m = Map::new();
            for k in FIELDS {
                m.insert(
                    k.to_string(),
                    a.get(*k).cloned().unwrap_or_else(|| json!("")),
                );
            }
            Value::Object(m)
        })
        .collect();
    let mut cfg = json!({"accounts": clean});
    if let Some(d) = default {
        cfg["default"] = json!(d);
    }
    let dflt = match default {
        Some(d) => d.to_string(),
        None => s(&config(store)?, "default"),
    };
    let clean = cfg["accounts"].as_array().cloned().unwrap_or_default();
    let prim = clean
        .iter()
        .find(|a| s(a, "id") == dflt)
        .or_else(|| clean.first());
    // keep the legacy single-account fields pointing at the default account
    cfg["email"] = json!(prim.map(|p| s(p, "email")).unwrap_or_default());
    cfg["display_name"] = json!(prim.map(|p| s(p, "display_name")).unwrap_or_default());
    store.save_connection("gmail", Some(&cfg), None, None)?;
    Ok(())
}

/// Add a mailbox, or update it when the email address is already connected (same address = new password).
pub fn save_account(
    store: &Store,
    email: &str,
    app_password: &str,
    display_name: &str,
    provider: &str,
    servers: Option<&Value>,
    oauth: Option<&Value>,
) -> Result<Value> {
    let mut accs = accounts(store)?;
    let idx = match accs
        .iter()
        .position(|a| s(a, "email").to_lowercase() == email.to_lowercase())
    {
        Some(i) => i,
        None => {
            let n = accs
                .iter()
                .filter_map(|a| s(a, "id").get(1..).and_then(|x| x.parse::<i64>().ok()))
                .max()
                .unwrap_or(0)
                + 1;
            accs.push(json!({"id": format!("g{n}"), "email": email}));
            accs.len() - 1
        }
    };
    {
        let acc = &mut accs[idx];
        let dn = if display_name.is_empty() {
            s(acc, "display_name")
        } else {
            display_name.to_string()
        };
        acc["display_name"] = json!(dn);
        acc["provider"] = json!(provider);
        if let Some(obj) = acc.as_object_mut() {
            for k in [
                "imap_host",
                "imap_port",
                "imap_security",
                "smtp_host",
                "smtp_port",
                "smtp_security",
                "username",
            ] {
                obj.remove(k);
            }
            if let Some(Value::Object(srv)) = servers {
                for (k, v) in srv {
                    obj.insert(k.clone(), v.clone());
                }
            }
        }
        acc["auth"] = json!(if oauth.is_some() { "oauth" } else { "password" });
        let filled = defaults(acc);
        if let (Some(dst), Some(src)) = (acc.as_object_mut(), filled.as_object()) {
            for (k, v) in src {
                dst.insert(k.clone(), v.clone());
            }
        }
    }
    let acc = accs[idx].clone();
    let secret = match oauth {
        Some(o) => json!({"oauth": o}),
        None => json!({"app_password": app_password}),
    };
    let aid = s(&acc, "id");
    store.put_secret("gmail", &secret, Some(&handle(&aid)))?;
    let mut dflt = s(&config(store)?, "default");
    if dflt.is_empty() {
        dflt = accs.first().map(|a| s(a, "id")).unwrap_or_default(); // first mailbox stays default
    }
    if !accs.iter().any(|a| s(a, "id") == dflt) {
        dflt = aid;
    }
    write(store, &accs, Some(&dflt))?;
    store.save_connection("gmail", None, None, Some(true))?;
    Ok(acc)
}

/// Remove a mailbox and its secret; the default moves to the first remaining account.
pub fn remove_account(store: &Store, aid: &str) -> Result<()> {
    let accs: Vec<Value> = accounts(store)?
        .into_iter()
        .filter(|a| s(a, "id") != aid)
        .collect();
    store.delete_secret(&handle(aid))?;
    let mut dflt = s(&config(store)?, "default");
    if dflt == aid || !accs.iter().any(|a| s(a, "id") == dflt) {
        dflt = accs.first().map(|a| s(a, "id")).unwrap_or_default();
    }
    write(store, &accs, Some(&dflt))
}

/// Make `aid` the default mailbox. `Ok(false)` when there is no such account (Python raised `KeyError`).
pub fn set_default(store: &Store, aid: &str) -> Result<bool> {
    let accs = accounts(store)?;
    if !accs.iter().any(|a| s(a, "id") == aid) {
        return Ok(false);
    }
    write(store, &accs, Some(aid))?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mailproviders::server_settings;

    fn store() -> (tempfile::TempDir, Store) {
        let dir = tempfile::tempdir().unwrap();
        let st = Store::new(dir.path()).unwrap();
        (dir, st)
    }

    #[test]
    fn legacy_migration_and_ids() {
        let (_d, st) = store();
        st.put_secret("gmail", &json!({"app_password": "abcdabcdabcdabcd"}), None)
            .unwrap(); // legacy cred_gmail_1
        st.save_connection(
            "gmail",
            Some(&json!({"email": "lucas@x.com", "display_name": "Lucas"})),
            None,
            None,
        )
        .unwrap();
        let accs = accounts(&st).unwrap();
        assert_eq!(
            accs.iter().map(|a| s(a, "id")).collect::<Vec<_>>(),
            vec!["g1"]
        );
        assert!(truthy_key(&accs[0], "ready"));
        assert_eq!(default_id(&st).unwrap(), "g1");
        let acc2 = save_account(
            &st,
            "work@y.com",
            "efghefghefghefgh",
            "Lucas Work",
            "gmail",
            None,
            None,
        )
        .unwrap();
        assert_eq!(acc2["id"], "g2");
        assert!(st.has_secret("cred_gmail_2").unwrap());
        assert_eq!(find(&st, Some("WORK@y.com")).unwrap().unwrap()["id"], "g2");
        assert_eq!(find(&st, None).unwrap().unwrap()["id"], "g1");
        assert_eq!(make_id("g1", "123456"), "123456");
        assert_eq!(make_id("g2", "123456"), "g2:123456");
        assert_eq!(
            split_id("g2:123456"),
            ("g2".to_string(), "123456".to_string())
        );
        assert_eq!(split_id("123456"), ("g1".to_string(), "123456".to_string()));
        assert!(set_default(&st, "g2").unwrap());
        assert!(!set_default(&st, "g9").unwrap());
        assert_eq!(default_id(&st).unwrap(), "g2");
        assert_eq!(
            st.connection("gmail").unwrap()["config"]["email"],
            "work@y.com"
        );
        assert_eq!(
            save_account(
                &st,
                "work@y.com",
                "zzzzzzzzzzzzzzzz",
                "",
                "gmail",
                None,
                None
            )
            .unwrap()["id"],
            "g2"
        ); // same email = update
        remove_account(&st, "g2").unwrap();
        assert_eq!(
            accounts(&st)
                .unwrap()
                .iter()
                .map(|a| s(a, "id"))
                .collect::<Vec<_>>(),
            vec!["g1"]
        );
        assert_eq!(default_id(&st).unwrap(), "g1");
        assert!(!st.has_secret("cred_gmail_2").unwrap());
    }

    #[test]
    fn provider_fields() {
        let (_d, st) = store();
        st.put_secret("gmail", &json!({"app_password": "abcdabcdabcdabcd"}), None)
            .unwrap();
        st.save_connection(
            "gmail",
            Some(&json!({"email": "lucas@gmail.com"})),
            None,
            None,
        )
        .unwrap();
        let a = &accounts(&st).unwrap()[0];
        assert_eq!(a["provider"], "gmail");
        assert_eq!(a["imap_port"], 993);
        assert_eq!(a["smtp_security"], "ssl");
        assert_eq!(a["auth"], "password");
        let q = save_account(
            &st,
            "me@qq.com",
            "authcode",
            "",
            "qq",
            Some(&server_settings("qq", "me@qq.com", None).unwrap()),
            None,
        )
        .unwrap();
        let o = save_account(
            &st,
            "me@outlook.com",
            "",
            "",
            "outlook",
            Some(&server_settings("outlook", "me@outlook.com", None).unwrap()),
            Some(&json!({"client_id": "c", "tenant": "consumers", "refresh_token": "r"})),
        )
        .unwrap();
        let accs = accounts(&st).unwrap();
        let qq = accs.iter().find(|x| s(x, "email") == "me@qq.com").unwrap();
        let ol = accs
            .iter()
            .find(|x| s(x, "email") == "me@outlook.com")
            .unwrap();
        assert_eq!(qq["imap_host"], "imap.qq.com");
        assert_eq!(ol["auth"], "oauth");
        assert_eq!(
            st.get_secret(&handle(&s(&o, "id"))).unwrap().unwrap()["oauth"]["refresh_token"],
            "r"
        );
        let qid = s(&q, "id");
        assert_eq!(
            split_id(&format!("{qid}:i-42")),
            (qid.clone(), "i-42".to_string())
        );
        assert_eq!(split_id("i-42"), ("g1".to_string(), "i-42".to_string()));
        assert_eq!(ready_accounts(&st).unwrap().len(), 3);
    }
}
