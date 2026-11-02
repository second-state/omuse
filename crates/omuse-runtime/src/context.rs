//! Personal context: long-term memory sorted into domains, and the few facts that matter for a request handed to the
//! agent at the start of the task, so it does not ask the user again for things it already knows (shoe size, cabin
//! class, preferred cuisine, which mailbox to send from …).
//!
//! Domains (域):
//! ```text
//!   person      人物与关系      people the user deals with and how they relate (colleagues, travel companions, family)
//!   preference  偏好            likes, sizes, styles, cuisines, cabin class, formats
//!   place       地点            home / office / city / time zone
//!   account     账户与会员      mailboxes, memberships, subscriptions, phone lines (numbers themselves live in the vault)
//!   site        网站习惯        how a given website works for the user (guest checkout, phone format, delivery option)
//!   rule        默认规则        standing instructions for OMuse ("always …", "never …", spend caps, fixed routines)
//!   work        工作与其他      companies, projects, everything else
//! ```
//!
//! Natural language is the model's (CLAUDE.md): code never reads the request's words or a fact's prose to choose.
//!
//! * **Writing.** Every fact carries structured fields set by the model when it writes memory (`memory_remember`, the
//!   extraction after a task, the sorting step `memory_tidy::classify` for facts written without them): domain,
//!   topics ([`crate::store::TOPICS`]), scope (always · topic · trigger + its phrase), kind (id_document), entity and
//!   the user's relation to it. Code validates them against the enums.
//! * **Reading the request.** At the start of a task the model turns the request into JSON choices from lists it is
//!   given ([`relevance`]): the topics it is about, the known entities it names (with their relation and aliases, so
//!   "my wife" finds Mei), the trigger phrases the user said, and — for facts not sorted yet — the ids that bear on it.
//! * **Selecting.** [`select`] scores the facts on those fields alone, plus structured task data ([`task_data`]: the
//!   bot, the skills loaded, the workflow tools called). Identity-document facts never enter the prompt (the model asks
//!   for them with `memory_search` `id_documents`). Facts learned by OMuse itself (not stated by the user) are
//!   "pending" until the user confirms them on the Memory page; they are still used, marked "unconfirmed".

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, LazyLock};

use omuse_common::util::{as_f64, get_str, py_str, take_chars};
use regex::Regex;
use serde_json::{json, Map, Value};

use crate::llm::{extract_json, ChatOpts, Llm};
use crate::store::{fact_topics, topic_keys, unsorted, Store, TOPICS};

pub use crate::store::{domain_label, render_facts as render, DOMAINS, DOMAIN_KEYS, USE_RULE};

/// At most this many entities, unsorted facts and trigger phrases go into the relevance call.
const MAX_ENTITIES: usize = 60;
const MAX_UNSORTED: usize = 40;

// ------------------------------------------------------------------------------------------------ task data
/// What the task is known to be about from structured data alone: the bot, the skills it loaded and the workflow /
/// connector tools it called (names from the transcript's tool calls, never the words).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TaskData {
    pub topics: Vec<String>,
}

/// The topics of a bot (`bots/<id>.md`).
pub fn bot_topics(bot_id: &str) -> &'static [&'static str] {
    match bot_id {
        "travel" => &["travel"],
        "insurance" => &["travel", "finance", "health"],
        "cards" => &["finance"],
        _ => &[],
    }
}

/// The topics of a skill, by its name (`skills/<name>/SKILL.md`).
pub fn skill_topics(name: &str) -> &'static [&'static str] {
    match name {
        "flight-booking"
        | "hotel-booking"
        | "hotel-requests"
        | "travel-planning"
        | "getting-around-asia"
        | "payments-connectivity-asia"
        | "flight-disruption"
        | "trip-today"
        | "entry-requirements" => &["travel"],
        "arrival-cards" | "eu-etias" | "uk-eta" | "jp-visit-japan-web" | "ph-etravel" => {
            &["travel", "forms"]
        }
        "baggage-delayed-lost" | "csr-trip-claim" | "fwd-travel-insurance" | "hk-apdt-refund" => {
            &["travel", "finance"]
        }
        "travel-health-emergency" => &["travel", "health"],
        "travel-expense-report" => &["travel", "finance", "documents"],
        "online-shopping" | "order-tracking" | "tax-refund-shopping" | "web-research-compare" => {
            &["shopping"]
        }
        "restaurant-booking" | "michelin-dining" | "dining-dietary-translate" => &["dining"],
        "reply-email" | "morning-email-brief" => &["email"],
        "news-research" | "news-search" => &["news", "documents"],
        "deep-research" => &["documents"],
        "pdf-forms" => &["forms"],
        "phone-call" => &["phone_calls"],
        "credit-card-picker" => &["finance"],
        n if n.ends_with("-arrival-card") => &["travel", "forms"],
        _ => &[],
    }
}

