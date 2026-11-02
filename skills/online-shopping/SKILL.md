---
name: online-shopping
description: 网购（购物助手）：用 shop_order 按用户所在地通过 API 搜索商店（先是 Shopify 全球商品目录），代码筛选排序后把商品以 JSON 给你，你选出最合适的几个（offer_id），代码显示选择卡（本地货币价格、商家、链接）；不用浏览器 Shop online (the Shopping bot) — call shop_order: code searches the stores for the user's location by API (Shopify's Global Catalog first), filters and ranks the offers and gives them to you as JSON; you pick the few that fit best by their offer_ids and code shows them as a choice card (price in the user's currency, merchant, link) — no browser.
form: on
browser: off
---
# Online shopping / 网购（搜索、挑选）

## Any product request → shop_order (by API, no browser)
Call `shop_order` with the user's words as JSON arguments — never search or open shops' pages yourself:
- `query` = the product words ("running shoes"); `stores` = only the stores or merchants the user named ("Shopify",
  "Amazon", "Walmart"; else empty: code picks the stores for the user's location); `brands` = the brands the user
  named ("Nike"; else empty) — a brand or a merchant named also searches Google Shopping, which costs a search credit,
  so fill them only from the user's own words; `country` = the ISO code of where to ship ONLY when the user named a place
  ("ship it to Japan" → JP) — else empty: code uses the profile's home country, and asks when there is none; `max_price` / `min_price` in the user's currency; `by` = `rating`, `price` or
  `ratings`; `min_stars`, `min_ratings`, `count` (how many to show), `title_has` (words every chosen name has),
  `title_not`; `action` = `compare` (find / show / which is best) or `buy`.
- The result `review`: the offers code found and ranked, each with its `offer_id`, price in the user's currency (and
  the shop's own price when converted), rating, merchant and link. Read them and call `shop_order` again with the same
  arguments plus `choices` = the offer_ids of the few offers that best fit the request (the product the user asked for,
  not an accessory; the criteria), best first. Code shows them as a choice card (`shown`).
- `pick` = the product the user chose on a card ("I choose: …"), with the same `query`: the result `link` is the
  merchant's page, where the user buys it — nothing is bought here.
- `nothing_found`: say what was searched and what was left out (code appends it). `needs_input` / `invalid`: ask the
  user / fix the arguments. The facts in `show` are appended to the reply by code.
- A shop's link the user gave: only stores searched by API are searched (Shopify merchants for now) — say so
  (`not_covered`); never open a shop's page or use the browser.

## Never
- No browser: never open, read or click a shop's page, never search the web for products — everything goes through
  `shop_order`. Nothing is bought or paid here; a purchase is the user's, on the merchant's page.
