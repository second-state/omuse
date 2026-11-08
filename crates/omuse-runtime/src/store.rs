//! Runtime persistent state: conversations, tasks, events, schedules, memory, settings
//! (`app/runtime/store.py`, class `RStore`).
//!
//! Every method keeps the Python name and returns `serde_json::Value` in the same shape the Python
//! dicts had, so the HTTP layer and the agent can be ported line by line. Keyword-argument methods
//! (`update_task(tid, status=..., plan=...)`) take a JSON object of the keywords.
//!
//! The settings defaults that Python read from the environment (`OMUSE_MODEL_URL`, `TZ`, …) come
//! from the [`Config`] the store holds.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::{Arc, LazyLock};

use anyhow::{anyhow, Result};
use fancy_regex::Regex as FancyRegex;
use omuse_common::db::Param;
use omuse_common::util::{get_str, get_truthy, loads, new_id, now_ts, py_str, take_chars, truthy};
use omuse_common::{params, pyjson, Config, Db};
use regex::Regex;
use serde_json::{json, Map, Value};

/// The SQLite schema (`SCHEMA` in `store.py`), byte for byte.
pub const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS settings (key TEXT PRIMARY KEY, value TEXT);
CREATE TABLE IF NOT EXISTS conversations (id TEXT PRIMARY KEY, title TEXT, kind TEXT DEFAULT 'chat', created_at REAL, updated_at REAL);
CREATE TABLE IF NOT EXISTS messages (
  id INTEGER PRIMARY KEY AUTOINCREMENT, conv_id TEXT, role TEXT, content TEXT, task_id TEXT, created_at REAL
);
CREATE INDEX IF NOT EXISTS ix_msg_conv ON messages(conv_id);
CREATE TABLE IF NOT EXISTS tasks (
  id TEXT PRIMARY KEY, conv_id TEXT, goal TEXT, status TEXT, plan TEXT, transcript TEXT, pending TEXT,
  result TEXT, error TEXT, source TEXT, schedule_id TEXT, parent_id TEXT, steps INTEGER DEFAULT 0,
  waiting TEXT, created_at REAL, updated_at REAL, finished_at REAL
);
CREATE INDEX IF NOT EXISTS ix_task_status ON tasks(status);
CREATE TABLE IF NOT EXISTS events (
  id INTEGER PRIMARY KEY AUTOINCREMENT, task_id TEXT, ts REAL, type TEXT, data TEXT
);
CREATE INDEX IF NOT EXISTS ix_ev_task ON events(task_id);
CREATE TABLE IF NOT EXISTS schedules (
  id TEXT PRIMARY KEY, name TEXT, goal TEXT, kind TEXT, spec TEXT, tz TEXT, enabled INTEGER,
  last_run REAL, next_run REAL, state TEXT, conv_id TEXT, created_at REAL, last_task TEXT
);
CREATE TABLE IF NOT EXISTS facts (
  id TEXT PRIMARY KEY, fact TEXT, category TEXT, entity TEXT, source TEXT, confidence REAL,
  created_at REAL, last_verified REAL, ttl_days INTEGER
);
CREATE VIRTUAL TABLE IF NOT EXISTS facts_fts USING fts5(id UNINDEXED, fact, entity, tokenize='trigram');
CREATE TABLE IF NOT EXISTS subjects (
  id TEXT PRIMARY KEY, slug TEXT UNIQUE, title TEXT, brief TEXT, status TEXT, conv_id TEXT, created_at REAL, updated_at REAL,
  last_task TEXT, runs INTEGER DEFAULT 0, chars INTEGER DEFAULT 0, error TEXT DEFAULT ''
);
CREATE VIRTUAL TABLE IF NOT EXISTS library_fts USING fts5(subject_id UNINDEXED, heading, text, tokenize='trigram');
CREATE TABLE IF NOT EXISTS episodes (id INTEGER PRIMARY KEY AUTOINCREMENT, ts REAL, task_id TEXT, summary TEXT);
CREATE TABLE IF NOT EXISTS entities (id TEXT PRIMARY KEY, type TEXT, name TEXT, attrs TEXT, created_at REAL);
CREATE TABLE IF NOT EXISTS relations (src TEXT, rel TEXT, dst TEXT, source TEXT, created_at REAL);
CREATE TABLE IF NOT EXISTS goals (
  id TEXT PRIMARY KEY, title TEXT, objective TEXT, criteria TEXT, status TEXT, deadline REAL, schedule_id TEXT,
  conv_id TEXT, progress TEXT, result TEXT, created_at REAL, updated_at REAL, finished_at REAL, ask TEXT DEFAULT ''
);
CREATE TABLE IF NOT EXISTS profile (key TEXT PRIMARY KEY, value TEXT, updated_at REAL, source TEXT);
CREATE TABLE IF NOT EXISTS profile_pending (
  id TEXT PRIMARY KEY, key TEXT, value TEXT, old TEXT, reason TEXT, source TEXT, status TEXT, created_at REAL, resolved_at REAL
);
CREATE TABLE IF NOT EXISTS memory_runs (id INTEGER PRIMARY KEY AUTOINCREMENT, ts REAL, kind TEXT, report TEXT);
CREATE TABLE IF NOT EXISTS notifications (
  id INTEGER PRIMARY KEY AUTOINCREMENT, ts REAL, title TEXT, body TEXT, task_id TEXT, level TEXT, read INTEGER DEFAULT 0
);
";

/// `DEFAULT_SETTINGS`: Settings → Models etc. The system's endpoint, key and model names come from `[model]`
/// in the config (the Python read `OMUSE_MODEL_URL`, `OMUSE_MODEL_API_KEY`, `OMUSE_MODEL` …). The user may
/// switch `model_override` on and use their own: then the `model_*` values saved here and their own
/// `model_api_key` are used instead, as a set; switched off, everything saved here is ignored and the system's
/// values are back (see [`effective_model`]). Key order is the Python dict's.
pub fn default_settings(cfg: &Config) -> Value {
    json!({
        "model_override": false,
        "model_api_key": "",      // the user's own bearer key (only used with model_override; never returned by the API)
        "model_base_url": cfg.model.url,
        "model_name": cfg.model.name,
        "planner_model": cfg.model.planner.trim(),
        "vision_model": cfg.model.vision.trim(),       // "" = the executor model
        "stt_model": cfg.model.stt.trim(),             // speech-to-text; "" = a whisper-like model on the endpoint, if any
        "image_base_url": "",     // the user's own Images endpoint for make_image ("" = their model endpoint); the system's
                                  // `[model] image_url` applies through system_model() while the system's models are in force
        "image_api_key": "",      // the user's own key for that endpoint ("" = model_api_key); never returned by the API
        "image_model": cfg.model.image,                // image model id; "" = auto-pick
        "video_base_url": "",     // the same for make_video (an OpenAI-Videos-compatible endpoint)
        "video_api_key": "",
        "video_model": cfg.model.video.trim(),         // video model id; "" = auto-pick
        "temperature": 0.3,
        "max_steps": 40,
        "llm_concurrency": 4,   // model requests in flight at once (parallel sub-agents each use one)
        "max_minutes": 20,      // time budget per run
        "max_tokens": 4096,
        "timezone": cfg.server.timezone,
        "user_name": "",
        "language": "",          // the agent's language (zh / en); "" = not chosen yet
        "ui_language": "",       // the web UI's language: "" = follow the browser
        "theme": "auto",         // auto / ink / paper
        "instance_name": "OMuse",
        "skills_disabled": null, // names turned off in Settings → Skills; null = never seeded
        "reply_language": "",    // "" = answer in Settings → Language; "match" = the language of each request
        "memory_extraction": true,
        "memory_consolidation": true,
        "memory_consolidate_at": "03:30",
        "profile_reminder": true,   // nudge to complete the profile (name, birthday, nationality) until done or dismissed
        "fast_mode": false,      // the user's own model: send service_tier "fast" (system models: [model] fast)
        "reasoning_effort": "",  // the user's own model: reasoning_effort ("" = not sent; system: [model] reasoning_effort)
        "llm_timeout": 600,
        "stream_answers": true,  // the answer shows in the chat while the model writes it (answer_delta events)
    })
}

/// The model settings a user may override as a set (Settings → Models → "use my own model").
pub const MODEL_KEYS: [&str; 9] = [
    "model_base_url",
    "model_name",
    "planner_model",
    "vision_model",
    "stt_model",
    "image_model",
    "image_base_url",
    "video_model",
    "video_base_url",
];
/// Bearer keys saved in Settings: the user's own model key and the optional keys of their image and video endpoints.
/// None of them is ever returned by the API or written to the audit log; `redacted()` reports `<key>_set` instead.
pub const SECRET_KEYS: [&str; 3] = ["model_api_key", "image_api_key", "video_api_key"];
/// Request options that follow the same rule as [`MODEL_KEYS`]: the config's values for the system's models, the
/// user's own (Settings → Models) only while `model_override` is on.
pub const MODEL_OPTIONS: [&str; 2] = ["fast_mode", "reasoning_effort"];
/// The values `reasoning_effort` may take ("" = not sent).
pub const REASONING_EFFORTS: [&str; 7] = ["", "none", "minimal", "low", "medium", "high", "xhigh"];

/// The system's [`MODEL_OPTIONS`] (`[model] fast`, `[model] reasoning_effort`); an unknown effort is not sent.
pub fn system_options(cfg: &Config) -> Value {
    let effort = cfg.model.reasoning_effort.trim().to_lowercase();
    let effort = if REASONING_EFFORTS.contains(&effort.as_str()) {
        effort
    } else {
        String::new()
    };
    json!({"fast_mode": cfg.model.fast, "reasoning_effort": effort})
}

/// What the user's own set must have before it can be switched on.
pub const MODEL_REQUIRED: [&str; 3] = ["model_base_url", "model_api_key", "model_name"];

/// `SYSTEM_MODEL`: the system's values of [`MODEL_KEYS`] (from the config), including its optional image and video
/// endpoints (`[model] image_url` / `video_url`; "" = the model endpoint).
pub fn system_model(cfg: &Config) -> Value {
    let d = default_settings(cfg);
    let mut m = Map::new();
    for k in MODEL_KEYS {
        m.insert(k.to_string(), d[k].clone());
    }
    m.insert("image_base_url".into(), json!(cfg.model.image_url.trim()));
    m.insert("video_base_url".into(), json!(cfg.model.video_url.trim()));
    Value::Object(m)
}

/// The system's bearer key for the model endpoint (`[model] api_key`, was `OMUSE_MODEL_API_KEY`). Config only:
/// it never shows up in Settings, the audit log or the API.
pub fn system_model_api_key(cfg: &Config) -> String {
    cfg.model.api_key.trim().to_string()
}

/// The model settings in force: the user's own endpoint, keys and model names when `model_override` is on, else the
/// system's (from the config) — never a mix, so the system's keys are never sent to a user's endpoint and the user's
/// keys never to the system's. The image and video endpoints and keys are optional on both sides: "" means the
/// model endpoint and its key (`imagegen::media_auth_headers`).
pub fn effective_model(s: &Value, cfg: &Config) -> Value {
    let mut out = s.as_object().cloned().unwrap_or_default();
    if get_truthy(s, "model_override") {
        for k in SECRET_KEYS {
            let key = s
                .get(k)
                .filter(|v| truthy(v))
                .map(py_str)
                .unwrap_or_default();
            out.insert(k.into(), Value::String(key));
        }
    } else {
        if let Value::Object(sys) = system_model(cfg) {
            for (k, v) in sys {
                out.insert(k, v);
            }
        }
        out.insert(
            "model_api_key".into(),
            Value::String(system_model_api_key(cfg)),
        );
        out.insert(
            "image_api_key".into(),
            Value::String(cfg.model.image_api_key.trim().to_string()),
        );
        out.insert(
            "video_api_key".into(),
            Value::String(cfg.model.video_api_key.trim().to_string()),
        );
        if let Value::Object(opts) = system_options(cfg) {
            out.extend(opts);
        }
    }
    Value::Object(out)
}

/// Settings as the API returns them: no key ever leaves the runtime, only whether one is saved (`<key>_set`).
pub fn redacted(s: &Value) -> Value {
    let mut out = Map::new();
    if let Some(o) = s.as_object() {
        for (k, v) in o {
            if !SECRET_KEYS.contains(&k.as_str()) {
                out.insert(k.clone(), v.clone());
            }
        }
    }
    for k in SECRET_KEYS {
        out.insert(format!("{}_set", k), Value::Bool(get_truthy(s, k)));
    }
    Value::Object(out)
}

/// The profile key a model means by a common other name of a field (`date_of_birth`, `dob` → `birthday`, `sex` →
/// `gender`, `email` → `email_personal` …); anything else is returned as given.
pub fn profile_alias(name: &str) -> String {
    let n = name.trim().to_lowercase().replace([' ', '-'], "_");
    match n.as_str() {
        "date_of_birth" | "dob" | "birth_date" | "birthdate" | "date_birth" => "birthday",
        "sex" => "gender",
        "email" | "email_address" | "personal_email" => "email_personal",
        "work_email" => "email_work",
        "mobile" | "mobile_phone" | "phone_number" | "mobile_number" | "tel" => "phone",
        "name" | "full_name" | "passport_name" | "legal_name" | "name_on_passport" => "name_en",
        "home_address" | "address" => "address_home",
        "work_address" | "office_address" => "address_work",
        "citizenship" | "country_of_citizenship" => "nationality",
        "place_of_birth" | "country_of_birth" | "birth_country" | "birth_place" => "birthplace",
        "employer" => "company",
        "occupation" | "title" | "position" => "job_title",
        _ => return name.trim().to_string(),
    }
    .to_string()
}

/// `PROFILE_FIELDS`: key, 中文, English.
pub const PROFILE_FIELDS: [(&str, &str, &str); 19] = [
    ("name_en", "英文姓名（证件拼写）", "Name as on passport"),
    // the key stays `name_zh` (profiles saved before keep their value); it holds any other name: Chinese, Japanese, a
    // married or former name
    ("name_zh", "其他姓名", "Other name"),
    ("preferred_name", "称呼", "Preferred name"),
    ("phone", "手机", "Mobile phone"),
    ("email_personal", "私人邮箱", "Personal email"),
    ("email_work", "工作邮箱", "Work email"),
    ("address_home", "家庭地址", "Home address"),
    ("address_work", "公司地址", "Work address"),
    // the places of the home address as codes (places.rs): code reads these, never the address's words
    ("home_city", "所在城市", "Home city"),
    ("home_country", "所在国家/地区", "Home country"),
    ("company", "公司", "Company"),
    ("job_title", "职位", "Job title"),
    ("birthday", "生日", "Date of birth"),
    // as on the passport (Male, Female or X): arrival cards and bookings ask for it; read from the ID photo
    ("gender", "性别", "Gender"),
    ("nationality", "国籍", "Nationality"),
    // a country name, as on the passport: arrival cards ask for it (SG, MDAC, Taiwan) and code never guesses it from the
    // nationality
    ("birthplace", "出生国家/地区", "Country of birth"),
    ("preferred_airline", "常用航空公司", "Preferred airline"),
    (
        "preferred_hotel_brand",
        "常用酒店品牌",
        "Preferred hotel brand",
    ),
    (
        "preferred_car_rental",
        "常用租车公司",
        "Preferred car rental",
    ),
];

