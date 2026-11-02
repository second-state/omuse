//! `shop_order` by API (the Shopping bot; docs/sites/shopify.md): no page is opened and nothing is filled in a browser.
//!
//! 1. Search: `shop_search` — the store providers for the user's location (Shopify's Global Catalog first; only the
//!    stores the user named when they named some), in parallel, the offers merged, each price also in the user's
//!    currency. The location is the profile's structured fields (`home_country`, else the home city's country) and its
//!    currency.
//! 2. Code filters and ranks the offers (in stock; the price range in the user's currency; words; rating) and returns
//!    the best ones as JSON (`status: review`, each with its `offer_id`). The model reads them and calls `shop_order`
//!    again with `choices` = the offer ids that best fit the request.
//! 3. Code checks the ids against that review result (unknown ids dropped; none left → the first ones by rule) and
//!    shows the choice card: the price in the user's currency, the merchant and the product link (`show`).
//! 4. A pick to buy: the store's checkout for that variant (the catalog's `checkout_url`, Shopify's cart link), else
//!    the product page (`status: link`) — the user pays there, in their own browser.
//!
//! No page is ever opened (the Shopping bot has no browser): a shop's link the user gave is reported back as not
//! searched (`not_covered`) — only the providers' stores are searched.

use omuse_common::util::get_str;
use serde_json::{json, Value};

use super::super::driver::Step;
use super::super::show as S;
use super::super::util::{content_of, stored_call, tool_calls};
use super::super::workflow::{RESULT_PREFIX, RESULT_SUFFIX};
use super::{calls_of, data_of, Ids, Machine, ShopReq, SHOP_ORDER};
use crate::listings::Listing;

/// The machine's code call to the store providers.
pub const SEARCH: &str = "shop_search";
/// The result status the model chooses from.
pub const REVIEW: &str = "review";
/// How many offers a review result carries.
const REVIEW_MAX: usize = 12;

fn money_text(p: Option<f64>, cur: &str) -> String {
    match p {
        Some(p) => format!("{cur} {p:.2}").trim().to_string(),
        None => String::new(),
    }
}

/// An offer for the review and the card: its id, title, the price in the user's currency (and the merchant's own when
/// it was converted), rating, merchant, link, provider.
fn offer_json(o: &Value) -> Value {
    let local = o["price_local"].as_f64();
    let (price, cur) = match local {
        Some(p) => (Some(p), get_str(o, "currency_local")),
        None => (o["price"].as_f64(), get_str(o, "currency")),
    };
    let mut out = json!({"offer_id": get_str(o, "offer_id"), "title": get_str(o, "title"),
        "price": price, "currency": cur, "merchant": get_str(&o["seller"], "name"),
        "merchant_site": get_str(&o["seller"], "url"), "url": get_str(o, "url"), "provider": get_str(o, "provider")});
    // the variant's own checkout (Shopify's cart link to the store's checkout): where the user pays
    let checkout = get_str(o, "checkout_url");
    if checkout.starts_with("https://") {
        out["checkout_url"] = json!(checkout);
    }
    if local.is_some() && o.get("fx_rate").is_some() {
        out["shop_price"] = o["price"].clone();
        out["shop_currency"] = json!(get_str(o, "currency"));
    }
    if let Some(r) = o["rating"].as_f64() {
        out["rating"] = json!((r * 10.0).round() / 10.0);
        out["ratings"] = json!(o["ratings"].as_u64().unwrap_or(0));
    }
    out
}

