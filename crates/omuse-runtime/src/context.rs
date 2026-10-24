//! Personal context v1 (roadmap batch 2): long-term memory sorted into domains, and the few facts that matter for a
//! request handed to the agent at the start of the task, so it does not ask the user again for things it already knows
//! (shoe size, cabin class, preferred cuisine, which mailbox to send from …).
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
//! Selection is deterministic (no model call): what the request is about (shopping, dining, travel, email …), names and
//! sites it mentions (with aliases: 迪卡侬 = Decathlon = decathlon.sg), and shared words. Facts learned by OMuse itself
//! (not stated by the user) are "pending" until the user confirms them on the Memory page; they are still used, marked
//! "unconfirmed".
//!
//! The pieces the store itself needs (domains, classification, request intents, tokens, the prompt rendering) live in
//! [`crate::store`] and are re-exported here under their Python names.

use std::sync::LazyLock;

use omuse_common::util::{as_f64, get_str, py_str};
use regex::Regex;
use serde_json::{Map, Value};

pub use crate::store::{
    domain_label, domain_of, intent_rx, intents, render_facts as render, tokens, DOMAINS,
    DOMAIN_KEYS, INTENTS, USE_RULE,
};

fn rx(p: &str) -> Regex {
    Regex::new(p).unwrap()
}

static EMAIL_ADDR: LazyLock<Regex> = LazyLock::new(|| rx(r"[\w.+-]+@[\w-]+(\.[\w-]+)+"));

/// Merchants and services people name in different ways.
pub const ALIASES: [&[&str]; 14] = [
    &["decathlon", "迪卡侬", "decathlon.sg"],
    &["amazon", "亚马逊", "amazon.sg"],
    &["shopee", "虾皮"],
    &["lazada", "来赞达"],
    &["taobao", "淘宝"],
    &["jd.com", "京东"],
    &[
        "singapore airlines",
        "新航",
        "新加坡航空",
        "singaporeair",
        "krisflyer",
    ],
    &["scoot", "酷航"],
    &["grab"],
    &["chope"],
    &["notion"],
    &["uniqlo", "优衣库"],
    &["ikea", "宜家"],
    &["fairprice", "职总平价"],
];

fn alias_groups(entities: &[Value]) -> Vec<Vec<String>> {
    let mut groups: Vec<Vec<String>> = ALIASES
        .iter()
        .map(|g| g.iter().map(|s| s.to_string()).collect())
        .collect();
    for e in entities {
        let mut names: Vec<String> = vec![get_str(e, "name")];
        if let Some(al) = e
            .get("attrs")
            .and_then(|a| a.get("aliases"))
            .and_then(Value::as_array)
        {
            names.extend(al.iter().map(|a| match a {
                Value::String(s) => s.clone(),
                other => py_str(other),
            }));
        }
        names.push(get_str(e, "relation"));
        let names: Vec<String> = names
            .iter()
            .filter(|n| !n.is_empty() && n.trim().chars().count() >= 2)
            .map(|n| n.trim().to_string())
            .collect();
        if !names.is_empty() {
            groups.push(names);
        }
    }
    groups
}

static HOST: LazyLock<Regex> =
    LazyLock::new(|| rx(r"(?i)\b([a-z0-9-]{3,})\.(?:[a-z]{2,6})(?:\.[a-z]{2})?\b"));

/// Alias groups the text mentions (迪卡侬 → [decathlon, 迪卡侬, decathlon.sg]); a website named by its address
/// counts too (shop.test → [shop.test, shop]).
pub fn named(text: &str, entities: Option<&[Value]>) -> Vec<Vec<String>> {
    let low = text.to_lowercase();
    let mut out: Vec<Vec<String>> = alias_groups(entities.unwrap_or(&[]))
        .into_iter()
        .filter(|g| g.iter().any(|a| low.contains(&a.to_lowercase())))
        .collect();
    let plain = EMAIL_ADDR.replace_all(&low, " ");
    for m in HOST.captures_iter(&plain) {
        let whole = m.get(0).map(|x| x.as_str()).unwrap_or("");
        let name = m.get(1).map(|x| x.as_str()).unwrap_or("");
        if !out
            .iter()
            .any(|g| g.iter().any(|a| a.to_lowercase().contains(name)))
        {
            let mut g = vec![whole.to_string()];
            if name.chars().count() >= 5 {
                g.push(name.to_string());
            }
            out.push(g);
        }
    }
    out
}