/// The profile keys that are standing preferences the executor always sees (`Runtime::standing_prefs`): the
/// fixed ones, and any custom field the user marked `standing` on the Memory page ([`Store::profile_standing`]).
pub const PREFERENCE_KEYS: [&str; 3] = [
    "preferred_airline",
    "preferred_hotel_brand",
    "preferred_car_rental",
];

/// A fixed field, a further address written as `address:<label>` (a flat in another city, the parents' house), or a
/// custom one written as `custom:<label>`; "" when none of these.
pub fn profile_key(key: &str) -> String {
    static WS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s+").unwrap());
    let key = key.trim();
    if PROFILE_FIELDS.iter().any(|(k, _, _)| *k == key) {
        return key.to_string();
    }
    let n = key.chars().count();
    for prefix in ["custom:", "address:", "address_country:"] {
        if let Some(rest) = key.strip_prefix(prefix) {
            if n > prefix.len() && n <= 60 && !rest.trim().is_empty() {
                let label = WS.replace_all(rest, " ");
                return format!("{}{}", prefix, take_chars(label.trim(), 50));
            }
        }
    }
    String::new()
}

/// Lower-cased, every non-word character dropped (`re.sub(r"\W+", "", s.lower())`).
pub fn norm_fact(s: &str) -> String {
    static RX: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\W+").unwrap());
    RX.replace_all(&s.to_lowercase(), "").into_owned()
}

static SENSITIVE: LazyLock<FancyRegex> = LazyLock::new(|| {
    FancyRegex::new(r"(?<![+\d])(?:\d[ -]?){12,19}|\b[A-Z]{1,2}\d{6,9}\b|\b[STFGM]\d{7}[A-Z]\b|(?i:password|密码|cvv|cvc)")
        .unwrap()
});
static EMAIL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[^@\s]+@[^@\s]+\.[A-Za-z]{2,}$").unwrap());

/// Basic shape checks so a suggestion like `email_work = 'bytetrade'` never reaches the user.
pub fn profile_value_ok(key: &str, value: &str) -> bool {
    static NON_DIGIT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\D").unwrap());
    static PHONE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[+\d\s().-]+$").unwrap());
    let v = value.trim();
    if v.is_empty() {
        return false;
    }
    if key.starts_with("email") {
        return EMAIL.is_match(v);
    }
    if key == "gender" {
        return gender_of(v).is_some();
    }
    if key == "home_city" || key == "home_country" || key.starts_with("address_country:") {
        return crate::places::clean_value(key, v).is_some();
    }
    if key == "phone" {
        let digits = NON_DIGIT.replace_all(v, "");
        let n = digits.chars().count();
        return (6..=15).contains(&n) && PHONE.is_match(v);
    }
    true
}

/// The profile's gender as on the passport ("Male", "Female", "X") from the ways it is written ("M", "f", "男", "female").
pub fn gender_of(v: &str) -> Option<&'static str> {
    match v.trim().to_lowercase().as_str() {
        "m" | "male" | "man" | "男" | "男性" => Some("Male"),
        "f" | "female" | "woman" | "女" | "女性" => Some("Female"),
        "x" | "<" | "unspecified" | "non-binary" | "nonbinary" => Some("X"),
        _ => None,
    }
}

/// Card / ID / passport-looking numbers and passwords never go into memory — they belong in the vault.
pub fn looks_sensitive(text: &str) -> bool {
    SENSITIVE.is_match(text).unwrap_or(false)
}

// ------------------------------------------------------------------------------------------------
// Personal context: the structured fields a fact carries. They are set by the model when it writes memory
// (memory_remember, the extraction after a task, the sorting step in `memory_tidy::classify`) or by the user on the
// Memory page, and validated here against these enums. Code never reads a fact's prose to sort or select it.
// ------------------------------------------------------------------------------------------------

/// `DOMAINS`: key, 中文, English.
pub const DOMAINS: [(&str, &str, &str); 7] = [
    ("person", "人物与关系", "People"),
    ("preference", "偏好", "Preferences"),
    ("place", "地点", "Places"),
    ("account", "账户与会员", "Accounts & memberships"),
    ("site", "网站习惯", "Site habits"),
    ("rule", "默认规则", "Rules & defaults"),
    ("work", "工作与其他", "Work & other"),
];
/// `DOMAIN_KEYS`.
pub const DOMAIN_KEYS: [&str; 7] = [
    "person",
    "preference",
    "place",
    "account",
    "site",
    "rule",
    "work",
];

/// `DOMAIN_LABEL[k]` = "中文 English".
pub fn domain_label(key: &str) -> String {
    DOMAINS
        .iter()
        .find(|(k, _, _)| *k == key)
        .map(|(_, zh, en)| format!("{} {}", zh, en))
        .unwrap_or_default()
}

/// What a fact is about, as an enum the model fills (key, what it covers — the description goes into the model's
/// schema). A task's topics come from the model's reading of the request (`context::relevance`) and from structured
/// task data (the bot, the skill loaded, the workflow tool called).
pub const TOPICS: [(&str, &str); 17] = [
    ("shopping", "buying things, shops, delivery, checkout"),
    ("clothing", "clothes and shoes, sizes"),
    ("electronics", "phones, computers, gadgets"),
    ("gifts", "presents for other people"),
    ("dining", "restaurants, food, drinks, diet"),
    ("travel", "flights, hotels, trips, visas, car rental"),
    ("email", "mail, mailboxes, signatures, replies"),
    ("calendar", "meetings, appointments, time zone"),
    (
        "documents",
        "reports, briefs, files, formats, the language of deliverables",
    ),
    ("news", "news and subjects the user follows"),
    ("fitness", "sport, running, hiking"),
    ("health", "medical matters, allergies"),
    ("phone_calls", "calling people or businesses"),
    ("forms", "filling in forms, sign-ups, applications"),
    (
        "finance",
        "money, budgets, payments, cards, banks, insurance",
    ),
    ("home", "household, family logistics, home services"),
    ("work", "the user's job, company, projects, colleagues"),
];

/// The topic keys.
pub fn topic_keys() -> Vec<&'static str> {
    TOPICS.iter().map(|(k, _)| *k).collect()
}

/// When a fact applies: `always` (every task: how the assistant should behave in general), `topic` (tasks about
/// its topics or its entity), `trigger` (only when the user says its trigger phrase). `""` = not sorted yet (a fact
/// written before 0.6.0 or typed on the Memory page): `memory_tidy::classify` sorts it.
pub const SCOPES: [&str; 3] = ["always", "topic", "trigger"];

/// A fact's kind: `general`, or `id_document` (passport / ID card / birth date facts: only for tasks that fill in a
/// form or check a document — `memory_search` with `id_documents`). `site_block` is code's own (a site that blocks
/// the automated browser, `Runtime::learn_site_block`). `""` = not sorted yet.
pub const FACT_KINDS: [&str; 3] = ["general", "id_document", "site_block"];

/// The listed topics in a JSON value (an array of strings, or one string), each once, in the list's order of
/// appearance; anything not in [`TOPICS`] is dropped.
pub fn clean_topics(v: Option<&Value>) -> Vec<String> {
    let keys = topic_keys();
    let items: Vec<String> = match v {
        Some(Value::Array(a)) => a
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect(),
        Some(Value::String(s)) => vec![s.clone()],
        _ => vec![],
    };
    let mut out: Vec<String> = Vec::new();
    for t in items {
        let t = t.trim().to_lowercase();
        if keys.contains(&t.as_str()) && !out.contains(&t) {
            out.push(t);
        }
    }
    out
}

/// A fact row's topics (stored as a JSON array in a text column; already an array after [`decode_fact`]).
pub fn fact_topics(f: &Value) -> Vec<String> {
    match f.get("topics") {
        Some(Value::String(s)) => clean_topics(Some(&loads(Some(s), json!([])))),
        other => clean_topics(other),
    }
}

/// A fact row as the API and the agent see it: `topics` parsed into an array.
pub fn decode_fact(mut r: Value) -> Value {
    let t = fact_topics(&r);
    set(&mut r, "topics", json!(t));
    r
}

/// Whether a fact still waits to be sorted (no scope yet).
pub fn unsorted(f: &Value) -> bool {
    !SCOPES.contains(&get_str(f, "scope").as_str())
}

fn rx(p: &str) -> Regex {
    Regex::new(p).unwrap()
}

/// `context.render`: selected facts grouped by domain, for the prompt.
pub fn render_facts(selected: &[Value], en: bool) -> String {
    if selected.is_empty() {
        return "(nothing on file that bears on this request)".to_string();
    }
    let mut by: HashMap<String, Vec<String>> = HashMap::new();
    for f in selected {
        let mut mark = if get_str(f, "status") == "pending" {
            " (unconfirmed — learned in an earlier task)".to_string()
        } else {
            String::new()
        };
        let trigger = get_str(f, "trigger");
        if get_str(f, "scope") == "trigger" && !trigger.is_empty() {
            mark.push_str(&format!(" (the user said “{trigger}”)"));
        }
        let dom = {
            let d = get_str(f, "domain");
            if d.is_empty() {
                "work".to_string()
            } else {
                d
            }
        };
        by.entry(dom)
            .or_default()
            .push(format!("- {}{}", get_str(f, "fact"), mark));
    }
    let mut out = Vec::new();
    for k in DOMAIN_KEYS {
        if let Some(lines) = by.get(k) {
            let label = domain_label(k);
            let shown = if en {
                label
                    .split_once(' ')
                    .map(|x| x.1.to_string())
                    .unwrap_or(label)
            } else {
                label
            };
            out.push(format!("[{}]", shown));
            out.extend(lines.iter().cloned());
        }
    }
    out.join("\n")
}

/// `context.USE_RULE`.
pub const USE_RULE: &str = "Use these directly — do NOT ask the user again for anything listed here (sizes, cabin class, cuisine, which \
mailbox, delivery habits …); say in your answer which of them you applied. Ask only for what is really \
missing, and memory_search before asking about the user's own preferences. Apply a fact only when the request \
is about it: for a general question (weather, visas, what to pack, sights) do not assume who travels with \
the user, group size or a business purpose, and never repeat document numbers, birth dates, family \
members' full names or passport details in the answer unless the user asked for them. Bookings found in \
e-mails marked as test / fictional are not the user's real plans.";

// ------------------------------------------------------------------------------------------------

/// Optional arguments of [`Store::add_fact`] (the Python keyword defaults) and the fact's structured fields.
#[derive(Debug, Clone)]
pub struct FactOpts {
    pub source: String,
    pub confidence: f64,
    pub ttl_days: Option<i64>,
    pub tier: String,
    pub domain: String,
    pub status: String,
    /// [`FACT_KINDS`]; "" = not sorted yet
    pub kind: String,
    /// [`SCOPES`]; "" = not sorted yet (`memory_tidy::classify` sorts it)
    pub scope: String,
    /// [`TOPICS`] keys
    pub topics: Vec<String>,
    /// the phrase that triggers a `scope = trigger` fact
    pub trigger: String,
    /// the user's relation to the fact's entity (wife, boss, travel companion), kept on the entity
    pub relation: String,
    /// the task that wrote the fact: its episode is linked to the fact (forgetting the fact forgets the episode)
    pub task_id: String,
}

impl Default for FactOpts {
    fn default() -> Self {
        Self {
            source: "user".into(),
            confidence: 0.9,
            ttl_days: None,
            tier: "long".into(),
            domain: String::new(),
            status: "active".into(),
            kind: String::new(),
            scope: String::new(),
            topics: Vec::new(),
            trigger: String::new(),
            relation: String::new(),
            task_id: String::new(),
        }
    }
}

impl FactOpts {
    /// The structured fields of a fact the model wrote (a `memory_remember` call, an extracted item, a sorting
    /// answer), validated: `domain` in [`DOMAIN_KEYS`], `topics` in [`TOPICS`], `scope` in [`SCOPES`] (a trigger
    /// scope needs its `trigger` phrase, else it is a topic scope), `id_document` (bool) → kind, `relation` (≤ 40
    /// chars). A field the model left out or got wrong keeps what `self` had. Given any of scope / topics / kind,
    /// the fact counts as sorted (scope defaults to `topic`, kind to `general`).
    pub fn with_model_fields(mut self, v: &Value) -> Self {
        let d = get_str(v, "domain").trim().to_lowercase();
        if DOMAIN_KEYS.contains(&d.as_str()) {
            self.domain = d;
        }
        let topics = clean_topics(v.get("topics"));
        let scope = get_str(v, "scope").trim().to_lowercase();
        let trigger = take_chars(get_str(v, "trigger").trim(), 60).to_string();
        let id_doc = v.get("id_document").and_then(Value::as_bool);
        let sorted = !topics.is_empty() || SCOPES.contains(&scope.as_str()) || id_doc.is_some();
        if !topics.is_empty() {
            self.topics = topics;
        }
        if SCOPES.contains(&scope.as_str()) {
            self.scope = scope;
        }
        if self.scope == "trigger" {
            if trigger.is_empty() {
                self.scope = "topic".into();
            } else {
                self.trigger = trigger;
            }
        }
        if let Some(b) = id_doc {
            self.kind = if b { "id_document" } else { "general" }.into();
        }
        if sorted {
            if self.scope.is_empty() {
                self.scope = "topic".into();
            }
            if self.kind.is_empty() {
                self.kind = "general".into();
            }
        }
        let rel = take_chars(get_str(v, "relation").trim(), 40).to_string();
        if !rel.is_empty() {
            self.relation = rel;
        }
        self
    }
}

/// The JSON-schema properties of a fact's structured fields, for a tool or a prompt the model answers in JSON
/// (`memory_remember`, the extraction, the sorting step).
pub fn fact_field_schema() -> Value {
    let topic_desc = TOPICS
        .iter()
        .map(|(k, d)| format!("{k} ({d})"))
        .collect::<Vec<_>>()
        .join(", ");
    json!({
        "domain": {"type": "string", "enum": DOMAIN_KEYS,
            "description": "person (people and how they relate) | preference (likes, sizes, styles) | place (home / office / city) | account (memberships, subscriptions, which mailbox is for what — never numbers) | site (how a website works for the user) | rule (a standing instruction) | work (companies, projects, anything else)"},
        "topics": {"type": "array", "items": {"type": "string", "enum": topic_keys()},
            "description": format!("the kinds of tasks this fact matters for: {topic_desc}")},
        "scope": {"type": "string", "enum": SCOPES,
            "description": "always = applies to every task (how the assistant should behave in general, the language or format of every answer); topic = applies to tasks about its topics or its entity; trigger = only when the user says the trigger phrase"},
        "trigger": {"type": "string", "description": "scope=trigger only: the phrase the user says, as they wrote it (e.g. 准备出差, prepare for a trip)"},
        "id_document": {"type": "boolean", "description": "the fact is about the user's identity documents (passport, ID card, visa, date of birth): used only for forms and document checks"},
        "relation": {"type": "string", "description": "when the entity is a person: the user's relation to them in one or two words (wife, colleague, travel companion); else empty"},
    })
}

/// Recent facts and episodes are kept this many days (`RStore.RECENT_DAYS`).
pub const RECENT_DAYS: f64 = 30.0;
/// Facts in these domains name entities (`RStore.ENTITY_DOMAINS`).
pub const ENTITY_DOMAINS: [&str; 4] = ["person", "place", "account", "site"];

