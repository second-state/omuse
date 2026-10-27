//! Daily memory housekeeping: prune → drop sensitive → merge duplicates → LLM review → report.
//!
//! Safety rules
//! * The profile is never changed here: profile details found in facts only become pending suggestions.
//! * Nothing the LLM proposes deletes a long-term fact outright. "Demote" moves it to recent memory, where it
//!   expires after 30 days (and can still be promoted back from the Memory page). Merges keep the merged-away
//!   texts in the surviving fact's history.
//! * A dry run computes the full plan and stores it; "apply" executes exactly that stored plan (ids re-checked),
//!   so what the user previewed is what happens.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, LazyLock};

use anyhow::{bail, Result};
use chrono::{Datelike, TimeZone, Timelike};
use futures::future::BoxFuture;
use omuse_common::util::{
    as_f64, as_i64, get_str, last_chars, now_ts, take_chars, truncate, truthy,
};
use omuse_common::{params, pyjson};
use regex::Regex;
use serde_json::{json, Map, Value};

use crate::llm::{extract_json, ChatOpts, Llm};
use crate::store::{looks_sensitive, norm_fact, profile_key, profile_value_ok, Store, RECENT_DAYS};

pub const BATCH: usize = 60;
pub const NEAR_DUP: f64 = 0.93;

pub const TIDY_PROMPT: &str = r#"You are tidying a personal assistant's long-term memory about its user. Below are memory facts, one per line as `id | tier | category | entity | uses | fact`.
tier "long" = kept for good; tier "recent" = one-off detail that expires after 30 days.

Return ONLY JSON with these optional lists (use ids exactly as given, never invent ids):
{"merge":   [{"ids": ["<id>", "<id>", ...], "fact": "<one combined short third-person sentence>", "category": "<preference|person|company|project|habit|other>"}],
 "rewrite": [{"id": "<id>", "fact": "<clearer, shorter sentence with the same meaning>"}],
 "demote":  [{"id": "<id>", "reason": "<why it is one-off / outdated / not about the user>"}],
 "promote": [{"id": "<id>", "reason": "<why this recent fact is durable>"}],
 "profile": [{"field": "<name_zh|name_en|preferred_name|phone|email_personal|email_work|address_home|address_work|address:<label>|company|job_title|birthday|nationality>", "value": "<value>", "ids": ["<id>"], "reason": "<short>"}]}

Rules:
- merge: facts that say the same thing or are clearly about one subject and read better as one sentence. Keep every detail.
- rewrite: only when the text is messy (logs, tool output, very long). Never change meaning and never add details or examples.
- demote (long → recent): process notes, task logs, one-off bookings/orders/dates, things that are no longer true, facts about web pages rather than the user (but how a website works FOR the user — guest checkout, phone format, delivery option — is durable: keep it).
- promote (recent → long): stable preferences, people, companies, habits that will matter for months.
- profile: a fixed personal detail of the user that belongs in their profile (name, phone, email, address, company, title, birthday, nationality). These are only suggested; the user confirms.
- Never output ID / passport / membership / card numbers, passwords or codes.
- When unsure, leave a fact alone. An empty answer {} is fine.

Facts:
"#;

/// What [`run`] needs from the runtime (`rt` in Python): the store, the LLM, and the audit / publish / Sentinel calls.
pub trait TidyRuntime: Send + Sync {
    fn store(&self) -> &Store;
    fn llm(&self) -> &Llm;
    /// `rt.audit(actor, action, result=..., detail=...)`.
    fn audit(&self, actor: &str, action: &str, result: &str, detail: Value) -> BoxFuture<'_, ()>;
    /// `rt.publish({...})`: an event for the UI stream.
    fn publish(&self, event: Value) -> BoxFuture<'_, ()>;
    /// `rt.sentinel(method, path, body, timeout=...)`: an internal call to Sentinel, JSON back.
    fn sentinel(
        &self,
        method: &str,
        path: &str,
        body: Option<Value>,
        timeout: f64,
    ) -> BoxFuture<'_, Result<Value>>;
}

fn mask(text: &str) -> String {
    static RX: LazyLock<fancy_regex::Regex> =
        LazyLock::new(|| fancy_regex::Regex::new(r"\d(?=[\d -]{3,}\d)").unwrap());
    RX.replace_all(text, "•").into_owned()
}

fn tier_of(f: &Value) -> String {
    let t = get_str(f, "tier");
    if t.is_empty() {
        "long".into()
    } else {
        t
    }
}

fn uses_of(f: &Value) -> i64 {
    f.get("uses").and_then(as_i64).unwrap_or(0)
}

fn line(f: &Value) -> String {
    static WS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s+").unwrap());
    let fact = WS.replace_all(&get_str(f, "fact"), " ").into_owned();
    let fact = take_chars(&fact, 300).replace('|', "/");
    format!(
        "{} | {} | {} | {} | {} | {}",
        get_str(f, "id"),
        tier_of(f),
        get_str(f, "category"),
        take_chars(&get_str(f, "entity"), 40),
        uses_of(f),
        fact
    )
}

fn all_facts(store: &Store) -> Result<Vec<Value>> {
    store.facts(5000, None)
}

/// `{"long", "recent", "episodes", "profile", "pending"}` counts.
pub fn counts(store: &Store) -> Result<Value> {
    let fs = all_facts(store)?;
    Ok(json!({
        "long": fs.iter().filter(|f| tier_of(f) == "long").count(),
        "recent": fs.iter().filter(|f| get_str(f, "tier") == "recent").count(),
        "episodes": store.episodes(100000)?.len(),
        "profile": store.profile()?.as_object().map(|o| o.len()).unwrap_or(0),
        "pending": store.profile_pending("pending")?.len(),
    }))
}

const FILLER_WORDS: [&str; 3] = ["a", "an", "the"];
const FILLER_CJK: &str = "的了地得之着";

fn is_filler(w: &str) -> bool {
    if FILLER_WORDS.contains(&w) {
        return true;
    }
    let mut cs = w.chars();
    matches!((cs.next(), cs.next()), (Some(c), None) if FILLER_CJK.contains(c))
}

