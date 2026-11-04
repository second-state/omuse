//! Several Google accounts per service: every "Connect" on the Calendar or the Drive card adds one (the same email
//! again updates it), like the mailboxes of the email connector.
//!
//! The accounts live in the service's connection config: `{"accounts": [{id, email, mode, time_zone, calendar_name}],
//! "default": "c1"}` — ids `c<n>` for calendars, `d<n>` for drives. Each account's secret is its own vault entry
//! `cred_<service>_<n>` (the refresh token and its client, or an iCal address); its cached access token is
//! `cred_<service>_access` for the first account and `cred_<service>_access_<n>` for the others. The first account
//! keeps the handles of the single-account versions, so a connection made before 0.6.7 is account 1 as it is.
//! Ids of things from an account other than the first are prefixed (`"c2:<event id>"`, `"d2:<file id>"`), so the agent
//! passes them around without knowing which account they came from.

use crate::Store;
use omuse_common::util::get_str;
use serde_json::{json, Map, Value};

type Result<T> = anyhow::Result<T>;

/// The services with accounts.
pub const SERVICES: [&str; 2] = ["calendar", "drive"];

/// The fields an account keeps in the connection config.
const FIELDS: [&str; 5] = ["id", "email", "mode", "time_zone", "calendar_name"];

/// The id letter of a service's accounts (`c` calendar, `d` drive).
pub fn letter(service: &str) -> char {
    if service == "drive" {
        'd'
    } else {
        'c'
    }
}

/// The number of an account id (`c2` → 2); 1 for anything else.
fn number(aid: &str) -> u32 {
    aid.get(1..)
        .and_then(|n| n.parse().ok())
        .filter(|n| *n > 0)
        .unwrap_or(1)
}

/// The vault handle of an account's secret (`c2` → `cred_calendar_2`).
pub fn handle(service: &str, aid: &str) -> String {
    format!("cred_{service}_{}", number(aid))
}

/// The vault handle of an account's cached access token (the first keeps `cred_<service>_access`).
pub fn access_handle(service: &str, aid: &str) -> String {
    match number(aid) {
        1 => format!("cred_{service}_access"),
        n => format!("cred_{service}_access_{n}"),
    }
}

fn config(store: &Store, service: &str) -> Result<Value> {
    Ok(store.connection(service)?["config"].take())
}

/// All accounts of a service, each with `ready` (= its secret is in the vault). A config from before 0.6.7 (one
/// account, its fields at the top) is account 1.
pub fn accounts(store: &Store, service: &str) -> Result<Vec<Value>> {
    let cfg = config(store, service)?;
    let list: Vec<Value> = match cfg.get("accounts") {
        Some(Value::Array(a)) => a.clone(),
        _ if store.has_secret(&format!("cred_{service}_1"))? => {
            let mut a = Map::new();
            a.insert("id".into(), json!(format!("{}1", letter(service))));
            for k in &FIELDS[1..] {
                a.insert((*k).into(), json!(get_str(&cfg, k)));
            }
            vec![Value::Object(a)]
        }
        _ => vec![],
    };
    let mut out = Vec::with_capacity(list.len());
    for mut a in list {
        let ready = store.has_secret(&handle(service, &get_str(&a, "id")))?;
        a["ready"] = json!(ready);
        out.push(a);
    }
    Ok(out)
}

/// The accounts that have their secret.
pub fn ready(store: &Store, service: &str) -> Result<Vec<Value>> {
    Ok(accounts(store, service)?
        .into_iter()
        .filter(|a| a["ready"] == json!(true))
        .collect())
}

/// The default account's id ("" when none is ready).
pub fn default_id(store: &Store, service: &str) -> Result<String> {
    let accs = ready(store, service)?;
    let want = get_str(&config(store, service)?, "default");
    if !want.is_empty() && accs.iter().any(|a| get_str(a, "id") == want) {
        return Ok(want);
    }
    Ok(accs.first().map(|a| get_str(a, "id")).unwrap_or_default())
}

/// An account by id (`c2`), email address, or None / "" for the default one.
pub fn find(store: &Store, service: &str, reference: Option<&str>) -> Result<Option<Value>> {
    let mut r = reference.unwrap_or("").trim().to_lowercase();
    if r.is_empty() {
        r = default_id(store, service)?;
    }
    Ok(ready(store, service)?
        .into_iter()
        .find(|a| get_str(a, "id") == r || get_str(a, "email").to_lowercase() == r))
}

