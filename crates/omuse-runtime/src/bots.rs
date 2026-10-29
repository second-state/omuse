//! Bots: the chat split into a main bot and specialised ones. A bot is a set of skills and a short prompt of its own;
//! every bot shares the memory, the profile, the Vault and the connectors. Each conversation belongs to one bot
//! (`conversations.bot_id`, `main` by default).
//!
//! The bots are built in (`bots/<id>.md`, compiled into the binary): front matter with the name (English and Chinese),
//! an icon, the order in the menu, a description the main bot routes by, and the skills; the body is the bot's prompt.
//! The main bot has every skill no specialised bot lists (imported skills included) plus the ones its own file names;
//! specialised bots may share skills. Tools work the same way: a tool a bot file names under `tools:` (the travel
//! searches) is only for the bots that name it, so the main bot cannot answer such a request itself; a connector's
//! tool `mcp_<server>__<name>` counts as `<name>`. The main bot hands a request to a specialised bot with `ask_bot`
//! (`Runtime::ask_bot`).

use std::collections::HashSet;
use std::sync::LazyLock;

use include_dir::{include_dir, Dir};
use omuse_common::util::get_str;
use regex::Regex;
use serde_json::{json, Value};

/// The built-in bots (`bots/*.md`), compiled in.
static BUILTIN: Dir<'static> = include_dir!("$CARGO_MANIFEST_DIR/../../bots");

/// The id of the main bot, the one every conversation belongs to unless it says otherwise.
pub const MAIN: &str = "main";

/// One bot.
#[derive(Debug, Clone, PartialEq)]
pub struct Bot {
    pub id: String,
    pub name: String,
    pub name_zh: String,
    pub icon: String,
    pub order: i64,
    pub description: String,
    pub description_zh: String,
    /// The skills its file names (for the main bot: the shared ones it keeps besides every unlisted skill).
    pub skills: Vec<String>,
    /// The tools only the bots that name them may use.
    pub tools: Vec<String>,
    /// Requests it handles, shown above the chat input of an empty conversation (`examples` / `examples_zh`, `|`
    /// between them).
    pub examples: Vec<String>,
    pub examples_zh: Vec<String>,
    /// The bot's own instructions, added to the executor prompt.
    pub prompt: String,
}

impl Bot {
    pub fn is_main(&self) -> bool {
        self.id == MAIN
    }

    /// The name in the agent's language.
    pub fn label(&self, en: bool) -> &str {
        if en || self.name_zh.is_empty() {
            &self.name
        } else {
            &self.name_zh
        }
    }

    /// The API view (`GET /api/bots`).
    pub fn to_json(&self) -> Value {
        json!({"id": self.id, "name": self.name, "name_zh": self.name_zh, "icon": self.icon, "order": self.order,
               "description": self.description, "description_zh": self.description_zh, "skills": self.skills,
               "examples": self.examples, "examples_zh": self.examples_zh})
    }
}

/// A bot file: `--- key: value … ---` then the prompt.
pub fn parse(id: &str, text: &str) -> Option<Bot> {
    static FM: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?s)^---\s*\n(.*?)\n---\s*\n?(.*)$").unwrap());
    let text = text.replace("\r\n", "\n");
    let m = FM.captures(&text)?;
    let field = |k: &str| {
        m[1].lines()
            .filter_map(|l| l.split_once(':'))
            .find(|(key, _)| key.trim() == k)
            .map(|(_, v)| v.trim().to_string())
            .unwrap_or_default()
    };
    let name = field("name");
    if name.is_empty() {
        return None;
    }
    Some(Bot {
        id: id.to_string(),
        name,
        name_zh: field("name_zh"),
        icon: field("icon"),
        order: field("order").parse().unwrap_or(99),
        description: field("description"),
        description_zh: field("description_zh"),
        skills: list(&field("skills")),
        tools: list(&field("tools")),
        examples: split(&field("examples"), '|'),
        examples_zh: split(&field("examples_zh"), '|'),
        prompt: m[2].trim().to_string(),
    })
}

