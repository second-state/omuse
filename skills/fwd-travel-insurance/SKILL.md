---
name: fwd-travel-insurance
description: FWD 旅行保险：在新加坡、香港或泰国买 FWD 旅行保险（报价、比较方案、填表、付款交给用户），以及出事后理赔（新加坡在 FWD 网上表格提交；香港的旅行保险由 bolttech 承保，理赔表填好后发邮件到 claims.hk@bolttechinsurance.com）FWD travel insurance in Singapore, Hong Kong and Thailand — buy a policy (quote, compare plans, fill the application, the user pays) and claim after a delay, lost bag, medical bill or cancellation (Singapore: FWD's online claim form; Hong Kong: plans underwritten by bolttech, the claim form filled and emailed to bolttech).
form: on
form_kind: claim
---
# FWD travel insurance (Singapore, Hong Kong, Thailand) / FWD 旅行保险

> **Ask first, buy or file on request.** A question about cover or a claim gets the eligibility, the requirements and
> the official link: call `insurance_buy` / `claim_file` with `file: false` (the default) — code answers and opens
> nothing (`action: compare` quotes the plans when the user wants to compare). Set `file: true` only when the user
> explicitly asks you to buy the cover or file the claim for them.

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
- **Thailand** — FWD's travel insurance is sold by **Dhipaya Insurance ("TIP Insure")** on
  `affinity.tipinsure.com`, for **residents of Thailand who start the trip in Thailand** (Dhipaya's conditions). Claims
  go to Dhipaya's call centre (1736).
- No market has a travel product for someone who lives elsewhere; never buy one market's plan for a resident of
  another.

## 1. Buy a policy: call `insurance_buy`
Buy before departure; it never covers something that already happened.
- Call **insurance_buy** with the trip from the request and the booking emails: `market` (sg / hk / th), `action` (`buy`, or
  `compare` when the user only asks which plan), `destination`, `depart`, `return`, `travellers`, the user's rule
  (`cover` + `cover_at_least`, e.g. cancellation 10000), `plan` when the user picked one from the comparison cards, and
  `card` when they named the Vault card to pay with. Trip missing and the mailbox connected: code reads the booking
  mail (result `mail_read`) — read those emails and call again with the trip. `needs_input`: ask once, then call again.
- **Singapore** — code fills FWD's quote (the form function: single trip, who, the region from the destination, the
  dates), reads the plans from the summary, picks one by the rule (the plan the user chose, else the cheapest with the
  cover asked for, else the cheapest), shows the comparison as cards, and for `buy` saves the quote with FWD (name,
  email, mobile from the profile, NRIC / FIN from the Vault) and gives FWD's own link to it (status `saved`): the user
  opens it on any device, types their mobile number, and continues there — FWD's eligibility statements (Singapore
  resident with an NRIC / FIN, never refused travel insurance, nothing withheld), the plan, the add-ons, the details
  and **Pay** are the user's; nothing is paid for them. Without a Singapore mobile in the profile the page is handed
  over instead (status `handed_over`).
- **Hong Kong** (MyTravel) — code prices the plans on FWD's page, picks by the rule, shows the cards, and for `buy`
  opens the plan's application and fills the insured's details (`title`, `family_name` + `given_names` split from the
  HKID name, `hkid_issue_date` of the new Smart HKID; the HKID number from the Vault): status `saved` with the
  application's link, which opens prefilled on any device. Eligibility: **HKID card holder, in Hong Kong when applying,
  the trip starts from Hong Kong**. "Cancel for any reason" cover needs the policy **bought within 30 days of the first
  trip payment** and the cancellation made **before the original departure**.