fn tokens_of(text: &str) -> Vec<String> {
    static RX: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"[A-Za-z0-9']+|[^\sA-Za-z0-9'\W]").unwrap());
    let low = text.to_lowercase();
    RX.find_iter(&low)
        .map(|m| {
            let w = m.as_str();
            if w.chars().all(|c| c.is_ascii_lowercase()) && w.len() > 3 {
                // seat / seats
                if let Some(s) = w.strip_suffix("es") {
                    return s.to_string();
                }
                if let Some(s) = w.strip_suffix('s') {
                    return s.to_string();
                }
            }
            w.to_string()
        })
        .collect()
}

/// `difflib.SequenceMatcher(None, a, b, autojunk=False).find_longest_match`.
fn longest_match(
    a: &[String],
    b2j: &HashMap<&str, Vec<usize>>,
    alo: usize,
    ahi: usize,
    blo: usize,
    bhi: usize,
) -> (usize, usize, usize) {
    let (mut besti, mut bestj, mut bestsize) = (alo, blo, 0usize);
    let mut j2len: HashMap<usize, usize> = HashMap::new();
    for (i, ai) in a.iter().enumerate().take(ahi).skip(alo) {
        let mut newj2len: HashMap<usize, usize> = HashMap::new();
        if let Some(js) = b2j.get(ai.as_str()) {
            for &j in js {
                if j < blo {
                    continue;
                }
                if j >= bhi {
                    break;
                }
                let k = j2len.get(&(j.wrapping_sub(1))).copied().unwrap_or(0) + 1;
                newj2len.insert(j, k);
                if k > bestsize {
                    besti = i + 1 - k;
                    bestj = j + 1 - k;
                    bestsize = k;
                }
            }
        }
        j2len = newj2len;
    }
    (besti, bestj, bestsize)
}

/// `SequenceMatcher.get_matching_blocks()` without the terminal sentinel (sorted by position in `a`).
fn matching_blocks(a: &[String], b: &[String]) -> Vec<(usize, usize, usize)> {
    let mut b2j: HashMap<&str, Vec<usize>> = HashMap::new();
    for (j, w) in b.iter().enumerate() {
        b2j.entry(w.as_str()).or_default().push(j);
    }
    let mut queue = vec![(0usize, a.len(), 0usize, b.len())];
    let mut blocks = Vec::new();
    while let Some((alo, ahi, blo, bhi)) = queue.pop() {
        let (i, j, k) = longest_match(a, &b2j, alo, ahi, blo, bhi);
        if k > 0 {
            blocks.push((i, j, k));
            if alo < i && blo < j {
                queue.push((alo, i, blo, j));
            }
            if i + k < ahi && j + k < bhi {
                queue.push((i + k, ahi, j + k, bhi));
            }
        }
    }
    blocks.sort();
    blocks
}

/// The same sentence up to articles, plurals or 的/了 ("an aisle seat" / "aisle seats").
/// A different word, name, number or a negation is never a duplicate.
pub fn near_dup(a: &str, b: &str) -> bool {
    let (ta, tb) = (tokens_of(a), tokens_of(b));
    let total = ta.len() + tb.len();
    let rqr = if total > 0 {
        2.0 * ta.len().min(tb.len()) as f64 / total as f64
    } else {
        1.0
    };
    if rqr < NEAR_DUP - 0.1 {
        return false;
    }
    let (mut i, mut j) = (0usize, 0usize);
    let mut blocks = matching_blocks(&ta, &tb);
    blocks.push((ta.len(), tb.len(), 0));
    for (bi, bj, bk) in blocks {
        if ta[i..bi]
            .iter()
            .chain(tb[j..bj].iter())
            .any(|w| !is_filler(w))
        {
            return false;
        }
        i = bi + bk;
        j = bj + bk;
    }
    true
}

/// `RStore._live(row)`: not expired by `expires_at` or `ttl_days`.
fn live(r: &Value) -> bool {
    let now = now_ts();
    if let Some(exp) = r.get("expires_at").filter(|v| truthy(v)).and_then(as_f64) {
        if exp < now {
            return false;
        }
    }
    match (
        r.get("ttl_days").filter(|v| truthy(v)).and_then(as_f64),
        r.get("created_at").and_then(as_f64),
    ) {
        (Some(ttl), Some(created)) => created + ttl * 86400.0 >= now,
        _ => true,
    }
}

fn count_old_episodes(store: &Store, cut: f64) -> Result<i64> {
    Ok(store
        .db
        .one(
            "SELECT COUNT(*) AS n FROM episodes WHERE ts < ?",
            params![cut],
        )?
        .and_then(|r| r.get("n").and_then(as_i64))
        .unwrap_or(0))
}

