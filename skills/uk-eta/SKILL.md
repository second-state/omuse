---
name: uk-eta
description: 英国电子旅行授权 ETA（gov.uk，收费，有效 2 年；免签旅客入境英国需要，英国和爱尔兰公民不需要）：准备资料、代填申请、付款由用户完成 The UK Electronic Travel Authorisation (paid, valid 2 years; needed by visa-exempt visitors, not British or Irish citizens): gather the data, fill the application, the user pays.
steps: 70
form: on
form_kind: arrival
profile: nationality
---
# UK ETA / 英国电子旅行授权

Visa-exempt visitors to the UK (EU, US, Australia, Singapore, Japan … — not British or Irish citizens, not people with UK immigration permission) need an ETA before travelling, including airside transit at some airports; enforced since February 2026. Apply on gov.uk (the "UK ETA" app or the web form at https://www.gov.uk/eta), a fee applies (£16, rising to £20 for applications from 8 April 2026 — verify), valid 2 years or until the passport expires, multiple entries up to 6 months each. Decisions usually come within 3 working days; apply at least a week ahead. The application needs a passport photo and a face photo taken live — those steps and the payment are the user's (takeover).
This is the traveller's own legal declaration: code fills it (a form function with fixed ids), the user reviews it and presses Submit. The user set the No defaults below themselves for exactly these questions (criminal record, refusals, terrorism / security): filling them is preparing their draft, not attesting for them — they read every answer on the page before they pay, and change any that is not true. Never leave them blank or ask them in the chat.

## How it runs (code)
Filling it in is ONE `arrival_card_fill` call — only when the user asks for it to be filled in or applied for (a
question about it is answered from this page and the official site, with no call). Arguments: `country: "United Kingdom"`, plus `occupation` (the user's job, when they said it) and `purpose` when the user gave them. The ETA has no trip section: no flight or stay is needed.
Code does everything else:
- the official form, filled by its form function (fixed ids) from these facts, the profile and the Vault, with the
  defaults (every health and declaration question No, departure = arrival + 7 days, next city = the city the flight
  came from), never asked;
- the hand-over for the user's own steps (upload the face photo, press Submit and pay the fee) and the reference on the confirmation page.

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