/// What [`Store::search_facts`] looks for: the model's query words, the topics and entities it named (enums /
/// names from the lists it was given), whether identity-document facts may come back, a tier, a limit.
#[derive(Debug, Clone, Default)]
pub struct MemoryQuery {
    pub query: String,
    pub topics: Vec<String>,
    pub entities: Vec<String>,
    pub include_id: bool,
    pub tier: Option<String>,
    pub limit: usize,
}

/// The runtime's SQLite store (`runtime.db` under the runtime data directory). `Send + Sync`; share it in an `Arc`.
pub struct Store {
    pub db: Db,
    pub cfg: Arc<Config>,
}

fn split_terms(query: &str) -> Vec<String> {
    static SEP: LazyLock<Regex> = LazyLock::new(|| rx(r"[\s,，。.!?？！;；:：]+"));
    SEP.split(query)
        .filter(|t| t.chars().count() >= 2)
        .take(12)
        .map(|t| t.to_string())
        .collect()
}

fn fts_query(terms: &[String]) -> String {
    terms
        .iter()
        .map(|t| format!("\"{}\"", t.replace('"', "")))
        .collect::<Vec<_>>()
        .join(" OR ")
}

fn opt_str(v: &Value) -> Option<&str> {
    match v {
        Value::String(s) => Some(s.as_str()),
        _ => None,
    }
}

/// `loads(row[key], default)` for a text column.
fn loads_col(row: &Value, key: &str, default: Value) -> Value {
    loads(row.get(key).and_then(opt_str), default)
}

fn set(row: &mut Value, key: &str, v: Value) {
    if let Some(o) = row.as_object_mut() {
        o.insert(key.to_string(), v);
    }
}

impl Store {
    /// Open (or create) `<data_dir>/runtime.db` and apply the schema and the column migrations.
    pub fn new(data_dir: &Path, cfg: Arc<Config>) -> Result<Store> {
        let db = Db::open(data_dir.join("runtime.db"))?;
        if db.script(SCHEMA).is_err() {
            // sqlite without trigram tokenizer: fall back to unicode61
            db.script(&SCHEMA.replace("tokenize='trigram'", "tokenize='unicode61'"))?;
        }
        let cols = |table: &str| -> Result<HashSet<String>> {
            Ok(db
                .all(&format!("PRAGMA table_info({})", table), params![])?
                .iter()
                .map(|r| get_str(r, "name"))
                .collect())
        };
        if !cols("schedules")?.contains("goal_id") {
            db.execute(
                "ALTER TABLE schedules ADD COLUMN goal_id TEXT DEFAULT ''",
                params![],
            )?;
        }
        if !cols("schedules")?.contains("task") {
            // a long task's schedule: the code tool's call as JSON `{"tool", "args"}`, run as it is (agent/longtask.rs)
            db.execute(
                "ALTER TABLE schedules ADD COLUMN task TEXT DEFAULT ''",
                params![],
            )?;
        }
        // the schedule offers made in a chat (agent/longtask.rs): one per task per conversation
        db.execute(
            "CREATE TABLE IF NOT EXISTS schedule_offers (conv_id TEXT, task_key TEXT, created_at REAL, \
PRIMARY KEY (conv_id, task_key))",
            params![],
        )?;
        if !cols("messages")?.contains("meta") {
            db.execute(
                "ALTER TABLE messages ADD COLUMN meta TEXT DEFAULT ''",
                params![],
            )?; // e.g. the user's attachments
        }
        let tcols = cols("tasks")?;
        if !tcols.contains("attachments") {
            db.execute(
                "ALTER TABLE tasks ADD COLUMN attachments TEXT DEFAULT ''",
                params![],
            )?;
        }
        if !tcols.contains("outcome") {
            // proof that a consequential task really finished (app/runtime/outcome.py)
            db.execute(
                "ALTER TABLE tasks ADD COLUMN outcome TEXT DEFAULT ''",
                params![],
            )?;
        }
        if !cols("goals")?.contains("ask") {
            // the user's own request a goal came from (crate::goals::set_ask)
            db.execute(
                "ALTER TABLE goals ADD COLUMN ask TEXT DEFAULT ''",
                params![],
            )?;
        }
        // bots (crate::bots): the bot a conversation belongs to, and the main-bot conversation that handed it over
        let ccols = cols("conversations")?;
        for (col, ddl) in [
            ("bot_id", "TEXT DEFAULT 'main'"),
            ("parent_conv", "TEXT DEFAULT ''"),
        ] {
            if !ccols.contains(col) {
                db.execute(
                    &format!("ALTER TABLE conversations ADD COLUMN {} {}", col, ddl),
                    params![],
                )?;
            }
        }
        // a bot merged into another (bots/<id>.md `aliases`): its conversations are the other bot's now
        for (old, new) in crate::bots::renamed() {
            db.execute(
                "UPDATE conversations SET bot_id=? WHERE bot_id=?",
                params![new, old],
            )?;
        }
        let fcols = cols("facts")?;
        for (col, ddl) in [
            ("tier", "TEXT DEFAULT 'long'"),
            ("uses", "INTEGER DEFAULT 0"),
            ("last_used", "REAL"),
            ("expires_at", "REAL"),
            ("history", "TEXT DEFAULT ''"),
            ("domain", "TEXT DEFAULT ''"),
            ("status", "TEXT DEFAULT 'active'"),
            // the structured fields the model sets when it writes a fact (0.6.0); '' = not sorted yet
            ("kind", "TEXT DEFAULT ''"),
            ("scope", "TEXT DEFAULT ''"),
            ("topics", "TEXT DEFAULT ''"),
            ("trigger", "TEXT DEFAULT ''"),
        ] {
            if !fcols.contains(col) {
                db.execute(
                    &format!("ALTER TABLE facts ADD COLUMN {} {}", col, ddl),
                    params![],
                )?;
            }
        }
        // the facts each episode's task used ('' = written before 0.6.0: unknown, forgotten with any fact)
        if !cols("episodes")?.contains("fact_ids") {
            db.execute(
                "ALTER TABLE episodes ADD COLUMN fact_ids TEXT DEFAULT ''",
                params![],
            )?;
        }
        // a custom profile field the agent always sees (the user ticks it on the Memory page)
        if !cols("profile")?.contains("standing") {
            db.execute(
                "ALTER TABLE profile ADD COLUMN standing INTEGER DEFAULT 0",
                params![],
            )?;
        }
        // what memory each task used: the topics and entities of its request (the model's JSON) and the fact ids
        db.execute(
            "CREATE TABLE IF NOT EXISTS task_memory (task_id TEXT PRIMARY KEY, topics TEXT, entities TEXT, \
             fact_ids TEXT, updated_at REAL)",
            params![],
        )?;
        Ok(Store { db, cfg })
    }

    /// [`default_settings`] for this store's config.
    pub fn default_settings(&self) -> Value {
        default_settings(&self.cfg)
    }

    /// [`effective_model`] for this store's config.
    pub fn effective_model(&self, s: &Value) -> Value {
        effective_model(s, &self.cfg)
    }

    /// [`system_model_api_key`] for this store's config.
    pub fn system_model_api_key(&self) -> String {
        system_model_api_key(&self.cfg)
    }

    // ------------------------------------------------------------ settings
    fn setting_rows(&self) -> Result<Vec<(String, Value)>> {
        Ok(self
            .db
            .all("SELECT key, value FROM settings", params![])?
            .into_iter()
            .map(|r| {
                let raw = get_str(&r, "value");
                let v = loads(Some(raw.as_str()), Value::String(raw.clone()));
                (get_str(&r, "key"), v)
            })
            .collect())
    }

    /// Defaults overlaid with what was saved — the `model_*` values as saved, whether or not they are in force.
    pub fn saved_settings(&self) -> Result<Value> {
        let mut s = self.default_settings();
        for (k, v) in self.setting_rows()? {
            set(&mut s, &k, v);
        }
        Ok(s)
    }

    /// The settings in force (`model_*` and `model_api_key` resolved by [`effective_model`]).
    pub fn settings(&self) -> Result<Value> {
        Ok(self.effective_model(&self.saved_settings()?))
    }

    /// The user's own model settings as saved (Settings → Models form), "" where never saved; the keys only as
    /// `api_key_set` / `image_api_key_set` / `video_api_key_set`.
    pub fn user_model(&self) -> Result<Value> {
        let rows: Map<String, Value> = self.setting_rows()?.into_iter().collect();
        let mut out = Map::new();
        for k in MODEL_KEYS {
            let v = rows
                .get(k)
                .filter(|v| truthy(v))
                .map(py_str)
                .unwrap_or_default();
            out.insert(k.to_string(), Value::String(v));
        }
        out.insert(
            "fast_mode".into(),
            Value::Bool(rows.get("fast_mode").is_some_and(truthy)),
        );
        out.insert(
            "reasoning_effort".into(),
            Value::String(
                rows.get("reasoning_effort")
                    .filter(|v| truthy(v))
                    .map(py_str)
                    .unwrap_or_default(),
            ),
        );
        out.insert(
            "api_key_set".into(),
            Value::Bool(rows.get("model_api_key").map(truthy).unwrap_or(false)),
        );
        for k in ["image_api_key", "video_api_key"] {
            out.insert(
                format!("{}_set", k),
                Value::Bool(rows.get(k).map(truthy).unwrap_or(false)),
            );
        }
        Ok(Value::Object(out))
    }

    /// Save the known keys of `d` and return the settings in force.
    pub fn set_settings(&self, d: &Value) -> Result<Value> {
        let defaults = self.default_settings();
        if let Some(o) = d.as_object() {
            for (k, v) in o {
                if defaults.get(k).is_some() {
                    self.db.execute(
                        "INSERT OR REPLACE INTO settings(key, value) VALUES (?,?)",
                        params![k.as_str(), pyjson::dumps(v)],
                    )?;
                }
            }
        }
        self.settings()
    }

    // ------------------------------------------------------------ conversations
    pub fn create_conv(&self, title: &str, kind: &str, cid: Option<&str>) -> Result<String> {
        let cid = cid
            .filter(|c| !c.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| new_id("conv"));
        self.db.execute(
            "INSERT OR IGNORE INTO conversations(id, title, kind, created_at, updated_at) VALUES (?,?,?,?,?)",
            params![cid.as_str(), take_chars(title, 80), kind, now_ts(), now_ts()],
        )?;
        Ok(cid)
    }

    pub fn convs(&self, limit: i64) -> Result<Vec<Value>> {
        self.db.all(
            "SELECT * FROM conversations ORDER BY updated_at DESC LIMIT ?",
            params![limit],
        )
    }

    /// Put a conversation in a bot (`crate::bots`); `parent_conv` is the main-bot conversation that handed it over.
    pub fn set_conv_bot(&self, cid: &str, bot_id: &str, parent_conv: &str) -> Result<()> {
        self.db.execute(
            "UPDATE conversations SET bot_id=?, parent_conv=? WHERE id=?",
            params![bot_id, parent_conv, cid],
        )?;
        Ok(())
    }

    /// The bot a conversation belongs to (`main` for an unknown one or one saved before bots existed).
    pub fn conv_bot(&self, cid: &str) -> String {
        self.conv(cid)
            .ok()
            .flatten()
            .map(|c| get_str(&c, "bot_id"))
            .filter(|b| !b.is_empty())
            .unwrap_or_else(|| crate::bots::MAIN.to_string())
    }

    pub fn conv(&self, cid: &str) -> Result<Option<Value>> {
        self.db
            .one("SELECT * FROM conversations WHERE id=?", params![cid])
    }

    // ------------------------------------------------------------ research library (app/runtime/library.py)
    pub fn subjects(&self) -> Result<Vec<Value>> {
        self.db
            .all("SELECT * FROM subjects ORDER BY updated_at DESC", params![])
    }

    pub fn subject(&self, sid: &str) -> Result<Option<Value>> {
        self.db
            .one("SELECT * FROM subjects WHERE id=?", params![sid])
    }

    pub fn subject_by_conv(&self, cid: &str) -> Result<Option<Value>> {
        if cid.is_empty() {
            return Ok(None);
        }
        self.db
            .one("SELECT * FROM subjects WHERE conv_id=?", params![cid])
    }

    /// Insert a subject record (`library.new_subject`), making its slug unique (`-2`, `-3`, …).
    pub fn create_subject(&self, rec: &Value) -> Result<Value> {
        let mut rec = rec.clone();
        let base = get_str(&rec, "slug");
        let mut n = 2;
        while self
            .db
            .one(
                "SELECT id FROM subjects WHERE slug=?",
                params![get_str(&rec, "slug")],
            )?
            .is_some()
        {
            set(&mut rec, "slug", Value::String(format!("{}-{}", base, n)));
            n += 1;
        }
        self.db.insert("subjects", &rec)?;
        self.subject(&get_str(&rec, "id"))?
            .ok_or_else(|| anyhow!("subject vanished"))
    }

    /// `update_subject(sid, title=…, brief=…, status=…, conv_id=…, last_task=…, runs=…, chars=…, error=…)`.
    pub fn update_subject(&self, sid: &str, kw: &Value) -> Result<()> {
        let mut data = Map::new();
        if let Some(o) = kw.as_object() {
            for (k, v) in o {
                if [
                    "title",
                    "brief",
                    "status",
                    "conv_id",
                    "last_task",
                    "runs",
                    "chars",
                    "error",
                ]
                .contains(&k.as_str())
                {
                    data.insert(k.clone(), v.clone());
                }
            }
        }
        data.insert("updated_at".into(), json!(now_ts()));
        self.db
            .update("subjects", "id", sid, &Value::Object(data))?;
        Ok(())
    }

    pub fn delete_subject(&self, sid: &str) -> Result<()> {
        self.db
            .execute("DELETE FROM library_fts WHERE subject_id=?", params![sid])?;
        self.db
            .execute("DELETE FROM subjects WHERE id=?", params![sid])?;
        Ok(())
    }

    /// Replace the indexed passages of a subject: `(heading, text)` pieces.
    pub fn library_replace(&self, sid: &str, pieces: &[(String, String)]) -> Result<()> {
        self.db
            .execute("DELETE FROM library_fts WHERE subject_id=?", params![sid])?;
        for (heading, text) in pieces {
            self.db.execute(
                "INSERT INTO library_fts(subject_id, heading, text) VALUES (?,?,?)",
                params![sid, heading, text],
            )?;
        }
        Ok(())
    }

    /// Report passages matching the query: FTS (trigram) first, a plain LIKE fallback; each hit names its subject.
    pub fn library_search(&self, query: &str, limit: i64) -> Result<Vec<Value>> {
        let terms = split_terms(query);
        if terms.is_empty() {
            return Ok(vec![]);
        }
        let subs: HashMap<String, Value> = self
            .subjects()?
            .into_iter()
            .map(|x| (get_str(&x, "id"), x))
            .collect();
        let q = fts_query(&terms);
        let mut rows = self
            .db
            .all(
                "SELECT subject_id, heading, text, bm25(library_fts) AS rank FROM library_fts WHERE library_fts MATCH ? \
                 ORDER BY rank LIMIT ?",
                params![q, limit],
            )
            .unwrap_or_default();
        if rows.is_empty() {
            let cond = terms
                .iter()
                .map(|_| "text LIKE ?")
                .collect::<Vec<_>>()
                .join(" OR ");
            let mut p: Vec<Param> = terms
                .iter()
                .map(|t| Param::Text(format!("%{}%", t)))
                .collect();
            p.push(Param::Int(limit));
            rows = self.db.all(
                &format!(
                    "SELECT subject_id, heading, text FROM library_fts WHERE {} LIMIT ?",
                    cond
                ),
                p,
            )?;
        }
        let mut out = Vec::new();
        for r in rows {
            if let Some(sub) = subs.get(&get_str(&r, "subject_id")) {
                out.push(json!({
                    "subject_id": sub["id"], "slug": sub["slug"], "title": sub["title"],
                    "heading": get_str(&r, "heading"), "text": r["text"],
                }));
            }
        }
        Ok(out)
    }