// ------------------------------------------------------------------ planning (no writes)
fn plan_local(store: &Store) -> Result<Value> {
    let now = now_ts();
    let fs = store.db.all("SELECT * FROM facts", params![])?;
    let prune_facts: Vec<String> = fs
        .iter()
        .filter(|f| {
            f.get("expires_at")
                .filter(|v| truthy(v))
                .and_then(as_f64)
                .map(|e| e < now)
                .unwrap_or(false)
        })
        .map(|f| get_str(f, "id"))
        .collect();
    let cut = now - RECENT_DAYS * 86400.0;
    let prune_episodes = count_old_episodes(store, cut)?;
    let pruned: HashSet<&str> = prune_facts.iter().map(|s| s.as_str()).collect();
    let mut live_rows: Vec<Value> = fs
        .iter()
        .filter(|f| !pruned.contains(get_str(f, "id").as_str()) && live(f))
        .cloned()
        .collect();
    let mut sensitive: Vec<Value> = Vec::new();
    for f in &live_rows {
        let fact = get_str(f, "fact");
        if looks_sensitive(&fact) {
            sensitive.push(json!({"id": get_str(f, "id"), "preview": mask(&truncate(&fact, 80))}));
        }
    }
    let gone: HashSet<String> = sensitive.iter().map(|x| get_str(x, "id")).collect();
    live_rows.retain(|f| !gone.contains(&get_str(f, "id")));
    // duplicates: same normalised text, or nearly the same within one entity/category
    live_rows.sort_by(|a, b| {
        let ka = (
            -uses_of(a),
            tier_of(a) != "long",
            a.get("created_at").and_then(as_f64).unwrap_or(0.0),
        );
        let kb = (
            -uses_of(b),
            tier_of(b) != "long",
            b.get("created_at").and_then(as_f64).unwrap_or(0.0),
        );
        ka.0.cmp(&kb.0)
            .then(ka.1.cmp(&kb.1))
            .then(ka.2.partial_cmp(&kb.2).unwrap_or(std::cmp::Ordering::Equal))
    });
    let mut taken: HashSet<String> = HashSet::new();
    let norms: HashMap<String, String> = live_rows
        .iter()
        .map(|f| (get_str(f, "id"), norm_fact(&get_str(f, "fact"))))
        .collect();
    let mut dups: Vec<Value> = Vec::new();
    for (i, a) in live_rows.iter().enumerate() {
        let aid = get_str(a, "id");
        if taken.contains(&aid) {
            continue;
        }
        let mut group: Vec<String> = Vec::new();
        for b in &live_rows[i + 1..] {
            let bid = get_str(b, "id");
            if taken.contains(&bid) {
                continue;
            }
            let mut same = norms[&aid] == norms[&bid];
            if !same && get_str(a, "entity") == get_str(b, "entity") {
                same = near_dup(&get_str(a, "fact"), &get_str(b, "fact"));
            }
            if same {
                group.push(bid.clone());
                taken.insert(bid);
            }
        }
        if !group.is_empty() {
            taken.insert(aid.clone());
            dups.push(json!({"keep": aid, "drop": group, "fact": get_str(a, "fact")}));
        }
    }
    // recent facts the agent keeps using are evidently durable
    let auto_promote: Vec<String> = live_rows
        .iter()
        .filter(|f| {
            get_str(f, "tier") == "recent" && uses_of(f) >= 2 && !taken.contains(&get_str(f, "id"))
        })
        .map(|f| get_str(f, "id"))
        .collect();
    Ok(
        json!({"prune_facts": prune_facts, "prune_episodes": prune_episodes, "sensitive": sensitive,
              "dups": dups, "auto_promote": auto_promote}),
    )
}

fn arr<'a>(v: &'a Value, key: &str) -> &'a [Value] {
    v.get(key)
        .and_then(Value::as_array)
        .map(|a| a.as_slice())
        .unwrap_or(&[])
}

fn reason_of(v: &Value) -> String {
    take_chars(&get_str(v, "reason"), 160).to_string()
}

async fn plan_llm(
    store: &Store,
    llm: &Llm,
    skip: &HashSet<String>,
) -> Result<(Value, Vec<String>)> {
    let mut facts: Vec<Value> = all_facts(store)?
        .into_iter()
        .filter(|f| !skip.contains(&get_str(f, "id")))
        .collect();
    let mut merge: Vec<Value> = Vec::new();
    let mut rewrite: Vec<Value> = Vec::new();
    let mut demote: Vec<Value> = Vec::new();
    let mut promote: Vec<Value> = Vec::new();
    let mut profile: Vec<Value> = Vec::new();
    let mut errors: Vec<String> = Vec::new();
    // group by entity/category so related facts land in the same batch
    let sort_key = |f: &Value| {
        let e = get_str(f, "entity");
        (
            if e.is_empty() {
                "~".to_string()
            } else {
                e.to_lowercase()
            },
            get_str(f, "category"),
        )
    };
    facts.sort_by(|a, b| {
        sort_key(a).cmp(&sort_key(b)).then(
            a.get("created_at")
                .and_then(as_f64)
                .unwrap_or(0.0)
                .partial_cmp(&b.get("created_at").and_then(as_f64).unwrap_or(0.0))
                .unwrap_or(std::cmp::Ordering::Equal),
        )
    });
    for (bi, chunk) in facts.chunks(BATCH).enumerate() {
        let ids: HashMap<String, &Value> = chunk.iter().map(|f| (get_str(f, "id"), f)).collect();
        let prompt = format!(
            "{}{}",
            TIDY_PROMPT,
            chunk.iter().map(line).collect::<Vec<_>>().join("\n")
        );
        let opts = ChatOpts {
            temperature: Some(0.1),
            max_tokens: Some(3000),
            purpose: "memory".into(),
            task_id: String::new(),
            model: None,
            no_think: true,
            stream_to: None,
        };
        let data = match llm
            .chat(&[json!({"role": "user", "content": prompt})], None, &opts)
            .await
        {
            Ok(r) => extract_json(&r.content).unwrap_or_else(|| json!({})),
            Err(e) => {
                errors.push(format!(
                    "batch {}: {}",
                    bi + 1,
                    take_chars(&e.to_string(), 160)
                ));
                continue;
            }
        };
        if !data.is_object() {
            continue;
        }
        let mut used: HashSet<String> = HashSet::new();
        let ok = |fid: Option<&Value>, used: &HashSet<String>| -> Option<String> {
            match fid {
                Some(Value::String(s)) if ids.contains_key(s) && !used.contains(s) => {
                    Some(s.clone())
                }
                _ => None,
            }
        };
        let fact_of = |fid: &str| get_str(ids[fid], "fact");
        for m in arr(&data, "merge") {
            if !m.is_object() {
                continue;
            }
            let mut mids: Vec<String> = Vec::new();
            for x in arr(m, "ids") {
                if let Some(s) = ok(Some(x), &used) {
                    if !mids.contains(&s) {
                        mids.push(s);
                    }
                }
            }
            let text = get_str(m, "fact").trim().to_string();
            if mids.len() < 2
                || text.is_empty()
                || text.chars().count() > 400
                || looks_sensitive(&text)
            {
                continue;
            }
            used.extend(mids.iter().cloned());
            let cat = get_str(m, "category").to_lowercase();
            let mut keep = mids[0].clone();
            let mut best = (tier_of(ids[&keep]) == "long", uses_of(ids[&keep]));
            for x in &mids[1..] {
                let k = (tier_of(ids[x]) == "long", uses_of(ids[x]));
                if k > best {
                    best = k;
                    keep = x.clone();
                }
            }
            merge.push(json!({
                "keep": keep,
                "drop": mids.iter().filter(|x| **x != keep).cloned().collect::<Vec<_>>(),
                "fact": text,
                "category": if ["preference", "person", "company", "project", "habit", "other"].contains(&cat.as_str()) { cat } else { String::new() },
                "before": mids.iter().map(|x| fact_of(x)).collect::<Vec<_>>(),
            }));
        }
        for w in arr(&data, "rewrite") {
            if !w.is_object() {
                continue;
            }
            let text = get_str(w, "fact").trim().to_string();
            // a rewrite tidies; it must not grow (that is how invented details sneak in)
            if let Some(fid) = ok(w.get("id"), &used) {
                let before = fact_of(&fid);
                if !text.is_empty()
                    && text.chars().count() <= (before.chars().count() as f64 * 1.1) as usize + 5
                    && norm_fact(&text) != norm_fact(&before)
                    && !looks_sensitive(&text)
                {
                    used.insert(fid.clone());
                    rewrite.push(json!({"id": fid, "fact": text, "before": before}));
                }
            }
        }
        for d in arr(&data, "demote") {
            if !d.is_object() {
                continue;
            }
            if let Some(fid) = ok(d.get("id"), &used) {
                if tier_of(ids[&fid]) == "long" {
                    used.insert(fid.clone());
                    demote.push(json!({"id": fid, "fact": fact_of(&fid), "reason": reason_of(d)}));
                }
            }
        }
        for p in arr(&data, "promote") {
            if !p.is_object() {
                continue;
            }
            if let Some(fid) = ok(p.get("id"), &used) {
                if get_str(ids[&fid], "tier") == "recent" {
                    used.insert(fid.clone());
                    promote.push(json!({"id": fid, "fact": fact_of(&fid), "reason": reason_of(p)}));
                }
            }
        }
        for p in arr(&data, "profile") {
            if !p.is_object() {
                continue;
            }
            let key = profile_key(&get_str(p, "field"));
            let val = get_str(p, "value").trim().to_string();
            if !key.is_empty()
                && !val.is_empty()
                && !looks_sensitive(&val)
                && val.chars().count() <= 300
                && profile_value_ok(&key, &val)
            {
                let pids: Vec<String> = arr(p, "ids")
                    .iter()
                    .filter_map(|x| x.as_str())
                    .filter(|x| ids.contains_key(*x))
                    .map(str::to_string)
                    .collect();
                profile
                    .push(json!({"field": key, "value": val, "reason": reason_of(p), "ids": pids}));
            }
        }
    }
    // one suggestion per field: the most complete value (e.g. the address with the unit number)
    let mut best: Vec<(String, Value)> = Vec::new();
    for s in profile {
        let field = get_str(&s, "field");
        match best.iter_mut().find(|(f, _)| *f == field) {
            Some((_, cur)) => {
                if get_str(&s, "value").chars().count() > get_str(cur, "value").chars().count() {
                    *cur = s;
                }
            }
            None => best.push((field, s)),
        }
    }
    let profile: Vec<Value> = best.into_iter().map(|(_, v)| v).collect();
    Ok((
        json!({"merge": merge, "rewrite": rewrite, "demote": demote, "promote": promote, "profile": profile}),
        errors,
    ))
}

