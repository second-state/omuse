---
name: taipei
description: 臺北指南（面向西方访客、商务旅行者和外籍居民）Taipei for Westerners — visa-free entry and the online arrival card (TWAC), Taoyuan and the Airport MRT, paying and getting online, getting around, the tech scene and business customs, where to live, typhoons and earthquakes, what to see and day trips; use for any question about visiting, doing business in or moving to Taipei.
browser: off
---
# Taipei for Westerners / 臺北

Taipei (臺北 / 台北, Táiběi) is Taiwan's capital: a compact, green, mountain-ringed city of night markets, temples,
hot springs and the semiconductor economy, one of the safest and easiest cities in Asia for a first visit, with
Google Maps, Uber and Western cards all working — but Mandarin (in traditional characters) on most menus. This skill
is a briefing for visitors, business travellers and people relocating from Europe, North America or Australia.
Figures were checked in October 2026 and carry their date.

## How to use this skill
- Answer from this sheet; keep answers practical ("do X, then Y"), with the Chinese name in traditional characters
  plus the romanisation so the user can show it to a taxi driver, e.g. Da'an 大安. Taipei street signs use Hanyu
  Pinyin, but many older names use Wade-Giles (Hsinchu, Tamsui, Beitou).
- **No browser.** Answer from this sheet: it is the curated data, and each volatile fact carries the date it was checked. For live numbers use data tools, never the browser: exchange rates → market_data; flights, hotels and prices → the travel data connector (mcp_* tools) when it is connected; the user's own bookings → mail and calendar via `trip-today`. When a fact here may have changed since its date (visa rules, fares, opening hours, new lines), say the date and name the official source from the list at the end; for a rule that decides whether the user can travel or enter, hand off to `entry-requirements`. A number not on this sheet and not from a data tool is "not in my data", not a guess.
- Compare to what the user knows: Tokyo, Hong Kong, Singapore. Say when something normal in the West works
  differently here (cash at night markets, scooters, typhoon days off, 110 V power).
- Taipei is not mainland China: different currency (New Taiwan dollar, NT$), entry rules, no internet censorship,
  traditional characters and its own apps (LINE, not WeChat).
- Hand off to the specialised skills when the question is about a task: arrival card → `tw-arrival-card` (overview `arrival-cards`), visas and entry rules → `entry-requirements`, paying and SIMs → `payments-connectivity-asia`, transit tickets and rail passes → `getting-around-asia`, tax refunds → `tax-refund-shopping`, allergies and menus → `dining-dietary-translate`, emergencies → `travel-health-emergency`, today's bookings → `trip-today`, restaurants → `restaurant-booking` / `michelin-dining`.

## Orientation
- **Scale.** About 2.4–2.5 million registered residents in Taipei City (2025–26, slowly falling), 272 km², 12
  districts; New Taipei 新北, which wraps around it, adds about 4 million, and greater Taipei with Keelung is roughly
  7 million of Taiwan's 23 million. UTC+8; 110 V, US-style plugs (A/B).
- **Story.** A Qing-era trading town; walled city built in 1884, capital of the Qing province of Taiwan; Japanese
  colonial capital 1895–1945 (the Presidential Office building dates from then); seat of the Republic of China
  government since 1949; martial law ended in 1987 and Taiwan is now a full democracy (first direct presidential
  election 1996). Taipei 101 (508 m) opened in 2004 and was the world's tallest building until 2010.
- **Districts** (traditional characters, pinyin):
  - *Zhongzheng* 中正: Taipei Main Station 台北車站, Presidential Office, Chiang Kai-shek Memorial Hall, ministries.
  - *Da'an* 大安: leafy, central, Daan Forest Park, Yongkang Street 永康街, National Taiwan University; the favourite
    of expats and families.
  - *Xinyi* 信義: the modern CBD, Taipei 101, department stores, Elephant Mountain 象山; priciest.
  - *Zhongshan* 中山: hotels, Japanese-era lanes, bars, Taipei Fine Arts Museum; *Songshan* 松山: the downtown airport,
    Raohe night market, Minsheng Community 民生社區 (quiet, tree-lined).
  - *Wanhua* 萬華: the oldest quarter, Longshan Temple, Ximending 西門町 (youth shopping); *Datong* 大同: Dihua Street
    迪化街 and Dadaocheng 大稻埕 on the river.
  - *Shilin* 士林: night market, National Palace Museum, Tianmu 天母 (the long-time expat enclave, American and
    Japanese schools); *Beitou* 北投: hot springs; *Neihu* 內湖 and *Nangang* 南港: tech parks and the exhibition centre.
