---
name: bangkok
description: 曼谷指南（面向西方访客、商务旅行者和外籍居民）Bangkok for Westerners — visa exemption, TDAC and the airports, money and apps, BTS/MRT and boats, business customs and long-stay visas, where to live, health and the laws that catch foreigners, food and day trips; use for any question about visiting, doing business in or moving to Bangkok.
browser: off
---
# Bangkok for Westerners / กรุงเทพฯ

Bangkok (กรุงเทพมหานคร, Krung Thep Maha Nakhon, "Krung Thep" for short) is Thailand's capital and its only real
metropolis: royal temples and canals beside glass towers and malls, cheap and easy for a visitor, with excellent
private hospitals and a large expat community, but with traffic, heat and seasonal haze that surprise newcomers.
This skill is a briefing for visitors, business travellers and people relocating from Europe, North America or
Australia. Figures were checked in October 2026 and carry their date.

## How to use this skill
- Answer from this sheet; keep answers practical ("do X, then Y"), with the Thai name so the user can show it to a
  driver: romanisation + Thai script, e.g. Silom สีลม. Romanisation varies (Thong Lo / Thonglor) — give both.
- **No browser.** Answer from this sheet: it is the curated data, and each volatile fact carries the date it was checked. For live numbers use data tools, never the browser: exchange rates → market_data; flights, hotels and prices → the travel data connector (mcp_* tools) when it is connected; the user's own bookings → mail and calendar via `trip-today`. When a fact here may have changed since its date (visa rules, fares, opening hours, new lines), say the date and name the official source from the list at the end; for a rule that decides whether the user can travel or enter, hand off to `entry-requirements`. A number not on this sheet and not from a data tool is "not in my data", not a guess.
- Compare to what the user knows: Singapore, London, New York. Say when something normal in the West is different
  here (the monarchy, drugs and vapes, crossing the street, tipping).
- Hand off to the specialised skills when the question is about a task: arrival card → `th-arrival-card` (overview `arrival-cards`), visas and entry rules → `entry-requirements`, paying and SIMs → `payments-connectivity-asia`, transit tickets and rail passes → `getting-around-asia`, tax refunds → `tax-refund-shopping`, allergies and menus → `dining-dietary-translate`, emergencies → `travel-health-emergency`, today's bookings → `trip-today`, restaurants → `restaurant-booking` / `michelin-dining`.

## Orientation
- **Scale.** 5.42 million registered residents at end-2025 (Central Registration Office, published March 2026, a
  record low), but 10–11 million people live in the city and its suburban provinces (Nonthaburi, Samut Prakan,
  Pathum Thani) once unregistered residents are counted. Thailand: 65.8 million. 50 districts (khet เขต), 1,569 km².
- **Story.** Capital since 1782, when King Rama I of the Chakri dynasty moved the court across the Chao Phraya from
  Thonburi after Ayutthaya fell (1767). A city of canals (khlong คลอง) until roads replaced most of them. Absolute
  monarchy ended in 1932; King Rama X (Maha Vajiralongkorn) has reigned since 2016. Politics swings between elected
  governments, courts and coups (2006, 2014); avoid demonstrations.
- **Neighbourhoods:**
  - *Rattanakosin* รัตนโกสินทร์ (old royal island: Grand Palace, Wat Pho), *Banglamphu* บางลำพู and Khao San Road
    ถนนข้าวสาร (backpackers), *Yaowarat* เยาวราช (Chinatown, street food at night).
  - *Silom* สีลม / *Sathorn* สาทร: finance district, embassies, Lumphini Park สวนลุมพินี, rooftop bars.
  - *Siam* สยาม / *Chit Lom* ชิดลม: the mall core (Siam Paragon, CentralWorld), Erawan Shrine.
  - *Sukhumvit* สุขุมวิท: the expat spine along the BTS — Nana and Asok อโศก (hotels, nightlife, interchange), Phrom
    Phong พร้อมพงษ์ (families, Japanese community), Thong Lo ทองหล่อ and Ekkamai เอกมัย (bars, cafés), On Nut อ่อนนุช
    (cheaper).
  - *Ari* อารีย์ (leafy, cafés), *Ratchada* รัชดา (night markets), *Riverside*/Charoen Krung เจริญกรุง and Iconsiam;
    exhibition venues at Bang Na บางนา (BITEC) and Muang Thong Thani เมืองทองธานี (IMPACT).
