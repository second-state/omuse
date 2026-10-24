//! Vault: ID / membership / card numbers the user stores once and approves on every single use.
//!
//! Values are Fernet-encrypted in the `secrets` table (handle `vault_<id>`); only metadata lives in
//! `vault_items`. Nothing here ever returns a value to the runtime or the web UI: the agent asks for a
//! fill with `browser_fill_secret(ref, item_id, field)`, the user approves that one fill, and Sentinel
//! types the value into the page itself. Afterwards every page text that leaves Sentinel is scrubbed of
//! vault numbers, and vision (browser_look / browser_locate) is refused on a page that holds a filled value.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, LazyLock};

use anyhow::Result;
use fancy_regex::Regex as FRegex;
use parking_lot::Mutex;
use regex::Regex;
use serde_json::{json, Map, Value};

use omuse_common::params;
use omuse_common::pyjson::dumps;
use omuse_common::util::{get_str, new_id, now_ts, py_str, take_chars, truthy};

use crate::store::{field_str, loads_field, Store};

/// Item kinds: `(kind, label, fields)` in the Python `KINDS` order.
pub const KINDS: &[(&str, &str, &[&str])] = &[
    (
        "id_document",
        "证件 ID document",
        &["number", "name", "expiry", "country"],
    ),
    (
        "membership",
        "会员 Membership",
        &["number", "name", "program"],
    ),
    (
        "card",
        "银行卡 Card",
        &["number", "expiry", "cvc", "holder"],
    ),
    ("other", "其他 Other", &["value"]),
];

/// Field labels shown in approval cards.
pub const FIELD_LABELS: &[(&str, &str)] = &[
    ("number", "号码 number"),
    ("name", "姓名 name"),
    ("expiry", "有效期 expiry"),
    ("country", "国家 country"),
    ("program", "计划 program"),
    ("cvc", "安全码 CVC"),
    ("holder", "持卡人 holder"),
    ("value", "内容 value"),
];

/// Fields whose values are scrubbed from page text after use (short or common values like CVC / names are not,
/// snapshot.js already hides card-security fields).
pub const SCRUB_FIELDS: &[&str] = &["number", "value"];

/// The `vault_items` table.
pub const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS vault_items (
  id TEXT PRIMARY KEY, label TEXT, kind TEXT, masked TEXT, fields TEXT, domains TEXT,
  created_at REAL, updated_at REAL, last_used REAL, uses INTEGER DEFAULT 0
);
";

/// `KINDS` as the Python dict (`{"card": {"label": .., "fields": [..]}, ..}`), e.g. for the settings UI.
pub fn kinds() -> Value {
    let mut m = Map::new();
    for (k, label, fields) in KINDS {
        m.insert(k.to_string(), json!({"label": label, "fields": fields}));
    }
    Value::Object(m)
}

fn kind_fields(kind: &str) -> Option<&'static [&'static str]> {
    KINDS
        .iter()
        .find(|(k, _, _)| *k == kind)
        .map(|(_, _, f)| *f)
}

fn kind_label(kind: &str) -> String {
    KINDS
        .iter()
        .find(|(k, _, _)| *k == kind)
        .map(|(_, l, _)| l.to_string())
        .unwrap_or_else(|| kind.to_string())
}

fn field_label(field: &str) -> String {
    FIELD_LABELS
        .iter()
        .find(|(f, _)| *f == field)
        .map(|(_, l)| l.to_string())
        .unwrap_or_else(|| field.to_string())
}

