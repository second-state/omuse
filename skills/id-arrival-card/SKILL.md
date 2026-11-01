---
name: id-arrival-card
description: 代填印尼 All Indonesia 电子入境卡（allindonesia.imigrasi.go.id，入境前 3 天内，免费；e-VOA 和巴厘岛旅游税另付）Fill in Indonesia's All Indonesia arrival card (free, within 3 days before arrival; the e-VOA and the Bali levy are separate, paid steps).
steps: 70
form: on
form_kind: arrival
profile: nationality
---
# Indonesia — All Indonesia arrival card / 印尼电子入境卡

Indonesia merged the customs declaration and the arrival card into the All Indonesia portal (https://allindonesia.imigrasi.go.id), filed free within the 3 days before arrival; it became mandatory at Jakarta (CGK), Bali (DPS), Surabaya (SUB) and Batam first and is extending to other ports — verify the current list; elsewhere the electronic customs declaration (e-CD) is still used. The e-VOA (visa on arrival, IDR 500,000 as of Oct 2026) and the Bali tourist levy (IDR 150,000) are separate, paid, on official .go.id sites — the user pays those (takeover at the payment step).
This is the traveller's own legal declaration: code fills it (a form function with fixed ids), the user reviews it and presses Submit.

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
- Flight and date from mail: gmail_search `(Indonesia OR Bali OR Denpasar OR DPS OR Jakarta OR CGK OR Surabaya OR Batam) (flight OR e-ticket OR itinerary) newer_than:60d`; the onward /
  return flight if the form asks for the next destination or length of stay.
- Accommodation: gmail_search `(hotel OR booking OR reservation) (Indonesia OR Bali OR Denpasar OR DPS OR Jakarta OR CGK OR Surabaya OR Batam) newer_than:60d` → name and address; or the
  host's address from the request.
- **The user's own address in Indonesia** (a resident going home, an owner's flat, family): profile_get the address fields
  — `address_home`, `address_work` and any number of `address:<label>` entries. Exactly one of them is in Indonesia → use it
  as the accommodation / address of stay without asking, and say so in the answer (a hotel booking for these dates in
  mail wins over it). Several are in Indonesia → **ask in the chat which one to use**, listing them by label, before
  opening the browser. None → the mail / calendar / memory sources above, then the one question. An address the
  user gives in the chat that the profile lacks → profile_suggest `address:<label>` so it is there next time.
- Identity: profile_get → name as in the passport, sex, date of birth, nationality, email, phone. Passport number and
  expiry are in the vault (vault_list, kind id_document) — never asked for in chat, never in the answer. Missing
  profile fields: ask the user to upload the passport data page once, file_look it for name / sex / birth date /
  nationality / expiry, propose those with profile_suggest, and ask them to save the number in the Vault.
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
  - next city / next port after this country → the city the arrival flight comes from (the last city before this
    country), unless an onward flight in mail or the request names another;
  - departure date from this country → arrival date + 7 days (count the days on the calendar, e.g. 01/11 → 08/11, 28/11 → 05/12; the calculate tool does no date arithmetic), unless a return / onward flight or the length
    of stay is known; length of stay → the days in between.
- Purpose of visit (when the request does not say it): **ask once**, in the same message as a missing flight or stay.
  Ask in the chat only for the flight, the stay and the purpose — nothing else stops the fill: any other value you do
  not have (occupation, a visa number, …) stays empty on the form and is named in the takeover reason for the user to
  type; the defaults below are never asked.
- The card asks: passport data, nationality, sex, date of birth, email, flight and arrival date, port, purpose, address in Indonesia, and the customs questions (goods above the allowance, cash, animals / plants, medicines). The customs questions follow the declaration defaults above (No) unless the user said otherwise.
## 2. Timing
More than 3 days before arrival: schedule_create a reminder for arrival − 3 days ("fill the All Indonesia arrival card for <flight> with the id-arrival-card skill"); an e-VOA can be applied for earlier. Inside the window: continue.

## 3. Fill the form (browser)
1. browser_navigate https://allindonesia.imigrasi.go.id/.
2. Code (not you) calls **form_fill** with the trip facts from step 1 — flight_no, airline and arrival_airport from the e-ticket, arrival_date,
   origin_city / origin_country, departure_date and next_city only when the mail or the user gave them, stay_name /
   stay_address / stay_kind (hotel, own, friends, other), purpose. Code fills the All Indonesia card from them, the profile and the
   Vault, step by step, and says what it filled and what it left.
3. Hand the page over now — browser_request_takeover, the reason naming the certification and Submit and anything form_fill left empty.
   Do not re-type what form_fill filled. After the hand-back: browser_snapshot and quote the confirmation number if it shows.
4. A page no form function knows (the site changed) goes to the `browser-use` skill.

## 4. Answer
Status (filed / ready to submit / waiting for data / too early + reminder set), the values entered (no passport
number), the reference when filed, what to show at the border, and anything on the official page that differed from
this skill (then follow the page and say so).

## Remember what you learn (for next time)
Nationality, passport expiry (not the number), usual accommodation and purpose → profile_suggest (`nationality`,
`custom:passport expiry`, `custom:travel purpose`); family members' names and birthdays only if the user asks you to
keep them (`custom:family travellers`). The passport number is added to the Vault by the user; a passport photo may be
read once with file_look for the other fields and should not stay in the workspace.
