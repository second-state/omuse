---
name: michelin-dining
description: 米其林餐厅：用旅行数据连接器查米其林星级/必比登餐厅和可订时段，比较后只在最后一步用浏览器订位（东京、大阪、首尔、新加坡、香港、曼谷、台北、上海…）Find MICHELIN-starred and Bib Gourmand restaurants in Asian cities, check real availability and book a table — data connector first, browser only to book.
---
# MICHELIN dining / 米其林订位

## 0. Pick the source (no browser yet)
- Look for a travel data connector: `mcp_<server>__michelin_search` and `mcp_<server>__restaurant_availability`
  (e.g. `mcp_travel__michelin_search`). If present, use them for finding restaurants and checking times — no guide
  website, no Google Maps, no search engine.
- No connector: browser_search + browser_read (guide.michelin.com pages, no snapshots) to find candidates, then the
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
- A deposit or card guarantee needs a card: vault_list → browser_fill_secret (number, expiry, cvc, holder). Never type
  or ask for card numbers in chat. No vault card → browser_request_takeover for the card fields.

## 5. Book (browser — only the booking page)
1. If money is charged now (a deposit): purchase_confirm once — site = booking page domain, items =
   [{name: "<restaurant> <date> <time>, <n> guests – deposit", price: deposit total}], total = deposit due now,
   currency = the local currency, delivery = "Table reservation", card_item_id, note = cancellation/refund rule.
   No deposit → skip purchase_confirm.
2. browser_navigate(slot booking_url) → the form with refs; check date, time, party and deposit match.
3. browser_type name, email, phone, requests (one turn); vault fills for the card.
   Send each browser_fill_secret as its own turn (one call, nothing else in that turn): every vault fill can wait for the user's approval, and a turn should hold at most one call that may wait.
4. browser_click "Confirm booking" once (Sentinel handles approval).
5. Read the confirmation from the click result; one browser_snapshot only if it is missing.

## 6. Answer
- Booking reference exactly as shown, restaurant + distinction, date (weekday), time, party size, deposit charged and
  until when it is refundable, seating limit, dress code. If Google Calendar is connected, offer an event
  ("🍽 <restaurant>", 2 h, reference in the description). No reference → not confirmed.
