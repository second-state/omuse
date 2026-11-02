//! Shopify by API only (docs/sites/shopify.md: every call recorded from the real services, 2026-11-02): product search
//! across merchants in Shopify's Global Catalog (`search_catalog` / `get_product` on `catalog.shopify.com/api/ucp/mcp`,
//! with the token the client id and secret are exchanged for), a store's own catalog and its UCP Checkout MCP
//! (`create_checkout` on `https://<shop>.myshopify.com/api/ucp/mcp`, found through the store's `/.well-known/ucp`).
//! No page is opened and no form is filled: the checkout is built with the buyer and the delivery address as JSON, and
//! its `continue_url` (Shopify's prefilled checkout) is where the user pays. `complete_checkout` is never called here
//! (see [`completion_blocker`]: our credentials cannot complete a purchase and the Vault's cards are raw numbers).
//!
//! Every request is one JSON-RPC `tools/call` (no MCP handshake) with `meta.ucp-agent.profile`. The answers are
//! normalised to small JSON objects ([`products_of`], [`product_of`], [`checkout_of`]) — prices in major units.

use std::time::{Duration, Instant};

use omuse_common::util::get_str;
use parking_lot::Mutex;
use serde_json::{json, Map, Value};

/// A Shopify API failure (HTTP, an error object, an unreadable answer).
#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct ShopifyError(pub String);

type Res<T> = Result<T, ShopifyError>;

/// The trust tier a merchant's Checkout MCP gave our requests (docs/sites/shopify.md, "Trust tier"): the Global API
/// token is refused there ("Invalid global access token"), so checkout calls go without credentials — Anonymous:
/// catalog, cart and checkout building, never `complete_checkout`.
pub const CHECKOUT_TIER: &str = "anonymous";

/// Currencies whose minor unit is the major unit (ISO 4217 exponent 0): their amounts are not divided by 100.
const ZERO_DECIMAL: &[&str] = &[
    "BIF", "CLP", "DJF", "GNF", "ISK", "JPY", "KMF", "KRW", "PYG", "RWF", "UGX", "UYI", "VND",
    "VUV", "XAF", "XOF", "XPF",
];

/// An amount `{amount, currency}` in minor units → (major units, currency).
pub fn money(v: &Value) -> Option<(f64, String)> {
    let cur = get_str(v, "currency").to_uppercase();
    let minor = v.get("amount").and_then(Value::as_f64)?;
    let major = if ZERO_DECIMAL.contains(&cur.as_str()) {
        minor
    } else {
        minor / 100.0
    };
    Some((major, cur))
}

fn minor_of(amount: f64, currency: &str) -> i64 {
    if ZERO_DECIMAL.contains(&currency.to_uppercase().as_str()) {
        amount.round() as i64
    } else {
        (amount * 100.0).round() as i64
    }
}

/// The JSON-RPC body of one tool call.
pub fn rpc_body(id: u64, name: &str, mut args: Value, profile: &str) -> Value {
    if let Some(o) = args.as_object_mut() {
        o.insert("meta".into(), json!({"ucp-agent": {"profile": profile}}));
    }
    json!({"jsonrpc": "2.0", "id": id, "method": "tools/call", "params": {"name": name, "arguments": args}})
}

/// A tool call's answer: its `structuredContent` (also when `isError`: a checkout's `messages` carry the reason), else
/// the JSON in its text content; an `error` object or a plain error text is an error.
pub fn tool_result(body: &Value) -> Res<Value> {
    if let Some(e) = body.get("error") {
        let msg = [get_str(e, "message"), get_str(e, "data")]
            .into_iter()
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join(": ");
        return Err(ShopifyError(if msg.is_empty() {
            e.to_string()
        } else {
            msg
        }));
    }
    let r = body.get("result").cloned().unwrap_or(Value::Null);
    if let Some(sc) = r.get("structuredContent").filter(|v| v.is_object()) {
        return Ok(sc.clone());
    }
    let text = r["content"]
        .as_array()
        .and_then(|c| c.iter().find_map(|x| x.get("text").and_then(Value::as_str)))
        .unwrap_or_default();
    match serde_json::from_str::<Value>(text) {
        Ok(v) if v.is_object() => Ok(v),
        _ if r.get("isError").and_then(Value::as_bool) == Some(true) || !text.is_empty() => {
            Err(ShopifyError(text.chars().take(400).collect()))
        }
        _ => Err(ShopifyError("no result in the answer".into())),
    }
}

