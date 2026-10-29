---
name: fwd-travel-insurance
description: FWD 旅行保险：在新加坡或香港买 FWD 旅行保险（报价、比较方案、填表、付款交给用户），以及出事后理赔（新加坡在 FWD 网上表格提交；香港的旅行保险由 bolttech 承保，理赔表填好后发邮件到 claims.hk@bolttechinsurance.com）FWD travel insurance in Singapore and Hong Kong — buy a policy (quote, compare plans, fill the application, the user pays) and claim after a delay, lost bag, medical bill or cancellation (Singapore: FWD's online claim form; Hong Kong: plans underwritten by bolttech, the claim form filled and emailed to bolttech).
---
# FWD travel insurance (Singapore, Hong Kong) / FWD 旅行保险

Use this for "buy FWD travel insurance for my trip to Tokyo", "which FWD plan covers cancellation?", "claim my FWD
travel insurance for the delayed flight", "file a claim with FWD for the clinic bill in Bangkok". FWD has no API: buying
and claiming go through its own websites (and, in Hong Kong, bolttech's claims mailbox). The user signs in, pays and
signs; you prepare everything else.

## 0. Which market (decide first, say it in the answer)
- The request ("FWD Hong Kong", "my HK policy"), else the policy email (sender / domain `fwd.com.sg` vs `fwd.com.hk` /
  `bolttechinsurance.hk`), else the profile's home address country. Still unclear → ask once.
- **Singapore** — FWD Singapore Pte. Ltd. is the insurer. Site `www.fwd.com.sg`.
- **Hong Kong** — FWD's travel plans (**MyTravel**, **TravelCare Plus**, **GBA Easy Travel**) are **underwritten by
  bolttech Insurance (Hong Kong) Company Limited**; FWD Financial Limited runs the website as bolttech's appointed
  agency. Buying is on `www.fwd.com.hk`; **claims go to bolttech**, not to FWD (FWD's eServices app is for FWD's own
  life and accident policies). Say this once so the user knows who decides the claim.
- Neither market has a travel product for someone who lives elsewhere; never buy one market's plan for a resident of
  the other.

## 1. Buy a policy
Buy before departure; it never covers something that already happened.
1. **Trip facts** from the request, then mail (gmail_search the flight / hotel bookings) and the profile in ONE turn:
   destination(s), departure and return dates, travellers (names as in the passport, ages or dates of birth), single
   trip or annual. Ask once for anything still missing.
2. **Quote** (no login needed):
   - SG: `https://www.fwd.com.sg/travel-insurance/online-quote/`
   - HK: the plan page's "Get a quote" — `https://www.fwd.com.hk/online-insurance/mytravel-insurance/en/`,
     `…/travelcare-plus-insurance/en/`, `…/gba-easy-travel-insurance/en` (Greater Bay Area trips).
     Eligibility (read it on the page and check it): **HKID card holder, in Hong Kong when applying, the trip starts
     from Hong Kong**. Not met → say so and stop.
   browser_navigate, type the trip facts, read the plans the page returns.
3. **Compare** with present_choices(kind="comparison"): label = the plan name exactly as on the page, details = the
   premium (with currency), medical limit, trip cancellation, delay benefit, baggage, and anything the user asked about
   — exact strings from the page; source_url = the quote page. Point out what the trip needs (a cancellation-for-any-
   reason section, cover for an activity such as skiing, pre-existing conditions — FWD Singapore lists the conditions
   on `…/travel-insurance/pre-existing-medical-conditions/`). HK MyTravel: its "cancel for any reason" cover needs the
   policy **bought within 30 days of the first trip payment** and the cancellation made **before the original
   departure** — check the dates against the booking emails and say whether it still qualifies. Recommend one plan in
   the note; wait for the pick unless the user already chose ("the cheapest", "the one with cancellation").
4. **Apply** (the plan the user picked):
   - vault_list first. Type names, dates of birth, email and phone (profile_get). The NRIC / HKID or passport number
     only from the Vault with browser_fill_secret — never typed from chat, never in the answer.
   - Answer the health and declaration questions only from what the user said; anything you do not know → ask, never
     tick "no" for them.
   - purchase_confirm once: site = the domain, items = [{name: "<plan> travel insurance, <destination>, <dates>,
     <n> travellers", price: premium}], total, currency, card_item_id when a card is in the Vault.
   - Fill the card from the Vault after the approval (each browser_fill_secret in its own turn). No card in the Vault →
     say so in one line.
   - Then browser_request_takeover: "the application is filled in — check it and press Pay". Never press Pay unless the
     user told you to complete the purchase.
5. After the hand-back: read the confirmation (policy number), memory_remember (domain=trip: insurer, plan, policy
   number, period — no ID numbers), and add the emergency line to the answer.

## 2. Claim (after a delay, cancellation, lost bag, medical bill …)
**Gather in ONE turn**: gmail_search the policy email (`(FWD OR bolttech) (policy OR certificate) travel
newer_than:365d`), the incident (`(<airline> OR <flight no>) (delay OR delayed OR cancelled OR irregularity OR
baggage) newer_than:60d`), the receipts (`(receipt OR invoice OR clinic OR hospital) newer_than:60d`), and profile_get
(name, email, phone, address). Take from them: policy number, plan, period, the insured persons; what happened, when
and where; the amounts. A delay benefit counts **hours of delay** (scheduled vs. actual departure — state both
times). Amounts with `calculate` (e.g. claimed = paid − refunded by the airline).

Documents the insurer asks for (list what is in mail and what is missing): the policy; booking / e-ticket and boarding
passes; the airline's delay or cancellation letter (or Property Irregularity Report for a bag); police report for
theft; medical report and itemised receipts; refunds already received. gmail_save_attachment each one that is an
attachment, so it can be uploaded or attached.

Medical details stay in the claim documents — never in memory, the profile or the answer beyond what the form needs.

### 2a. Singapore (FWD's online claim form)
- **Tell FWD within 30 days** of the incident (loss date + 30 days: state the date). Overseas emergencies: **+65 6322
  2072**; in Singapore: **+65 6820 8888**.
- Online: browser_navigate `https://www.fwd.com.sg/claim-online/travel.html` (or the customer portal
  `https://www.fwd.com.sg/portal/` when the user prefers to sign in). Fill what is known: policy number, claimant,
  contact, the incident (type, date, place, what happened), the amount; the NRIC / passport only through
  browser_fill_secret; the documents with browser_upload (approval) from the saved attachments.
- **The sign-in and the final Submit are the user's**: when the form is filled, or when it asks for a login, Singpass or
  a one-time code → browser_request_takeover ("check the claim and press Submit"). Never press Submit yourself.
- Record the claim reference after the hand-back.

### 2b. Hong Kong (bolttech's claim form by email)
- The form: **bolttech's Travel Insurance Claim Form** (fillable PDF):
  `https://www.fwd.com.hk/files/v3/assets/blta9d684affff23c8c/bltf22c68114bd0cfb6/649e7bda1ea829102cf584d2/travel-insurance-clm-form-fillable.pdf`
  browser_navigate to it: the result's `pdf.path` is the saved copy under downloads/ (if `pdf` is missing, say so and
  ask the user to upload the form instead).
- pdf_form_fields(path) — the labels say what each field is (policy number, full name, address, email, date / time /
  place of incident, original and delayed schedule with flight numbers, hours delayed, how it happened, amount claimed,
  airline name and its reference). Fill with pdf_form_fill only from mail, the profile and what the user said.
  **Leave empty** (the user completes them): HKID card number, bank account details, signature and date of signing.
  Read `problems` and `required_still_empty` and fix field names.
- send_file the filled form with a short list of what you filled and what the user must add (HKID, bank account for a
  direct credit, signature).
- gmail_create_draft to **claims.hk@bolttechinsurance.com**: subject "Travel insurance claim — policy <number> —
  <what> <date>", body = policyholder, policy number, plan, what happened (one paragraph, no diagnosis), the amount
  claimed and the list of attachments; attachments = the filled form + the saved documents. A draft, not sent: the
  user signs the form, replaces it in the draft and sends it (or uploads the signed form to the chat and asks you to
  send — then gmail_send, which asks for approval).
- bolttech claims enquiries: **+852 3123 3344**. Emergency assistance: the 24-hour number on the policy schedule
  (quote it from the policy email). Time limit: quote the policy wording's claims section; if the wording is not in
  mail, say the limit could not be checked and that the claim should go in as soon as possible.

### 2c. Keep it moving (both markets)
- schedule_create: a weekly check of mail for news on the claim ("any email from FWD / bolttech about claim <ref or
  policy number>? notify me") until it is paid or closed, and for Singapore a reminder 5 days before the 30-day date
  if the claim is not submitted yet.
- memory_remember (domain=trip): insurer, policy number, claim reference, incident date, amount, deadlines — no ID or
  bank numbers.

## 3. Answer
- Buying: market and insurer (HK: "underwritten by bolttech, sold by FWD"), the plans compared with premiums, the
  recommendation and why, what was filled, what the user must still do (check and press Pay), the emergency number.
- Claiming: the first line says which channel and the deadline as a date (SG) or the time limit found (HK); then the
  amount and how it was computed, the documents (have / missing), what was filled, what the user must do (sign in and
  Submit; or add HKID, bank details and signature, then send the draft), the reminders set. The insurer decides; say so.

## Remember what you learn
- Policy number, plan, period, insurer and claim references: memory_remember (domain=trip).
- Profile facts (address, phone, the insurer as `custom:travel insurer`): profile_suggest — the user confirms.
- NRIC / HKID, passport, card and bank numbers, passwords and one-time codes: never in memory, files or the answer — the
  Vault holds the documents and cards; the bank account and signature stay with the user.