- **Climate and hazards.** Subtropical and humid: hot summers (33–36 °C, afternoon thunderstorms), "plum rain" 梅雨
  in May–June, **typhoon season** roughly July–October — when the city declares a typhoon day off 颱風假, offices and
  schools close and you stay indoors; winters are mild (10–18 °C) but damp and grey, with little indoor heating.
  **Earthquakes** are frequent: buildings are engineered for them, phones get alerts 國家級警報; drop, cover, hold on.
  The M7.2 Hualien earthquake of 3 April 2024 was felt strongly in Taipei. The annual Han Kuang/urban-resilience
  drills (summer) briefly clear the streets — check dates. Air quality is moderate, worse in winter.

## Entering Taiwan and getting in from the airports
- **Visa-free entry** (Bureau of Consular Affairs list, checked October 2026; verify at boca.gov.tw): **90 days** for
  ordinary passports of the US, UK (British citizens), Canada, Australia, New Zealand, all EU states, Switzerland,
  Norway, Iceland, Japan, South Korea, Israel and others; extensions are not allowed, except that Canadian and UK
  citizens may extend to 180 days. Passport valid for at least 6 months on entry and a confirmed onward or return
  ticket. Singapore and Malaysia get 30 days; Thailand, Brunei and the Philippines 14 days (trial to 31 July 2027).
  No paid work on visa-free entry; business meetings are fine.
- **Online arrival card (TWAC)** 入國登記表: since **1 October 2025** paper cards were discontinued and foreign
  visitors without residence file the online form at twac.immigration.gov.tw, up to 3 days before arrival
  (free; kiosks at Taoyuan for those without a phone). ARC holders do not file. Step-by-step: `tw-arrival-card`.
- **Taoyuan International Airport (TPE)** 桃園國際機場, about 40 km west, Terminals 1 and 2.
  - **Airport MRT** 機場捷運 (Taoyuan Metro): the purple *Express* runs to A1 Taipei Main Station in about 35–39 min,
    NT$160 (per 2026 guides; verify at tymetro.com.tw), every 15 min or so; the *Commuter* stops everywhere and takes
    about 50 min. Pay with EasyCard or contactless card, or buy a token.
  - **Buses** (Kuo-Kuang 國光 1819 to Main Station, others to Songshan/Xinyi) take about an hour; **taxis** from the
    official rank take about 40–60 min (roughly NT$1,100–1,500 per 2026 guides; verify).
- **Taipei Songshan (TSA)** 松山機場: the downtown airport on the brown Wenhu metro line, 10 min from Xinyi; flights to
  Tokyo Haneda, Seoul Gimpo, Shanghai Hongqiao and domestic islands (Kinmen, Penghu, Matsu).
- **High-speed rail (THSR)** 高鐵 also stops at Taoyuan HSR station, linked to the airport by the Airport MRT.

## Money
- Currency: New Taiwan dollar (NT$, TWD). Restaurants with table service often add a 10 % service charge; otherwise
  no tipping.
- **Cards**: Visa/Mastercard (and Apple Pay / Google Pay with foreign cards) are fine at hotels, department stores,
  chains, convenience stores and most taxis; **night markets, small eateries, traditional breakfast shops and temples
  are cash only** — carry NT$1,000–2,000 in small notes. ATMs at convenience stores (7-Eleven, FamilyMart) and banks
  take foreign cards. Local wallets (LINE Pay, JKOPay 街口, Taiwan Pay) mostly need a local bank account.