/// `"c2:abc"` → `("c2", "abc")`; a bare id belongs to the service's first account.
pub fn split_id(service: &str, id: &str) -> (String, String) {
    let id = id.trim();
    let l = letter(service);
    if let Some((aid, raw)) = id.split_once(':') {
        if aid.len() >= 2
            && aid.starts_with(l)
            && aid[1..].bytes().all(|b| b.is_ascii_digit())
            && !raw.is_empty()
        {
            return (aid.to_string(), raw.to_string());
        }
    }
    (format!("{l}1"), id.to_string())
}

/// An id of an account's thing, prefixed with the account (the first stays bare).
pub fn make_id(aid: &str, raw: &str) -> String {
    if number(aid) == 1 {
        raw.to_string()
    } else {
        format!("{aid}:{raw}")
    }
}

fn write(store: &Store, service: &str, accs: &[Value], default: &str) -> Result<()> {
    let clean: Vec<Value> = accs
        .iter()
        .map(|a| {
            Value::Object(
                FIELDS
                    .iter()
                    .map(|k| ((*k).to_string(), json!(get_str(a, k))))
                    .collect(),
            )
        })
        .collect();
    let prim = clean
        .iter()
        .find(|a| get_str(a, "id") == default)
        .or_else(|| clean.first())
        .cloned()
        .unwrap_or_else(|| json!({}));
    // the single-account fields stay those of the default account (older readers, the card's summary)
    let mut cfg = json!({"accounts": clean, "default": default});
    for k in &FIELDS[1..] {
        cfg[*k] = json!(get_str(&prim, k));
    }
    store.save_connection(service, Some(&cfg), None, Some(!accs.is_empty()))?;
    Ok(())
}

/// Add an account, or update the one with the same email (a new sign-in for it): its secret goes to the vault, its
/// fields (`mode`, `time_zone`, `calendar_name`) to the config, its cached access token is dropped. The first account
/// stays the default. Returns the account.
pub fn save_account(
    store: &Store,
    service: &str,
    email: &str,
    secret: &Value,
    fields: &Value,
) -> Result<Value> {
    let mut accs = accounts(store, service)?;
    let at = match accs.iter().position(|a| {
        !email.is_empty() && get_str(a, "email").to_lowercase() == email.to_lowercase()
    }) {
        Some(i) => i,
        None => {
            let n = accs
                .iter()
                .map(|a| number(&get_str(a, "id")))
                .max()
                .unwrap_or(0)
                + 1;
            accs.push(json!({"id": format!("{}{n}", letter(service)), "email": email}));
            accs.len() - 1
        }
    };
    if let (Some(dst), Some(src)) = (accs[at].as_object_mut(), fields.as_object()) {
        for (k, v) in src {
            if FIELDS[1..].contains(&k.as_str()) {
                dst.insert(k.clone(), v.clone());
            }
        }
    }
    let acc = accs[at].clone();
    let aid = get_str(&acc, "id");
    store.put_secret(service, secret, Some(&handle(service, &aid)))?;
    store.delete_secret(&access_handle(service, &aid))?;
    let mut dflt = get_str(&config(store, service)?, "default");
    if !accs.iter().any(|a| get_str(a, "id") == dflt) {
        dflt = get_str(&accs[0], "id");
    }
    write(store, service, &accs, &dflt)?;
    Ok(acc)
}

/// Remove an account, its secret and its cached token; the default moves to the first remaining account.
pub fn remove_account(store: &Store, service: &str, aid: &str) -> Result<()> {
    let accs: Vec<Value> = accounts(store, service)?
        .into_iter()
        .filter(|a| get_str(a, "id") != aid)
        .collect();
    store.delete_secret(&handle(service, aid))?;
    store.delete_secret(&access_handle(service, aid))?;
    let mut dflt = get_str(&config(store, service)?, "default");
    if dflt == aid || !accs.iter().any(|a| get_str(a, "id") == dflt) {
        dflt = accs.first().map(|a| get_str(a, "id")).unwrap_or_default();
    }
    write(store, service, &accs, &dflt)
}

