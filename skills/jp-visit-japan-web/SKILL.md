---
name: jp-visit-japan-web
description: 代填日本 Visit Japan Web（入境审查 + 海关申报，一个二维码；可选但强烈建议；需要账号登录）Register the Visit Japan Web entry: immigration and customs declaration in one QR code (optional but recommended; needs the traveller's own account).
steps: 70
form: on
form_kind: arrival
profile: nationality
---
# Visit Japan Web / 日本入境二维码

Visit Japan Web (https://www.vjw.digital.go.jp, from the landing page vjw-lp.digital.go.jp) is optional: it pre-fills the immigration card and the customs declaration and gives one QR code used at both counters; paper forms still work. It needs an account (email + password) — the user signs in themselves (browser_request_takeover for the login and any 2-factor code), then you fill the trip. Register at least a few hours before departure; days before is better. The tax-free shopping registration can be added in the same account.
This is the traveller's own legal declaration: code fills it (a form function with fixed ids), the user reviews it and presses Submit.

## How it runs (code)
Filling it in is ONE `arrival_card_fill` call — only when the user asks for it to be filled in or applied for (a
question about it is answered from this page and the official site, with no call). Arguments: `country: "Japan"` and every trip detail the user gave or a booking shows: `flight_no` (the flight arriving in Japan), `arrival_date` (YYYY-MM-DD), `origin_city`, `stay_name` / `stay_address`, `purpose`, and `departure_date` / `next_city` only when they are known.
Code does everything else:
- the bookings: with a mailbox connected and the trip not given, code runs the fixed mail searches and fetches
  the e-ticket and the hotel booking. The result `mail_read` means: read the flight and the stay in those mails
  and call `arrival_card_fill` again with them;
- the user's own address in Japan from the profile when there is no booking (one: used; several: a choice
  card — then call again with `own_address` = the label the user picked);
- the filing window (too early: a reminder for the morning it opens, nothing filed);
- the official form, filled by its form function (fixed ids) from these facts, the profile and the Vault, with the
  defaults (every health and declaration question No, departure = arrival + 7 days, next city = the city the flight
  came from), never asked;
- the hand-over for the user's own steps (sign in to their own account first, then type the security check and press Register) and the reference on the confirmation page.

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
