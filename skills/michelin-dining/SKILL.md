---
name: michelin-dining
description: 米其林餐厅：用旅行数据连接器查米其林星级/必比登餐厅和可订时段，比较后只在最后一步用浏览器订位（东京、大阪、首尔、新加坡、香港、曼谷、台北、上海…）Find MICHELIN-starred and Bib Gourmand restaurants in Asian cities, check real availability and book a table — data connector first, browser only to book.
form: on
---
# MICHELIN dining / 米其林订位

Data first: `places_search` / `place_details` (Google Maps data — rating, price, hours, phone, website, menu or
reservation links with the SerpApi key of Settings → Connections → Flights & hotels; without it, or when Google finds
nothing, places_search falls back to web search by itself) is the only search call — **no `web_search` beside it** —
and comes before any browsing. Book on the restaurant's own site or the platform the user names; the browser
is for that booking page only.


**No plan for the search.** Searching is just searching: never call plan_task to search, compare or check a date range — one turn of search calls, then the answer. Plan only once one result has been picked and booking it takes several steps (and a "book it" follow-up goes straight to the booking page).

## 0. Pick the source (no browser yet)
- Look for a travel data connector: `mcp_<server>__michelin_search` and `mcp_<server>__restaurant_availability`
  (e.g. `mcp_travel__michelin_search`). If present, use them for finding restaurants and checking times — no guide
  website, no Google Maps, no search engine.
- No connector: web_search + browser_read (guide.michelin.com pages, no snapshots) to find candidates, then the
  restaurant-booking skill for availability.

## 1. Understand the request
- City, date (exact YYYY-MM-DD; say the weekday — many places close one or two days a week), time window, party size,
  distinction wanted, cuisine, budget per person, dietary needs / occasion.
- Distinctions: **3 Stars** = exceptional, worth a special journey; **2 Stars** = excellent, worth a detour;
  **1 Star** = high-quality cooking; **Bib Gourmand** = good food at moderate prices — it is NOT a star. Never call a
  Bib Gourmand place "starred". "Michelin-starred" means 1–3 Stars only.
- Prices are per person in local currency (JPY, KRW, SGD, HKD, THB, TWD, CNY); quote them as given, with the currency.
  Times are local restaurant time.

## 2. Find and check (data tools)
- One `michelin_search(city, distinction, cuisine)` call. Pick 2–4 candidates that fit (closed days, price, cuisine).
- `restaurant_availability(restaurant_id, date, party_size)` for each candidate (up to 4 calls). Only a slot returned
  here counts as available; if a restaurant is closed or full, use its next_available dates as an alternative.
- Note **deposits** (charged now and refundable until when), card guarantees / no-show fees, seating time limits and
  dress code. Never invent times or prices.

## 3. Show the options
- present_choices(kind="comparison"): label = restaurant `name` exactly, details = exact strings (`distinction`,
  `cuisine`, `price_range`, slot `time`, `deposit_policy`), source_url = the restaurant's or slot's `booking_url`.
  Recommendation in note.
- If the user already chose ("book the 2-star kaiseki at 19:00"), continue to step 4; otherwise stop and wait.

## 4. Guest details
- Name, email, phone (request → memory / profile_get → ask once for what is missing). Dietary needs / occasion go in
  the requests field.
- A deposit or card guarantee needs a card: vault_list for it (code fills it from the Vault). Never ask for card
  numbers in chat. No vault card → the user types it in the hand-over.

## 5. Book (browser — only the booking page)
1. If money is charged now (a deposit): purchase_confirm once — site = booking page domain, items =
   [{name: "<restaurant> <date> <time>, <n> guests – deposit", price: deposit total}], total = deposit due now,
   currency = the local currency, delivery = "Table reservation", card_item_id, note = cancellation/refund rule.
   No deposit → skip purchase_confirm.
2. Code opens the booking link (the slot's booking_url) and fills it on known sites — name, email, phone, requests and
   the card from the Vault (form_fill is called by code, not by you). A site with no form function goes to the
   `browser-use` skill.
3. **The last click is the user's**: never press "Confirm booking" (Sentinel refuses it). Once code has filled the page
   (a card saved by name only, or no card in the Vault: the card boxes stay empty), call
   browser_request_takeover with the reason "The booking page is filled in: check it (type the card for the deposit if it is empty), press Confirm booking yourself and
   hand the browser back". The chat shows the user a link to take over.
4. After the hand-back, read the confirmation from one browser_snapshot. No confirmation → the booking is not made:
   say so and that the page stays open for them.

## 6. Answer
- Booking reference exactly as shown, restaurant + distinction, date (weekday), time, party size, deposit charged and
  until when it is refundable, seating limit, dress code. If Google Calendar is connected, offer an event
  ("🍽 <restaurant>", 2 h, reference in the description). No reference → not confirmed.