fn arr(v: &Value, k: &str) -> Vec<Value> {
    v.get(k)
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

fn option_labels(v: &Value) -> Vec<Value> {
    arr(v, "options")
        .iter()
        .map(|o| json!({"name": get_str(o, "name"), "label": get_str(o, "label")}))
        .collect()
}

/// One variant, normalised: id, title, price (major units), currency, available, options, the seller and links.
fn variant_of(v: &Value) -> Value {
    let (price, currency) = money(&v["price"]).unwrap_or((f64::NAN, String::new()));
    let seller = &v["seller"];
    let mut o = json!({
        "id": get_str(v, "id"),
        "title": get_str(v, "title"),
        "available": v["availability"]["available"].as_bool().unwrap_or(false),
        "options": option_labels(v),
        "url": get_str(v, "url"),
        "checkout_url": get_str(v, "checkout_url"),
        "currency": currency,
    });
    if price.is_finite() {
        o["price"] = json!(price);
    }
    if let Some((lp, _)) = money(&v["list_price"]) {
        o["list_price"] = json!(lp);
    }
    if seller.is_object() {
        o["seller"] = json!({"name": get_str(seller, "name"), "url": get_str(seller, "url"),
                             "domain": get_str(seller, "domain")});
    }
    o
}

fn rating_of(p: &Value, out: &mut Value) {
    if let Some(r) = p["rating"]["value"].as_f64() {
        let max = p["rating"]["scale_max"].as_f64().unwrap_or(5.0);
        out["rating"] = json!(if max > 0.0 { r * 5.0 / max } else { r });
        out["ratings"] = json!(p["rating"]["count"].as_u64().unwrap_or(0));
    }
}

/// `search_catalog`'s products, normalised: id, title, rating (out of 5) and ratings, the first variant's price,
/// availability, options, seller and links.
pub fn products_of(sc: &Value) -> Vec<Value> {
    arr(sc, "products")
        .iter()
        .filter_map(|p| {
            let id = get_str(p, "id");
            let first = arr(p, "variants").into_iter().next()?;
            let v = variant_of(&first);
            let mut o = json!({"id": id, "title": get_str(p, "title"), "handle": get_str(p, "handle"),
                               "variant_id": v["id"], "price": v["price"], "currency": v["currency"],
                               "available": v["available"], "options": v["options"], "url": v["url"],
                               "checkout_url": v["checkout_url"], "seller": v["seller"]});
            if let Some((lo, _)) = money(&p["price_range"]["min"]) {
                o["price_min"] = json!(lo);
            }
            rating_of(p, &mut o);
            Some(o)
        })
        .collect()
}

/// The market symbols whose rates convert the products' currencies into `target` (Yahoo's `GBPSGD=X`).
pub fn fx_symbols(products: &[Value], target: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for p in products {
        let c = get_str(p, "currency").to_uppercase();
        let s = format!("{c}{}=X", target.to_uppercase());
        if !c.is_empty() && c != target.to_uppercase() && !out.contains(&s) {
            out.push(s);
        }
    }
    out
}

/// The rate `from` → `to` in the market results (`{symbol, last}`), from the pair's own symbol.
pub fn rate_in(results: &[Value], from: &str, to: &str) -> Option<f64> {
    let own = format!("{}{}=X", from.to_uppercase(), to.to_uppercase());
    results
        .iter()
        .find(|r| get_str(r, "symbol") == own)
        .and_then(|r| r.get("last").and_then(Value::as_f64))
        .filter(|x| *x > 0.0)
}

/// Each product's price in the user's currency: `price_local` / `currency_local` — the price itself when the merchant
/// already prices in it, else converted at the market rate (`fx_rate`); none when no rate came.
pub fn localise(products: &mut [Value], target: &str, results: &[Value]) {
    let target = target.to_uppercase();
    for p in products.iter_mut() {
        let cur = get_str(p, "currency").to_uppercase();
        let Some(price) = p["price"].as_f64() else {
            continue;
        };
        let rate = if cur == target {
            Some(1.0)
        } else {
            rate_in(results, &cur, &target)
        };
        if let Some(r) = rate {
            let dp = if ZERO_DECIMAL.contains(&target.as_str()) {
                0
            } else {
                2
            };
            let f = 10f64.powi(dp);
            p["price_local"] = json!((price * r * f).round() / f);
            p["currency_local"] = json!(target);
            if cur != target {
                p["fx_rate"] = json!(r);
            }
        }
    }
}

/// `get_product`'s product, normalised: the options with each value's availability, the variants, the selected one.
pub fn product_of(sc: &Value) -> Option<Value> {
    let p = sc.get("product")?;
    let options: Vec<Value> = arr(p, "options")
        .iter()
        .map(|o| {
            json!({"name": get_str(o, "name"),
                   "values": arr(o, "values").iter().map(|x| json!({"label": get_str(x, "label"),
                        "available": x["available"].as_bool().unwrap_or(true)})).collect::<Vec<_>>()})
        })
        .collect();
    let mut o = json!({"id": get_str(p, "id"), "title": get_str(p, "title"), "options": options,
                       "variants": arr(p, "variants").iter().map(variant_of).collect::<Vec<_>>(),
                       "selected": option_labels(&json!({"options": p.get("selected").cloned().unwrap_or(json!([]))}))});
    rating_of(p, &mut o);
    Some(o)
}

/// The store's UCP MCP endpoint from its `/.well-known/ucp`, when the store offers the checkout capability.
pub fn checkout_endpoint(well_known: &Value) -> Option<String> {
    let ucp = well_known.get("ucp")?;
    ucp["capabilities"].get("dev.ucp.shopping.checkout")?;
    arr(&ucp["services"], "dev.ucp.shopping")
        .iter()
        .find(|s| get_str(s, "transport") == "mcp")
        .map(|s| get_str(s, "endpoint"))
        .filter(|e| e.starts_with("https://"))
}

/// The payment handlers a checkout names (`ucp.payment_handlers`: `shopify.card`, `shop_pay`, …).
pub fn payment_handlers(sc: &Value) -> Vec<String> {
    sc["ucp"]["payment_handlers"]
        .as_object()
        .map(|m| {
            m.values()
                .flat_map(|v| v.as_array().cloned().unwrap_or_default())
                .map(|h| get_str(&h, "id"))
                .filter(|s| !s.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

/// A checkout, normalised: id, status, continue_url, currency, totals (major units by type), lines, the selected
/// shipping option, the messages (type, code, severity, content), the payment handlers, expires_at.
pub fn checkout_of(sc: &Value) -> Value {
    let currency = get_str(sc, "currency").to_uppercase();
    let amount =
        |t: &Value| money(&json!({"amount": t["amount"], "currency": currency})).map(|(a, _)| a);
    let mut totals = Map::new();
    for t in arr(sc, "totals") {
        if let Some(a) = amount(&t) {
            totals.insert(get_str(&t, "type"), json!(a));
        }
    }
    let lines: Vec<Value> = arr(sc, "line_items")
        .iter()
        .map(|l| {
            json!({"title": get_str(&l["item"], "title"), "variant_id": get_str(&l["item"], "id"),
                   "quantity": l["quantity"].as_u64().unwrap_or(1),
                   "price": amount(&json!({"amount": l["item"]["price"]}))})
        })
        .collect();
    let mut shipping = Value::Null;
    for m in arr(&sc["fulfillment"], "methods") {
        for g in arr(&m, "groups") {
            let sel = get_str(&g, "selected_option_id");
            if let Some(opt) = arr(&g, "options").iter().find(|o| get_str(o, "id") == sel) {
                let cost = arr(opt, "totals")
                    .iter()
                    .find(|t| get_str(t, "type") == "total")
                    .and_then(amount);
                shipping = json!({"title": get_str(opt, "title"), "amount": cost});
            }
        }
    }
    let messages: Vec<Value> = arr(sc, "messages")
        .iter()
        .map(|m| {
            json!({"type": get_str(m, "type"), "code": get_str(m, "code"),
                   "severity": get_str(m, "severity"), "content": get_str(m, "content")})
        })
        .collect();
    let status = match get_str(sc, "status") {
        s if s.is_empty() && messages.iter().any(|m| get_str(m, "type") == "error") => {
            "error".to_string()
        }
        s => s,
    };
    json!({"id": get_str(sc, "id"), "status": status, "continue_url": get_str(sc, "continue_url"),
           "currency": currency, "totals": totals, "lines": lines, "shipping": shipping, "messages": messages,
           "payment_handlers": payment_handlers(sc), "expires_at": get_str(sc, "expires_at")})
}

/// The buyer and the delivery address of a checkout, as UCP takes them.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ShipTo {
    pub first_name: String,
    pub last_name: String,
    pub email: String,
    pub phone: String,
    pub street: String,
    pub street2: String,
    pub city: String,
    pub region: String,
    pub postal_code: String,
    /// ISO 3166-1 alpha-2
    pub country: String,
}

impl ShipTo {
    pub fn from_json(v: &Value) -> ShipTo {
        let g = |k: &str| get_str(v, k).trim().to_string();
        ShipTo {
            first_name: g("first_name"),
            last_name: g("last_name"),
            email: g("email"),
            phone: g("phone"),
            street: g("street"),
            street2: g("street2"),
            city: g("city"),
            region: g("region"),
            postal_code: g("postal_code"),
            country: g("country").to_uppercase(),
        }
    }
    pub fn to_json(&self) -> Value {
        json!({"first_name": self.first_name, "last_name": self.last_name, "email": self.email, "phone": self.phone,
               "street": self.street, "street2": self.street2, "city": self.city, "region": self.region,
               "postal_code": self.postal_code, "country": self.country})
    }
}

/// `create_checkout`'s arguments: one line, the buyer, the shipping destination (UCP's field names).
pub fn checkout_request(variant_id: &str, quantity: u64, to: &ShipTo) -> Value {
    let mut buyer = Map::new();
    if !to.email.is_empty() {
        buyer.insert("email".into(), json!(to.email));
    }
    if !to.phone.is_empty() {
        buyer.insert("phone_number".into(), json!(to.phone));
    }
    let mut dest = Map::new();
    for (k, v) in [
        ("first_name", &to.first_name),
        ("last_name", &to.last_name),
        ("phone_number", &to.phone),
        ("street_address", &to.street),
        ("extended_address", &to.street2),
        ("address_locality", &to.city),
        ("address_region", &to.region),
        ("postal_code", &to.postal_code),
        ("address_country", &to.country),
    ] {
        if !v.is_empty() {
            dest.insert(k.into(), json!(v));
        }
    }
    let mut checkout = json!({"line_items": [{"quantity": quantity.max(1), "item": {"id": variant_id}}],
                              "buyer": buyer});
    if !to.country.is_empty() {
        checkout["context"] = json!({"address_country": to.country});
    }
    if !to.street.is_empty() {
        checkout["fulfillment"] =
            json!({"methods": [{"type": "shipping", "destinations": [dest]}]});
    }
    json!({"checkout": checkout})
}

/// `search_catalog`'s arguments: the words, the buyer's country, a price range (major → minor units), how many.
pub fn search_request(
    query: &str,
    country: &str,
    currency: &str,
    min: Option<f64>,
    max: Option<f64>,
    limit: u64,
) -> Value {
    let mut filters = json!({"available": true});
    let mut price = Map::new();
    if let Some(m) = min.filter(|m| *m > 0.0) {
        price.insert("min".into(), json!(minor_of(m, currency)));
    }
    if let Some(m) = max.filter(|m| *m > 0.0) {
        price.insert("max".into(), json!(minor_of(m, currency)));
    }
    if !price.is_empty() {
        filters["price"] = Value::Object(price);
    }
    let mut cat =
        json!({"query": query, "filters": filters, "pagination": {"limit": limit.clamp(1, 40)}});
    if country.len() == 2 {
        // the buyer's market (prices contextualised where the merchant sells there) and only what ships there
        cat["context"] = json!({"address_country": country.to_uppercase()});
        if !currency.is_empty() {
            cat["context"]["currency"] = json!(currency.to_uppercase());
        }
        cat["filters"]["ships_to"] = json!({"country": country.to_uppercase()});
    }
    json!({"catalog": cat})
}

/// Why a checkout cannot be paid by API, or `None` when it can. A purchase is completed by API only with a Token-tier
/// credential allowed to complete purchases AND a payment instrument a listed handler takes: the `shopify.card`
/// handler takes a tokenised card credential (never a card number), `shop_pay` a Shop Pay approval. The Vault holds
/// card numbers, never such a credential — so today the user always pays on the checkout's `continue_url`.
pub fn completion_blocker(
    tier: &str,
    handlers: &[String],
    vault_card: bool,
) -> Option<&'static str> {
    if !vault_card {
        return Some("no payment card in the Vault");
    }
    if tier != "token" {
        return Some("Shopify lets only a Token-tier agent complete a purchase, and our credentials reach merchants' checkouts as Anonymous");
    }
    if !handlers.iter().any(|h| h == "shopify.card") {
        return Some("the store's checkout takes no card handler");
    }
    Some("Shopify's card handler takes a tokenised card credential, and the Vault holds the card number itself")
}

/// The client: the Global Catalog with the token (cached until a minute before it expires), any store's MCP.
pub struct Shopify {
    client: reqwest::Client,
    auth: String,
    catalog: String,
    profile: String,
    client_id: String,
    secret: String,
}

/// (client id, token, valid until)
static TOKEN: Mutex<Option<(String, String, Instant)>> = Mutex::new(None);

const UA: &str = "Mozilla/5.0 (compatible; OMuse/0.6; +https://github.com/second-state/omuse)";

impl Shopify {
    pub fn new(
        client: reqwest::Client,
        auth: &str,
        catalog: &str,
        profile: &str,
        client_id: &str,
        secret: &str,
    ) -> Self {
        Shopify {
            client,
            auth: auth.trim_end_matches('/').to_string(),
            catalog: catalog.trim_end_matches('/').to_string(),
            profile: profile.to_string(),
            client_id: client_id.trim().to_string(),
            secret: secret.trim().to_string(),
        }
    }

    pub fn has_credentials(&self) -> bool {
        !self.client_id.is_empty() && !self.secret.is_empty()
    }

    async fn token(&self) -> Res<String> {
        if let Some((id, t, until)) = TOKEN.lock().clone() {
            if id == self.client_id && Instant::now() < until {
                return Ok(t);
            }
        }
        let r = self
            .client
            .post(format!("{}/auth/access_token", self.auth))
            .timeout(Duration::from_secs(30))
            // without a User-Agent Shopify's edge answers 403 "Access denied" (shopping_real 2026-11-02, from Docker)
            .header("User-Agent", UA)
            .json(
                &json!({"client_id": self.client_id, "client_secret": self.secret,
                          "grant_type": "client_credentials"}),
            )
            .send()
            .await
            .map_err(|e| ShopifyError(format!("token request failed: {e}")))?;
        let st = r.status();
        let v: Value = r.json().await.unwrap_or(Value::Null);
        let t = get_str(&v, "access_token");
        if !st.is_success() || t.is_empty() {
            return Err(ShopifyError(format!(
                "token refused (HTTP {})",
                st.as_u16()
            )));
        }
        // the token lasts 60 minutes; renewed after 55
        *TOKEN.lock() = Some((
            self.client_id.clone(),
            t.clone(),
            Instant::now() + Duration::from_secs(55 * 60),
        ));
        Ok(t)
    }

    async fn call(
        &self,
        endpoint: &str,
        name: &str,
        args: Value,
        token: Option<&str>,
    ) -> Res<Value> {
        let mut rq = self
            .client
            .post(endpoint)
            .timeout(Duration::from_secs(45))
            .header("Accept", "application/json")
            .header("User-Agent", UA)
            .json(&rpc_body(1, name, args, &self.profile));
        if let Some(t) = token {
            rq = rq.bearer_auth(t);
        }
        let r = rq
            .send()
            .await
            .map_err(|e| ShopifyError(format!("{name} failed: {e}")))?;
        let st = r.status();
        let text = r.text().await.unwrap_or_default();
        let body: Value = serde_json::from_str(&text).map_err(|_| {
            ShopifyError(format!(
                "{name}: HTTP {} {}",
                st.as_u16(),
                text.chars().take(160).collect::<String>()
            ))
        })?;
        tool_result(&body)
    }

    /// Products across Shopify merchants (the Global Catalog).
    pub async fn catalog_search(&self, req: Value) -> Res<Vec<Value>> {
        let t = self.token().await?;
        let sc = self
            .call(
                &format!("{}/api/ucp/mcp", self.catalog),
                "search_catalog",
                req,
                Some(&t),
            )
            .await?;
        Ok(products_of(&sc))
    }

    /// One product of the Global Catalog, with `selected` option labels.
    pub async fn catalog_product(&self, id: &str, selected: &[Value], country: &str) -> Res<Value> {
        let t = self.token().await?;
        let mut cat = json!({"id": id});
        if !selected.is_empty() {
            cat["selected"] = json!(selected);
            cat["preferences"] = json!(selected
                .iter()
                .map(|s| get_str(s, "name"))
                .collect::<Vec<_>>());
        }
        if country.len() == 2 {
            cat["context"] = json!({"address_country": country.to_uppercase()});
        }
        let sc = self
            .call(
                &format!("{}/api/ucp/mcp", self.catalog),
                "get_product",
                json!({"catalog": cat}),
                Some(&t),
            )
            .await?;
        product_of(&sc).ok_or_else(|| ShopifyError("get_product: no product".into()))
    }

    /// A store's UCP checkout endpoint (its `/.well-known/ucp`), `None` when the store offers no UCP checkout.
    pub async fn store_endpoint(&self, origin: &str) -> Res<Option<String>> {
        let r = self
            .client
            .get(format!("{}/.well-known/ucp", origin.trim_end_matches('/')))
            .timeout(Duration::from_secs(20))
            .header("Accept", "application/json")
            .header("User-Agent", UA)
            .send()
            .await
            .map_err(|e| ShopifyError(format!("the store did not answer: {e}")))?;
        if !r.status().is_success() {
            return Ok(None);
        }
        let v: Value = r.json().await.unwrap_or(Value::Null);
        Ok(checkout_endpoint(&v))
    }

    /// A store's own catalog search (anonymous).
    pub async fn store_search(&self, endpoint: &str, req: Value) -> Res<Vec<Value>> {
        Ok(products_of(
            &self.call(endpoint, "search_catalog", req, None).await?,
        ))
    }

    /// A store's own product (anonymous), with `selected` option labels.
    pub async fn store_product(&self, endpoint: &str, id: &str, selected: &[Value]) -> Res<Value> {
        let mut cat = json!({"id": id});
        if !selected.is_empty() {
            cat["selected"] = json!(selected);
            cat["preferences"] = json!(selected
                .iter()
                .map(|s| get_str(s, "name"))
                .collect::<Vec<_>>());
        }
        let sc = self
            .call(endpoint, "get_product", json!({"catalog": cat}), None)
            .await?;
        product_of(&sc).ok_or_else(|| ShopifyError("get_product: no product".into()))
    }

    /// A checkout session on the store (anonymous: the Global API token is refused by merchants' Checkout MCP).
    pub async fn create_checkout(&self, endpoint: &str, req: Value) -> Res<Value> {
        let sc = self.call(endpoint, "create_checkout", req, None).await?;
        Ok(checkout_of(&sc))
    }
}

#[cfg(test)]
mod tests {
    //! On responses recorded from the real services on 2026-11-02 (`shopify_recorded/`, trimmed of images and
    //! descriptions; the buyer data in them is the suites' clearly fake profile). Recorded data, labelled as such —
    //! not a stand-in service.
    use super::*;

    fn rec(name: &str) -> Value {
        let s = match name {
            "catalog_search" => include_str!("shopify_recorded/catalog_search.json"),
            "catalog_get_product" => include_str!("shopify_recorded/catalog_get_product.json"),
            "create_checkout" => include_str!("shopify_recorded/create_checkout.json"),
            "create_checkout_no_address" => {
                include_str!("shopify_recorded/create_checkout_no_address.json")
            }
            "create_checkout_bad_variant" => {
                include_str!("shopify_recorded/create_checkout_bad_variant.json")
            }
            "update_checkout_invalid_args" => {
                include_str!("shopify_recorded/update_checkout_invalid_args.json")
            }
            "well_known_ucp" => include_str!("shopify_recorded/well_known_ucp.json"),
            "token_rejected" => include_str!("shopify_recorded/token_rejected.json"),
            _ => unreachable!(),
        };
        serde_json::from_str(s).unwrap()
    }

    /// The token request names us (User-Agent): Shopify's edge refuses one without (403 "Access denied" — every
    /// shopping_real search said "token refused" on 2026-11-02).
    #[tokio::test]
    async fn the_token_request_carries_a_user_agent() {
        use axum::{http::HeaderMap, routing::post, Json, Router};
        let app = Router::new().route(
            "/auth/access_token",
            post(|h: HeaderMap| async move {
                if h.get("user-agent").is_none() {
                    return (axum::http::StatusCode::FORBIDDEN, Json(json!({})));
                }
                (
                    axum::http::StatusCode::OK,
                    Json(json!({"access_token": "t", "token_type": "Bearer"})),
                )
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let s = Shopify::new(
            reqwest::Client::new(),
            &base,
            &base,
            "",
            "ua-test-id",
            "secret",
        );
        assert_eq!(s.token().await.unwrap(), "t");
    }

    #[test]
    fn catalog_products_carry_price_rating_seller_and_links() {
        let ps = products_of(&tool_result(&rec("catalog_search")).unwrap());
        assert_eq!(ps.len(), 4);
        let p = &ps[0];
        assert_eq!(
            p["title"],
            "Men's Wool Runner - Natural Grey (Light Grey Sole)"
        );
        assert_eq!(p["price"], 110.0);
        assert_eq!(p["currency"], "USD");
        assert_eq!(p["rating"], 4.6);
        assert_eq!(p["ratings"], 7064);
        assert_eq!(p["seller"]["domain"], "weareallbirds.myshopify.com");
        assert_eq!(p["seller"]["url"], "https://www.allbirds.com");
        assert!(get_str(p, "variant_id").starts_with("gid://shopify/ProductVariant/"));
        assert!(get_str(p, "checkout_url").starts_with("https://weareallbirds.myshopify.com/cart/"));
        // a product without ratings has none
        assert!(ps.iter().any(|p| p.get("rating").is_none()));
    }

    #[test]
    fn a_product_lists_its_options_with_availability_and_the_selected_variant() {
        let p = product_of(&tool_result(&rec("catalog_get_product")).unwrap()).unwrap();
        let size = p["options"]
            .as_array()
            .unwrap()
            .iter()
            .find(|o| o["name"] == "Size")
            .unwrap();
        let labels: Vec<&str> = size["values"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v["label"].as_str().unwrap())
            .collect();
        assert_eq!(labels, ["9", "10", "11", "13"]);
        assert_eq!(p["selected"][1], json!({"name": "Size", "label": "10"}));
        assert_eq!(p["variants"][0]["available"], true);
    }

    #[test]
    fn a_checkout_with_the_address_has_totals_shipping_and_the_continue_url() {
        let c = checkout_of(&tool_result(&rec("create_checkout")).unwrap());
        assert_eq!(c["status"], "requires_escalation");
        assert!(
            get_str(&c, "continue_url").starts_with("https://weareallbirds.myshopify.com/cart/c/")
        );
        assert_eq!(c["totals"]["subtotal"], 105.0);
        assert_eq!(c["totals"]["tax"], 7.61);
        assert_eq!(c["totals"]["total"], 112.61);
        assert_eq!(c["shipping"]["title"], "Ground Shipping");
        assert_eq!(c["shipping"]["amount"], 0.0);
        assert_eq!(c["lines"][0]["quantity"], 1);
        assert_eq!(c["messages"][0]["code"], "extension_interaction_required");
        assert_eq!(c["payment_handlers"], json!(["shopify.card", "shop_pay"]));
    }

    #[test]
    fn a_checkout_without_the_address_says_what_is_missing() {
        let c = checkout_of(&tool_result(&rec("create_checkout_no_address")).unwrap());
        assert_eq!(c["status"], "requires_escalation");
        let codes: Vec<&str> = c["messages"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| m["code"].as_str().unwrap())
            .collect();
        assert!(codes.contains(&"delivery_address_required"));
    }

    #[test]
    fn an_unknown_variant_is_an_error_status_with_the_shops_reason() {
        let c = checkout_of(&tool_result(&rec("create_checkout_bad_variant")).unwrap());
        assert_eq!(c["status"], "error");
        assert_eq!(c["messages"][0]["code"], "invalid");
        assert!(!get_str(&c, "continue_url").contains("/cart/c/"));
    }

    #[test]
    fn invalid_arguments_and_a_refused_token_are_errors() {
        let e = tool_result(&rec("update_checkout_invalid_args")).unwrap_err();
        assert!(e.0.contains("missing required properties"));
        let e = tool_result(&rec("token_rejected")).unwrap_err();
        assert_eq!(e.0, "AuthenticationFailed: Invalid global access token.");
    }

    #[test]
    fn the_stores_checkout_endpoint_comes_from_its_well_known_ucp() {
        assert_eq!(
            checkout_endpoint(&rec("well_known_ucp")).as_deref(),
            Some("https://weareallbirds.myshopify.com/api/ucp/mcp")
        );
        assert_eq!(checkout_endpoint(&json!({"ucp": {"services": {}}})), None);
    }

    #[test]
    fn the_checkout_request_has_ucps_fields() {
        let to = ShipTo {
            first_name: "OMUSE".into(),
            last_name: "TESTER".into(),
            email: "omuse.e2e.tester@example.com".into(),
            phone: "+14155550123".into(),
            street: "1 Test Street".into(),
            street2: "Apt 2".into(),
            city: "San Francisco".into(),
            region: "CA".into(),
            postal_code: "94105".into(),
            country: "US".into(),
        };
        let r = checkout_request("gid://shopify/ProductVariant/41243415740496", 1, &to);
        assert_eq!(
            r["checkout"]["line_items"][0]["item"]["id"],
            "gid://shopify/ProductVariant/41243415740496"
        );
        assert_eq!(
            r["checkout"]["buyer"]["email"],
            "omuse.e2e.tester@example.com"
        );
        let d = &r["checkout"]["fulfillment"]["methods"][0]["destinations"][0];
        assert_eq!(d["street_address"], "1 Test Street");
        assert_eq!(d["address_country"], "US");
        assert_eq!(
            r["checkout"]["fulfillment"]["methods"][0]["type"],
            "shipping"
        );
        let body = rpc_body(3, "create_checkout", r, "https://x/profile.json");
        assert_eq!(
            body["params"]["arguments"]["meta"]["ucp-agent"]["profile"],
            "https://x/profile.json"
        );
    }

    #[test]
    fn prices_go_to_minor_units_and_back() {
        let r = search_request("wool runners", "US", "USD", None, Some(150.0), 6);
        assert_eq!(r["catalog"]["filters"]["price"]["max"], 15000);
        assert_eq!(
            money(&json!({"amount": 1500, "currency": "JPY"})),
            Some((1500.0, "JPY".into()))
        );
        assert_eq!(
            money(&json!({"amount": 1500, "currency": "usd"})),
            Some((15.0, "USD".into()))
        );
    }

    #[test]
    fn a_singapore_search_ships_to_sg_and_prices_convert_into_sgd() {
        let r = search_request("running shoes", "SG", "SGD", None, Some(150.0), 10);
        assert_eq!(r["catalog"]["filters"]["ships_to"]["country"], "SG");
        assert_eq!(
            r["catalog"]["context"],
            json!({"address_country": "SG", "currency": "SGD"})
        );
        let sc: Value =
            serde_json::from_str(include_str!("shopify_recorded/catalog_search_sg.json")).unwrap();
        let mut ps = products_of(&tool_result(&sc).unwrap());
        // the merchants price in their own currencies: USD, GBP, SGD …
        let curs: Vec<String> = ps.iter().map(|p| get_str(p, "currency")).collect();
        assert!(
            curs.contains(&"SGD".to_string()) && curs.contains(&"GBP".to_string()),
            "{curs:?}"
        );
        let syms = fx_symbols(&ps, "SGD");
        assert!(
            syms.contains(&"GBPSGD=X".to_string()) && !syms.iter().any(|s| s.starts_with("SGD"))
        );
        let rates = vec![
            json!({"symbol": "GBPSGD=X", "last": 1.7}),
            json!({"symbol": "USDSGD=X", "last": 1.3}),
        ];
        localise(&mut ps, "SGD", &rates);
        let sgd = ps.iter().find(|p| p["currency"] == "SGD").unwrap();
        assert_eq!(sgd["price_local"], sgd["price"]);
        assert!(sgd.get("fx_rate").is_none());
        let gbp = ps.iter().find(|p| p["currency"] == "GBP").unwrap();
        assert_eq!(gbp["currency_local"], "SGD");
        assert_eq!(
            gbp["price_local"].as_f64().unwrap(),
            (gbp["price"].as_f64().unwrap() * 1.7 * 100.0).round() / 100.0
        );
        // no rate for a currency: no local price (never a guess)
        assert!(ps
            .iter()
            .filter(|p| p["currency"] == "EUR")
            .all(|p| p.get("price_local").is_none()));
    }

    #[test]
    fn a_purchase_is_never_completed_by_api_with_our_tier_or_a_raw_vault_card() {
        let handlers = vec!["shopify.card".to_string(), "shop_pay".to_string()];
        assert!(completion_blocker(CHECKOUT_TIER, &handlers, true)
            .unwrap()
            .contains("Anonymous"));
        assert_eq!(
            completion_blocker(CHECKOUT_TIER, &handlers, false),
            Some("no payment card in the Vault")
        );
        assert!(completion_blocker("token", &handlers, true)
            .unwrap()
            .contains("tokenised"));
        assert!(completion_blocker("token", &[], true)
            .unwrap()
            .contains("no card handler"));
    }
}
