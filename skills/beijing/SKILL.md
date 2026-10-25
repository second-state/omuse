---
name: beijing
description: 北京指南（面向西方访客、商务旅行者和外籍居民）Beijing for Westerners — visas and the two airports, paying and getting online, the metro and taxis, the government-and-tech business scene and customs, where to live, the Forbidden City and the Great Wall; use for any question about visiting, doing business in or moving to Beijing.
browser: off
---
# Beijing for Westerners / 北京指南

Beijing (北京, Běijīng) is China's capital: the seat of the Party, the government, the ministries, the regulators and
the embassies, the country's biggest university and AI cluster, and a 3,000-year-old city laid out around the
Forbidden City. It is more formal, more political and colder than Shanghai or Shenzhen, and English is limited outside
the embassy district and the big hotels. This skill is a briefing for visitors, business travellers and people
relocating from Europe, North America or Australia. Figures were checked in October 2026 and carry their date.

## How to use this skill
- Answer from this sheet; keep answers practical ("do X, then Y", with the Chinese name so the user can show it to a
  taxi driver: pinyin + characters, e.g. Sanlitun 三里屯).
- **No browser.** Answer from this sheet: it is the curated data, and each volatile fact carries the date it was checked. For live numbers use data tools, never the browser: exchange rates → market_data; flights, hotels and prices → the travel data connector (mcp_* tools) when it is connected; the user's own bookings → mail and calendar via `trip-today`. When a fact here may have changed since its date (visa rules, fares, opening hours, new lines), say the date and name the official source from the list at the end; for a rule that decides whether the user can travel or enter, hand off to `entry-requirements`. A number not on this sheet and not from a data tool is "not in my data", not a guess.
- Compare to what the user knows: Washington DC (a capital of ministries and lobbyists), London, Paris. Say when
  something that is normal in the West does not work here (Google, credit cards, English, walk-up museum tickets).
- Hand off to the specialised skills when the question is about a task: arrival card → `cn-arrival-card` (overview `arrival-cards`), visas and entry rules → `entry-requirements`, paying and SIMs → `payments-connectivity-asia`, transit tickets and rail passes → `getting-around-asia`, tax refunds → `tax-refund-shopping`, allergies and menus → `dining-dietary-translate`, emergencies → `travel-health-emergency`, today's bookings → `trip-today`, restaurants → `restaurant-booking` / `michelin-dining`.

## Orientation
- **Scale.** About 21.80 million permanent residents at end-2025 (official figures, spring 2026; slightly down as
  the city caps its population); GDP ¥5.21 trillion in 2025 (+5.4 %, first time above ¥5 trillion), second on the
  mainland after Shanghai; 16,410 km² in 16 districts, mostly mountains and suburbs.
- **Story.** Capital of Kublai Khan's Yuan dynasty (Dadu, Marco Polo's "Khanbaliq"); the Ming emperor Yongle moved
  the court here and built the Forbidden City (completed 1420); the Qing ruled from here until 1912. Mao proclaimed
  the People's Republic from Tiananmen on 1 October 1949. The only city to host both Summer (2008) and Winter (2022)
  Olympics.
- **Layout.** Concentric **ring roads** (the 2nd Ring 二环 traces the old city wall, out to the 6th) around a
  north–south **central axis** through the Forbidden City. Addresses are often given by ring road or nearest
  bridge (桥, qiáo), e.g. Guomao 国贸桥.
- **Districts** (where visitors go):
  - *Dongcheng* 东城 and *Xicheng* 西城: the old city inside the 2nd Ring — Forbidden City 故宫, Tiananmen 天安门,
    hutong 胡同 alleys, Wangfujing 王府井 shopping, Houhai 后海 lakes; Financial Street 金融街 (central bank,
    regulators, state banks) is in Xicheng.
  - *Chaoyang* 朝阳: the CBD at Guomao 国贸 (China World), Sanlitun 三里屯 (bars, shopping, the embassy district),
    Liangmaqiao 亮马桥 (embassies, Kempinski/Lufthansa Center), Wangjing 望京 (Korean community, tech offices),
    798 Art District 798艺术区.
  - *Haidian* 海淀: Zhongguancun 中关村 ("China's Silicon Valley"), Peking University 北京大学 and Tsinghua 清华大学,
    the Summer Palace 颐和园, ByteDance, Baidu and the AI start-ups.
  - *Shunyi* 顺义: the Capital airport and the expat villa compounds and international schools around it.
  - *Tongzhou* 通州 (city government, Universal Beijing Resort); *Fengtai* 丰台 (Lize 丽泽 business district,
    Beijing South and Fengtai stations); *Daxing* 大兴 (Daxing airport); *Yizhuang* 亦庄 (Beijing
    Economic-Technological Development Area: JD.com, chip and robotics plants).