    pub fn set_conv_title(&self, cid: &str, title: &str) -> Result<()> {
        self.db.execute(
            "UPDATE conversations SET title=? WHERE id=?",
            params![take_chars(title, 80), cid],
        )?;
        Ok(())
    }

    /// Append a message; `meta` (e.g. the user's attachments) is stored as JSON. Returns the message id.
    pub fn add_msg(
        &self,
        cid: &str,
        role: &str,
        content: &str,
        task_id: &str,
        meta: Option<&Value>,
    ) -> Result<i64> {
        let meta_txt = match meta {
            Some(m) if truthy(m) => pyjson::dumps(m),
            _ => String::new(),
        };
        let mid = self.db.insert(
            "messages",
            &json!({"conv_id": cid, "role": role, "content": content, "task_id": task_id,
                    "created_at": now_ts(), "meta": meta_txt}),
        )?;
        self.db.execute(
            "UPDATE conversations SET updated_at=? WHERE id=?",
            params![now_ts(), cid],
        )?;
        Ok(mid)
    }

    /// The last `limit` messages of a conversation, oldest first; `meta` parsed (null when none).
    pub fn msgs(&self, cid: &str, limit: i64) -> Result<Vec<Value>> {
        let mut rows = self.db.all(
            "SELECT * FROM messages WHERE conv_id=? ORDER BY id DESC LIMIT ?",
            params![cid, limit],
        )?;
        for r in rows.iter_mut() {
            let meta = loads_col(r, "meta", Value::Null);
            set(r, "meta", meta);
        }
        rows.reverse();
        Ok(rows)
    }

    /// Every file the user attached in this conversation (oldest first).
    pub fn conv_attachments(&self, cid: &str) -> Result<Vec<Value>> {
        let mut out = Vec::new();
        for m in self.msgs(cid, 400)? {
            if let Some(meta) = m.get("meta").and_then(|v| v.as_object()) {
                if let Some(list) = meta.get("attachments").and_then(|v| v.as_array()) {
                    out.extend(list.iter().cloned());
                }
            }
        }
        Ok(out)
    }

    /// Deletes a chat and its messages only. A schedule asked in it, or posting into it, is untouched: a task is
    /// deleted on the Tasks page alone (the user, 2026-11-07), and `Scheduler::ensure_conv` makes its chat again.
    pub fn delete_conv(&self, cid: &str) -> Result<()> {
        self.db
            .execute("DELETE FROM messages WHERE conv_id=?", params![cid])?;
        self.db
            .execute("DELETE FROM conversations WHERE id=?", params![cid])?;
        Ok(())
    }

    // ------------------------------------------------------------ tasks
    /// Create a task in state `CREATED` and return its record (as [`Store::task`] gives it).
    pub fn create_task(
        &self,
        goal: &str,
        conv_id: &str,
        source: &str,
        schedule_id: &str,
        parent_id: &str,
        attachments: Option<&Value>,
    ) -> Result<Value> {
        let tid = new_id("task");
        let att = match attachments {
            Some(a) if truthy(a) => pyjson::dumps(a),
            _ => String::new(),
        };
        self.db.insert(
            "tasks",
            &json!({"attachments": att,
                "id": tid, "conv_id": conv_id, "goal": goal, "status": "CREATED", "plan": "{}", "transcript": "[]",
                "pending": "null", "result": "", "error": "", "source": source, "schedule_id": schedule_id,
                "parent_id": parent_id, "steps": 0, "waiting": "null", "created_at": now_ts(), "updated_at": now_ts(),
                "finished_at": null,
            }),
        )?;
        self.task(&tid)?.ok_or_else(|| anyhow!("task vanished"))
    }

    /// The latest task of conversation `cid` other than `exclude` (any parent: a bot's task too), with its transcript.
    pub fn prev_task(&self, cid: &str, exclude: &str) -> Result<Option<Value>> {
        let id = self
            .db
            .one(
                "SELECT id FROM tasks WHERE conv_id=? AND id<>? ORDER BY created_at DESC LIMIT 1",
                params![cid, exclude],
            )?
            .map(|r| get_str(&r, "id"));
        match id {
            Some(id) => self.task(&id),
            None => Ok(None),
        }
    }

    /// A task with `plan`, `transcript`, `pending`, `waiting`, `attachments`, `outcome` parsed.
    pub fn task(&self, tid: &str) -> Result<Option<Value>> {
        let Some(mut r) = self
            .db
            .one("SELECT * FROM tasks WHERE id=?", params![tid])?
        else {
            return Ok(None);
        };
        for (k, d) in [
            ("plan", json!({})),
            ("transcript", json!([])),
            ("pending", Value::Null),
            ("waiting", Value::Null),
        ] {
            let v = loads_col(&r, k, d);
            set(&mut r, k, v);
        }
        let att = loads_col(&r, "attachments", json!([]));
        set(
            &mut r,
            "attachments",
            if truthy(&att) { att } else { json!([]) },
        );
        let oc = loads_col(&r, "outcome", Value::Null);
        set(&mut r, "outcome", oc);
        Ok(Some(r))
    }

    /// `update_task(tid, status=…, plan=…, transcript=…, pending=…, waiting=…, outcome=…, result=…, error=…,
    /// steps=…, finished_at=…)`: JSON columns are serialised, the rest stored as given.
    pub fn update_task(&self, tid: &str, kw: &Value) -> Result<()> {
        let mut data = Map::new();
        if let Some(o) = kw.as_object() {
            for (k, v) in o {
                if ["plan", "transcript", "pending", "waiting", "outcome"].contains(&k.as_str()) {
                    data.insert(k.clone(), Value::String(pyjson::dumps(v)));
                } else {
                    data.insert(k.clone(), v.clone());
                }
            }
        }
        data.insert("updated_at".into(), json!(now_ts()));
        self.db.update("tasks", "id", tid, &Value::Object(data))?;
        Ok(())
    }

    /// Top-level tasks (no parent), newest first; `status` may be a comma list.
    pub fn tasks(
        &self,
        status: Option<&str>,
        limit: i64,
        conv_id: Option<&str>,
    ) -> Result<Vec<Value>> {
        let mut q = "SELECT id, conv_id, goal, status, plan, result, error, source, schedule_id, parent_id, steps, waiting, created_at, updated_at, finished_at, outcome FROM tasks WHERE parent_id=''".to_string();
        let mut p: Vec<Param> = Vec::new();
        if let Some(st) = status.filter(|s| !s.is_empty()) {
            let parts: Vec<&str> = st.split(',').collect();
            q += &format!(
                " AND status IN ({})",
                parts.iter().map(|_| "?").collect::<Vec<_>>().join(",")
            );
            p.extend(parts.iter().map(|s| Param::Text(s.to_string())));
        }
        if let Some(c) = conv_id.filter(|c| !c.is_empty()) {
            q += " AND conv_id=?";
            p.push(Param::Text(c.to_string()));
        }
        q += " ORDER BY created_at DESC LIMIT ?";
        p.push(Param::Int(limit));
        let mut rows = self.db.all(&q, p)?;
        for r in rows.iter_mut() {
            let plan = loads_col(r, "plan", json!({}));
            set(r, "plan", plan);
            let w = loads_col(r, "waiting", Value::Null);
            set(r, "waiting", w);
            let oc = loads_col(r, "outcome", Value::Null);
            set(r, "outcome", oc);
        }
        Ok(rows)
    }

    // ------------------------------------------------------------ events
    pub fn add_event(&self, task_id: &str, type_: &str, data: &Value) -> Result<Value> {
        let ts = now_ts();
        let eid = self.db.insert(
            "events",
            &json!({"task_id": task_id, "ts": ts, "type": type_, "data": pyjson::dumps(data)}),
        )?;
        Ok(json!({"id": eid, "task_id": task_id, "ts": ts, "type": type_, "data": data}))
    }

    pub fn events(&self, task_id: &str, after: i64) -> Result<Vec<Value>> {
        let mut rows = self.db.all(
            "SELECT * FROM events WHERE task_id=? AND id>? ORDER BY id",
            params![task_id, after],
        )?;
        for r in rows.iter_mut() {
            let d = loads_col(r, "data", json!({}));
            set(r, "data", d);
        }
        Ok(rows)
    }

    // ------------------------------------------------------------ memory
    // Three layers: the profile (fixed fields for forms and emails; changes need the user's OK), long-term facts
    // (preferences, people, companies, projects, habits) and recent facts/episodes (one-off details, kept 30 days).

    /// Remember a fact; a restatement of a known fact refreshes it (`{"duplicate": true}`), a new one is indexed
    /// and its entity synced. `None` for an empty fact. The domain is the one given (`o.domain`), else the
    /// category when that is a domain key ("company" / "project" → work), else empty until the fact is sorted.
    pub fn add_fact(
        &self,
        fact: &str,
        category: &str,
        entity: &str,
        o: &FactOpts,
    ) -> Result<Option<Value>> {
        let fact = fact.trim();
        if fact.is_empty() {
            return Ok(None);
        }
        let tier = if o.tier == "recent" { "recent" } else { "long" };
        let norm = norm_fact(fact);
        for r in self
            .db
            .all("SELECT id, fact, tier, scope FROM facts", params![])?
        {
            if norm_fact(&get_str(&r, "fact")) == norm {
                let fid = get_str(&r, "id");
                let mut upd = Map::new();
                upd.insert("last_verified".into(), json!(now_ts()));
                if o.status == "active" {
                    // the user said it again: no longer just something OMuse learned
                    upd.insert("status".into(), json!("active"));
                }
                if tier == "long" && get_str(&r, "tier") == "recent" {
                    // said again / asked to remember: keep it for good
                    upd.insert("tier".into(), json!("long"));
                    upd.insert("expires_at".into(), Value::Null);
                }
                if unsorted(&r) && SCOPES.contains(&o.scope.as_str()) {
                    // restated with its fields: sorted now
                    upd.extend(Self::field_cols(o));
                }
                self.db.update("facts", "id", &fid, &Value::Object(upd))?;
                self.note_task_memory(&o.task_id, &[], &[], std::slice::from_ref(&fid))?;
                return Ok(Some(
                    json!({"id": r["id"], "fact": r["fact"], "duplicate": true}),
                ));
            }
        }
        let fid = new_id("fact");
        let exp = if tier == "recent" {
            json!(now_ts() + RECENT_DAYS * 86400.0)
        } else {
            Value::Null
        };
        let cat = category.trim().to_lowercase();
        // a relation names the user's relation to the person the fact is about, so the fact is about a person — whatever
        // domain the model chose (memory_real 2026-11-08: "my wife Mei collects jazz vinyl" filed as a preference, so
        // People and the Entities card had no Mei)
        let domain: &str = if !o.relation.trim().is_empty() && !entity.trim().is_empty() {
            "person"
        } else if DOMAIN_KEYS.contains(&o.domain.as_str()) {
            o.domain.as_str()
        } else if DOMAIN_KEYS.contains(&cat.as_str()) {
            cat.as_str()
        } else if cat == "company" || cat == "project" {
            "work"
        } else {
            ""
        };
        let status = if o.status == "pending" {
            "pending"
        } else {
            "active"
        };
        let mut row = json!({"id": fid, "fact": fact, "category": category, "entity": entity, "source": o.source,
                "confidence": o.confidence, "created_at": now_ts(), "last_verified": now_ts(),
                "ttl_days": o.ttl_days, "tier": tier, "uses": 0, "expires_at": exp, "history": "",
                "domain": domain, "status": status});
        if let Some(m) = row.as_object_mut() {
            m.extend(Self::field_cols(o));
        }
        self.db.insert("facts", &row)?;
        self.db.execute(
            "INSERT INTO facts_fts(id, fact, entity) VALUES (?,?,?)",
            params![fid.as_str(), fact, entity],
        )?;
        self.sync_entity(domain, entity)?;
        if !o.relation.is_empty() {
            self.set_relation_if_unset(entity, &o.relation)?;
        }
        self.note_task_memory(&o.task_id, &[], &[], std::slice::from_ref(&fid))?;
        Ok(Some(
            json!({"id": fid, "fact": fact, "tier": tier, "domain": domain, "status": status,
                   "scope": row["scope"], "topics": o.topics}),
        ))
    }

    /// The structured-field columns of a new or newly sorted fact.
    fn field_cols(o: &FactOpts) -> Map<String, Value> {
        let mut m = Map::new();
        let kind = if FACT_KINDS.contains(&o.kind.as_str()) {
            o.kind.as_str()
        } else {
            ""
        };
        let scope = if SCOPES.contains(&o.scope.as_str()) {
            o.scope.as_str()
        } else {
            ""
        };
        let topics = clean_topics(Some(&json!(o.topics)));
        m.insert("kind".into(), json!(kind));
        m.insert("scope".into(), json!(scope));
        m.insert(
            "topics".into(),
            json!(if topics.is_empty() {
                String::new()
            } else {
                pyjson::dumps(&json!(topics))
            }),
        );
        m.insert(
            "trigger".into(),
            json!(if scope == "trigger" {
                o.trigger.as_str()
            } else {
                ""
            }),
        );
        m
    }

    /// The user's relation to a person entity the model named with a fact ("wife"), unless one is set already (the
    /// user's own edit on the Memory page wins).
    pub fn set_relation_if_unset(&self, name: &str, rel: &str) -> Result<()> {
        let Some(e) = self.db.one(
            "SELECT id, type FROM entities WHERE lower(name)=lower(?)",
            params![name.trim()],
        )?
        else {
            return Ok(());
        };
        let eid = get_str(&e, "id");
        if get_str(&e, "type") != "person"
            || self
                .db
                .one(
                    "SELECT rel FROM relations WHERE src='me' AND dst=?",
                    params![eid.as_str()],
                )?
                .is_some()
        {
            return Ok(());
        }
        self.db.insert(
            "relations",
            &json!({"src": "me", "rel": take_chars(rel.trim(), 40), "dst": eid, "source": "model",
                    "created_at": now_ts()}),
        )?;
        Ok(())
    }

