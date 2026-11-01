---
name: hk-apdt-refund
description: 申请退还香港机场离境税（APDT）：在官方网站填好所有资料、上传登机牌和付款证明等文件，给用户参考编号和链接，由用户自己打开完成人脸识别 Hong Kong air passenger departure tax (APDT) refund on the official site: every field filled and every document uploaded, then the reference code and a link for the user to open themselves for the face-recognition step.
form: on
---
# Hong Kong departure tax refund (APDT) / 香港机场离境税退款

Official site only: https://app.apdtrefund.hk/hkTaxMobile/?lang=en (the Airport Authority's Refund Application
Platform; free — any other site, or one that charges, is a scam). Code fills every field and uploads every document;
the application ends with a **face-recognition step that only the user can do, with their own phone's camera** — you never
start it, never ask to take over for it and never click "Proceed with Facial Recognition".

**Refund to a card**: code fills the card into the refund page from the Vault (number, expiry, holder) — this page
receives money, so no purchase_confirm (never call it here). Only when the Vault has no complete card (none, or one
saved by name with its number UNKNOWN) does the user type it, in a takeover. Never stop or end the task because of the
card.

## 1. Who qualifies (code checks it on the tool's arguments; this is for questions)
- Arrived in Hong Kong and **left by air from HKIA on the same or the next calendar day** (Hong Kong time), and the
  HK$120 / HK$200 departure tax was **paid** (it shows on the ticket receipt as "HK", "Air Passenger Departure Tax").
  Typical case: two separate one-way tickets through Hong Kong (on one through ticket a transfer is usually not taxed).
- Arrival by air ("Air (2 one-way air tickets)") or, since 1 Oct 2025, by land / sea through a control point ("Land /
  Sea"; not eligible if they left Hong Kong by land / sea on the departure day or the day before). SkyPier ferry /
  coach passengers claim at the SkyPier desk instead.
- Apply **within 28 calendar days of the departure flight**, and only after departing (the platform checks the
  departure record). Children under 12 pay no tax, so there is nothing to refund.
- The travel document must be the one used to enter Hong Kong; the names on the boarding passes and the receipt must
  match it.
Not eligible (stayed longer, more than 28 days ago, tax not on the receipt): the tool's result says why, with the rule.

## 2. How it runs (code)
Applying is ONE `apdt_refund` call — only when the user asks to apply (a question about the refund is answered from
this page, with no call). Arguments: what the user gave — `refund_method` (card, cash, alipay, wechat, alipayHK,
wechatHK), `wallet_account` for an e-wallet, `card` when they named a Vault card, `passport_file` (the passport photo
they attached, its `uploads/…` path) — and what the documents show: `in_flight` / `in_date` (the flight into Hong
Kong), `out_flight` / `out_date` (the flight out), `in_pass_file`, `out_pass_file`, `receipt_file` (their paths) and
`tax_hkd` (the departure tax on the receipt: 120 or 200, 0 when it shows none).
Code does everything else:
- the documents: with a mailbox connected and the documents not given, code runs the fixed mail search, saves the
  boarding passes and the receipt (`attachments/…`) and reads them, with the PDFs the user attached. The result
  `mail_read` lists the files: read the flights, the dates and the tax in them and call `apdt_refund` again with every
  field;
- the rule above (not eligible: the result says why, nothing is opened);
- the platform's pages, filled by their form functions (fixed ids), every upload in its box, the passport number and
  the card from the Vault (a card the Vault cannot fill is typed by the user in a takeover), stopping before the face
  recognition.

The result `needs_input` lists what only the user can give (the passport photo, a document, the refund method): ask
for exactly that, in one message, then call again with every field; `invalid`: fix the arguments and call again. Never
fill, tick, upload or click on the platform yourself, never call purchase_confirm here, and never start the face
recognition.

## 3. Answer
Word the reply from the tool's JSON result: its status (confirmed, not_eligible, stopped, not_confirmed,
handed_over), every fact in its `must_say` — the reference number exactly as the result gives it, the application's
link, the last day for the face recognition, the refund (card as "Visa ending 1111") — what was uploaded, and the face
recognition as the user's own step on their phone: open the link (or "Resume Application" at
https://app.apdtrefund.hk/hkTaxMobile/?lang=en with the travel document number and the reference), press "Proceed with
Facial Recognition" and follow the actions; approval and the refund usually follow within about three weeks. Never the
passport or card number; never "I completed the face check". Offer a reminder (schedule_create) a few days before the
deadline.

## Remember what you learn (for next time)
- **Profile fields** (name as in the passport, birthday, nationality, email): profile_suggest when missing.
- **Secrets** — passport and card numbers, and photos of them: never into memory, files or the answer. A passport photo
  the user uploads is used only for this upload; say it should not stay in the workspace afterwards.
