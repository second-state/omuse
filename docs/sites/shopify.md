# Shopify by API — the map (recorded 2026-11-02)

The Money bot's shopping reaches Shopify **by API only**: no store page is opened in the agent's browser and no form is filled.
Every shape below was recorded from the real services on 2026-11-02 (clearly fake buyer data: OMUSE TESTER,
`omuse.e2e.tester@example.com`, `+1 415 555 0123`, 1 Test Street Apt 2, San Francisco CA 94105; nothing paid, never
`complete_checkout`). Trimmed copies of the responses are the unit tests' recorded data:
`crates/omuse-apps/src/shopify_recorded/` (labelled as recorded, not a stand-in service).

Code: `crates/omuse-apps/src/shopify.rs` (client, normalisers, request builders), `crates/omuse-apps/src/shops.rs` (the
store-provider interface: providers by location, in parallel, merged), Sentinel `shop_search`
(`crates/omuse-sentinel/src/actions.rs`), the machine `crates/omuse-runtime/src/agent/booking/shopping.rs` behind the
model's `shop_order` (the Money bot's shopping, `bots/money.md`).

## 1. Authentication and trust tier

| what | request | answer |
|---|---|---|
| token | `POST https://api.shopify.com/auth/access_token` JSON `{"client_id", "client_secret", "grant_type": "client_credentials"}` | `200 {"access_token": "<JWT, 462 chars>", "token_type": "Bearer"}` |

- The token is an ES256 JWT; its claims (decoded, the token itself never printed): `iss https://api.shopify.com`,
  `exp = iat + 3600` (60 minutes; code renews after 55), `scopes: "write_global_api_app_events"`,
  `limits: {"catalog": {"max": 5, "period": 1}}` — 5 Catalog requests per second.
- **Trust tier (recorded, not guessed):** the token is a *global API token*. The Global Catalog takes it (Token tier:
  `Authorization: Bearer <token>`). A merchant's Checkout MCP **refuses it**:
  `create_checkout` on `weareallbirds.myshopify.com/api/ucp/mcp` with the bearer token, with and without a
  `Shopify-Buyer-IP` header → `401 {"jsonrpc":"2.0","id":9,"error":{"code":-32000,"message":"AuthenticationFailed","data":"Invalid global access token."}}`.
  Without credentials the same call works — so on merchants' checkouts OMuse is **Anonymous**. Shopify's tier table
  (shopify.dev/docs/agents/profiles/auth-and-rate-limiting): Anonymous = Signed = catalog, cart and checkout building;
  `complete_checkout` and order tools are **not available**; Token = all, `complete_checkout` only "when the token is
  granted permission to complete purchases". Our scope has no such permission.
- Every UCP request carries `meta.ucp-agent.profile` (required). OMuse sends Shopify's published example profile
  `https://shopify.dev/ucp/agent-profiles/examples/2026-08-25/valid-with-capabilities.json` (`[endpoints]
  shopify_agent_profile`) until it hosts its own.
- Transport: one JSON-RPC 2.0 `tools/call` per request, `Content-Type: application/json`, no MCP handshake needed;
  answers are plain JSON (`application/json`), the data in `result.structuredContent`.

## 2. Global Catalog — `https://catalog.shopify.com/api/ucp/mcp` (Token)

`tools/list` → `search_catalog`, `get_product`, `lookup_catalog`.

### search_catalog

```json
{"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": "search_catalog", "arguments": {
  "meta": {"ucp-agent": {"profile": "<profile url>"}},
  "catalog": {"query": "running shoes",
              "context": {"address_country": "SG", "currency": "SGD"},
              "filters": {"available": true, "ships_to": {"country": "SG"}, "price": {"max": 15000}},
              "pagination": {"limit": 10}}}}}
```

- Location parameters the Catalog has (input schema from `tools/list`): `context` {`address_country` (ISO 3166-1
  alpha-2), `address_region`, `postal_code`, `language`, `currency` (ISO 4217), `intent`}; `filters` {`ships_to`
  {`country`, `region`, `postal_code`} — only variants that ship there, `ships_from`, `available` (default true),
  `price` {`min`, `max`} in **minor units**, `categories`, `shops` (Shop GIDs), `condition`, `attributes` (Color, Size,
  Target gender), `rating`}; `pagination` {`limit`, `cursor`}.