fn list(v: &str) -> Vec<String> {
    split(v, ',')
}

fn split(v: &str, sep: char) -> Vec<String> {
    v.split(sep)
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

static BOTS: LazyLock<Vec<Bot>> = LazyLock::new(|| {
    let mut out: Vec<Bot> = BUILTIN
        .files()
        .filter_map(|f| {
            let id = f.path().file_stem()?.to_str()?.to_string();
            parse(&id, f.contents_utf8()?)
        })
        .collect();
    out.sort_by(|a, b| (a.order, &a.id).cmp(&(b.order, &b.id)));
    out
});

/// Every bot, the main one first.
pub fn all() -> &'static [Bot] {
    &BOTS
}

/// The bot with this id.
pub fn get(id: &str) -> Option<&'static Bot> {
    BOTS.iter().find(|b| b.id == id)
}

/// The bot with this id, or the main bot for an empty or unknown id (a conversation saved before bots existed).
pub fn get_or_main(id: &str) -> &'static Bot {
    get(id)
        .or_else(|| get(MAIN))
        .expect("bots/main.md is built in")
}

/// Whether `bot` may use the skill `name`: a specialised bot its own list; the main bot its list and every skill no
/// specialised bot claims.
pub fn has_skill(bot: &Bot, name: &str) -> bool {
    if bot.skills.iter().any(|s| s == name) {
        return true;
    }
    bot.is_main()
        && !BOTS
            .iter()
            .any(|b| !b.is_main() && b.skills.iter().any(|s| s == name))
}

/// The tool's name without a connector's `mcp_<server>__` prefix.
fn base_tool(name: &str) -> &str {
    name.strip_prefix("mcp_")
        .and_then(|n| n.split_once("__"))
        .map_or(name, |(_, n)| n)
}

/// Whether `bot` may use the tool `name`: every tool except the ones other bots' files claim under `tools:`.
pub fn has_tool(bot: &Bot, name: &str) -> bool {
    let n = base_tool(name);
    bot.tools.iter().any(|t| t == n) || !BOTS.iter().any(|b| b.tools.iter().any(|t| t == n))
}

/// The bots whose files claim the tool `name` (for the refusal).
pub fn tool_owners(name: &str) -> Vec<&'static Bot> {
    let n = base_tool(name);
    BOTS.iter()
        .filter(|b| b.tools.iter().any(|t| t == n))
        .collect()
}

/// The enabled skills (as `Skills::enabled` lists them) that `bot` may use.
pub fn filter_skills(bot: &Bot, skills: Vec<Value>) -> Vec<Value> {
    skills
        .into_iter()
        .filter(|s| has_skill(bot, &get_str(s, "name")))
        .collect()
}

/// The specialised bots that have the skill `name` (for the refusal when another bot's skill is asked for).
pub fn owners(name: &str) -> Vec<&'static Bot> {
    BOTS.iter()
        .filter(|b| !b.is_main() && b.skills.iter().any(|s| s == name))
        .collect()
}

