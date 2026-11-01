//! The one-time fill of the profile's place fields (`home_city`, `home_country`, `address_country:<label>`) for
//! addresses saved without them (a profile from before 0.6.0, an address typed in without its country): the model
//! reads the addresses (natural language) and answers JSON with codes of the tables; code validates every code
//! (`places::clean_value`) and saves only those. Code never reads an address's words itself.

use std::collections::HashSet;
use std::sync::LazyLock;

use omuse_common::util::{get_str, take_chars};
use parking_lot::Mutex;
use serde_json::{json, Map, Value};

use super::runtime::opts;
use super::Runtime;
use crate::llm::{extract_json, ChatOpts};
use crate::places;

/// The system prompt of the fill.
pub const PLACES_SYSTEM: &str = "You read the user's own addresses for a program and give the place of each as codes. \
You get the addresses as JSON ({\"<key>\": \"<address>\"}) and the allowed city codes and country codes. Answer with JSON \
only: {\"home_city\": \"<a listed city code, or empty when the home address is in none of the listed cities>\", \
\"countries\": {\"<key>\": \"<a listed ISO country code, or empty when unsure>\"}} — one entry per address key.";

/// The fields to save from the model's answer: `(profile key, code)` for each valid code, for the keys asked only.
pub fn place_fields(answer: &Value, asked: &Value, home_city_asked: bool) -> Vec<(String, String)> {
    let mut out = Vec::new();
    if home_city_asked {
        if let Some(code) = places::clean_value("home_city", &get_str(answer, "home_city")) {
            if !code.is_empty() {
                out.push(("home_city".to_string(), code));
            }
        }
    }
    let Some(countries) = answer.get("countries").and_then(Value::as_object) else {
        return out;
    };
    for (k, v) in countries {
        if asked.get(k).is_none() {
            continue;
        }
        let ck = places::country_key(k);
        let Some(iso) = v
            .as_str()
            .and_then(|v| places::clean_value(&ck, v))
            .filter(|c| !c.is_empty())
        else {
            continue;
        };
        out.push((ck, iso));
    }
    out
}

/// What the fill was already asked (the addresses as they were), so it runs once per address.
static TRIED: LazyLock<Mutex<HashSet<String>>> = LazyLock::new(|| Mutex::new(HashSet::new()));

impl Runtime {
    /// Fill the profile's missing place fields once (see the module doc). Called before a workflow tool's arguments
    /// are checked, so the machines read the codes.
    pub(crate) async fn fill_places(&self, task_id: &str) {
        let profile = self.store.profile().unwrap_or_else(|_| json!({}));
        let mut asked: Map<String, Value> = places::addresses_without_country(&profile)
            .as_object()
            .cloned()
            .unwrap_or_default();
        let home = get_str(&profile, "address_home");
        let city_asked = !home.trim().is_empty() && get_str(&profile, "home_city").is_empty();
        if city_asked {
            asked.insert("address_home".into(), json!(home));
        }
        if asked.is_empty() {
            return;
        }
        let asked = Value::Object(asked);
        if !TRIED.lock().insert(asked.to_string()) {
            return;
        }
        let codes = places::choices();
        let user = format!(
            "Addresses: {}\n\nCity codes: {}\n\nCountry codes: {}",
            take_chars(&asked.to_string(), 4000),
            json!(codes["cities"]),
            json!(codes["countries"])
        );
        let o = ChatOpts {
            max_tokens: Some(300),
            temperature: Some(0.0),
            no_think: true,
            ..opts("decide", task_id)
        };
        let msgs = [
            json!({"role": "system", "content": PLACES_SYSTEM}),
            json!({"role": "user", "content": user}),
        ];
        let Ok(r) = self.llm.chat(&msgs, None, &o).await else {
            return;
        };
        let answer = extract_json(&r.content).unwrap_or(Value::Null);
        let fields = place_fields(&answer, &asked, city_asked);
        for (k, v) in &fields {
            let _ = self.store.set_profile(k, v, "model:places");
        }
        self.event(
            task_id,
            "profile_places",
            json!({"asked": asked.as_object().map(|o| o.keys().cloned().collect::<Vec<_>>()), "saved": fields}),
        )
        .await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_valid_codes_for_the_addresses_asked_are_saved() {
        let asked = json!({"address_home": "12 Harbour St, Sydney", "address:Singapore flat": "10 Bayfront Ave"});
        let answer = json!({"home_city": "syd", "countries": {
            "address_home": "AU", "address:Singapore flat": "Singapore", "address:Other": "SG"}});
        assert_eq!(
            place_fields(&answer, &asked, true),
            vec![
                ("home_city".to_string(), "SYD".to_string()),
                ("home_country".to_string(), "AU".to_string()),
            ]
        );
        // a city off the table, prose: nothing
        let answer = json!({"home_city": "Sydney", "countries": {"address:Singapore flat": "sg"}});
        assert_eq!(
            place_fields(&answer, &asked, true),
            vec![(
                "address_country:Singapore flat".to_string(),
                "SG".to_string()
            )]
        );
        assert!(place_fields(&json!("It is in Sydney"), &asked, true).is_empty());
    }
}
