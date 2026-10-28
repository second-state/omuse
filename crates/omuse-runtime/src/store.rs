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
  conv_id TEXT, progress TEXT, result TEXT, created_at REAL, updated_at REAL, finished_at REAL
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

/// `PROFILE_FIELDS`: key, 中文, English.
pub const PROFILE_FIELDS: [(&str, &str, &str); 14] = [
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
    ("company", "公司", "Company"),
    ("job_title", "职位", "Job title"),
    ("birthday", "生日", "Date of birth"),
    ("nationality", "国籍", "Nationality"),
    ("preferred_airline", "常用航空公司", "Preferred airline"),
    (
        "preferred_hotel_brand",
        "常用酒店品牌",
        "Preferred hotel brand",
    ),
];

/// The profile keys that are standing preferences the executor always sees (`Runtime::standing_prefs`): the two
/// fixed ones, and any custom field whose label carries one of [`PREFERENCE_WORDS`].
pub const PREFERENCE_KEYS: [&str; 2] = ["preferred_airline", "preferred_hotel_brand"];

/// Words (lower-case) that mark a custom profile field as a travel / loyalty preference.
pub const PREFERENCE_WORDS: [&str; 16] = [
    "airline", "carrier", "航空", "alliance", "loyalty", "miles", "mileage", "status", "seat",
    "hotel", "酒店", "brand", "chain", "会员", "member", "常用",
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
    for prefix in ["custom:", "address:"] {
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
    if key == "phone" {
        let digits = NON_DIGIT.replace_all(v, "");
        let n = digits.chars().count();
        return (6..=15).contains(&n) && PHONE.is_match(v);
    }
    true
}

/// Card / ID / passport-looking numbers and passwords never go into memory — they belong in the vault.
pub fn looks_sensitive(text: &str) -> bool {
    SENSITIVE.is_match(text).unwrap_or(false)
}

/// Whether `text` restates `fact`: its numbers plus at least two of its words (CJK bigrams or latin words).
pub fn mentions_fact(fact: &str, text: &str) -> bool {
    static NUMS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\d[\d,.]*\d|\d{2,}").unwrap());
    static CJK: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[一-鿿]+").unwrap());
    static WORDS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[A-Za-z]{4,}").unwrap());
    let nums: HashSet<&str> = NUMS.find_iter(fact).map(|m| m.as_str()).collect();
    let mut toks: HashSet<String> = HashSet::new();
    for run in CJK.find_iter(fact) {
        let cs: Vec<char> = run.as_str().chars().collect();
        for w in cs.windows(2) {
            toks.insert(w.iter().collect());
        }
    }
    for w in WORDS.find_iter(fact) {
        toks.insert(w.as_str().to_lowercase());
    }
    let low = text.to_lowercase();
    let hits = toks.iter().filter(|t| low.contains(t.as_str())).count();
    if !nums.is_empty() {
        return nums.iter().any(|n| text.contains(n)) && hits >= 2;
    }
    !toks.is_empty() && hits >= std::cmp::max(3, toks.len() / 2)
}

// ------------------------------------------------------------------------------------------------
// From app/runtime/context.py: the pieces the store itself needs (domains, request intents, tokens) and
// the prompt rendering of selected facts. `context.rs` is ported separately and may `pub use` these.
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

fn rx(p: &str) -> Regex {
    Regex::new(p).unwrap()
}

static EMAIL_ADDR: LazyLock<Regex> = LazyLock::new(|| rx(r"[\w.+-]+@[\w-]+(\.[\w-]+)+"));
static WEBSITE: LazyLock<Regex> = LazyLock::new(|| {
    rx(r"(?i)\b[a-z0-9-]+\.(?:com|sg|co|net|org|io|my|hk|cn|jp|uk|de)(?:\.[a-z]{2})?\b")
});
static SITE_HOW: LazyLock<Regex> = LazyLock::new(|| {
    rx(
        r"(?i)网站|网页|结账|结算|付款页|下单页|访客|登录|注册|填写|格式|表单|送货到家|checkout|guest|log ?in|sign ?in|form|format|field|delivery option|home delivery|accepts|without \+|8 digits|位数",
    )
});
static RULE: LazyLock<Regex> = LazyLock::new(|| {
    rx(
        r"(?i)以后|每次|总是|一律|默认|上限|不超过|不要|别再|必须|先问|先确认|固定流程|\b(always|never|whenever|every time|by default|default to|must|do not|don't|avoid|limit|cap|at most|ask (me |the user )?(first|before))\b|the assistant|OMuse|助手",
    )
});
static PERSON: LazyLock<Regex> = LazyLock::new(|| {
    rx(
        r"(?i)同事|同行|家人|太太|老婆|丈夫|老公|孩子|儿子|女儿|父母|爸|妈|朋友|老板|助理|秘书|\b(travels? with|colleague|collaborat\w*|works? with|wife|husband|son|daughter|kids?|family|friend|boss|assistant to|partner|manager)\b",
    )
});
static PLACE: LazyLock<Regex> = LazyLock::new(|| {
    rx(
        r"(?i)地址|住在|住址|邮编|办公室|公司地址|城市|时区|located at|\blives? in\b|(home|office|work|street|postal|mailing) address|address (is|at)|postal|\bRd\b|road|street|avenue|time ?zone|SGT|UTC|\bhome\b|office",
    )
});
static ACCOUNT: LazyLock<Regex> = LazyLock::new(|| {
    rx(
        r"(?i)账户|账号|会员|积分|订阅|邮箱|手机号|电话号码|号码是|\b(account|membership|member|subscription|loyalty|points|krisflyer|mailbox|email accounts?|phone (number|line))\b",
    )
});
static PREF: LazyLock<Regex> = LazyLock::new(|| {
    rx(
        r"(?i)喜欢|偏好|偏爱|爱吃|常用|习惯|尺码|鞋码|\d+码|身高|体重|只要|想要|\b(prefer\w*|likes?|enjoys?|wants?|favou?rite|interested|size|uses)\b",
    )
});

/// Best-guess domain for a fact (the user can change it on the Memory page).
pub fn domain_of(fact: &str, category: &str, entity: &str) -> &'static str {
    static WEBSITE_WORD: LazyLock<Regex> = LazyLock::new(|| rx(r"(?i)网站|website"));
    static LOCATED: LazyLock<Regex> = LazyLock::new(|| rx(r"(?i)located|地址|address"));
    static PREFERS: LazyLock<Regex> = LazyLock::new(|| {
        rx(r"(?i)prefer|interested in (buying|purchasing|dining)|enjoys?|喜欢|偏好")
    });
    let text = format!("{} {}", fact, entity);
    let plain = EMAIL_ADDR.replace_all(&text, " ");
    let cat = category.to_lowercase();
    match cat.as_str() {
        "place" => return "place",
        "account" => return "account",
        "site" => return "site",
        "rule" => return "rule",
        "work" => return "work",
        _ => {}
    }
    if (WEBSITE.is_match(&plain) || WEBSITE_WORD.is_match(&plain)) && SITE_HOW.is_match(&plain) {
        return "site";
    }
    if RULE.is_match(&plain) && cat != "person" {
        return "rule";
    }
    if cat == "person" {
        return "person";
    }
    if cat == "company" || cat == "project" {
        if PLACE.is_match(&plain) && LOCATED.is_match(&plain) {
            return "place";
        }
        if ACCOUNT.is_match(&text) {
            return "account";
        }
        return if PREFERS.is_match(&plain) {
            "preference"
        } else {
            "work"
        };
    }
    if PERSON.is_match(&plain) {
        return "person";
    }
    if PLACE.is_match(&plain) {
        return "place";
    }
    if ACCOUNT.is_match(&text) {
        return "account";
    }
    if cat == "preference" || cat == "habit" || PREF.is_match(&plain) {
        return "preference";
    }
    "work"
}

