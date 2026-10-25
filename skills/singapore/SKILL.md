---
name: singapore
description: 新加坡指南（面向西方访客、商务旅行者和外籍居民）Singapore for Westerners — entry and the SG Arrival Card, Changi and the MRT, paying and apps, getting around, regional HQs and employment passes, where to live and the rules that carry fines, hawker food, what to see and day trips to Johor; use for any question about visiting, doing business in or moving to Singapore.
browser: off
---
# Singapore for Westerners / 新加坡 · Singapura

Singapore is the island city-state at the tip of the Malay Peninsula: Asia's most Western-friendly hub, with English
as the working language, rule of law, a world-class airport and a reputation for being clean, safe, efficient,
expensive and strict. This skill is a briefing for visitors, business travellers and people relocating from
Europe, North America or Australia. Figures were checked in October 2026 and carry their date.

## How to use this skill
- Answer from this sheet; keep answers practical ("do X, then Y"), with the local name where it helps: Malay place
  names (Kampong Glam, Bukit Timah = "tin hill"), Chinese for hawker dishes (chicken rice 海南鸡饭), and Singlish
  where it explains something ("chope" = reserve a seat, "lah", "can or not?").
- **No browser.** Answer from this sheet: it is the curated data, and each volatile fact carries the date it was
  checked. For live numbers use data tools, never the browser: exchange rates → market_data; flights, hotels and
  prices → the travel data connector (mcp_* tools) when it is connected; the user's own bookings → mail and calendar
  via `trip-today`. When a fact here may have changed since its date (visa rules, fares, opening hours, new lines),
  say the date and name the official source from the list at the end; for a rule that decides whether the user can
  travel or enter, hand off to `entry-requirements`. A number not on this sheet and not from a data tool is "not in
  my data", not a guess.
- Compare to what the user knows: Hong Kong, London, Sydney, New York. Say when something normal at home is an
  offence here (vaping, eating on the MRT, littering) and when Singapore is simply easier (English everywhere,
  contactless everything, tap water is drinkable).
- Hand off to the specialised skills when the question is about a task: arrival card → `sg-arrival-card` (overview `arrival-cards`), visas and entry rules → `entry-requirements`, paying and SIMs → `payments-connectivity-asia`, transit tickets and rail passes → `getting-around-asia`, tax refunds → `tax-refund-shopping`, allergies and menus → `dining-dietary-translate`, emergencies → `travel-health-emergency`, today's bookings → `trip-today`, restaurants → `restaurant-booking` / `michelin-dining`.

## Orientation
- **Scale.** 6.21 million people at June 2026 (Population in Brief 2026, released 25 September 2026): 3.68 million
  citizens, 0.55 million permanent residents and 1.98 million non-residents (work-pass holders, students,
  dependants). Land area about 735 km² (half of Greater London), roughly 50 km east to west.
- **Story.** A Malay fishing settlement and Johor-Riau outpost (Temasek) until Stamford Raffles set up a British
  trading post in 1819; part of the Straits Settlements from 1826, it grew into the British Empire's great free port
  in Asia. Japanese occupation 1942–45. Self-government 1959 under Lee Kuan Yew's People's Action Party
  (PAP), merger into Malaysia 1963, independent on **9 August 1965** (National Day). Lee built the export-led,
  public-housing (HDB), meritocratic model; the PAP has governed ever since. Prime Minister Lawrence Wong since May 2024, re-elected May 2025.
- **People and language.** About three-quarters of residents are ethnic Chinese, with Malay, Indian and Eurasian
  communities; four official languages (English, Mandarin, Malay — the national language — and Tamil). English is
  the language of government, business and schooling; **Singlish** is the street creole. Racial and religious
  harmony is protected by law.