- **Climate.** Hot all year: "cool" and dry Nov–Feb (25–32 °C), hot Mar–May (up to 35–40 °C), rainy May/June–Oct
  with violent afternoon downpours that flood streets for an hour or two. **Haze/PM2.5** peaks January–March (crop
  burning, traffic): check the AQI, wear an N95; schools may close on bad days. Drink bottled or filtered water.

## Entering Thailand and getting in from the airports
- **Visa exemption — changed on 15 September 2026** (verify at mfa.go.th). The 60-day visa-free stay (July 2024–
  September 2026) became a **30-day exemption** for about 60 countries and territories, including the **US, UK,
  Canada, Australia, New Zealand, all EU states, Switzerland, Norway, Iceland**, Japan, Singapore and India (TAT
  Newsroom, September 2026). Entries on or before 14 September keep their 60-day stamp. **By land**, visa-free
  entries are limited to **two per calendar year** (ASEAN neighbours Malaysia, Singapore, Indonesia, Brunei
  excepted). A 30-day extension at an immigration office has been possible — verify. Immigration may ask for an
  onward ticket, an address and proof of funds; repeated "visa runs" are refused. Passport valid 6 months.
- **TDAC** (Thailand Digital Arrival Card): mandatory since 1 May 2025 for every foreigner arriving by air, land or
  sea; free, within the **3 days before arrival**, only at tdac.immigration.go.th (fee-charging sites are scams).
  Use `th-arrival-card`.
- **Planned tourist entry fee**: 450 baht per foreign arrival (air first, land and sea later) was under committee
  review in October 2026, targeting early 2027 (Khaosod English, 9 October 2026). **Not collected yet**; earlier
  300-baht plans were postponed several times.
- **Destination Thailand Visa (DTV)** for remote workers and "soft power" activities (Muay Thai, cooking, medical
  care): 5-year multiple entry, up to 180 days per entry, extendable once by 180 days in Thailand, fee about 10,000
  baht (varies by embassy), proof of 500,000 baht. Since 31 August 2026 applicants also need a recent criminal record
  certificate and proof of residence where they apply (MFA notice, August 2026). Online at thaievisa.go.th. No work
  for Thai clients.
- **Suvarnabhumi (BKK)** สุวรรณภูมิ, 30 km east, main international airport. **Airport Rail Link** to Phaya Thai
  พญาไท (BTS interchange) in about 26–30 min, 15–45 baht, about 05:30–24:00 every 10–15 min (contactless bank cards
  reported at the gates since late 2025 — verify). Taxi from the official queue (level 1): meter + 50-baht airport
  surcharge + tolls, roughly 300–500 baht to Sukhumvit; Grab and Bolt pick up at marked gates.
- **Don Mueang (DMK)** ดอนเมือง, north, low-cost carriers (AirAsia, Nok Air, Thai Lion): SRT **Red Line** from the
  airport footbridge to Krung Thep Aphiwat (Bang Sue) for the MRT Blue Line, or a taxi (30–60 min). Allow 2–3 h
  between the two airports.

## Money
- Baht (THB, ฿). Cash still rules street food, markets, taxis and tuk-tuks; cards (Visa/Mastercard, less Amex) work
  in hotels, malls and chains.
- **PromptPay** พร้อมเพย์ QR codes are everywhere but need a Thai bank account. Visitors from several Asian countries
  scan them with their home bank app; Westerners can use the **TAGTHAi Easy Pay** tourist e-wallet, Alipay+/WeChat
  Pay with a foreign card where shown, or check whether their Wise app now scans Thai QR (announced for 2026).
- **ATMs charge foreign cards a flat fee per withdrawal** (around 220–250 baht, verify) plus your bank's fee:
  withdraw larger sums, decline dynamic currency conversion (pay in baht), or change cash at SuperRich-type
  exchange booths (best rates, passport needed).
