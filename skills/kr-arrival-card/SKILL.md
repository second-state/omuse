---
name: kr-arrival-card
description: 代填韩国电子入境卡 e-Arrival Card（e-arrivalcard.go.kr，入境前 3 天内，免费；K-ETA 另行申请）Fill in the Korea e-Arrival Card (free, within 3 days before arrival; K-ETA is a separate application).
steps: 70
form: on
form_kind: arrival
profile: nationality
---
# Korea e-Arrival Card / 韩国电子入境卡

Foreign visitors without a K-ETA or a Korean residence file the e-Arrival Card, free, within the 3 days before arrival at https://www.e-arrivalcard.go.kr; it stays valid for 72 hours. K-ETA holders and registered residents are exempt (verify the exemption list; the K-ETA waiver for a list of countries runs to 31 Dec 2026 as of Oct 2026). Children under 14 are filed by a parent. K-ETA itself (k-eta.go.kr, a paid authorisation applied for days ahead) is a different form: say which one the traveller needs.
This is the traveller's own legal declaration: code fills it (a form function with fixed ids), the user reviews it and presses Submit.

## How it runs (code)
Filling it in is ONE `arrival_card_fill` call — only when the user asks for it to be filled in or applied for (a
question about it is answered from this page and the official site, with no call). Arguments: `country: "Korea"` and every trip detail the user gave or a booking shows: `flight_no` (the flight arriving in Korea), `arrival_date` (YYYY-MM-DD), `origin_city`, `stay_name` / `stay_address`, `purpose`, and `departure_date` / `next_city` only when they are known.
Code does everything else:
- the bookings: with a mailbox connected and the trip not given, code runs the fixed mail searches and fetches
  the e-ticket and the hotel booking. The result `mail_read` means: read the flight and the stay in those mails
  and call `arrival_card_fill` again with them;
- the user's own address in Korea from the profile when there is no booking (one: used; several: a choice
  card — then call again with `own_address` = the label the user picked);
- the filing window (too early: a reminder for the morning it opens, nothing filed);
- the official form, filled by its form function (fixed ids) from these facts, the profile and the Vault, with the
  defaults (every health and declaration question No, departure = arrival + 7 days, next city = the city the flight
  came from), never asked;
- the hand-over for the user's own steps (press Submit and enter the code Korea emails) and the reference on the confirmation page.

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
