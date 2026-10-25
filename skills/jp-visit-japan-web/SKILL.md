---
name: jp-visit-japan-web
description: 代填日本 Visit Japan Web（入境审查 + 海关申报，一个二维码；可选但强烈建议；需要账号登录）Register the Visit Japan Web entry: immigration and customs declaration in one QR code (optional but recommended; needs the traveller's own account).
---
# Visit Japan Web / 日本入境二维码

Visit Japan Web (https://vjw-lp.digital.go.jp) is optional: it pre-fills the immigration card and the customs declaration and gives one QR code used at both counters; paper forms still work. It needs an account (email + password) — the user signs in themselves (browser_request_takeover for the login and any 2-factor code), then you fill the trip. Register at least a few hours before departure; days before is better. The tax-free shopping registration can be added in the same account.
This is the traveller's own legal declaration, so the user confirms every value before you submit.

## 1. Gather the data (no browser yet, one turn)
- Flight and date from mail: gmail_search `(Japan OR Tokyo OR NRT OR HND OR Osaka OR KIX OR Nagoya OR Fukuoka OR Sapporo) (flight OR e-ticket OR itinerary) newer_than:60d`; the onward /
  return flight if the form asks for the next destination or length of stay.
- Accommodation: gmail_search `(hotel OR booking OR reservation) Japan OR Tokyo OR NRT OR HND OR Osaka OR KIX OR Nagoya OR Fukuoka OR Sapporo newer_than:60d` → name and address; or the
  host's address from the request.
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
- Purpose of visit and the form's own questions (below). **Ask once** for anything missing; never guess health or
  customs answers.
- Visit Japan Web asks: passport data (it can read the passport chip in the app, or the number is typed), the flight and arrival date, the address and phone of the first stay in Japan, and the customs questions (cash over ¥1,000,000, goods over the duty-free allowance, prohibited items, unaccompanied baggage). Ask the user for the customs answers explicitly.
## 2. Timing
Any time before the flight; if departure is more than 10 days away, offer a reminder (schedule_create) instead of filling now, since flight changes would mean redoing it.

## 3. Fill the form (browser)
1. browser_navigate the official page above and read it before every step — forms change; follow the screen, not this
   list. Choose the individual (or family) form.
2. browser_type / browser_select the non-secret values: name as in the passport, sex, date of birth, nationality,
   email, phone, flight number and arrival date, last port / next destination, length of stay, accommodation
   (hotel name and address), purpose. Passport number and expiry only with browser_fill_secret from the vault item
   (fields `number`, `expiry`), one fill per turn, each approved by the user; if Sentinel says the item is restricted to
   other sites, ask the user to add this site to the vault item (Memory → Profile → Vault) and stop.
3. Health, customs and declaration questions: tick exactly what the user told you; never guess.
4. A CAPTCHA, OTP, sign-in or payment step → browser_request_takeover (payments: the user pays; you never enter card data
   here); never try to solve a CAPTCHA.
5. **The Submit is the user's**: with every field filled, call browser_request_takeover with the reason "Review the
   form and press Submit yourself; hand the browser back when the confirmation shows" — a card opens in the chat and
   the live view, the user checks the values (passport number masked on the page), presses Submit (and pays, where a
   fee applies), then hands back. Never click Submit yourself unless the user wrote "submit it for me without review"
   in the request; then click it once and read the result page.
6. After the hand-back: browser_snapshot. A reference / QR / confirmation on the page = filed: quote it exactly, say
   where the acknowledgement arrives (email / app), offer to attach it to the arrival-day calendar event and
   memory_remember "<card> filed for <trip>". Still the form = not submitted: say so, leave the values in place and
   tell the user the tab stays open for them (or ask you again).

## 4. Answer
Status (filed / ready to submit / waiting for data / too early + reminder set), the values entered (no passport
number), the reference when filed, what to show at the border, and anything on the official page that differed from
this skill (then follow the page and say so).

## Remember what you learn (for next time)
Nationality, passport expiry (not the number), usual accommodation and purpose → profile_suggest (`nationality`,
`custom:passport expiry`, `custom:travel purpose`); family members' names and birthdays only if the user asks you to
keep them (`custom:family travellers`). The passport number is added to the Vault by the user; a passport photo may be
read once with file_look for the other fields and should not stay in the workspace.