/// The full plan: `{"local": {...}, "llm": {...}, "errors": [...]}`. Reads only.
pub async fn plan(store: &Store, llm: &Llm) -> Result<Value> {
    let local = plan_local(store)?;
    let mut skip: HashSet<String> = arr(&local, "prune_facts")
        .iter()
        .map(|v| get_str(&json!({"x": v}), "x"))
        .collect();
    skip.extend(arr(&local, "sensitive").iter().map(|x| get_str(x, "id")));
    for g in arr(&local, "dups") {
        skip.extend(
            arr(g, "drop")
                .iter()
                .filter_map(|d| d.as_str())
                .map(str::to_string),
        );
    }
    let (llm_plan, errors) = plan_llm(store, llm, &skip).await?;
    // don't let the LLM act on facts the local pass already handles
    Ok(json!({"local": local, "llm": llm_plan, "errors": errors}))
}

// ------------------------------------------------------------------ applying a plan
fn str_list(v: &Value, key: &str) -> Vec<String> {
    arr(v, key)
        .iter()
        .map(|x| match x {
            Value::String(s) => s.clone(),
            other => omuse_common::util::py_str(other),
        })
        .collect()
}

/// Execute a plan; returns the counts of what was done.
pub fn apply(store: &Store, p: &Value) -> Result<Value> {
    let mut done: Map<String, Value> = Map::new();
    for k in [
        "pruned_facts",
        "pruned_episodes",
        "sensitive",
        "merged",
        "rewritten",
        "demoted",
        "promoted",
        "profile_suggestions",
        "skipped",
    ] {
        done.insert(k.into(), json!(0));
    }
    let bump = |done: &mut Map<String, Value>, k: &str, n: i64| {
        let cur = done.get(k).and_then(as_i64).unwrap_or(0);
        done.insert(k.into(), json!(cur + n));
    };
    let empty = json!({});
    let local = p.get("local").filter(|v| truthy(v)).unwrap_or(&empty);
    let llm = p.get("llm").filter(|v| truthy(v)).unwrap_or(&empty);
    for fid in str_list(local, "prune_facts") {
        if store.fact(&fid)?.is_some() {
            store.delete_fact(&fid)?;
            bump(&mut done, "pruned_facts", 1);
        }
    }
    let cut = now_ts() - RECENT_DAYS * 86400.0;
    done.insert(
        "pruned_episodes".into(),
        json!(count_old_episodes(store, cut)?),
    );
    store
        .db
        .execute("DELETE FROM episodes WHERE ts < ?", params![cut])?;
    for s in arr(local, "sensitive") {
        let sid = get_str(s, "id");
        if store.fact(&sid)?.is_some() {
            store.delete_fact(&sid)?;
            bump(&mut done, "sensitive", 1);
        }
    }

    let merge = |done: &mut Map<String, Value>,
                 keep: &str,
                 drop: &[String],
                 text: Option<&str>,
                 cat: &str|
     -> Result<bool> {
        let k = store.fact(keep)?;
        let mut others: Vec<Value> = Vec::new();
        for x in drop {
            if let Some(o) = store.fact(x)? {
                others.push(o);
            }
        }
        let Some(k) = k else {
            bump(done, "skipped", 1);
            return Ok(false);
        };
        if others.is_empty() {
            bump(done, "skipped", 1);
            return Ok(false);
        }
        let hist = others
            .iter()
            .map(|o| get_str(o, "fact"))
            .collect::<Vec<_>>()
            .join("\n");
        let mut upd = Map::new();
        upd.insert(
            "uses".into(),
            json!(uses_of(&k) + others.iter().map(uses_of).sum::<i64>()),
        );
        let joined = format!("{}\n{}", get_str(&k, "history"), hist);
        upd.insert(
            "history".into(),
            Value::String(last_chars(joined.trim(), 2000).to_string()),
        );
        if others.iter().any(|o| tier_of(o) == "long") && get_str(&k, "tier") == "recent" {
            upd.insert("tier".into(), json!("long"));
        }
        if !cat.is_empty() {
            upd.insert("category".into(), json!(cat));
        }
        let status_of = |x: &Value| {
            let s = get_str(x, "status");
            if s.is_empty() {
                "active".to_string()
            } else {
                s
            }
        };
        if std::iter::once(&k)
            .chain(others.iter())
            .any(|x| status_of(x) == "active")
        {
            upd.insert("status".into(), json!("active"));
        }
        if let Some(t) = text {
            if !t.is_empty() && t != get_str(&k, "fact") {
                upd.insert("fact".into(), json!(t));
            }
        }
        store.update_fact(keep, &Value::Object(upd))?;
        for o in &others {
            store.delete_fact(&get_str(o, "id"))?;
        }
        Ok(true)
    };

    for g in arr(local, "dups") {
        let drop = str_list(g, "drop");
        if merge(&mut done, &get_str(g, "keep"), &drop, None, "")? {
            bump(&mut done, "merged", drop.len() as i64);
        }
    }
    for m in arr(llm, "merge") {
        let drop = str_list(m, "drop");
        if merge(
            &mut done,
            &get_str(m, "keep"),
            &drop,
            Some(&get_str(m, "fact")),
            &get_str(m, "category"),
        )? {
            bump(&mut done, "merged", drop.len() as i64);
        }
    }
    for w in arr(llm, "rewrite") {
        let f = store.fact(&get_str(w, "id"))?;
        let matches = f.as_ref().map(|f| {
            let cur = get_str(f, "fact");
            match w.get("before") {
                Some(b) => cur == get_str(&json!({"x": b}), "x"),
                None => true,
            }
        });
        if matches == Some(true) {
            store.update_fact(&get_str(w, "id"), &json!({"fact": get_str(w, "fact")}))?;
            bump(&mut done, "rewritten", 1);
        } else {
            bump(&mut done, "skipped", 1);
        }
    }
    for d in arr(llm, "demote") {
        let did = get_str(d, "id");
        if let Some(f) = store.fact(&did)? {
            if tier_of(&f) == "long" {
                store.update_fact(&did, &json!({"tier": "recent"}))?;
                bump(&mut done, "demoted", 1);
            }
        }
    }
    let mut promote_ids = str_list(local, "auto_promote");
    promote_ids.extend(arr(llm, "promote").iter().map(|x| get_str(x, "id")));
    for pid in promote_ids {
        if let Some(f) = store.fact(&pid)? {
            if get_str(&f, "tier") == "recent" {
                store.update_fact(&pid, &json!({"tier": "long"}))?;
                bump(&mut done, "promoted", 1);
            }
        }
    }
    for s in arr(llm, "profile") {
        let reason = get_str(s, "reason");
        let reason = if reason.is_empty() {
            "memory tidy".to_string()
        } else {
            reason
        };
        if store
            .suggest_profile(
                &get_str(s, "field"),
                &get_str(s, "value"),
                &reason,
                "memory-tidy",
            )?
            .is_some()
        {
            bump(&mut done, "profile_suggestions", 1);
        }
    }
    Ok(Value::Object(done))
}

