---
name: hong-kong
description: 香港指南（面向西方访客、商务旅行者和外籍居民）Hong Kong for Westerners — entry and e-Channel, the Airport Express, Octopus and paying, getting around, finance and the talent visas, where to live, typhoon signals, dim sum and cha chaan teng, hikes, islands, Macau and the Shenzhen crossings; use for any question about visiting, doing business in or moving to Hong Kong.
browser: off
---
# Hong Kong for Westerners / 香港

Hong Kong (香港, Cantonese *Hēung Góng*, "fragrant harbour") is the Special Administrative Region of China at the
mouth of the Pearl River: a former British colony with its own currency, border, common-law courts and visa regime,
English as an official language, Asia's densest skyline and some of its best hiking. This skill is a briefing for
visitors, business travellers and people relocating from Europe, North America or Australia. Figures were checked
in October 2026 and carry their date.

## How to use this skill
- Answer from this sheet; keep answers practical ("do X, then Y"), with the Chinese name and the English/Cantonese
  name so the user can show it to a taxi driver (e.g. Sai Ying Pun 西營盤, Tsim Sha Tsui 尖沙咀). Hong Kong uses
  traditional characters; the street language is **Cantonese**, not Mandarin.
- **No browser.** Answer from this sheet: it is the curated data, and each volatile fact carries the date it was checked. For live numbers use data tools, never the browser: exchange rates → market_data; flights, hotels and prices → the travel data connector (mcp_* tools) when it is connected; the user's own bookings → mail and calendar via `trip-today`. When a fact here may have changed since its date (visa rules, fares, opening hours, new lines), say the date and name the official source from the list at the end; for a rule that decides whether the user can travel or enter, hand off to `entry-requirements`. A number not on this sheet and not from a data tool is "not in my data", not a guess.
- Compare to what the user knows: London, New York, Singapore. Say when Hong Kong differs from mainland China
  (Google, WhatsApp and Visa cards work; Shenzhen is another visa regime) and from the West (tiny flats, typhoon
  days off, service charge instead of tips).
- Hand off to the specialised skills when the question is about a task: arrival card → none needed for most visitors (overview `arrival-cards`; Shenzhen trips → `cn-arrival-card`), visas and entry rules → `entry-requirements`, paying and SIMs → `payments-connectivity-asia`, transit tickets and rail passes → `getting-around-asia`, tax refunds → `tax-refund-shopping`, allergies and menus → `dining-dietary-translate`, emergencies → `travel-health-emergency`, today's bookings → `trip-today`, restaurants → `restaurant-booking` / `michelin-dining`.
- Mainland questions (Shenzhen, Guangzhou) → the `shenzhen` skill.

## Orientation
- **Scale.** 7,518,300 people at mid-2026 (provisional, Census and Statistics Department, 18 August 2026), up
  0.3 % in a year as talent-scheme arrivals offset record-low births. About 1,110 km², most of it hills, country
  parks and islands.
- **Story.** Hong Kong Island was ceded to Britain in 1842 (Treaty of Nanking, after the First Opium War),
  Kowloon in 1860, and the New Territories leased for 99 years in 1898. Japanese occupation 1941–45; refugees from
  the mainland after 1949 built the factories, then finance and trade took over as China opened in 1978. Returned to
  China on **1 July 1997** under "one country, two systems" and the Basic Law. After the 2019 protests came the
  National Security Law (2020), electoral changes (2021) and the local Safeguarding National Security Ordinance
  (Article 23, March 2024). Chief Executive: John Lee.