- **Areas** (centre outwards):
  - *Downtown Core / CBD*: Raffles Place, Marina Bay (Marina Bay Sands, Gardens by the Bay), Tanjong Pagar; the
    Civic District (Padang, National Gallery) by the Singapore River with Boat Quay and Clarke Quay.
  - *Orchard Road* and *River Valley*, *Tanglin*, *Dempsey*: shopping, embassies, expat condos.
  - Heritage quarters: *Chinatown* 牛车水 (Niu Che Shui), *Kampong Glam* (Sultan Mosque, Arab Street, Haji Lane),
    *Little India* (Serangoon Road, Tekka), *Tiong Bahru* (1930s Art Deco flats, cafés), *Joo Chiat / Katong*
    (Peranakan shophouses, laksa).
  - *Holland Village*, *Bukit Timah* (landed houses, schools, nature reserve), *Novena*, *one-north* (Biopolis,
    Fusionopolis), *Sentosa* (resort island), *East Coast* (beach park), *Changi* (airport), *Jurong* (west,
    industry), *Woodlands* (north, causeway to Johor).
- **Climate.** Equatorial: 25–33 °C and humid all year. Northeast monsoon December–March (wettest
  December–January), southwest monsoon June–September, inter-monsoon months with afternoon thunderstorms; no
  typhoons. **Haze** from Indonesian land fires can arrive in dry years, usually August–October (NEA publishes the
  PSI). Dengue is endemic; use repellent. Tap water is safe.

## Entering and getting in from the airport
- **Visas (verify per nationality on ica.gov.sg; checked October 2026).** Most Western passports enter visa-free:
  EU/EEA, Swiss, UK, US, Canadian, Australian and New Zealand citizens get a Short-Term Visit Pass at the border,
  commonly 30 or 90 days depending on nationality — the officer sets the stay and it is emailed as an e-Pass, the
  passport is not stamped. The visa-required list (India, Russia, Ukraine and others) is on the ICA site. Passport
  must be valid six months. Since **30 January 2026** ICA can issue **No-Boarding Directives**: airlines refuse
  boarding to travellers who are barred or lack a visa, valid passport or arrival card.
- **SG Arrival Card** (everyone, including residents): free on the ICA e-service or the MyICA app within the
  **3 days before arrival**; never use a paid site. Airside transit passengers do not file. Hand off to
  `sg-arrival-card` to fill it.
- **Immigration**: since 2024 all foreign visitors can use the **automated lanes** at Changi without enrolling
  (passport scan plus face and iris). Expect minutes, not queues.
- **Changi Airport (SIN)**: four terminals (T1–T4) plus **Jewel** (the Rain Vortex waterfall and gardens, opened
  2019, linked to T1–T3). **Terminal 5** broke ground in May 2025 and is due in the mid-2030s; a third runway is
  expected around end-2027.
- **To town**: the MRT East-West Line airport branch (green, Changi Airport → Expo → Tanah Merah, change for the
  city) runs about 05:30–24:00, roughly 45 min to Raffles Place, a few dollars with a contactless card. The
  Thomson-East Coast Line (brown) will take over the airport branch and reach T5 in the mid-2030s — **not yet**.
  Taxis and Grab take 20–30 min to the CBD, with an airport surcharge.

## Money
- Currency is the Singapore dollar (S$, SGD). **Contactless cards and phone wallets work almost everywhere**,
  including taxis and the MRT. Many hawker stalls take the SGQR/PayNow code (for local bank apps) and some take
  cards, but **carry some cash** for hawker centres and kopitiams. Amex is less accepted.
- **PayNow** is the instant local transfer system (by phone or ID number) residents use for everything.
- **GST is 9 %** (since January 2024), included in shop prices. Restaurant menus showing "++" add a **10 % service
  charge plus 9 % GST** on top. **Tipping is not expected** anywhere; rounding up a taxi is fine.
- **Tourist Refund Scheme**: GST back on goods from participating shops, minimum S$100 per shop, claimed at the
  eTRS kiosks at Changi with goods and passport, within two months → `tax-refund-shopping`.
- Banks: DBS/POSB, OCBC, UOB; an account needs a work pass. Wise and Revolut are common.

