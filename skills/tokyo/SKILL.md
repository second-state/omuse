---
name: tokyo
description: 东京指南（面向西方访客、商务旅行者和外籍居民）Tokyo for Westerners — entry rules and the airports, cash and IC cards, getting online, trains and rail passes, business customs, where to live and what it costs, food, what to see and day trips; use for any question about visiting, doing business in or moving to Tokyo.
browser: off
---
# Tokyo for Westerners / 東京

Tokyo (東京, Tōkyō) is Japan's capital and the core of one of the world's largest metropolitan areas: safe, clean,
punctual and enormous, with superb public transport, yet more cash-based, paper-based and rule-bound than Western
visitors expect, and with less spoken English than its signage suggests. This skill is a briefing for visitors,
business travellers and people relocating from Europe, North America or Australia. Figures were checked in
October 2026 and carry their date.

## How to use this skill
- Answer from this sheet; keep answers practical ("do X, then Y", with the Japanese name so the user can show it to
  a taxi driver or type it into a map: romaji + kanji, e.g. Shibuya 渋谷).
- **No browser.** Answer from this sheet: it is the curated data, and each volatile fact carries the date it was checked. For live numbers use data tools, never the browser: exchange rates → market_data; flights, hotels and prices → the travel data connector (mcp_* tools) when it is connected; the user's own bookings → mail and calendar via `trip-today`. When a fact here may have changed since its date (visa rules, fares, opening hours, new lines), say the date and name the official source from the list at the end; for a rule that decides whether the user can travel or enter, hand off to `entry-requirements`. A number not on this sheet and not from a data tool is "not in my data", not a guess.
- Compare to what the user knows: London, New York, Paris. Say when something normal in the West works differently
  here (cash, tipping, phone calls on trains, rubbish bins, eating while walking).
- Hand off to the specialised skills when the question is about a task: arrival and customs → `jp-visit-japan-web` (overview `arrival-cards`), visas and entry rules → `entry-requirements`, paying and SIMs → `payments-connectivity-asia`, transit tickets and rail passes → `getting-around-asia`, tax refunds → `tax-refund-shopping`, allergies and menus → `dining-dietary-translate`, emergencies → `travel-health-emergency`, today's bookings → `trip-today`, restaurants → `restaurant-booking` / `michelin-dining`.

## Orientation
- **Scale.** Tokyo Metropolis (東京都, Tōkyō-to) had an estimated **14.30 million** residents on 1 September 2026
  (Tokyo Metropolitan Government; being rebased on the 2025 census). About two-thirds live in the **23 special
  wards** (特別区), each with its own mayor and ward office; the rest in the Tama cities to the west and the islands.
 
- **Story.** The fishing village of Edo (江戸) became the shogun's capital in 1603 under Tokugawa Ieyasu and one of
  the world's largest cities by 1700; in 1868 (Meiji Restoration) the Emperor moved here and it was renamed Tokyo,
  "eastern capital". Flattened by the Great Kantō earthquake (1923) and the 1945 firebombing, it was rebuilt for the
  1964 Olympics and hosted the delayed 2020 Games in 2021: a city of many centres and constant redevelopment.
- **Neighbourhoods**:
  - *Marunouchi* 丸の内 / *Ōtemachi* 大手町 (Chiyoda 千代田区): Tokyo Station, Imperial Palace, head offices, banks.
  - *Ginza* 銀座 / *Nihonbashi* 日本橋 (Chūō 中央区): department stores, luxury, the old merchant quarter; Tsukiji 築地
    outer market; Tsukishima 月島 for monjayaki.
  - *Roppongi* 六本木, *Azabu* 麻布, *Hiroo* 広尾, *Akasaka* 赤坂, *Toranomon* 虎ノ門 (Minato 港区): embassies, foreign
    firms, expat housing, nightlife.
  - *Shibuya* 渋谷 / *Ebisu* 恵比寿 / *Omotesandō* 表参道 / *Harajuku* 原宿: the scramble crossing, tech start-ups,
    fashion, Meiji Shrine. *Shinjuku* 新宿: the world's busiest station, government towers, Kabukichō nightlife.
  - *Asakusa* 浅草 / *Ueno* 上野 (Taitō 台東区): Sensō-ji, museums, the old shitamachi downtown; *Akihabara* 秋葉原
    electronics and anime; *Ikebukuro* 池袋; *Odaiba* お台場 in the bay; *Setagaya* 世田谷 and *Meguro* 目黒 leafy
    residential; *Kichijōji* 吉祥寺 in the west.