/// The offers that meet the criteria, best first (code's rule), in the user's currency where a price is compared.
pub fn ranked(sr: &ShopReq, offers: &[Value], currency: &str) -> Vec<Value> {
    let rows: Vec<Value> = offers
        .iter()
        .filter(|o| o["available"].as_bool().unwrap_or(true))
        .map(offer_json)
        // a price range is compared in the user's currency only (a price with no rate is not compared — left out)
        .filter(|o| {
            (sr.ranking.max_price.is_none() && sr.ranking.min_price.is_none())
                || get_str(o, "currency").eq_ignore_ascii_case(currency)
        })
        .collect();
    let listings: Vec<Listing> = rows
        .iter()
        .map(|o| Listing {
            title: get_str(o, "offer_id"),
            stars: o["rating"].as_f64().unwrap_or(0.0),
            ratings: o["ratings"].as_u64().unwrap_or(0),
            rating_text: String::new(),
            price: o["price"].as_f64(),
            price_text: money_text(o["price"].as_f64(), &get_str(o, "currency")),
            url: get_str(o, "url"),
        })
        .collect();
    // the words are matched on the titles; the rule's order and the range on the listings
    let mut rule = sr.ranking.clone();
    rule.count = Some(REVIEW_MAX);
    let words_ok = |o: &Value| {
        let t = get_str(o, "title").to_lowercase();
        sr.ranking
            .title_has
            .iter()
            .all(|w| t.contains(&w.trim().to_lowercase()))
            && !sr
                .ranking
                .title_not
                .iter()
                .any(|w| !w.trim().is_empty() && t.contains(&w.trim().to_lowercase()))
    };
    rule.title_has.clear();
    rule.title_not.clear();
    let mut kept: Vec<Value> = Vec::new();
    // Ranking::apply keeps at most 8: apply it to the rows that pass the words, in chunks of the full list order
    let order = apply_all(&rule, &listings);
    for id in order {
        if let Some(o) = rows.iter().find(|o| get_str(o, "offer_id") == id) {
            if words_ok(o) {
                kept.push(o.clone());
            }
        }
        if kept.len() >= REVIEW_MAX {
            break;
        }
    }
    kept
}

/// Every listing that meets the rule, in the rule's order (`Ranking::apply` without its cap of 8).
fn apply_all(rule: &crate::listings::Ranking, listings: &[Listing]) -> Vec<String> {
    let mut left: Vec<Listing> = listings.to_vec();
    let mut out: Vec<String> = Vec::new();
    loop {
        let mut r = rule.clone();
        r.count = Some(8);
        let got = r.apply(&left);
        if got.is_empty() {
            break;
        }
        for l in &got {
            out.push(l.title.clone());
        }
        left.retain(|l| !got.iter().any(|g| g.title == l.title));
    }
    // apply() orders each batch; batches come best first, so the concatenation is in order
    out
}

/// The latest review result of `shop_order` in `transcript` (code's own JSON).
pub fn last_review(transcript: &[Value]) -> Option<Value> {
    let want = format!("{SHOP_ORDER}{RESULT_SUFFIX}");
    let id = transcript
        .iter()
        .flat_map(tool_calls)
        .filter(|tc| get_str(tc, "id").starts_with(RESULT_PREFIX) && stored_call(tc).0 == want)
        .map(|tc| get_str(tc, "id"))
        .next_back()?;
    let m = transcript
        .iter()
        .find(|m| get_str(m, "role") == "tool" && get_str(m, "tool_call_id") == id)?;
    serde_json::from_str::<Value>(&content_of(m))
        .ok()
        .filter(|v| get_str(v, "status") == REVIEW)
}

/// Whether `shop_order` stays offered on the reply turn: right after a review result, the model chooses from it.
pub fn review_open(transcript: &[Value]) -> bool {
    let want = format!("{SHOP_ORDER}{RESULT_SUFFIX}");
    let last = transcript
        .iter()
        .flat_map(tool_calls)
        .filter(|tc| get_str(tc, "id").starts_with(RESULT_PREFIX))
        .map(|tc| stored_call(tc).0)
        .next_back();
    last.as_deref() == Some(want.as_str()) && last_review(transcript).is_some()
}

/// The offers the model chose, checked against the review: the known ids in its order (at most `max`), unknown ones
/// reported; none known → the first by code's rule.
pub fn chosen(review: &Value, ids: &[String], max: usize) -> (Vec<Value>, Vec<String>) {
    let offers = review["offers"].as_array().cloned().unwrap_or_default();
    let mut out: Vec<Value> = Vec::new();
    let mut unknown: Vec<String> = Vec::new();
    for id in ids {
        match offers.iter().find(|o| get_str(o, "offer_id") == id.trim()) {
            Some(o) if !out.iter().any(|x| get_str(x, "offer_id") == id.trim()) => {
                out.push(o.clone())
            }
            Some(_) => {}
            None => unknown.push(id.clone()),
        }
        if out.len() >= max {
            break;
        }
    }
    if out.is_empty() {
        out = offers.into_iter().take(max.min(3)).collect();
    }
    (out, unknown)
}

