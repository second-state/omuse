---
name: travel-expense-report
description: 整理一次出差/旅行的全部花费并生成报销单：从邮件收据、信用卡账单（上传的 CSV/PDF）和用户上传的餐厅等纸质收据照片（先用视觉模型 OCR）汇总，去重、分类、按交易日汇率折算，生成 Excel 明细和 PDF 报销申请 Build the expense report and reimbursement request for a whole trip: receipts from mail, card statements (uploaded CSV / PDF) and photographed paper receipts (OCR with the vision model first), de-duplicated, categorised, converted at the transaction-date rate, delivered as an Excel ledger and a PDF claim.

plan: on
---
# Travel expense report / 出差报销

Numbers come only from documents and tools, never from memory. Every line in the report points to its evidence
(email id, statement row, receipt file). Figures are added with calculate / data_query, never in your head.

## 0. Ask for the inputs first (one message)
- Trip name, dates, purpose, the currency the report must be in (home / company currency), and the policy if there is
  one (per-diem rates, receipt threshold, excluded items, mileage) — or the company's template (attach it).
- **Paper receipts** (restaurants, taxis, pharmacies, shops): ask the user to upload photos or scans of all of them
  before you start; a receipt that exists only on paper cannot be reconstructed from anything else.
- **Card statements**: the statement CSV / Excel / PDF for the trip months, if the card is not in the mailbox.
Stop and wait if receipts or statements are still missing; say what you will do once they arrive.

## 1. Intake — every source, same shape
Target shape per expense: date (local), merchant, city/country, category, amount, currency, tax, tip, payment method
and card last-4, reference (booking / invoice no.), evidence (id or file), notes.
- **Email receipts**: gmail_find_receipts for the trip window with the brands' domains (airline, hotel chains and
  OTAs, Grab / Uber / Didi / Kakao T, Klook, Agoda, Booking, Trip.com, eSIM providers, restaurants' POS mails), then
  gmail_read_amounts for the rest of the hits from gmail_search `(receipt OR invoice OR 收据 OR 領収書 OR 영수증)
  after:<start> before:<end>`. One purchase often has several mails (hold, charge, receipt, tip update): keep the
  final total, note the others as the same purchase.
- **Uploaded photos / scanned PDFs** (the vision model does the OCR): for each file call file_look with one question
  that asks for a JSON object `{merchant, address, date, time, currency, total, subtotal, tax, service_charge, tip,
  payment_method, card_last4, items:[{name, qty, amount}], receipt_no, language, notes}` — values exactly as printed,
  `null` when unreadable. A multi-page or multi-receipt image: ask for an array. If the picture is blurred or cut,
  say which value is unreadable and ask for a better photo rather than guessing. Text PDFs (hotel folios, airline
  invoices): files_read, then the same structure.
- **Card statements**: files_read (PDF / text) or data_query (CSV / Excel): rows in the trip window plus 3 days
  after; capture posted amount in the card currency, the original foreign amount when printed, FX fees, and refunds.
- **Non-receipt items the policy allows**: per diems (days × rate from the policy — the user confirms the rate),
  mileage, tips without receipt below the threshold — mark "no receipt, per policy".

## 2. Reconcile (like a statement import: accept only what adds up)
- Receipt arithmetic: items + tax + service + tip = total (calculate); a mismatch is a note on the line, not a fix.
- **Match** each paper / email receipt to a statement row (same merchant or acquirer name, amount within FX
  rounding, date ± 3 days) and **de-duplicate**: one expense = one line, whatever the number of documents. A
  statement row with no receipt → "receipt missing" (flag above the policy threshold); a receipt with no statement
  row → "paid cash / other card", keep it.
- Hotel folios: split room, tax, meals, minibar, laundry when the policy treats them differently; exclude personal
  items the user names.
- Exclude or flag: alcohol if the policy excludes it, personal purchases, refunds already received, duplicate holds.

## 3. Currency
Convert each line at the **transaction date** with market_data (FX pair, that date; the card's own rate when the
statement prints the converted amount — then use the statement figure and say so). Record the rate used per line;
totals per currency and in the report currency (calculate). Never apply today's rate to old transactions.

## 4. Deliver
- make_xlsx `expenses/<trip>-expenses.xlsx`: sheet "Lines" (one row per expense with every column above + rate +
  home-currency amount + evidence), sheet "Summary" (by category, by day, by currency, totals, per diem, grand total,
  flags), sheet "Missing" (statement rows without receipts, unreadable receipts).
- make_pdf `expenses/<trip>-claim.pdf`: the reimbursement request — traveller, trip, dates, purpose, totals by
  category, grand total in the report currency, a list of attached evidence (file names / email subjects), the
  declaration line, and the flagged items the approver must see. Use the company template's headings when one was
  attached.
- send_file both; the answer itself carries the summary table and the flags, so the user needn't open the file.
- Offer: an email draft to finance with both files attached (gmail_create_draft), and memory_remember the policy
  details (domain=work) for next time.

## 5. Never
Invent an amount, a rate or a date; round totals; submit anything to a company system yourself (fill a company
portal only if the user asks, with approvals); put card numbers in the files (last-4 only).

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