    /// Record what memory a task used: the topics and entities of its request and the facts it was given, found or
    /// wrote (merged with what is recorded). An episode of that task carries the fact ids, so forgetting a fact
    /// forgets the episodes that used it. A blank `task_id` records nothing.
    pub fn note_task_memory(
        &self,
        task_id: &str,
        topics: &[String],
        entities: &[String],
        fact_ids: &[String],
    ) -> Result<()> {
        if task_id.trim().is_empty() {
            return Ok(());
        }
        let cur = self.task_memory(task_id)?;
        let merge = |key: &str, add: &[String]| -> Vec<String> {
            let mut v: Vec<String> = cur
                .get(key)
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(Value::as_str)
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default();
            for x in add {
                if !x.is_empty() && !v.contains(x) {
                    v.push(x.clone());
                }
            }
            v
        };
        let (t, e, f) = (
            merge("topics", topics),
            merge("entities", entities),
            merge("fact_ids", fact_ids),
        );
        self.db.execute(
            "INSERT OR REPLACE INTO task_memory(task_id, topics, entities, fact_ids, updated_at) VALUES (?,?,?,?,?)",
            params![
                task_id,
                pyjson::dumps(&json!(t)),
                pyjson::dumps(&json!(e)),
                pyjson::dumps(&json!(f)),
                now_ts()
            ],
        )?;
        // an episode already written for the task (the extraction runs after it) gets the new ids too
        if !fact_ids.is_empty() {
            self.db.execute(
                "UPDATE episodes SET fact_ids=? WHERE task_id=?",
                params![pyjson::dumps(&json!(f)), task_id],
            )?;
        }
        Ok(())
    }

    /// `{"topics": [...], "entities": [...], "fact_ids": [...]}` recorded for a task (empty lists when none).
    pub fn task_memory(&self, task_id: &str) -> Result<Value> {
        let row = self.db.one(
            "SELECT topics, entities, fact_ids FROM task_memory WHERE task_id=?",
            params![task_id],
        )?;
        let col = |k: &str| {
            row.as_ref()
                .map(|r| loads_col(r, k, json!([])))
                .filter(Value::is_array)
                .unwrap_or_else(|| json!([]))
        };
        Ok(
            json!({"topics": col("topics"), "entities": col("entities"), "fact_ids": col("fact_ids")}),
        )
    }

    pub fn fact(&self, fid: &str) -> Result<Option<Value>> {
        Ok(self
            .db
            .one("SELECT * FROM facts WHERE id=?", params![fid])?
            .map(decode_fact))
    }

    /// `update_fact(fid, fact=…, category=…, entity=…, tier=…, uses=…, last_used=…, expires_at=…, history=…,
    /// last_verified=…, confidence=…, domain=…, status=…, kind=…, scope=…, topics=[…], trigger=…)`; returns the
    /// updated row, `None` when unknown. Enum fields outside their list are dropped.
    pub fn update_fact(&self, fid: &str, kw: &Value) -> Result<Option<Value>> {
        let Some(r) = self.fact(fid)? else {
            return Ok(None);
        };
        let mut data = Map::new();
        if let Some(o) = kw.as_object() {
            for (k, v) in o {
                if [
                    "fact",
                    "category",
                    "entity",
                    "tier",
                    "uses",
                    "last_used",
                    "expires_at",
                    "history",
                    "last_verified",
                    "confidence",
                    "domain",
                    "status",
                    "kind",
                    "scope",
                    "trigger",
                ]
                .contains(&k.as_str())
                {
                    data.insert(k.clone(), v.clone());
                }
            }
            if let Some(t) = o.get("topics") {
                let t = clean_topics(Some(t));
                data.insert(
                    "topics".into(),
                    json!(if t.is_empty() {
                        String::new()
                    } else {
                        pyjson::dumps(&json!(t))
                    }),
                );
            }
        }
        let in_list = |data: &mut Map<String, Value>, key: &str, list: &[&str]| {
            if let Some(d) = data.get(key) {
                if !d.is_null() && !d.as_str().is_some_and(|s| list.contains(&s)) {
                    data.remove(key);
                }
            }
        };
        in_list(&mut data, "domain", &DOMAIN_KEYS);
        in_list(&mut data, "status", &["active", "pending"]);
        in_list(&mut data, "kind", &FACT_KINDS);
        in_list(&mut data, "scope", &SCOPES);
        let r_fact = get_str(&r, "fact");
        let r_ent = get_str(&r, "entity");
        let tier = data
            .get("tier")
            .and_then(|v| v.as_str())
            .map(str::to_string);
        if tier.as_deref() == Some("long") {
            data.entry("expires_at").or_insert(Value::Null);
        } else if tier.as_deref() == Some("recent") && get_str(&r, "tier") != "recent" {
            data.entry("expires_at")
                .or_insert_with(|| json!(now_ts() + RECENT_DAYS * 86400.0));
        }
        if let Some(nf) = data.get("fact").map(py_str) {
            if nf != r_fact {
                let hist = format!("{}\n{}", get_str(&r, "history"), r_fact);
                let hist = hist.trim();
                let hist = omuse_common::util::last_chars(hist, 2000).to_string();
                data.insert("history".into(), Value::String(hist));
                let ent = data
                    .get("entity")
                    .map(py_str)
                    .unwrap_or_else(|| r_ent.clone());
                self.db
                    .execute("DELETE FROM facts_fts WHERE id=?", params![fid])?;
                self.db.execute(
                    "INSERT INTO facts_fts(id, fact, entity) VALUES (?,?,?)",
                    params![fid, nf, ent],
                )?;
            }
        }
        if !data.is_empty() {
            let dom = data
                .get("domain")
                .map(py_str)
                .unwrap_or_else(|| get_str(&r, "domain"));
            let ent = data
                .get("entity")
                .map(py_str)
                .unwrap_or_else(|| r_ent.clone());
            let sync = data.contains_key("domain") || data.contains_key("entity");
            self.db.update("facts", "id", fid, &Value::Object(data))?;
            if sync {
                self.sync_entity(&dom, &ent)?;
            }
        }
        self.fact(fid)
    }

    // ------------------------------------------------------------ personal context (domains + entities)
    /// People, places, accounts and sites named by facts become entities (aliases + relation live there).
    pub fn sync_entity(&self, domain: &str, name: &str) -> Result<()> {
        let name = name.trim();
        let n = name.chars().count();
        if !ENTITY_DOMAINS.contains(&domain) || !(2..=60).contains(&n) || name.contains('@') {
            return Ok(());
        }
        if self
            .db
            .one(
                "SELECT id FROM entities WHERE lower(name)=lower(?)",
                params![name],
            )?
            .is_none()
        {
            self.db.insert(
                "entities",
                &json!({"id": new_id("ent"), "type": domain, "name": name, "attrs": pyjson::dumps(&json!({"aliases": []})),
                        "created_at": now_ts()}),
            )?;
        }
        Ok(())
    }

    /// Entities (optionally of one type) with `attrs` parsed and the user's `relation` to each.
    pub fn entities(&self, kind: Option<&str>) -> Result<Vec<Value>> {
        let kind = kind.filter(|k| !k.is_empty());
        let mut rows = match kind {
            Some(k) => self.db.all(
                "SELECT * FROM entities WHERE type=? ORDER BY type, name",
                params![k],
            )?,
            None => self
                .db
                .all("SELECT * FROM entities ORDER BY type, name", params![])?,
        };
        let rel: HashMap<String, String> = self
            .db
            .all("SELECT dst, rel FROM relations WHERE src='me'", params![])?
            .iter()
            .map(|r| (get_str(r, "dst"), get_str(r, "rel")))
            .collect();
        for r in rows.iter_mut() {
            let attrs = loads_col(r, "attrs", json!({}));
            set(r, "attrs", if truthy(&attrs) { attrs } else { json!({}) });
            let relation = rel.get(&get_str(r, "id")).cloned().unwrap_or_default();
            set(r, "relation", Value::String(relation));
        }
        Ok(rows)
    }

    /// Set an entity's aliases (≤12, each ≤40 chars), the user's relation to it and/or its type.
    pub fn update_entity(
        &self,
        eid: &str,
        aliases: Option<&[String]>,
        relation: Option<&str>,
        kind: Option<&str>,
    ) -> Result<Option<Value>> {
        let Some(e) = self
            .db
            .one("SELECT * FROM entities WHERE id=?", params![eid])?
        else {
            return Ok(None);
        };
        let attrs = loads_col(&e, "attrs", json!({}));
        let mut attrs = if truthy(&attrs) { attrs } else { json!({}) };
        if let Some(al) = aliases {
            let list: Vec<Value> = al
                .iter()
                .filter(|a| !a.trim().is_empty())
                .map(|a| Value::String(take_chars(a.trim(), 40).to_string()))
                .take(12)
                .collect();
            set(&mut attrs, "aliases", Value::Array(list));
        }
        let mut upd = Map::new();
        upd.insert("attrs".into(), Value::String(pyjson::dumps(&attrs)));
        if let Some(k) = kind.filter(|k| ENTITY_DOMAINS.contains(k)) {
            upd.insert("type".into(), json!(k));
        }
        self.db.update("entities", "id", eid, &Value::Object(upd))?;
        if let Some(rel) = relation {
            self.db.execute(
                "DELETE FROM relations WHERE src='me' AND dst=?",
                params![eid],
            )?;
            if !rel.trim().is_empty() {
                self.db.insert(
                    "relations",
                    &json!({"src": "me", "rel": take_chars(rel.trim(), 40), "dst": eid, "source": "user",
                            "created_at": now_ts()}),
                )?;
            }
        }
        Ok(self
            .entities(None)?
            .into_iter()
            .find(|x| get_str(x, "id") == eid))
    }

    pub fn delete_entity(&self, eid: &str) -> Result<()> {
        self.db
            .execute("DELETE FROM entities WHERE id=?", params![eid])?;
        self.db.execute(
            "DELETE FROM relations WHERE dst=? OR src=?",
            params![eid, eid],
        )?;
        Ok(())
    }

    /// Delete a fact and the episodes whose task used it (by the fact ids each episode carries — 2026-10-04 M4-21:
    /// after "forget my taxi budget" the agent still answered "300" from a past-task summary).
    pub fn delete_fact(&self, fid: &str) -> Result<()> {
        self.db
            .execute("DELETE FROM facts WHERE id=?", params![fid])?;
        self.db
            .execute("DELETE FROM facts_fts WHERE id=?", params![fid])?;
        for e in self.episodes_using(fid)? {
            self.db
                .execute("DELETE FROM episodes WHERE id=?", params![e])?;
        }
        Ok(())
    }

    /// The user (or the agent, on the user's word) forgets a fact: [`Store::delete_fact`], and the episodes written
    /// before 0.6.0 too — they carry no fact ids, so whether they quote the fact is unknown (they expire after
    /// [`RECENT_DAYS`] anyway).
    pub fn forget_fact(&self, fid: &str) -> Result<()> {
        self.delete_fact(fid)?;
        self.db.execute(
            "DELETE FROM episodes WHERE COALESCE(fact_ids,'')=''",
            params![],
        )?;
        Ok(())
    }

    /// A fact merged into another (memory tidy): the episodes that used it now point to the one kept.
    pub fn relink_fact(&self, from: &str, to: &str) -> Result<()> {
        for eid in self.episodes_using(from)? {
            let row = self
                .db
                .one("SELECT fact_ids FROM episodes WHERE id=?", params![eid])?;
            let mut ids: Vec<String> = row
                .map(|r| loads_col(&r, "fact_ids", json!([])))
                .and_then(|v| serde_json::from_value(v).ok())
                .unwrap_or_default();
            ids.retain(|x| x != from);
            if !ids.iter().any(|x| x == to) {
                ids.push(to.to_string());
            }
            self.db.execute(
                "UPDATE episodes SET fact_ids=? WHERE id=?",
                params![pyjson::dumps(&json!(ids)), eid],
            )?;
        }
        Ok(())
    }

    /// The ids of the episodes whose `fact_ids` hold `fid`.
    fn episodes_using(&self, fid: &str) -> Result<Vec<i64>> {
        let rows = self.db.all(
            "SELECT id, fact_ids FROM episodes WHERE fact_ids LIKE ?",
            params![format!("%\"{}\"%", fid.replace('%', ""))],
        )?;
        Ok(rows
            .iter()
            .filter(|r| {
                loads_col(r, "fact_ids", json!([]))
                    .as_array()
                    .is_some_and(|a| a.iter().any(|x| x.as_str() == Some(fid)))
            })
            .filter_map(|r| r.get("id").and_then(Value::as_i64))
            .collect())
    }

    fn live(r: &Value) -> bool {
        let now = now_ts();
        if let Some(exp) = r
            .get("expires_at")
            .filter(|v| truthy(v))
            .and_then(Value::as_f64)
        {
            if exp < now {
                return false;
            }
        }
        match (
            r.get("ttl_days")
                .filter(|v| truthy(v))
                .and_then(Value::as_f64),
            r.get("created_at").and_then(Value::as_f64),
        ) {
            (Some(ttl), Some(created)) => created + ttl * 86400.0 >= now,
            _ => true,
        }
    }

    /// Live facts, newest first; `tier` = `Some("long")` (the default in Python), `Some("recent")` or `None` for all.
    pub fn facts(&self, limit: i64, tier: Option<&str>) -> Result<Vec<Value>> {
        let rows = match tier.filter(|t| !t.is_empty()) {
            Some(t) => self.db.all(
                "SELECT * FROM facts WHERE COALESCE(tier,'long')=? ORDER BY created_at DESC LIMIT ?",
                params![t, limit],
            )?,
            None => self.db.all("SELECT * FROM facts ORDER BY created_at DESC LIMIT ?", params![limit])?,
        };
        Ok(rows
            .into_iter()
            .filter(Self::live)
            .map(decode_fact)
            .collect())
    }

    /// Live long-term facts that are not sorted yet (no scope), the most used first.
    pub fn unsorted_facts(&self, limit: usize) -> Result<Vec<Value>> {
        let mut v: Vec<Value> = self
            .facts(5000, None)?
            .into_iter()
            .filter(unsorted)
            .collect();
        v.sort_by_key(|f| std::cmp::Reverse(f.get("uses").and_then(Value::as_i64).unwrap_or(0)));
        v.truncate(limit);
        Ok(v)
    }

    /// Facts for a [`MemoryQuery`]: the ones whose topics or entity the query names (structured fields the model
    /// filled in on both sides), then the full-text hits of the model's query words (FTS, a LIKE fallback) — a
    /// search engine over the stored text, nothing decided by it. Identity-document facts and facts not sorted yet
    /// only with `include_id`.
    pub fn search_facts(&self, q: &MemoryQuery) -> Result<Vec<Value>> {
        let tier = q.tier.as_deref().filter(|t| !t.is_empty());
        let limit = q.limit.max(1);
        let keep = |r: &Value| -> bool {
            let rt = {
                let t = get_str(r, "tier");
                if t.is_empty() {
                    "long".to_string()
                } else {
                    t
                }
            };
            Self::live(r)
                && tier.is_none_or(|t| rt == t)
                && (q.include_id || (get_str(r, "kind") != "id_document" && !unsorted(r)))
        };
        let mut rows: Vec<Value> = Vec::new();
        let mut have: HashSet<String> = HashSet::new();
        if !q.topics.is_empty() || !q.entities.is_empty() {
            let ents: Vec<String> = q.entities.iter().map(|e| e.trim().to_lowercase()).collect();
            let mut scored: Vec<(usize, Value)> = Vec::new();
            for r in self.facts(5000, tier)? {
                if !keep(&r) {
                    continue;
                }
                let ent = get_str(&r, "entity").trim().to_lowercase();
                let by_ent = !ent.is_empty() && ents.contains(&ent);
                let shared = fact_topics(&r)
                    .iter()
                    .filter(|t| q.topics.contains(t))
                    .count();
                if by_ent || shared > 0 {
                    scored.push((if by_ent { 10 } else { 0 } + shared, r));
                }
            }
            scored.sort_by_key(|x| std::cmp::Reverse(x.0));
            for (_, r) in scored {
                if have.insert(get_str(&r, "id")) {
                    rows.push(r);
                }
            }
        }
        let terms = split_terms(&q.query);
        if !terms.is_empty() && rows.len() < limit {
            let fq = fts_query(&terms);
            let mut ids: Vec<String> = self
                .db
                .all(
                    "SELECT id FROM facts_fts WHERE facts_fts MATCH ? LIMIT ?",
                    params![fq, (limit * 3) as i64],
                )
                .unwrap_or_default()
                .iter()
                .map(|r| get_str(r, "id"))
                .collect();
            if ids.is_empty() {
                let cond = terms
                    .iter()
                    .map(|_| "fact LIKE ?")
                    .collect::<Vec<_>>()
                    .join(" OR ");
                let mut p: Vec<Param> = terms
                    .iter()
                    .map(|t| Param::Text(format!("%{}%", t)))
                    .collect();
                p.push(Param::Int((limit * 3) as i64));
                ids = self
                    .db
                    .all(&format!("SELECT id FROM facts WHERE {} LIMIT ?", cond), p)?
                    .iter()
                    .map(|r| get_str(r, "id"))
                    .collect();
            }
            for i in ids {
                if have.contains(&i) {
                    continue;
                }
                if let Some(r) = self.fact(&i)? {
                    if keep(&r) {
                        have.insert(i);
                        rows.push(r);
                    }
                }
            }
        }
        rows.truncate(limit);
        Ok(rows)
    }

