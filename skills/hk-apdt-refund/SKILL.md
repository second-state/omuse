---
name: hk-apdt-refund
description: 申请退还香港机场离境税（APDT）：核对资格和期限，在官方平台上传用户在对话中给的证件，申请随即保存并得到参考编号，用户在任何设备上用参考编号继续，选择退款方式、人脸识别和提交由用户完成 Hong Kong air passenger departure tax (APDT) refund: the rule and the deadline checked; code uploads the documents the user gave in the chat to the official platform, which saves the application under a reference number the user opens it with on any device; the refund method, the face recognition and Submit are the user's.
form: on
---
# Hong Kong departure tax refund (APDT) / 香港机场离境税退款

> **Ask first, apply on request.** A question about the refund gets who can claim, the deadline, the documents and the
> official link: call `apdt_refund` with `file: false` (the default) — code answers and opens nothing. Set `file: true`
> only when the user explicitly asks you to apply for them.

Official site only: https://app.apdtrefund.hk/hkTaxMobile/?lang=en (the Airport Authority's Refund Application
Platform; free — any other site, or one that charges, is a scam). Code checks the rule, goes through the platform's
notes and entry mode and uploads the documents the user gave in the chat (the travel document's photo page, the
boarding passes, the receipt); the recognised travel document saves the application under a **reference number**. The
user opens it on any device with that number and their travel document number; the refund method, the
**face-recognition step** and Submit are theirs — you never upload or click yourself, never ask to take over and never
click "Proceed with Facial Recognition".

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
wechatHK), `wallet_account` for an e-wallet, `entry_mode` (how they entered Hong Kong: `air`, `land_sea` through a land
or sea control point, `skypier` by the SkyPier ferry / coach or Park & Fly; by air also the arrival flight and its
boarding pass) — and the trip, from their words or the documents: `out_flight` / `out_date`
(the flight out), `in_flight` / `in_date` (the flight into Hong Kong — leave them out when the user did not give them:
they add the arrival on the platform), `tax_hkd` (the departure tax on the receipt: 120 or 200, 0 when it shows none)
and the documents of this chat (`passport_file`, `out_pass_file`, `in_pass_file`, `receipt_file`). Call with what you
have; the result `needs_input` lists what is still missing, the documents among it as optional — ask once for all of
it in one message, then call again with everything the user gave (what they skip, they upload later in the saved
application, and the result says so).
Code does everything else:
- the documents: with a mailbox connected and the trip not given, code runs the fixed mail search, saves the
  boarding passes and the receipt (`attachments/…`) and reads them, with the PDFs the user attached. The result
  `mail_read` lists the files: read the flights, the dates and the tax in them and call `apdt_refund` again with every
  field;
- the rule above (not eligible: the result says why, nothing is opened);
- the platform (its form function): the notices, the two statements, Start, New Application, entry by air, then the
  documents of this chat into their slots; the travel document recognised, the platform saves the application and
  code reads its reference number (status `saved`). Without a travel document it stops at the upload (status `ready`).

The result `needs_input` lists what only the user can give (the departure, the refund method, the documents as
optional): ask for exactly that, in one message, then call again with every field; `invalid`: fix the arguments and call again. Never
fill, tick, upload or click on the platform yourself, never call purchase_confirm here, and never start the face
recognition.

## 3. Answer
Word the reply from the tool's JSON result: its status (saved, ready, not_eligible, stopped); the facts in its `show` are
appended by code — the rule's verdict, the tax to be refunded, the last day to apply, the reference number, the steps
left and the platform's link. With `saved` the user opens the platform on any device, chooses "View Application /
Continue Application" with their travel document number and the reference number, adds what is missing (the arrival,
when it was not given), chooses the refund, does the face recognition and submits; approval and the refund usually
follow within about three weeks. Never the
passport or card number; never "I applied" or "I completed the face check". Offer a reminder (schedule_create) a few days before the
deadline.

## Remember what you learn (for next time)
- **Profile fields** (name as in the passport, birthday, nationality, email): profile_suggest when missing.
- **Secrets** — passport and card numbers, and photos of them: never into memory, files or the answer. A passport photo
  the user uploads is used only for this upload; say it should not stay in the workspace afterwards.