fn card_option(o: &Value) -> Value {
    let cur = get_str(o, "currency");
    let mut details = vec![money_text(o["price"].as_f64(), &cur)];
    if let (Some(p), c) = (o["shop_price"].as_f64(), get_str(o, "shop_currency")) {
        details[0] = format!(
            "{} (the shop's price: {})",
            details[0],
            money_text(Some(p), &c)
        );
    }
    if let Some(r) = o["rating"].as_f64() {
        details.push(format!(
            "{r:.1}/5 ({} ratings)",
            o["ratings"].as_u64().unwrap_or(0)
        ));
    }
    let merchant = get_str(o, "merchant");
    if !merchant.is_empty() {
        details.push(format!("sold by {merchant}"));
    }
    let mut c = json!({"label": get_str(o, "title"), "details": details, "source_url": get_str(o, "url"),
                       "currency": cur, "offer_id": get_str(o, "offer_id")});
    if let Some(p) = o["price"].as_f64() {
        c["price"] = json!(p);
    }
    c
}

fn offer_line(o: &Value) -> Value {
    let text = [
        get_str(o, "title"),
        money_text(o["price"].as_f64(), &get_str(o, "currency")),
        get_str(o, "merchant"),
    ]
    .into_iter()
    .filter(|s| !s.is_empty())
    .collect::<Vec<_>>()
    .join(" — ");
    S::link("Offer", "商品", &text, &get_str(o, "url"))
}

/// The user's location: the profile's country (structured) and its currency.
fn location(profile: &Value) -> (String, String) {
    let country = crate::places::home_country(profile);
    let currency = super::home_currency(profile, "");
    (country, currency)
}

/// The next step of a shopping run by API.
pub(super) fn next(sr: &ShopReq, t: &[Value], m: &Machine, ids: &Ids) -> Step {
    next_api(sr, t, m, ids).unwrap_or_else(|| {
        Step::Done(json!({"status": "not_covered", "url": sr.url,
            "show": [S::fact("Not searched", "未搜索",
                &format!("{} — I search stores by API only (Shopify merchants for now) and never open a shop's page", sr.url))],
            "hints": []}))
    })
}