- **Climate.** **Rainy season** (梅雨, tsuyu) early June to mid/late July; hot humid summers, regularly 35 °C+ in
  July–August with frequent heatstroke alerts (熱中症警戒アラート); **typhoons** mainly August–October (trains stop,
  flights cancel, stay in); cool sunny winters (0–12 °C). Best: late March–May (cherry blossom around late March and
  early April) and October–November.
- **Earthquakes.** Small tremors are routine and buildings are built for them. The government puts the chance of an
  M7 quake under the capital region at about 70 % within 30 years. Phones receive early warnings (緊急地震速報)
  automatically; on a strong shake drop, cover, hold on, do not run outside. The
  JNTO **Safety tips** app pushes alerts in English.

## Entering Japan and getting in from the airports
- **Visa waiver (verify at mofa.go.jp; hand off to `entry-requirements`).** Japan exempts 74 countries and regions
  (MOFA list, September 2025). The **US, Canada, Australia, New Zealand and EU/Schengen states** get up to **90 days**
  as a Temporary Visitor; **UK, Ireland, Germany, Austria, Switzerland, Liechtenstein and Mexico** nationals can
  extend to six months by applying to immigration in Japan before day 90 (verify for the UK). It covers tourism,
  meetings and conferences, **not paid work**. Those who need a visa: fees rose to **¥15,000 single / ¥30,000
  multiple entry** (from ¥3,000/¥6,000) for applications from 1 July 2026.
- **JESTA**, a pre-travel authorisation for visa-exempt visitors like the US ESTA, is planned for **fiscal 2028**
  (April 2028–March 2029); date and fee not set (reaffirmed in the Diet, April 2026). Nothing to do yet — verify.
- **Visit Japan Web** (Digital Agency): register passport, flight and customs declaration before departure and
  show one QR code at immigration and customs; paper forms still work.
  Automated gates do not stamp passports unless you ask — ask, since some shops still look for the stamp.
- **International Tourist Tax** (departure tax): **¥3,000 per departure since 1 July 2026** (was ¥1,000; Japan
  Tourism Agency), included in the air fare; tickets issued by 30 June 2026 kept the old rate.
- **Haneda (HND)** 羽田空港, the convenient airport (international flights at Terminal 3): Keikyū 京急 to Shinagawa
  品川 in about 15 min (~¥400–500), Tokyo Monorail 東京モノレール to Hamamatsuchō 浜松町 in 13 min (¥520).
- **Narita (NRT)** 成田空港, 60–70 km east: **Keisei Skyliner** スカイライナー to Nippori 日暮里 (36 min) and Ueno
  上野 (41 min), ¥2,580 walk-up, less online (2026 guides; verify on keisei.co.jp). **Narita Express** (N'EX 成田
  エクスプレス, JR) to Tokyo Station in about 55–60 min, then Shinagawa, Shibuya, Shinjuku, Yokohama; about
  ¥3,000–3,300 after JR East's March 2026 fare rise, with a discounted round-trip ticket for foreign passport holders
  (verify). Budget: Keisei Access Express (~¥1,300, 60 min) or the Airport Bus Tokyo–Narita to Tokyo Station/Ginza
  (about ¥1,300, verify). Taxi: fixed fares around ¥20,000–30,000 (verify).

## Money
- Yen (¥, JPY). **No tipping anywhere** — it confuses or offends. Shelf prices include 10 % consumption tax (8 % on
  takeaway food).
- **Cash still matters.** Cashless payment was 42.8 % of consumer spending in 2024 (METI). Cards and phones work in
  hotels, chains, department and convenience stores and most taxis; small restaurants, shrines, temples, markets,
  some ticket machines and lockers want cash. Carry ¥10,000–20,000.
- **ATMs that take foreign cards**: Seven Bank in every 7-Eleven (24 h, English), Japan Post Bank (shorter hours),
  AEON, Lawson and FamilyMart; most MUFG/Mizuho/SMBC machines do not. About ¥110–220 per withdrawal plus your bank's
  fee; limits ¥50,000–100,000 per transaction (2026, check on screen).