- **Climate.** Continental and dry: hot, humid summers (30–38 °C) with heavy **July–August rainstorms**; cold, dry,
  sunny winters (−10 to +3 °C) with central heating from 15 November to 15 March; **spring sandstorms**
  (March–May); autumn (September–October) is the best season. **Air quality** has improved enormously: PM2.5
  averaged 27 µg/m³ in 2025, the lowest on record (89.5 in 2013); winter smog days still happen — check an AQI app
  and wear an N95 above about 150.

## Entering China and getting in from the airports
- **Visas (changes often; checked October 2026 — confirm with `entry-requirements` or nia.gov.cn).** China's
  unilateral **30-day visa-free entry** for ordinary passports (tourism, business, visiting family, transit), extended
  to 31 December 2026, covers nearly all EU states, Switzerland, Norway, Australia, New Zealand and others; **the UK
  and Canada** were added from 17 February 2026. **Czech and Lithuanian** passports are not on the unilateral list.
  **US citizens are not on it either** and use the **240-hour (10-day) visa-free transit**: 57 nationalities
  (including the US, UK, Canada, Australia and EU states, list of 20 August 2026), 65 designated ports in 24
  provincial-level regions, a confirmed onward ticket to a **third** country or region (Hong Kong and Macau count;
  a round trip to the country you came from does not), and travel allowed across the 24 regions. Beijing Capital and
  Daxing airports are designated ports; check the port list before arriving by train. The clock runs from 00:00 the
  day after arrival (verify). Otherwise apply for an L (tourist) or M (business) visa before travelling; there is
  no visa on arrival. Work, study and journalism always need a visa.
- **Arrival card.** Since 20 November 2025 foreigners can file the arrival card online before travel (free, at
  s.nia.gov.cn, the NIA 12367 app, or the WeChat/Alipay mini-program), or fill a paper card or kiosk at the airport.
  Beware paid look-alike sites. See `cn-arrival-card`.
- **Registration.** Hotels register you automatically; in a private home, register at the police station 派出所
  within 24 hours. Carry your passport: it is your ID for trains, museums and checks around Tiananmen.
- **Beijing Capital International (PEK)** 北京首都国际机场, northeast, about 25 km from the centre; Terminals 2 and 3
  (T3 has most international flights, Air China and Star Alliance). **Capital Airport Express** 首都机场线 to Sanyuanqiao,
  Dongzhimen 东直门 and Beixinqiao 北新桥 (connect to Lines 2/5/10/13), about 20–30 min, flat fare ¥25, separate
  ticket (verify current fare). Taxi to the CBD or Sanlitun about 40–60 min, roughly ¥100–150 plus tolls; official rank
  only, ignore touts.
