---
name: cn-arrival-card
description: 代填中国在线入境卡（国家移民管理局 s.nia.gov.cn / NIA 12367 App，入境前或口岸填写，免费，含免签旅客）Fill in China's online arrival card for foreigners (NIA, free, before arrival or at the port, visa-free travellers included).
steps: 70
form: on
form_kind: arrival
profile: nationality
---
# China online arrival card / 中国在线入境卡

> **Ask first, fill on request.** A question about this card (what to file, when, what it needs) gets the requirements
> and the official link: call `arrival_card_fill` with `file: false` (the default) — code answers and opens nothing.
> Set `file: true` only when the user explicitly asks you to fill it in or apply for them.

Since 20 November 2025 foreigners entering mainland China file the arrival card online, free, at https://s.nia.gov.cn/ArrivalCardFillingPC/ (or the NIA 12367 app and the WeChat / Alipay mini programs); it can also be done on the devices at the port, and paper cards still exist. It applies with or without a visa (visa-free entries included) and to air, land and sea arrivals. Exempt: holders of the Foreign Permanent Resident ID card, Mainland Travel Permit holders (HK / Macau residents), group-visa travellers, direct transit under 24 h inside the restricted area, cruise passengers in and out on the same ship, crew. The NIA warns about fake paid sites — only the addresses above.
This is the traveller's own legal declaration: code fills it (a form function with fixed ids), the user reviews it and presses Submit.

## How it runs (code)
Filling it in is ONE `arrival_card_fill` call — only when the user asks for it to be filled in or applied for (a
question about it is answered from this page and the official site, with no call). Arguments: `country: "China"` and every trip detail the user gave or a booking shows: `flight_no` (the flight arriving in China), `arrival_date` (YYYY-MM-DD), `origin_city`, `stay_name` / `stay_address`, `purpose`, and `departure_date` / `next_city` only when they are known.
Code does everything else:
- the bookings: with a mailbox connected and the trip not given, code runs the fixed mail searches and fetches
  the e-ticket and the hotel booking. The result `mail_read` means: read the flight and the stay in those mails
  and call `arrival_card_fill` again with them;
- the user's own address in China from the profile when there is no booking (one: used; several: a choice
  card — then call again with `own_address` = the label the user picked);
- the filing window (too early: a reminder for the morning it opens, nothing filed);
- the official form, filled by its form function (fixed ids) from these facts, the profile and the Vault, with the
  defaults (every health and declaration question No, departure = arrival + 7 days, next city = the city the flight
  came from), never asked;
- the hand-over for the user's own steps (upload the passport page first, then answer the companion question and press Submit) and the reference on the confirmation page.

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
