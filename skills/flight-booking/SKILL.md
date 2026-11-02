---
name: flight-booking
description: 订机票：用 travel_search(kind=flight) 查航班比价，用 travel_book 预订——把用户的要求作为参数（日期写成 YYYY-MM-DD），代码搜索一次并列出当天的全部航班，由你用 travel_pick 按用户的要求选出航班（不擅自挑最便宜的），代码展示、打开预订页交给 browser-use 技能（付款由用户点）；回复按结果 JSON 写（东京、大阪、首尔、新加坡、香港、曼谷、台北、上海…）Search and compare flights with travel_search(kind=flight) and book with travel_book — the user's details as JSON arguments; code runs ONE search and lists every flight of the day, you pick exactly the flights the user asked for with travel_pick (their offer ids), code shows them, opens the booking page for the browser-use skill (the Pay click is the user's); you word the reply from the JSON result.

form: on
---
# Flight booking / 订机票

Code does the work; you turn the user's words into the tool's arguments and the tool's JSON result into the reply.

## 1. Call the tool — once, with every detail as an argument
- **Search / compare** ("cheapest flight…", "show me flights…", "which is the fastest…") → `travel_search` with
  `kind: "flight"`. **Book** ("book it", "book the SQ 638", a choice card's "I choose: …" after a search in this chat) →
  `travel_book` with the same arguments, plus `pick` = the label or offer_id the user chose (empty: the search's flights come back for travel_pick). Never
  `flights_search`, `present_choices`, `purchase_confirm` or the browser yourself for this: the tool runs them.
- `date`: the departure day as YYYY-MM-DD — resolve "next Friday", "in 3 weeks", "tomorrow" from today's date in the
  live state. `return_date` makes it a round trip. Any day of a range ("between 1 and 10 December", "flexible next
  week") → `date` = the first day, `date_to` = the last (one search covers them).
- `origin` / `destination`: a city or airport code — a city with several airports by its city code (the tool covers
  all of them), two cities as a comma list (`TYO,OSA`). Leave `origin` empty when the user did not say: code uses the
  home city from the profile.
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
- `cabin`: business for "business class / C class / lie-flat", first, premium_economy, else economy. `nonstop: true`
  for "direct / nonstop". `party`: the number of adults when the user said it. `max_price`: the budget per adult.
- `airlines`: the airlines or alliances the user asked for ("on Singapore Airlines", "Star Alliance so I earn miles");
  leave it out to use the profile's standing preference (code adds it). `all_airlines: true` when the user asks for
  every airline ("any airline", "not just Star Alliance").
- `named`: a flight or airline the user named for the pick ("the 9 am JAL", "SQ 638").
- `mail_results_to`: only an address the user wrote and asked the options to be sent to. `contact`: name, email and
  phone the user gave in the chat (the profile and the connected mailbox come first). `card`: the Vault card the user
  named. `use_connector: true` when the user books through their travel agency / travel connector.
- `next`: the other searches the same request asks for (a hotel, a rental car) — ONE travel search per request; the
  result lists them as `next_items` for you to offer.

## 2. What the tool answers
- `{"status": "needs_input", "missing": [...]}` → ask the user for exactly those fields (one short message), then call
  again with every field. `{"status": "invalid", "problems": [...]}` → fix the arguments and call again.
- `{"status": "running"}` → code is working (it may stop for the user's approval or the hand-over); the JSON result
  follows as `travel_search_result` / `travel_book_result`.
- `{"status": "pick_needed", "flights": [...]}` → every flight the search found (offer_id, departure and arrival with
  airport and local time, `arrives_days_later`, stops, price). Read them against the user's words and call
  `travel_pick` with the offer_ids of exactly the flights asked for, in the order to show: ONE id for a request that
  names one flight ("the last flight that lands before midnight" = the latest-leaving flight with `arrives_days_later`
  0), several only when the user asked for options. Never add a cheaper or "better" flight; the cheapest only when
  the user gave no criterion. None meets the request → do not call it; say so.

## 3. Write the reply from the result
- The facts in `show` are appended to the reply by code (names, prices with currency, links, references).
- `shown`: the flights picked (airline, flight numbers, times, stops, price and each offer's `booking_url`, `book_at`
  the airline's own site) — only those, the first is the pick — the cabin searched (`cabin_searched`), `days_searched` for a range, the
  Kayak `links.compare_url` once, `sellers` when present. The cards are already on screen. Offer to book ("say book
  it"). `next_items` → offer the next search ("Shall I search the hotel now?").
- `nothing_found`: say so and what was searched; `next_question: "search_all_airlines"` → ask whether to search all
  airlines (the user's yes is a new `travel_search` with `all_airlines: true`). `unmet` → say which criterion nothing
  met and offer to relax it.
- `not_confirmed`: the booking is NOT made (the payment was not approved). `browser_use`: the payment is approved
  (when charged), the booking page is open and the browser-use skill is loaded — fill that one page, hand it over for
  Pay; the reference comes from the confirmation page after the hand-back (`confirm_reference`).
- `mailed`: say it was sent, or not and why.
- Times are local to each airport; an arrival marked (+1) lands the next day. Never add a price, time or rule the
  result does not have.
