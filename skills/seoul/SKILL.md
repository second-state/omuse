---
name: seoul
description: 首尔指南（面向西方访客、商务旅行者和外籍居民）Seoul for Westerners — visa waiver, K-ETA and the e-Arrival card, Incheon and AREX, paying and getting online (Naver Map, Kakao T), getting around, business customs, where to live (jeonse and wolse), what to see and day trips; use for any question about visiting, doing business in or moving to Seoul.
browser: off
---
# Seoul for Westerners / 서울

Seoul (서울, Seoul) is the capital of South Korea and the core of a capital region that holds about half the
country's 51 million people: fast, safe, wired and open late, with superb transit, yet with its own map apps,
payment habits and a language most signs only partly translate. This skill is a briefing for visitors, business
travellers and people relocating from Europe, North America or Australia. Figures were checked in October 2026 and
carry their date.

## How to use this skill
- Answer from this sheet; keep answers practical ("do X, then Y"), with the Korean name in Hangul plus the
  romanisation so the user can show it to a taxi driver or type it into Naver Map, e.g. Itaewon 이태원.
- **No browser.** Answer from this sheet: it is the curated data, and each volatile fact carries the date it was checked. For live numbers use data tools, never the browser: exchange rates → market_data; flights, hotels and prices → the travel data connector (mcp_* tools) when it is connected; the user's own bookings → mail and calendar via `trip-today`. When a fact here may have changed since its date (visa rules, fares, opening hours, new lines), say the date and name the official source from the list at the end; for a rule that decides whether the user can travel or enter, hand off to `entry-requirements`. A number not on this sheet and not from a data tool is "not in my data", not a guess.
- Compare to what the user knows: Tokyo, London, New York. Say when something normal in the West does not work here
  (Google Maps navigation, booking apps that need a Korean phone number, foreign cards in some apps).
- Hand off to the specialised skills when the question is about a task: arrival card → `kr-arrival-card` (overview `arrival-cards`), visas and entry rules → `entry-requirements`, paying and SIMs → `payments-connectivity-asia`, transit tickets and rail passes → `getting-around-asia`, tax refunds → `tax-refund-shopping`, allergies and menus → `dining-dietary-translate`, emergencies → `travel-health-emergency`, today's bookings → `trip-today`, restaurants → `restaurant-booking` / `michelin-dining`.

## Orientation
- **Scale.** About 9.4 million registered residents in Seoul proper (9.39 million in 2024, slowly falling as people
  move to cheaper Gyeonggi suburbs); the capital region (수도권: Seoul, Incheon, Gyeonggi) has about 26 million. Area
  605 km², 25 districts (구, gu). About 2.87 million foreign nationals were in Korea in August 2026, and just over half
  of registered foreigners live in the capital region (Yonhap, September 2026). UTC+9; 220 V, plugs C/F.
- **Story.** Joseon capital from 1394 (Hanyang 한양); Japanese rule 1910–1945; fought over in the Korean War
  (1950–53); then the "Miracle on the Han River", democracy in 1987, the 1988 Olympics. The DMZ is about 50 km north.
- **Districts and neighbourhoods** (north of the Han River 강북, Gangbuk, is the old city; south 강남, Gangnam, the
  new one):
  - *Jongno* 종로 and *Jung-gu* 중구: palaces, Gwanghwamun 광화문 square, Insadong 인사동, Bukchon 북촌, City Hall,
    Myeongdong 명동 shopping, Namsan 남산 and Seoul Station 서울역.
  - *Yongsan* 용산: Itaewon 이태원 and Hannam-dong 한남동 (expat heartland, embassies, nightlife), National Museum.
  - *Mapo* 마포: Hongdae 홍대 (indie music, clubs), Yeonnam-dong 연남동 and Mangwon 망원 (cafés), Sangam 상암 (media).
  - *Seongdong* 성동: Seongsu-dong 성수동, warehouse cafés and pop-up stores.
  - *Yeouido* 여의도 (Yeongdeungpo-gu): finance, the National Assembly, cherry blossoms and Han River park.
  - *Gangnam* 강남 / *Seocho* 서초: business towers along Teheran-ro 테헤란로, Apgujeong 압구정 and Cheongdam 청담
    (luxury, K-pop agencies, clinics), Seorae Village 서래마을 (French quarter).
  - *Songpa* 송파: Jamsil 잠실, Lotte World Tower (555 m), Olympic Park.
  - Beyond: Pangyo 판교 (Seongnam; Kakao, Nexon, NCSoft; Naver in nearby Jeongja), Songdo 송도 (Incheon, near ICN).