- **IC cards** (Suica, PASMO) pay for transport, convenience stores, vending machines and many shops. Contactless
  Visa/Mastercard/JCB taps work at a growing number of rail gates (mostly private lines), not yet everywhere.
- **Tax-free shopping changed on 1 November 2026** (verify with Japan Customs): you pay the full tax-inclusive price
  (show your passport; minimum ¥5,000 before tax per shop per day); on departure you complete the customs check at a
  kiosk or via Visit Japan Web **before check-in**, then the shop refunds the tax by its own method. Goods must be
  bought within **90 days** of departure, carried out with you, every item on the receipt present; the sealed-bag
  rule and the consumables cap are gone. Hand off to `tax-refund-shopping`.

## Getting online and the apps that matter
- No firewall: Google, WhatsApp and Western services work. A **travel eSIM** or airport pocket Wi-Fi is easiest; a
  voice SIM with a Japanese number needs a residence card.
- **Apps**: Google Maps (train routing with platforms, exits, fares), NAVITIME / Japan Transit Planner, **GO**
  (taxis, English, foreign cards; Uber also books licensed taxis), **Welcome Suica Mobile** or Suica/PASMO in Apple
  Wallet, **Tabelog** 食べログ (ratings; 3.5+ is very good), **TableCheck** / **Omakase** (bookings in English),
  Google Translate camera for menus, **LINE** (the messaging app shops and landlords use), **PayPay** (QR payments;
  full use needs a Japanese number)

## Getting around
- **IC card first.** Suica (JR East) and PASMO (private/metro) work on every Tokyo train and bus and across Japan.
  Visitors can buy a **Welcome Suica** (no deposit, 28 days) at airport stations or set up **Welcome Suica Mobile**
  (iPhone, since March 2025, valid 180 days; Green Car and shinkansen tickets in-app since spring 2026).
- **Three systems, one network.** JR East (the Yamanote loop 山手線 through Tokyo, Shinagawa, Shibuya, Shinjuku,
  Ikebukuro, Ueno; the Chūō line 中央線 west), **Tokyo Metro** (9 lines; listed October 2024) and **Toei** subway (4
  lines), plus private railways (Tokyu, Odakyu, Keio, Seibu, Tobu, Keisei, Keikyu) to the suburbs. Fares by distance,
  about ¥150–400 in town; JR East raised fares an average 7 % in March 2026 (Tokyo–Shinjuku now ¥260).
- **Rush hour** 07:30–09:30 is crush-loaded (women-only cars in the morning). **Last trains** around 00:00–00:30;
  then it is a taxi, a capsule hotel or karaoke till the first train.
- **Train manners**: no phone calls (silent mode, マナーモード), low voices, backpack on your front, queue at the
  marked door positions, priority seats by the doors; no eating on commuter trains (fine on the shinkansen).
- **Taxis**: clean, metered, automatic doors (don't touch). Flag fall about ¥500 for the first ~1 km, about 20 %
  more late at night (verify). Show the address in Japanese or book with **GO**; most take cards and IC.
- **Shinkansen** 新幹線 from Tokyo/Shinagawa on the Tōkaidō line (Nozomi about 2 h 30 min to Shin-Osaka, ¥14,720
  reserved — verify) and from Tokyo/Ueno north (Tōhoku, Jōetsu, Hokuriku). Book on smartEX or JR-EAST Train
  Reservation; oversized luggage on the Tōkaidō line needs a reserved space.
- **Japan Rail Pass** (JR only): ordinary 7/14/21 days **¥50,000 / ¥80,000 / ¥100,000** on the official site,
  **¥53,000 / ¥84,000 / ¥105,000** from overseas agents since 1 October 2026 (Green ¥70,000–147,000). Since the
  October 2023 rise (7 days used to be ¥29,650) a Tokyo–Kyoto–Osaka round trip no longer pays off; it suits long
  multi-region trips within a week. Nozomi needs an extra ticket. Hand off to `getting-around-asia`.

## Business scene and customs
- **Why it matters.** Tokyo hosts most Japanese giants (Sony, Mitsubishi, Mitsui, SoftBank, NTT, Hitachi, the
  megabanks), the Tokyo Stock Exchange (Kabutochō) and most foreign firms' Japan offices (Marunouchi, Toranomon,
  Roppongi; Shibuya for tech).
