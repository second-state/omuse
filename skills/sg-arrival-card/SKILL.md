---
name: sg-arrival-card
description: 代填新加坡电子入境卡 SG Arrival Card（ICA 官网，入境前 3 天内免费提交）：从邮件取航班和酒店，从档案取姓名生日，护照号从保险箱填入，健康和申报问题默认填「否」，由用户核对后自己提交 Fill in the Singapore SG Arrival Card on the ICA site (free, within 3 days before arrival): flight and hotel from mail, name and birthday from the profile, passport number from the vault, health and declaration questions default to No, the user reviews and submits.
steps: 70
form: on
form_kind: arrival
profile: nationality
---
# Singapore SG Arrival Card / 新加坡电子入境卡

Every traveller entering Singapore (citizens and residents too) files the SG Arrival Card with its electronic health
declaration, free, within the 3 days before arrival, only at the ICA site: https://eservices.ica.gov.sg/sgarrivalcard/
(any site that charges a fee is a scam; never use one). Transit passengers who stay airside do not file. This is a
legal declaration by the traveller: code fills it (a form function with fixed ids), the user reviews it and presses Submit.

## How it runs (code)
Filling it in is ONE `arrival_card_fill` call — only when the user asks for it to be filled in or applied for (a
question about it is answered from this page and the official site, with no call). Arguments: `country: "Singapore"` and every trip detail the user gave or a booking shows: `flight_no` (the flight arriving in Singapore), `arrival_date` (YYYY-MM-DD), `origin_city`, `stay_name` / `stay_address`, `purpose`, and `departure_date` / `next_city` only when they are known.
Code does everything else:
- the bookings: with a mailbox connected and the trip not given, code runs the fixed mail searches and fetches
  the e-ticket and the hotel booking. The result `mail_read` means: read the flight and the stay in those mails
  and call `arrival_card_fill` again with them;
- the user's own address in Singapore from the profile when there is no booking (one: used; several: a choice
  card — then call again with `own_address` = the label the user picked);
- the filing window (too early: a reminder for the morning it opens, nothing filed);
- the official form, filled by its form function (fixed ids) from these facts, the profile and the Vault, with the
  defaults (every health and declaration question No, departure = arrival + 7 days, next city = the city the flight
  came from), never asked;
- the hand-over for the user's own steps (tick the declaration and press Submit) and the reference on the confirmation page.

The result `needs_input` lists what only the user can give: ask for exactly that, in one message, then call again
with every field; `invalid`: fix the arguments and call again. Never fill, click or type on the form yourself.

## Answer
Word the reply from the tool's JSON result: its status (submitted, scheduled, needs_choice, waiting_on_user,
not_confirmed, handed_over), every fact in its `must_say` (the reference, the day the window opens, the step left to
the user), what was assumed, and — when submitted — that the passport is what to show at the checkpoint. Never the
passport number.

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
