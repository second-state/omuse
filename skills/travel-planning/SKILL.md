---
name: travel-planning
description: 旅行：机票/酒店比价、行程规划、签证入境、天气与行李、当地交通（Google Flights、Booking.com、Google 地图、官方来源）Flights, hotels, itineraries, visas, weather and packing, local transport.

plan: on

---
# Travel planning / 旅行规划

Flights, hotels and cars — one data tool each, and nothing beside it:
**ONE travel search per request** — flights_search, hotels_search, cars_search and places_search together (a second one
is not run). A trip that needs several ("a flight, a hotel and a car"): search the first part the user named, answer it,
and offer the next ("Shall I search the hotel now?"); each later message runs the next search. No plan for this.
- **Flights:** `flights_search` only — ONE search per call (Google Flights with the
  Flights & hotels key for one date; Kiwi.com for a range of dates, self-transfer, without the key or when Google fails).
  Every offer has its `booking_url` (Google Flights' page for that itinerary, or Kiwi.com's booking page; `book_at` is
  the airline's own website): give it with the offer.
- **Hotels:** `hotels_search` only — ONE search per call (Google Hotels with the key,
  else web search by itself); each hotel's `booking_url` is Google's page for it and the dates (every seller); `hotel_rooms` needs the key.
- **Cars:** `cars_search` only — ONE search per call: the pickup airport, dates, times,
  car class and the preferred companies. It returns web results (companies and "from" prices) and the `booking_url`:
  Kayak's page for exactly this rental with the live offers (Rentalcars.com's when Kayak's page does not answer).
Never open Google Flights, Kayak, Skyscanner, Booking, Agoda, Rentalcars or the companies' search pages in the browser
to search; the browser is for the final booking page only.

## 0. Ground rules
- Research only: never book, pay or hold anything. Stop at the page that asks for passenger or card details and say so.
- Every price, time and policy needs a source link and the time you checked it ("查询于 10/12 14:05 SGT"). Prices with currency.
- Do NOT open gmail / calendar / memory tools for general travel questions. Use them only when the request is about the
  user's *own* booked trip ("my flight", "我订的酒店", "从邮件里找行程").
- Do not assume group size, companions or a business purpose. Use what the request says; otherwise plan for the
  number of people stated, or say "按 1 人计".
- The user's home, currency and passport come from the profile (nationality, home address) and the time zone line of
  your instructions — never assume a country: plug adapters, visa rules and prices follow from them.
- Never repeat passport numbers, expiry dates or birth dates in an answer unless a form needs them.
- Independent lookups run at the same time: weather + visa + transport, several pages to read (never two travel
  searches: a request gets one — flights, hotels, cars and places together).
  Give them one `parallel` label in the plan and start them in ONE turn — one delegate(role="researcher", task=…,
  step=<its step id>) per lookup, or several browser_read calls in the same turn. Only work that drives one browser
  session step by step (clicking through a site) stays with you, in sequence.

## 1. Flights
- `flights_search` first, ONE call per route — a range of dates is `date` + `date_to` in that same call, never a call or
  a plan step per day: take the **date, cabin and stop limit from the user's words**
  (business class asked → `cabin=business`, never economy instead; "direct" → `stops="nonstop"`, nothing said → `stops="any"`), and give a city with
  several airports its **city code** or name — one search covers all its airports, never one per airport:
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
  The result's `searched` block repeats the cabin, stops and date used: present only those. A business itinerary with a
  short economy connection (`cabin_mix`) still counts as business — keep it and name the economy leg.
- Standing preferences are in your instructions (the "Standing preferences" block: the profile's preferred airline or
  alliance, hotel brand or group, car rental company, loyalty status) — pass them as `airlines=[…]` / `brands=[…]` (an
  alliance or group stands for its members) and `companies=[…]` for the car; a wish in the request
  wins; show the preferred option next to the best value with the price difference.
- **One search, with the preference in it** — flights, hotels and cars alike: never a second search without the
  preference to compare. The user asks for all airlines / hotels / companies ("any", "search everything") → leave the
  preference out of that one search. No good preferred result → answer with what was found and ask whether to search
  all airlines / hotels / rental companies; search again only after the user says yes.
- flights_search takes IATA codes (SIN, NRT, HND, KIX, ICN, HKG, YVR); "Tokyo" = `TYO` covers both airports.
- When flights_search finds nothing usable, say so and name the airlines' own sites; never search flight sites in the
  browser.
- Do not spend more than ~6 steps on one site; switch source instead.

## 2. Hotels
- `hotels_search` first, ONE call for the stay (the area, dates, guests and budget from the user's words); read a
  hotel's own page only for what the search does not give (a policy, a room detail). Never search hotel sites in the
  browser.
- Report per hotel: total and per-night price (taxes in or out), review score, nearest station + walk minutes,
  free cancellation yes/no and until when, link.
- If nothing fits the budget, widen before giving up: nearby areas (Ginza → Tsukiji, Higashi-Ginza, Kyobashi,
  Nihonbashi, Shimbashi), a lower score threshold, or one less night — and say what you widened.

## 3. Itineraries and travel times
- Check every "X minutes from Y" with Google Maps directions, transit mode:
  `https://www.google.com/maps/dir/?api=1&origin=Kyoto+Station&destination=Nishiki+Market&travelmode=transit`
  Never guess distances or which area a place is in — look it up (e.g. the Nintendo Museum is in Uji, not near Kyoto Station).
- Group sights by area, at most the number per day the user asked for, with travel mode and time between them,
  opening hours / closed days, and whether a reservation is needed (Ghibli Museum, teamLab, Nintendo Museum …).
- With children or older people: shorter walks, rest breaks, earlier evenings.

## 4. Visas, entry, health
- Use official sources only: Japan MOFA (mofa.go.jp/j_info/visit/visa), the embassy site, Visit Japan Web
  (vjw-lp.digital.go.jp), ICA for Singapore, the destination's immigration site. Quote what they say; do not add
  rules they don't state (e.g. a passport-validity rule) — if unsure, say "以官方为准" and link it.

## 5. Weather and packing
- Use climate normals from the national weather service (Japan: JMA `https://www.data.jma.go.jp/obd/stats/etrn/`) or
  a named forecast site for dates within ~10 days. Give average high/low and rain days for the actual period asked;
  label forecast vs. historical average.
- Packing list by category (clothes for the stated temperatures, documents, electronics incl. the plug adapter,
  health, money/IC card), matched to the trip length.

## 6. Rail, local transport, money
- Japan rail fares: JR Central / JR West official fare pages, SmartEX; JR Pass price at japanrailpass.net. Show the sum.
- Airport transfers: compare 3 options with time, price and source (e.g. Keikyu, monorail, limousine bus, taxi).
- Exchange rates: name the source and time (e.g. MAS, xe.com, Google Finance).
- Budgets: every line needs a basis — a looked-up price (hotel search, fare table) or a named estimate source
  (e.g. JNTO / Japan Guide typical costs). Say "1 人" or the stated group size, and keep totals consistent with the rows.
- Check rule changes with a current official page before stating them (e.g. Nozomi non-reserved seats are suspended
  only in peak periods — check JR Central's notice for the dates in question).

## 7. Car rental
- Any destination: ONE `cars_search` — `pickup` = the pickup airport's code (a city code is its main airport; `dropoff`
  only for a one-way rental), `pickup_date` / `dropoff_date`, the times when the user gave them, `car_class` (small,
  medium, large, suv, van, luxury, convertible) from the user's words, and `companies` = the preferred rental company
  from the Standing preferences (the profile's Preferred car rental, e.g. Hertz) unless the user asked for all
  companies. Never `web_search` for it, never a second cars_search in the same request. The pickup airport and the
  dates are enough: search right away — never ask for the driver's age (default 30), times (default 10:00) or a class
  first; mention that the price on the page can depend on the driver's age.
- Answer with the companies and "from" prices the web results show (say they are "from" prices) and the `booking_url`:
  the live offers for these dates and this car are there (Kayak's results page, or Rentalcars.com's). No good offer from
  the preferred company → say so and ask whether to search all rental companies. Book on the page the user picks
  (the company's own site or the aggregator), at the end.

### Japan
- Compare on one aggregator plus one company site: ToCoo! (`https://www2.tocoo.jp/en/`), Tabirai (`https://www.tabirai.net/car/`),
  Rakuten Travel rent-a-car, or the companies directly — Toyota Rent a Car (`https://rent.toyota.co.jp/en/`), Times Car
  Rental, Nippon Rent-A-Car, Orix. Set pick-up/return place, dates and car class, then read the quote page (stop there).
- For winter (Dec–Mar, Hokkaido/Tohoku) report: studless winter tyres included or extra (and the fee), 4WD option,
  CDW (免責補償) price, NOC (Non-Operation Charge, 营业损失赔偿) amount and the cover that waives it, ETC card, and
  whether an International Driving Permit (1949 Geneva) is accepted for a Singapore licence.
- If a quote needs a login or a date picker that will not take input after two tries, use the other site.

## 8. Insurance and other plan tables
- Read the insurer's official plan / benefits table or policy wording (PDF). Quote numbers with plan names exactly as
  written; give premiums only from an actual quote page.
- Sanity-check tables you extract: higher tiers normally have equal or higher limits. If a number breaks the pattern
  (e.g. a top plan with a lower cancellation limit than a middle plan), re-read that row or mark it "需核实".

## 8b. Attraction tickets (teamLab, museums, theme parks)
- Prices: read the official price page with browser_read. Availability: open the official ticket calendar for the date
  (it usually shows sold-out / few left per time slot) — navigate to its URL rather than clicking "Buy" buttons.
- A click on "Buy / Reserve / Book" asks the user for approval. When the user said "stop before payment", it is fine to
  ask once to reach the date/time selection, but report prices and slots you already found in the same answer.

## 9. Multi-city trips
- Fix concrete dates first (e.g. day 1 = a date the user gave, or the next suitable date ≥ 2 weeks out, and say so),
  then search every leg on its own date in that same trip: SIN→ICN day 1, ICN→NRT day N, NRT→SIN last day. Never mix
  prices from unrelated dates. Also check a multi-city search (Google Flights "Multi-city") against the sum of one-ways.
- Cabin: what the request says; if it says nothing and memory has a cabin preference, use it and say so in one line.

## 10. Answer format
- A table first (option / price / time / key condition / link), then a 2–3 line recommendation with the reason.
- Before sending, check every summary sentence against the table: if an option misses a criterion (budget, rating,
  dates), say which ones meet it and which don't — never write "all meet …" when a row does not.
- Say what was not verified.