/// The search, the review and the card; `None` for a shop's link (not searched: no page is opened).
fn next_api(sr: &ShopReq, t: &[Value], m: &Machine, ids: &Ids) -> Option<Step> {
    if !sr.url.is_empty() && sr.query.is_empty() && sr.choices.is_empty() && sr.pick.is_empty() {
        return None;
    }
    let (country, currency) = location(m.profile);
    let count = sr.ranking.count.unwrap_or(4).clamp(1, 6);
    // the model chose from the review: the card
    if !sr.choices.is_empty() || !sr.pick.is_empty() {
        if let Some(review) = last_review(m.before) {
            if !sr.pick.is_empty() {
                let want = sr.pick.trim().to_lowercase();
                let offers = review["offers"].as_array().cloned().unwrap_or_default();
                if let Some(o) = offers
                    .iter()
                    .find(|o| get_str(o, "title").to_lowercase() == want)
                {
                    // the store's checkout for that variant, where the user pays in their own browser (no card is
                    // ever sent by API: docs/sites/shopify.md, "Trust tier"); else the product page
                    let checkout = get_str(o, "checkout_url");
                    let mut show = Vec::new();
                    if checkout.is_empty() {
                        show.push(S::link(
                            "Buy it on the merchant's page",
                            "在商家页面购买",
                            &get_str(o, "title"),
                            &get_str(o, "url"),
                        ));
                    } else {
                        show.push(S::link(
                            "Check out and pay in your browser",
                            "在你的浏览器里结账付款",
                            &get_str(o, "title"),
                            &checkout,
                        ));
                        show.push(S::link(
                            "Product page",
                            "商品页面",
                            &get_str(o, "title"),
                            &get_str(o, "url"),
                        ));
                    }
                    show.push(S::amount(
                        "Price",
                        "价格",
                        o["price"].as_f64().unwrap_or(0.0),
                        &get_str(o, "currency"),
                    ));
                    return Some(Step::Done(
                        json!({"status": "link", "offer": o, "show": show,
                        "hints": ["nothing has been bought: the user pays on the store's checkout"]}),
                    ));
                }
            } else {
                let (picked, unknown) = chosen(&review, &sr.choices, count);
                if calls_of(t, "present_choices").is_empty() {
                    return Some(ids.calls("present_choices", json!({
                        "question": format!("{} — offers that ship to {}", get_str(&review, "query"), get_str(&review, "country")),
                        "kind": "comparison", "options": picked.iter().map(card_option).collect::<Vec<_>>(),
                        "picked": picked.first().map(|o| get_str(o, "title")).unwrap_or_default()})));
                }
                let mut show: Vec<Value> = picked.iter().map(offer_line).collect();
                for p in review["providers_left_out"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default()
                {
                    show.push(S::fact("Not searched", "未搜索", &get_str(&p, "why")));
                }
                return Some(Step::Done(
                    json!({"status": "shown", "query": review["query"], "country": review["country"],
                    "currency": review["currency"], "options": picked, "unknown_ids": unknown, "show": show, "hints": []}),
                ));
            }
        }
    }
    // the search
    let Some((_, msg)) = calls_of(t, SEARCH).into_iter().next() else {
        let mut a = json!({"query": if sr.query.is_empty() { sr.pick.clone() } else { sr.query.clone() },
                           "country": country, "currency": currency, "limit": 30});
        if !sr.stores.is_empty() {
            a["stores"] = json!(sr.stores);
        }
        if let Some(x) = sr.ranking.max_price {
            a["max_price"] = json!(x);
        }
        if let Some(x) = sr.ranking.min_price {
            a["min_price"] = json!(x);
        }
        return Some(ids.calls(SEARCH, a));
    };
    let found = data_of(msg);
    let providers = found["providers"].as_array().cloned().unwrap_or_default();
    let left_out: Vec<Value> = providers
        .iter()
        .filter(|p| p["ok"].as_bool() != Some(true))
        .map(|p| {
            let why = match get_str(p, "error").as_str() {
                "no_credentials" => {
                    format!("{}: not set up (no credentials)", get_str(p, "provider"))
                }
                e => format!("{}: {e}", get_str(p, "provider")),
            };
            json!({"provider": get_str(p, "provider"), "why": why})
        })
        .collect();
    let not_covered = found["not_covered"].as_array().cloned().unwrap_or_default();
    let offers = found["offers"].as_array().cloned().unwrap_or_default();
    let list = ranked(sr, &offers, &currency);
    let query = if sr.query.is_empty() {
        sr.pick.clone()
    } else {
        sr.query.clone()
    };
    if list.is_empty() {
        let mut show = vec![S::fact(
            "Searched",
            "搜索",
            &format!("{query} (ships to {country}): nothing in stock meets the criteria"),
        )];
        for p in &left_out {
            show.push(S::fact("Not searched", "未搜索", &get_str(p, "why")));
        }
        if !not_covered.is_empty() {
            show.push(S::fact(
                "Not a store I can search",
                "无法搜索的商店",
                &not_covered
                    .iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join(", "),
            ));
        }
        return Some(Step::Done(
            json!({"status": "nothing_found", "query": query, "country": country,
            "found": offers.len(), "providers_left_out": left_out, "not_covered": not_covered,
            "show": show, "hints": []}),
        ));
    }
    Some(Step::Done(
        json!({"status": REVIEW, "query": query, "country": country, "currency": currency,
        "criteria": sr.ranking.to_json(), "found": offers.len(), "offers": list,
        "providers_left_out": left_out,
        "call_again": SHOP_ORDER,
        "choose": format!("call shop_order again with the same arguments and choices = the offer_ids of the {count} offers that best fit the request, best first"),
        "hints": []}),
    ))
}