- **Thailand** (TIP Insure) — code quotes Dhipaya's packages (Backpack 1 … Backpack Go 3, THB), picks by the rule and
  shows the cards; for `buy` pass `title`, `family_name` + `given_names` (as on the passport) and the Thai home address
  in its parts from the profile (`address_house_no`, `address_road`, `address_subdistrict`, `address_district`,
  `address_province`, `address_postcode`; the passport number comes from the Vault). Code fills the application, then
  the user approves in the app (the package, the premium, Dhipaya's conditions); only then code accepts the conditions
  and opens the payment page: status `payment_link` with its link (2C2P — the user enters the card and pays, on any
  device). Declined: `not_submitted`, no order made.
- Point out what the trip needs (cancellation for any reason, cover for an activity such as skiing, pre-existing
  conditions — FWD Singapore lists them on `…/travel-insurance/pre-existing-medical-conditions/`).
- After a purchase: memory_remember (domain=trip: insurer, plan, policy number, period — no ID numbers).

## 2. Claim a flight delay: call `claim_file`
Call **claim_file** with `insurer: "fwd"` and the facts from the user's words, the policy email and the airline's delay
letter: `market`, `benefit: "flight_delay"`, `policy_no`, `plan`, `incident_date`, `flight_no`, `from`, `to`,
`airline`, `airline_ref`, `scheduled_departure` and `actual_departure` (HH:MM, from the airline's letter), `delay_reason`,
the policy's delay rule (`benefit_per_block`, `block_hours`, `benefit_cap`), `claim_within_days` when the policy states a
time limit, `emergency_phone`, and `certificate_message_id` (the airline's email with the PDF). Singapore also:
`leave_sg` / `return_sg` (the trip's dates), `destination` (the country) and the documents of this chat by path —
`itinerary_file`, `boarding_pass_file`, `airline_letter_file`, `other_file`. The **policy number is required**: from the
policy email or the profile, else ask the user. FWD also **requires the itinerary and the boarding pass** (its form does
not go on without them): from the mail or the chat, else ask; the airline's letter and other documents are optional. Facts missing and the mailbox connected: code reads the policy and
delay emails (result `mail_read`) — read them and call again with every fact. `needs_input`: ask once, then call again.
- Code counts the hours (actual − scheduled), the benefit by the policy's rule (per full block of hours, up to the
  cap) and the deadline, and:
  - **Singapore**: saves the airline's certificate and fills every step of FWD's online claim (the form function,
    `docs/sites/fwd-claim.md`: the claimant with the NRIC / FIN from the Vault, Travel Disruption → Flight Delay, the
    trip, the delayed flight, the documents, PayNow by the NRIC / FIN), then asks the user's approval in the app
    (`submit_confirm`: the facts and FWD's declaration) and submits it — no hand-over (the user's decision,
    2026-11-03). Declined: the form is closed unsubmitted (`not_submitted`). Submitted: the result has FWD's claim page
    and says FWD emails the claim link; a weekly mail check follows.
  - **Hong Kong**: fills **bolttech's Travel Insurance Claim Form** (the fillable PDF; HKID, bank account, signature and
    date left for the user), sends it to the chat and puts an email draft to **claims.hk@bolttechinsurance.com** with
    the form and the airline's letter attached — not sent. bolttech claims enquiries: **+852 3123 3344**.
- Other FWD claims (a lost bag, a medical bill, a cancellation): `benefit: "other"` — the result says where the claim
  goes (Singapore: FWD's online claim form; Hong Kong: bolttech's claim form by email); help the user gather the
  documents (the policy; booking / e-ticket and boarding passes; the airline's letter or Property Irregularity Report;
  police report for theft; medical report and itemised receipts; refunds received).
- Medical details stay in the claim documents — never in memory, the profile or the answer beyond what the form needs.
- memory_remember (domain=trip): insurer, policy number, claim reference, incident date, amount, deadlines — no ID or
  bank numbers.

## 3. Answer
Write it from the tool's JSON result, using its `hints` (code appends the facts in `show` to the reply).
- Buying: market and insurer (HK: "underwritten by bolttech, sold by FWD"), the plans compared with premiums, the plan
  chosen and the rule that chose it, what was filled, what the user must still do (check and press Pay), the emergency
  number.
- Claiming: the channel and the deadline as a date first; then the delay (scheduled and actual times), the amount and
  how it was computed, the documents, what was filled, what the user must do (Submit; or add HKID, bank details and
  signature, then send the draft), the reminders. The insurer decides; say so.

## Remember what you learn
- Policy number, plan, period, insurer and claim references: memory_remember (domain=trip).
- Profile facts (address, phone, the insurer as `custom:travel insurer`): profile_suggest — the user confirms.
- NRIC / HKID, passport, card and bank numbers, passwords and one-time codes: never in memory, files or the answer — the
  Vault holds the documents and cards; the bank account and signature stay with the user.