/// `INTENTS`: name → (what a request mentions, what the facts that matter for it mention).
pub const INTENTS: [(&str, &str, &str); 13] = [
    (
        "shopping",
        r"买|购|下单|订购|购物车|挑(一|个|件|双|支|款|几)|选(一|个|件|双|支|款)|推荐.{0,8}(款|个|件|双|支)|送人|礼物|\b(buy|purchase|order|shop|cart|gift)\b",
        r"送货|\bdeliver(y|ed)?\b|结账|checkout|访客|guest|注册|个人信息|personal information|shops? on|购物|预算|budget",
    ),
    (
        "wear",
        r"鞋|靴|衣|裤|裙|外套|夹克|t恤|衬衫|袜|帽|\b(shoes?|sneakers?|boots?|shirts?|t-shirt|tee|jacket|pants|dress|socks)\b",
        r"尺码|鞋码|\d+码|身高|体重|\bsize\b|\bcm\b|\bkg\b",
    ),
    (
        "phone_gear",
        r"手机|iphone|充电|耳机|\b(phone|case|charger|earbuds|airpods)\b",
        r"iphone|手机壳|phone cases?|otterbox|android|pixel|galaxy",
    ),
    ("pen", r"笔|钢笔|\bpens?\b", r"笔|\bpens?\b"),
    (
        "dining",
        r"餐厅|吃饭|晚饭|午饭|晚餐|午餐|订位|订座|订台|饭店|\b(restaurant|dinner|lunch|brunch|book a table|cuisine)\b",
        r"菜|cuisine|口味|吃|餐|restaurant|dining|chope|饮食|素食|vegetarian|辣|spicy|japanese|日本|湘|hunan",
    ),
    (
        "travel",
        r"机票|航班|飞往|飞去|出差|旅行|旅游|酒店|住宿|行程|签证|\b(flights?|fly|hotels?|trip|travel|itinerary)\b",
        r"航班|flight|直飞|\bdirect\b|舱|\bclass\b|座位|seat|酒店|hotel|精品|boutique|安静|quiet|同行|travels? with|companion|一起|签证|护照|krisflyer|airline|航空",
    ),
    (
        "email",
        r"邮件|邮箱|回复|回信|写信|草稿|\b(e-?mail|mail|reply|draft)\b",
        r"邮箱|e-?mails?|mail|发件|sending|签名|signature|bytetradelab|sixwings",
    ),
    (
        "meeting",
        r"会议|开会|日历|约(他|她|一下|个)|\b(meeting|calendar|appointment)\b",
        r"日历|calendar|时区|time ?zone|sgt|会议|meeting",
    ),
    (
        "report",
        r"报告|简报|总结|汇总|新闻|周报|日报|\b(report|brief|summary|news|digest|pdf)\b",
        r"pdf|报告|\breports?\b|中文|chinese|格式|\bformat\b|新闻|\bnews\b|deliverables?",
    ),
    (
        "outdoor",
        r"徒步|爬山|远足|跑步|健身|\b(hike|hiking|trail|run|running|exercise|workout)\b",
        r"徒步|hik|强度|intensity|小时|hours",
    ),
    (
        "phone",
        r"打电话|致电|电话给|\b(call|phone)\b",
        r"电话|phone|号码|number",
    ),
    (
        "form",
        r"填表|表格|表单|报名|\b(form|apply|sign ?up|register)\b",
        r"地址|address|电话|phone|邮箱|e-?mail|姓名|name|公司|company|个人信息|personal information|注册",
    ),
    (
        "unsubscribe",
        r"退订|取消订阅|广告邮件|垃圾邮件|\b(unsubscribe|newsletters?|spam|promotions?)\b",
        r"退订|unsubscribe|已读|as read|non-important",
    ),
];

