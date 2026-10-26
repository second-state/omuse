---
name: sg-arrival-card
description: 代填新加坡电子入境卡 SG Arrival Card（ICA 官网，入境前 3 天内免费提交）：从邮件取航班和酒店，从档案取姓名生日，护照号从保险箱填入，健康申报问用户，提交前给用户核对 Fill in the Singapore SG Arrival Card on the ICA site (free, within 3 days before arrival): flight and hotel from mail, name and birthday from the profile, passport number from the vault, health questions asked, the user confirms before submission.
---
# Singapore SG Arrival Card / 新加坡电子入境卡

Every traveller entering Singapore (citizens and residents too) files the SG Arrival Card with its electronic health
declaration, free, within the 3 days before arrival, only at the ICA site: https://eservices.ica.gov.sg/sgarrivalcard/
(any site that charges a fee is a scam; never use one). Transit passengers who stay airside do not file. This is a
legal declaration by the traveller, so the user confirms every value before you submit.

## 1. Gather the data (no browser yet, one turn)
- Arrival flight: gmail_search `(Singapore OR SIN OR Changi) (flight OR e-ticket OR itinerary) newer_than:60d` →
  flight number, arrival date, origin ("last city before Singapore"), and the departure flight after Singapore if any
  ("next city after Singapore" and length of stay).
- Accommodation: gmail_search `(hotel OR booking OR reservation) Singapore newer_than:60d` → hotel name and address;
  or the host's address from the request (friends / family: residential address, postal code).
- **The user's own address in Singapore** (a resident going home, an owner's flat, family): profile_get the address fields
  — `address_home`, `address_work` and any number of `address:<label>` entries. Exactly one of them is in Singapore → use it
  as the accommodation / address of stay without asking, and say so in the answer (a hotel booking for these dates in
  mail wins over it). Several are in Singapore → **ask in the chat which one to use**, listing them by label, before
  opening the browser. None → the mail / calendar / memory sources above, then the one question. An address the
  user gives in the chat that the profile lacks → profile_suggest `address:<label>` so it is there next time.
- Identity: profile_get → full name as in the passport, sex, date of birth, nationality, email, mobile number.
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
- Trip purpose (holiday / business / visiting family / transit), and the health declaration answers: fever, cough,
  shortness of breath, rash in the last days; visits to Africa or South America (yellow fever) in the last 6 days.
  **Ask the user** for anything missing, in one message — never guess health answers or the accommodation.
- Group: a family can be filed together (up to 10) — collect each traveller's details the same way; each needs a vault
  item or the user types it during a takeover.

## 2. Timing
- Earlier than 3 days before arrival: don't fill it — set a reminder instead (schedule_create for the morning of
  arrival − 3 days, goal "fill the SG Arrival Card for <flight> on <date>") and say so.
- Within the window: continue.

## 3. Fill the form (browser)
1. browser_navigate https://eservices.ica.gov.sg/sgarrivalcard/ → "Foreign Visitors" (or Citizens / Residents),
   individual or group submission. Read the page before every step; the form changes over time — follow what is on
   the screen, not this list.
2. Type what you have with browser_type / browser_select: name, sex, date of birth, nationality, email, mobile, arrival
   date, mode (air), flight number, last city, next city, length of stay, accommodation type and hotel / address,
   purpose of visit. Passport number and expiry: browser_fill_secret from the vault item (field `number`, `expiry`) —
   one fill per turn, each one is approved by the user. If Sentinel says the item is restricted to other sites, tell
   the user to add ica.gov.sg to that vault item (Memory → Profile → Vault) and stop until they have.
3. Health declaration: tick exactly what the user told you.
4. A CAPTCHA, OTP or sign-in → browser_request_takeover; never try to solve it.
5. **The Submit is the user's**: with every field filled, call browser_request_takeover with the reason "Review the
   SG Arrival Card and press Submit yourself; hand the browser back when the DE number shows" — a card opens in the
   chat and the live browser view, the user checks the values (the passport number is masked on the page) and presses
   Submit, then hands back. Never click Submit yourself unless the user wrote "submit it for me without review" in
   the request; then click it once (Sentinel shows it in the approval history) and read the result page.
6. After the hand-back: browser_snapshot. A DE number (Declaration Entry) on the page = filed: quote it, the names
   covered and the arrival date, say ICA emails an acknowledgement, offer to add it to the arrival-day calendar event
   and memory_remember "SGAC filed for <trip>". Still the form = not submitted: say so, leave the values in place and
   tell the user the tab stays open for them to press Submit (or ask you again).

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
