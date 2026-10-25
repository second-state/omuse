---
name: arrival-cards
description: 各国电子入境卡/行前授权总览与代填：新加坡 SGAC、泰国 TDAC、马来西亚 MDAC、韩国 e-Arrival、日本 Visit Japan Web、中国在线入境卡、台湾入境卡、印尼 All Indonesia、菲律宾 eTravel、柬埔寨 e-Arrival、英国 ETA、欧盟 ETIAS/EES；按行程判断需要哪些、什么时候填，并按每个国家的技能代填 Which electronic arrival cards and travel authorisations a trip needs (Singapore SGAC, Thailand TDAC, Malaysia MDAC, Korea e-Arrival, Japan Visit Japan Web, China online arrival card, Taiwan arrival card, Indonesia All Indonesia, Philippines eTravel, Cambodia e-Arrival, UK ETA, EU ETIAS / EES), when each opens, and the per-country skill that fills it.
---
# Arrival cards and travel authorisations / 电子入境卡

One skill per system fills the form (`sg-arrival-card`, `th-arrival-card`, `my-arrival-card`, `kr-arrival-card`,
`jp-visit-japan-web`, `cn-arrival-card`, `tw-arrival-card`, `id-arrival-card`, `ph-etravel`, `kh-arrival-card`,
`uk-eta`, `eu-etias`). This one decides which apply and when, from the trip (mail / calendar via `trip-today`, or the
request) and the traveller's passport nationality (profile_get). Facts checked October 2026; verify the official page
(browser_read, date in the answer) before stating a window or a fee. **Every official card is free except the UK ETA
and ETIAS; any other site that charges is a scam.**

| Country | System | Window | Who | Official site |
|---|---|---|---|---|
| Singapore | SG Arrival Card + health declaration | 3 days before arrival | everyone, citizens too | eservices.ica.gov.sg/sgarrivalcard |
| Thailand | TDAC | 3 days before arrival | all foreigners; airside transit exempt | tdac.immigration.go.th |
| Malaysia | MDAC | 3 days before arrival | foreigners; some nationals and airside transit exempt | imigresen-online.imi.gov.my/mdac |
| South Korea | e-Arrival Card (K-ETA separately) | 3 days before arrival | foreigners without K-ETA / residence; children filed by a parent | e-arrivalcard.go.kr · k-eta.go.kr |
| Japan | Visit Japan Web (optional, one QR for immigration + customs) | any time before landing (≥ 6 h before departure advised) | all arrivals | vjw-lp.digital.go.jp |
| Mainland China | Online arrival card (NIA) | before arrival or at the port | all foreigners incl. visa-free; some exemptions | s.nia.gov.cn/ArrivalCardFillingPC · NIA 12367 app |
| Taiwan | Taiwan Arrival Card (TWAC) | 3 days before arrival | visa-exempt and visa visitors; residents exempt | niaspeedy.immigration.gov.tw/webacard |
| Indonesia | All Indonesia arrival card (+ e-VOA, Bali levy) | 3 days before arrival | foreigners at the listed airports / ports | allindonesia.imigrasi.go.id |
| Philippines | eTravel | 72 h before arrival (and departure) | arriving passengers; departing citizens | etravel.gov.ph |
| Cambodia | e-Arrival Card | 7 days before arrival | air arrivals | arrival.gov.kh |
| United Kingdom | ETA (paid authorisation, 2 years) | apply days ahead | visa-exempt visitors incl. EU; not British / Irish | gov.uk/eta |
| EU / Schengen | EES at the border (no form); ETIAS not yet live (announced for 2027) | — | visa-exempt visitors | travel-europe.europa.eu |
| Hong Kong, Macau, Vietnam | no arrival card (Vietnam: e-visa) | — | — | — |

## How to run it
1. List the trip's entries in order (country, arrival date, port, flight). No mail connected or no booking found:
   check the calendar and memory (memory_search "trip"), then ask once for the destinations, dates and flights (or an
   uploaded itinerary, read with file_look) — the per-country skills ask for the hotel themselves. Each entry (including a return to the
   home country, e.g. Singapore) gets its row: needed? window opens on <date>, closes at arrival.
2. Too early for everything → set one reminder per card (schedule_create on the day the window opens, goal "fill <card>
   for <flight> with the <skill> skill") and say so. Inside a window → call that country's skill now.
3. Authorisations with lead time (UK ETA, K-ETA for passports not on the waiver, Vietnam e-visa, China visa, Indonesia
   e-VOA) go first; say the processing time from the official page.
4. One line on what stays with the user: health answers, submitting (every skill stops before Submit for the user's
   OK), CAPTCHAs and logins (takeover), and the passport number (vault).
5. Answer with the table for this trip (country / card / open–close / status / skill), then the reminders created.

## Remember what you learn (for next time)
Nationality, passport expiry (not the number), usual accommodation type and purpose of travel → profile_suggest
(`nationality`, `custom:passport expiry`, `custom:travel purpose`); "always file for the whole family" → memory_remember.
The passport number lives in the Vault, added by the user; a passport photo may be read once with file_look for the
other fields and should not stay in the workspace.
