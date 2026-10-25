---
name: tax-refund-shopping
description: 游客退税：日本 2026 年 11 月 1 日起改为出境时退税（90 天内购买、行李托运前过机）、韩国即时退税与机场退税、新加坡 eTRS、泰国 P.P.10、台湾电子退税；门槛、单据、机场流程与时间，按行程提醒 Tourist tax refunds — Japan's switch to refund-at-departure from 1 November 2026, Korea instant and airport refunds, Singapore eTRS, Thailand P.P.10, Taiwan e-VAT; thresholds, paperwork, airport steps and timing, with reminders for the departure day.
---
# Tourist tax refunds / 游客退税

Thresholds and procedures change; the facts here were checked in October 2026 and carry that date. Before quoting a
threshold, browser_read the official customs / tax page named and give the date checked. Keep the user's receipts out
of the answer except as a list.

## 1. Country notes (verify, then apply to the trip dates)
- **Japan** — until 31 Oct 2026: tax-free at the till (passport shown, 10% off, consumables sealed, ¥5,000 minimum per
  shop per day). **From 1 Nov 2026**: pay the full price in the shop (passport scanned, purchase recorded), and claim
  the consumption tax back at departure: scan the passport at the tax-free terminal in the public area **before
  checking in luggage**; green = done, red = show the goods at the customs counter. Goods bought within 90 days of
  departure; everything on one receipt must be present or that receipt is void; the consumables cap and sealing are
  gone. No overlap period — a trip spanning 1 Nov follows both procedures. Refund paid later to the card / account
  chosen in the shop's system. Official: Japan Customs / National Tax Agency pages.
- **Korea** — VAT (and some individual consumption tax) refunded for foreigners staying under 6 months; "Tax Free"
  shops; instant refund at the till up to a per-receipt and per-stay cap, otherwise airport refund (kiosk or counter
  after customs stamping of goods above the inspection threshold); minimum per receipt around ₩15,000 — figures
  differ between sources, verify on the Korea Customs Service page.
- **Singapore** — GST (9%) refund through eTRS for tourists: minimum SGD 100 per shop (up to 3 same-day receipts),
  passport at purchase, self-help eTRS kiosk at Changi **before immigration** for checked goods and **after** for
  hand-carried, within 2 months of purchase; refund to card or cash counter; not for Singapore citizens / PRs /
  long-term pass holders. Official: IRAS.
- **Thailand** — VAT refund for tourists: shops with the "VAT Refund for Tourists" sign, at least ฿2,000 per shop per
  day, ask for the P.P.10 form with the original receipt; at the airport show goods and forms at the VAT refund
  office **before check-in** (customs stamp), collect after immigration (cash below ฿30,000, otherwise card / draft,
  fees apply); minimum total ฿5,000 per trip. Official: Thai Revenue Department.
- **Taiwan** — 5% VAT refund for foreign passports: NT$2,000 minimum per shop per day at TRS shops, within 90 days;
  small amounts refunded in the shop or at E-VAT refund machines / service counters at the airport before check-in
  (goods inspection for larger claims). Official: Taiwan Tax Refund (taxrefund.net.tw).
- **Hong Kong, Macau**: no sales tax, no refund. **Mainland China**: departure tax refund at designated shops in
  listed cities (¥200 minimum per shop per day, 11% VAT refunded at ~9%), at the airport before check-in — verify.
- **Malaysia** (TRS at airports, RM300 minimum), **Indonesia** (VAT refund, IDR 500,000 per receipt, airport), **Vietnam**
  (VAT refund counters at international airports, VND 2 million minimum) — verify each before quoting.

## 2. Plan it into the trip
- Before shopping: which shops, the passport (a copy is not enough), one receipt per shop per day where possible,
  don't use or open goods that must be shown; for Japan after 1 Nov the goods and receipts must be in hand at the
  terminal before bags are checked.
- Departure day: add "tax refund — 60 min before check-in" to the airport timeline (calendar event or reminder,
  schedule_create), with the counter's location (browser_read the airport page) and what to carry.
- Expense trips: refunded tax is not a reimbursable cost — note it for `travel-expense-report`.

## 3. Answer
Per country: eligible? / minimum / where and when / documents / refund form (cash, card) / date checked / official
link; then the departure-day steps in order. Say what was not verified; never invent a threshold.

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
