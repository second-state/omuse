---
name: ph-etravel
description: 代填菲律宾 eTravel（etravel.gov.ph，入境前 72 小时内，免费，出境的菲律宾公民也要填）Fill in the Philippines eTravel registration (free, within 72 hours before arrival; departing Filipino citizens too).
---
# Philippines eTravel / 菲律宾 eTravel

Arriving passengers (Filipino and foreign) register on eTravel, free, within the 72 hours before arrival at https://etravel.gov.ph; departing Filipino citizens register too (no later than 3 hours before departure). The portal gives a QR code shown at immigration. Diplomats and some official passports are exempt. Any site charging a fee is not eTravel.
This is the traveller's own legal declaration, so the user confirms every value before you submit.

## 1. Gather the data (no browser yet, one turn)
- Flight and date from mail: gmail_search `(Philippines OR Manila OR MNL OR Cebu OR CEB OR Clark OR CRK) (flight OR e-ticket OR itinerary) newer_than:60d`; the onward /
  return flight if the form asks for the next destination or length of stay.
- Accommodation: gmail_search `(hotel OR booking OR reservation) Philippines OR Manila OR MNL OR Cebu OR CEB OR Clark OR CRK newer_than:60d` → name and address; or the
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
- eTravel asks: passport data, nationality, sex, date of birth, email, phone, country of residence, flight and arrival date, purpose, address in the Philippines, and a health / travel-history section; returning residents answer a few more questions.
## 2. Timing
More than 72 hours before arrival: schedule_create a reminder for arrival − 3 days ("register eTravel for <flight> with the ph-etravel skill"). Inside the window: continue.

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