    /// The live facts hidden from a search without `include_id`: identity-document facts and facts not sorted yet.
    pub fn hidden_fact_count(&self) -> Result<(usize, usize)> {
        let fs = self.facts(5000, None)?;
        Ok((
            fs.iter()
                .filter(|f| get_str(f, "kind") == "id_document")
                .count(),
            fs.iter().filter(|f| unsorted(f)).count(),
        ))
    }

    pub fn mark_used(&self, ids: &[String]) -> Result<()> {
        for fid in ids {
            self.db.execute(
                "UPDATE facts SET uses=COALESCE(uses,0)+1, last_used=? WHERE id=?",
                params![now_ts(), fid.as_str()],
            )?;
        }
        Ok(())
    }

    /// Drop expired recent facts and episodes older than [`RECENT_DAYS`]: `{"facts": n, "episodes": n}`.
    pub fn prune_memory(&self) -> Result<Value> {
        let gone: Vec<String> = self
            .db
            .all(
                "SELECT id FROM facts WHERE expires_at IS NOT NULL AND expires_at < ?",
                params![now_ts()],
            )?
            .iter()
            .map(|r| get_str(r, "id"))
            .collect();
        for fid in &gone {
            self.delete_fact(fid)?;
        }
        let cut = now_ts() - RECENT_DAYS * 86400.0;
        let n_ep = self.db.scalar(
            "SELECT COUNT(*) AS n FROM episodes WHERE ts < ?",
            params![cut],
        )?;
        self.db
            .execute("DELETE FROM episodes WHERE ts < ?", params![cut])?;
        Ok(json!({"facts": gone.len(), "episodes": n_ep}))
    }

    /// A past task's summary, with the ids of the facts the task used ([`Store::note_task_memory`]; `[]` when none).
    pub fn add_episode(&self, task_id: &str, summary: &str) -> Result<()> {
        let ids = self.task_memory(task_id)?["fact_ids"].clone();
        self.db.insert(
            "episodes",
            &json!({"ts": now_ts(), "task_id": task_id, "summary": take_chars(summary, 1000),
                    "fact_ids": pyjson::dumps(&ids)}),
        )?;
        Ok(())
    }

    /// Recent episodes whose task used one of `fact_ids` or had one of `topics` / `entities` (structured links), then
    /// the ones whose summary holds one of the model's query words (a plain text search), at most `limit`.
    pub fn episodes_for(
        &self,
        fact_ids: &[String],
        topics: &[String],
        entities: &[String],
        query: &str,
        limit: usize,
    ) -> Result<Vec<Value>> {
        let words: Vec<&str> = query
            .split_whitespace()
            .filter(|w| w.chars().count() > 1)
            .collect();
        let ents: Vec<String> = entities.iter().map(|e| e.to_lowercase()).collect();
        let mut linked = Vec::new();
        let mut by_word = Vec::new();
        for e in self.episodes(200)? {
            let ids = loads_col(&e, "fact_ids", json!([]));
            let tm = self.task_memory(&get_str(&e, "task_id"))?;
            let any_in = |v: &Value, list: &[String], low: bool| {
                v.as_array().is_some_and(|a| {
                    a.iter().filter_map(Value::as_str).any(|x| {
                        if low {
                            list.contains(&x.to_lowercase())
                        } else {
                            list.iter().any(|y| y == x)
                        }
                    })
                })
            };
            if any_in(&ids, fact_ids, false)
                || any_in(&tm["topics"], topics, false)
                || any_in(&tm["entities"], &ents, true)
            {
                linked.push(e);
            } else {
                let s = get_str(&e, "summary");
                if words.iter().any(|w| s.contains(w)) {
                    by_word.push(e);
                }
            }
        }
        linked.extend(by_word);
        linked.truncate(limit);
        Ok(linked)
    }

    pub fn episodes(&self, limit: i64) -> Result<Vec<Value>> {
        let cut = now_ts() - RECENT_DAYS * 86400.0;
        self.db.all(
            "SELECT * FROM episodes WHERE ts >= ? ORDER BY id DESC LIMIT ?",
            params![cut, limit],
        )
    }

    // ------------------------------------------------------------ profile (fixed fields; every change needs the user)
    /// `{key: value}` of the non-blank profile fields.
    pub fn profile(&self) -> Result<Value> {
        let mut out = Map::new();
        for r in self.db.all("SELECT key, value FROM profile", params![])? {
            let v = get_str(&r, "value");
            if !v.trim().is_empty() {
                out.insert(get_str(&r, "key"), Value::String(v));
            }
        }
        Ok(Value::Object(out))
    }

    /// The custom profile fields (`custom:<label>`) the user marked as standing preferences: the executor always sees
    /// them (`Runtime::standing_prefs`), like the fixed [`PREFERENCE_KEYS`].
    pub fn profile_standing(&self) -> Result<Vec<String>> {
        Ok(self
            .db
            .all(
                "SELECT key FROM profile WHERE COALESCE(standing,0)=1 AND key LIKE 'custom:%' ORDER BY key",
                params![],
            )?
            .iter()
            .map(|r| get_str(r, "key"))
            .collect())
    }

    /// Mark a saved custom field as a standing preference (or not). Errors for anything but a saved `custom:` field.
    pub fn set_profile_standing(&self, key: &str, standing: bool) -> Result<()> {
        let key = profile_key(key);
        if !key.starts_with("custom:") {
            return Err(anyhow!("only a custom field can be a standing preference"));
        }
        let n = self.db.execute(
            "UPDATE profile SET standing=? WHERE key=?",
            params![i64::from(standing), key.as_str()],
        )?;
        if n == 0 {
            return Err(anyhow!("unknown profile field"));
        }
        Ok(())
    }

    /// Set (or, with a blank value, clear) a profile field. Errors with `unknown profile field` (Python
    /// `ValueError`) for a key [`profile_key`] rejects.
    pub fn set_profile(&self, key: &str, value: &str, source: &str) -> Result<()> {
        let key = profile_key(key);
        if key.is_empty() {
            return Err(anyhow!("unknown profile field"));
        }
        let mut value = take_chars(value.trim(), 500).to_string();
        if key == "gender" && !value.is_empty() {
            value = gender_of(&value)
                .ok_or_else(|| anyhow!("invalid value"))?
                .to_string();
        }
        // a place field holds a code of the tables (a city code, an ISO country)
        value = crate::places::clean_value(&key, &value).ok_or_else(|| anyhow!("invalid value"))?;
        if !value.is_empty() {
            // an upsert, so the field's `standing` flag stays
            self.db.execute(
                "INSERT INTO profile(key, value, updated_at, source) VALUES (?,?,?,?) \
                 ON CONFLICT(key) DO UPDATE SET value=excluded.value, updated_at=excluded.updated_at, source=excluded.source",
                params![key.as_str(), value, now_ts(), source],
            )?;
        } else {
            self.db
                .execute("DELETE FROM profile WHERE key=?", params![key.as_str()])?;
        }
        // a pending suggestion for this field is settled by the user's own edit
        self.db.execute(
            "UPDATE profile_pending SET status='superseded', resolved_at=? WHERE key=? AND status='pending'",
            params![now_ts(), key.as_str()],
        )?;
        Ok(())
    }

    /// Queue a change for the user to confirm. Never changes the profile by itself. `None` when the suggestion is
    /// unusable (unknown field, blank, sensitive, bad shape, already the value, already pending).
    pub fn suggest_profile(
        &self,
        key: &str,
        value: &str,
        reason: &str,
        source: &str,
    ) -> Result<Option<Value>> {
        let key = profile_key(key);
        let value = take_chars(value.trim(), 500).to_string();
        let value = match (key.as_str(), gender_of(&value)) {
            ("gender", Some(g)) => g.to_string(),
            _ => value,
        };
        if key.is_empty()
            || value.is_empty()
            || looks_sensitive(&value)
            || !profile_value_ok(&key, &value)
        {
            return Ok(None);
        }
        let cur = get_str(&self.profile()?, &key);
        if norm_fact(&cur) == norm_fact(&value) {
            return Ok(None);
        }
        for r in self.db.all(
            "SELECT * FROM profile_pending WHERE key=? AND status='pending'",
            params![key.as_str()],
        )? {
            if norm_fact(&get_str(&r, "value")) == norm_fact(&value) {
                return Ok(None);
            }
        }
        let pid = new_id("pp");
        self.db.insert(
            "profile_pending",
            &json!({"id": pid, "key": key, "value": value, "old": cur, "reason": take_chars(reason, 300),
                    "source": take_chars(source, 80), "status": "pending", "created_at": now_ts(),
                    "resolved_at": null}),
        )?;
        self.db
            .one("SELECT * FROM profile_pending WHERE id=?", params![pid])
    }

    pub fn profile_pending(&self, status: &str) -> Result<Vec<Value>> {
        self.db.all(
            "SELECT * FROM profile_pending WHERE status=? ORDER BY created_at DESC LIMIT 100",
            params![status],
        )
    }

    /// Accept (optionally with an edited value) or reject a pending suggestion; `None` when it is not pending.
    pub fn resolve_profile(
        &self,
        pid: &str,
        accept: bool,
        value: Option<&str>,
    ) -> Result<Option<Value>> {
        let Some(r) = self
            .db
            .one("SELECT * FROM profile_pending WHERE id=?", params![pid])?
        else {
            return Ok(None);
        };
        if get_str(&r, "status") != "pending" {
            return Ok(None);
        }
        if accept {
            let v = value
                .map(str::to_string)
                .unwrap_or_else(|| get_str(&r, "value"));
            self.set_profile(
                &get_str(&r, "key"),
                &v,
                &format!("confirmed:{}", get_str(&r, "source")),
            )?;
        }
        self.db.execute(
            "UPDATE profile_pending SET status=?, resolved_at=? WHERE id=?",
            params![if accept { "accepted" } else { "rejected" }, now_ts(), pid],
        )?;
        self.db
            .one("SELECT * FROM profile_pending WHERE id=?", params![pid])
    }

    // ------------------------------------------------------------ memory housekeeping runs
    pub fn add_memory_run(&self, kind: &str, report: &Value) -> Result<i64> {
        self.db.insert(
            "memory_runs",
            &json!({"ts": now_ts(), "kind": kind, "report": pyjson::dumps(report)}),
        )
    }

    pub fn memory_runs(&self, limit: i64) -> Result<Vec<Value>> {
        let mut rows = self.db.all(
            "SELECT * FROM memory_runs ORDER BY id DESC LIMIT ?",
            params![limit],
        )?;
        for r in rows.iter_mut() {
            let rep = loads_col(r, "report", json!({}));
            set(r, "report", rep);
        }
        Ok(rows)
    }

    // ------------------------------------------------------------ schedules
    pub fn schedules(&self) -> Result<Vec<Value>> {
        let mut rows = self.db.all(
            "SELECT * FROM schedules ORDER BY created_at DESC",
            params![],
        )?;
        for r in rows.iter_mut() {
            let st = loads_col(r, "state", json!({}));
            set(r, "state", st);
        }
        Ok(rows)
    }

    pub fn schedule(&self, sid: &str) -> Result<Option<Value>> {
        let mut r = self
            .db
            .one("SELECT * FROM schedules WHERE id=?", params![sid])?;
        if let Some(row) = r.as_mut() {
            let st = loads_col(row, "state", json!({}));
            set(row, "state", st);
        }
        Ok(r)
    }

    /// Stores a long task's call (`{"tool", "args"}`) on schedule `sid`: its runs call the tool with it.
    pub fn set_schedule_task(&self, sid: &str, task: &Value) -> Result<()> {
        self.db.execute(
            "UPDATE schedules SET task=? WHERE id=?",
            params![pyjson::dumps(task), sid],
        )?;
        Ok(())
    }

    /// A schedule's long-task call (`{"tool", "args"}`), `None` for a schedule run from its goal text.
    pub fn schedule_task(sch: &Value) -> Option<Value> {
        let raw = get_str(sch, "task");
        serde_json::from_str::<Value>(&raw).ok().filter(|v| {
            v.get("tool")
                .and_then(Value::as_str)
                .is_some_and(|t| !t.is_empty())
        })
    }

    /// Records that conversation `conv_id` was offered a schedule for the task `key`; `false` when it was already.
    pub fn record_schedule_offer(&self, conv_id: &str, key: &str) -> Result<bool> {
        let n = self.db.execute(
            "INSERT OR IGNORE INTO schedule_offers (conv_id, task_key, created_at) VALUES (?, ?, ?)",
            params![conv_id, key, now_ts()],
        )?;
        Ok(n > 0)
    }

    /// Whether conversation `conv_id` was offered a schedule for the task `key`.
    pub fn schedule_offered(&self, conv_id: &str, key: &str) -> Result<bool> {
        Ok(self
            .db
            .one(
                "SELECT 1 AS x FROM schedule_offers WHERE conv_id=? AND task_key=?",
                params![conv_id, key],
            )?
            .is_some())
    }

    // ------------------------------------------------------------ goals
    pub fn goals(&self) -> Result<Vec<Value>> {
        let mut rows = self.db.all(
            "SELECT * FROM goals ORDER BY (status='active') DESC, created_at DESC",
            params![],
        )?;
        for r in rows.iter_mut() {
            let p = loads_col(r, "progress", json!([]));
            set(r, "progress", p);
        }
        Ok(rows)
    }

    pub fn goal(&self, gid: &str) -> Result<Option<Value>> {
        let mut r = self
            .db
            .one("SELECT * FROM goals WHERE id=?", params![gid])?;
        if let Some(row) = r.as_mut() {
            let p = loads_col(row, "progress", json!([]));
            set(row, "progress", p);
        }
        Ok(r)
    }

