---
name: flight-booking
description: 订机票：只用 flights_search 查航班比价（每次调用只搜一次，每个请求只做一次旅行搜索（机票、酒店、租车、餐厅合计）；有密钥用 Google Flights，日期范围、自行中转、没有密钥或 Google 出错时用 Kiwi.com；每个结果都带预订链接 booking_url），最后一步用浏览器在 booking_url、航空公司官网或用户选定的代理商下单（东京、大阪、首尔、新加坡、香港、曼谷、台北、上海…）Search and compare flights with flights_search ONLY — ONE search per call and ONE travel search per request — flights, hotels, cars and places together (Google Flights with the key; Kiwi.com for a range of dates, self-transfer, without the key or when Google fails; every offer has its booking_url), then book from the offer's booking_url, on the airline's own site or with the agency the user chooses — the browser only for that final booking page.

form: on
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
  offers come back flagged `preferred`. Mention when a fare is on the user's status airline (lounge, baggage, miles).
- **One search, with the preference in it.** The airline preference goes into the single `flights_search` — never a
  second search without it to compare. When the user asks for **all airlines** ("any airline", "search everything",
  "not just Star Alliance"), leave `airlines` out of that one search. Answer from what the search returned: the best
  preferred offer, and the cheapest other offer when the result has one, with the price difference.
- **No good preferred result** (`preferred_found` is 0, or none fits the date, cabin, stops or budget): still answer —
  say plainly what the preferred airlines have (or that they have nothing) instead of silently switching, and ask
  "Shall I search all airlines?". Search again without `airlines` only after the user says yes.
- Every result carries `searched` (cabin, stops, date, trip, adults): check it matches the request before you answer,
  and name the cabin and the stop limit in the answer.

## 1. One search per question (no plan)
- Any question about flights — "cheapest flight from A to B", "the last flight tonight that lands before midnight",
  "business class on Friday" — is **one `flights_search` call and nothing else**, then the answer. **No `web_search`
  beside it**, no plan_task, no search per airport, no extra dates unless the user asked for flexibility. A second
  `flights_search` in the same turn or for the same request is not run: when the result is empty or not good enough,
  answer with what it has and ask the user what to change; their next message can search again.
- **A range of dates is still one search**: "cheapest between 1 and 10 December", "any day next week", "flexible ±3
  days" → `date` = the first day, `date_to` = the last (up to 14 days, one Kiwi.com search; the result
  says when it cut the range). Every offer carries its `date`, and `by_date` lists the cheapest per day — use it to
  name the cheapest day. Never one call per day, never a plan for it.
- **Self-transfer** (separate tickets with your own connections, the bags collected and checked in again) is off by
  default: every connection is on one booking. Set `self_transfer=true` only when the user explicitly asks for it
  ("self-transfer is fine", "separate tickets are OK") — never because it is cheaper.
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
- `flights_search` is always there and makes ONE search per call. Its source: Google Flights when the Flights & hotels
  key is set and the request is one departure date (each offer's `ticket` says whether it is one ticket and who sells
  it — prefer one ticket, e.g. a United ticket with an Air China leg, over separate ones); Kiwi.com for a range of
  dates, for self-transfer, without the key, or when Google fails. An empty result is the answer. **Every offer has a
  `booking_url`**: for Google data, Google Flights' page for that itinerary (it lists who sells it, the airline's own
  site first; for a round trip it opens on the outbound, where the return is picked); for Kiwi.com data, Kiwi.com's
  booking page for that itinerary (Kiwi sells it as an agency) and `book_at`, the airline's own website. Give the
  `booking_url` with each offer in the answer. The result's `compare_url` is the same search on Kayak (built from the
  request, no extra call): give it once, for comparing prices across sites.
- A travel MCP connector in the tool list (`mcp_<server>__flights_search` …) can be used in the same turn too.
- Do NOT search on Google Flights, Kayak, Skyscanner or airline search pages in the browser — the browser is for the
  final booking page only: the offer's `booking_url`, the airline's own site, or the agency the user chooses (ask
  which when they have one).
- Times in the data are local to each airport (Singapore/Hong Kong/Taipei/Shanghai UTC+8, Tokyo/Osaka/Seoul UTC+9,
  Bangkok UTC+7). An arrival marked (+1) lands the next calendar day; mention it. Never convert times yourself unless asked.