- **Beijing Daxing International (PKX)** 北京大兴国际机场 (opened 2019, Zaha Hadid's starfish),
  46 km south; China Eastern, China Southern and SkyTeam partners use it. **Daxing Airport Express** 大兴机场线 to
  Caoqiao 草桥 (Line 10/19) in about 20 min, ¥35 standard / ¥50 business (fares since opening; verify). The 3.5 km
  extension to Lize 丽泽 and the through-running of the Beijing–Xiong'an express were still not open in October 2026.
  Taxi to the CBD about 60–80 min, roughly ¥200+.
- **Which airport?** Check the ticket: they are 80 km apart, 1.5–2 h by road.

## Money
- Currency is the renminbi (RMB, ¥, CNY). No tipping anywhere, including taxis and restaurants.
- **Mobile payment is universal**: Alipay 支付宝 and WeChat Pay 微信支付 accept foreign Visa, Mastercard, Amex, JCB
  and Discover linked in the app; no Chinese bank account needed. Set both up **before arrival** (passport and a
  phone number that receives SMS). Typical terms (October 2026, verify in the app): no fee on payments up to ¥200, 3 %
  above; limits for verified foreign users of about **US$5,000 per payment and US$50,000 a year**; no sending money
  to individuals or red packets.
- Scan-to-pay works for taxis, metro, hutong stalls, parks and temples. Foreign cards work at international hotels,
  big malls (SKP, China World) and some restaurants; Beijing has pushed hospitals and attractions to accept them.
  Cash is legal tender and must be accepted, but keep small notes. Bank of China and ICBC ATMs take foreign cards.
- Expense receipts are **fapiao** 发票: ask at payment.
- **Tax refund**: departure refunds for foreigners at "Tax Free" shops (SKP, Wangfujing, Sanlitun); upgraded rules
  from July 2026 drop paper forms and allow 28 days to leave from any port; some stores refund on the spot. See
  `tax-refund-shopping`.

## Getting online and the apps that matter
- **The Great Firewall** blocks Google (search, Gmail, Maps, Drive), WhatsApp, Signal, Facebook, Instagram, X, YouTube,
  Wikipedia, many Western news sites and most Western AI services. Install and test a **VPN before arrival**; use by
  individuals is tolerated, but Beijing filtering tightens around political dates (the March "Two Sessions",
  4 June, 1 October). **Roaming or a travel eSIM** (Airalo, Holafly, a Hong Kong SIM) routes through the home
  network and is *not* filtered — easiest for a short trip. A local SIM (China Mobile, Unicom, Telecom;
  passport required) is filtered.
- **Must-have apps**: WeChat 微信 and Alipay 支付宝 (payments, mini-programs, DiDi, metro QR), Amap 高德地图
  (English mode; Google Maps does not work), DiDi 滴滴 (no Uber), Meituan 美团 and Dianping 大众点评, Trip.com and
  12306 for trains, Yitongxing 亿通行 (Beijing metro QR).
- Hotels: only hotels licensed for foreigners can check you in; filter for "accepts foreigners" on Trip.com.

## Getting around
- **Metro** 地铁: 909 km after three openings on 27 December 2025, the longest network in China by the Ministry of
  Transport's count (August 2026); Line 22 to Pinggu was in trial running, due around end-2026. Roughly 05:00–23:00,
  ¥3–10 by distance (airport lines extra); pay with the Alipay/WeChat/Yitongxing QR or a Yikatong 一卡通 card.
  **Security screening** at every station. Line 2 loops the old city wall, Line 1 runs under Chang'an Avenue, Line 10
  circles the 3rd Ring. Rush hours 07:30–09:30 and 17:30–19:30 are brutal.
- **Taxis** start at ¥13 for 3 km (since 2013), about ¥2.3/km after, extra at night; drivers rarely speak English —
  show the address in Chinese. DiDi is easier. Traffic is heavy and distances are huge: allow an hour across town.
- **Rail stations**: Beijing South 北京南 (HSR to Shanghai, about 4.5 h on the fastest Fuxing trains; Tianjin 30 min),
  Beijing West 北京西 (south and west, Hong Kong West Kowloon sleeper and HSR), Beijing Chaoyang 北京朝阳 (northeast),
  Beijing North/Qinghe 北京北/清河 (Badaling Great Wall, Zhangjiakou), Beijing Fengtai 丰台. Book with passport on
  12306/Trip.com; the passport is the ticket.
- Shared bikes (Meituan, HelloBike, via Alipay) suit the flat, wide streets; ride in the bike lane, watch e-bikes.

## The business scene and customs
- **Why it matters.** Beijing is where policy, regulation and state capital sit: the ministries, the NDRC, the central
  bank and financial regulators, most central state-owned enterprises (Sinopec, China Mobile, State Grid, the big four
  banks), the embassies and chambers (AmCham China, EU Chamber, British Chamber are all headquartered here). For
  licences, government relations, SOE partners and lobbying, you come to Beijing; for manufacturing you go south.
- **Tech and AI**: Zhongguancun 中关村 and Haidian host Baidu, ByteDance, Kuaishou, Lenovo, Xiaomi (Xiaomi's EV plant
  is in Yizhuang), Meituan, JD.com, DiDi, and the large-model start-ups Zhipu and Moonshot (Kimi), fed by
  Tsinghua and Peking University. The Beijing Stock Exchange opened in 2021 for small innovative firms.
- **Events**: "Two Sessions" (early March; tight security), Auto China (April, even years), World Robot Conference
  (August), CIFTIS services fair (September).
- **Business customs**: exchange **WeChat** first, business cards second (both hands, read it); hierarchy and titles
  matter more than in Shanghai — surname plus title (王总 Wáng zǒng for a boss); seating and speaking
  order follow rank; a direct "no" is rare ("we'll study it" usually means no); with officials
  and SOEs expect formal meetings, prepared statements and decisions taken later; banquets with **baijiu** 白酒
  and many toasts seal relationships — clink below a senior person's glass,
  and say early if you will drink tea; punctuality is expected; contracts bilingual with the Chinese
  text prevailing. Avoid politics (Taiwan, Tibet, Xinjiang, 1989) in conversation.
- Work permits: Z visa plus work permit (degree plus two years' experience as a rule); the employer handles it.

## Living here
- **Where expats live**: *Shunyi* 顺义 (villas and compounds near the international schools — family life, car needed,
  45–60 min to the CBD); *Chaoyang* — Sanlitun 三里屯, Liangmaqiao 亮马桥 and the CBD/Guomao 国贸 (serviced
  apartments, walkable, embassies, nightlife), Wangjing 望京 (cheaper, Korean and tech); *Dongcheng* hutong
  courtyards for character (cold in winter, shared toilets in some); Wudaokou 五道口 (Haidian) for students.
- **Costs** (approximate, 2026): a one-bedroom in Sanlitun/CBD about ¥7,000–11,000 a month, serviced apartments and
  Shunyi villas far more, ¥4,000–6,000 further out; local meal ¥30–60, Western ¥150–300; metro ¥200–400 a month.
  Landlords want one month's deposit and usually three months' rent in advance; agents (Lianjia/Beike 贝壳, Ziroom 自如)
  charge up to a month's rent. Check heating and air purifiers.
- **Health**: Beijing United Family Hospital 北京和睦家医院 (Jiangtai Road, Chaoyang; English-speaking, 24-hour
  emergency, expensive) and its clinics; Peking Union Medical College Hospital International Medical Services
  北京协和医院国际医疗部 (Dongcheng; top public hospital, English-speaking department); Raffles Medical and other
  private clinics in Chaoyang. As of late 2025, 28 hospitals offered full English booking through the city's
  "Jingtong" 京通 mini-program. Private care costs Western prices: have insurance. Emergency 120 is Mandarin only —
  call your hospital's hotline. Bring prescriptions; antibiotics need one.
- **Schools** (fees roughly ¥200,000–360,000 a year, apply early; check each school): International School of Beijing
  (ISB, Shunyi), Western Academy of Beijing (WAB, Chaoyang), Dulwich College Beijing, The British School of Beijing,
  Beijing City International School (BCIS), Canadian International School of Beijing, Harrow Beijing; Keystone
  Academy for a bilingual track.
- **Daily life**: very safe (violent crime against foreigners is rare), but security is visible — ID checks and bag
  scans at Tiananmen, Chang'an Avenue and metro stations, and police may ask for your passport. Drones are banned over
  most of the city. Smoking is banned indoors.

## Food
- **Peking duck** 北京烤鸭 (Siji Minfu 四季民福, Da Dong 大董, Quanjude 全聚德);
  **zhajiangmian** 炸酱面 (noodles with fermented bean sauce), **jiaozi** 饺子 dumplings, **mutton hotpot** 涮羊肉
  (copper pot, Donglaishun 东来顺) in winter, imperial-court cuisine, Muslim Hui food around Niujie 牛街, jianbing
  煎饼 breakfast crêpes, and the acquired tastes: douzhi 豆汁 (fermented mung-bean drink) and luzhu 卤煮 (offal stew).
  Every regional cuisine is here, including the provincial government office restaurants loved by expats.
- Where: Gui Jie 簋街 (Ghost Street: crayfish, late nights), Sanlitun and Liangmaqiao for
  Western food and cocktails, the hutongs around Gulou 鼓楼 and Wudaokou for craft beer (Great Leap Brewing).
  Vegetarian: 我吃素; halal 清真.

## What to see (a weekend, plus day trips)
- **Book ahead, with your passport**: the Palace Museum (Forbidden City 故宫博物院) sells timed real-name tickets online
  only, released seven days ahead, capped at 40,000 a day, ¥60 April–October / ¥40 November–March, **closed Mondays**
  except holidays (October 2026, verify). Tiananmen Square and the National Museum (both free) also need a
  reservation; weekends and holiday weeks sell out.
- **Day 1**: Tiananmen Square 天安门广场 → Forbidden City (enter south via the Meridian Gate, exit north) → Jingshan
  Park 景山公园 for the view back over the roofs → hutongs of Shichahai 什刹海/Houhai and the Drum and Bell Towers
  鼓楼钟楼 → Peking duck.
- **Day 2**: Temple of Heaven 天坛 at 07:00 (locals doing tai chi and dancing) → Lama Temple 雍和宫 and Confucius
  Temple 孔庙 → 798 Art District 798艺术区 → Sanlitun. Or: Summer Palace 颐和园, Bird's Nest 鸟巢, Universal
  Beijing Resort 北京环球度假区.
- **The Great Wall** 长城: **Mutianyu** 慕田峪 (1.5–2 h by car, restored, cable car and toboggan, the best all-round
  choice), **Badaling** 八达岭 (HSR from Qinghe/Beijing North to Badaling Great Wall station; most crowded; real-name
  booking), **Jinshanling** 金山岭 (2.5 h, half-restored, best for hiking and photos), **Jiankou** 箭扣 (wild, for
  experienced hikers only). Go on a weekday, early.
- **Day trips**: Ming Tombs 十三陵; Tianjin 天津 (30 min HSR, concession-era architecture); Chengde 承德 (Qing summer
  resort, about 1–1.5 h by HSR from Beijing Chaoyang).
- Avoid **Golden Week** (1–7 October) and the Spring Festival rush for sightseeing; and the "tea ceremony"/"art
  student" scam near Wangfujing and Tiananmen (friendly English speakers who lead you to a ¥1,000+ bill).

## Timeline
| Year | Event |
|---|---|
| 1267 | Kublai Khan begins Dadu, capital of the Yuan dynasty |
| 1420 | Ming Forbidden City completed; capital moved from Nanjing (1421) |
| 1949 | People's Republic proclaimed at Tiananmen (1 October) |
| 1989 | Tiananmen protests and crackdown (4 June) |
| 2008 | Summer Olympics; Capital airport T3 and Beijing–Tianjin HSR |
| 2019 | Daxing airport opens; city government moves to Tongzhou |
| 2022 | Winter Olympics |
| 2024 | Central axis inscribed as UNESCO World Heritage |
| 2025 | GDP passes ¥5 trillion; metro reaches 909 km (December) |

## Sources to verify against
- Visas, transit and arrival card: National Immigration Administration en.nia.gov.cn (s.nia.gov.cn for the card);
  Chinese embassy sites; Ministry of Foreign Affairs fmprc.gov.cn.
- City facts: Beijing government english.beijing.gov.cn; Beijing Municipal Bureau of Statistics tjj.beijing.gov.cn
  (2025 bulletin, April 2026); Beijing Ecology and Environment Bureau for air quality.
- Airports: bcia.com.cn (Capital), Daxing airport site; metro: Beijing Subway bjsubway.com and the Yitongxing app.
- Sights: Palace Museum dpm.org.cn; visitbeijing.com.cn; tax refunds: taxfree.mofcom.gov.cn.
- News in English: China Daily, thebeijinger.com, beijing-kids.com (schools).

## Remember what you learn (for next time)
Where the user stays or lives, their office or school area, dietary needs and the apps they have set up are worth
keeping: profile_suggest for profile fields (`custom:home city`, `custom:<city> address`, `custom:allergies`),
memory_remember for preferences ("prefers the metro to taxis in <City>"). Never store passport, card or ID numbers
outside the Vault.
In Beijing, note which airport they use (Capital or Daxing), whether they live in Shunyi or Chaoyang, and their
school shortlist.
