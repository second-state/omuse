---
name: flight-booking
description: 订机票：用 flights_search（Google Flights 或 Kiwi.com 数据，仅供参考）和 web_search 同时查航班比价，只在最后一步用浏览器在航空公司官网或用户选定的代理商预订页下单（东京、大阪、首尔、新加坡、香港、曼谷、台北、上海…）Search and compare flights with flights_search (Google Flights or Kiwi.com data, information only) and web_search in one turn, then book on the airline's own site or with the agency the user chooses — the browser only for that final booking page.
---
# Flight booking / 订机票

The goal is a correct booking with as few browser steps as possible. Search and compare with data tools; the browser
is only for the provider's booking page at the very end.

## 0. Pick the source (no browser yet)
- `flights_search` is always there: Google Flights data when the Flights & hotels key is set, else Kiwi.com data —
  **information only**. Call it and `web_search` in the SAME turn: `flights_search` for the schedule, prices, stops and
  baggage; `web_search` ("<airline> <route> <date> fare", "site:kayak.com …") for the airline's or an agency's fare
  page and where to book. Never book through Kiwi, never quote or open a Kiwi link; `flight_offer` gives the details
  of one offer (no link for Kiwi data).
- A travel MCP connector in the tool list (`mcp_<server>__flights_search` …) can be used in the same turn too.
- Do NOT open Google Flights, Kayak, Skyscanner or airline search pages in the browser — the browser is for the
  final booking page only: the airline's own site, or the agency the user chooses (ask which when they have one).

## 1. Understand the request
- Origin, destination, date, one way / return, adults (default 1), cabin (default economy), direct only?, budget,
  time-of-day wishes. City names are fine (`Tokyo` covers HND + NRT, `Seoul` ICN + GMP, `Osaka` KIX, `Bangkok` BKK + DMK,
  `Taipei` TPE + TSA, `Shanghai` PVG + SHA).
- Dates: "next Friday", "in 3 weeks" → compute the exact YYYY-MM-DD from today's date in the user's timezone and say it.
  The date is the **local departure date at the origin**.
- Times in the data are local to each airport (Singapore/Hong Kong/Taipei/Shanghai UTC+8, Tokyo/Osaka/Seoul UTC+9,
  Bangkok UTC+7). An arrival marked (+1) lands the next calendar day; mention it. Never convert times yourself unless asked.

## 2. Search and compare (data tools)
- One `flights_search` call with origin, destination, date, adults, cabin (and nonstop_only=true when the user wants
  direct flights), and the `web_search` calls beside it in the same turn. Search again only if the user changes something.
- Shortlist 2–4 offers that fit. "Cheapest direct" = the lowest fare among offers with stops 0. Compare total price
  (fare × adults), departure/arrival time, duration, stops, **baggage** (low-cost fares often include only a 7 kg cabin
  bag — say what a checked bag costs) and **change/refund rules**.
- Call `flight_offer(offer_id)` once for the offer you will book: it has the fare breakdown, rules and the booking_url.
- Never invent or round prices, times or rules: report only what the tools returned, with currency.

## 3. Show the options
- present_choices(kind="comparison"): label = the offer's `title` exactly (e.g. "Scoot TR 765"), details = exact strings
  from the data (`schedule`, `duration`, `stops_text`, `fare`, `baggage`), source_url = that offer's `booking_url`.
  Put your recommendation and caveats in note.
- If the user already said which one to book ("book the cheapest direct", "book the 9 am JAL"), continue straight to
  step 4 after showing the cards. Otherwise finish with a short answer and wait for their pick.

## 4. Traveller details (before booking)
- Needed: full name exactly as in the passport, email, phone, passport number, card. Use what the request says, then
  memory / profile (profile_get). If something is still missing, ask once — one message listing everything missing.
- Passport and card numbers are never typed or asked for in chat: check vault_list for an ID document (passport) and a
  card. Fill them with browser_fill_secret (field `number` for the passport; `number`, `expiry`, `cvc`, `holder` for a
  card). Optional fields (e.g. passport expiry) stay empty unless the page requires them. No vault item → browser_request_takeover so the user types it.

## 5. Book (browser — only the booking page: the airline's site, or the agency the user chose; never Kiwi)
1. purchase_confirm once: site = the booking page's domain, items = [{name: "<flight no> <from>-<to> <date>, <n> adult",
   price: fare}], total = the total from the data (it must match the page), currency, delivery = "E-ticket to <email>",
   card_item_id = the vault card id, note = baggage + refund rules.
2. browser_navigate(booking_url). The snapshot lists the form fields with refs; check the page total matches.
3. browser_type name, email, phone (several calls in one turn are fine); browser_fill_secret for passport and card.
   Send each browser_fill_secret as its own turn (one call, nothing else in that turn): every vault fill can wait for the user's approval, and a turn should hold at most one call that may wait.
4. browser_click the "Pay now" / "Confirm booking" button once. Sentinel handles any approval; do not ask in chat.
5. The click result shows the confirmation page. Only if it does not, take one browser_snapshot.
- Do not re-open the page, do not take extra snapshots, do not use browser_look on payment pages.
- An error on the page ("Please complete: …") → fill only what it names and click again.

## 6. Answer
- Booking reference exactly as shown, flight number, route, local departure/arrival times (+1 if next day), total paid,
  baggage allowance, change/refund rules, and that the e-ticket goes to the email. If no reference was shown, say the
  booking is NOT confirmed.
- If Google Calendar is connected, offer to add the flight (do not add it unasked).