/// Errors of [`save_item`]: `Value` is Python's `ValueError` (bad input, 400), `Key` its `KeyError` (unknown id, 404).
#[derive(Debug, thiserror::Error)]
pub enum VaultError {
    #[error("{0}")]
    Value(String),
    #[error("{0}")]
    Key(String),
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

/// Create the vault table (called by `Store::new`).
pub fn init(store: &Store) -> Result<()> {
    store.db.script(SCHEMA)
}

static SPACE_DASH: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[\s-]").unwrap());

fn mask(v: &str) -> String {
    let s = SPACE_DASH.replace_all(v, "").into_owned();
    let n = s.chars().count();
    if n == 0 {
        String::new()
    } else if n > 4 {
        format!("•••• {}", omuse_common::util::last_chars(&s, 4))
    } else {
        "•".repeat(n)
    }
}

fn clean_domains(v: Option<&Value>) -> Vec<String> {
    static SPLIT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[\s,]+").unwrap());
    static SCHEME: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^https?://").unwrap());
    static OK: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[a-z0-9.-]+$").unwrap());
    let items: Vec<String> = match v {
        Some(Value::String(s)) => SPLIT.split(s).map(str::to_string).collect(),
        Some(Value::Array(a)) => a.iter().map(py_str).collect(),
        _ => Vec::new(),
    };
    let mut out = Vec::new();
    for d in items {
        let d = d.trim().to_lowercase();
        let d = SCHEME.replace(&d, "").into_owned();
        let d = d.split('/').next().unwrap_or("");
        let d = d.strip_prefix("www.").unwrap_or(d);
        if !d.is_empty() && OK.is_match(d) {
            out.push(d.to_string());
        }
    }
    out.truncate(20);
    out
}

fn row(r: &Value) -> Value {
    let kind = field_str(r, "kind");
    json!({
        "id": r["id"], "label": r["label"], "kind": r["kind"], "kind_label": kind_label(&kind),
        "masked": r["masked"], "fields": loads_field(r, "fields", json!([])), "domains": loads_field(r, "domains", json!([])),
        "created_at": r["created_at"], "updated_at": r["updated_at"], "last_used": r["last_used"],
        "uses": if truthy(&r["uses"]) { r["uses"].clone() } else { json!(0) },
    })
}

/// All items (metadata only), oldest first.
pub fn list_items(store: &Store) -> Result<Vec<Value>> {
    Ok(store
        .db
        .all("SELECT * FROM vault_items ORDER BY created_at", params![])?
        .iter()
        .map(row)
        .collect())
}

/// One item (metadata only) or `None`.
pub fn item(store: &Store, iid: &str) -> Result<Option<Value>> {
    Ok(store
        .db
        .one("SELECT * FROM vault_items WHERE id=?", params![iid])?
        .as_ref()
        .map(row))
}

/// The fields of an item as a `Vec<String>`.
pub fn item_fields(it: &Value) -> Vec<String> {
    it["fields"]
        .as_array()
        .map(|a| a.iter().map(py_str).collect())
        .unwrap_or_default()
}

/// Create or update an item. On update, empty field values keep the stored ones.
pub fn save_item(
    store: &Store,
    data: &Value,
    iid: Option<&str>,
) -> std::result::Result<Value, VaultError> {
    let kind = {
        let k = get_str(data, "kind");
        if k.is_empty() {
            "other".to_string()
        } else {
            k
        }
    };
    let Some(fields) = kind_fields(&kind) else {
        return Err(VaultError::Value("unknown kind".into()));
    };
    let label = take_chars(get_str(data, "label").trim(), 80).to_string();
    if label.is_empty() {
        return Err(VaultError::Value("请填写名称 label is required".into()));
    }
    let old = match iid {
        Some(id) => store
            .get_secret(&format!("vault_{id}"))?
            .unwrap_or_else(|| json!({})),
        None => json!({}),
    };
    if let Some(id) = iid {
        if item(store, id)?.is_none() {
            return Err(VaultError::Key(id.to_string()));
        }
    }
    let values = data
        .get("values")
        .filter(|v| truthy(v))
        .cloned()
        .unwrap_or_else(|| json!({}));
    let mut vals = Map::new();
    for f in fields {
        let v = take_chars(get_str(&values, f).trim(), 200).to_string();
        let v = if v.is_empty() { get_str(&old, f) } else { v };
        vals.insert(f.to_string(), Value::String(v));
    }
    let main = if kind == "other" { "value" } else { "number" };
    if !truthy(&vals[main]) {
        return Err(VaultError::Value(
            "请填写号码 the number is required".into(),
        ));
    }
    let iid = iid.map(str::to_string).unwrap_or_else(|| new_id("vlt"));
    let vals = Value::Object(vals);
    store.put_secret("vault", &vals, Some(&format!("vault_{iid}")))?;
    let present: Vec<&str> = vals
        .as_object()
        .expect("object")
        .iter()
        .filter(|(_, v)| truthy(v))
        .map(|(f, _)| f.as_str())
        .collect();
    let masked = mask(&get_str(&vals, main));
    let mut rec = json!({
        "label": label, "kind": kind, "masked": masked, "fields": dumps(&json!(present)),
        "domains": dumps(&json!(clean_domains(data.get("domains")))), "updated_at": now_ts(),
    });
    if store
        .db
        .one("SELECT id FROM vault_items WHERE id=?", params![&iid])?
        .is_some()
    {
        store.db.update("vault_items", "id", iid.as_str(), &rec)?;
    } else {
        let o = rec.as_object_mut().expect("object");
        o.insert("id".into(), json!(iid));
        o.insert("created_at".into(), json!(now_ts()));
        o.insert("last_used".into(), Value::Null);
        o.insert("uses".into(), json!(0));
        store.db.insert("vault_items", &rec)?;
    }
    *store.vault_patterns.lock() = None;
    Ok(item(store, &iid)?.unwrap_or(Value::Null))
}

/// Delete an item and its secret; `false` when it does not exist.
pub fn delete_item(store: &Store, iid: &str) -> Result<bool> {
    if item(store, iid)?.is_none() {
        return Ok(false);
    }
    store
        .db
        .execute("DELETE FROM vault_items WHERE id=?", params![iid])?;
    store.delete_secret(&format!("vault_{iid}"))?;
    *store.vault_patterns.lock() = None;
    Ok(true)
}

/// The stored value of one field (`""` when missing). Only Sentinel's own fill code calls this.
pub fn value(store: &Store, iid: &str, field: &str) -> Result<String> {
    let sec = store
        .get_secret(&format!("vault_{iid}"))?
        .unwrap_or_else(|| json!({}));
    Ok(get_str(&sec, field))
}

/// Bump `uses` / `last_used`.
pub fn mark_used(store: &Store, iid: &str) -> Result<()> {
    store.db.execute(
        "UPDATE vault_items SET last_used=?, uses=COALESCE(uses,0)+1 WHERE id=?",
        params![now_ts(), iid],
    )?;
    Ok(())
}

/// May this item be used on `dom` (no domain list = anywhere)?
pub fn domain_ok(it: &Value, dom: &str) -> bool {
    let allowed: Vec<String> = it["domains"]
        .as_array()
        .map(|a| a.iter().map(py_str).collect())
        .unwrap_or_default();
    if allowed.is_empty() {
        return true;
    }
    let dom = dom.to_lowercase();
    let dom = dom.strip_prefix("www.").unwrap_or(&dom);
    allowed
        .iter()
        .any(|d| dom == d || dom.ends_with(&format!(".{d}")))
}

// ------------------------------------------------------------------ after-fill protection
struct Filled {
    urls: HashSet<String>,
    ts: f64,
}

static FILLED: LazyLock<Mutex<HashMap<String, Filled>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

fn page_key(url: &str) -> String {
    url.split('#').next().unwrap_or("").to_string()
}

/// Remember that a vault value was typed into this page of this task.
pub fn note_fill(task_id: &str, url: &str) {
    let mut m = FILLED.lock();
    let rec = m.entry(task_id.to_string()).or_insert_with(|| Filled {
        urls: HashSet::new(),
        ts: 0.0,
    });
    rec.urls.insert(page_key(url));
    rec.ts = now_ts();
}

/// Was a vault value filled on this page (same URL without fragment) within the last 6 hours?
pub fn filled_here(task_id: &str, url: &str) -> bool {
    let mut m = FILLED.lock();
    let Some(rec) = m.get(task_id) else {
        return false;
    };
    if now_ts() - rec.ts > 6.0 * 3600.0 {
        m.remove(task_id);
        return false;
    }
    rec.urls.contains(&page_key(url))
}

fn compile(store: &Store) -> Result<Arc<Vec<FRegex>>> {
    static ALNUM: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[A-Za-z0-9]+$").unwrap());
    if let Some(p) = store.vault_patterns.lock().as_ref() {
        return Ok(p.clone());
    }
    let mut pats = Vec::new();
    for it in list_items(store)? {
        let sec = store
            .get_secret(&format!("vault_{}", field_str(&it, "id")))?
            .unwrap_or_else(|| json!({}));
        for f in SCRUB_FIELDS {
            let v = get_str(&sec, f).trim().to_string();
            let compact = SPACE_DASH.replace_all(&v, "").into_owned();
            if compact.chars().count() < 5 {
                continue;
            }
            let is_digits = compact.chars().all(|c| c.is_numeric());
            let pat = if is_digits || ALNUM.is_match(&compact) {
                // tolerate the spacing / dashes sites add ("4111 1111 1111 1111", "4111-1111-…")
                let joined = compact
                    .chars()
                    .map(|c| regex::escape(&c.to_string()))
                    .collect::<Vec<_>>()
                    .join(r"[\s-]?");
                format!(r"(?i)(?<![A-Za-z0-9]){joined}(?![A-Za-z0-9])")
            } else {
                format!("(?i){}", regex::escape(&v))
            };
            if let Ok(rx) = FRegex::new(&pat) {
                pats.push(rx);
            }
        }
    }
    let arc = Arc::new(pats);
    *store.vault_patterns.lock() = Some(arc.clone());
    Ok(arc)
}

fn scrub_inner(pats: &[FRegex], obj: &Value, depth: usize) -> Value {
    if depth > 6 {
        return obj.clone();
    }
    match obj {
        Value::String(s) => {
            let mut s = s.clone();
            for p in pats {
                s = p.replace_all(&s, "[VAULT_VALUE]").into_owned();
            }
            Value::String(s)
        }
        Value::Object(o) => Value::Object(
            o.iter()
                .map(|(k, v)| {
                    (
                        k.clone(),
                        if k == "image_b64" {
                            v.clone()
                        } else {
                            scrub_inner(pats, v, depth + 1)
                        },
                    )
                })
                .collect(),
        ),
        Value::Array(a) => {
            Value::Array(a.iter().map(|v| scrub_inner(pats, v, depth + 1)).collect())
        }
        other => other.clone(),
    }
}

/// Remove every stored vault number from strings inside `obj` (dicts / lists). Images are left alone.
pub fn scrub(store: &Store, obj: &Value) -> Value {
    let pats = match compile(store) {
        Ok(p) => p,
        Err(_) => return obj.clone(),
    };
    if pats.is_empty() {
        return obj.clone();
    }
    scrub_inner(&pats, obj, 0)
}

/// Text for the approval card: `label · field · masked-or-hidden`.
pub fn summary_text(it: &Value, field: &str) -> String {
    let f = field_label(field);
    let shown = if field == "number" || field == "value" {
        field_str(it, "masked")
    } else {
        "（已隐藏 hidden）".to_string()
    };
    format!("{} · {} · {}", field_str(it, "label"), f, shown)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mask_and_domains() {
        assert_eq!(mask("4111 1111 1111 1234"), "•••• 1234");
        assert_eq!(mask("12-3"), "•••");
        assert_eq!(mask(""), "");
        assert_eq!(
            clean_domains(Some(&json!("https://www.Shop.test/x, b.com other"))),
            vec!["shop.test", "b.com", "other"]
        );
        assert_eq!(
            clean_domains(Some(&json!(["bad domain", "ok.sg"]))),
            vec!["ok.sg"]
        );
        assert!(clean_domains(None).is_empty());
    }

    #[test]
    fn items_scrub_and_fill_state() {
        let d = tempfile::tempdir().unwrap();
        let store = Store::new(d.path()).unwrap();
        let it = save_item(
            &store,
            &json!({"kind": "card", "label": "Visa", "domains": "shop.test",
                    "values": {"number": "4111 1111 1111 1234", "expiry": "12/28", "cvc": "123"}}),
            None,
        )
        .unwrap();
        assert_eq!(it["masked"], "•••• 1234");
        assert_eq!(it["fields"], json!(["number", "expiry", "cvc"]));
        assert_eq!(it["kind_label"], "银行卡 Card");
        assert!(!dumps(&json!(list_items(&store).unwrap())).contains("4111"));
        assert!(
            domain_ok(&it, "www.shop.test")
                && domain_ok(&it, "pay.shop.test")
                && !domain_ok(&it, "evil.example")
        );
        assert_eq!(
            summary_text(&it, "number"),
            "Visa · 号码 number · •••• 1234"
        );
        assert_eq!(
            summary_text(&it, "cvc"),
            "Visa · 安全码 CVC · （已隐藏 hidden）"
        );

        let out = scrub(
            &store,
            &json!({"snapshot": "Card 4111-1111-1111-1234 / 4111111111111234 total 123", "image_b64": "4111111111111234"}),
        );
        let snap = out["snapshot"].as_str().unwrap();
        assert!(!snap.replace("[VAULT_VALUE]", "").contains("1234") && snap.contains("total 123"));
        assert_eq!(out["image_b64"], "4111111111111234");

        note_fill("t1", "https://shop.test/pay#x");
        assert!(
            filled_here("t1", "https://shop.test/pay")
                && !filled_here("t1", "https://shop.test/done")
        );
        assert!(!filled_here("t2", "https://shop.test/pay"));

        let id = it["id"].as_str().unwrap().to_string();
        save_item(
            &store,
            &json!({"kind": "card", "label": "Visa 2", "values": {"number": ""}}),
            Some(&id),
        )
        .unwrap();
        assert_eq!(value(&store, &id, "number").unwrap(), "4111 1111 1111 1234");
        assert_eq!(item(&store, &id).unwrap().unwrap()["label"], "Visa 2");
        mark_used(&store, &id).unwrap();
        assert_eq!(item(&store, &id).unwrap().unwrap()["uses"], 1);

        assert!(matches!(
            save_item(&store, &json!({"kind": "nope", "label": "x"}), None),
            Err(VaultError::Value(_))
        ));
        assert!(matches!(
            save_item(&store, &json!({"kind": "card", "label": "x"}), None),
            Err(VaultError::Value(_))
        ));
        assert!(matches!(
            save_item(
                &store,
                &json!({"kind": "card", "label": "x", "values": {"number": "1"}}),
                Some("vlt_missing")
            ),
            Err(VaultError::Key(_))
        ));

        assert!(delete_item(&store, &id).unwrap());
        assert!(!store.has_secret(&format!("vault_{id}")).unwrap());
        assert!(!delete_item(&store, &id).unwrap());
        assert_eq!(
            scrub(&store, &json!("4111 1111 1111 1234")),
            json!("4111 1111 1111 1234")
        );
    }
}