- **Tipping**: not expected, appreciated — round up taxis, 20–100 baht for good service; many restaurants and
  hotels already add 10 % service + 7 % VAT ("++"). VAT refund for tourists at the airport (`tax-refund-shopping`).
- A Thai bank account needs a long-stay visa and usually a work permit or embassy letter.

## Getting online and the apps that matter
- **SIM/eSIM**: AIS or True (passport registration) at Suvarnabhumi arrivals, or a travel eSIM bought before
  departure. No censorship of Western services.
- **Apps**: **Grab** (rides, food, the default instead of street taxis), **Bolt** (cheaper cars and motorbike taxis,
  helmets required), **LINE** (the national messenger — landlords, clinics, colleagues and shops expect LINE more than
  WhatsApp), **LINE MAN** and **GrabFood** (delivery), **Google Maps** (good, incl. transit), **ViaBus** (buses),
  **Agoda** (hotels), **Wongnai** (restaurant reviews, Thai), Google Translate camera mode for menus.
- English is common in hotels, malls, hospitals and the BTS; limited with taxi and motorbike drivers — show the Thai.

## Getting around
- **BTS Skytrain** รถไฟฟ้าบีทีเอส: Sukhumvit and Silom lines, about 06:00–24:00, 17–47 baht (2026). Pay with a
  **Rabbit card** (passport to register) or single tickets (cash/QR); BTS gates did not take bank cards in 2026.
- **MRT**: **Blue Line** (loop via Silom, Sukhumvit, Chinatown, Thonburi; 17–44 baht from July 2026), **Purple Line**
  (Nonthaburi), **Yellow Line** monorail (Lat Phrao–Samrong, 2023), **Pink Line** monorail (Khae Rai–Min Buri, January
  2024; IMPACT Muang Thong Thani branch June 2025). Blue and Purple take contactless Visa/Mastercard at the gates.
  **Orange Line** (east): trial runs expected December 2027 (MRTA via Khaosod, October 2026). The **Gold Line**
  shuttle serves Iconsiam.
- **Fares are not integrated yet**: you pay again at each change between BTS, MRT and ARL. A **common ticket** capped
  at 17–45 baht per journey was approved by cabinet on 23 June 2026, target start **1 January 2027** (Bangkok Post).
  The earlier "20-baht flat fare" was for Thai ID holders only and has ended.
- **Boats**: **Chao Phraya Express** (orange flag, flat fare) and the blue-flag tourist boat from **Sathorn pier**
  (BTS Saphan Taksin สะพานตากสิน) to Tha Chang (Grand Palace), Tha Tien (Wat Pho, ferry to Wat Arun) — the best way to
  the old town. **Khlong Saen Saep** canal boats beat the traffic between Pratunam and the east.
- **Taxis** must use the meter (start 35 baht); refuse "fixed price" offers or book Grab/Bolt. **Tuk-tuks**: agree
  the price first and never take a cheap "tour" — the classic scam is "the Grand Palace is closed today", then a gem
  or tailor shop. Traffic at 07:00–09:30 and 16:30–19:30 (worse in rain) can triple journey times; take the train.
  Pedestrians have no right of way and motorbikes use the pavements — look both ways.
- **Intercity**: SRT trains from **Krung Thep Aphiwat** กรุงเทพอภิวัฒน์ (the new hub, 2023) to Chiang Mai, Ayutthaya and
  the south; buses from Mo Chit (north), Ekkamai (east coast) and Sai Tai Mai (south); cheap domestic flights.

## Business scene and customs
- **Why it matters.** ASEAN's second-largest economy and a hub for cars and EVs (the Eastern Seaboard, with Japanese
  and now Chinese makers such as BYD), electronics, food processing, tourism, medical tourism and data centres.
  Many regional HQs choose Bangkok for cost and lifestyle; Singapore stays the finance and legal hub.
- **Investment**: the **BOI** (Board of Investment) gives promoted companies tax holidays, land ownership and easier
  work permits; the **Eastern Economic Corridor** (Chonburi, Rayong, Chachoengsao) adds more. Otherwise the Foreign
  Business Act limits foreign ownership in many services (Americans have the Treaty of Amity).
