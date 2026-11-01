---
name: hotel-booking
description: 订酒店：先用 hotels_search 查酒店、房型、价格和取消政策（每次调用只搜一次，每个请求只做一次旅行搜索；每家酒店带官网预订链接 booking_url），只在最后一步用浏览器在预订页下单（东京、大阪、首尔、新加坡、香港、曼谷、台北、上海…）Search, compare and book hotels in Asian cities — one travel search per request (each hotel with its booking_url), browser only for the final booking page.

form: on
---
# Hotel booking / 订酒店

Search and compare with data tools; open the browser only for the provider's booking page at the end.


**No plan for the search.** Searching is just searching: never call plan_task to search, compare or check a date range — one turn of search calls, then the answer. Plan only once one result has been picked and booking it takes several steps (and a "book it" follow-up goes straight to the booking page).

## 0. One tool: hotels_search (no browser yet)
- `hotels_search` is the only search call a hotel question needs, and a request gets **one travel search** in all —
  flights, hotels, cars and places together (a second one is not run): ONE search per call — Google Hotels data with the Flights & hotels key; without the key,
  or when Google fails, web search by itself (its result then says so in `hotels_fallback`: web pages, not hotels —
  read the 1–3 best with browser_read for prices). An empty result is the answer: say so and ask what to change (dates,
  area, stars, price, brands); the user's next message can search again. **No `web_search` beside it.** Each hotel's
  `booking_url` is Google's page for that hotel and these dates (every seller with its price, the hotel's own site among
  them; `hotel_site` is the hotel's own site) — give it with the hotel; the result's `compare_url` is the same stay on Kayak (no
  extra call) — give it once, for comparing prices. `hotel_rooms` (rates per room and seller) needs the key.
- A travel MCP connector in the tool list (`mcp_<server>__hotels_search` / `__hotel_rooms`) may be used instead.
- Never open Booking, Agoda, Google Hotels or hotel search pages in the browser to search — the browser is for the
  final booking page only.

## 1. Understand the request
- City (and area / near what), check-in and check-out dates (compute exact YYYY-MM-DD from relative dates and say them;
  nights = check-out − check-in), guests, rooms, budget per night or total, minimum stars / rating, breakfast,
  free cancellation needed?
- Check-in / check-out times and cancellation deadlines are in the hotel's local time zone — say so when you quote them.
- **Brand / chain**: in this order — the request ("a Marriott", "somewhere I can use Hilton points"), then the
  **Standing preferences** block of your instructions (the profile's Preferred hotel brand — a brand such as Holiday
  Inn or a group such as IHG — and any custom hotel / loyalty field), then memory (`memory_search("hotel")` in the
  same turn as the search when neither says anything). Pass them as `brands=["Holiday Inn"]` or `brands=["IHG"]` (a
  group stands for all its brands): one or two go into the search itself and matching hotels come back flagged
  `preferred`. Show the best preferred-brand hotel **next to** the best value when the result has one, with the price
  difference.
- **One search, with the preference in it.** The brand goes into the single `hotels_search` — never a second search
  without it to compare. When the user asks for **all hotels** ("any hotel", "search everything", "not just
  Marriott"), leave `brands` out of that one search.
- **No good preferred result** (`preferred_found` is 0, or none fits the area, dates, budget or rating): still answer —
  say plainly what the preferred brand has (or that it has nothing) instead of presenting another chain as if it were
  the one asked for, and ask "Shall I search all hotels?". Search again without `brands` only after the user says yes.
  Membership numbers stay in the vault (never in chat); mention when a rate earns status benefits.

## 2. Search and compare (data tools)
- One `hotels_search(city, check_in, check_out, guests, …)` call (min_stars / max_price_per_night / area / brands when
  given) — one call per turn and per request, not one with the brand and another without. The result's `searched` block repeats the city, dates, guests and brands used, and `preferred_found` says how
  many hotels matched the brands.
- Shortlist 2–4 hotels from the one search and answer with each hotel's `booking_url` (Google's page for it and the
  dates: every seller and price). `hotel_rooms(hotel_id, check_in, check_out, guests)` costs one more search: call it
  only when the user asks for the room types or the rates per seller of one hotel, or to book it — never to answer a
  search. Compare: total for the stay vs. per night, taxes included or not, area + nearest station,
  rating, room type and beds, breakfast, **cancellation policy** (free until when, what is charged after) and
  **payment** (pay at the hotel vs. prepay now; non-refundable rates are cheaper but cannot be changed).
- Prefer a free-cancellation rate unless the user wants the cheapest at any condition — say the price difference.
- Report only what the tools returned; never estimate prices.

## 3. Show the options
- present_choices(kind="comparison"): label = the hotel `name` (or room type) exactly as in the data, details = exact
  strings (`area`, `rating`, `price_per_night`, `total`, `cancellation`), source_url = the room's or hotel's
  `booking_url`. Recommendation in note.
- If the user already told you what to book ("book the cheapest 4-star with free cancellation"), go on to step 4 after
  the cards; otherwise finish with a short answer and wait.
- "Book it" after an earlier turn already found the hotel: **no plan_task, no new search, no cards again** — take the
  room from the conversation and run steps 4–5 as one flow (vault_list, the booking page, guest details, and for a
  prepaid rate purchase_confirm with card_item_id before the card is filled from the vault), then hand the filled page over with browser_request_takeover for the user to check
  and confirm, unless they said to complete the booking now.

## 4. Guest details
- Name, email, phone; arrival time and requests are optional. Use the request, then memory / profile_get; ask once for
  anything missing. Membership numbers: from the Vault, never in chat.
- Prepaid rates need a card: vault_list for it (code fills it from the Vault after purchase_confirm). Pay-at-hotel
  rates do not.

## 5. Book (browser — only the booking page)
1. purchase_confirm once: site = booking page domain, items = [{name: "<hotel> – <room type>, <check-in> to
   <check-out> (<n> nights)", price: total}], total = the stay total from the data, currency, delivery = "Hotel stay,
   <pay at hotel | prepaid>", card_item_id when paying by card, note = the cancellation policy.
2. Code opens the booking link (the room's booking_url) and fills it on known sites — name, email, phone and, for a
   prepaid rate, the card from the Vault (form_fill is called by code, not by you). A site with no form function goes
   to the `browser-use` skill.
3. **The last click is the user's**: never press "Confirm booking" (Sentinel refuses it). Once code has filled the page
   (a card saved by name only, or no card in the Vault: the card boxes stay empty), call
   browser_request_takeover with the reason "The booking page is filled in: check it (type the card if a prepaid rate needs one), press Confirm booking yourself and
   hand the browser back". The chat shows the user a link to take over.
4. After the hand-back, read the confirmation from one browser_snapshot. No confirmation → the booking is not made:
   say so and that the page stays open for them.

## 6. Answer
- Booking reference exactly as shown, hotel + address area, room, dates and nights, total and how it is paid,
  cancellation deadline (local time), check-in/check-out times. No reference shown → say it is not confirmed.