    // ------------------------------------------------------------ notifications
    pub fn notify(&self, title: &str, body: &str, task_id: &str, level: &str) -> Result<Value> {
        let nid = self.db.insert(
            "notifications",
            &json!({"ts": now_ts(), "title": title, "body": body, "task_id": task_id, "level": level, "read": 0}),
        )?;
        Ok(
            json!({"id": nid, "ts": now_ts(), "title": title, "body": body, "task_id": task_id, "level": level, "read": 0}),
        )
    }

    pub fn notifications(&self, limit: i64) -> Result<Vec<Value>> {
        self.db.all(
            "SELECT * FROM notifications ORDER BY id DESC LIMIT ?",
            params![limit],
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(toml: &str) -> Arc<Config> {
        Arc::new(Config::parse(toml).unwrap())
    }

    fn store() -> (tempfile::TempDir, Store) {
        let d = tempfile::tempdir().unwrap();
        let st = Store::new(d.path(), cfg("")).unwrap();
        (d, st)
    }

    #[test]
    fn the_previous_task_of_a_conversation_comes_with_its_transcript_a_bots_child_task_too() {
        let (_d, st) = store();
        let c = st.create_conv("travel", "chat", None).unwrap();
        let first = st
            .create_task("apply", &c, "chat", "", "task_main", None)
            .unwrap();
        let fid = get_str(&first, "id");
        st.update_task(
            &fid,
            &json!({"transcript": [{"role": "user", "content": "apply"}]}),
        )
        .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        let second = st
            .create_task("go ahead", &c, "chat", "", "", None)
            .unwrap();
        let sid = get_str(&second, "id");
        let prev = st.prev_task(&c, &sid).unwrap().unwrap();
        assert_eq!(get_str(&prev, "id"), fid);
        assert_eq!(prev["transcript"][0]["content"], "apply");
        assert!(st
            .prev_task(&c, &fid)
            .unwrap()
            .is_some_and(|t| get_str(&t, "id") == sid));
        assert!(st.prev_task("conv_none", &sid).unwrap().is_none());
    }

    #[test]
    fn a_merged_bots_conversations_move_to_the_bot_it_was_merged_into() {
        let d = tempfile::tempdir().unwrap();
        let st = Store::new(d.path(), cfg("")).unwrap();
        let (a, b, m) = (
            st.create_conv("a", "chat", None).unwrap(),
            st.create_conv("b", "chat", None).unwrap(),
            st.create_conv("m", "chat", None).unwrap(),
        );
        st.set_conv_bot(&a, "cities", "").unwrap();
        st.set_conv_bot(&b, "arrival", &m).unwrap();
        drop(st);
        let st = Store::new(d.path(), cfg("")).unwrap(); // the upgrade: the store opens again
        assert_eq!(st.conv_bot(&a), "travel");
        assert_eq!(st.conv_bot(&b), "travel");
        assert_eq!(get_str(&st.conv(&b).unwrap().unwrap(), "parent_conv"), m);
        assert_eq!(st.conv_bot(&m), "main");
    }

    #[test]
    fn memory_tiers_profile_and_sensitive() {
        let (_d, st) = store();
        assert!(
            looks_sensitive("card 4111 1111 1111 1111") && looks_sensitive("passport E12345678")
        );
        assert!(
            looks_sensitive("my password is x") && !looks_sensitive("call me at +65 9123 4567")
        );
        let recent = FactOpts {
            tier: "recent".into(),
            ..Default::default()
        };
        let r = st
            .add_fact("Booking ref ABC for Friday", "other", "", &recent)
            .unwrap()
            .unwrap();
        assert_eq!(r["tier"], "recent");
        assert!(st.facts(500, Some("long")).unwrap().is_empty());
        assert_eq!(st.facts(500, Some("recent")).unwrap().len(), 1);
        let again = st
            .add_fact(
                "Booking ref ABC for Friday",
                "other",
                "",
                &FactOpts::default(),
            )
            .unwrap()
            .unwrap();
        assert_eq!(again["duplicate"], true);
        assert_eq!(
            st.fact(r["id"].as_str().unwrap()).unwrap().unwrap()["tier"],
            "long"
        );
        assert!(st
            .suggest_profile("phone", "+65 9123 4567", "said in chat", "")
            .unwrap()
            .is_some());
        assert!(st
            .suggest_profile("phone", "+65 9123 4567", "", "")
            .unwrap()
            .is_none()); // same pending twice
        assert!(st
            .suggest_profile("passport", "E1", "", "")
            .unwrap()
            .is_none()); // not a profile field
        assert!(st
            .suggest_profile("custom:Loyalty", "4111 1111 1111 1111", "", "")
            .unwrap()
            .is_none()); // sensitive
        assert_eq!(st.profile().unwrap(), json!({})); // nothing changes without the user
        let p = st.profile_pending("pending").unwrap().remove(0);
        st.resolve_profile(p["id"].as_str().unwrap(), true, None)
            .unwrap();
        assert_eq!(st.profile().unwrap(), json!({"phone": "+65 9123 4567"}));
        st.set_profile("phone", "+65 8000 0000", "user").unwrap();
        assert_eq!(st.profile().unwrap()["phone"], "+65 8000 0000");
        assert!(st.set_profile("passport", "x", "user").is_err());
    }

    #[test]
    fn gender_is_as_on_the_passport() {
        let (_d, st) = store();
        for (given, saved) in [
            ("M", "Male"),
            ("female", "Female"),
            ("女", "Female"),
            ("X", "X"),
        ] {
            st.set_profile("gender", given, "user").unwrap();
            assert_eq!(st.profile().unwrap()["gender"], saved);
        }
        assert!(st.set_profile("gender", "maybe", "user").is_err());
        assert!(profile_value_ok("gender", "F") && !profile_value_ok("gender", "tall"));
        st.set_profile("gender", "", "user").unwrap();
        let p = st.suggest_profile("gender", "m", "", "").unwrap().unwrap();
        assert_eq!(p["value"], "Male");
    }

    #[test]
    fn profile_shapes() {
        assert!(
            profile_value_ok("email_work", "lucas@bytetradelab.io")
                && !profile_value_ok("email_work", "bytetrade")
        );
        assert!(
            profile_value_ok("phone", "+65 9123 4567") && !profile_value_ok("phone", "call me")
        );
        assert_eq!(profile_key("custom:  My   label "), "custom:My label");
        assert_eq!(
            profile_key("address: Singapore  flat"),
            "address:Singapore flat"
        );
        assert_eq!(profile_key("address:"), "");
        assert_eq!(profile_key("nope"), "");
        let (_d, st) = store();
        st.add_fact(
            "The user prefers metal pens.",
            "preference",
            "",
            &FactOpts::default(),
        )
        .unwrap();
        assert!(st
            .suggest_profile("email_work", "bytetrade", "", "")
            .unwrap()
            .is_none());
    }

    #[test]
    fn forget_removes_the_episodes_that_used_the_fact() {
        let (_d, st) = store();
        let o = FactOpts {
            source: "user-request:t1".into(),
            confidence: 0.95,
            task_id: "t1".into(),
            ..Default::default()
        };
        let f = st
            .add_fact("每月打车预算 300 新元（SGD）", "preference", "", &o)
            .unwrap()
            .unwrap();
        let fid = f["id"].as_str().unwrap().to_string();
        // t1 wrote the fact, t2 was given it in its prompt (both by id), t3 never saw it
        st.add_episode("t1", "[测试] 请记住：我每月打车预算 300 新元 → 已记住")
            .unwrap();
        st.note_task_memory("t2", &["travel".into()], &[], std::slice::from_ref(&fid))
            .unwrap();
        st.add_episode("t2", "[测试] 叫车去机场 → 已叫车").unwrap();
        st.add_episode("t3", "[测试] 买一双徒步鞋，预算不超过 S$250 → 已筛选 3 双")
            .unwrap();
        assert_eq!(st.task_memory("t2").unwrap()["topics"], json!(["travel"]));
        st.delete_fact(&fid).unwrap();
        let left: Vec<String> = st
            .episodes(10)
            .unwrap()
            .iter()
            .map(|e| get_str(e, "summary"))
            .collect();
        assert_eq!(left.len(), 1, "{left:?}");
        assert!(left[0].contains("徒步鞋"));
        // an episode written before 0.6.0 (no fact ids) goes when the user forgets any fact; a merge relinks
        st.db
            .execute(
                "INSERT INTO episodes(ts, task_id, summary, fact_ids) VALUES (?,?,?,'')",
                params![now_ts(), "old", "old summary"],
            )
            .unwrap();
        let g = st
            .add_fact(
                "Likes tea",
                "preference",
                "",
                &FactOpts {
                    task_id: "t4".into(),
                    ..Default::default()
                },
            )
            .unwrap()
            .unwrap();
        let h = st
            .add_fact("Likes green tea", "preference", "", &FactOpts::default())
            .unwrap()
            .unwrap();
        st.add_episode("t4", "tea → ok").unwrap();
        st.relink_fact(g["id"].as_str().unwrap(), h["id"].as_str().unwrap())
            .unwrap();
        st.delete_fact(g["id"].as_str().unwrap()).unwrap();
        assert_eq!(st.episodes(10).unwrap().len(), 3); // relinked: kept; the old one stays on a plain delete
        st.forget_fact(h["id"].as_str().unwrap()).unwrap();
        let left: Vec<String> = st
            .episodes(10)
            .unwrap()
            .iter()
            .map(|e| get_str(e, "summary"))
            .collect();
        assert_eq!(left.len(), 1, "{left:?}");
    }

    #[test]
    fn search_by_topics_entities_and_words() {
        let (_d, st) = store();
        let shoes = FactOpts::default().with_model_fields(
            &json!({"domain": "preference", "topics": ["clothing", "shopping", "bogus"], "scope": "topic"}),
        );
        assert_eq!(shoes.topics, vec!["clothing", "shopping"]);
        st.add_fact(
            "我身高177，体重76公斤，运动鞋43码。",
            "preference",
            "",
            &shoes,
        )
        .unwrap();
        let site = FactOpts {
            domain: "site".into(),
            status: "pending".into(),
            ..Default::default()
        }
        .with_model_fields(&json!({"topics": ["shopping"]}));
        let f = st
            .add_fact(
                "decathlon.sg asks for the phone as 8 digits without +65.",
                "habit",
                "decathlon.sg",
                &site,
            )
            .unwrap()
            .unwrap();
        assert_eq!(
            (f["status"].as_str(), f["domain"].as_str()),
            (Some("pending"), Some("site"))
        );
        let passport = FactOpts::default()
            .with_model_fields(&json!({"topics": ["travel", "forms"], "id_document": true}));
        st.add_fact(
            "The user's passport expires in March 2027.",
            "other",
            "",
            &passport,
        )
        .unwrap();
        st.add_fact(
            "An old fact nobody sorted.",
            "other",
            "",
            &FactOpts::default(),
        )
        .unwrap();
        let q = |topics: &[&str], entities: &[&str], query: &str, id: bool| {
            st.search_facts(&MemoryQuery {
                query: query.into(),
                topics: topics.iter().map(|x| x.to_string()).collect(),
                entities: entities.iter().map(|x| x.to_string()).collect(),
                include_id: id,
                tier: Some("long".into()),
                limit: 5,
            })
            .unwrap()
            .iter()
            .map(|f| get_str(f, "fact"))
            .collect::<Vec<_>>()
        };
        let got = q(&["clothing"], &[], "", false);
        assert_eq!(got.len(), 1);
        assert!(got[0].contains("43码"));
        let got = q(&[], &["Decathlon.sg"], "", false);
        assert!(got.len() == 1 && got[0].contains("8 digits"));
        // the entity first, then the topic
        let got = q(&["shopping"], &["decathlon.sg"], "", false);
        assert!(got[0].contains("8 digits") && got.len() == 2);
        // identity documents and unsorted facts only with include_id
        assert!(q(&["travel"], &[], "", false).is_empty());
        assert_eq!(q(&["travel"], &[], "", true).len(), 1);
        assert!(q(&[], &[], "nobody sorted", false).is_empty());
        assert_eq!(q(&[], &[], "nobody sorted", true).len(), 1);
        // the model's query words: a plain text search
        assert!(q(&[], &[], "phone", false)[0].contains("decathlon"));
        assert_eq!(st.hidden_fact_count().unwrap(), (1, 1));
        assert_eq!(st.unsorted_facts(10).unwrap().len(), 1);
        // the user confirms; a bad status is dropped; a new text keeps the old one in the history
        let fid = f["id"].as_str().unwrap();
        st.update_fact(fid, &json!({"status": "active"})).unwrap();
        st.update_fact(
            fid,
            &json!({"status": "bogus", "fact": "decathlon.sg wants 8 digits"}),
        )
        .unwrap();
        let r = st.fact(fid).unwrap().unwrap();
        assert_eq!(r["status"], "active");
        assert!(get_str(&r, "history").contains("without +65"));
    }

    #[test]
    fn model_fields_are_validated() {
        let o = FactOpts::default().with_model_fields(&json!({
            "domain": "Rule", "scope": "trigger", "trigger": "准备出差", "topics": "travel", "relation": "wife"}));
        assert_eq!(
            (
                o.domain.as_str(),
                o.scope.as_str(),
                o.trigger.as_str(),
                o.kind.as_str()
            ),
            ("rule", "trigger", "准备出差", "general")
        );
        assert_eq!(o.topics, vec!["travel"]);
        // a trigger scope without its phrase is a topic scope; nothing valid given = still unsorted
        let o = FactOpts::default().with_model_fields(&json!({"scope": "trigger"}));
        assert_eq!(o.scope, "topic");
        let o = FactOpts::default()
            .with_model_fields(&json!({"domain": "nonsense", "scope": "sometimes"}));
        assert!(o.domain.is_empty() && o.scope.is_empty() && o.kind.is_empty());
        let (_d, st) = store();
        let r = st
            .add_fact(
                "Always ask before spending over S$100.",
                "",
                "",
                &FactOpts::default()
                    .with_model_fields(&json!({"domain": "rule", "scope": "always"})),
            )
            .unwrap()
            .unwrap();
        let id = r["id"].as_str().unwrap();
        let f = st.fact(id).unwrap().unwrap();
        assert_eq!(
            (get_str(&f, "scope"), get_str(&f, "kind")),
            ("always".into(), "general".into())
        );
        assert_eq!(f["topics"], json!([]));
        st.update_fact(
            id,
            &json!({"topics": ["finance", "x"], "scope": "nope", "kind": "id_document"}),
        )
        .unwrap();
        let f = st.fact(id).unwrap().unwrap();
        assert_eq!(f["topics"], json!(["finance"]));
        assert_eq!(get_str(&f, "scope"), "always");
        assert_eq!(get_str(&f, "kind"), "id_document");
        // a person with a relation: kept on the entity, the first one (or the user's own) wins
        for (fact, rel) in [
            ("Mei collects jazz vinyl.", "wife"),
            ("Mei likes tea.", "friend"),
        ] {
            st.add_fact(
                fact,
                "person",
                "Mei",
                &FactOpts {
                    domain: "person".into(),
                    relation: rel.into(),
                    ..Default::default()
                },
            )
            .unwrap();
        }
        let e = st
            .entities(None)
            .unwrap()
            .into_iter()
            .find(|e| e["name"] == "Mei")
            .unwrap();
        assert_eq!(e["relation"], "wife");
        // a fact with a relation is about that person whatever domain the model chose (memory_real 2026-11-08)
        let r = st
            .add_fact(
                "The user's wife, Mei, collects jazz vinyl records.",
                "collecting preference",
                "Mei",
                &FactOpts {
                    domain: "preference".into(),
                    relation: "wife".into(),
                    ..Default::default()
                },
            )
            .unwrap()
            .unwrap();
        assert_eq!(get_str(&r, "domain"), "person");
        let r = st
            .add_fact(
                "Ken likes tea.",
                "person",
                "Ken",
                &FactOpts {
                    domain: "preference".into(),
                    relation: "friend".into(),
                    ..Default::default()
                },
            )
            .unwrap()
            .unwrap();
        assert_eq!(get_str(&r, "domain"), "person");
        let ken = st
            .entities(None)
            .unwrap()
            .into_iter()
            .find(|e| e["name"] == "Ken")
            .unwrap();
        assert_eq!(ken["type"], "person");
        assert_eq!(ken["relation"], "friend");
    }

    #[test]
    fn standing_custom_fields() {
        let (_d, st) = store();
        st.set_profile("custom:Star Alliance status", "Gold", "user")
            .unwrap();
        st.set_profile("custom:Shoe size", "43", "user").unwrap();
        assert!(st.profile_standing().unwrap().is_empty()); // off by default
        st.set_profile_standing("custom:Star Alliance status", true)
            .unwrap();
        assert!(st.set_profile_standing("phone", true).is_err());
        assert!(st.set_profile_standing("custom:Nope", true).is_err());
        // a new value keeps the flag
        st.set_profile("custom:Star Alliance status", "Platinum", "user")
            .unwrap();
        assert_eq!(
            st.profile_standing().unwrap(),
            vec!["custom:Star Alliance status"]
        );
        st.set_profile_standing("custom:Star Alliance status", false)
            .unwrap();
        assert!(st.profile_standing().unwrap().is_empty());
    }

    #[test]
    fn entities_and_relations() {
        let (_d, st) = store();
        st.add_fact(
            "Mei collects jazz vinyl records.",
            "person",
            "Mei",
            &FactOpts::default(),
        )
        .unwrap();
        st.add_fact(
            "The user likes jazz.",
            "preference",
            "",
            &FactOpts::default(),
        )
        .unwrap();
        let e = st
            .entities(None)
            .unwrap()
            .into_iter()
            .find(|x| x["name"] == "Mei")
            .unwrap();
        assert_eq!(e["type"], "person");
        let u = st
            .update_entity(
                e["id"].as_str().unwrap(),
                Some(&["梅梅".to_string()]),
                Some("太太"),
                None,
            )
            .unwrap()
            .unwrap();
        assert_eq!(u["attrs"]["aliases"], json!(["梅梅"]));
        assert_eq!(st.entities(None).unwrap()[0]["relation"], "太太");
        assert_eq!(st.entities(Some("person")).unwrap().len(), 1);
        st.delete_entity(e["id"].as_str().unwrap()).unwrap();
        assert!(st.entities(None).unwrap().is_empty());
    }

    #[test]
    fn model_override_effective_settings() {
        let c = cfg("[model]\nurl='http://sys/v1'\nname='sys-llm'\nplanner='sys-planner'\napi_key='sys-key'\nimage_url='http://sys-img/v1'\nimage_api_key='sys-img-key'\n");
        let d = tempfile::tempdir().unwrap();
        let st = Store::new(d.path(), c).unwrap();
        let s = st.settings().unwrap();
        assert_eq!(s["model_override"], false);
        assert_eq!(s["model_base_url"], "http://sys/v1");
        assert_eq!(s["model_name"], "sys-llm");
        assert_eq!(s["model_api_key"], "sys-key");
        assert_eq!(s["instance_name"], "OMuse");
        assert_eq!(
            st.user_model().unwrap(),
            json!({"model_base_url": "", "model_name": "", "planner_model": "", "vision_model": "", "stt_model": "",
                   "image_model": "", "image_base_url": "", "video_model": "", "video_base_url": "",
                   "fast_mode": false, "reasoning_effort": "", "api_key_set": false,
                   "image_api_key_set": false, "video_api_key_set": false})
        );
        // the system's models: its image endpoint and image key (the config's), the user's never
        assert_eq!(s["image_base_url"], "http://sys-img/v1");
        assert_eq!(s["image_api_key"], "sys-img-key");
        assert_eq!(s["video_base_url"], "");
        // the user's own values saved but switched off: ignored
        st.set_settings(&json!({"model_base_url": "http://me/v1", "model_name": "my-llm", "model_api_key": "my-key"})).unwrap();
        let s = st.settings().unwrap();
        assert_eq!(s["model_base_url"], "http://sys/v1");
        assert_eq!(s["model_name"], "sys-llm");
        assert_eq!(s["model_api_key"], "sys-key");
        assert_eq!(s["planner_model"], "sys-planner");
        assert_eq!(st.user_model().unwrap()["model_name"], "my-llm");
        assert_eq!(st.user_model().unwrap()["api_key_set"], true);
        // switched on: the user's set, and only the user's key goes to their endpoint
        st.set_settings(&json!({"model_override": true})).unwrap();
        let s = st.settings().unwrap();
        assert_eq!(s["model_base_url"], "http://me/v1");
        assert_eq!(s["model_name"], "my-llm");
        assert_eq!(s["model_api_key"], "my-key");
        // never saved by the user: the default (the system's planner) shows through, as DEFAULT_SETTINGS did in Python
        assert_eq!(s["planner_model"], "sys-planner");
        // but never the system's image endpoint or key: the user's own (none saved = their model endpoint and key)
        assert_eq!(s["image_base_url"], "");
        assert_eq!(s["image_api_key"], "");
        let red = redacted(&s);
        assert!(red.get("model_api_key").is_none());
        assert_eq!(red["model_api_key_set"], true);
        assert_eq!(red["image_api_key_set"], false);
        assert_eq!(red["model_name"], "my-llm");
        let txt = pyjson::dumps(&red);
        assert!(
            !txt.contains("sys-key") && !txt.contains("my-key") && !txt.contains("sys-img-key")
        );
        // the user's optional video endpoint and key, used only while their set is on
        st.set_settings(&json!({"video_base_url": "http://me-vid/v1", "video_api_key": "vid-key", "video_model": "sora-2"})).unwrap();
        let s = st.settings().unwrap();
        assert_eq!(s["video_base_url"], "http://me-vid/v1");
        assert_eq!(s["video_api_key"], "vid-key");
        assert_eq!(st.user_model().unwrap()["video_api_key_set"], true);
        assert!(redacted(&s).get("video_api_key").is_none());
        // off again: back to the system's, the user's values kept
        st.set_settings(&json!({"model_override": false})).unwrap();
        assert_eq!(st.settings().unwrap()["model_name"], "sys-llm");
        assert_eq!(st.settings().unwrap()["video_base_url"], "");
        assert_eq!(st.settings().unwrap()["video_api_key"], "");
        assert_eq!(st.user_model().unwrap()["model_name"], "my-llm");
        // unknown keys are ignored; saved values round-trip through JSON
        st.set_settings(&json!({"nope": 1, "skills_disabled": ["a"], "vision_model": "saved-v"}))
            .unwrap();
        let saved = st.saved_settings().unwrap();
        assert!(saved.get("nope").is_none());
        assert_eq!(saved["skills_disabled"], json!(["a"]));
        assert_eq!(st.settings().unwrap()["vision_model"], ""); // system's (empty) while override is off
    }

    #[test]
    fn fast_mode_and_reasoning_follow_the_model_in_force() {
        let c = cfg(
            "[model]\nurl='http://sys/v1'\nname='sys-llm'\nfast=true\nreasoning_effort='High'\n",
        );
        let d = tempfile::tempdir().unwrap();
        let st = Store::new(d.path(), c).unwrap();
        // the system's models: the config's values, whatever was saved
        let s = st.settings().unwrap();
        assert_eq!(
            (&s["fast_mode"], &s["reasoning_effort"]),
            (&json!(true), &json!("high"))
        );
        st.set_settings(&json!({"fast_mode": false, "reasoning_effort": "low"}))
            .unwrap();
        let s = st.settings().unwrap();
        assert_eq!(
            (&s["fast_mode"], &s["reasoning_effort"]),
            (&json!(true), &json!("high"))
        );
        // the user's own model: the user's values
        st.set_settings(&json!({"model_override": true, "model_base_url": "http://me/v1", "model_name": "m", "model_api_key": "k"}))
            .unwrap();
        let s = st.settings().unwrap();
        assert_eq!(
            (&s["fast_mode"], &s["reasoning_effort"]),
            (&json!(false), &json!("low"))
        );
        assert_eq!(st.user_model().unwrap()["reasoning_effort"], "low");
        // an unknown effort in the config is not sent
        let c = cfg("[model]\nreasoning_effort='turbo'\n");
        let d2 = tempfile::tempdir().unwrap();
        assert_eq!(
            Store::new(d2.path(), c).unwrap().settings().unwrap()["reasoning_effort"],
            ""
        );
    }

    #[test]
    fn conversations_tasks_events() {
        let (_d, st) = store();
        let cid = st.create_conv("t", "chat", None).unwrap();
        let t1 = st
            .create_task(
                "在 Decathlon 新加坡官网（decathlon.sg）真实下单买一件小东西",
                &cid,
                "chat",
                "",
                "",
                None,
            )
            .unwrap();
        let tid1 = t1["id"].as_str().unwrap().to_string();
        assert_eq!(t1["status"], "CREATED");
        assert_eq!(t1["plan"], json!({}));
        assert_eq!(t1["transcript"], json!([]));
        assert_eq!(t1["pending"], Value::Null);
        assert_eq!(t1["attachments"], json!([]));
        st.add_msg(
            &cid,
            "user",
            "hello",
            &tid1,
            Some(&json!({"attachments": [{"name": "a.pdf"}]})),
        )
        .unwrap();
        st.add_event(
            &tid1,
            "tool_call",
            &json!({"name": "browser_navigate", "args": {"url": "https://www.decathlon.sg/cart"}}),
        )
        .unwrap();
        st.update_task(
            &tid1,
            &json!({"status": "CANCELLED", "plan": {"objective": "x", "steps": []}, "steps": 3}),
        )
        .unwrap();
        let t = st.task(&tid1).unwrap().unwrap();
        assert_eq!(t["status"], "CANCELLED");
        assert_eq!(t["plan"]["objective"], "x");
        assert_eq!(t["steps"], 3);
        let ev = st.events(&tid1, 0).unwrap();
        assert_eq!(ev.len(), 1);
        assert_eq!(ev[0]["data"]["name"], "browser_navigate");
        let msgs = st.msgs(&cid, 200).unwrap();
        assert_eq!(msgs[0]["meta"]["attachments"][0]["name"], "a.pdf");
        assert_eq!(st.conv_attachments(&cid).unwrap().len(), 1);
        let list = st.tasks(Some("CANCELLED,FAILED"), 10, Some(&cid)).unwrap();
        assert_eq!(list.len(), 1);
        assert!(st.tasks(Some("COMPLETED"), 10, None).unwrap().is_empty());
        assert_eq!(st.convs(10).unwrap().len(), 1);
        st.set_conv_title(&cid, "new").unwrap();
        assert_eq!(st.conv(&cid).unwrap().unwrap()["title"], "new");
        st.delete_conv(&cid).unwrap();
        assert!(st.conv(&cid).unwrap().is_none());
        let n = st.notify("hi", "body", "", "info").unwrap();
        assert_eq!(n["read"], 0);
        assert_eq!(st.notifications(5).unwrap().len(), 1);
        let rid = st.add_memory_run("tidy", &json!({"merged": 1})).unwrap();
        assert_eq!(rid, 1);
        assert_eq!(st.memory_runs(5).unwrap()[0]["report"]["merged"], 1);
        assert!(st.schedules().unwrap().is_empty() && st.goals().unwrap().is_empty());
    }

    #[test]
    fn library_subjects_and_search() {
        let (_d, st) = store();
        let rec = |slug: &str, title: &str, id: &str| {
            json!({"id": id, "slug": slug, "title": title, "brief": "cheap", "status": "new", "conv_id": "",
                   "created_at": now_ts(), "updated_at": now_ts(), "last_task": "", "runs": 0, "chars": 0, "error": ""})
        };
        let sub = st
            .create_subject(&rec("home-nas-2026", "Home NAS 2026", "sub_1"))
            .unwrap();
        let sub2 = st
            .create_subject(&rec("home-nas-2026", "Home NAS 2026", "sub_2"))
            .unwrap();
        assert_eq!(sub["slug"], "home-nas-2026");
        assert_eq!(sub2["slug"], "home-nas-2026-2");
        st.library_replace(
            "sub_1",
            &[
                ("Candidates".into(), "UGREEN DXP4800 runs Docker.".into()),
                ("Noise".into(), "quiet at idle".into()),
            ],
        )
        .unwrap();
        st.update_subject(
            "sub_1",
            &json!({"status": "ready", "chars": 40, "bogus": 1}),
        )
        .unwrap();
        assert_eq!(st.subject("sub_1").unwrap().unwrap()["status"], "ready");
        let hits = st.library_search("docker nas", 5).unwrap();
        assert!(!hits.is_empty());
        assert_eq!(hits[0]["title"], "Home NAS 2026");
        assert_eq!(hits[0]["heading"], "Candidates");
        assert!(get_str(&hits[0], "text").contains("DXP4800"));
        assert!(st.library_search("", 5).unwrap().is_empty());
        assert!(st.library_search("zzzz-nothing", 5).unwrap().is_empty());
        st.delete_subject("sub_1").unwrap();
        assert!(st.library_search("docker", 5).unwrap().is_empty());
        assert!(st.subject("sub_1").unwrap().is_none());
        assert!(st.subject_by_conv("").unwrap().is_none());
    }

    #[test]
    fn prune_expired() {
        let (_d, st) = store();
        let o = FactOpts {
            tier: "recent".into(),
            ..Default::default()
        };
        let f = st
            .add_fact("Order 123 is on the way", "other", "", &o)
            .unwrap()
            .unwrap();
        st.db
            .execute(
                "UPDATE facts SET expires_at=1 WHERE id=?",
                params![f["id"].as_str().unwrap()],
            )
            .unwrap();
        assert!(st.facts(10, Some("recent")).unwrap().is_empty()); // expired = not live
        let r = st.prune_memory().unwrap();
        assert_eq!(r["facts"], 1);
        assert!(st.fact(f["id"].as_str().unwrap()).unwrap().is_none());
    }

    #[test]
    fn render_groups_by_domain() {
        let sel = vec![
            json!({"fact": "The user wears size 43.", "domain": "preference", "score": 2.0, "status": "pending"}),
        ];
        let txt = render_facts(&sel, true);
        assert!(txt.starts_with("[Preferences]\n- The user wears size 43. (unconfirmed"));
        assert!(render_facts(&sel, false).starts_with("[偏好 Preferences]"));
        let rule = vec![
            json!({"fact": "Check the mail first.", "domain": "rule", "scope": "trigger", "trigger": "prepare for a trip", "score": 10.0}),
        ];
        assert!(render_facts(&rule, true)
            .contains("Check the mail first. (the user said “prepare for a trip”)"));
        assert_eq!(
            render_facts(&[], false),
            "(nothing on file that bears on this request)"
        );
    }
}