static INTENT_RX: LazyLock<Vec<(&'static str, Regex, Regex)>> = LazyLock::new(|| {
    INTENTS
        .iter()
        .map(|(k, a, b)| (*k, rx(&format!("(?i){}", a)), rx(&format!("(?i){}", b))))
        .collect()
});

/// `_INTENT_RX[k]`: (request pattern, facts pattern) for an intent name.
pub fn intent_rx(name: &str) -> Option<(&'static Regex, &'static Regex)> {
    INTENT_RX
        .iter()
        .find(|(k, _, _)| *k == name)
        .map(|(_, a, b)| (a, b))
}

/// The intents a request matches.
pub fn intents(text: &str) -> Vec<&'static str> {
    INTENT_RX
        .iter()
        .filter(|(_, ask, _)| ask.is_match(text))
        .map(|(k, _, _)| *k)
        .collect()
}

static STOP: LazyLock<HashSet<&'static str>> = LazyLock::new(|| {
    [
        "the", "user", "users", "and", "for", "with", "that", "this", "from", "their", "they",
        "are", "has", "have", "was", "who", "about", "into", "when", "will", "not", "but", "any",
        "all", "can", "you", "your", "our", "his", "her", "帮我", "一下", "我的", "用户", "一个",
        "这个", "那个", "可以", "需要", "看看", "给我", "什么",
    ]
    .into_iter()
    .collect()
});

