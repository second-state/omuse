---
name: hk-apdt-refund
description: 申请退还香港机场离境税（APDT）：核对资格和期限，在官方平台走到上传证件那一步，上传证件照片、选择退款方式和人脸识别由用户在自己手机上完成 Hong Kong air passenger departure tax (APDT) refund: the rule and the deadline checked, the official platform opened up to its document upload; the documents' photos, the refund method and the face recognition are the user's, on their own phone.
form: on
---
# Hong Kong departure tax refund (APDT) / 香港机场离境税退款

Official site only: https://app.apdtrefund.hk/hkTaxMobile/?lang=en (the Airport Authority's Refund Application
Platform; free — any other site, or one that charges, is a scam). Code checks the rule and goes through the platform's
notes and entry mode to its document upload; the documents' photos (the travel document, the boarding passes, the
receipt), the refund method and the **face-recognition step** are the user's, on their own phone — you never upload,
never ask to take over and never click "Proceed with Facial Recognition".

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
wechatHK), `wallet_account` for an e-wallet — and the trip, from their words or the documents: `in_flight` / `in_date`
(the flight into Hong Kong), `out_flight` / `out_date` (the flight out) and `tax_hkd` (the departure tax on the receipt:
120 or 200, 0 when it shows none). Never ask for the documents before calling: no document is needed for the call — the
user uploads them on the platform, on their phone.
Code does everything else:
- the documents: with a mailbox connected and the trip not given, code runs the fixed mail search, saves the
  boarding passes and the receipt (`attachments/…`) and reads them, with the PDFs the user attached. The result
  `mail_read` lists the files: read the flights, the dates and the tax in them and call `apdt_refund` again with every
  field;
- the rule above (not eligible: the result says why, nothing is opened);
- the platform (its form function): the anti-scam notice, the two statements, Start, New Application, entry by air and
  Confirm — then it stops at the travel document's upload.

The result `needs_input` lists what only the user can give (the trip, the refund method): ask
for exactly that, in one message, then call again with every field; `invalid`: fix the arguments and call again. Never
fill, tick, upload or click on the platform yourself, never call purchase_confirm here, and never start the face
recognition.

## 3. Answer
Word the reply from the tool's JSON result: its status (ready, not_eligible, stopped); the facts in its `show` are
appended by code — the rule's verdict, the tax to be refunded, the last day to apply, the steps left and the platform's
link. The user opens the link on their phone, uploads the travel document, the boarding passes and the receipt, chooses
the refund and does the face recognition; approval and the refund usually follow within about three weeks. Never the
passport or card number; never "I applied" or "I completed the face check". Offer a reminder (schedule_create) a few days before the
deadline.

## Remember what you learn (for next time)
- **Profile fields** (name as in the passport, birthday, nationality, email): profile_suggest when missing.
- **Secrets** — passport and card numbers, and photos of them: never into memory, files or the answer. A passport photo
  the user uploads is used only for this upload; say it should not stay in the workspace afterwards.
