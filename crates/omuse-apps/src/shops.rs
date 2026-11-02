//! The Shopping bot's store providers behind one search (`shop_search`, called by the `shop_order` machine): each
//! provider turns `(query, criteria, location)` into structured offers; code picks the providers for the user's
//! location (or only the one the user named), queries them in parallel with a time limit each, leaves a late or failing
//! one out (the result says so) and merges the offers into one list. Shopify's Global Catalog is the first provider;
//! others (Amazon, Shopee …) are added here without changing the model's tool or the skill.

use std::time::Duration;

use futures_util::future::join_all;
use omuse_common::util::get_str;
use serde_json::{json, Value};

use crate::serpapi::SerpApi;
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
    /// the user's city, for the providers that search from a place (Google Shopping's `location`); "" = the country
    pub location: String,
    /// the brands the user named (the model's JSON): searched with the product words
    pub brands: Vec<String>,
    /// the merchants the user named that no provider is (Amazon, Walmart …): Google Shopping keeps only their offers
    pub merchants: Vec<String>,
}

impl SearchReq {
    /// The words searched: the brands the user named before the product words (once each).
    pub fn words(&self) -> String {
        let q = self.query.trim().to_string();
        let lc = q.to_lowercase();
        let mut out: Vec<String> = self
            .brands
            .iter()
            .map(|b| b.trim().to_string())
            .filter(|b| !b.is_empty() && !lc.contains(&b.to_lowercase()))
            .collect();
        out.push(q);
        out.join(" ").trim().to_string()
    }
}

/// A store provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provider {
    /// Shopify's Global Catalog: products across Shopify merchants that ship to the country
    ShopifyCatalog,
    /// Google Shopping through SerpApi: every store Google lists for the country (each search costs a SerpApi
    /// credit — only when the user named a brand or a merchant, or Google itself)
    GoogleShopping,
}

/// One provider: its id, the names a user may call it by, the countries it serves (empty = everywhere), whether it is
/// searched by default (false: only when asked for).
type ProviderRow = (
    Provider,
    &'static str,
    &'static [&'static str],
    &'static [&'static str],
    bool,
);

/// Every provider.
const PROVIDERS: &[ProviderRow] = &[
    (Provider::ShopifyCatalog, "shopify", &["shopify"], &[], true),
    (
        Provider::GoogleShopping,
        "google",
        &["google", "google shopping"],
        &[],
        false,
    ),
];

impl Provider {
    pub fn id(self) -> &'static str {
        PROVIDERS
            .iter()
            .find(|p| p.0 == self)
            .map(|p| p.1)
            .unwrap_or("")
    }
}

/// The providers for a search and the merchants to keep (the model's JSON, compared whole, any case):
/// - stores named that are providers → those providers; stores named that are not (Amazon, Walmart …) → merchants,
///   searched on Google Shopping and kept only from those sellers;
/// - a brand named → the default providers and Google Shopping;
/// - nothing named → the default providers for the country (no SerpApi credit spent).
pub fn providers_for(
    country: &str,
    named: &[String],
    brands: &[String],
) -> (Vec<Provider>, Vec<String>) {
    let country = country.to_uppercase();
    let mut out: Vec<Provider> = Vec::new();
    let mut merchants: Vec<String> = Vec::new();
    for n in named {
        let lc = n.trim().to_lowercase();
        if lc.is_empty() {
            continue;
        }
        match PROVIDERS.iter().find(|p| p.2.iter().any(|a| *a == lc)) {
            Some(p) if !out.contains(&p.0) => out.push(p.0),
            Some(_) => {}
            None => merchants.push(n.trim().to_string()),
        }
    }
    let google = !merchants.is_empty() || brands.iter().any(|b| !b.trim().is_empty());
    if out.is_empty() && merchants.is_empty() {
        out = PROVIDERS
            .iter()
            .filter(|p| p.4 && (p.3.is_empty() || p.3.contains(&country.as_str())))
            .map(|p| p.0)
            .collect();
    }
    if google && !out.contains(&Provider::GoogleShopping) {
        out.push(Provider::GoogleShopping);
    }
    (out, merchants)
}