- **Districts** (Chinese · English):
  - *Hong Kong Island*: Central 中環 (banks, Lan Kwai Fong 蘭桂坊 bars, SoHo), Sheung Wan 上環, Admiralty 金鐘
    (government), Wan Chai 灣仔 (convention centre, nightlife), Causeway Bay 銅鑼灣 (shopping), Mid-Levels 半山, The Peak
    山頂, Happy Valley 跑馬地, Sai Ying Pun 西營盤, Kennedy Town 堅尼地城, Quarry Bay 鰂魚涌 (offices), Southside: Repulse
    Bay 淺水灣, Stanley 赤柱, Pok Fu Lam 薄扶林.
  - *Kowloon* 九龍: Tsim Sha Tsui 尖沙咀 (harbourfront, hotels), Yau Ma Tei 油麻地 (Temple Street), Mong Kok 旺角
    (markets), Sham Shui Po 深水埗 (old Hong Kong, electronics), Kowloon Tong 九龍塘 (schools), West Kowloon 西九龍
    (museums, HSR terminus), Kai Tak 啟德 (old airport, sports park).
  - *New Territories* 新界: Sha Tin 沙田 (Science Park), Sai Kung 西貢 (seafood, beaches), Tseung Kwan O 將軍澳, the
    border towns.
  - *Lantau* 大嶼山: the airport (Chek Lap Kok 赤鱲角), Tung Chung 東涌, Discovery Bay 愉景灣 (car-free expat enclave),
    Disneyland, Tai O 大澳; plus 260-odd outlying islands such as Lamma 南丫島 and Cheung Chau 長洲.
- **Climate.** Subtropical: hot, humid and stormy May–September (31–34 °C), the best weather October–December,
  cool and grey January–March (12–18 °C, flats rarely heated), foggy humid spring. Typhoon season about May–November.
  The **Hong Kong Observatory** (MyObservatory app) issues:
  - **Typhoon signals**: **T1** (standby, a cyclone within 800 km), **T3** (strong wind; kindergartens close),
    **T8** (gale or storm: offices, schools and many shops close, buses and ferries stop, MTR runs a reduced
    service — go home when it is pre-announced, about two hours ahead), T9, **T10** (hurricane; rare, e.g. Mangkhut
    2018). After a severe storm the government may declare "extreme conditions" (stay home two more hours).
  - **Rainstorm warnings**: amber, red, **black** (over 70 mm an hour: stay where you are; flash floods and
    landslides).
  - Winter haze drifts in from the Pearl River Delta; check the AQHI.

## Entering and getting in from the airport
- **Visas (ImmD, checked October 2026).** Visa-free visits: **British citizens 180 days**; **US, Canada, Australia,
  New Zealand, Ireland and every EU state 90 days**. Bring an onward or return ticket and funds. A visit does not
  allow work (unpaid business meetings and conferences are fine). Mainland China is a separate border and visa regime.
- **No arrival card** for most visitors; you get a small landing slip with your permitted stay — keep it.
- **e-Channel**: residents and enrolled frequent visitors use the automated gates. Since 27 February 2026 visitors
  who have come through the airport twice in 24 months can enrol free. **Smart Departure**: visitors aged 11+ with
  e-passports from about 74 designated countries get a landing slip with the logo and can leave through the
  e-Channel without enrolling.
- **Hong Kong International Airport (HKG)**: three runways since November 2024; in-town check-in at Hong Kong
  Station for many airlines (verify for yours).
- **To town**: the **Airport Express** (機場快綫) reaches Kowloon in about 22 min and Hong Kong Station (Central) in
  about 24 min, every 10–12 min from about 05:50 to after midnight; a single to Hong Kong Station is around HK$120
  with Octopus (after the 2025 fare rise; verify on mtr.com.hk). Since June 2026 contactless Visa/Mastercard work at
  its gates too. Cheaper: buses A11 (Central/Causeway Bay) and A21 (Tsim Sha Tsui, Mong Kok). Red taxis to the
  centre; blue Lantau taxis stay on Lantau.
- **Macau and Zhuhai by bridge**: the Hong Kong–Zhuhai–Macao Bridge (HZMB) port beside the airport has 24-hour shuttle
  buses to Macau (about 40 min).

## Money
- Currency is the Hong Kong dollar (HK$, HKD), pegged at 7.75–7.85 to the US dollar. Visa, Mastercard and
  contactless phones are widely accepted; Amex less so. Mainland Alipay and WeChat Pay work at most shops, plus local
  AlipayHK, WeChat Pay HK and PayMe.
- **Octopus** (八達通) is the stored-value card that runs the city: MTR, buses, minibuses, trams, Star Ferry,
  convenience stores, supermarkets, vending machines, many cafés and car parks. Buy one at an MTR customer service
  centre or the airport; on iPhone and Android it can live in the phone wallet; the Octopus app lets visitors top up
  with an overseas card (verify the current options). Keep HK$100–200 on it.
