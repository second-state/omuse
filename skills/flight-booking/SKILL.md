---
name: flight-booking
description: 订机票：用 flights_search（Google Flights 或 Kiwi.com 数据，仅供参考）和 web_search 同时查航班比价，只在最后一步用浏览器在航空公司官网或用户选定的代理商预订页下单（东京、大阪、首尔、新加坡、香港、曼谷、台北、上海…）Search and compare flights with flights_search (Google Flights or Kiwi.com data, information only) and web_search in one turn, then book on the airline's own site or with the agency the user chooses — the browser only for that final booking page.

---
# Flight booking / 订机票

The goal is a correct booking with as few browser steps as possible. Search and compare with data tools; the browser
is only for the provider's booking page at the very end.


**No plan for the search.** Searching is just searching: never call plan_task to search, compare or check a date range — one turn of search calls, then the answer. Plan only once one result has been picked and booking it takes several steps (and a "book it" follow-up goes straight to the booking page).

## 0. Read the request first — these decide the search, and they are not negotiable
Before any call, take from the user's words (ask once only when one is genuinely missing and matters):
- **Date**: the exact local departure date (YYYY-MM-DD); "next Friday", "in 3 weeks" → compute from today's date in the
  user's timezone and say the date in the answer. A return date makes it a round trip.
- **Cabin**: `cabin` = what the user asked for — `business` for "business class / C class / lie-flat", `first`,
  `premium_economy`, else `economy`. **Never answer a business-class request with economy fares.** If the search in
  that cabin returns nothing, say so and offer the nearest thing (another date, a stop, premium economy) — do not
  quietly show a cheaper cabin.
- **Mixed cabins count.** An itinerary whose main legs are in business but with a short economy leg (a regional
  connection, `cabin_mix: "business + economy"`, the legs listed in `economy_legs`) **is a business-class result** —
  keep it in the comparison and say which leg is economy ("business HND–SIN; the KIX–HND connection is economy"). Only
  an offer marked `cabin: "economy (not the cabin searched)"` — every leg in economy — is left out.
- **Stops**: "direct / nonstop" → `stops="nonstop"`; "one stop at most" → `stops="one_stop_max"`; nothing said → `stops="any"` and
  sort by price, then say how many stops each offer has.
- **Passengers** (default 1 adult), **currency** (the user's, else the profile's), **time of day** wishes (filter the
  results yourself).
- **Airline**: in this order — the request ("on Singapore Airlines", "Star Alliance so I earn miles"), then the
  **Standing preferences** block of your instructions (the profile's Preferred airline — an airline such as United or
  an alliance such as Star Alliance — and any custom airline / loyalty field), then memory (`memory_search("airline")`
  in the same turn as the search when neither says anything). Pass them as `airlines=["Singapore Airlines", "NH"]` or
  `airlines=["Star Alliance"]` (names, 2-letter codes or an alliance, which stands for all its members): matching
  offers come back flagged `preferred`, nothing is dropped. In the answer show the best preferred-airline option **next to** the cheapest, with the price
  difference, and let the user choose; if the user asked for an airline that has no offer in the data, say so plainly
  instead of silently switching. Mention when a fare is on the user's status airline (lounge, baggage, miles).
- Every result carries `searched` (cabin, stops, date, trip, adults): check it matches the request before you answer,
  and name the cabin and the stop limit in the answer.

## 1. One search per question (no plan)
- "Cheapest flight / cheapest business-class flight from A to B on <date>" is **one `flights_search` call** (plus
  `web_search` in the same turn for the airline's or an agency's fare page), then the answer. Do not call plan_task,
  do not search airport by airport, do not search several dates unless the user asked for flexibility.
- **A range of dates is still one search**: "cheapest between 1 and 10 December", "any day next week", "flexible ±3
  days" → `date` = the first day, `date_to` = the last (Kiwi.com data covers up to 14 days, Google Flights 7; the result
  says when it cut the range). Every offer carries its `date`, and `by_date` lists the cheapest per day — use it to
  name the cheapest day. Never one call per day, never a plan for it.
