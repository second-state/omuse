# Google Shopping through SerpApi (the Money bot's shopping opt-in provider)

Walked 2026-11-02 with the e2e SerpApi key: two requests, recorded (trimmed) in
`crates/omuse-apps/src/serpapi_recorded/`.

## When it is used

Only when the model's JSON names a brand (`brands`) or a merchant that is not one of our providers (`stores`:
Amazon, Walmart …): each search costs a SerpApi credit. A plain request ("running shoes under S$150") stays on
Shopify's Global Catalog (`omuse_apps::shops::providers_for`).

## 1. Search — `engine=google_shopping`

| parameter | value |
|---|---|
| `q` | the brands named before the product words (`SearchReq::words`) |
| `gl` | the user's country, lower case (`sg`) |
| `location` | the user's home city by name (`Singapore`), when known; else omitted (the country is searched) |
| `hl` | `en` |
| `min_price` / `max_price` | whole units of the country's currency |

`start` and `num` are ignored by SerpApi (one page of about 40). `shopping_results[]`: `title`, `price` ("$179.00" —
no currency code: code takes the country's), `extracted_price`, `source` (the seller), `rating`, `reviews`,
`delivery` (text, often absent), `product_link` (Google's page for the query, NOT the store's),
`immersive_product_page_token` (for step 2). Singapore, "Nike running shoes": 40 offers from iRUN Singapore and
Nike Official (SGD).

**Delivery:** there is no ship-to parameter. Results are what Google lists for a searcher in that city / country;
whether a store delivers to the user's address is confirmed on its checkout. The answer says so.

## 2. A pick's stores — `engine=google_immersive_product`

`page_token` = the offer's token, `gl`. `product_results.stores[]`: `name`, `link` (the store's own page),
`extracted_price`, `shipping_extracted`, `extracted_total`, `details_and_offers` ("In stock online", "Free delivery
on orders of $80+", "14-day returns"). One request per pick, never per result. Nike Vomero Plus in Singapore: iRUN
Singapore S$179 + S$5, ZALORA Singapore S$220, Nike Official S$249, JD Sports Singapore S$249.

Both requests pass `omuse_apps::serpapi::valid_request` before they are sent.
