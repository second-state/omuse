//! The Shopping bot's store providers behind one search (`shop_search`, called by the `shop_order` machine): each
//! provider turns `(query, criteria, location)` into structured offers; code picks the providers for the user's
//! location (or only the one the user named), queries them in parallel with a time limit each, leaves a late or failing
//! one out (the result says so) and merges the offers into one list. Shopify's Global Catalog is the first provider;
//! others (Amazon, Shopee …) are added here without changing the model's tool or the skill.

use std::time::Duration;

use futures_util::future::join_all;
use omuse_common::util::get_str;
use serde_json::{json, Value};

use crate::shopify::{self, Shopify};

/// What a provider is asked: the product words, the price range in the user's currency, the user's location.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SearchReq {
    pub query: String,
    /// ISO 3166-1 alpha-2 of the user's location (where the goods ship to)
    pub country: String,
    /// ISO 4217 of the user's location
    pub currency: String,
    pub min_price: Option<f64>,
    pub max_price: Option<f64>,
    pub limit: u64,
}

/// A store provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provider {
    /// Shopify's Global Catalog: products across Shopify merchants that ship to the country
    ShopifyCatalog,
}

/// Every provider: (id, the names a user may call it by, the countries it serves — empty = everywhere).
const PROVIDERS: &[(Provider, &str, &[&str], &[&str])] =
    &[(Provider::ShopifyCatalog, "shopify", &["shopify"], &[])];

impl Provider {
    pub fn id(self) -> &'static str {
        PROVIDERS
            .iter()
            .find(|p| p.0 == self)
            .map(|p| p.1)
            .unwrap_or("")
    }
}

/// The providers for a search: the ones the user named (by a provider's own name, any case — the model's JSON), else
/// every provider that serves the user's country. `Err(names)` lists named stores no provider covers.
pub fn providers_for(country: &str, named: &[String]) -> Result<Vec<Provider>, Vec<String>> {
    let country = country.to_uppercase();
    if !named.is_empty() {
        let mut out = Vec::new();
        let mut unknown = Vec::new();
        for n in named {
            let n = n.trim().to_lowercase();
            match PROVIDERS.iter().find(|p| p.2.iter().any(|a| *a == n)) {
                Some(p) if !out.contains(&p.0) => out.push(p.0),
                Some(_) => {}
                None => unknown.push(n),
            }
        }
        return if out.is_empty() {
            Err(unknown)
        } else {
            Ok(out)
        };
    }
    Ok(PROVIDERS
        .iter()
        .filter(|p| p.3.is_empty() || p.3.contains(&country.as_str()))
        .map(|p| p.0)
        .collect())
}

/// The clients the providers use.
pub struct Clients {
    pub shopify: Shopify,
}

async fn search_one(p: Provider, c: &Clients, r: &SearchReq) -> Result<Vec<Value>, String> {
    match p {
        Provider::ShopifyCatalog => {
            if !c.shopify.has_credentials() {
                return Err("no_credentials".into());
            }
            let req = shopify::search_request(
                &r.query,
                &r.country,
                &r.currency,
                r.min_price,
                r.max_price,
                r.limit,
            );
            c.shopify.catalog_search(req).await.map_err(|e| e.0)
        }
    }
}

/// Every provider at once, each within `limit`; the offers merged (each tagged with its `provider` and an `id`
/// `<provider>-<n>`), and how each provider did (`ok` with its count, `timed_out`, or its `error`).
pub async fn search_all(
    providers: &[Provider],
    c: &Clients,
    r: &SearchReq,
    limit: Duration,
) -> Value {
    let runs = providers
        .iter()
        .map(|p| async move { (*p, tokio::time::timeout(limit, search_one(*p, c, r)).await) });
    let mut offers: Vec<Value> = Vec::new();
    let mut status: Vec<Value> = Vec::new();
    for (p, res) in join_all(runs).await {
        match res {
            Ok(Ok(list)) => {
                status.push(json!({"provider": p.id(), "ok": true, "count": list.len()}));
                offers.extend(tag(p, list));
            }
            Ok(Err(e)) => status.push(json!({"provider": p.id(), "ok": false, "error": e})),
            Err(_) => status.push(json!({"provider": p.id(), "ok": false, "timed_out": true,
                                          "error": format!("no answer within {} s", limit.as_secs())})),
        }
    }
    json!({"offers": offers, "providers": status})
}

/// A provider's offers with their ids and the provider's name.
pub fn tag(p: Provider, list: Vec<Value>) -> Vec<Value> {
    list.into_iter()
        .enumerate()
        .map(|(i, mut o)| {
            o["provider"] = json!(p.id());
            o["offer_id"] = json!(format!("{}-{}", p.id(), i + 1));
            o
        })
        .collect()
}

/// The offers' currencies that need a rate into `target`.
pub fn fx_symbols(offers: &[Value], target: &str) -> Vec<String> {
    shopify::fx_symbols(offers, target)
}

/// Whether every offer of the result has an id (a check on code's own output).
pub fn ids_ok(result: &Value) -> bool {
    result["offers"]
        .as_array()
        .is_some_and(|a| a.iter().all(|o| !get_str(o, "offer_id").is_empty()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn providers_by_location_or_by_the_name_the_user_gave() {
        assert_eq!(providers_for("SG", &[]), Ok(vec![Provider::ShopifyCatalog]));
        assert_eq!(
            providers_for("SG", &["Shopify".into()]),
            Ok(vec![Provider::ShopifyCatalog])
        );
        assert_eq!(
            providers_for("SG", &["Amazon".into()]),
            Err(vec!["amazon".to_string()])
        );
    }

    #[tokio::test]
    async fn a_provider_without_credentials_is_left_out_and_the_result_says_so() {
        let c = Clients {
            shopify: Shopify::new(
                reqwest::Client::new(),
                "http://127.0.0.1:9",
                "http://127.0.0.1:9",
                "p",
                "",
                "",
            ),
        };
        let r = search_all(
            &[Provider::ShopifyCatalog],
            &c,
            &SearchReq::default(),
            Duration::from_secs(2),
        )
        .await;
        assert_eq!(r["offers"], json!([]));
        assert_eq!(
            r["providers"][0],
            json!({"provider": "shopify", "ok": false, "error": "no_credentials"})
        );
    }

    #[tokio::test]
    async fn a_late_provider_is_left_out_within_the_time_limit() {
        // a port that accepts and never answers would be slow to set up; an unroutable address times out instead
        let c = Clients {
            shopify: Shopify::new(
                reqwest::Client::new(),
                "http://10.255.255.1",
                "http://10.255.255.1",
                "p",
                "id",
                "secret",
            ),
        };
        let t = std::time::Instant::now();
        let r = search_all(
            &[Provider::ShopifyCatalog],
            &c,
            &SearchReq::default(),
            Duration::from_millis(300),
        )
        .await;
        assert!(t.elapsed() < Duration::from_secs(3));
        assert_eq!(r["providers"][0]["ok"], false);
    }

    #[test]
    fn offers_are_tagged_with_their_provider_and_an_id() {
        let t = tag(
            Provider::ShopifyCatalog,
            vec![json!({"title": "a"}), json!({"title": "b"})],
        );
        assert_eq!(t[1]["offer_id"], "shopify-2");
        assert_eq!(t[0]["provider"], "shopify");
        assert!(ids_ok(&json!({"offers": t})));
    }
}
