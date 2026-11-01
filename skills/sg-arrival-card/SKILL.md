---
name: sg-arrival-card
description: 代填新加坡电子入境卡 SG Arrival Card（ICA 官网，入境前 3 天内免费提交）：从邮件取航班和酒店，从档案取姓名生日，护照号从保险箱填入，健康和申报问题默认填「否」，由用户核对后自己提交 Fill in the Singapore SG Arrival Card on the ICA site (free, within 3 days before arrival): flight and hotel from mail, name and birthday from the profile, passport number from the vault, health and declaration questions default to No, the user reviews and submits.
steps: 70
form: on
form_kind: arrival
profile: nationality
---
# Singapore SG Arrival Card / 新加坡电子入境卡

Every traveller entering Singapore (citizens and residents too) files the SG Arrival Card with its electronic health
declaration, free, within the 3 days before arrival, only at the ICA site: https://eservices.ica.gov.sg/sgarrivalcard/
(any site that charges a fee is a scam; never use one). Transit passengers who stay airside do not file. This is a
legal declaration by the traveller: code fills it (a form function with fixed ids), the user reviews it and presses Submit.

**How the user wants this done — applies to every step below.** Fill the whole form in one go and end with ONE
browser_request_takeover; never end the task with a question in the chat about a value that has a default. Answer the
defaults yourself and say what you assumed in the takeover reason, so the user corrects it on the page if needed:
every health and declaration question **No** (symptoms, travel history, other names / passports, refused entry,
criminal record, security, goods / cash to declare); **departure date = arrival + 7 days** and **next city = the city
the flight came from** when no return / onward flight or check-out date is known (a one-way ticket and a hotel booking
without a check-out are exactly that case — not a reason to ask); sex from the profile. These are draft answers the
user reviews before pressing Submit, like every value you took from the mail. The only questions you may ask in the
chat, once and before filling: the flight, the stay and the purpose of the visit, when neither the request nor the
mail / calendar / memory has them. Anything else you lack stays empty and is named in the takeover reason. Example
reason: "Filled from your e-ticket, hotel booking, profile and Vault. I answered every health and declaration
question No, set departure to 08/11/2026 (arrival + 7 days) and next city Sydney — change any that are not right,
type the CAPTCHA and press Submit; hand the browser back when the confirmation shows."

## 1. Gather the data (no browser yet, one turn)
- Arrival flight: gmail_search `(Singapore OR SIN OR Changi) (flight OR e-ticket OR itinerary) newer_than:60d` →
  flight number, arrival date, origin ("last city before Singapore"), and the departure flight after Singapore if any
  ("next city after Singapore" and length of stay).
- Accommodation: gmail_search `(hotel OR booking OR reservation) (Singapore) newer_than:60d` → hotel name and address;
  or the host's address from the request (friends / family: residential address, postal code).
- **The user's own address in Singapore** (a resident going home, an owner's flat, family): profile_get the address fields
  — `address_home`, `address_work` and any number of `address:<label>` entries. Exactly one of them is in Singapore → use it
  as the accommodation / address of stay without asking, and say so in the answer (a hotel booking for these dates in
  mail wins over it). Several are in Singapore → **ask in the chat which one to use**, listing them by label, before
  opening the browser. None → the mail / calendar / memory sources above, then the one question. An address the
  user gives in the chat that the profile lacks → profile_suggest `address:<label>` so it is there next time.
- Identity: profile_get → full name as in the passport, gender, date of birth, nationality, email, mobile number.
  Passport number, expiry and country of issue are in the vault (vault_list, kind id_document) — never ask for them in
  chat and never put them in the answer. If the profile lacks these fields, ask the user to upload a photo of the
  passport data page once: file_look it for name (as printed, in the machine-readable line order), sex, date of
  birth, nationality, expiry, and propose those non-secret fields with profile_suggest; the number goes into the
  Vault by the user's hand (Memory → Profile → Vault), never into memory, files or chat.