- **Climate.** Four sharp seasons. Winter is cold and dry (often −10 °C, wind chill lower; flats have underfloor
  heating, 온돌 ondol); spring brings cherry blossoms (early April) and **fine dust** 미세먼지 and yellow dust 황사 from
  the west — check the AQI and wear a KF94 mask on bad days; summer is hot and humid (30–35 °C) with the **monsoon**
  장마 (jangma) late June–late July, flash floods (Gangnam flooded in August 2022) and occasional typhoons in
  August–September; autumn (late September–November) is the best season.

## Entering Korea and getting in from the airports
- **Visa waiver (verify at visa.go.kr / k-eta.go.kr).** Ordinary-passport holders of the US, UK, EU states,
  Australia and New Zealand enter visa-free for tourism or meetings, typically up to 90 days (Canadians 6 months).
- **K-ETA.** Visa-waiver travellers normally need a K-ETA (online pre-authorisation, fee about ₩10,000, apply at
  least 72 hours ahead on the official site k-eta.go.kr only — beware look-alike agencies). **Temporary exemption
  through 31 December 2026** (extended in January 2026; checked October 2026) for 22 countries and regions: Australia,
  Austria, Belgium, Canada, Denmark, Finland, France, Germany, Italy, Japan, the Netherlands, New Zealand, Norway,
  Poland, Singapore, Spain, Sweden, the UK, the US (incl. Guam), plus Hong Kong, Macau and Taiwan. Exempt travellers
  may still buy a K-ETA voluntarily, which spares them the arrival card. Whether the exemption continues into 2027 was
  not announced as of this check — for any trip from January 2027, hand off to `entry-requirements`.
- **e-Arrival card** 전자입국신고서 (e-arrivalcard.go.kr, free, from 3 days before arrival): launched 24 February 2025 to
  replace the paper card. Holders of a valid K-ETA and registered residents are exempt. Some guides say paper was
  dropped from 2026, but the Ministry of Justice told VisasNews that both formats are still accepted (checked October
  2026) — file it online anyway. Step-by-step: `kr-arrival-card`.
- **Incheon International Airport (ICN)** 인천국제공항, on Yeongjong Island about 50 km west, has Terminal 1 and
  Terminal 2 (Korean Air and SkyTeam use T2; check your ticket — a free shuttle links them).
  - **AREX** 공항철도 airport railway: the *Express* 직통열차 runs non-stop to Seoul Station in about 43 min from T1 /
    51 min from T2 (about ₩9,500 per press and travel guides in 2026; verify at arex.or.kr — services added in October
    2026); the *All-stop* 일반열차 takes about an hour, costs about ₩5,000 with a transit card and stops at Gimpo,
    Digital Media City and Hongik Univ. (Hongdae).
  - **Limousine buses** 공항리무진 to major hotels and districts in 60–90 min; best with luggage. Taxis about 1 h.
- **Gimpo (GMP)** 김포공항 (west Seoul; Lines 5, 9, AREX): domestic flights and shuttles to Haneda, Osaka, Shanghai,
  Taipei Songshan.

## Money
- Currency: Korean won (₩, KRW). No tipping, ever — not in taxis, restaurants or hotels; prices include VAT.
- **Cards** are accepted almost everywhere, even at street stalls and taxis; foreign Visa/Mastercard usually work, but
  some small shops' terminals and many apps reject them. Keep some cash for markets and to top up T-money.
- **T-money** 티머니 transit card: buy at convenience stores or stations, top up with cash only; also pays in shops
  and most taxis. **WOWPASS** is a prepaid
  card for foreigners with T-money built in (fees vary; verify). The **Climate Card** 기후동행카드 tourist pass gives
  unlimited Seoul subway and bus rides for 1–7 days (₩5,000 for 1 day, ₩20,000 for 7 per 2026 guides; verify).
