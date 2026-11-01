---
name: flight-booking
description: 订机票：用 travel_search(kind=flight) 查航班比价，用 travel_book 预订——把用户的要求作为参数（日期写成 YYYY-MM-DD），代码搜索一次、按规则挑选、展示卡片、预订并把付款交给用户；回复按结果 JSON 写（东京、大阪、首尔、新加坡、香港、曼谷、台北、上海…）Search and compare flights with travel_search(kind=flight) and book with travel_book — the user's details as JSON arguments; code runs ONE search, picks by rule, shows the cards, fills the booking page and hands the Pay click to the user; you word the reply from the JSON result.

form: on
---
# Flight booking / 订机票

Code does the work; you turn the user's words into the tool's arguments and the tool's JSON result into the reply.

## 1. Call the tool — once, with every detail as an argument
- **Search / compare** ("cheapest flight…", "show me flights…", "which is the fastest…") → `travel_search` with
  `kind: "flight"`. **Book** ("book it", "book the SQ 638", a choice card's "I choose: …" after a search in this chat) →
  `travel_book` with the same arguments, plus `pick` = the label the user chose (empty: code picks by rule). Never
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

## 3. Write the reply from the result
- Every fact in `must_say` goes into the reply, as given (names, prices with currency, links, references).
- `shown`: the options (airline, flight numbers, times, stops, price and each offer's `booking_url`, `book_at` the
  airline's own site), the pick and its rule, the cabin searched (`cabin_searched`), `days_searched` for a range, the
  Kayak `links.compare_url` once, `sellers` when present. The cards are already on screen. Offer to book ("say book
  it"). `next_items` → offer the next search ("Shall I search the hotel now?").
- `nothing_found`: say so and what was searched; `next_question: "search_all_airlines"` → ask whether to search all
  airlines (the user's yes is a new `travel_search` with `all_airlines: true`). `unmet` → say which criterion nothing
  met and offer to relax it.
- `booked`: the reference exactly, the flight, the total and card, the confirmation e-mail. `not_confirmed`: the
  booking is NOT made (say why: not approved, or no reference after the hand-back; the page stays open).
  `handed_over`: the filled page is with the user to press Pay. `browser_use`: the booking page is open and the
  browser-use skill is loaded — fill that one page, hand it over for Pay.
- `mailed`: say it was sent, or not and why.
- Times are local to each airport; an arrival marked (+1) lands the next day. Never add a price, time or rule the
  result does not have.