/// The topics of a tool, by its name.
pub fn tool_topics(name: &str) -> &'static [&'static str] {
    match name {
        "travel_search" | "travel_book" | "travel_pick" | "flights_search" | "flight_offer"
        | "hotels_search" | "hotel_rooms" | "cars_search" | "bag_trace" | "apdt_refund"
        | "entry_requirements" => {
            &["travel"]
        }
        "arrival_card_fill" => &["travel", "forms"],
        "claim_file" | "insurance_buy" => &["travel", "finance"],
        "shop_order" | "orders_list" => &["shopping"],
        "news_gather" | "news_publish" => &["news", "documents"],
        "phone_call" => &["phone_calls"],
        "make_pdf" | "make_docx" | "make_xlsx" => &["documents"],
        "pdf_form_fields" | "pdf_form_fill" => &["forms"],
        n if n.starts_with("gmail_") => &["email"],
        n if n.starts_with("calendar_") => &["calendar"],
        _ => &[],
    }
}

/// [`TaskData`] from the task's bot and its transcript (the tool calls made so far: `load_skill {name}` and the
/// tools by name).
pub fn task_data(bot_id: &str, transcript: &[Value]) -> TaskData {
    let mut topics: Vec<String> = Vec::new();
    let mut add = |ts: &[&str]| {
        for t in ts {
            if !topics.iter().any(|x| x == t) {
                topics.push(t.to_string());
            }
        }
    };
    add(bot_topics(bot_id));
    for m in transcript {
        let Some(calls) = m.get("tool_calls").and_then(Value::as_array) else {
            continue;
        };
        for c in calls {
            let f = c.get("function").unwrap_or(c);
            let name = get_str(f, "name");
            if name == "load_skill" {
                let args = match f.get("arguments") {
                    Some(Value::String(s)) => serde_json::from_str(s).unwrap_or(Value::Null),
                    Some(v) => v.clone(),
                    None => Value::Null,
                };
                add(skill_topics(&get_str(&args, "name")));
            } else {
                add(tool_topics(&name));
            }
        }
    }
    TaskData { topics }
}

// ------------------------------------------------------------------------------------------------ the model's reading
/// A known entity the model may name: its name, the user's relation to it, its aliases, and one fact about it (so
/// the model can tell who "my wife" is).
#[derive(Debug, Clone, PartialEq)]
pub struct EntityChoice {
    pub name: String,
    pub relation: String,
    pub aliases: Vec<String>,
    pub about: String,
}

/// The lists the model chooses from when it reads a request.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Menu {
    pub entities: Vec<EntityChoice>,
    pub triggers: Vec<String>,
    /// (id, text) of facts not sorted yet
    pub unsorted: Vec<(String, String)>,
    /// whether any fact applies by topic (else the topics need no answer)
    pub by_topic: bool,
}

impl Menu {
    /// Nothing for the model to choose: no topic-scoped fact, no entity, no trigger, nothing unsorted.
    pub fn is_empty(&self) -> bool {
        !self.by_topic
            && self.entities.is_empty()
            && self.triggers.is_empty()
            && self.unsorted.is_empty()
    }
}

/// What the model made of the request: the topics, the entities (canonical names from the menu), the trigger
/// phrases the user said and the unsorted fact ids that bear on it — every item one of the listed choices.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Relevance {
    pub topics: Vec<String>,
    pub entities: Vec<String>,
    pub triggers: Vec<String>,
    pub facts: Vec<String>,
}