- **EasyCard** 悠遊卡 (or iPASS 一卡通): stored-value card for metro, buses, YouBike, trains and convenience stores; buy
  and top up (cash) at metro stations and convenience stores. The **TPASS** 基北北桃 30-day unlimited pass (NT$1,200 when
  launched in 2023, covering Taipei, New Taipei, Keelung and Taoyuan transit, YouBike and local trains) suits
  residents (verify current price).
- **Tax refunds** (5 % VAT; verify): at "TRS" stores, spend NT$2,000+ in a day at one store; the on-site
  small-amount refund covers NT$2,000–48,000 per store per day (yearly caps apply), larger amounts are claimed at the
  airport. A service fee is deducted, so you net roughly 4 %. See
  `tax-refund-shopping`.

## Getting online and the apps that matter
- No censorship; Google, WhatsApp and every Western service work, and free Wi-Fi ("iTaiwan", "TPE-Free") is common.
  SIMs and eSIMs at Taoyuan arrivals (Chunghwa Telecom 中華電信, Taiwan Mobile 台灣大哥大, FarEasTone 遠傳; passport
  plus a second ID).
- **LINE** is the national messenger — everyone, including businesses, landlords and doctors, uses it; set it up.
- **Google Maps** works well (transit, walking, bus arrival times). Bus Tracker / "台北等公車" apps for buses.
- **Taxis**: **Uber** operates (often taxis or licensed drivers), **55688** (Taiwan Taxi 台灣大車隊) and **LINE Taxi**
  book yellow cabs in English-friendly apps.
- **Delivery**: Uber Eats and foodpanda. Shopping: momo, PChome 24h, Shopee. Booking: inline / EZTABLE for
  restaurants; THSR app T Express and the TR (Taiwan Railway) app for trains.

## Getting around
- **Taipei Metro (MRT)** 台北捷運: about 152 km and 131 stations (2025; Taipei Rapid Transit Corporation lines), six
  colour-coded lines plus New Taipei's Circular Line and Sanying Line (opened 30 June 2026); clean, punctual, signed
  in English, roughly 06:00–24:00; NT$20–65 a ride. **No eating or drinking — not even water or gum — past the
  gates**, fines apply. Stand on the right on escalators. Wenhu 文湖 (brown) serves Songshan airport and Neihu.
- **Buses** are cheap and thorough (tap EasyCard when boarding and/or alighting as displayed); **YouBike** 2.0 public
  bikes everywhere (tap EasyCard; register via the app).
- **Taxis** are yellow, metered and inexpensive; few drivers speak English — show the address in Chinese.
- **Scooters** dominate the roads: look both ways at every crossing, even on green; pedestrians are not prioritised.
  Driving: right-hand traffic; an International Driving Permit is accepted for short stays (verify rules for
  residents) but few visitors need a car in the city.
- **Intercity**: THSR from Taipei Main Station to Taichung (about 1 h) and Kaohsiung Zuoying (about 1.5–2 h); Taiwan
  Railway 台鐵 to Hualien, Yilan and Jiufen's Ruifang 瑞芳; buses from Taipei Bus Station next to Main Station.

## Business scene and customs
- **Why it matters**: the centre of the world's advanced chip supply chain — TSMC (HQ in Hsinchu, an hour south by
  THSR), MediaTek, UMC (Hsinchu), Foxconn/Hon Hai (Tucheng, New Taipei), ASUS (Beitou), Acer (Xizhi), Quanta and
  Wistron (AI servers) — plus banks and insurers in Xinyi. Neihu Science Park and Nangang Software Park host tech
  offices; Computex (early June) fills the Nangang Exhibition Center. AmCham Taiwan and the European Chamber (ECCT)
  are active.
- **Customs**: friendly and polite, less hierarchical than Japan or Korea but seniority matters. Exchange business
  cards with both hands; **LINE** replaces email for quick follow-ups. Relationships (關係, guānxì) and banquets
  build trust; the host orders; toast "gānbēi" 乾杯 (sip if you prefer). Avoid politics unless invited, and say
  "Taiwan", not "Chinese Taipei", in conversation. Gifts are wrapped and opened later; avoid clocks (a funeral
  connotation). Red envelopes 紅包 at Lunar New Year and weddings.