/// A seller is one of the merchants the user named: its name and the merchant's, letters and digits only, any case —
/// the one starts with the other ("Amazon.sg" is Amazon).
pub fn seller_is(seller: &str, merchants: &[String]) -> bool {
    let norm = |s: &str| {
        s.chars()
            .filter(|c| c.is_alphanumeric())
            .collect::<String>()
            .to_lowercase()
    };
    let s = norm(seller);
    !s.is_empty()
        && merchants.iter().any(|m| {
            let m = norm(m);
            !m.is_empty() && (s.starts_with(&m) || m.starts_with(&s))
        })
}

/// The clients the providers use.
pub struct Clients {
    pub shopify: Shopify,
    /// the SerpApi client when a key is set
    pub serpapi: Option<SerpApi>,
}

async fn search_one(p: Provider, c: &Clients, r: &SearchReq) -> Result<Vec<Value>, String> {
    match p {
        Provider::ShopifyCatalog => {
            if !c.shopify.has_credentials() {
                return Err("no_credentials".into());
            }
            let req = shopify::search_request(
                &r.words(),
                &r.country,
                &r.currency,
                r.min_price,
                r.max_price,
                r.limit,
            );
            c.shopify.catalog_search(req).await.map_err(|e| e.0)
        }
        Provider::GoogleShopping => {
            let Some(s) = &c.serpapi else {
                return Err("no_key".into());
            };
            let mut a = json!({"query": r.words(), "country": r.country, "currency": r.currency,
                               "location": r.location});
            if let Some(m) = r.min_price {
                a["min_price"] = json!(m);
            }
            if let Some(m) = r.max_price {
                a["max_price"] = json!(m);
            }
            let list = s.shopping(&a).await.map_err(|e| e.to_string())?;
            Ok(if r.merchants.is_empty() {
                list
            } else {
                list.into_iter()
                    .filter(|o| seller_is(&get_str(&o["seller"], "name"), &r.merchants))
                    .collect()
            })
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
    fn google_shopping_only_when_a_brand_or_a_merchant_is_named() {
        let none: Vec<String> = Vec::new();
        // nothing named: Shopify only — no SerpApi credit
        assert_eq!(
            providers_for("SG", &none, &none),
            (vec![Provider::ShopifyCatalog], vec![])
        );
        assert_eq!(
            providers_for("SG", &["Shopify".into()], &none),
            (vec![Provider::ShopifyCatalog], vec![])
        );
        // a brand: Shopify and Google Shopping
        assert_eq!(
            providers_for("SG", &none, &["Nike".into()]),
            (
                vec![Provider::ShopifyCatalog, Provider::GoogleShopping],
                vec![]
            )
        );
        // a merchant that is no provider: Google Shopping, kept to that seller
        assert_eq!(
            providers_for("SG", &["Amazon".into()], &none),
            (vec![Provider::GoogleShopping], vec!["Amazon".to_string()])
        );
        assert_eq!(
            providers_for("SG", &["google".into()], &none),
            (vec![Provider::GoogleShopping], vec![])
        );
        // a seller is a named merchant by its name, letters and digits only
        let amazon = vec!["Amazon".to_string()];
        assert!(seller_is("Amazon.sg", &amazon) && seller_is("amazon", &amazon));
        assert!(!seller_is("iRUN Singapore", &amazon) && !seller_is("", &amazon));
        // the brand goes in the words searched, once
        let r = SearchReq {
            query: "running shoes".into(),
            brands: vec!["Nike".into()],
            ..Default::default()
        };
        assert_eq!(r.words(), "Nike running shoes");
        let r = SearchReq {
            query: "Nike running shoes".into(),
            brands: vec!["nike".into()],
            ..Default::default()
        };
        assert_eq!(r.words(), "Nike running shoes");
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
            serpapi: None,
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
            serpapi: None,
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
