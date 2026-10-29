---
name: baggage-delayed-lost
description: 托运行李延误/丢失/损坏：离开机场前先填 PIR、跟踪状态、必需品购买留收据、21 天规则与蒙特利尔公约、向航空公司索赔和保险理赔的邮件草稿与清单 Delayed, lost or damaged checked bags — the PIR before leaving the airport, tracking, essentials receipts, the 21-day rule and Montreal Convention limits, claim letters to the airline and the insurer.
---
# Baggage delayed, lost or damaged / 行李延误与丢失

## 1. Right now (if the user is still at the airport)
- **File the PIR** (Property Irregularity Report) at the airline's baggage desk before leaving the airport, for every
  missing or damaged bag; keep the PIR / file reference (format like `SINSQ12345`) and the bag tag numbers (on the
  boarding-pass stub or the airline app). Without a PIR, later claims are usually refused.
- Give the delivery address for the coming days (hotel name and dates — from the booking email) and a phone number.
- Damage: photograph the bag and the tag at the desk; airlines want damage reported within 7 days of receipt.

## 2. Track and chase
- Most airlines show the file on their own baggage-tracing page (WorldTracer behind it): browser_read the airline's
  "delayed baggage" page with the file reference and name; quote the status and date. Set a watch_create or a
  schedule_create check twice a day and notify_user on change.
- Delayed bags are usually found within a few days. If nothing after 5 days the airline asks for a detailed
  inventory (contents, values, purchase dates) — help the user write it (make_xlsx if long).
- **21 days**: a bag not found within 21 days of the date it should have arrived is treated as lost, and the lost-bag
  claim starts (airline form or email) — many airlines then pay the depreciated value of the contents and the bag
  against receipts, up to the Montreal Convention limit for international flights (about 1,519 SDR per passenger in
  2026 — verify the current figure and the airline's own policy page, with the date). Carriers in the same country
  may apply national rules instead; check the airline's conditions of carriage.

## 3. Essentials while waiting
Reasonable necessities (toiletries, underwear, a change of clothes; charger, medication, work clothes if needed) are
usually reimbursed by the airline on receipts, or by the insurer's "baggage delay" benefit (often after 6–12 hours,
capped per day). Say: keep every receipt, buy modestly, keep the boarding pass and tag. Some airlines pay a fixed
daily allowance instead — read their page.

## 4. Insurance
gmail_search `insurance (policy OR certificate OR travel)` and read the baggage section: delay benefit, loss limit,
per-item caps (electronics, jewellery often low or excluded), excess, and the documents required (PIR, airline
settlement letter, receipts, photos, boarding passes, bag tags). Many policies pay only what the airline does not;
claim from the airline first and keep its reply. No policy found → one line: lost and delayed luggage are among the
most common travel claims; ask whether a credit card or an employer covers it.

An FWD policy (Singapore, or a Hong Kong plan underwritten by bolttech) → load_skill("fwd-travel-insurance") for the
claim itself: it knows FWD's claim form and bolttech's claims mailbox.

## 5. Write the letters (drafts, approved by the user)
- To the airline: delayed-bag expenses claim (file reference, flight, bag tags, itemised receipts, bank details to be
  given securely — not in the email body), or the lost-bag claim after 21 days with the inventory.
- To the insurer: the claim with the airline's response attached.
- Keep every reference in memory_remember (domain=trip) so follow-ups find them.

## 6. Answer
What to do now (numbered), the references found, the tracking status with date, what the airline and the policy owe
(quoted, with sources), the receipts to keep, the watch you set up, and the drafts ready to send. No guesses on
amounts: "not found on the policy" is an answer.

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