- **Work and residence**: the **Employment Gold Card** 就業金卡 (combined open work permit, residence visa and ARC for
  professionals, 1–3 years; apply online at goldcard.nat.gov.tw) is the easiest route; otherwise employer-sponsored
  work permits. Entrepreneur visas exist for founders.

## Living here
- **Where expats live**: Da'an 大安 (central, walkable, near schools and parks), Xinyi 信義 (modern towers, priciest),
  Zhongshan 中山 / Minsheng 民生社區 (quieter, good value), Tianmu 天母 (Shilin; suburban, near Taipei American and
  European schools), Neihu 內湖 (near tech offices). Budget seekers look at Wenshan 文山 or New Taipei (Yonghe,
  Banqiao).
- **Costs** (2026, listing-based guides; verify): a one-bedroom roughly NT$22,000–32,000 a month in Da'an and up to
  NT$40,000 in Xinyi, NT$17,000–28,000 in Songshan/Zhongshan; small studios 套房 (what many newcomers rent) around
  NT$15,000–20,000. Deposits are usually two months; utilities extra; landlords often prefer a year's lease. Rental
  sites: 591.com.tw (Chinese), Facebook expat groups, agents. A local meal NT$100–200, a night-market snack NT$50–100.
- **Health**: excellent and cheap. Residents with an ARC join **National Health Insurance** 全民健保 (employees at once,
  others after six months; verify). Hospitals with English services: National Taiwan University Hospital 台大醫院
  (International Medical Service Center), Taipei Veterans General 臺北榮總 (Beitou), Taiwan Adventist Hospital
  臺安醫院 (Songshan; Priority Care Center, Western-style but pricier), Mackay Memorial 馬偕, Taipei Medical University
  Hospital. Pharmacies and clinics on every street; 119 ambulance/fire, 110 police, 0800-011-765 tourist hotline.
- **Schools**: Taipei American School (Tianmu), Taipei European School (British, German and French sections),
  Dominican International School, Kang Chiao International School (New Taipei), Taipei Japanese School; fees are high
  and waitlists common — apply early.
- **ARC** 居留證: holders of a resident visa apply at a National Immigration Agency service station within 15 days of
  arrival (Gold Card holders already have one); you need it for a bank account, NHI and phone contracts in your name.
- **Daily life**: extremely safe (lost wallets come back); convenience stores pay bills, print, ship and sell
  meals 24 h. Rubbish is collected by **music-playing trucks** at set evening times — you bring your bags (official
  city-issued bags in Taipei) and recycling to the truck. English is decent among the young; Mandarin dominates, and
  Taiwanese Hokkien 台語 is heard among older people. Stray-dog packs on hiking trails, scooters and typhoons are the
  main hazards.

## Food
- Beef noodle soup 牛肉麵, braised pork rice 滷肉飯 (lǔròufàn), xiaolongbao 小籠包 (Din Tai Fung 鼎泰豐 began on Xinyi
  Road), gua bao 刈包, oyster omelette 蚵仔煎, stinky tofu 臭豆腐, pepper buns 胡椒餅, shaved ice 剉冰 and mango ice,
  bubble tea 珍珠奶茶 (invented in Taiwan), soy-milk breakfasts 豆漿 (Fuhang 阜杭豆漿: queue early). Night markets:
  Raohe 饒河, Ningxia 寧夏 (compact, food-focused), Shilin 士林 (biggest), Tonghua/Linjiang 臨江. Taipei has a
  Michelin Guide with many Bib Gourmand street stalls.
- **Vegetarian-friendly**: Buddhist vegetarian 素食 restaurants and self-serve buffets (pay by weight) are everywhere;
  "我吃素" (wǒ chī sù) = I'm vegetarian; "全素" = vegan. Allergies: carry a card in Chinese
  (`dining-dietary-translate`); peanut and shellfish are common in sauces.