## 2. Compare (data tools)
- Shortlist 2–4 offers that fit **the cabin and stop limit searched**. "Cheapest direct" = the lowest fare among offers
  with stops 0. Compare total price (fare × adults), departure/arrival time, duration, stops, **baggage** (low-cost
  fares often include only a 7 kg cabin bag — say what a checked bag costs) and **change/refund rules**.
- Every offer already has its `booking_url`. Call `flight_offer(offer_id)` only when the user wants the sellers'
  prices, baggage or fare rules for one offer (Google data: it costs one more search credit).
- Never invent or round prices, times or rules: report only what the tools returned, with currency.
- Every offer in the answer names **its airline(s) and flight numbers** (the offer's `title` / `airlines`, each leg's
  carrier on a connection — e.g. "China Southern CZ 8490 + CZ 3085"), the local departure and arrival times, the stops,
  the price and its `booking_url` — a route and a price without the airline is not an answer the user can book.

## 3. Show the options
- present_choices(kind="comparison"): label = the offer's `title` exactly (e.g. "Scoot TR 765"), details = exact strings
  from the data (`schedule`, `duration`, `stops_text`, `fare`, `baggage`), source_url = that offer's `booking_url`.
  Put your recommendation and caveats in note.
- If the user already said which one to book ("book the cheapest direct", "book the 9 am JAL"), continue straight to
  step 4 after showing the cards. Otherwise finish with a short answer and wait for their pick.

## 3a. "Book it" — the flight was already found in this conversation
- When an earlier turn already searched and the user now says "book it", "book the SQ 638", "go ahead with the 9 am
  JAL": **no plan_task, no new search, no present_choices again.** Take the offer from the conversation (its
  booking_url) and run steps 4–5 as ONE flow: vault_list; profile_get for name / email / phone when not in the
  conversation; **purchase_confirm with card_item_id** (the total shown on the page — a card is filled only after this
  summary); code opens the booking page and fills the traveller, the passport and the complete card; then hand the filled page over with
  browser_request_takeover ("the booking page is filled in, check it and press Pay") — the Pay click is always the
  user's. No card in the vault, or only a card saved by name (number UNKNOWN) → say so in one line and leave the card
  boxes for them to type; never ask for the number in chat.
- The offer's `booking_url` is where the booking starts: on Google Flights' page pick the seller it lists (the
  airline's own site first); a Kiwi.com `booking_url` is Kiwi.com's booking page (Kiwi sells it as an agency) — when the
  user prefers to book with the airline, open its `book_at` site with the route and date instead. No web_search for it.

## 4. Traveller details (before booking)
- Needed: full name exactly as in the passport, email, phone, passport number, card. Use what the request says, then
  memory / profile (profile_get). If something is still missing, ask once — one message listing everything missing.
- Passport and card numbers are never typed or asked for in chat: check vault_list for an ID document (passport) and a
  card; code fills them from the Vault on the booking page. No vault item → the user types it in the hand-over.

## 5. Book (browser — only the booking page: the offer's booking_url, the airline's site, or the agency the user chose)
1. purchase_confirm once: site = the booking page's domain, items = [{name: "<flight no> <from>-<to> <date>, <n> adult",
   price: fare}], total = the total from the data (it must match the page), currency, delivery = "E-ticket to <email>",
   card_item_id = the vault card id, note = baggage + refund rules.
2. Code opens the booking link (booking_url) and fills it on known sites — name, email, phone, the passport and the
   card from the Vault (form_fill is called by code, not by you). A site with no form function goes to the
   `browser-use` skill.
3. **The last click is the user's**: never press "Pay now / Confirm booking" (Sentinel refuses it). Once code has filled the page
   (a card saved by name only, or no card in the Vault: the card boxes stay empty), call
   browser_request_takeover with the reason "The booking page is filled in: check it (type the card if it is empty), press Pay now / Confirm booking yourself and
   hand the browser back". The chat shows the user a link to take over.
4. After the hand-back, read the confirmation from one browser_snapshot. No confirmation → the booking is not made:
   say so and that the page stays open for them.
- Do not re-open the page, do not take extra snapshots, do not use browser_look on payment pages.

## 6. Answer
- Booking reference exactly as shown, flight number, route, local departure/arrival times (+1 if next day), total paid,
  baggage allowance, change/refund rules, and that the e-ticket goes to the email. If no reference was shown, say the
  booking is NOT confirmed.
- If Google Calendar is connected, offer to add the flight (do not add it unasked).