/// Latin words (3+ chars) and CJK bigrams of a text, lower-cased, stop words dropped.
pub fn tokens(text: &str) -> HashSet<String> {
    static WORD: LazyLock<Regex> = LazyLock::new(|| rx(r"[a-z][a-z0-9.+-]{2,}"));
    static CJK: LazyLock<Regex> = LazyLock::new(|| rx(r"[一-鿿]{2,}"));
    let t = text.to_lowercase();
    let mut out: HashSet<String> = WORD.find_iter(&t).map(|m| m.as_str().to_string()).collect();
    for run in CJK.find_iter(&t) {
        let cs: Vec<char> = run.as_str().chars().collect();
        for w in cs.windows(2) {
            out.insert(w.iter().collect());
        }
    }
    out.retain(|w| !STOP.contains(w.as_str()));
    out
}

/// `context.render`: selected facts grouped by domain, for the prompt.
pub fn render_facts(selected: &[Value], en: bool) -> String {
    if selected.is_empty() {
        return "(nothing on file that bears on this request)".to_string();
    }
    let mut by: HashMap<String, Vec<String>> = HashMap::new();
    for f in selected {
        let mark = if get_str(f, "status") == "pending" {
            " (unconfirmed — learned in an earlier task)"
        } else {
            ""
        };
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

/// Optional arguments of [`Store::add_fact`] (the Python keyword defaults).
#[derive(Debug, Clone)]
pub struct FactOpts {
    pub source: String,
    pub confidence: f64,
    pub ttl_days: Option<i64>,
    pub tier: String,
    pub domain: String,
    pub status: String,
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
        }
    }
}