fn n_of(v: &Value, k: &str) -> i64 {
    v.get(k).and_then(as_i64).unwrap_or(0)
}

/// The report lines shown on the Memory page / sent to Telegram (`done = None` → a preview).
pub fn summary_lines(
    p: &Value,
    done: Option<&Value>,
    before: &Value,
    after: Option<&Value>,
    en: bool,
) -> Vec<String> {
    let empty = json!({});
    let local = p.get("local").filter(|v| truthy(v)).unwrap_or(&empty);
    let llm = p.get("llm").filter(|v| truthy(v)).unwrap_or(&empty);
    let n: Value = match done {
        None => json!({
            "pruned_facts": arr(local, "prune_facts").len(),
            "pruned_episodes": n_of(local, "prune_episodes"),
            "sensitive": arr(local, "sensitive").len(),
            "merged": arr(local, "dups").iter().map(|g| arr(g, "drop").len()).sum::<usize>()
                + arr(llm, "merge").iter().map(|m| arr(m, "drop").len()).sum::<usize>(),
            "rewritten": arr(llm, "rewrite").len(),
            "demoted": arr(llm, "demote").len(),
            "promoted": arr(local, "auto_promote").len() + arr(llm, "promote").len(),
            "profile_suggestions": arr(llm, "profile").len(),
        }),
        Some(d) => d.clone(),
    };
    let mut lines = Vec::new();
    if en {
        lines.push(format!(
            "Long-term {} · recent {} · episodes {}{}",
            n_of(before, "long"),
            n_of(before, "recent"),
            n_of(before, "episodes"),
            after
                .map(|a| format!(
                    " → long-term {} · recent {} · episodes {}",
                    n_of(a, "long"),
                    n_of(a, "recent"),
                    n_of(a, "episodes")
                ))
                .unwrap_or_default()
        ));
        lines.push(format!(
            "Merged duplicates: {} · rewritten: {}",
            n_of(&n, "merged"),
            n_of(&n, "rewritten")
        ));
        lines.push(format!(
            "Moved to recent (expire in 30 days): {} · kept for good: {}",
            n_of(&n, "demoted"),
            n_of(&n, "promoted")
        ));
        lines.push(format!(
            "Expired / removed: {} facts, {} episodes",
            n_of(&n, "pruned_facts"),
            n_of(&n, "pruned_episodes")
        ));
        if n_of(&n, "sensitive") != 0 {
            lines.push(format!(
                "Removed {} facts with ID/card numbers or passwords (put those in the vault)",
                n_of(&n, "sensitive")
            ));
        }
        if n_of(&n, "profile_suggestions") != 0 {
            lines.push(format!(
                "Profile suggestions waiting for you: {} (Memory page)",
                n_of(&n, "profile_suggestions")
            ));
        }
    } else {
        lines.push(format!(
            "长期 {} 条 · 近期 {} 条 · 经历 {} 条{}",
            n_of(before, "long"),
            n_of(before, "recent"),
            n_of(before, "episodes"),
            after
                .map(|a| format!(
                    " → 长期 {} · 近期 {} · 经历 {}",
                    n_of(a, "long"),
                    n_of(a, "recent"),
                    n_of(a, "episodes")
                ))
                .unwrap_or_default()
        ));
        lines.push(format!(
            "合并重复：{} 条 · 改写：{} 条",
            n_of(&n, "merged"),
            n_of(&n, "rewritten")
        ));
        lines.push(format!(
            "降为近期（30 天后过期）：{} 条 · 升为长期：{} 条",
            n_of(&n, "demoted"),
            n_of(&n, "promoted")
        ));
        lines.push(format!(
            "过期清理：记忆 {} 条，经历 {} 条",
            n_of(&n, "pruned_facts"),
            n_of(&n, "pruned_episodes")
        ));
        if n_of(&n, "sensitive") != 0 {
            lines.push(format!(
                "删除了 {} 条含证件号/卡号/密码的记忆（这类信息请放进保险箱）",
                n_of(&n, "sensitive")
            ));
        }
        if n_of(&n, "profile_suggestions") != 0 {
            lines.push(format!(
                "有 {} 条档案修改建议等你确认（记忆页）",
                n_of(&n, "profile_suggestions")
            ));
        }
    }
    lines
}