- Answer `structuredContent`: `ucp` {version `2026-08-25`, capabilities}, `products[]`, `messages[]`, `pagination`
  {`has_next_page`, `total_count`, `cursor`}. A product: `id` (`gid://shopify/p/…`), `title`, `handle`, `description`,
  `rating` {`value`, `scale_min` 1, `scale_max` 5, `count`} (absent for some), `media[]`, `options[]` {`name`,
  `values[]{label}`}, `price_range` {`min`, `max`: `{amount, currency}`}, `variants[]` (search gives the featured one):
  `id` (`gid://shopify/ProductVariant/<n>` — the merchant's own variant id), `title`, `url` (the merchant's product
  page with `?variant=…&_gsid=…&utm_source=shopify&utm_medium=catalog`), `price` / `list_price` `{amount, currency}`
  (minor units), `availability` {`available`}, `options[]` {`name`, `label`}, `seller` {`id` `gid://shopify/Shop/<n>`,
  `name`, `url` (the storefront), `domain` (`<shop>.myshopify.com`), `links[]` (policies)}, `checkout_url` (the cart
  permalink `https://<shop>/cart/<variant>:1?…`), `rating`, `condition`, `eligible` {`native_checkout`: false on every
  offer recorded, Allbirds included — and Allbirds' UCP checkout works anyway}, `source` "shopify".
- **Localisation, recorded for SG** ("running shoes", ships_to SG, context SG/SGD, price max 15000): every offer ships
  to SG, but merchants price in their own markets' currencies — SGD (League Outfitters 124.00, Tracksmith 160.00), USD
  (Xero Shoes 99.99), GBP (Start Fitness), EUR (Elverys, bshopzone). `messages`: `price_filter_applied` — "Price
  filtering was applied on a USD basis; returned prices are contextualized to the buyer's market and may differ"
  (`data.applied_usd.max` 117.30, `conversion.max: fx_converted`). So the price filter is approximate: Tracksmith at
  SGD 160 passed a SGD 150 cap. **Code therefore converts every price into the user's currency** (the merchant's own
  price when it is already in it, else Yahoo's market rate `GBPSGD=X` …, `shopify::localise`) and applies the range on
  that (`booking::shopping::ranked`); a price with no rate is not compared (left out under a price cap).
- Without the token the Catalog still answers (Anonymous, the shared unauthenticated limit — not for production).
- `messages` on a zero-hit or filtered search carry `type`, `code`, `path`, `content`.

### get_product / lookup_catalog

`get_product` `{"catalog": {"id": "gid://shopify/p/…", "selected": [{"name": "Size", "label": "10"}], "preferences":
["Size"], "context": {"address_country": "US"}}}` → `structuredContent.product`: `options[]` with
`values[]{label, available, exists}`, `selected[]` {`name`, `label`} (the anchor the server chose), `variants[]` (the
selected one; its `availability`, `price`, `checkout_url`, `seller`), `rating`, `price_range`. `lookup_catalog`
resolves up to 50 ids (`gid://shopify/p/…` or `gid://shopify/ProductVariant/…`); unresolved ids come back in
`messages`.

## 3. A merchant's UCP — `https://<shop>.myshopify.com/api/ucp/mcp` (Anonymous)

- Discovery: `GET https://<storefront or myshopify domain>/.well-known/ucp` → `ucp.services["dev.ucp.shopping"][]`
  (`transport: "mcp"`, `endpoint: "https://weareallbirds.myshopify.com/api/ucp/mcp"`), `capabilities`
  (`dev.ucp.shopping.checkout`, `.cart`, `.fulfillment`, `.discount`, `.buyer_consent`, `.order`, catalog search /
  lookup), `payment_handlers` (`com.google.pay`, `dev.shopify.card` id `shopify.card`, `dev.shopify.shop_pay` id
  `shop_pay`). Recorded on www.allbirds.com, www.brooklinen.com, www.tentree.com and gymshark.myshopify.com (Gymshark's
  headless storefront www.gymshark.com answers 404 HTML for `/.well-known/ucp`: discovery goes to the seller's
  `myshopify.com` domain from the Catalog's `seller.domain`).
- `tools/list` (no credentials): `get_checkout`, `create_checkout`, `update_checkout`, `complete_checkout`,
  `cancel_checkout`, `get_cart`, `create_cart`, `update_cart`, `cancel_cart`, `get_order`, `ask_business`,
  `search_catalog`, `lookup_catalog`, `get_product`, `search_locations`, `lookup_locations`.

### create_checkout (recorded on Allbirds, no payment)

```json
{"checkout": {
  "line_items": [{"quantity": 1, "item": {"id": "gid://shopify/ProductVariant/41243415740496"}}],
  "buyer": {"email": "omuse.e2e.tester@example.com", "phone_number": "+14155550123"},
  "context": {"address_country": "US"},
  "fulfillment": {"methods": [{"type": "shipping", "destinations": [{"first_name": "OMUSE", "last_name": "TESTER",
     "phone_number": "+14155550123", "street_address": "1 Test Street", "extended_address": "Apt 2",
     "address_locality": "San Francisco", "address_region": "CA", "postal_code": "94105", "address_country": "US"}]}]}}}
```

Answer (`structuredContent`): `id` `gid://shopify/Checkout/<token>?key=…`, `status` **`requires_escalation`**,
`continue_url` `https://weareallbirds.myshopify.com/cart/c/<token>?key=…`, `currency` USD, `totals[]` {`type`
subtotal 10500 · fulfillment 0 · tax 761 · total 11261, `display_text`}, `line_items[]` {`id` CartLine gid, `item`
{`id`, `title`, `price`}, `quantity`, `totals`}, `buyer` (with Shopify's consent defaults), `fulfillment.methods[]`
{`id`, `type` shipping, `line_item_ids`, `selected_destination_id`, `destinations[]` (with ids), `groups[]` {`id`,
`selected_option_id`, `options[]` {`id`, `title` "Ground Shipping", `totals`}}} — the cheapest option is selected by the
store — `payment` {`instruments`: []}, `ucp.payment_handlers` (`shopify.card` with `enabled_card_brands`, `shop_pay`
with `shop_id`), `expires_at` (30 days), `links[]` (policies), `messages[]`:
`{"type": "error", "code": "extension_interaction_required", "severity": "requires_buyer_input", "content": "An
extension interaction is required to complete the checkout."}`.

- **The `continue_url` opens Shopify's own checkout already filled** (walked in a browser once to confirm the hand-off,
  2026-11-02): `https://www.allbirds.com/checkouts/cn/<token>/en-us` shows the email, the country, first / last name,
  address, apartment, city, state and ZIP from the API, "Ground Shipping" selected, and the Pay now button
  (`#checkout-pay-button`) with the total — the user's step.
- Without the address: `status requires_escalation`, `messages` `extension_interaction_required`,
  `delivery_address_required` (recoverable), `delivery_no_delivery_available` (recoverable); totals without shipping.
- A variant that does not exist: `result.isError: true` with `structuredContent` (no `status`), `messages[0]`
  `{"type": "error", "code": "invalid", "severity": "unrecoverable", "path": "$.line_items[0].item.id", "content": "The
  merchandise with id gid://shopify/ProductVariant/1 does not exist."}`; `continue_url` is the store's home page.
- `update_checkout` replaces, it does not merge: `{"id", "checkout": {"buyer", "fulfillment"}}` without `line_items`
  and the methods' `line_item_ids` → `isError` text "Invalid arguments: object at `/checkout/fulfillment/methods/0` is
  missing required properties: line_item_ids, object at `/checkout` is missing required properties: line_items".
  Code builds the whole checkout in one `create_checkout`.
- Statuses (shopify.dev/docs/agents/checkout): `incomplete`, `requires_escalation`, `ready_for_complete` are
  non-terminal and carry `continue_url`; escalation = hand the buyer the prefilled checkout.

### Paying

- `complete_checkout` needs `meta.idempotency-key` and `checkout.payment.instruments[]` `{id, handler_id, type,
  credential{type, token …}, billing_address}`: a **tokenised** credential from a payment handler (`shopify.card`'s
  card credential, a Shop Pay approval, Google Pay's token) — never a card number. OMuse's Vault holds card numbers
  only, and our tier cannot call `complete_checkout` at all. So **the user always pays on the `continue_url`**; the
  result says why (`omuse_apps::shopify::completion_blocker`). Never called in any test or run.

## 4. The store's own JSON endpoints (recorded, not used)

Recorded on Allbirds, Brooklinen and Tentree for reference only — shopping uses the APIs above:
`/products.json?limit=250` and `/collections/<c>/products.json` (`products[]` with `variants[]{id, title,
option1..3, price "105.00", available}` — no currency, no ratings; a hidden collection answers `{"products": []}`;
curl got Cloudflare `429 local_rate_limited` on `/products.json` while the browser got 200), `/products/<handle>.js`
(prices in cents, `options[]{name, values}`, `variants[]{available, options}`), `/search/suggest.json?q=…&resources[type]=product`,
`/cart/add.js` POST `{items:[{id, quantity}]}`, `/cart.js` (`token`, `item_count`, `total_price`, `currency`), and
the checkout page's fixed field names (`#email`, `countryCode`, `firstName`, `lastName`, `#shipping-address1`,
`address2`, `city`, `zone`, `postalCode`, `#marketing_opt_in`, hidden `autofill_*` duplicates; the card fields are
iframes `card-fields-number-…`; `#checkout-pay-button`). Allbirds' colours are separate products (one option, Size);
Tentree and Brooklinen use Color + Size.