/// Recent facts and episodes are kept this many days (`RStore.RECENT_DAYS`).
pub const RECENT_DAYS: f64 = 30.0;
/// Facts in these domains name entities (`RStore.ENTITY_DOMAINS`).
pub const ENTITY_DOMAINS: [&str; 4] = ["person", "place", "account", "site"];

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
    /// Open (or create) `<data_dir>/runtime.db`, apply the schema and the column migrations, and give older
    /// facts a domain.
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
        let fcols = cols("facts")?;
        for (col, ddl) in [
            ("tier", "TEXT DEFAULT 'long'"),
            ("uses", "INTEGER DEFAULT 0"),
            ("last_used", "REAL"),
            ("expires_at", "REAL"),
            ("history", "TEXT DEFAULT ''"),
            ("domain", "TEXT DEFAULT ''"),
            ("status", "TEXT DEFAULT 'active'"),
        ] {
            if !fcols.contains(col) {
                db.execute(
                    &format!("ALTER TABLE facts ADD COLUMN {} {}", col, ddl),
                    params![],
                )?;
            }
        }
        let st = Store { db, cfg };
        st.sort_domains()?;
        Ok(st)
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
    /// and its entity synced. `None` for an empty fact.
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
        for r in self.db.all("SELECT id, fact, tier FROM facts", params![])? {
            if norm_fact(&get_str(&r, "fact")) == norm {
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
                self.db
                    .update("facts", "id", get_str(&r, "id"), &Value::Object(upd))?;
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
        let domain: &str = if DOMAIN_KEYS.contains(&o.domain.as_str()) {
            o.domain.as_str()
        } else {
            domain_of(fact, category, entity)
        };
        let status = if o.status == "pending" {
            "pending"
        } else {
            "active"
        };
        self.db.insert(
            "facts",
            &json!({"id": fid, "fact": fact, "category": category, "entity": entity, "source": o.source,
                    "confidence": o.confidence, "created_at": now_ts(), "last_verified": now_ts(),
                    "ttl_days": o.ttl_days, "tier": tier, "uses": 0, "expires_at": exp, "history": "",
                    "domain": domain, "status": status}),
        )?;
        self.db.execute(
            "INSERT INTO facts_fts(id, fact, entity) VALUES (?,?,?)",
            params![fid.as_str(), fact, entity],
        )?;
        self.sync_entity(domain, entity)?;
        Ok(Some(
            json!({"id": fid, "fact": fact, "tier": tier, "domain": domain, "status": status}),
        ))
    }

    pub fn fact(&self, fid: &str) -> Result<Option<Value>> {
        self.db.one("SELECT * FROM facts WHERE id=?", params![fid])
    }

    /// `update_fact(fid, fact=…, category=…, entity=…, tier=…, uses=…, last_used=…, expires_at=…, history=…,
    /// last_verified=…, confidence=…, domain=…, status=…)`; returns the updated row, `None` when unknown.
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
                ]
                .contains(&k.as_str())
                {
                    data.insert(k.clone(), v.clone());
                }
            }
        }
        if let Some(d) = data.get("domain") {
            if !d.is_null() && !d.as_str().is_some_and(|s| DOMAIN_KEYS.contains(&s)) {
                data.remove("domain");
            }
        }
        if let Some(s) = data.get("status") {
            if !s.is_null() && !matches!(s.as_str(), Some("active") | Some("pending")) {
                data.remove("status");
            }
        }
        let r_fact = get_str(&r, "fact");
        let r_cat = get_str(&r, "category");
        let r_ent = get_str(&r, "entity");
        if !data.contains_key("domain")
            && ["fact", "category", "entity"]
                .iter()
                .any(|k| data.contains_key(*k))
        {
            // re-sort only if the domain was the automatic one (a domain the user picked stays)
            let cur = get_str(&r, "domain");
            if cur.is_empty() || cur == domain_of(&r_fact, &r_cat, &r_ent) {
                let nf = data
                    .get("fact")
                    .map(py_str)
                    .unwrap_or_else(|| r_fact.clone());
                let nc = data
                    .get("category")
                    .map(py_str)
                    .unwrap_or_else(|| r_cat.clone());
                let ne = data
                    .get("entity")
                    .map(py_str)
                    .unwrap_or_else(|| r_ent.clone());
                data.insert("domain".into(), json!(domain_of(&nf, &nc, &ne)));
            }
        }
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
    /// Give older facts a domain (one-off, cheap; the user can change it on the Memory page).
    fn sort_domains(&self) -> Result<()> {
        let rows = self.db.all(
            "SELECT id, fact, category, entity FROM facts WHERE COALESCE(domain,'')=''",
            params![],
        )?;
        for r in rows {
            let dom = domain_of(
                &get_str(&r, "fact"),
                &get_str(&r, "category"),
                &get_str(&r, "entity"),
            );
            self.db.execute(
                "UPDATE facts SET domain=? WHERE id=?",
                params![dom, get_str(&r, "id")],
            )?;
            self.sync_entity(dom, &get_str(&r, "entity"))?;
        }
        Ok(())
    }

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

    /// Forget a fact — and the episodes that quote it (2026-10-04 M4-21: after "forget my taxi budget" the agent
    /// still answered "300" from a past-task summary).
    pub fn delete_fact(&self, fid: &str) -> Result<()> {
        let row = self
            .db
            .one("SELECT fact FROM facts WHERE id=?", params![fid])?;
        self.db
            .execute("DELETE FROM facts WHERE id=?", params![fid])?;
        self.db
            .execute("DELETE FROM facts_fts WHERE id=?", params![fid])?;
        if let Some(fact) = row.map(|r| get_str(&r, "fact")).filter(|f| !f.is_empty()) {
            for e in self.db.all("SELECT id, summary FROM episodes", params![])? {
                if mentions_fact(&fact, &get_str(&e, "summary")) {
                    self.db
                        .execute("DELETE FROM episodes WHERE id=?", params![e["id"].clone()])?;
                }
            }
        }
        Ok(())
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
        Ok(rows.into_iter().filter(Self::live).collect())
    }

    /// Facts matching a query: FTS, a LIKE fallback, then (with `fuzzy`) a ranking by shared tokens and topic
    /// (a Chinese query is one long "word").
    pub fn search_facts(
        &self,
        query: &str,
        limit: i64,
        tier: Option<&str>,
        fuzzy: bool,
    ) -> Result<Vec<Value>> {
        let tier = tier.filter(|t| !t.is_empty());
        let terms = split_terms(query);
        let mut rows: Vec<Value> = Vec::new();
        if !terms.is_empty() {
            let q = fts_query(&terms);
            let mut ids: Vec<String> = self
                .db
                .all(
                    "SELECT id FROM facts_fts WHERE facts_fts MATCH ? LIMIT ?",
                    params![q, limit * 3],
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
                p.push(Param::Int(limit * 3));
                ids = self
                    .db
                    .all(&format!("SELECT id FROM facts WHERE {} LIMIT ?", cond), p)?
                    .iter()
                    .map(|r| get_str(r, "id"))
                    .collect();
            }
            for i in ids {
                if let Some(r) = self.fact(&i)? {
                    let rt = {
                        let t = get_str(&r, "tier");
                        if t.is_empty() {
                            "long".to_string()
                        } else {
                            t
                        }
                    };
                    if Self::live(&r) && tier.is_none_or(|t| rt == t) {
                        rows.push(r);
                    }
                }
            }
        }
        if fuzzy && (rows.len() as i64) < limit {
            let qt = tokens(query);
            let its = intents(query);
            let have: HashSet<String> = rows.iter().map(|r| get_str(r, "id")).collect();
            let pool = self.facts(1000, tier)?;
            let mut ranked: Vec<(usize, Value)> = Vec::new();
            for r in pool {
                if have.contains(&get_str(&r, "id")) {
                    continue;
                }
                let txt = format!("{} {}", get_str(&r, "fact"), get_str(&r, "entity"));
                let ft = tokens(&txt);
                let mut sc = qt.intersection(&ft).count();
                for k in &its {
                    if let Some((_, facts_rx)) = intent_rx(k) {
                        if facts_rx.is_match(&txt) {
                            sc += 2;
                        }
                    }
                }
                if sc >= 2 {
                    ranked.push((sc, r));
                }
            }
            ranked.sort_by_key(|a| std::cmp::Reverse(a.0));
            let room = (limit as usize).saturating_sub(rows.len());
            rows.extend(ranked.into_iter().take(room).map(|(_, r)| r));
        }
        rows.truncate(limit.max(0) as usize);
        Ok(rows)
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

    pub fn add_episode(&self, task_id: &str, summary: &str) -> Result<()> {
        self.db.insert(
            "episodes",
            &json!({"ts": now_ts(), "task_id": task_id, "summary": take_chars(summary, 1000)}),
        )?;
        Ok(())
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

    /// Set (or, with a blank value, clear) a profile field. Errors with `unknown profile field` (Python
    /// `ValueError`) for a key [`profile_key`] rejects.
    pub fn set_profile(&self, key: &str, value: &str, source: &str) -> Result<()> {
        let key = profile_key(key);
        if key.is_empty() {
            return Err(anyhow!("unknown profile field"));
        }
        let value = take_chars(value.trim(), 500).to_string();
        if !value.is_empty() {
            self.db.execute(
                "INSERT OR REPLACE INTO profile(key, value, updated_at, source) VALUES (?,?,?,?)",
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
    fn forget_removes_episodes_that_quote_the_fact() {
        let (_d, st) = store();
        let o = FactOpts {
            source: "user-request:t".into(),
            confidence: 0.95,
            ..Default::default()
        };
        let f = st
            .add_fact("每月打车预算 300 新元（SGD）", "preference", "", &o)
            .unwrap()
            .unwrap();
        st.add_episode(
            "t1",
            "[测试] 请记住：我每月打车预算 300 新元 → 已记住：打车 300 新元/月",
        )
        .unwrap();
        st.add_episode("t2", "[测试] 买一双徒步鞋，预算不超过 S$250 → 已筛选 3 双")
            .unwrap();
        st.delete_fact(f["id"].as_str().unwrap()).unwrap();
        let left: Vec<String> = st
            .episodes(10)
            .unwrap()
            .iter()
            .map(|e| get_str(e, "summary"))
            .collect();
        assert_eq!(left.len(), 1);
        assert!(left[0].contains("徒步鞋"));
    }

    #[test]
    fn old_memory_gets_domains_and_chinese_search() {
        let d = tempfile::tempdir().unwrap();
        let st = Store::new(d.path(), cfg("")).unwrap();
        st.add_fact(
            "我身高177，体重76公斤，鞋子42码，运动鞋43码。",
            "preference",
            "",
            &FactOpts::default(),
        )
        .unwrap();
        st.db
            .execute("UPDATE facts SET domain=''", params![])
            .unwrap();
        let st2 = Store::new(d.path(), cfg("")).unwrap(); // restart: older facts are sorted
        assert_eq!(
            st2.facts(500, Some("long")).unwrap()[0]["domain"],
            "preference"
        );
        let hits = st2
            .search_facts("我平时穿多大码的鞋", 5, Some("long"), true)
            .unwrap();
        assert!(!hits.is_empty() && get_str(&hits[0], "fact").contains("43"));
    }

    #[test]
    fn learned_facts_and_domains() {
        let (_d, st) = store();
        let o = FactOpts {
            domain: "site".into(),
            status: "pending".into(),
            ..Default::default()
        };
        let f = st
            .add_fact(
                "decathlon.sg asks for the phone as 8 digits without +65.",
                "habit",
                "decathlon.sg",
                &o,
            )
            .unwrap()
            .unwrap();
        let hit = st
            .search_facts("decathlon phone", 5, Some("long"), true)
            .unwrap()
            .remove(0);
        assert_eq!(hit["status"], "pending");
        assert_eq!(hit["domain"], "site");
        let g = st
            .add_fact(
                "The user wants quiet boutique hotels.",
                "other",
                "",
                &FactOpts::default(),
            )
            .unwrap()
            .unwrap();
        assert_eq!(g["status"], "active");
        assert_eq!(g["domain"], "preference");
        st.update_fact(f["id"].as_str().unwrap(), &json!({"status": "active"}))
            .unwrap();
        assert_eq!(
            st.fact(f["id"].as_str().unwrap()).unwrap().unwrap()["status"],
            "active"
        );
        st.update_fact(
            f["id"].as_str().unwrap(),
            &json!({"status": "bogus", "fact": "decathlon.sg wants 8 digits"}),
        )
        .unwrap();
        let r = st.fact(f["id"].as_str().unwrap()).unwrap().unwrap();
        assert_eq!(r["status"], "active");
        assert!(get_str(&r, "history").contains("without +65"));
        assert_eq!(
            domain_of("Mei collects jazz vinyl records.", "person", "Mei"),
            "person"
        );
        assert_eq!(
            domain_of("Always ask before spending over S$100.", "", ""),
            "rule"
        );
        assert_eq!(
            domain_of("The user's office is at 20 Anson Rd.", "", ""),
            "place"
        );
        assert!(intents("帮我买双鞋").contains(&"shopping"));
        assert!(tokens("The user prefers aisle seats 我喜欢靠过道").contains("prefers"));
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
    fn prune_and_mentions() {
        assert!(mentions_fact(
            "每月打车预算 300 新元",
            "我每月打车预算 300 新元 → 已记住"
        ));
        assert!(!mentions_fact(
            "每月打车预算 300 新元",
            "买一双徒步鞋，预算不超过 S$250"
        ));
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
        assert_eq!(
            render_facts(&[], false),
            "(nothing on file that bears on this request)"
        );
    }
}