- **ATMs**: use those marked "Global" (Shinhan, Woori, KB, Hana, convenience-store ATMs); ₩50,000 is the largest note.
- **Tax refunds** (verify): purchases of ₩15,000 or more at "Tax Free" shops; many stores deduct VAT on the spot
  ("immediate refund", per 2026 guides up to ₩1,000,000 per purchase and ₩5,000,000 per trip); otherwise claim at
  kiosks in Myeongdong or at the airport before check-in. See `tax-refund-shopping`.

## Getting online and the apps that matter
- No Great Firewall; every Western service works. Buy a SIM or eSIM at Incheon (KT, SK Telecom, LG U+ counters, or a
  travel eSIM); a local number matters because many Korean apps need SMS verification.
- **Maps**: use **Naver Map** 네이버 지도 (has English) or **KakaoMap** 카카오맵. Google Maps shows places and transit but
  gives **no turn-by-turn walking or driving navigation** — Korea approved Google's map-data export on 27 February
  2026 with conditions, but navigation had not launched as of this check (October 2026). Search Korean names, or paste
  the address in Hangul, for best results.
- **Taxis**: **Kakao T** 카카오 T is the main app but card registration often fails without a Korean number; Kakao's
  foreigner app **k.ride** accepts overseas cards and English sign-up; **Uber** (Uber Taxi) also works in Seoul.
- **KakaoTalk** 카카오톡 is the national messenger and business channel (WhatsApp is little used); **Papago** 파파고
  translates Korean better than Google, with a camera mode for menus.
- **Shopping and food**: **Coupang** 쿠팡 (Amazon-style, overnight "Rocket" delivery; needs a Korean phone and usually
  a Korean card), Baemin 배달의민족 and Coupang Eats for food delivery, Naver for reservations and reviews, CatchTable
  for restaurant bookings. Korail Talk / SRT apps for trains.

## Getting around
- **Subway** 지하철: Seoul Metro lines 1–9 plus AREX, Shinbundang, Gyeongui-Jungang, Suin-Bundang and others form
  one of the world's largest metropolitan networks; signs and announcements in English, numbered stations, clean
  and safe, roughly 05:30–24:00 (no night service; night buses "N" fill the gap). **Fare** ₩1,550 base with a transit
  card (since 28 June 2025; ₩1,650 single ticket), distance-based beyond 10 km; free transfers to buses within 30 min.
  Tap in *and out*, also on buses.
- **GTX-A** express commuter rail: Suseo–Dongtan (March 2024) and Unjeong (Paju)–Seoul Station (December 2024). The
  Seoul Station–Suseo link missed its August 2026 target; Samseong station not before 2028 (press, May 2026).
- **Taxis** 택시: metered, cheap by Western standards, card or T-money; black "deluxe" 모범 cost more; late-night
  surcharge. Few drivers speak English — show the Hangul address or book via k.ride/Uber.
- **Intercity**: KTX/SRT to Busan (about 2.5 h), Gyeongju, Jeonju, Gangneung; flights from Gimpo to Jeju.
- Public bikes **Ttareungyi** 따릉이 (English app) suit the Han River paths; few expats drive in the city.

## Business scene and customs
- **Why it matters**: home of the chaebol — Samsung (Seocho/Suwon), Hyundai Motor, SK, LG (Yeouido Twin Towers),
  Lotte — plus Naver and Kakao, a top-tier semiconductor, battery, shipbuilding, games and content (K-pop, K-drama,
  webtoons) economy. Teheran-ro and Pangyo are the start-up corridors, Yeouido the finance district; Seoul Global
  Center (Jongno) helps foreign business people and residents.
- **Customs**: hierarchy and age matter; address people by title and surname (e.g. "Director Kim", 김 이사님), not first
  name. A slight bow with the handshake; give and receive business cards with both hands and study them before
  putting them away. Meetings are punctual; the senior person speaks and decides, often after the meeting. "That
  may be difficult" usually means no. **Hoesik** 회식, the after-work team dinner, builds relationships: the senior
  pays, pour for others with two hands, turn slightly away when drinking in front of a senior; declining alcohol is
  now acceptable — say so early. KakaoTalk replies are expected fast.