## What to see (a weekend plus day trips)
- **Classic Taipei**: Taipei 101 台北101 (observatory on the 89th floor), Elephant Mountain 象山 hike for the skyline at
  sunset, National Palace Museum 國立故宮博物院 (the imperial collection; allow half a day), Chiang Kai-shek Memorial
  Hall 中正紀念堂 (guard change), Longshan Temple 龍山寺, Dihua Street and Dadaocheng wharf at sunset, Ximending,
  Huashan 1914 華山1914 and Songshan 松山文創園區 creative parks, Maokong Gondola 貓空纜車 to tea houses.
- **Day trips**:
  - **Beitou** 北投: hot-spring baths, Thermal Valley 地熱谷 and the Hot Spring Museum, by MRT to Xinbeitou 新北投.
  - **Yangmingshan** 陽明山 National Park: volcanic peaks, fumaroles at Xiaoyoukeng 小油坑, Qixingshan 七星山 hike
    (1,120 m), calla lilies in spring; bus from Jiantan or Shilin MRT.
  - **Jiufen** 九份: hillside gold-mining town of teahouses and lanterns (crowded at weekends); bus 1062 from near
    Zhongxiao Fuxing MRT or train to Ruifang then bus; combine with **Shifen** 十分 sky lanterns and waterfall on the
    Pingxi line 平溪線, or **Yehliu Geopark** 野柳 (the "Queen's Head" rock) on the north coast.
  - **Tamsui** 淡水 (river sunsets, Fort San Domingo) by MRT; **Wulai** 烏來 (Atayal village, hot springs) by bus.
  - **Taroko Gorge** 太魯閣 (Hualien, about 2 h by train): heavily damaged by the 3 April 2024 earthquake. As of
    August 2026 per park notices and guides, Highway 8 through the gorge opens only in set time windows, most famous
    trails (Shakadang, Swallow Grotto, Baiyang, Zhuilu) remain closed, a few short trails and Tianxiang have reopened,
    and the Qingshui Cliffs viewpoint at Chongde reopened July 2025. Check taroko.gov.tw before promising anything.

## Timeline
| Year | Event |
|---|---|
| 1884 | Taipei city walls completed under the Qing |
| 1895 | Japanese colonial rule begins (to 1945) |
| 1949 | ROC government relocates to Taipei |
| 1987 | Martial law lifted |
| 1996 | First metro line (Muzha) opens; first direct presidential election |
| 2004 | Taipei 101 opens |
| 2007 | Taiwan High Speed Rail opens |
| 2017 | Taoyuan Airport MRT opens |
| 2020 | Circular Line opens |
| 2024 | Hualien earthquake (3 April) closes Taroko Gorge |
| 2025 | Online arrival card TWAC replaces paper (1 October) |
| 2026 | Sanying metro line opens in New Taipei (30 June) |

## Sources to verify against
- Entry: Bureau of Consular Affairs boca.gov.tw (visa-exempt list); National Immigration Agency immigration.gov.tw
  and twac.immigration.gov.tw; Employment Gold Card goldcard.nat.gov.tw.
- Travel: Taiwan Tourism Administration eng.taiwan.net.tw; Taipei Travel travel.taipei; tourist hotline 0800-011-765.
- Transport: Taipei Metro english.metro.taipei; Taoyuan Metro tymetro.com.tw; THSR thsrc.com.tw; Taoyuan Airport
  taoyuan-airport.com.
- Parks: Taroko National Park taroko.gov.tw; Yangmingshan ymsnp.gov.tw.
- News in English: Focus Taiwan (CNA) focustaiwan.tw, Taipei Times, Taiwan News.

## Remember what you learn (for next time)
Where the user stays or lives, their office or school area, dietary needs and the apps they have set up are worth
keeping: profile_suggest for profile fields (`custom:home city`, `custom:<city> address`, `custom:allergies`),
memory_remember for preferences ("prefers the metro to taxis in <City>"). Never store passport, card or ID numbers
outside the Vault.
In Taipei, also note whether the user has an ARC or Gold Card, an EasyCard and LINE set up, and whether they are
vegetarian — Taipei's Buddhist vegetarian scene is worth pointing them to every time.