## Getting online and the apps that matter
- No censorship of Western services. eSIMs or a tourist SIM from Singtel, StarHub, M1 or SIMBA (passport needed).
- **Must-have apps**: **Grab** (ride hailing, food, payments — the default; Uber left in 2018), **Gojek**, **TADA**
  and **CDG Zig** (ComfortDelGro taxis), **Citymapper** or Google Maps (excellent transit data), **SimplyGo**
  (transit card account), MyTransport.SG (bus arrivals), **Chope** (restaurant bookings — also the Singlish word),
  **Burpple** (reviews), GrabFood and foodpanda (delivery), **MyICA** (arrival card), **myENV** (weather, haze,
  dengue), **Singpass** (residents' digital ID, with a long-term pass).

## Getting around
- **MRT**: six lines, about 243 km and 146 stations (July 2026), clean, air-conditioned, signed in four
  languages; runs roughly 05:30–24:00. The **Circle Line** became a full loop on **12 July 2026** (Keppel,
  Cantonment, Prince Edward Road opened; trains now show clockwise/anticlockwise). Thomson-East Coast Line Stage 5
  (Bedok South, Sungei Bedok) was due in the second half of 2026; check LTA whether it has opened. Plan: about
  460 km by 2040.
- **Pay**: tap a contactless Visa/Mastercard or phone wallet at the gates (SimplyGo, distance fares); EZ-Link
  cards are the local stored-value option. Buses take the same cards — tap in **and out**.
- **No eating or drinking on the MRT and buses** (fine up to S$500), no durians, no flammables.
- **Taxis** are metered and honest, with surcharges for peak hours, midnight, the CBD and the airport; Grab is often
  cheaper. Cars are very expensive (the COE ownership certificate alone runs to six figures); most expats do not drive.
- Covered walkways and underground malls link much of the centre; e-scooters are banned on footpaths.

## Business scene and customs
- **Why it matters.** Asia's leading regional-headquarters location and a top-four global financial centre: banks,
  asset managers and family offices (regulated by MAS), commodity traders, shipping, biomedical, semiconductors and
  the regional HQs of most Western multinationals and tech firms. Common-law courts, SIAC arbitration, corporate tax
  17 %, personal income tax up to 24 % for residents, no capital gains tax.
- **Where**: Raffles Place and Marina Bay Financial Centre (finance), one-north (tech, biomedical), Changi Business
  Park, Jurong (industry).
- **Big events**: Singapore FinTech Festival (November), Formula 1 night race (around October), Shangri-La
  Dialogue (June), Singapore Airshow (February, even years), ATxSG, SWITCH.
- **Customs**: punctual, polite and hierarchical; business cards given and received with both hands; first names
  are normal with Westerners but use titles with senior Singaporeans until invited; a soft "we will consider" can
  mean no; decisions can be consensus-driven; avoid public criticism ("face"); gifts modest (no clocks for Chinese
  hosts, no alcohol for Muslim hosts). Strict anti-corruption law: no gifts to officials.
- **Work passes** (MOM; verify current figures on mom.gov.sg):
  - **Employment Pass (EP)**: minimum qualifying salary **S$5,600 a month (S$6,200 in financial services)**, rising
    with age to about S$10,700 (S$11,800) at 45+; rising to **S$6,000 (S$6,600)** for new applications from
    **1 January 2027**. Plus the **COMPASS** points test (40 points from salary, qualifications, diversity and local
    hiring); exempt at a fixed salary of S$22,500 a month or more. Employers usually advertise on MyCareersFuture first.
  - **ONE Pass** (Overseas Networks & Expertise): five years, not tied to one employer, lets the spouse work; needs
    a fixed salary of **S$30,000 a month** (with an employer above a size threshold if abroad), or outstanding
    achievement in arts, sports, science or academia.
  - S Pass (mid-skilled), EntrePass (founders), Personalised Employment Pass; Dependant's Pass for family.

## Living here
- **Where expats live**: *River Valley / Orchard / Tanglin* (central condos); *Holland Village / Bukit Timah*
  (families, landed houses, near Tanglin Trust and UWCSEA Dover); *East Coast / Katong* (beach park, more space,
  near UWCSEA East); *Tiong Bahru / Tanjong Pagar* (young professionals, walk to the CBD); *Sentosa Cove*
  (waterfront villas); *Woodlands / Upper Bukit Timah* (near the Singapore American School).
  Foreigners cannot buy HDB flats and pay a 60 % Additional Buyer's Stamp Duty on private homes (verify).
- **Rents** (approximate, 2026): median two-bedroom condo rent about **S$5,300 a month in the Core Central Region,
  S$4,300 in the Rest of Central Region, S$3,600 Outside Central** (leases Q4 2025–Q3 2026, PropKaki from URA data);
  one-bedrooms in the centre roughly S$3,500–5,000; landed houses S$10,000–20,000+. Condo rents hit a record in
  2026 and were up about 2.4 % year on year in September 2026 (SRX). Leases are usually two years with a two-month
  deposit and a diplomatic clause.
- **Healthcare**: excellent and expensive; get international health insurance (employers usually provide it).
  Private: Mount Elizabeth, Gleneagles, Raffles Hospital; public: Singapore General, National University Hospital,
  Tan Tock Seng.
  Emergency: **995** ambulance/fire, **999** police.
- **International schools** (fees roughly S$35,000–55,000+ a year, plus application and building fees; popular
  schools have **waitlists, apply a year ahead**): United World College of South East Asia (UWCSEA, Dover and East),
  Tanglin Trust School (British), Singapore American School, Australian International School, Dulwich College,
  Stamford American, Canadian International School, German European School, Lycée Français, Nexus. Local
  schools take foreigners only if places remain.
- **Rules and fines you must know**: no chewing gum for sale or import; littering fines (S$300 first offence, higher with corrective work orders); smoking only in
  designated areas (Orchard Road is a no-smoking zone outside marked spots); **vaping is illegal** — possession is a
  S$700 fine for adults, devices are seized, repeat offenders can be barred from re-entry and **etomidate vapes
  ("Kpods") are a Class C drug** (deportation); bin any vape at Changi's disposal bins on arrival. Drug trafficking
  carries the death penalty. Jaywalking, feeding pigeons and not flushing are offences.
- **Daily life and safety**: one of the safest cities in the world at any hour; the real risks are heat, dengue
  and phone scams (fake "officials", fake ICA sites). Live-in domestic helpers are common in expat families.

## Food
- **Hawker culture** was inscribed on UNESCO's Representative List of the Intangible Cultural Heritage of
  Humanity in **December 2020**. Hawker centres are open-air food courts of licensed stalls, run by the government
  (NEA) with stalls at S$4–8 a dish. Etiquette: "chope" a table with a tissue packet, order at each stall, return
  trays (it is now required, with fines for not doing so), and queues signal the best stalls.
- **Where**: Maxwell (chicken rice), Lau Pa Sat (CBD, satay street at night), Chinatown Complex, Amoy Street,
  Tiong Bahru Market, Old Airport Road, Newton (touristy, check prices), Tekka (Little India).
- **What**: Hainanese chicken rice, laksa (Katong style), char kway teow, bak kut teh (pork rib soup), Hokkien mee,
  satay, nasi lemak, roti prata, mee goreng, fish-head curry, chilli crab and black pepper crab (seafood restaurants
  on East Coast; priced by weight — ask first), kaya toast, soft-boiled eggs and kopi for breakfast. Kopi code:
  kopi-O (black, sweet), kopi-C (evaporated milk), "kosong" (no sugar).
- **Michelin**: some hawker stalls hold Bib Gourmand listings (they change yearly → `michelin-dining`). Alcohol is
  heavily taxed.
- Halal food is everywhere (look for the MUIS certificate); vegetarian is easy in Little India and at Buddhist
  vegetarian stalls. Allergies: hawker stalls rarely list ingredients — use `dining-dietary-translate`.

## What to see (a weekend plus day trips)
- **Day 1, Marina Bay and the colonial core**: Gardens by the Bay (Supertrees, Cloud Forest, Flower Dome; free
  evening light show), Marina Bay Sands SkyPark, the Merlion, the National Gallery, a river walk to Clarke Quay;
  satay at Lau Pa Sat.
- **Day 2, neighbourhoods**: Chinatown (Buddha Tooth Relic Temple), Tiong Bahru, Kampong Glam (Sultan Mosque),
  Little India, Katong; the **Botanic Gardens** (UNESCO World Heritage since 2015, National Orchid Garden).
- **Nature**: MacRitchie TreeTop Walk, Bukit Timah Nature Reserve, the Southern Ridges; Mandai wildlife parks
  (Zoo, Night Safari, Bird Paradise).
- **Sentosa**: Universal Studios, S.E.A. Aquarium, beaches — Sentosa Express from VivoCity, cable car or the
  Boardwalk on foot.
- **Pulau Ubin**: a bumboat from Changi Point Ferry Terminal (cash, leaves when full) to the last kampong island;
  cycle to Chek Jawa wetlands.
- **Johor Bahru (Malaysia)**: passport, about 1 h each way in traffic. Today: the KTM **Shuttle Tebrau** train
  Woodlands–JB Sentral (5 min) or buses across the Causeway (CW1/CW2, SBS 160/170) — avoid Friday evenings and
  holiday peaks. The **RTS Link** (Woodlands North ↔ Bukit Chagar, 4 km, co-located immigration) was in testing in
  August 2026; RTS Operations said on 2 October 2026 that passenger service starts in **February 2027**; the fare is announced jointly — check LTA. Shuttle Tebrau is due to stop about six months after RTS opens (under negotiation).
  Malaysia requires its own arrival card (`my-arrival-card`).
- **Further**: ferries to Batam and Bintan (Indonesia) from HarbourFront and Tanah Merah; buses to Malacca (3–4 h).

## Timeline
| Year | Event |
|---|---|
| 1819 | Stamford Raffles founds a British trading post |
| 1826 | Becomes part of the Straits Settlements |
| 1942–45 | Japanese occupation after the fall of Singapore |
| 1959 | Self-government; Lee Kuan Yew becomes prime minister |
| 1963 | Merger into Malaysia |
| 1965 | Independence on 9 August |
| 1987 | First MRT line opens |
| 2015 | Botanic Gardens become a UNESCO World Heritage Site; Lee Kuan Yew dies |
| 2019 | Jewel Changi opens |
| 2020 | Hawker culture inscribed by UNESCO (December) |
| 2024 | Lawrence Wong becomes prime minister; automated immigration lanes for all visitors |
| 2025 | Changi Terminal 5 construction begins (May) |
| 2026 | No-Boarding Directives (30 January); Circle Line loop completed (12 July); population 6.21 million (June) |
| 2027 | RTS Link to Johor Bahru expected (February); EP salary threshold rises (1 January) |

## Sources to verify against
- Entry, visas, SG Arrival Card, automated lanes: Immigration & Checkpoints Authority ica.gov.sg.
- Work passes and salaries: Ministry of Manpower mom.gov.sg (EP, COMPASS, ONE Pass).
- MRT, buses, RTS Link: Land Transport Authority lta.gov.sg; Changi Airport changiairport.com.
- Population: Population in Brief (population.gov.sg / strategygroup.gov.sg) and SingStat singstat.gov.sg.
- Rents and property: URA ura.gov.sg rental data; SRX and PropKaki monthly reports.
- GST and tourist refunds: IRAS iras.gov.sg, Singapore Customs customs.gov.sg.
- Haze, weather, dengue, hawker centres: NEA nea.gov.sg; Meteorological Service weather.gov.sg.
- Rules and vaping: HSA hsa.gov.sg; tourism: visitsingapore.com; news: The Straits Times, CNA, Business Times.

## Remember what you learn (for next time)
Where the user stays or lives, their office or school area, dietary needs and the apps they have set up are worth
keeping: profile_suggest for profile fields (`custom:home city`, `custom:<city> address`, `custom:allergies`),
memory_remember for preferences ("prefers the metro to taxis in <City>"). Never store passport, card or ID numbers
outside the Vault.
In Singapore, also worth remembering: whether they hold a work pass (EP, ONE Pass, Dependant's Pass — the type, not
the number), their condo area or school shortlist, and whether they cross to Johor regularly.