/// `dry_run` → compute & store a plan. `from_run` → apply that stored plan. Otherwise plan + apply at once.
/// Returns the report (with its `id`).
pub async fn run(
    rt: &dyn TidyRuntime,
    dry_run: bool,
    kind: &str,
    from_run: Option<i64>,
) -> Result<Value> {
    let store = rt.store();
    let en = get_str(&store.settings()?, "language") == "en";
    let before = counts(store)?;
    let mut prev: Option<Value> = None;
    let p = if let Some(fr) = from_run {
        let found = store
            .memory_runs(50)?
            .into_iter()
            .find(|r| r.get("id").and_then(as_i64) == Some(fr));
        let usable = found.as_ref().filter(|r| {
            get_str(r, "kind") == "dry_run"
                && !r
                    .get("report")
                    .and_then(|rep| rep.get("applied"))
                    .map(truthy)
                    .unwrap_or(false)
        });
        let Some(r) = usable else {
            bail!("该预览不存在或已执行 (preview not found or already applied)");
        };
        let p = r
            .get("report")
            .and_then(|rep| rep.get("plan"))
            .cloned()
            .unwrap_or_else(|| json!({}));
        prev = Some(r.clone());
        p
    } else {
        plan(store, rt.llm()).await?
    };
    let mut report = json!({
        "plan": p,
        "before": before,
        "errors": p.get("errors").filter(|v| truthy(v)).cloned().unwrap_or_else(|| json!([])),
        "ts": now_ts(),
    });
    if dry_run {
        report["lines"] = json!(summary_lines(&p, None, &before, None, en));
        let id = store.add_memory_run("dry_run", &report)?;
        report["id"] = json!(id);
        return Ok(report);
    }
    let done = apply(store, &p)?;
    let after = counts(store)?;
    report["done"] = done.clone();
    report["after"] = after.clone();
    report["lines"] = json!(summary_lines(&p, Some(&done), &before, Some(&after), en));
    report["from_run"] = json!(from_run);
    let id = store.add_memory_run(if from_run.is_some() { "applied" } else { kind }, &report)?;
    report["id"] = json!(id);
    if let (Some(fr), Some(mut prev)) = (from_run, prev) {
        prev["report"]["applied"] = json!(id);
        store.db.execute(
            "UPDATE memory_runs SET report=? WHERE id=?",
            params![pyjson::dumps(&prev["report"]), fr],
        )?;
    }
    rt.audit(
        "memory",
        "memory.consolidate",
        "success",
        json!({"kind": kind, "done": done, "before": before, "after": after}),
    )
    .await;
    rt.publish(json!({"kind": "memory_update"})).await;
    let changed = done
        .as_object()
        .map(|o| o.iter().any(|(k, v)| k != "skipped" && truthy(v)))
        .unwrap_or(false)
        || truthy(&report["errors"]);
    if !changed && kind == "daily" {
        return Ok(report); // a quiet day: no Telegram message
    }
    let title = if en {
        "🧠 Memory tidied"
    } else {
        "🧠 记忆整理完成"
    };
    let body = report["lines"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|l| get_str(&json!({"x": l}), "x"))
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default();
    let _ = rt
        .sentinel(
            "POST",
            "/internal/notify",
            Some(json!({"task_id": "", "text": format!("{}\n{}", title, body)})),
            20.0,
        )
        .await;
    Ok(report)
}

/// True once per local day, after `memory_consolidate_at`.
pub fn due(store: &Store, now: Option<f64>) -> Result<bool> {
    let s = store.settings()?;
    if !s.get("memory_consolidation").map(truthy).unwrap_or(true) {
        return Ok(false);
    }
    let tz_name = get_str(&s, "timezone");
    let tz: chrono_tz::Tz = if tz_name.is_empty() {
        "UTC"
    } else {
        tz_name.as_str()
    }
    .parse()
    .unwrap_or(chrono_tz::UTC);
    let now = now.unwrap_or_else(now_ts);
    let to_local = |ts: f64| {
        let secs = ts.floor();
        tz.timestamp_opt(secs as i64, ((ts - secs) * 1e9) as u32)
            .single()
            .unwrap_or_else(|| tz.timestamp_opt(0, 0).single().unwrap())
    };
    let now_dt = to_local(now);
    let at = get_str(&s, "memory_consolidate_at");
    let at = if at.is_empty() {
        "03:30".to_string()
    } else {
        at
    };
    let parts: Vec<Option<i32>> = at
        .split(':')
        .take(2)
        .map(|x| x.trim().parse::<i32>().ok())
        .collect();
    let (hh, mm) = match parts.as_slice() {
        [Some(h), Some(m)] => (*h, *m),
        _ => (3, 30),
    };
    if (now_dt.hour() as i32, now_dt.minute() as i32) < (hh, mm) {
        return Ok(false);
    }
    if needs_first_review(store)? {
        return Ok(false);
    }
    let last = store.db.one(
        "SELECT ts FROM memory_runs WHERE kind='daily' ORDER BY id DESC LIMIT 1",
        params![],
    )?;
    let same_day = last
        .and_then(|r| r.get("ts").and_then(as_f64))
        .map(|ts| {
            let d = to_local(ts);
            (d.year(), d.ordinal()) == (now_dt.year(), now_dt.ordinal())
        })
        .unwrap_or(false);
    Ok(!same_day)
}

