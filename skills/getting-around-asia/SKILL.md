---
name: getting-around-asia
description: 亚洲本地交通：机场进城、地铁和交通卡、高铁/新干线/KTX/台湾高铁购票（12306 需护照实名）、JR Pass 是否划算、打车软件（Grab、Kakao T、滴滴）、台风和假期的影响；优先用数据连接器和官方时刻表，少用浏览器 Getting around in Asia — airport to city, metros and IC cards, high-speed rail tickets (12306 needs passport registration), whether a rail pass pays off, ride-hailing apps, typhoon and holiday effects; data connectors and official timetables first, little browsing.
---
# Getting around Asia / 当地交通

## 1. Know the trip
From the request, memory (home, mobility needs, luggage) and `trip-today` if bookings matter: cities, dates, times of
arrival, group size, luggage, budget vs speed. Times in local zones; say the zone when two differ.

## 2. Answer from knowledge, verify what changes
General patterns are stable; prices, timetables, strikes, typhoon closures and app availability are not. For a
specific fare, schedule or service status: a connected data tool first (`mcp_<server>__*` rail / transit tools), else
browser_read of the operator's official page (JR East / Central, THSR, KTX / Korail, 12306's English site, MTR, SMRT,
BTS / MRT Bangkok) — 1–3 pages, no clicking around, date checked in the answer.
- **Airport to city** (first ask: bags, time of day, hotel area): Tokyo Narita — Narita Express / Keisei Skyliner /
  limousine bus (Haneda is much closer); Osaka KIX — Haruka / Nankai Rapi:t; Seoul ICN — AREX express / all-stop /
  limousine bus; Singapore Changi — MRT East-West line, taxis with surcharges at night, Grab; Hong Kong — Airport
  Express (in-town check-in), buses; Taipei — Taoyuan Airport MRT; Bangkok Suvarnabhumi — Airport Rail Link, Grab /
  metered taxi (insist on the meter, tollway extra); Shanghai Pudong — Maglev + metro, Didi; Bali — pre-booked car
  or Grab/Gojek from the designated pickup. Night arrivals: check the last train (many stop around midnight).
- **IC cards and tickets**: see `payments-connectivity-asia` for which card where; metro day passes rarely beat the
  IC card unless 4+ rides. Contactless bank cards: Singapore MRT, London-style tap works on parts of HK / Taipei /
  Japanese buses — verify per operator.
- **High-speed rail**: Japan Shinkansen (SmartEX / JR West online, or station machines; reserved seats and the
  oversized-luggage seat for bags over 160 cm total); the JR Pass is only worth it for long multi-city legs after
  the 2023 price rise — compute with calculate against point-to-point fares; Korea KTX (Korail app / Let's Korail);
  Taiwan THSR (early-bird discounts, app); mainland China (12306 app / site needs passport registration; e-tickets, ID
  check at gates; Trip.com as a fallback with a fee); Vietnam / Thailand sleeper trains book out early.
- **Ride-hailing**: Grab (SE Asia), Kakao T (Korea), Didi (China; English version inside Alipay / WeChat mini apps),
  GO / Uber (Japan, taxis via the app), Gojek (Indonesia), Bolt in some cities; airport pick-up points are fixed.
  Taxis: Tokyo and Singapore are metered and honest; Bangkok and Bali — meter or agreed price, avoid touts.
- **Buses, ferries, driving**: left-hand traffic in Japan, HK, Singapore, Malaysia, Thailand, Indonesia; an
  International Driving Permit (1949 convention) for Japan, Korea, Taiwan, Thailand — China does not accept IDPs.
- **Disruptions**: typhoon season (Jul–Oct for Japan / Taiwan / HK / Philippines) closes rail and ferries with a day's
  notice; Golden Week (Japan, late Apr–early May; China, 1–7 Oct), Chuseok, Lunar New Year sell out trains and
  double hotel prices; Japanese holidays move trains to holiday timetables.

## 3. Plan the legs
Give door-to-door options per leg (mode / time / cost with currency / how to buy / notes), recommend one, and when the
user agrees add it to the itinerary (calendar_create_event with the departure station and platform notes) or set a
reminder for booking opening dates (Shinkansen 1 month ahead, THSR 29 days, 12306 15 days — verify).

## 4. Answer
Per leg a short table, then the recommendation and the "buy it here" link; dates checked for anything live; what is
not verified. No invented prices or times.

## Remember what you learn (for next time)
Whatever the user tells you here is worth keeping, so they are never asked twice — but each kind has its place:
- **Profile fields** (name as in the passport, birthday, nationality, phone, emails, addresses, company, loyalty
  programme names, dietary needs, home currency, employer's expense policy, insurer's name and assistance line):
  profile_suggest(field, value) — standard fields or `custom:<label>` (e.g. `custom:passport expiry`,
  `custom:allergies`, `custom:travel insurer`); the user confirms it on the Memory page.
- **Preferences and facts** ("prefers aisle seats", "always files SGAC for the family", "company reimburses per diem
  at …"): memory_remember with its domain.
- **Secrets** — passport, ID, membership and card numbers, and photos of them: never into memory, files or the
  answer. A passport photo the user uploads may be read once with file_look to fill the form or the profile fields
  (name, birthday, nationality, expiry); then ask the user to save the number in the Vault (Memory → Profile → Vault),
  which browser_fill_secret uses from then on, and say the photo itself should not stay in the workspace.
- Uploaded documents that will be needed again (insurance policy PDF, expense policy, company template): keep them
  where they were uploaded and memory_remember the path (domain=account / work).