- **No mail, or no booking in it** (the gmail tools are missing, or the searches find no flight / stay for this
  trip): in the same turn look at calendar_list_events around the travel dates (flight and hotel entries),
  memory_search "trip", "flight", "hotel" with the country name, and the travel connector's tracked flights if it is
  connected. Still missing → **ask once**, naming exactly what you need: "Which flight do you arrive on (number and
  date)? Where are you staying (hotel name or address)? Or upload the e-ticket / booking confirmation." Fill
  everything else from the profile and the vault meanwhile — never hand the user a blank form because a booking is
  missing. An uploaded PDF or screenshot: file_look it for flight number, date, origin, onward flight, hotel name and
  address. With no mail connected, add one line that connecting a mailbox (Settings → Connections) lets you find
  bookings yourself next time. Keep what the user tells you: memory_remember "Trip <country> <dates>: <flight>,
  staying at <hotel>" (domain=place), so the other travel skills don't ask again.
- **"I'll fill it in myself"** (or the user won't give the trip details): open the official form, fill what you
  already have (profile, vault), then browser_request_takeover with the reason "Fill in the flight and accommodation,
  review the form and press Submit yourself; hand the browser back when the confirmation shows".
- **Defaults — fill these without asking** (only what the user said in this conversation overrides them):
  - every health question → **No**: symptoms (fever, cough, shortness of breath, rash, diarrhoea, any illness) and
    travel history (Africa / South America / yellow-fever countries, farms, contact with sick people or animals);
  - every declaration question → **No**: other names or other passports used, refused entry / deported / overstayed,
    criminal record or convictions, goods, cash or items to declare;
  - sex / gender → the profile's `gender` (as on the passport); missing → ask once and profile_suggest it;
  - next city / next port after Singapore → the city the arrival flight comes from (the last city before this
    country), unless an onward flight in mail or the request names another;
  - departure date from Singapore → arrival date + 7 days (count the days on the calendar, e.g. 01/11 → 08/11, 28/11 → 05/12; the calculate tool does no date arithmetic), unless a return / onward flight or the length
    of stay is known; length of stay → the days in between.
- Trip purpose (holiday / business / visiting family / transit) and the accommodation when none was found: **ask once**,
  in one message, for what is still missing — never guess the accommodation.
  Ask in the chat only for the flight, the stay and the purpose — nothing else stops the fill: any other value you do
  not have stays empty on the form and is named in the takeover reason for the user to type; the defaults are never asked.
- Group: a family can be filed together (up to 10) — collect each traveller's details the same way; each needs a vault
  item or the user types it during a takeover.

## 2. Timing
- Earlier than 3 days before arrival: don't fill it — set a reminder instead (schedule_create for the morning of
  arrival − 3 days, goal "fill the SG Arrival Card for <flight> on <date>") and say so.
- Within the window: continue.

## 3. Fill the form (browser)
1. browser_navigate https://eservices.ica.gov.sg/sgarrivalcard/.
2. Code (not you) calls **form_fill** with the trip facts from step 1 — flight_no, arrival_date, origin_city / origin_country, departure_date and
   next_city only when the mail or the user gave them, stay_name / stay_address / stay_kind (hotel, own, friends, other),
   purpose. Code fills the whole form from them, the profile and the Vault (the visitor's form, the passport, the dates,
   the lookups, No to the health and declaration questions) and stops at Review. The result lists what was filled and
   what was not.
3. Fields form_fill could not fill are left for the user: hand the page over now — browser_request_takeover, the reason
   naming the declaration, Submit and anything form_fill left empty. Do not re-type what form_fill filled.
4. A page no form function knows (the site changed) goes to the `browser-use` skill.
5. After the hand-back: browser_snapshot. A DE number (Declaration Entry) on the page = filed: quote it, the names
   covered, what the user changed and their email. No confirmation → say it is not confirmed yet.

## 4. Answer
Status (filed / ready to submit / waiting for data / too early), what was entered (no passport number), the DE
number when filed, and one line on what to show at the checkpoint (passport only; automated lanes need the SGAC on
record). If anything on the ICA page contradicts this skill (new fields, new window), follow the page and say so.

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