/// Skill names the bot files name that are not built-in skills (a unit test keeps this empty).
pub fn unknown_skills(builtin: &HashSet<String>) -> Vec<String> {
    BOTS.iter()
        .flat_map(|b| b.skills.iter())
        .filter(|s| !builtin.contains(*s))
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn travel_searches_are_the_travel_bots_only() {
        let (main, travel, insurance, cities) = (
            get(MAIN).unwrap(),
            get("travel").unwrap(),
            get("insurance").unwrap(),
            get("cities").unwrap(),
        );
        for t in [
            "flights_search",
            "flight_offer",
            "hotels_search",
            "hotel_rooms",
        ] {
            assert!(!has_tool(main, t), "{t}");
            assert!(has_tool(travel, t), "{t}");
            assert!(!has_tool(main, &format!("mcp_travel__{t}")), "{t}");
            assert!(has_tool(travel, &format!("mcp_travel__{t}")), "{t}");
        }
        assert!(has_tool(insurance, "flights_search") && !has_tool(insurance, "hotels_search"));
        assert!(!has_tool(cities, "flights_search"));
        for t in [
            "web_search",
            "places_search",
            "calculate",
            "ask_bot",
            "mcp_x__web_search",
            "mcp_flights_search",
        ] {
            assert!(has_tool(main, t), "{t}");
        }
        let owners: Vec<&str> = tool_owners("flights_search")
            .iter()
            .map(|b| b.id.as_str())
            .collect();
        assert_eq!(owners, ["travel", "insurance"]);
    }

    #[test]
    fn built_in_bots_parse_with_main_first_and_known_skills() {
        let ids: Vec<&str> = all().iter().map(|b| b.id.as_str()).collect();
        assert_eq!(
            ids,
            [
                "main",
                "travel",
                "cities",
                "arrival",
                "insurance",
                "cards",
                "photos"
            ]
        );
        for b in all() {
            assert!(
                !b.name.is_empty() && !b.name_zh.is_empty() && !b.icon.is_empty(),
                "{}",
                b.id
            );
            assert!(
                !b.description.is_empty() && !b.prompt.is_empty(),
                "{}",
                b.id
            );
            // examples above the chat input: a few, in both languages, one for one
            assert!(
                (3..=5).contains(&b.examples.len()) && b.examples.len() == b.examples_zh.len(),
                "{}: {:?} / {:?}",
                b.id,
                b.examples,
                b.examples_zh
            );
        }
        let builtin: HashSet<String> = crate::skills::BUILTIN
            .dirs()
            .filter_map(|d| {
                d.path()
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
            })
            .collect();
        assert!(
            unknown_skills(&builtin).is_empty(),
            "{:?}",
            unknown_skills(&builtin)
        );
        // every built-in skill is reachable from some bot
        for s in &builtin {
            assert!(all().iter().any(|b| has_skill(b, s)), "{s}");
        }
    }

    #[test]
    fn main_keeps_unclaimed_skills_and_shares_listed_ones() {
        let main = get_or_main("");
        assert!(main.is_main() && get_or_main("nope").is_main());
        let travel = get("travel").unwrap();
        assert!(has_skill(travel, "flight-booking") && !has_skill(main, "flight-booking"));
        assert!(has_skill(main, "news-search") && !has_skill(travel, "news-search"));
        // an imported skill no bot names belongs to main
        assert!(has_skill(main, "my-imported-skill") && !has_skill(travel, "my-imported-skill"));
        // shared: listed by a specialised bot and by main
        assert!(has_skill(main, "pdf-forms") && has_skill(get("arrival").unwrap(), "pdf-forms"));
        assert!(
            has_skill(get("cities").unwrap(), "michelin-dining")
                && has_skill(travel, "michelin-dining")
        );
        let cards = get("cards").unwrap();
        assert!(has_skill(cards, "credit-card-picker") && !has_skill(main, "credit-card-picker"));
        assert!(
            has_skill(cards, "csr-trip-claim")
                && has_skill(get("insurance").unwrap(), "csr-trip-claim")
        );
        let photos = get("photos").unwrap();
        assert!(has_skill(photos, "drive-video-edit") && !has_skill(main, "drive-video-edit"));
        assert_eq!(owners("pdf-forms").len(), 2);
        let skills = vec![
            json!({"name": "flight-booking"}),
            json!({"name": "news-search"}),
        ];
        assert_eq!(filter_skills(travel, skills.clone()).len(), 1);
        assert_eq!(
            get_str(&filter_skills(main, skills)[0], "name"),
            "news-search"
        );
    }

    #[test]
    fn parse_reads_front_matter_and_prompt() {
        let b = parse(
            "x",
            "---\nname: X\nicon: 🧪\norder: 3\nskills: a, b ,\n---\nDo things.\n",
        )
        .unwrap();
        assert_eq!(
            (
                b.name.as_str(),
                b.order,
                b.skills.clone(),
                b.prompt.as_str()
            ),
            ("X", 3, vec!["a".to_string(), "b".to_string()], "Do things.")
        );
        assert!(parse("y", "no front matter").is_none());
    }
}