- A city with several airports is still one search: give its **city code** (or the city's name) and the tool covers all
  of them — `TYO` = HND + NRT, never two searches. Codes the tool knows (airport codes for single-airport cities such as
  SIN, HKG, SGN, HAN, SYD, DEL work as they are):
  TYO = Tokyo (HND, NRT)
  OSA = Osaka (KIX, ITM)
  SEL = Seoul (ICN, GMP)
  BJS = Beijing (PEK, PKX)
  SHA = Shanghai (PVG, SHA)
  CTU = Chengdu (TFU, CTU)
  TPE = Taipei (TPE, TSA)
  BKK = Bangkok (BKK, DMK)
  KUL = Kuala Lumpur (KUL, SZB)
  JKT = Jakarta (CGK, HLP)
  DXB = Dubai (DXB, DWC)
  IST = Istanbul (IST, SAW)
  MEL = Melbourne (MEL, AVV)
  LON = London (LHR, LGW, STN, LTN, LCY)
  PAR = Paris (CDG, ORY)
  ROM = Rome (FCO, CIA)
  MIL = Milan (MXP, LIN, BGY)
  NYC = New York (JFK, EWR, LGA)
  WAS = Washington (IAD, DCA, BWI)
  CHI = Chicago (ORD, MDW)
  QSF = San Francisco (SFO, OAK, SJC)
  QLA = Los Angeles (LAX, BUR, LGB, SNA, ONT)
  MIA = Miami (MIA, FLL)
  HOU = Houston (IAH, HOU)
  DFW = Dallas (DFW, DAL)
  YTO = Toronto (YYZ, YTZ)
  MEX = Mexico City (MEX, NLU)
  SAO = São Paulo (GRU, CGH, VCP)
  RIO = Rio de Janeiro (GIG, SDU)
  BUE = Buenos Aires (EZE, AEP)
  Two cities on one side: a comma list (`TYO,OSA`). The result's `searched.origin` shows the airports actually used.
- `flights_search` is always there and has its own fallback chain: Kiwi.com data, then Google Flights when the Flights & hotels key
  is set, then the web search chain (its result then has web pages, not offers — read the fare pages with browser_read) —
  **information only**. Never book through Kiwi, never quote or open a Kiwi link; `flight_offer` gives the details
  of one offer (no link for Kiwi data).
- A travel MCP connector in the tool list (`mcp_<server>__flights_search` …) can be used in the same turn too.
- Do NOT open Google Flights, Kayak, Skyscanner or airline search pages in the browser — the browser is for the
  final booking page only: the airline's own site, or the agency the user chooses (ask which when they have one).
- Times in the data are local to each airport (Singapore/Hong Kong/Taipei/Shanghai UTC+8, Tokyo/Osaka/Seoul UTC+9,
  Bangkok UTC+7). An arrival marked (+1) lands the next calendar day; mention it. Never convert times yourself unless asked.

## 2. Compare (data tools)
- Shortlist 2–4 offers that fit **the cabin and stop limit searched**. "Cheapest direct" = the lowest fare among offers
  with stops 0. Compare total price (fare × adults), departure/arrival time, duration, stops, **baggage** (low-cost
  fares often include only a 7 kg cabin bag — say what a checked bag costs) and **change/refund rules**.
- Call `flight_offer(offer_id)` once for the offer you will book: it has the fare breakdown, rules and the booking_url.
- Never invent or round prices, times or rules: report only what the tools returned, with currency.

## 3. Show the options
- present_choices(kind="comparison"): label = the offer's `title` exactly (e.g. "Scoot TR 765"), details = exact strings
  from the data (`schedule`, `duration`, `stops_text`, `fare`, `baggage`), source_url = that offer's `booking_url`.
  Put your recommendation and caveats in note.
- If the user already said which one to book ("book the cheapest direct", "book the 9 am JAL"), continue straight to
  step 4 after showing the cards. Otherwise finish with a short answer and wait for their pick.

## 3a. "Book it" — the flight was already found in this conversation
- When an earlier turn already searched and the user now says "book it", "book the SQ 638", "go ahead with the 9 am
  JAL": **no plan_task, no new search, no present_choices again.** Take the offer from the conversation (its
  booking_url; for Kiwi data the airline and flight) and run steps 4–5 as ONE flow: vault_list; the booking page; the
  traveller typed in from the profile (profile_get for name / email / phone when not in the conversation); the passport
  filled from the vault; **purchase_confirm with card_item_id** (the total shown on the page — Sentinel fills a card
  only after the user approves this summary); the card filled from the vault; then — unless the user said to pay or
  complete the booking now — hand the filled page over with browser_request_takeover ("the booking page is filled in,
  check it and press Pay") so they review it and pay themselves. No card in the vault → say so in one line and hand the page over for
  them to type it; never ask for the number in chat.
- Kiwi data has no link: open the airline's own booking page (the fare page web_search found earlier, or the airline's
  site with the route and date), or the agency the user named earlier.

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
