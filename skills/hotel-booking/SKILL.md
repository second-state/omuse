---
name: hotel-booking
description: 订酒店：先用旅行数据连接器查酒店、房型、价格和取消政策，只在最后一步用浏览器在预订页下单（东京、大阪、首尔、新加坡、香港、曼谷、台北、上海…）Search, compare and book hotels in Asian cities — data connector first, browser only for the final booking page.

---
# Hotel booking / 订酒店

Search and compare with data tools; open the browser only for the provider's booking page at the end.


**No plan for the search.** Searching is just searching: never call plan_task to search, compare or check a date range — one turn of search calls, then the answer. Plan only once one result has been picked and booking it takes several steps (and a "book it" follow-up goes straight to the booking page).

## 0. One tool: hotels_search (no browser yet)
- `hotels_search` is the only search call a hotel question needs: Google Hotels data with the Flights & hotels key,
  else — or when Google finds nothing — it falls back to web search by itself (its result then says so in
  `hotels_fallback`: web pages, not hotels — read the 1–3 best with browser_read for prices). **No `web_search`
  beside it.** `hotel_rooms` (rates per room and seller) needs the key.
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
  given) — one call, not one with the brand and another without. The result's `searched` block repeats the city, dates, guests and brands used, and `preferred_found` says how
  many hotels matched the brands.
- Shortlist 2–4 hotels; call `hotel_rooms(hotel_id, check_in, check_out, guests)` for the one(s) you would book
  (at most 3 calls). Compare: total for the stay vs. per night, taxes included or not, area + nearest station,
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
  anything missing. Membership numbers: vault (browser_fill_secret), never in chat.
- Prepaid rates need a card: vault_list → browser_fill_secret (number, expiry, cvc, holder). Pay-at-hotel rates do not;
  leave optional card fields empty.

## 5. Book (browser — only the booking page)
1. purchase_confirm once: site = booking page domain, items = [{name: "<hotel> – <room type>, <check-in> to
   <check-out> (<n> nights)", price: total}], total = the stay total from the data, currency, delivery = "Hotel stay,
   <pay at hotel | prepaid>", card_item_id when paying by card, note = the cancellation policy.
2. browser_navigate(room booking_url) → the snapshot has the form with refs; check the total matches.
3. browser_type name, email, phone (one turn); vault fills if needed.
   Send each browser_fill_secret as its own turn (one call, nothing else in that turn): every vault fill can wait for the user's approval, and a turn should hold at most one call that may wait.
4. browser_click "Confirm booking" once (Sentinel takes care of approval).
5. Read the confirmation from the click result; one browser_snapshot only if it is missing.

## 6. Answer
- Booking reference exactly as shown, hotel + address area, room, dates and nights, total and how it is paid,
  cancellation deadline (local time), check-in/check-out times. No reference shown → say it is not confirmed.