/// A memory that already holds a lot is tidied the first time only after the user previewed and applied it.
pub fn needs_first_review(store: &Store) -> Result<bool> {
    let applied = store.db.one(
        "SELECT id FROM memory_runs WHERE kind IN ('manual','daily','applied') LIMIT 1",
        params![],
    )?;
    Ok(applied.is_none() && all_facts(store)?.len() > 20)
}

// ------------------------------------------------------------------ background job (the LLM pass can take minutes)
static JOB: LazyLock<parking_lot::Mutex<Map<String, Value>>> = LazyLock::new(|| {
    let mut m = Map::new();
    m.insert("running".into(), json!(false));
    parking_lot::Mutex::new(m)
});

/// The state of the background tidy job (`{"running", "started", "dry_run", "kind", "last_id", "error", "finished"}`).
pub fn job_state() -> Value {
    Value::Object(JOB.lock().clone())
}

/// Start a tidy run in the background; `false` when one is already running.
pub fn start(rt: Arc<dyn TidyRuntime>, dry_run: bool, kind: &str, from_run: Option<i64>) -> bool {
    {
        let mut job = JOB.lock();
        if job.get("running").map(truthy).unwrap_or(false) {
            return false;
        }
        job.clear();
        job.insert("running".into(), json!(true));
        job.insert("started".into(), json!(now_ts()));
        job.insert("dry_run".into(), json!(dry_run));
        job.insert("kind".into(), json!(kind));
    }
    let kind = kind.to_string();
    tokio::spawn(async move {
        match run(rt.as_ref(), dry_run, &kind, from_run).await {
            Ok(r) => {
                let mut job = JOB.lock();
                job.insert(
                    "last_id".into(),
                    r.get("id").cloned().unwrap_or(Value::Null),
                );
                job.insert("error".into(), json!(""));
            }
            Err(e) => {
                eprintln!("[memory_tidy] {:?}", e);
                JOB.lock()
                    .insert("error".into(), json!(take_chars(&e.to_string(), 300)));
            }
        }
        {
            let mut job = JOB.lock();
            job.insert("running".into(), json!(false));
            job.insert("finished".into(), json!(now_ts()));
        }
        rt.publish(json!({"kind": "memory_update"})).await;
    });
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::{HttpRequest, HttpResponse, LlmTransport, SettingsFn};
    use crate::store::FactOpts;
    use omuse_common::Config;
    use parking_lot::Mutex;

    /// Always answers with the same JSON content.
    struct Fixed(String);

    impl LlmTransport for Fixed {
        fn send(
            &self,
            _req: HttpRequest,
        ) -> BoxFuture<'_, std::result::Result<HttpResponse, String>> {
            let body = json!({"choices": [{"message": {"role": "assistant", "content": self.0}}], "usage": {}});
            Box::pin(async move {
                Ok(HttpResponse {
                    status: 200,
                    retry_after: None,
                    text: pyjson::dumps(&body),
                })
            })
        }
    }

    fn fixed_llm(content: Value) -> Llm {
        let settings: SettingsFn = Arc::new(
            || json!({"model_base_url": "http://x/v1", "model_name": "m", "temperature": 0, "max_tokens": 10}),
        );
        Llm::with_transport(
            settings,
            None,
            None,
            Arc::new(Fixed(pyjson::dumps(&content))),
        )
    }

    struct Rt {
        store: Store,
        llm: Llm,
        sent: Mutex<Vec<String>>,
    }

    impl TidyRuntime for Rt {
        fn store(&self) -> &Store {
            &self.store
        }
        fn llm(&self) -> &Llm {
            &self.llm
        }
        fn audit(&self, _a: &str, _b: &str, _c: &str, _d: Value) -> BoxFuture<'_, ()> {
            Box::pin(async {})
        }
        fn publish(&self, _e: Value) -> BoxFuture<'_, ()> {
            Box::pin(async {})
        }
        fn sentinel(
            &self,
            _m: &str,
            _p: &str,
            body: Option<Value>,
            _t: f64,
        ) -> BoxFuture<'_, Result<Value>> {
            self.sent
                .lock()
                .push(get_str(body.as_ref().unwrap_or(&Value::Null), "text"));
            Box::pin(async { Ok(json!({})) })
        }
    }

    fn store() -> (tempfile::TempDir, Store) {
        let d = tempfile::tempdir().unwrap();
        let st = Store::new(d.path(), Arc::new(Config::parse("").unwrap())).unwrap();
        (d, st)
    }

    fn add(st: &Store, fact: &str, cat: &str, entity: &str) -> String {
        let r = st
            .add_fact(fact, cat, entity, &FactOpts::default())
            .unwrap()
            .unwrap();
        get_str(&r, "id")
    }

    #[test]
    fn near_duplicates() {
        assert!(near_dup(
            "The user prefers aisle seats on flights.",
            "The user prefers an aisle seat on flights."
        ));
        assert!(!near_dup(
            "The user is a vegetarian",
            "The user is not a vegetarian"
        ));
        assert!(near_dup(
            "Lucas likes quiet hotels!",
            "Lucas likes the quiet hotels"
        ));
        assert!(!near_dup(
            "The user's sister Anna lives in Tokyo",
            "The user's sister Anna lives in Osaka"
        ));
        assert_eq!(mask("Card 4111 1111 1111 1234"), "Card •••• •••• •••• 1234");
    }

    #[tokio::test]
    async fn memory_tidy_plan_and_apply() {
        let (_d, st) = store();
        let a = add(
            &st,
            "The user prefers aisle seats on flights.",
            "preference",
            "",
        );
        let b = add(
            &st,
            "The user prefers an aisle seat on flights.",
            "preference",
            "",
        );
        let c = add(&st, "Opened zipair.net and clicked search", "other", "");
        let s = add(&st, "Card 4111 1111 1111 1234", "other", "");
        let keep = add(
            &st,
            "The user's sister Anna lives in Tokyo",
            "person",
            "Anna",
        );
        let other = add(
            &st,
            "The user's sister Anna lives in Osaka",
            "person",
            "Anna",
        );
        let llm = fixed_llm(json!({
            "demote": [{"id": c, "reason": "log"}, {"id": "nope"}],
            "profile": [{"field": "phone", "value": "+65 9123 4567"}],
            "rewrite": [{"id": keep, "fact": "x".repeat(400)}],
        }));
        let rt = Rt {
            store: st,
            llm,
            sent: Mutex::new(vec![]),
        };
        let prev = run(&rt, true, "manual", None).await.unwrap();
        let st = &rt.store;
        assert!(st.fact(&b).unwrap().is_some() && st.fact(&s).unwrap().is_some());
        assert!(rt.sent.lock().is_empty()); // preview changes nothing
        let prev_id = prev["id"].as_i64().unwrap();
        let done = run(&rt, false, "manual", Some(prev_id)).await.unwrap();
        assert_eq!(done["done"]["merged"], json!(1));
        assert_eq!(done["done"]["sensitive"], json!(1));
        assert_eq!(done["done"]["demoted"], json!(1));
        assert!(st.fact(&a).unwrap().is_some() && st.fact(&b).unwrap().is_none());
        assert!(get_str(&st.fact(&a).unwrap().unwrap(), "history").contains("an aisle seat"));
        assert_eq!(get_str(&st.fact(&c).unwrap().unwrap(), "tier"), "recent");
        assert!(st.fact(&keep).unwrap().is_some() && st.fact(&other).unwrap().is_some());
        assert_eq!(
            get_str(&st.fact(&keep).unwrap().unwrap(), "fact"),
            "The user's sister Anna lives in Tokyo"
        ); // over-long rewrite ignored
        assert_eq!(st.profile().unwrap(), json!({}));
        assert_eq!(
            get_str(&st.profile_pending("pending").unwrap()[0], "value"),
            "+65 9123 4567"
        );
        let sent = rt.sent.lock().clone();
        assert!(!sent.is_empty() && !sent[0].contains("4111"));
        assert!(sent[0].starts_with("🧠 记忆整理完成\n长期 "));
        let runs = st.memory_runs(10).unwrap();
        let prev_row = runs
            .iter()
            .find(|r| r["id"].as_i64() == Some(prev_id))
            .unwrap();
        assert_eq!(prev_row["report"]["applied"], done["id"]);
        // a preview applies once
        let err = run(&rt, false, "manual", Some(prev_id)).await.unwrap_err();
        assert!(err.to_string().contains("已执行"));
    }

    #[tokio::test]
    async fn profile_suggestion_shapes_and_one_per_field() {
        let (_d, st) = store();
        let f = add(&st, "The user prefers metal pens.", "preference", "");
        assert!(st
            .suggest_profile("email_work", "bytetrade", "", "x")
            .unwrap()
            .is_none());
        let llm = fixed_llm(json!({
            "rewrite": [{"id": f, "fact": "The user prefers metal pens, such as Parker pens and Lamy."}],
            "profile": [{"field": "address_work", "value": "20 Anson Rd"},
                        {"field": "address_work", "value": "#1001 20 Anson Rd, Singapore 079912"},
                        {"field": "email_work", "value": "bytetrade"}],
        }));
        let p = plan(&st, &llm).await.unwrap();
        assert_eq!(p["llm"]["rewrite"], json!([])); // rewrites may not grow / add details
        let prof: Vec<(String, String)> = p["llm"]["profile"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| (get_str(s, "field"), get_str(s, "value")))
            .collect();
        assert_eq!(
            prof,
            vec![(
                "address_work".to_string(),
                "#1001 20 Anson Rd, Singapore 079912".to_string()
            )]
        );
        assert!(!needs_first_review(&st).unwrap());
        assert_eq!(job_state(), json!({"running": false}));
    }

    #[test]
    fn summary_and_due() {
        let (_d, st) = store();
        let p = json!({"local": {"prune_facts": ["a"], "prune_episodes": 2, "sensitive": [], "dups": [{"keep": "k", "drop": ["x", "y"]}], "auto_promote": []},
                       "llm": {"merge": [], "rewrite": [], "demote": [{"id": "d"}], "promote": [], "profile": [{"field": "phone"}]}});
        let before = json!({"long": 5, "recent": 1, "episodes": 3});
        let zh = summary_lines(&p, None, &before, None, false);
        assert_eq!(zh[0], "长期 5 条 · 近期 1 条 · 经历 3 条");
        assert_eq!(zh[1], "合并重复：2 条 · 改写：0 条");
        assert_eq!(zh[2], "降为近期（30 天后过期）：1 条 · 升为长期：0 条");
        assert_eq!(zh[3], "过期清理：记忆 1 条，经历 2 条");
        assert_eq!(zh[4], "有 1 条档案修改建议等你确认（记忆页）");
        let en = summary_lines(
            &p,
            None,
            &before,
            Some(&json!({"long": 4, "recent": 2, "episodes": 1})),
            true,
        );
        assert_eq!(
            en[0],
            "Long-term 5 · recent 1 · episodes 3 → long-term 4 · recent 2 · episodes 1"
        );
        assert_eq!(
            en[4],
            "Profile suggestions waiting for you: 1 (Memory page)"
        );
        // due: after 03:30 UTC on an empty memory, once per day
        st.set_settings(&json!({"timezone": "UTC"})).unwrap();
        let noon = 1_700_000_000.0_f64 - (1_700_000_000.0_f64 % 86400.0) + 12.0 * 3600.0;
        assert!(due(&st, Some(noon)).unwrap());
        assert!(!due(&st, Some(noon - 11.0 * 3600.0)).unwrap()); // 01:00 < 03:30
        st.set_settings(&json!({"memory_consolidation": false}))
            .unwrap();
        assert!(!due(&st, Some(noon)).unwrap());
    }
}