- **FPS** 轉數快 (Faster Payment System) is the instant local transfer by phone number or email — the local
  equivalent of PayNow; needs a Hong Kong bank account.
- **No sales tax or VAT**, so there is no tourist refund — shelf prices are final.
- **Tipping**: restaurants add a **10 % service charge**; nothing extra is expected. Cha chaan teng and dai pai
  dong add none. Round up taxis; tip hotel porters HK$10–20.
- Carry a few hundred dollars cash for wet markets, dai pai dong and some taxis.

## Getting online and the apps that matter
- No Great Firewall: Google and WhatsApp work. Tourist SIMs/eSIMs (csl, SmarTone, 3HK, China Mobile HK) at the
  airport and 7-Eleven; a "China + Hong Kong" SIM stays unfiltered in Shenzhen.
- **Must-have apps**: **MTR Mobile** (journeys, fares, service status), **Citymapper** (excellent here; also
  minibuses and ferries), **HKeMobility** (government journey planner, live traffic and bus arrivals), **Octopus**,
  **MyObservatory** (typhoon/rain warnings), **OpenRice** (restaurant reviews and bookings — the local Yelp),
  **Chope**, **foodpanda** and **Keeta** (delivery), **HKTaxi** and **Uber** (an October 2025 law licenses
  ride-hailing, first licensed platforms expected late 2026 at the earliest — verify), **WhatsApp** (how everyone,
  including businesses, messages).

## Getting around
- **MTR** 港鐵: nine heavy-rail lines plus the Airport Express, Light Rail in the northwest and the high-speed rail
  from West Kowloon; fast, cheap (HK$5–30 most trips), signed in English, about 06:00–01:00. Pay with Octopus or
  tap a contactless bank card or phone at the gates. East Rail runs to the Lo Wu and Lok Ma Chau borders.
- **Star Ferry** 天星小輪: Central or Wan Chai ↔ Tsim Sha Tsui, under 10 minutes, the best cheap ride in Asia.
  **Trams** ("ding ding" 叮叮) along the north of the island, flat fare. **Peak Tram** to The Peak.
- **Buses** (double-deckers, KMB and Citybus) and **red/green minibuses** (16–19 seats; shout "yáuh lohk!" 有落 to
  get off at the next stop, or press the bell) reach everywhere the MTR does not.
- **Taxis**: red (urban), green (New Territories), blue (Lantau); metered, cheap, tunnel tolls extra; show the
  Chinese address. Ask before relying on card payment.
- **Ferries** from Central Piers to Lamma, Cheung Chau, Mui Wo (Lantau) and Discovery Bay.

## Business scene and customs
- **Why it matters.** One of the world's top financial centres and Asia's largest equity fundraising venue in strong
  years (HKEX), the main gateway for capital into and out of mainland China, a hub for private banking, insurance,
  shipping, trading and regional headquarters. Common law, an independent judiciary in commercial matters, free
  capital flows, low simple taxes: salaries tax at a standard 15 % (16 % on income above HK$5 million), profits tax
  16.5 %, no capital gains, dividend or sales tax. Employers and employees each pay 5 % into the MPF pension (capped).
- **Where business happens**: Central and Admiralty (finance, law), Quarry Bay/Taikoo Place and Kowloon East
  (corporate offices), Science Park (Sha Tin) and Cyberport (Pok Fu Lam) for tech, Kai Tak and the **Northern
  Metropolis** 北部都會區 for the future: a government plan for a tech and housing hub along the Shenzhen border,
  with San Tin Technopole 新田科技城 (a development company set up 25 June 2026) and the Northern Metropolis
  Development Bill gazetted 3 July 2026.
- **Events**: Art Basel Hong Kong (March), Hong Kong Sevens (spring, at the **Kai Tak Sports Park**, 50,000-seat
  stadium, opened March 2025), HKTDC electronics fairs (April, October), FinTech Week (autumn).
- **Customs**: business cards with both hands; English is the business language (Cantonese for rapport, Mandarin
  for mainland counterparts); fast and deal-focused, but face and seniority matter; red packets (lai see 利是) at
  Chinese New Year for staff and doormen; avoid politics in small talk.