- **Meishi** 名刺 (business cards): bring plenty, ideally bilingual. Give and receive with both hands, text facing
  the recipient, a slight bow; read it and keep it on the table in seating order during the meeting; never write on
  it or pocket it casually.
- **Hierarchy**: the senior guest sits farthest from the door (上座, kamiza); wait to be seated; surname + *-san*
  (never for yourself). Bows replace handshakes, though many will offer a hand to foreigners.
- **Decisions** come by consensus: **nemawashi** 根回し (groundwork before meetings) and **ringi** 稟議 (written
  approval circulation) mean meetings confirm rather than decide, and answers take weeks. A direct "no" is rare:
  "that is difficult" (難しい) or "we will consider it" (検討します) usually means no. Send materials in advance.
- **Nomikai** 飲み会 (after-work drinks) is where trust forms: wait for the *kanpai* 乾杯, pour for others not
  yourself, top up the senior person's glass; soft drinks are fine.
- Punctuality is absolute. Dark suits; "Cool Biz" (no tie) May–September. Wrapped gifts (omiyage) from home are
  appreciated.

## Living here
- **Where expats live**: Minato 港区 (Azabu, Hiroo, Roppongi, Akasaka: schools, embassies, English clinics, top
  rents), Shibuya/Ebisu and Meguro (younger), Setagaya 世田谷 (families, space), Bunkyō 文京区 (quiet), and Kichijōji or Yokohama for value.
- **Costs** (approximate, 2026): studio/1K in central wards ¥100,000–130,000 a month; 1LDK across the 23 wards
  ¥120,000–180,000, expat-grade flats in Minato ¥300,000+. 23-ward rents rose 6.0 % year on year in Q3 2026
  (Savills). Move-in costs 4–6 months' rent (deposit, key money 礼金 reikin, agent fee, guarantor company);
  foreigner-friendly agents and "no key money" listings help. A set lunch ¥1,000–1,500.
