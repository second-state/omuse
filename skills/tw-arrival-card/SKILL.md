---
name: tw-arrival-card
description: 代填台湾线上入境登记表 TWAC（niaspeedy.immigration.gov.tw/webacard，入境前 3 天内，免费，2025 年 10 月起取代纸本）Fill in the Taiwan Arrival Card (TWAC, free, within 3 days before arrival; replaced the paper card in October 2025).
steps: 70
form: on
form_kind: arrival
profile: nationality
---
# Taiwan Arrival Card (TWAC) / 台湾线上入境登记

> **Ask first, fill on request.** A question about this card (what to file, when, what it needs) gets the requirements
> and the official link: call `arrival_card_fill` with `file: false` (the default) — code answers and opens nothing.
> Set `file: true` only when the user explicitly asks you to fill it in or apply for them.

Visa-exempt and visa visitors file the Taiwan Arrival Card online, free, within the 3 days before arrival at https://twac.immigration.gov.tw/ (paper cards ended on 1 October 2025); residents (ARC) and some permit holders are exempt. The English name field takes letters and spaces only; a confirmation email follows, and the QR / passport is shown at immigration. e-Gate enrolment is a separate counter on arrival.
This is the traveller's own legal declaration: code fills it (a form function with fixed ids), the user reviews it and presses Submit.

## How it runs (code)
Filling it in is ONE `arrival_card_fill` call — only when the user asks for it to be filled in or applied for (a
question about it is answered from this page and the official site, with no call). Arguments: `country: "Taiwan"` and every trip detail the user gave or a booking shows: `flight_no` (the flight arriving in Taiwan), `arrival_date` (YYYY-MM-DD), `origin_city`, `stay_name` / `stay_address`, `purpose`, and `departure_date` / `next_city` only when they are known.
Code does everything else:
- the bookings: with a mailbox connected and the trip not given, code runs the fixed mail searches and fetches
  the e-ticket and the hotel booking. The result `mail_read` means: read the flight and the stay in those mails
  and call `arrival_card_fill` again with them;
- the user's own address in Taiwan from the profile when there is no booking (one: used; several: a choice
  card — then call again with `own_address` = the label the user picked);
- the filing window (too early: a reminder for the morning it opens, nothing filed);
- the official form, filled by its form function (fixed ids) from these facts, the profile and the Vault, with the
  defaults (every health and declaration question No, departure = arrival + 7 days, next city = the city the flight
  came from), never asked;
- the hand-over for the user's own steps (verify the email first, then tick the declaration and press SUBMIT) and the reference on the confirmation page.

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