/// Make `aid` the default account; `false` when there is no such account.
pub fn set_default(store: &Store, service: &str, aid: &str) -> Result<bool> {
    let accs = accounts(store, service)?;
    if !accs.iter().any(|a| get_str(a, "id") == aid) {
        return Ok(false);
    }
    write(store, service, &accs, aid)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> (tempfile::TempDir, Store) {
        let dir = tempfile::tempdir().unwrap();
        let st = Store::new(dir.path()).unwrap();
        (dir, st)
    }

    #[test]
    fn each_connect_adds_an_account_and_the_same_email_updates_it() {
        let (_d, st) = store();
        let a = save_account(
            &st,
            "calendar",
            "me@example.com",
            &json!({"refresh_token": "r1"}),
            &json!({"mode": "google", "time_zone": "Asia/Singapore"}),
        )
        .unwrap();
        assert_eq!(get_str(&a, "id"), "c1");
        let b = save_account(
            &st,
            "calendar",
            "work@example.com",
            &json!({"refresh_token": "r2"}),
            &json!({"mode": "google"}),
        )
        .unwrap();
        assert_eq!(get_str(&b, "id"), "c2");
        assert_eq!(
            st.get_secret("cred_calendar_2").unwrap().unwrap()["refresh_token"],
            "r2"
        );
        // the same email again: the same account, a new token
        let again = save_account(
            &st,
            "calendar",
            "WORK@example.com",
            &json!({"refresh_token": "r3"}),
            &json!({}),
        )
        .unwrap();
        assert_eq!(get_str(&again, "id"), "c2");
        assert_eq!(accounts(&st, "calendar").unwrap().len(), 2);
        assert_eq!(
            st.get_secret("cred_calendar_2").unwrap().unwrap()["refresh_token"],
            "r3"
        );
        // the first stays the default; find by email or id
        assert_eq!(default_id(&st, "calendar").unwrap(), "c1");
        assert_eq!(
            get_str(
                &find(&st, "calendar", Some("work@example.com"))
                    .unwrap()
                    .unwrap(),
                "id"
            ),
            "c2"
        );
        assert_eq!(
            get_str(&find(&st, "calendar", None).unwrap().unwrap(), "email"),
            "me@example.com"
        );
        // the card's single-account fields are the default's
        assert_eq!(
            st.connection("calendar").unwrap()["config"]["time_zone"],
            "Asia/Singapore"
        );
        // drives count on their own
        let d = save_account(
            &st,
            "drive",
            "me@example.com",
            &json!({"refresh_token": "d"}),
            &json!({}),
        )
        .unwrap();
        assert_eq!(
            (get_str(&d, "id"), handle("drive", "d1")),
            ("d1".to_string(), "cred_drive_1".to_string())
        );
    }

    #[test]
    fn removing_the_default_moves_it_and_a_pre_0_6_7_connection_is_account_1() {
        let (_d, st) = store();
        // a connection from 0.6.6: one secret, the fields at the top of the config
        st.put_secret("calendar", &json!({"refresh_token": "old"}), None)
            .unwrap();
        st.save_connection(
            "calendar",
            Some(&json!({"email": "old@example.com", "mode": "google"})),
            None,
            Some(true),
        )
        .unwrap();
        let accs = accounts(&st, "calendar").unwrap();
        assert_eq!(accs.len(), 1);
        assert_eq!(
            (get_str(&accs[0], "id"), get_str(&accs[0], "email")),
            ("c1".into(), "old@example.com".into())
        );
        save_account(
            &st,
            "calendar",
            "new@example.com",
            &json!({"refresh_token": "n"}),
            &json!({}),
        )
        .unwrap();
        assert!(set_default(&st, "calendar", "c2").unwrap());
        remove_account(&st, "calendar", "c2").unwrap();
        assert_eq!(default_id(&st, "calendar").unwrap(), "c1");
        assert!(st.get_secret("cred_calendar_2").unwrap().is_none());
        remove_account(&st, "calendar", "c1").unwrap();
        assert!(accounts(&st, "calendar").unwrap().is_empty());
        assert_eq!(default_id(&st, "calendar").unwrap(), "");
    }

    #[test]
    fn ids_carry_their_account_except_the_first() {
        assert_eq!(make_id("c1", "ev1"), "ev1");
        assert_eq!(make_id("c3", "ev1"), "c3:ev1");
        assert_eq!(split_id("calendar", "c3:ev1"), ("c3".into(), "ev1".into()));
        assert_eq!(split_id("calendar", "ev1"), ("c1".into(), "ev1".into()));
        // another service's prefix, or a colon inside an id, is part of the id
        assert_eq!(split_id("drive", "c3:ev1"), ("d1".into(), "c3:ev1".into()));
        assert_eq!(split_id("drive", "d2:1AbC"), ("d2".into(), "1AbC".into()));
        assert_eq!(access_handle("drive", "d1"), "cred_drive_access");
        assert_eq!(access_handle("drive", "d2"), "cred_drive_access_2");
    }
}