- **Arriving to live**: long-term arrivals receive a **residence card** (在留カード) at the major airports; within
  **14 days** of moving in register your address at the **ward office** (区役所, kuyakusho), enrol in National
  Health Insurance (or your employer's; 70 % of costs covered) and the pension, and apply for a **My Number**
  card. Carry the residence card at all times.
- **Health**: ambulance/fire **119**, police **110**, medical advice **#7119**; JNTO Visitor Hotline
  **050-3816-2787** (24 h, English). English-speaking care: St. Luke's International Hospital 聖路加国際病院 (Tsukiji),
  Tokyo Medical University Hospital 東京医科大学病院 (Shinjuku), clinics in Roppongi and Hiroo; the AMDA
  International Medical Information Center gives referrals. Some medicines (stimulant ADHD drugs, strong codeine)
  are banned or need an import certificate — check before flying.
- **Schools** (often ¥2.5–4 million a year, verify): The American School in Japan (Chōfu), The British School in
  Tokyo, Nishimachi International School, St. Mary's International School (boys), Seisen International School
  (girls), Tokyo International School, Lycée français international de Tokyo, Deutsche Schule Tokyo
  Yokohama, Yokohama International School. Apply early; waitlists are common.
- **Daily life rules**: **rubbish sorting** by ward (burnable, non-burnable, plastics, cans/bottles/PET, on set days;
  large items need a paid sticker) — the top complaint from neighbours. Few public bins: carry rubbish home. Smoking only in designated outdoor areas (fines in many wards). Quiet after
  22:00 in flats. Crime is low; lost wallets usually come back via the police box (交番, kōban).

## Food
- Tokyo has more Michelin-starred restaurants than any other city. Local style is **Edomae** 江戸前: sushi (counter
  omakase ¥20,000–50,000+ down to conveyor belt), tempura, soba, unagi, **monjayaki** もんじゃ焼き in Tsukishima,
  chanko-nabe in Ryōgoku; plus ramen, yakitori, izakaya and department-store food halls (デパ地下, depachika). Toyosu 豊洲 is the wholesale fish market (tuna
  auction viewing); the Tsukiji outer market stays for breakfast.
- **Reservations**: top places book out 1–3 months ahead, some only by Japanese phone or introduction; use
  TableCheck, Omakase or the hotel concierge; no-shows are charged. Hand off to `restaurant-booking` / `michelin-dining`.
- **Dietary needs are hard**: dashi (fish stock) is in almost everything, so "vegetable" dishes often are not;
  pork and mirin (alcohol) are common. Look for shōjin ryōri 精進料理 (temple cuisine), dedicated vegan and
  certified halal places, and carry a Japanese allergy card. Hand off to `dining-dietary-translate`.

## What to see (a weekend plus day trips)
- **Day 1, old Tokyo**: Sensō-ji 浅草寺 and Nakamise in Asakusa (before 08:00), Tokyo Skytree 東京スカイツリー
  (634 m), Ueno Park and the Tokyo National Museum, evening in Shinjuku (Golden Gai).
- **Day 2, modern Tokyo**: Meiji Jingū 明治神宮, Harajuku and Omotesandō, Shibuya Crossing and Shibuya Sky, teamLab
  (Planets in Toyosu, Borderless in Azabudai Hills), Ginza, Imperial Palace East Gardens (free, closed Mondays and
  Fridays), sunset from the free Metropolitan Government observatory.
- **Kamakura** 鎌倉 (1 h, JR Yokosuka line): the Great Buddha, Hase-dera, Tsurugaoka Hachimangū, Enoshima by the
  Enoden tram; packed at weekends.
- **Nikkō** 日光 (about 2 h, Tōbu limited express from Asakusa): Tōshō-gū, Ieyasu's lavish shrine-mausoleum, Kegon
  Falls, Lake Chūzenji; autumn colour mid–late October.
- **Hakone** 箱根 (about 85 min, Odakyu Romancecar from Shinjuku): onsen ryokan (tattoos may be refused in shared
  baths — ask), the ropeway over Ōwakudani, Lake Ashi with Fuji views on clear days; Hakone Free Pass.
- **Mount Fuji** 富士山: the Kawaguchiko 河口湖 lakes by bus or Fuji Excursion train (about 2 h). Climbing only in
  season (Yoshida trail 1 July–10 September in 2026) with a **¥4,000 fee and advance booking** on the Yamanashi
  side and a 4,000-a-day cap (verify each year).
- Also: Yokohama (30 min), the Ghibli Museum (sells out monthly), sumo in Ryōgoku (Jan, May, Sep).

## Timeline
| Year | Event |
|---|---|
| 1603 | Tokugawa Ieyasu makes Edo the shogunate capital |
| 1868 | Meiji Restoration; Edo renamed Tokyo |
| 1923 | Great Kantō earthquake |
| 1945 | Firebombing of 10 March kills about 100,000 |
| 1964 | Summer Olympics; Tōkaidō Shinkansen opens |
| 2021 | Delayed 2020 Olympics, without spectators |
| 2023 | JR Pass price rise; Azabudai Hills opens |
| 2024 | Tokyo Metro listed on the stock exchange |
| 2026 | JR East fare rise (March); departure tax ¥3,000 (1 July); tax-free refund system (1 November) |

## Sources to verify against
- Entry: MOFA mofa.go.jp (visa exemptions), Immigration Services Agency moj.go.jp/isa, Visit Japan Web
  vjw-lp.digital.go.jp; departure tax and tax-free: Japan Tourism Agency mlit.go.jp/kankocho, customs.go.jp, nta.go.jp.
- Travel: JNTO japan.travel, GO TOKYO gotokyo.org.
- Transport: JR East jreast.co.jp/e, japanrailpass.net, Tokyo Metro tokyometro.jp/en, Toei kotsu.metro.tokyo.jp,
  keisei.co.jp, Keikyu, Tokyo Monorail, Narita and Haneda airport sites.
- City facts and living: Tokyo Metropolitan Government metro.tokyo.lg.jp and toukei.metro.tokyo.lg.jp, ward office
  sites (rubbish, registration), the "himawari" medical information service, AMDA IMIC, embassy doctor lists.
- News: The Japan Times, NHK World, Nikkei Asia; Savills (rents).

## Remember what you learn (for next time)
Where the user stays or lives, their office or school area, dietary needs and the apps they have set up are worth
keeping: profile_suggest for profile fields (`custom:home city`, `custom:<city> address`, `custom:allergies`),
memory_remember for preferences ("prefers the metro to taxis in <City>"). Never store passport, card or ID numbers
outside the Vault.
In Tokyo, remember their airport (Haneda or Narita), their ward (rubbish rules, ward office), whether they use
Suica/PASMO or Welcome Suica Mobile, and whether they hold a residence card.
