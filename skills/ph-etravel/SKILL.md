---
name: ph-etravel
description: 代填菲律宾 eTravel（etravel.gov.ph，入境前 72 小时内，免费，出境的菲律宾公民也要填）Fill in the Philippines eTravel registration (free, within 72 hours before arrival; departing Filipino citizens too).
steps: 70
form: on
form_kind: arrival
profile: nationality
---
# Philippines eTravel / 菲律宾 eTravel

Arriving passengers (Filipino and foreign) register on eTravel, free, within the 72 hours before arrival at https://etravel.gov.ph; departing Filipino citizens register too (no later than 3 hours before departure). The portal gives a QR code shown at immigration. Diplomats and some official passports are exempt. Any site charging a fee is not eTravel.
This is the traveller's own legal declaration: code fills it (a form function with fixed ids), the user reviews it and presses Submit.

## How it runs (code)
Filling it in is ONE `arrival_card_fill` call — only when the user asks for it to be filled in or applied for (a
question about it is answered from this page and the official site, with no call). Arguments: `country: "Philippines"` and every trip detail the user gave or a booking shows: `flight_no` (the flight arriving in Philippines), `arrival_date` (YYYY-MM-DD), `origin_city`, `stay_name` / `stay_address`, `purpose`, and `departure_date` / `next_city` only when they are known.
Code does everything else:
- the bookings: with a mailbox connected and the trip not given, code runs the fixed mail searches and fetches
  the e-ticket and the hotel booking. The result `mail_read` means: read the flight and the stay in those mails
  and call `arrival_card_fill` again with them;
- the user's own address in Philippines from the profile when there is no booking (one: used; several: a choice
  card — then call again with `own_address` = the label the user picked);
- the filing window (too early: a reminder for the morning it opens, nothing filed);
- the official form, filled by its form function (fixed ids) from these facts, the profile and the Vault, with the
  defaults (every health and declaration question No, departure = arrival + 7 days, next city = the city the flight
  came from), never asked;
- the hand-over for the user's own steps (type the CAPTCHA and press Submit) and the reference on the confirmation page.

The result `needs_input` lists what only the user can give: ask for exactly that, in one message, then call again
with every field; `invalid`: fix the arguments and call again. Never fill, click or type on the form yourself.

## Answer
Word the reply from the tool's JSON result: its status (submitted, scheduled, needs_choice, waiting_on_user,
not_confirmed, handed_over), every fact in its `must_say` (the reference, the day the window opens, the step left to
the user), what was assumed, and — when submitted — that the passport is what to show at the checkpoint. Never the
passport number.

## Remember what you learn (for next time)
Nationality, passport expiry (not the number), usual accommodation and purpose → profile_suggest (`nationality`,
`custom:passport expiry`, `custom:travel purpose`); family members' names and birthdays only if the user asks you to
keep them (`custom:family travellers`). The passport number is added to the Vault by the user; a passport photo may be
read once with file_look for the other fields and should not stay in the workspace.