- **Visas for work** (Immigration Department; verify, rules change each Policy Address):
  - **General Employment Policy (GEP)**: a job offer from a Hong Kong employer.
  - **Top Talent Pass Scheme (TTPS)** 高端人才通行證計劃: Category A — annual income HK$2.5 million+ in the year before,
    **36 months** first stay; Category B — a degree from one of the **200 listed universities** (list updated 1 January
    2026) plus three years' work experience in the last five; Category C — such a degree within five years, under
    three years' experience, annual quota; B and C get **24 months**. No job needed to arrive; extensions need a job
    or business (normally up to three years; six years for top earners with HK$2 million+ assessable income).
  - **Quality Migrant Admission Scheme (QMAS)**: since late 2024 a checklist of 12 criteria (meet six), no quota.
  - **IANG** for graduates of Hong Kong universities: 24 months, no job offer needed.
  - Permanent residency after seven years of continuous ordinary residence.

## Living here
- **Where expats live**: Mid-Levels 半山 (towers, walk or escalator to Central, near schools), Sai Ying Pun / Kennedy
  Town (young professionals, cafés), Happy Valley 跑馬地 (families, racecourse), Southside — Repulse Bay, Stanley,
  Pok Fu Lam (space, beaches), The Peak, Discovery Bay 愉景灣 (car-free, ferry to Central), Sai Kung 西貢 (village
  houses, hiking), Kowloon Tong (schools).
- **Rents**: among the highest in the world per square foot and at a record — the government rental index reached
  its highest since 1993 and rose for eight straight months to June 2026 (205.8); agents expected a 5–8 % rise
  for 2026 (RTHK, July 2026). Roughly (2026, approximate, sizes in saleable sq ft): a 400–500 sq ft one-bedroom in Sai
  Ying Pun or Wan Chai HK$18,000–30,000 a month; family flats in Mid-Levels or Southside HK$50,000–120,000+
  (e.g. a 1,513 sq ft Mid-Levels flat at HK$110,000, Knight Frank, February 2026). Two-year leases with a one-year
  break clause, two months' deposit, agent's fee about half a month's rent.
- **Healthcare**: excellent public hospitals (Queen Mary, Queen Elizabeth, Prince of Wales), cheap for residents with
  an HKID and much dearer for non-residents; private hospitals (Hong Kong Sanatorium, Matilda, Adventist, Gleneagles,
  CUHK Medical Centre) need insurance. Emergency **999**.
- **Schools**: ESF (English Schools Foundation), Hong Kong International School (American), Canadian International
  School, German Swiss, French International, Harrow Hong Kong, Kellett, Chinese International School.
  Expect high fees, waitlists and often a **debenture or capital levy** on top — apply a year ahead.
- **Daily life and safety**: very low street crime and safe at night; the hazards are typhoons, heat, steep stairs
  and phone scams. Small flats mean life happens outside. Live-in domestic helpers are common. E-cigarettes are
  banned from sale and import.

## Food
- **Dim sum** 點心 and **yum cha** 飲茶 (har gow, siu mai, char siu bao, cheung fun) from morning to mid-afternoon:
  Maxim's Palace in City Hall (trolleys), Tim Ho Wan (budget, Michelin-listed).
- **Cha chaan teng** 茶餐廳: Hong Kong's diners — milk tea 奶茶 (pantyhose-strained), yuenyeung 鴛鴦 (coffee + tea),
  pineapple bun with butter 菠蘿油, macaroni soup, French toast, baked pork chop rice. Share tables, order fast.