- **Visas for work**: E-7 (skilled professionals), D-8 (investors/company setup), D-10 (job seeking), F-1-D
  "workcation" digital-nomad visa (launched 2024; remote workers for foreign employers with high income; verify the
  current income threshold), E-2 (English teachers). Invest Korea (KOTRA) helps investors.

## Living here
- **Where expats live**: Hannam-dong 한남동 / Itaewon / UN Village (embassies, houses, Western groceries; pricey),
  Seorae Village 서래마을 (French families), Yeonhui-dong 연희동 (near Seoul Foreign School), Gangnam (business,
  officetels), Seongsu and Mapo (younger professionals), Pangyo and Songdo (tech families; newer, further out).
- **How renting works**: **Jeonse** 전세 — a large lump-sum deposit (often well over half the flat's value) and no
  monthly rent, returned at the end of the lease (usually two years); **wolse** 월세 — a smaller deposit (typically
  ₩10–100 million) plus monthly rent; maintenance fees 관리비 come on top. Wolse is now dominant: about 70 % of Seoul
  rental deals in January–April 2026 (MOLIT). Before paying a deposit, check the property register 등기부등본 for
  mortgages, use a licensed agent 공인중개사 and consider HUG deposit-guarantee insurance (jeonse fraud hit 2022–23).
- **Costs** (2026): average Seoul monthly rent ₩1.25 million across housing types (Korea Real Estate Board, April
  2026); small officetels under 30 m² typically take a ₩10–30 million deposit, and one in eight such leases signed
  January–August 2026 cost ₩1 million+ a month (Asia Economy, October 2026); family flats in Gangnam or Hannam cost
  several times that. A local meal is roughly ₩10,000–15,000.
- **Health**: excellent, affordable hospitals with international clinics and English-speaking coordinators: Severance
  세브란스병원 (Sinchon), Seoul National University Hospital 서울대학교병원 (Jongno), Samsung Medical Center 삼성서울병원
  (Irwon-dong), Asan Medical Center 서울아산병원 (Songpa).
  Residents staying six months or more are enrolled in National Health Insurance 국민건강보험 (verify current rules).
  Pharmacies 약국 are everywhere; 119 for ambulance, 1330 for the tourist helpline (English, 24 h).
- **Schools**: Seoul Foreign School (Yeonhui-dong), Yongsan International School of Seoul, Dwight School
  Seoul (Sangam), Dulwich College Seoul (Banpo), Seoul International School and Korea International School (Pangyo
  area), Chadwick International (Songdo), Lycée Français de Séoul, Deutsche Schule Seoul International. Fees run to
  tens of millions of won a year; apply early.
- **Residence**: long-stay visa holders register within 90 days of entry for a Residence Card 외국인등록증 at the local
  immigration office (book on hikorea.go.kr); it unlocks a bank account, a phone contract and most apps.
- **Daily life**: very safe, including at night; theft is rare (people leave laptops in cafés). Hazards are traffic
  (scooters on pavements), summer floods and winter ice. Recycling and food-waste sorting are strict (district
  pay-as-you-throw bags). Korean is needed for bureaucracy; English is better among the young. Gwanghwamun
  demonstrations are frequent and peaceful.

## Food
- Korean BBQ 고기구이 (samgyeopsal 삼겹살 pork belly, galbi 갈비), fried chicken and beer 치맥 (chimaek), stews 찌개
  (kimchi-jjigae, sundubu), bibimbap 비빔밥, naengmyeon 냉면 cold noodles, samgyetang 삼계탕, tteokbokki 떡볶이;
  banchan side dishes are refilled free. Gwangjang Market 광장시장
  (bindaetteok mung-bean pancakes, yukhoe), Euljiro 을지로 "hip-jiro" bars, Mangwon market, Noryangjin fish market.
  Michelin-starred fine dining clusters in Cheongdam and Hannam (book on CatchTable).
- Vegetarians and allergy sufferers struggle: anchovy or meat stock is in most soups and kimchi contains fish sauce
  or salted shrimp. Temple food 사찰음식 is fully vegan. "고기 빼 주세요" = no meat please; carry an allergy card in Korean
  (see `dining-dietary-translate`). BBQ and stews often need two portions minimum.

## What to see (a weekend plus day trips)
- **Palaces**: Gyeongbokgung 경복궁 (main palace, guard-changing ceremony; free in hanbok; closed Tuesdays),
  Changdeokgung 창덕궁 with the Secret Garden 후원 (UNESCO; timed tours; closed Mondays), Deoksugung 덕수궁 and Jongmyo
  Shrine 종묘. Check opening days before planning.
- **Old Seoul**: Bukchon Hanok Village 북촌한옥마을 (people live here; the core lanes have visiting hours, roughly
  10:00–17:00, with fines enforced since 2025 — verify), Insadong crafts, Ikseon-dong 익선동, Cheonggyecheon
  청계천 stream walk, Namsan Seoul Tower N서울타워, Seoul City Wall trail 한양도성.
- **Modern Seoul**: Dongdaemun Design Plaza DDP (Zaha Hadid), Lotte World Tower Seoul Sky, Starfield COEX Library,
  Seongsu cafés and pop-ups, Hongdae buskers, a Han River park 한강공원 at sunset (Yeouido, Banpo rainbow fountain).
- **Museums**: National Museum of Korea 국립중앙박물관 and War Memorial of Korea 전쟁기념관 (both free), Leeum (Hannam), MMCA.
- **Day trips**:
  - **DMZ** 비무장지대: tours (passport required, book ahead) to Imjingak 임진각, the Third Infiltration Tunnel and Dora
    Observatory. **Panmunjom/JSA** 판문점 tours were suspended for long periods in 2023–2026 and resumed only in small,
    limited form at best — check status with the operator before promising it.
  - **Suwon Hwaseong** 수원 화성: UNESCO-listed 18th-century fortress walk, about an hour by subway Line 1 or 30 min by
    train; Suwon is also Samsung's home.
  - **Bukhansan** 북한산 National Park: granite peaks inside the city limits; Baegundae 백운대 (836 m) is a steep half-day
    hike with cables; go early on weekends. Gentler: Inwangsan 인왕산 and Achasan 아차산 for city views.
  - Also: Nami Island 남이섬, Jeonju hanok village and Gangneung beach by KTX, Busan for a weekend.

## Timeline
| Year | Event |
|---|---|
| 1394 | Hanyang becomes the Joseon dynasty capital |
| 1910 | Japanese annexation; city renamed Keijō |
| 1945 | Liberation; renamed Seoul |
| 1950–53 | Korean War; Seoul changes hands four times |
| 1974 | Subway Line 1 opens |
| 1988 | Summer Olympics |
| 2002 | FIFA World Cup co-hosted with Japan |
| 2005 | Cheonggyecheon stream restored |
| 2023 | K-ETA temporary exemption for 22 countries begins (April) |
| 2024 | GTX-A opens (March) |
| 2025 | e-Arrival card launched (24 February); subway base fare ₩1,550 (June) |
| 2026 | Google map-data export approved (27 February); K-ETA exemption extended to 31 December 2026 |

## Sources to verify against
- Entry: K-ETA k-eta.go.kr; e-Arrival card e-arrivalcard.go.kr; Korea Visa Portal visa.go.kr; HiKorea hikorea.go.kr
  (residence); Korea Immigration Service immigration.go.kr.
- Travel: Visit Korea english.visitkorea.or.kr and the 1330 helpline; Visit Seoul english.visitseoul.net; Seoul
  Metropolitan Government english.seoul.go.kr; Seoul Global Center global.seoul.go.kr.
- Transport: AREX arex.or.kr; Seoul Metro seoulmetro.co.kr; Korail letskorail.com; Incheon Airport airport.kr.
- Housing and statistics: Korea Real Estate Board reb.or.kr; MOLIT molit.go.kr; Statistics Korea kostat.go.kr.
- News: Yonhap en.yna.co.kr, Korea Herald, Korea JoongAng Daily.

## Remember what you learn (for next time)
Where the user stays or lives, their office or school area, dietary needs and the apps they have set up are worth
keeping: profile_suggest for profile fields (`custom:home city`, `custom:<city> address`, `custom:allergies`),
memory_remember for preferences ("prefers the metro to taxis in <City>"). Never store passport, card or ID numbers
outside the Vault.
In Seoul, also note whether the user has a Korean phone number, a Residence Card and which of Naver Map, k.ride/Kakao T
and WOWPASS they already use — it decides which apps and payments will work for them.
