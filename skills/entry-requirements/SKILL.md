---
name: entry-requirements
description: 亚洲入境准备：签证/免签、电子入境卡（泰国 TDAC、新加坡 SGAC、韩国 e-Arrival/K-ETA、马来西亚 MDAC、日本 Visit Japan Web、印尼、越南、中国过境免签）、护照有效期、海关与药品规定，按出发日期设提醒 Visas and visa-free rules, digital arrival cards (Thailand TDAC, Singapore SGAC, Korea e-Arrival / K-ETA, Malaysia MDAC, Japan Visit Japan Web, Indonesia, Vietnam, China transit), passport validity, customs and medication rules, with reminders timed to the trip.
---
# Entry requirements / 入境准备

Rules change often and depend on the passport. Facts below were checked in October 2026 and carry that date; before
quoting a rule, browser_read the official page named here and say the date you checked. A fee for an arrival card is
always a scam — only the government sites named here.

## 1. What you need from the user (profile / memory first, then ask once)
Passport nationality (profile_get / memory_search "passport"), destination(s) and dates, transit points, purpose,
whether they have visited the country before on this passport (some ETAs are multi-year).

## 2. Checklist per destination (verify, then tailor)
- **Passport validity**: most of Asia wants 6 months beyond arrival (Singapore, Thailand, Malaysia, Indonesia,
  Vietnam, China); Japan, Korea, Hong Kong, Taiwan accept validity for the stay — check the destination's rule.
  Blank pages: 1–2.
- **Singapore**: SG Arrival Card + health declaration, free, within 3 days before arrival, everyone including
  citizens → `sg-arrival-card` skill fills it. Official: eservices.ica.gov.sg/sgarrivalcard.
- **Thailand**: TDAC (Thailand Digital Arrival Card) replaced the paper TM6; file within 3 days before arrival at
  tdac.immigration.go.th; airside transit is exempt. Visa exemption length depends on nationality — verify.
- **Malaysia**: MDAC (Malaysia Digital Arrival Card), one per traveller, within 3 days; airside transit and
  Singapore citizens exempt (check the current exemption list). imigresen-online.imi.gov.my/mdac.
- **Korea**: K-ETA (electronic travel authorisation) is temporarily waived for a list of countries until
  31 Dec 2026 (Singapore among them, as of Oct 2026); travellers using the waiver file the free Korea e-Arrival Card
  within 3 days (e-arrivalcard.go.kr). Others apply for K-ETA days ahead (k-eta.go.kr).
- **Japan**: Visit Japan Web (vjw-lp.digital.go.jp) is optional but speeds immigration and customs (one QR code);
  JESTA, a pre-departure authorisation, is announced for 2028 — not needed now. Visa-free for many passports; verify.
  Medicines: up to a 1-month personal supply of prescription medicine without paperwork (narcotics, stimulants
  excluded; some ADHD and pseudoephedrine products are banned); more, or injectables, need a Yakkan Shoumei import
  certificate applied for weeks ahead — point to the MHLW page and tell the user to confirm their exact drug.
- **Hong Kong**: no arrival card for most passports; visa-free periods vary. **Taiwan**: online Arrival Card
  (niaspeedy.immigration.gov.tw) for visa-exempt visitors; e-Gate registration possible on arrival.
- **Indonesia** (Bali): e-VOA / visa on arrival for many passports, plus the electronic customs declaration and the
  Bali tourist levy — all on official .go.id sites. **Vietnam**: e-visa (evisa.gov.vn) for most, days ahead.
- **Mainland China**: visa-free transit (up to 240 hours in listed regions) and unilateral visa-free entry for a list
  of countries that changes — verify on the embassy / NIA site; everyone needs an arrival card at the border (paper or
  the online version); Alipay / WeChat Pay set-up belongs to `payments-connectivity-asia`.
- **Customs**: cash declaration limits (e.g. SGD 20,000 in Singapore, JPY 1,000,000 in Japan), duty-free allowances,
  banned items (vapes in Singapore and Thailand, drones in some places, meat / fruit), medicines (above).
- **Onward / return ticket and hotel proof** are asked for at some borders; have the booking emails ready.

## 3. Act
- A timed plan: "file TDAC on <arrival−2>", "file SGAC on <arrival−2>", "apply K-ETA now (takes up to 72 h)",
  "Yakkan Shoumei by <date>". Offer schedule_create reminders (one per item) and calendar notes; create them only
  when the user says yes.
- Forms that need passport data: fill only the Singapore card yourself (`sg-arrival-card`); for the others give the
  official link and the list of values the user will need (from mail / profile), unless the user asks you to fill a
  specific one — then the same rules as the Singapore skill (vault for the passport number, confirm before submit,
  takeover for CAPTCHAs). Never type a passport number from chat.
- Nothing found or unsure → say "not verified" and link the official page; never state a visa rule from memory alone.

## 4. Answer
A table (destination / requirement / when / status / official link / date checked), then the next action and the
reminders created. One line on travel insurance when the trip has flights and checked bags and no policy email was
found (delays, medical and lost luggage are the common claims).

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