fn aliases_of(e: &Value) -> Vec<String> {
    e.get("attrs")
        .and_then(|a| a.get("aliases"))
        .and_then(Value::as_array)
        .map(|al| {
            al.iter()
                .map(|a| match a {
                    Value::String(s) => s.trim().to_string(),
                    other => py_str(other),
                })
                .filter(|a| !a.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

fn uses(f: &Value) -> f64 {
    f.get("uses").and_then(as_f64).unwrap_or(0.0)
}

/// The menu for a request from the live long-term facts and the entities (Memory page: aliases, relation).
pub fn build_menu(facts: &[Value], entities: &[Value]) -> Menu {
    let mut by_uses: Vec<&Value> = facts
        .iter()
        .filter(|f| get_str(f, "kind") != "id_document")
        .collect();
    by_uses.sort_by(|a, b| {
        uses(b)
            .partial_cmp(&uses(a))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let mut ents: Vec<EntityChoice> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    let about = |name: &str| -> String {
        by_uses
            .iter()
            .find(|f| get_str(f, "entity").trim().eq_ignore_ascii_case(name))
            .map(|f| take_chars(&get_str(f, "fact"), 140).to_string())
            .unwrap_or_default()
    };
    // the facts' entities first (most used), then the ones only on the Memory page
    for f in &by_uses {
        let name = get_str(f, "entity").trim().to_string();
        if name.chars().count() < 2 || !seen.insert(name.to_lowercase()) {
            continue;
        }
        let e = entities
            .iter()
            .find(|e| get_str(e, "name").trim().eq_ignore_ascii_case(&name));
        ents.push(EntityChoice {
            relation: e.map(|e| get_str(e, "relation")).unwrap_or_default(),
            aliases: e.map(aliases_of).unwrap_or_default(),
            about: about(&name),
            name,
        });
    }
    for e in entities {
        let name = get_str(e, "name").trim().to_string();
        if name.chars().count() < 2 || !seen.insert(name.to_lowercase()) {
            continue;
        }
        ents.push(EntityChoice {
            relation: get_str(e, "relation"),
            aliases: aliases_of(e),
            about: String::new(),
            name,
        });
    }
    ents.truncate(MAX_ENTITIES);
    let mut triggers: Vec<String> = Vec::new();
    for f in &by_uses {
        let t = get_str(f, "trigger").trim().to_string();
        if get_str(f, "scope") == "trigger" && !t.is_empty() && !triggers.contains(&t) {
            triggers.push(t);
        }
    }
    let unsorted_list: Vec<(String, String)> = by_uses
        .iter()
        .filter(|f| unsorted(f))
        .take(MAX_UNSORTED)
        .map(|f| {
            (
                get_str(f, "id"),
                take_chars(&get_str(f, "fact"), 200).to_string(),
            )
        })
        .collect();
    Menu {
        entities: ents,
        triggers,
        unsorted: unsorted_list,
        by_topic: by_uses.iter().any(|f| get_str(f, "scope") == "topic"),
    }
}

/// The user message of the relevance call: the request (and the user's recent messages), then the lists to choose
/// from as JSON.
pub fn relevance_prompt(request: &str, recent: &str, menu: &Menu) -> String {
    let topics: Map<String, Value> = TOPICS
        .iter()
        .map(|(k, d)| (k.to_string(), json!(d)))
        .collect();
    let entities: Vec<Value> = menu
        .entities
        .iter()
        .map(|e| {
            let mut m = Map::new();
            m.insert("name".into(), json!(e.name));
            if !e.relation.is_empty() {
                m.insert("relation".into(), json!(e.relation));
            }
            if !e.aliases.is_empty() {
                m.insert("aliases".into(), json!(e.aliases));
            }
            if !e.about.is_empty() {
                m.insert("about".into(), json!(e.about));
            }
            Value::Object(m)
        })
        .collect();
    let unsorted: Vec<Value> = menu
        .unsorted
        .iter()
        .map(|(id, f)| json!({"id": id, "fact": f}))
        .collect();
    let choices = json!({"topics": topics, "entities": entities, "triggers": menu.triggers, "facts": unsorted});
    let mut s = format!("The request:\n{}\n", take_chars(request.trim(), 2000));
    if !recent.trim().is_empty() {
        s += &format!(
            "\nThe user's recent messages in this chat (for a follow-up):\n{}\n",
            take_chars(recent.trim(), 1500)
        );
    }
    s += &format!("\nChoices:\n{}", choices);
    s
}

fn str_items(v: &Value, key: &str) -> Option<Vec<String>> {
    match v.get(key) {
        None | Some(Value::Null) => Some(vec![]),
        Some(Value::Array(a)) => Some(
            a.iter()
                .filter_map(|x| match x {
                    Value::String(s) => Some(s.trim().to_string()),
                    Value::Object(o) => o
                        .get("name")
                        .or_else(|| o.get("id"))
                        .and_then(Value::as_str)
                        .map(|s| s.trim().to_string()),
                    _ => None,
                })
                .collect(),
        ),
        Some(Value::String(s)) => Some(vec![s.trim().to_string()]),
        Some(_) => None,
    }
}

/// The model's answer, every item checked against the menu: topics in [`TOPICS`], entities by name or alias (any
/// case) → the canonical name, triggers exactly as listed (any case), fact ids as listed. `None` when the reply is
/// not a JSON object with these lists.
pub fn parse_relevance(reply: &str, menu: &Menu) -> Option<Relevance> {
    let v = extract_json(reply)?;
    if !v.is_object() {
        return None;
    }
    let keys = topic_keys();
    let mut rel = Relevance::default();
    for t in str_items(&v, "topics")? {
        let t = t.to_lowercase();
        if keys.contains(&t.as_str()) && !rel.topics.contains(&t) {
            rel.topics.push(t);
        }
    }
    for n in str_items(&v, "entities")? {
        let hit = menu.entities.iter().find(|e| {
            e.name.eq_ignore_ascii_case(&n) || e.aliases.iter().any(|a| a.eq_ignore_ascii_case(&n))
        });
        if let Some(e) = hit {
            if !rel.entities.contains(&e.name) {
                rel.entities.push(e.name.clone());
            }
        }
    }
    for t in str_items(&v, "triggers")? {
        if let Some(x) = menu.triggers.iter().find(|x| x.eq_ignore_ascii_case(&t)) {
            if !rel.triggers.contains(x) {
                rel.triggers.push(x.clone());
            }
        }
    }
    for id in str_items(&v, "facts")? {
        if menu.unsorted.iter().any(|(i, _)| *i == id) && !rel.facts.contains(&id) {
            rel.facts.push(id);
        }
    }
    Some(rel)
}

/// The model reads the request and answers with choices from the menu (temperature 0, no tools, JSON only); a reply
/// that is not that JSON gets one retry, then nothing is chosen (the facts that always apply still come). This is
/// the model's side of the line — language into JSON — never a code tool calling the model.
pub async fn relevance(
    llm: &Llm,
    task_id: &str,
    request: &str,
    recent: &str,
    menu: &Menu,
) -> Relevance {
    if menu.is_empty() || request.trim().is_empty() {
        return Relevance::default();
    }
    let o = ChatOpts {
        temperature: Some(0.0),
        max_tokens: Some(400),
        purpose: "memory".into(),
        task_id: task_id.to_string(),
        model: None,
        no_think: true,
        stream_to: None,
        tool_choice: None,
    };
    let mut msgs = vec![
        json!({"role": "system", "content": crate::prompts::MEMORY_RELEVANCE}),
        json!({"role": "user", "content": relevance_prompt(request, recent, menu)}),
    ];
    for _ in 0..2 {
        let Ok(r) = llm.chat(&msgs, None, &o).await else {
            break;
        };
        if let Some(rel) = parse_relevance(&r.content, menu) {
            return rel;
        }
        msgs.push(json!({"role": "assistant", "content": take_chars(&r.content, 300)}));
        msgs.push(json!({"role": "user", "content":
            "Answer with the JSON object only: {\"topics\": [...], \"entities\": [...], \"triggers\": [...], \"facts\": [...]}, each item copied from the choices."}));
    }
    Relevance::default()
}

// ------------------------------------------------------------------------------------------------ selection
fn with_extra(f: &Value, score: f64, why: &str) -> Value {
    let mut m: Map<String, Value> = f.as_object().cloned().unwrap_or_default();
    if get_str(f, "domain").is_empty() {
        m.insert("domain".into(), json!("work"));
    }
    m.insert("score".into(), Value::from(score));
    m.insert("why".into(), Value::String(why.to_string()));
    Value::Object(m)
}

/// The facts that matter for this request, best first, from their structured fields: a trigger the user said (10),
/// an entity the request names (6, with its aliases), a rule that always applies (5), shared topics with the
/// request or the task (3 + up to 2; a site habit only when its site is named), an unsorted fact the model picked
/// (4); plus a little for facts used often. Identity-document facts never. Each returned fact gets `score` and `why`.
pub fn select(
    facts: &[Value],
    rel: &Relevance,
    task: &TaskData,
    entities: &[Value],
    limit: usize,
) -> Vec<Value> {
    let mut topics: Vec<&str> = rel.topics.iter().map(String::as_str).collect();
    for t in &task.topics {
        if !topics.contains(&t.as_str()) {
            topics.push(t.as_str());
        }
    }
    // the chosen entities with their aliases, lower-cased
    let mut names: HashSet<String> = HashSet::new();
    for n in &rel.entities {
        names.insert(n.to_lowercase());
        if let Some(e) = entities
            .iter()
            .find(|e| get_str(e, "name").trim().eq_ignore_ascii_case(n))
        {
            names.extend(aliases_of(e).iter().map(|a| a.to_lowercase()));
        }
    }
    let mut scored: Vec<Value> = Vec::new();
    for f in facts {
        let kind = get_str(f, "kind");
        if kind == "id_document" {
            continue; // passport / ID / birth date: only when a form needs them (memory_search id_documents)
        }
        let id = get_str(f, "id");
        if unsorted(f) {
            if rel.facts.contains(&id) {
                scored.push(with_extra(f, 4.0 + (uses(f) / 50.0).min(1.0), "picked"));
            }
            continue;
        }
        let scope = get_str(f, "scope");
        if scope == "trigger" {
            let t = get_str(f, "trigger");
            if rel.triggers.iter().any(|x| x.eq_ignore_ascii_case(&t)) {
                scored.push(with_extra(f, 10.0, &format!("trigger: {t}")));
            }
            continue;
        }
        let mut s = 0.0_f64;
        let mut why: Vec<String> = Vec::new();
        let ent = get_str(f, "entity").trim().to_lowercase();
        let by_entity = !ent.is_empty() && names.contains(&ent);
        if by_entity {
            s += 6.0;
            why.push(format!("entity: {}", get_str(f, "entity").trim()));
        }
        if scope == "always" {
            s += 5.0;
            why.push("always".into());
        }
        let shared: Vec<String> = fact_topics(f)
            .into_iter()
            .filter(|t| topics.contains(&t.as_str()))
            .collect();
        let site_elsewhere = get_str(f, "domain") == "site" && !by_entity;
        if !shared.is_empty() && !site_elsewhere {
            s += 3.0 + std::cmp::min(2, shared.len() - 1) as f64;
            why.push(format!("topics: {}", shared.join("/")));
        }
        if s >= 3.0 {
            scored.push(with_extra(
                f,
                s + (uses(f) / 50.0).min(1.0),
                &why.join("; "),
            ));
        }
    }
    scored.sort_by(|a, b| {
        let sa = a.get("score").and_then(as_f64).unwrap_or(0.0);
        let sb = b.get("score").and_then(as_f64).unwrap_or(0.0);
        sb.partial_cmp(&sa).unwrap_or(std::cmp::Ordering::Equal)
    });
    scored.truncate(limit);
    scored
}

// ------------------------------------------------------------------------------------------------ for a task
/// The model's reading per task (the executor asks for a plan, or loads a skill: the same task, no second call).
static READINGS: LazyLock<parking_lot::Mutex<HashMap<String, (String, Relevance)>>> =
    LazyLock::new(|| parking_lot::Mutex::new(HashMap::new()));

fn cached(task_id: &str, goal: &str) -> Option<Relevance> {
    READINGS
        .lock()
        .get(task_id)
        .filter(|(g, _)| g == goal)
        .map(|(_, r)| r.clone())
}

fn keep_reading(task_id: &str, goal: &str, rel: &Relevance) {
    if task_id.is_empty() {
        return;
    }
    let mut m = READINGS.lock();
    if m.len() > 256 {
        m.clear();
    }
    m.insert(task_id.to_string(), (goal.to_string(), rel.clone()));
}

/// The long-term facts for a task: the model reads the request against the menu (once per task), [`select`] picks
/// on the structured fields and the task data, and the store records what the task used (an episode of the task
/// then carries the fact ids). Facts not sorted yet are sorted in the background (`memory_tidy::classify`).
pub async fn facts_for_task(
    store: &Arc<Store>,
    llm: &Arc<Llm>,
    task_id: &str,
    goal: &str,
    recent: &str,
    task: &TaskData,
) -> (Vec<Value>, Relevance) {
    let facts = store.facts(500, Some("long")).unwrap_or_default();
    if facts.is_empty() {
        return (vec![], Relevance::default());
    }
    if facts.iter().any(unsorted) {
        crate::memory_tidy::classify_in_background(store.clone(), llm.clone());
    }
    let ents = store.entities(None).unwrap_or_default();
    let rel = match cached(task_id, goal) {
        Some(r) => r,
        None => {
            let menu = build_menu(&facts, &ents);
            let r = relevance(llm, task_id, goal, recent, &menu).await;
            keep_reading(task_id, goal, &r);
            r
        }
    };
    let out = select(&facts, &rel, task, &ents, 12);
    let ids: Vec<String> = out.iter().map(|f| get_str(f, "id")).collect();
    let _ = store.mark_used(&ids);
    let mut topics = rel.topics.clone();
    topics.extend(task.topics.iter().cloned());
    let _ = store.note_task_memory(task_id, &topics, &rel.entities, &ids);
    (out, rel)
}

// ------------------------------------------------------------------------------------------------ sites
/// `'https://www.decathlon.sg/p/123'` → `'decathlon'`.
pub fn site_name(url: &str) -> String {
    static SCHEME_HOST: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"^[a-z]+://([^/:?#]+)").unwrap());
    let low = url.trim().to_lowercase();
    let Some(m) = SCHEME_HOST.captures(&low) else {
        return String::new();
    };
    let mut parts: Vec<&str> = m[1]
        .split('.')
        .filter(|p| *p != "www" && *p != "m")
        .collect();
    if parts.len() >= 3
        && ["com", "co", "org", "net", "gov", "edu"].contains(&parts[parts.len() - 2])
    {
        parts.truncate(parts.len() - 2);
    } else if parts.len() >= 2 {
        parts.truncate(parts.len() - 1);
    }
    parts.last().map(|p| p.to_string()).unwrap_or_default()
}

/// The site an entity names (a structured field): `decathlon.sg` / `https://www.decathlon.sg/` → `decathlon`; a
/// plain name → itself, lower-cased.
fn entity_site(entity: &str) -> String {
    let e = entity.trim().to_lowercase();
    if e.is_empty() || e.contains(' ') || e.contains('@') {
        return e;
    }
    if e.contains("://") {
        return site_name(&e);
    }
    if e.contains('.') {
        return site_name(&format!("https://{e}"));
    }
    e
}

/// Site habits (and other facts) about the website at `url`: the facts whose entity is that site (the entity, or
/// one of its aliases on the Memory page, names the same site).
pub fn site_facts(url: &str, facts: &[Value], entities: &[Value]) -> Vec<Value> {
    let name = site_name(url);
    if name.chars().count() < 3 {
        return vec![];
    }
    let same = |entity: &str| -> bool {
        if entity.trim().is_empty() {
            return false;
        }
        if entity_site(entity) == name {
            return true;
        }
        entities
            .iter()
            .find(|e| {
                get_str(e, "name")
                    .trim()
                    .eq_ignore_ascii_case(entity.trim())
            })
            .is_some_and(|e| aliases_of(e).iter().any(|a| entity_site(a) == name))
    };
    let mut out: Vec<Value> = facts
        .iter()
        .filter(|f| get_str(f, "kind") != "id_document" && same(&get_str(f, "entity")))
        .filter(|f| {
            ["site", "account", "rule", "preference"].contains(&get_str(f, "domain").as_str())
        })
        .cloned()
        .collect();
    out.sort_by(|a, b| {
        let ka = (get_str(a, "domain") != "site", -uses(a));
        let kb = (get_str(b, "domain") != "site", -uses(b));
        ka.0.cmp(&kb.0)
            .then(ka.1.partial_cmp(&kb.1).unwrap_or(std::cmp::Ordering::Equal))
    });
    out.truncate(5);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fact(
        id: &str,
        fact: &str,
        domain: &str,
        entity: &str,
        scope: &str,
        topics: &[&str],
    ) -> Value {
        json!({"id": id, "fact": fact, "domain": domain, "entity": entity, "scope": scope, "kind": "general",
               "topics": topics, "uses": 0, "status": "active", "trigger": ""})
    }

    /// Facts shaped like a real user's memory (invented wording), with the fields the model sets when it writes them.
    fn memory() -> Vec<Value> {
        let mut v = vec![
            fact(
                "shoes",
                "我身高177，体重76公斤，鞋子42码，运动鞋43码。",
                "preference",
                "",
                "topic",
                &["clothing", "shopping"],
            ),
            fact(
                "site",
                "shop.example 网站支持访客结账；电话号码填 8 位，不要 +65。",
                "site",
                "shop.example",
                "topic",
                &["shopping", "forms"],
            ),
            fact(
                "cabin",
                "The user prefers business class seats.",
                "preference",
                "",
                "topic",
                &["travel"],
            ),
            fact(
                "hunan",
                "The user prefers dining at Hunan cuisine restaurants.",
                "preference",
                "",
                "topic",
                &["dining"],
            ),
            fact(
                "pens",
                "The user prefers metal pens with an aesthetic design.",
                "preference",
                "",
                "topic",
                &["shopping", "gifts"],
            ),
            fact(
                "mei",
                "The user's wife Mei collects jazz vinyl records.",
                "person",
                "Mei",
                "topic",
                &["gifts"],
            ),
            fact(
                "auto",
                "The user prefers the assistant to act autonomously.",
                "rule",
                "",
                "always",
                &[],
            ),
            fact(
                "pdf",
                "The user prefers receiving reports in Chinese, as PDF.",
                "preference",
                "",
                "topic",
                &["documents", "news"],
            ),
        ];
        let mut trip = fact(
            "trip",
            "以后每次说「准备出差」，先查邮件里的航班和酒店，再生成一页行程 PDF。",
            "rule",
            "",
            "trigger",
            &["travel"],
        );
        trip["trigger"] = json!("准备出差");
        v.push(trip);
        let mut passport = fact(
            "pp",
            "The user's passport expires in March 2027.",
            "account",
            "",
            "topic",
            &["travel", "forms"],
        );
        passport["kind"] = json!("id_document");
        v.push(passport);
        v.push(json!({"id": "old", "fact": "The user lives in Singapore.", "domain": "place", "entity": "", "scope": "",
                      "kind": "", "topics": [], "uses": 3, "status": "active"}));
        v
    }

    fn ids(sel: &[Value]) -> Vec<String> {
        sel.iter().map(|f| get_str(f, "id")).collect()
    }

    fn rel(topics: &[&str], entities: &[&str]) -> Relevance {
        Relevance {
            topics: topics.iter().map(|s| s.to_string()).collect(),
            entities: entities.iter().map(|s| s.to_string()).collect(),
            ..Default::default()
        }
    }

    #[test]
    fn selection_uses_the_structured_fields_only() {
        let facts = memory();
        // "help me pick running shoes on shop.example": the model chose clothing + shopping and the site
        let sel = select(
            &facts,
            &rel(&["clothing", "shopping"], &["shop.example"]),
            &TaskData::default(),
            &[],
            12,
        );
        let got = ids(&sel);
        assert!(
            got.contains(&"shoes".into())
                && got.contains(&"site".into())
                && got.contains(&"auto".into()),
            "{got:?}"
        );
        assert!(
            !got.contains(&"cabin".into())
                && !got.contains(&"mei".into())
                && !got.contains(&"trip".into())
        );
        assert_eq!(got[0], "site"); // the named site and a shared topic first
        assert!(get_str(&sel[0], "why").contains("entity: shop.example"));
        // a site habit is not handed over when its site is not in play, even on a shared topic
        let got = ids(&select(
            &facts,
            &rel(&["shopping"], &[]),
            &TaskData::default(),
            &[],
            12,
        ));
        assert!(
            got.contains(&"shoes".into())
                && got.contains(&"pens".into())
                && !got.contains(&"site".into())
        );
        // never an identity document; the unsorted fact only when the model picked it
        let mut r = rel(&["travel", "forms"], &[]);
        assert!(!ids(&select(&facts, &r, &TaskData::default(), &[], 12))
            .iter()
            .any(|x| x == "pp" || x == "old"));
        r.facts = vec!["old".into()];
        assert!(ids(&select(&facts, &r, &TaskData::default(), &[], 12)).contains(&"old".into()));
        // a trigger rule only when the model says the user said it
        assert!(!ids(&select(
            &facts,
            &rel(&["travel"], &[]),
            &TaskData::default(),
            &[],
            12
        ))
        .contains(&"trip".into()));
        let mut r = rel(&["travel"], &[]);
        r.triggers = vec!["准备出差".into()];
        let sel = select(&facts, &r, &TaskData::default(), &[], 12);
        assert_eq!(get_str(&sel[0], "id"), "trip");
        let txt = render(&sel, false);
        assert!(
            txt.contains("[默认规则 Rules & defaults]")
                && txt.contains("(the user said “准备出差”)")
        );
        // the task's own data adds topics: the Travel bot gets the cabin class with nothing chosen
        let got = ids(&select(
            &facts,
            &Relevance::default(),
            &task_data("travel", &[]),
            &[],
            12,
        ));
        assert!(
            got.contains(&"cabin".into())
                && got.contains(&"auto".into())
                && !got.contains(&"shoes".into())
        );
        assert!(select(&facts, &Relevance::default(), &TaskData::default(), &[], 1).len() == 1);
    }

    #[test]
    fn an_alias_on_the_memory_page_finds_the_person() {
        let facts = memory();
        let ents = vec![
            json!({"name": "Mei", "type": "person", "attrs": {"aliases": ["梅梅"]}, "relation": "wife"}),
        ];
        let menu = build_menu(&facts, &ents);
        let mei = menu.entities.iter().find(|e| e.name == "Mei").unwrap();
        assert_eq!(mei.relation, "wife");
        assert_eq!(mei.aliases, vec!["梅梅"]);
        assert!(mei.about.contains("jazz"));
        assert!(menu.entities.iter().any(|e| e.name == "shop.example"));
        assert_eq!(menu.triggers, vec!["准备出差"]);
        assert_eq!(
            menu.unsorted,
            vec![("old".into(), "The user lives in Singapore.".into())]
        );
        assert!(!menu.is_empty() && Menu::default().is_empty());
        // the model answered with the alias: the canonical name comes back, and the fact by its entity
        let r =
            parse_relevance("{\"topics\": [\"gifts\"], \"entities\": [\"梅梅\"]}", &menu).unwrap();
        assert_eq!(r.entities, vec!["Mei"]);
        let sel = select(&facts, &r, &TaskData::default(), &ents, 12);
        assert_eq!(get_str(&sel[0], "id"), "mei");
        // the prompt lists the choices as JSON (no passport fact anywhere in it)
        let p = relevance_prompt("Suggest a birthday present for my wife", "", &menu);
        assert!(
            p.contains("\"relation\":\"wife\"")
                && p.contains("\"travel\"")
                && p.contains("准备出差")
        );
        assert!(!p.contains("passport"));
    }

    #[test]
    fn the_models_answer_is_checked_against_the_lists() {
        let menu = build_menu(&memory(), &[]);
        let r = parse_relevance(
            "```json\n{\"topics\": [\"Travel\", \"weather\", \"travel\"], \"entities\": [\"shop.EXAMPLE\", \"Bob\"], \"triggers\": [\"准备出差\", \"go\"], \"facts\": [\"old\", \"pp\"]}\n```",
            &menu,
        )
        .unwrap();
        assert_eq!(r.topics, vec!["travel"]);
        assert_eq!(r.entities, vec!["shop.example"]);
        assert_eq!(r.triggers, vec!["准备出差"]);
        assert_eq!(r.facts, vec!["old"]); // an id that is not an unsorted choice is dropped
        assert_eq!(parse_relevance("{}", &menu), Some(Relevance::default()));
        assert!(parse_relevance("travel and shopping", &menu).is_none());
        assert!(parse_relevance("{\"topics\": 3}", &menu).is_none());
    }

    #[test]
    fn task_data_reads_names_only() {
        let transcript = vec![
            json!({"role": "user", "content": "book me a flight and a restaurant"}),
            json!({"role": "assistant", "tool_calls": [
                {"id": "1", "function": {"name": "load_skill", "arguments": "{\"name\": \"restaurant-booking\"}"}},
                {"id": "2", "function": {"name": "gmail_search", "arguments": "{}"}}]}),
            json!({"role": "assistant", "tool_calls": [{"id": "3", "function": {"name": "load_skill", "arguments": {"name": "th-arrival-card"}}}]}),
        ];
        assert_eq!(
            task_data("main", &transcript).topics,
            vec!["dining", "email", "travel", "forms"]
        );
        assert_eq!(
            task_data("insurance", &[]).topics,
            vec!["travel", "finance", "health"]
        );
        assert!(task_data("main", &[]).topics.is_empty());
    }

    #[test]
    fn sites_by_their_entity() {
        assert_eq!(site_name("https://www.decathlon.sg/p/123"), "decathlon");
        assert_eq!(site_name("https://www.amazon.com.sg/x"), "amazon");
        assert_eq!(site_name("https://shop.example/"), "shop");
        assert_eq!(site_name("not a url"), "");
        let facts = vec![
            fact(
                "a",
                "decathlon.sg 支持访客结账；电话填 8 位，不要 +65。",
                "site",
                "decathlon.sg",
                "topic",
                &["shopping"],
            ),
            fact("b", "The user likes jazz.", "preference", "", "topic", &[]),
            fact(
                "c",
                "迪卡侬 always delivers to the office.",
                "preference",
                "迪卡侬",
                "topic",
                &[],
            ),
        ];
        let found = site_facts("https://www.decathlon.sg/checkout", &facts, &[]);
        assert_eq!(ids(&found), vec!["a"]);
        // an alias set on the Memory page ties a name to the site
        let ents =
            vec![json!({"name": "迪卡侬", "type": "site", "attrs": {"aliases": ["decathlon.sg"]}})];
        assert_eq!(
            ids(&site_facts("https://www.decathlon.sg/", &facts, &ents)),
            vec!["a", "c"]
        );
        assert!(site_facts("https://www.ikea.com/sg", &facts, &ents).is_empty());
    }
    /// A model that answers every call with the same text, counting the calls.
    struct Scripted(String, Arc<std::sync::atomic::AtomicUsize>);

    impl crate::llm::LlmTransport for Scripted {
        fn send(
            &self,
            _req: crate::llm::HttpRequest,
        ) -> futures::future::BoxFuture<'_, Result<crate::llm::HttpResponse, String>> {
            self.1.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let body = json!({"choices": [{"message": {"role": "assistant", "content": self.0}}], "usage": {}});
            Box::pin(async move {
                Ok(crate::llm::HttpResponse {
                    status: 200,
                    retry_after: None,
                    text: omuse_common::pyjson::dumps(&body),
                })
            })
        }
    }

    #[tokio::test]
    async fn a_task_gets_the_facts_the_model_reading_points_to() {
        let d = tempfile::tempdir().unwrap();
        let st = Arc::new(
            Store::new(d.path(), Arc::new(omuse_common::Config::parse("").unwrap())).unwrap(),
        );
        let add = |fact: &str, entity: &str, fields: Value| {
            let o = crate::store::FactOpts::default().with_model_fields(&fields);
            get_str(&st.add_fact(fact, "", entity, &o).unwrap().unwrap(), "id")
        };
        let mei = add(
            "The user's wife Mei collects jazz vinyl records.",
            "Mei",
            json!({"domain": "person", "topics": ["gifts"], "scope": "topic", "relation": "wife"}),
        );
        add(
            "The user prefers business class.",
            "",
            json!({"domain": "preference", "topics": ["travel"], "scope": "topic"}),
        );
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let settings: crate::llm::SettingsFn = Arc::new(
            || json!({"model_base_url": "http://x/v1", "model_name": "m", "temperature": 0, "max_tokens": 10}),
        );
        let llm = Arc::new(Llm::with_transport(settings, None, None, Arc::new(Scripted(
            "{\"topics\": [\"gifts\"], \"entities\": [\"mei\"], \"triggers\": [], \"facts\": []}".into(), calls.clone()))));
        let (sel, rel) = facts_for_task(
            &st,
            &llm,
            "t1",
            "Suggest a birthday present for my wife",
            "",
            &TaskData::default(),
        )
        .await;
        assert_eq!(rel.entities, vec!["Mei"]);
        assert_eq!(ids(&sel), vec![mei.clone()]);
        // recorded for the task: its episode will carry the fact id
        assert_eq!(st.task_memory("t1").unwrap()["fact_ids"], json!([mei]));
        assert_eq!(st.task_memory("t1").unwrap()["entities"], json!(["Mei"]));
        // the same task asks again (a plan, a skill): no second model call
        let n = calls.load(std::sync::atomic::Ordering::SeqCst);
        facts_for_task(
            &st,
            &llm,
            "t1",
            "Suggest a birthday present for my wife",
            "",
            &TaskData::default(),
        )
        .await;
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), n);
        // an empty memory needs no call at all
        let d2 = tempfile::tempdir().unwrap();
        let empty = Arc::new(
            Store::new(
                d2.path(),
                Arc::new(omuse_common::Config::parse("").unwrap()),
            )
            .unwrap(),
        );
        assert!(
            facts_for_task(&empty, &llm, "t2", "hello", "", &TaskData::default())
                .await
                .0
                .is_empty()
        );
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), n);
    }
}