- **Long-Term Resident (LTR) visa** (ltr.boi.go.th): 10 years (5 + 5) for wealthy global citizens, wealthy
  pensioners, work-from-Thailand professionals and highly skilled professionals (the latter taxed at a flat 17 %);
  typical income bar US$80,000 a year (US$40,000 with a master's degree in some categories) — check the BOI page.
  Thailand Privilege (ex-Thailand Elite) sells 5–20-year visas. Any paid work needs a **work permit** (SMART Visa
  for tech talent).
- **Customs**: the **wai** ไหว้ (palms together, small bow) is returned to peers and elders, not to waiters or
  children; a handshake is fine for foreigners. **Face** and **kreng jai** เกรงใจ (not imposing) mean disagreement is
  indirect and a "yes" may mean "I heard you"; never criticise anyone in public. Address people as **Khun** คุณ +
  first name; the most senior person decides. Stay calm (**jai yen** ใจเย็น, "cool heart"): losing your temper loses
  the room. Relationships, meals and LINE groups move deals; contracts in Thai and English. Never touch a head or
  point your feet at people or Buddha images; women do not touch monks.

## Living here
- **Where expats live**: Sukhumvit (Phrom Phong, Thong Lo, Ekkamai for families near schools and Samitivej;
  Asok/Nana for convenience), **Sathorn/Silom** (near offices and Lumphini Park), **Ari** (leafy, more Thai, cheaper),
  **Riverside** (views), and Nichada Thani gated community (Nonthaburi) near ISB. Live near a BTS/MRT station.
- **Costs** (approximate, 2026): a decent one-bedroom condo in Thong Lo/Phrom Phong 30,000–45,000 baht a month, Ari
  or On Nut roughly 15,000–25,000; condos come with pool and gym; two months' deposit + one in advance; air-con makes
  electricity a real bill. Street meal 60–100 baht; imported wine and cheese are heavily taxed. Foreigners may own
  condo units (49 % foreign quota per building) but not land.
- **Health**: private hospitals are world-class: **Bumrungrad International** (Sukhumvit Soi 3), **Samitivej
  Sukhumvit**, **Bangkok Hospital** (New Phetchaburi Rd), **BNH** (Silom). Far cheaper than the US but rising — get
  international insurance. Ambulance **1669**, tourist police **1155** (English), police 191. Dengue year-round.
- **Schools**: International School Bangkok (ISB, American, Nichada Thani), NIST (IB, Sukhumvit 15), Bangkok Patana
  (British, Bang Na), Harrow Bangkok, Shrewsbury (riverside); top schools cost roughly 700,000–1,000,000+ baht a year
  (verify). Apply early.
- **Laws that catch foreigners**:
  - **Lèse-majesté** (Criminal Code s.112): 3–15 years per count for insulting the king, queen, heir or regent,
    including online posts and shares. Don't joke about the monarchy; stand for the royal anthem in cinemas.
  - **Drugs**: very harsh penalties. **Cannabis** flower became a controlled herb again on **23 June 2025**: legal only
    with a prescription from a licensed Thai practitioner (up to 30 days' supply); walk-in shops selling for smoking
    operate outside the law; never carry it across a border.
  - **Vapes and e-cigarettes are banned** — import, sale and possession; tourists have been fined and detained
    (2025–26). Leave them at home.
  - **Alcohol** is sold 11:00–24:00 (the old 14:00–17:00 break was suspended in December 2025 and reportedly scrapped
    from 29 May 2026 — verify), not on major Buddhist holidays or election days.
  - Smoking is banned in airports, on beaches and in most public places. Carry your passport or a copy. Overstay is
    fined 500 baht a day and can lead to a ban.
- **Daily life**: very safe for violent crime; watch for bag snatching, card skimming, padded bar bills and tourist
  scams. Cover shoulders and knees and remove shoes in temples. A 7-Eleven on every corner; malls are the
  air-conditioned social life.

## Food
- **Street food** is the city's pride: pad kra pao ผัดกะเพรา, khao man gai ข้าวมันไก่, boat noodles ก๋วยเตี๋ยวเรือ
  (Victory Monument), som tam ส้มตำ, moo ping, mango sticky rice; Yaowarat at night, Or Tor Kor อ.ต.ก. market. The
  **Michelin Guide** (Bangkok since 2018) lists dozens of **Bib Gourmand** stalls and shophouses; Jay Fai's crab
  omelette has a star and a long queue; fine dining (Sorn, Gaggan, Le Normandie) is world-class.
- Spice: **mai phet** ไม่เผ็ด (not spicy), **phet nit noi** เผ็ดนิดหน่อย (a little). Fish sauce, shrimp paste and
  peanuts are everywhere — use a written allergy card (`dining-dietary-translate`). Vegetarian: the yellow **jay** เจ
  flag; halal around Sukhumvit Soi 3/1 ("Soi Arab") and Bang Rak.

## What to see (a weekend plus day trips)
- **Day 1, old city by boat**: Grand Palace and Wat Phra Kaew พระบรมมหาราชวัง (strict dress code; mornings are
  coolest), Wat Pho วัดโพธิ์ (reclining Buddha, massage school), ferry to Wat Arun วัดอรุณ at sunset, dinner in Yaowarat.
- **Day 2, modern Bangkok**: Jim Thompson House, Lumphini Park, Chatuchak Weekend Market จตุจักร (Sat–Sun), Bangkok
  Art and Culture Centre, Mahanakhon SkyWalk or a rooftop bar, Muay Thai at Rajadamnern or Lumpinee stadium; cycle
  Bang Krachao บางกระเจ้า, the "green lung".
- **Day trips**: **Ayutthaya** อยุธยา (ruined former capital, UNESCO; 1–2 h by train or minivan); **floating markets**
  — Damnoen Saduak (touristy, go early) or Amphawa อัมพวา (weekend evenings) with the Maeklong railway market;
  **Kanchanaburi** กาญจนบุรี (Bridge on the River Kwai, Death Railway, war cemeteries, Erawan Falls; 2–3 h); **Khao Yai**
  national park; beaches at Hua Hin and Pattaya (2–3 h); flights to Chiang Mai, Phuket, Krabi, Koh Samui.

## Timeline
| Year | Event |
|---|---|
| 1782 | King Rama I founds Bangkok (Rattanakosin) |
| 1932 | End of absolute monarchy |
| 1999 | BTS Skytrain opens |
| 2004 | First MRT (Blue Line) opens |
| 2006 | Suvarnabhumi Airport opens |
| 2010 | Airport Rail Link opens |
| 2011 | Great flood |
| 2016 | King Rama IX dies; Rama X accedes |
| 2023 | Krung Thep Aphiwat terminal and Yellow Line open |
| 2024 | Pink Line opens; 60-day visa exemption (July) |
| 2025 | TDAC mandatory (May); cannabis re-regulated (June) |
| 2026 | 30-day visa exemption from 15 September; common ticket approved for 2027 |

## Sources to verify against
- Entry: Ministry of Foreign Affairs mfa.go.th and thaievisa.go.th; Immigration Bureau immigration.go.th; TDAC
  tdac.immigration.go.th; TAT Newsroom tatnews.org; LTR ltr.boi.go.th; BOI boi.go.th.
- Transport: BTS bts.co.th; MRT metro.bemplc.co.th, MRTA mrta.co.th; Airport Rail Link srtet.co.th; SRT railway.co.th;
  airports airportthai.co.th.
- Statistics: Department of Provincial Administration; National Statistical Office nso.go.th; BMA bangkok.go.th.
- News: Bangkok Post, Khaosod English, Thai PBS World, The Nation. Air quality: air4thai.pcd.go.th.

## Remember what you learn (for next time)
Where the user stays or lives, their office or school area, dietary needs and the apps they have set up are worth
keeping: profile_suggest for profile fields (`custom:home city`, `custom:<city> address`, `custom:allergies`),
memory_remember for preferences ("prefers the metro to taxis in <City>"). Never store passport, card or ID numbers
outside the Vault.
For Bangkok, also their nearest BTS/MRT station, their visa type and entry date (the 30-day clock), and whether they
have a Rabbit card or a Thai bank account for PromptPay.