- **Dai pai dong** 大牌檔 (open-air stalls, few left), roast goose and char siu (Joy Hing, Kam's Roast Goose),
  wonton noodles (Mak's), egg tarts, Sai Kung and Lamma seafood (ask the price per catty first).
- **Michelin**: Hong Kong has its own Michelin guide with many affordable Bib Gourmand and street-food picks; stars
  change yearly → `michelin-dining`. Book popular places on OpenRice or Chope; many local places do not take bookings.

## What to see (a weekend plus day trips)
- **Day 1, Island**: The Peak (Peak Tram or bus 15; walk the Lugard Road loop for the view), Central's old
  Police Station **Tai Kwun** 大館 (heritage and arts), Man Mo Temple 文武廟, Sheung Wan, the tram east, the Star Ferry
  across at dusk for the Symphony of Lights skyline.
- **Day 2, Kowloon**: Tsim Sha Tsui promenade, **M+** and the **Hong Kong Palace Museum** (West Kowloon), Temple
  Street night market, Mong Kok markets, Wong Tai Sin Temple 黃大仙祠, Chi Lin Nunnery.
- **Lantau**: Ngong Ping 360 cable car to the **Tian Tan Buddha** 天壇大佛, Tai O stilt-house village, Disneyland.
- **Outlying islands**: Lamma (village-to-village walk, seafood), Cheung Chau (spring bun festival); summer junks.
- **Hiking**: **Dragon's Back** 龍脊 to Big Wave Bay, the **MacLehose Trail** 麥理浩徑 (100 km; Tai Long Wan
  beaches on stage 2), Lion Rock. Start early, carry water, check the warnings.
- **Macau** 澳門 (day trip; passport, separate border, currency is the pataca but HK$ accepted): TurboJET or Cotai
  ferry about 1 h from Sheung Wan or Tsim Sha Tsui, or the HZMB bus from the airport side. Most Western passports
  enter visa-free (30–180 days by nationality; verify). The Historic Centre (UNESCO, 2005): Ruins of St Paul's,
  Senado Square, A-Ma Temple; Portuguese-Macanese food (egg tarts, minchi, African chicken); the Cotai Strip casinos.
- **Shenzhen** 深圳 (mainland China: visa or visa-free entry by nationality — see the `shenzhen` skill and
  `cn-arrival-card`): East Rail to Lo Wu 羅湖 (06:30–24:00) or Lok Ma Chau 落馬洲 (06:30–22:30), high-speed rail from West
  Kowloon to Futian (about 14 min), buses over the Shenzhen Bay bridge (06:30–24:00), or the 24-hour Huanggang
  crossing (verify).

## Timeline
| Year | Event |
|---|---|
| 1842 | Hong Kong Island ceded to Britain (Treaty of Nanking) |
| 1860 | Kowloon ceded |
| 1898 | New Territories leased for 99 years |
| 1941–45 | Japanese occupation |
| 1979 | First MTR line opens |
| 1997 | Handover to China (1 July) |
| 1998 | Chek Lap Kok airport and Airport Express open |
| 2018 | High-speed rail to Shenzhen/Guangzhou; Hong Kong–Zhuhai–Macao Bridge opens |
| 2020 | National Security Law |
| 2022 | Hong Kong Palace Museum opens; Top Talent Pass Scheme launched (December) |
| 2024 | Article 23 security ordinance (March); airport three-runway system commissioned (November) |
| 2025 | Kai Tak Sports Park opens (March); ride-hailing licensing law passed (October) |
| 2026 | TTPS list expanded to 200 universities (January); population 7.52 million (mid-year) |

## Sources to verify against
- Entry, visas, e-Channel, talent schemes: Immigration Department immd.gov.hk.
- Population: Census and Statistics Department censtatd.gov.hk; government press releases info.gov.hk.
- Transport: MTR mtr.com.hk; Transport Department td.gov.hk (HKeMobility); airport hongkongairport.com.
- Weather and warnings: Hong Kong Observatory hko.gov.hk; air quality aqhi.gov.hk.
- Northern Metropolis: nm.gov.hk; rents: Rating and Valuation Department rvd.gov.hk, Knight Frank and agency reports.
- Tourism: Hong Kong Tourism Board discoverhongkong.com; Macau: Macao Government Tourism Office macaotourism.gov.mo.
- News: SCMP, RTHK, The Standard.

## Remember what you learn (for next time)
Where the user stays or lives, their office or school area, dietary needs and the apps they have set up are worth
keeping: profile_suggest for profile fields (`custom:home city`, `custom:<city> address`, `custom:allergies`),
memory_remember for preferences ("prefers the metro to taxis in <City>"). Never store passport, card or ID numbers
outside the Vault.
In Hong Kong, also worth remembering: their visa route (visitor, TTPS, GEP — the type, not the number), whether
they hold an Octopus or HKID, and how often they cross to Shenzhen or Macau.
