---
name: kh-arrival-card
description: 代填柬埔寨电子入境卡 e-Arrival Card（arrival.gov.kh，乘机入境前 7 天内，免费；陆路/水路仍用纸本）Fill in the Cambodia e-Arrival Card (free, within 7 days before arrival by air; land and river crossings still use paper).
steps: 70
form: on
form_kind: arrival
profile: nationality
---
# Cambodia e-Arrival Card / 柬埔寨电子入境卡

> **Ask first, fill on request.** A question about this card (what to file, when, what it needs) gets the requirements
> and the official link: call `arrival_card_fill` with `file: false` (the default) — code answers and opens nothing.
> Set `file: true` only when the user explicitly asks you to fill it in or apply for them.

Air arrivals file the Cambodia e-Arrival Card, free, within the 7 days before arrival at https://arrival.gov.kh (immigration, customs and health in one); land and river borders still use paper. Visas (e-visa or on arrival) are separate; the e-Arrival Card is needed even for visa-free entries.
This is the traveller's own legal declaration: code fills it (a form function with fixed ids), the user reviews it and presses Submit.

## How it runs (code)
Filling it in is ONE `arrival_card_fill` call — only when the user asks for it to be filled in or applied for (a
question about it is answered from this page and the official site, with no call). Arguments: `country: "Cambodia"` and every trip detail the user gave or a booking shows: `flight_no` (the flight arriving in Cambodia), `arrival_date` (YYYY-MM-DD), `arrival_airport` (IATA), `origin_city` / `origin_country`, `stay_name` / `stay_address`, `stay_phone` (the hotel's or host's phone number: the card requires it), `purpose`, and `departure_date` / `next_city` only when they are known.
Code does everything else:
- the bookings: with a mailbox connected and the trip not given, code runs the fixed mail searches and fetches
  the e-ticket and the hotel booking. The result `mail_read` means: read the flight and the stay in those mails
  and call `arrival_card_fill` again with them;
- the user's own address in Cambodia from the profile when there is no booking (one: used; several: a choice
  card — then call again with `own_address` = the label the user picked);
- the filing window (too early: a reminder for the morning it opens, nothing filed);
- the official form, filled by its form function (fixed ids) from these facts, the profile and the Vault, with the
  defaults (every health and declaration question No, departure = arrival + 7 days, next city = the city the flight
  came from), never asked;
- the hand-over on the card's last step, Confirm Declaration, for the user's own step (tick the declaration and press Submit) and the reference on the confirmation page.

The result `needs_input` lists what only the user can give: ask for exactly that, in one message, then call again
with every field; `invalid`: fix the arguments and call again. Never fill, click or type on the form yourself.

## Answer
Word the reply from the tool's JSON result: its status (submitted, scheduled, needs_choice, waiting_on_user,
not_confirmed, handed_over), its `show` facts (the reference, the day the window opens, the step left to
the user), what was assumed, and — when submitted — that the passport is what to show at the checkpoint. Never the
passport number.

## Remember what you learn (for next time)
Nationality, passport expiry (not the number), usual accommodation and purpose → profile_suggest (`nationality`,
`custom:passport expiry`, `custom:travel purpose`); family members' names and birthdays only if the user asks you to
keep them (`custom:family travellers`). The passport number is added to the Vault by the user; a passport photo may be
read once with file_look for the other fields and should not stay in the workspace.