fn mentions(fact_text: &str, group: &[String]) -> bool {
    let low = fact_text.to_lowercase();
    group.iter().any(|a| low.contains(&a.to_lowercase()))
}

/// Identity documents (passport number / expiry, ID card, date of birth …): only when the request needs them filled in.
pub static ID_DOC: LazyLock<Regex> = LazyLock::new(|| {
    rx(
        r"(?i)护照|身份证|出生日期|生日|签发|有效期至|证件号|\b(passport|date of birth|DOB|born on|national id|NRIC|FIN|id (card|number)|identity card|expir(y|es|ation))\b",
    )
});
static NEEDS_ID: LazyLock<Regex> = LazyLock::new(|| {
    rx(
        r"(?i)填写|填表|表格|表单|申请|值机|登机|旅客信息|乘客信息|入住登记|护照.{0,6}(到期|过期|有效|续)|证件|\b(form|apply|application|check-?in|passenger (details|info)|traveller details|renew)\b",
    )
});

/// Does the request need identity documents filled in (a form, a check-in, a renewal)?
pub fn needs_id(request: &str) -> bool {
    NEEDS_ID.is_match(request)
}

static TRIGGER: LazyLock<Regex> =
    LazyLock::new(|| rx(r#"(?:说|says?|ask(?:s)? for)\s*[「“"']([^」”"']{2,20})[」”"']"#));
/// A rule that applies to every task (how the assistant should behave in general).
pub static GENERAL_RULE: LazyLock<Regex> =
    LazyLock::new(|| rx(r"(?i)the assistant|OMuse|助手|autonomous|自主|直接执行"));

fn with_extra(f: &Value, dom: &str, score: f64, why: &str) -> Value {
    let mut m: Map<String, Value> = f.as_object().cloned().unwrap_or_default();
    m.insert("domain".into(), Value::String(dom.to_string()));
    m.insert("score".into(), Value::from(score));
    m.insert("why".into(), Value::String(why.to_string()));
    Value::Object(m)
}

fn domain_for(f: &Value) -> String {
    let d = get_str(f, "domain");
    if !d.is_empty() {
        return d;
    }
    domain_of(
        &get_str(f, "fact"),
        &get_str(f, "category"),
        &get_str(f, "entity"),
    )
    .to_string()
}

/// The facts that matter for this request, best first. Each returned fact gets `score` and `why`.
/// `context` = the recent conversation (a follow-up like "好，就买那双" carries little on its own).
pub fn select(
    request: &str,
    facts: &[Value],
    entities: Option<&[Value]>,
    limit: usize,
    context: &str,
) -> Vec<Value> {
    let req = request;
    let whole = if context.is_empty() {
        req.to_string()
    } else {
        format!("{}\n{}", req, context)
    };
    let mut its = intents(req);
    if its.is_empty() {
        its = intents(context);
    }
    let groups = named(&whole, entities);
    let rtoks = tokens(req);
    let want_id = needs_id(req);
    let mut scored: Vec<Value> = Vec::new();
    for f in facts {
        let fact = get_str(f, "fact");
        let entity = get_str(f, "entity");
        let text = format!("{} {}", fact, entity);
        if !want_id && ID_DOC.is_match(&fact) {
            continue; // passport / ID / birth date stay out of answers unless a form needs them
        }
        let dom = domain_for(f);
        let mut s = 0.0_f64;
        let mut why: Vec<String> = Vec::new();
        let trig: Vec<String> = TRIGGER
            .captures_iter(&fact)
            .map(|c| c[1].to_string())
            .collect();
        if dom == "rule" && !trig.is_empty() {
            // "以后每次说「准备出差」…": only when the user says it
            if trig
                .iter()
                .any(|t| !t.is_empty() && req.contains(t.as_str()))
            {
                scored.push(with_extra(f, &dom, 10.0, &format!("触发词：{}", trig[0])));
            }
            continue;
        }
        let g: Vec<&Vec<String>> = groups.iter().filter(|x| mentions(&text, x)).collect();
        if !g.is_empty() {
            s += 6.0;
            why.push(format!("提到 {}", g[0][0]));
        }
        let hits: Vec<&str> = its
            .iter()
            .copied()
            .filter(|k| {
                intent_rx(k)
                    .map(|(_, b)| b.is_match(&text))
                    .unwrap_or(false)
            })
            .collect();
        if !hits.is_empty() {
            s += 3.0 + std::cmp::min(2, hits.len() - 1) as f64;
            why.push(format!("相关：{}", hits.join("/")));
            if dom == "site" && g.is_empty() {
                s -= 2.0; // a site habit matters when that site is in play
            }
        }
        let ftoks = tokens(&text);
        let mut shared: Vec<&String> = rtoks.intersection(&ftoks).collect();
        if !shared.is_empty() {
            s += std::cmp::min(3, shared.len()) as f64;
            if why.is_empty() {
                shared.sort();
                let top: Vec<&str> = shared.iter().take(3).map(|x| x.as_str()).collect();
                why.push(format!("词：{}", top.join(",")));
            }
        }
        if dom == "rule" && GENERAL_RULE.is_match(&text) {
            s += 3.0;
            why.push("通用规则".to_string());
        }
        if s >= 3.0 {
            let uses = f.get("uses").and_then(as_f64).unwrap_or(0.0);
            scored.push(with_extra(
                f,
                &dom,
                s + (uses / 50.0).min(1.0),
                &why.join("；"),
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

/// `'https://www.decathlon.sg/p/123'` → `'decathlon'`.
pub fn site_name(url: &str) -> String {
    static SCHEME_HOST: LazyLock<Regex> = LazyLock::new(|| rx(r"^[a-z]+://([^/:?#]+)"));
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

/// Site habits (and other facts) about the website at `url`.
pub fn site_facts(url: &str, facts: &[Value]) -> Vec<Value> {
    let name = site_name(url);
    if name.chars().count() < 3 {
        return vec![];
    }
    let group: Vec<String> = ALIASES
        .iter()
        .find(|g| {
            g.iter().any(|a| {
                a.to_lowercase()
                    .split('.')
                    .next()
                    .map(|p| p == name)
                    .unwrap_or(false)
            })
        })
        .map(|g| g.iter().map(|s| s.to_string()).collect())
        .unwrap_or_else(|| vec![name.clone()]);
    let mut out: Vec<Value> = Vec::new();
    for f in facts {
        let text = format!("{} {}", get_str(f, "fact"), get_str(f, "entity")).to_lowercase();
        if group.iter().any(|a| text.contains(&a.to_lowercase())) {
            let dom = domain_for(f);
            if ["site", "account", "rule", "preference"].contains(&dom.as_str()) {
                let mut m: Map<String, Value> = f.as_object().cloned().unwrap_or_default();
                m.insert("domain".into(), Value::String(dom));
                out.push(Value::Object(m));
            }
        }
    }
    out.sort_by(|a, b| {
        let ka = (
            get_str(a, "domain") != "site",
            -a.get("uses").and_then(as_f64).unwrap_or(0.0),
        );
        let kb = (
            get_str(b, "domain") != "site",
            -b.get("uses").and_then(as_f64).unwrap_or(0.0),
        );
        ka.0.cmp(&kb.0)
            .then(ka.1.partial_cmp(&kb.1).unwrap_or(std::cmp::Ordering::Equal))
    });
    out.truncate(5);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// `tests/fixtures/memory_like.py`: facts shaped like a real user's memory (invented wording).
    const FACTS: [(&str, &str, &str); 39] = [
        ("other", "", "decathlon.sg 网站支持访客结账，无需登录。结账时选「送货到家 (Home delivery)」会自动带出已保存的地址；电话号码填 8 位，不要 +65。"),
        ("preference", "", "我身高177，体重76公斤，鞋子42码，运动鞋43码。"),
        ("other", "", "用户要求：以后每次说「准备出差」，按固定流程执行——查邮件里的航班和酒店、查目的地天气、查日历冲突、生成一页行程 PDF。"),
        ("price-monitoring", "Olares One 价格监控", "Olares One price baseline (2026-10-05): single unit $4,999 USD."),
        ("preference", "hiking", "The user prefers hiking with medium-low intensity and a duration of 3 to 5 hours."),
        ("preference", "", "我美国手机号码是 +1-669-000-0000"),
        ("preference", "Parker pen", "The user prefers metal pens with an aesthetic design."),
        ("preference", "AI", "The user is interested in news regarding AI and robotics."),
        ("preference", "unsubscribes", "The user prefers to review and select unsubscribes individually rather than using bulk actions."),
        ("other", "email accounts", "The user manages four connected email accounts."),
        ("preference", "", "The user operates in the Singapore Time Zone (SGT)."),
        ("company", "Ogena Solutions", "The user works with or receives communications from Ogena Solutions."),
        ("company", "lucas@example.io", "The user manages the email account lucas@example.io."),
        ("preference", "", "The user prefers business class seats."),
        ("person", "LU A, LU B", "The user travels with LU A and LU B."),
        ("preference", "Hunan cuisine", "The user prefers dining at Hunan cuisine restaurants."),
        ("preference", "OtterBox Defender", "The user prefers aesthetically pleasing phone cases, specifically the OtterBox Defender series."),
        ("preference", "iPhone 17 Pro Max", "The user uses an iPhone 17 Pro Max."),
        ("project", "Olares One", "The user is working on the Olares One PCBA project."),
        ("company", "BEC Lab", "The user works at BEC Lab, located at #10-01 20 Anson Rd, Singapore 079912."),
        ("habit", "", "The user travels with a group of three other people."),
        ("preference", "", "The user prefers to sit together with their travel companions, ideally in the same row."),
        ("preference", "", "The user prefers to avoid leaving personal information or registering accounts during online shopping."),
        ("preference", "email management", "The user prefers to mark non-important emails as read."),
        ("other", "Amazon Singapore", "The user is interested in purchasing an iPhone 17 Pro Max case from Amazon Singapore."),
        ("habit", "Amazon Singapore", "The user shops on Amazon Singapore (amazon.sg)."),
        ("company", "XIANGXI HOMETOWN", "The user is interested in dining at the restaurant 'XIANGXI HOMETOWN' in Singapore."),
        ("preference", "", "The user prefers the assistant to execute tasks autonomously and directly, without requiring confirmation for each step."),
        ("preference", "", "For travel, the user prefers direct flights and quiet boutique hotels."),
        ("person", "Frank, Daniel", "The user collaborates with colleagues named Frank, Daniel and Michael."),
        ("preference", "", "The user prefers receiving deliverables in PDF format."),
        ("preference", "", "The user prefers receiving reports in Chinese."),
        ("preference", "Japanese cuisine", "The user enjoys dining at Japanese restaurants."),
        ("other", "Singapore", "The user lives in Singapore."),
        ("preference", "Chope", "The user prefers to use Chope or restaurant official websites for checking availability."),
        ("preference", "example.io", "The user prefers sending emails from their example.io address rather than the gmail one."),
        ("person", "James", "The user works with James Zhan on business matters."),
        ("habit", "Notion", "The user uses Notion."),
        ("company", "Clapper", "The user works at or manages the company Clapper and approves PRDs."),
    ];

    /// `tests/fixtures/ctx_requests.py`: request, regex a useful fact must match.
    const REQUESTS: [(&str, &str, &str); 10] = [
        (
            "P01",
            "帮我在迪卡侬挑一双适合日常跑步的跑鞋，选好尺码加入购物车就行，先不用下单。",
            r"43码|运动鞋43",
        ),
        (
            "P02",
            "帮我网上挑一个好看的手机壳，给我 3 个候选，先不用下单。",
            r"iPhone 17 Pro Max",
        ),
        (
            "P03",
            "帮我找下个月从新加坡飞东京的机票，给我两三个选择，先不要订。",
            r"business class|商务舱",
        ),
        (
            "P04",
            "帮我在东京找一家酒店，下个月住 3 晚，给几个选项，先不要订。",
            r"boutique|精品",
        ),
        (
            "P05",
            "帮我找个这周末在新加坡的徒步路线。",
            r"medium-low|3 to 5",
        ),
        (
            "P06",
            "这周六晚上想出去吃饭，帮我找两家餐厅看看有没有空位，先别订。",
            r"Hunan|Japanese|湘|日本",
        ),
        (
            "P07",
            "帮我在迪卡侬买一件运动T恤，挑合适的尺码加入购物车，先不用下单。",
            r"身高177|177",
        ),
        (
            "P08",
            "给 James 写封邮件约他下周二下午开会，先存草稿就行。",
            r"sending emails from|James",
        ),
        (
            "P09",
            "帮我整理一份本周 AI 和机器人领域的新闻简报。",
            r"PDF|Chinese|中文",
        ),
        (
            "P10",
            "帮我挑一支笔送朋友，给我 3 个推荐。",
            r"metal pens|金属",
        ),
    ];

    fn ctx_facts() -> Vec<Value> {
        FACTS
            .iter()
            .enumerate()
            .map(|(i, (c, e, t))| {
                json!({"id": format!("f{}", i), "category": c, "entity": e, "fact": t, "uses": 0,
                       "status": "active", "domain": domain_of(t, c, e)})
            })
            .collect()
    }

    #[test]
    fn memory_is_sorted_into_domains() {
        assert_eq!(
            domain_of(
                "decathlon.sg 网站支持访客结账，电话号码填 8 位，不要 +65。",
                "",
                ""
            ),
            "site"
        );
        assert_eq!(
            domain_of("我身高177，体重76公斤，运动鞋43码。", "preference", ""),
            "preference"
        );
        assert_eq!(
            domain_of("用户要求：以后每次说「准备出差」，按固定流程执行", "", ""),
            "rule"
        );
        assert_eq!(
            domain_of(
                "The user prefers the assistant to execute tasks autonomously.",
                "preference",
                ""
            ),
            "rule"
        );
        assert_eq!(
            domain_of("The user travels with LU A and LU B.", "person", ""),
            "person"
        );
        assert_eq!(
            domain_of(
                "The user works at BEC Lab, located at 20 Anson Rd.",
                "company",
                ""
            ),
            "place"
        );
        assert_eq!(
            domain_of(
                "The user manages the email account lucas@example.io.",
                "company",
                ""
            ),
            "account"
        );
        assert_eq!(
            domain_of("我美国手机号码是 +1-669-000-0000", "preference", ""),
            "account"
        );
        assert_eq!(
            domain_of(
                "The user prefers sending emails from their example.io address.",
                "preference",
                ""
            ),
            "preference"
        );
        assert_eq!(
            domain_of(
                "The user is working on the Olares One PCBA project.",
                "project",
                ""
            ),
            "work"
        );
        assert_eq!(domain_of("anything", "site", ""), "site"); // a domain the user picked stays
    }

    #[test]
    fn context_picks_what_the_request_needs() {
        let facts = ctx_facts();
        for (pid, req, pat) in REQUESTS {
            let sel = select(req, &facts, None, 12, "");
            let r = Regex::new(&format!("(?i){}", pat)).unwrap();
            assert!(
                sel.iter().any(|f| r.is_match(&get_str(f, "fact"))),
                "{} {:?}",
                pid,
                sel.iter().map(|f| get_str(f, "fact")).collect::<Vec<_>>()
            );
            assert!(sel.len() <= 12);
        }
        let shoes = select(REQUESTS[0].1, &facts, None, 12, "")
            .iter()
            .map(|f| get_str(f, "fact"))
            .collect::<Vec<_>>()
            .join(" | ");
        assert!(shoes.contains("decathlon.sg") && shoes.contains("43")); // the shop's habits + the size
        assert!(
            !shoes.contains("metal pens")
                && !shoes.contains("phone cases")
                && !shoes.contains("business class")
        );
        // a rule with a trigger phrase only when the user says it
        assert!(!select("帮我订下个月去东京的机票", &facts, None, 12, "")
            .iter()
            .any(|f| get_str(f, "fact").contains("准备出差")));
        assert!(get_str(
            &select("准备出差，下周去东京", &facts, None, 12, "")[0],
            "fact"
        )
        .starts_with("用户要求"));
        let txt = render(&select(REQUESTS[0].1, &facts, None, 12, ""), false);
        assert!(txt.contains("[网站习惯 Site habits]") && txt.contains("[偏好 Preferences]"));
    }

    #[test]
    fn aliases_relations_and_sites() {
        let facts = vec![
            json!({"id": "a", "fact": "Mei collects jazz vinyl records.", "category": "person", "entity": "Mei", "uses": 0}),
            json!({"id": "b", "fact": "The user likes jazz.", "category": "preference", "entity": "", "uses": 0}),
        ];
        let ents = vec![json!({"name": "Mei", "type": "person", "attrs": {}, "relation": ""})];
        assert!(
            !select("帮我给太太挑个生日礼物", &facts, Some(&ents), 12, "")
                .iter()
                .any(|f| get_str(f, "fact").contains("Mei"))
        );
        let ents = vec![
            json!({"name": "Mei", "type": "person", "attrs": {"aliases": ["梅梅"]}, "relation": "太太"}),
        ];
        let sel = select("帮我给太太挑个生日礼物", &facts, Some(&ents), 12, "");
        assert!(
            !sel.is_empty()
                && get_str(&sel[0], "fact").contains("Mei")
                && get_str(&sel[0], "why").contains("提到")
        );
        assert_eq!(site_name("https://www.decathlon.sg/p/123"), "decathlon");
        assert_eq!(site_name("https://www.amazon.com.sg/x"), "amazon");
        assert_eq!(site_name("https://shop.test/"), "shop");
        assert_eq!(site_name("not a url"), "");
        let facts = vec![
            json!({"id": "a", "fact": "decathlon.sg 支持访客结账；电话填 8 位，不要 +65。", "category": "other", "entity": "", "uses": 0}),
            json!({"id": "b", "fact": "The user likes jazz.", "category": "preference", "entity": "", "uses": 0}),
        ];
        let found = site_facts("https://www.decathlon.sg/checkout", &facts);
        assert_eq!(found.len(), 1);
        assert!(get_str(&found[0], "fact").contains("8 位"));
        assert_eq!(get_str(&found[0], "domain"), "site");
        assert!(site_facts("https://www.ikea.com/sg", &facts).is_empty());
        let groups = named("在 shop.test 买双袜子，迪卡侬也看看", None);
        assert!(groups.iter().any(|g| g[0] == "decathlon"));
        assert!(groups.iter().any(|g| g[0] == "shop.test"));
        assert!(needs_id("帮我填表申请签证") && !needs_id("买双鞋"));
    }
}
