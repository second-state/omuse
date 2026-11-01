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

## 1. Who qualifies (check before opening the site)
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
Not eligible (stayed longer, more than 28 days ago, tax not on the receipt) → say why, with the rule, and stop.

## 2. Gather everything first (no browser yet; ask once for what is missing)
- **Flights and receipt from mail**: gmail_search `(boarding pass OR e-ticket OR receipt OR itinerary) (Hong Kong OR HKG) newer_than:40d`
  → gmail_get_message for each hit. Needed: the **arrival** flight into HKG (number, date) and its boarding pass, the
  **departure** flight out of HKG (number, date) and its boarding pass, and the **ticket receipt / itinerary of the
  departure ticket showing the departure tax paid**. Save every attachment you will upload with gmail_save_attachment
  (they land in `attachments/…`; code uploads them from there).
- **Files the user attached in the chat** (passport photo, boarding passes, receipt) are in the workspace already; use
  their paths. Drive (when connected): drive_search for a passport scan or a boarding pass, then drive_download.
- **Travel document**: the site needs a **photo / scan of the passport photo page** (or the ID used to enter Hong Kong).
  The Vault keeps only the number, not the picture: if no photo was attached or found, ask for it.
- **Identity** from profile_get: name as in the passport, date of birth, nationality, email. The name on the site is
  the one printed on the passport photo page (file_look the photo for the order: surname first, e.g. "LU LUCAS"). The passport number is
  only in the Vault (vault_list, kind id_document): code fills it from there; never ask for it in chat, never put it in
  an answer, memory or a file.
- **Refund method**: what the user asked for; otherwise ask (Credit / Debit Card — Visa, Mastercard, AmEx, about 3
  working days, fee up to HK$18; Alipay / WeChat Pay / AlipayHK / WeChat Pay HK with an account ID; Cash only at the
  airport's restricted area). For a card, it comes from the Vault; the user types it only when the Vault has
  no complete card.
- Missing anything (a boarding pass, the receipt, the passport photo, the refund method) → **one message** naming exactly
  what you need ("Please attach the photo page of your passport and tell me how you want the refund: card or e-wallet").
  Never open the form with a document missing.

## 3. Fill the application (code)
Code fills the application on the official site (the facts from step 2, the uploads, the Vault passport number and
card) and stops at "Submit your application — Proceed with Facial Recognition"; you do not fill, tick or upload
anything here. A CAPTCHA, a sign-in, an OTP or a card the Vault cannot fill is handed over by code. **The face
recognition is the user's**: never press "Proceed with Facial Recognition", never open the camera page, never request
a takeover for it — it needs the user's own face and phone camera, after their departure procedures at HKIA.

## 4. Answer
- **Reference number: XXXXXX** — exactly as the site shows it (quote it, never retype it from memory).
- What was filled and uploaded: the travel document (type only, never the number), arrival and departure flights with
  dates, the receipt showing HK$120 departure tax, the refund method (card as "Visa ending 1111").
- **Face recognition — your step**: open https://app.apdtrefund.hk/hkTaxMobile/?lang=en on your phone (the camera
  must be the phone's front camera) → "Resume Application" → enter your travel document number and this reference
  number → continue → "Proceed with Facial Recognition" and follow the actions (blink, open your mouth, turn your head).
  Do it within 28 days of the departure flight (say the last date). After it the site shows the application status and
  offers email notifications (their email);
  approval and the refund usually follow within about three weeks (card: about 3 working days after approval).
- Never the passport or card number; no "I completed the face check" — you did not.
- memory_remember "APDT refund applied <date>: <flights>, reference <code>, face recognition pending" (domain=travel),
  and offer a reminder (schedule_create) a few days before the 28-day deadline if the user has not done the face step.

## Remember what you learn (for next time)
- **Profile fields** (name as in the passport, birthday, nationality, email): profile_suggest when missing.
- **Secrets** — passport and card numbers, and photos of them: never into memory, files or the answer. A passport photo
  the user uploads is used only for this upload; say it should not stay in the workspace afterwards.
