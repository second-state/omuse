---
name: csr-trip-claim
description: 用 Chase Sapphire Reserve（或 Preferred）刷的机票/酒店/行程，因生病、受伤等原因没去成或中途中断：判断该报行程取消还是行程中断、20 天/90 天时限、先向航空公司要退款和书面说明、医生证明、在理赔管理方的 eClaimsLine（www.eclaimsline.com）立案（登录和最终提交交给用户）、整理理赔材料包 PDF、设提醒 Trip cancellation / interruption claim on the Chase Sapphire Reserve (or Preferred) card — you fell sick or were hurt and missed a flight, hotel or tour paid with the card: which benefit applies, the 20-day and 90-day clocks, the airline refund and letter first, the physician statement, the claim started on the administrator's eClaimsLine portal, www.eclaimsline.com (login and final submit are the user's), a claim packet PDF, and reminders until it is paid.

plan: on
form: on
form_kind: claim
---
# Chase Sapphire trip cancellation / interruption claim / Sapphire 卡行程保险理赔

Use this for "I got sick and missed my flight, get my money back from the Sapphire Reserve insurance", "claim the trip
insurance on my Chase card", "we had to cut the trip short, the CSR covers that?". The card's **Trip Cancellation /
Trip Interruption Insurance** reimburses prepaid, non-refundable travel the user paid for (even partly) with the card
or with Ultimate Rewards points, when a covered reason — sickness or injury of the cardholder, a travelling companion
or an immediate family member, among others — stopped the trip. The claim is handled by Chase's benefit administrator
(Card Benefit Services), not by Chase itself: the card number identifies the policy, the administrator decides.

## 1. Which benefit (get this right — wrong benefit is the common denial)
- **Trip Cancellation**: the trip had not started (the user never left home; the missed outbound flight is the
  classic case). Pays the prepaid non-refundable fares, hotels and tours that were lost.
- **Trip Interruption**: the trip had started (on the way to the airport or already away) and was cut short or
  changed. Pays the unused prepaid part plus the extra cost of getting home (and up to $250 emergency ground transport).
- **Not this skill**: a flight delayed 6+ hours (Trip Delay, up to $500 a ticket, different form) → `flight-disruption`;
  bags → `baggage-delayed-lost`.
Say which one applies in the first line of the answer and file only that one.

## 2. File it: call `claim_file` (code does the steps)
Call **claim_file** with `insurer: "chase_sapphire"` and every fact you can read in the user's words and their emails:
`benefit` (step 1), `incident_date` (the loss date: the day the trip was cancelled / the departure that was missed /
the day it was cut short), `reason` and `who` in general terms (never the diagnosis), `paid`, `refunded`,
`non_refundable` (what a supplier's letter states as not refunded), `change_fee` (interruption: the fee to fly home
early), `bookings` (supplier and booking reference), `trip`, `card_product`, `card_last4` (from the charge email — never
a full number), `doctor_letter`, `supplier_letter`, `travellers`.
- Facts missing and the mailbox connected: code reads the booking, charge, refund and insurance emails with fixed
  searches (result `mail_read`, the emails above it) — read them and call claim_file again with every fact.
- `needs_input`: ask the user once for the missing facts, then call again with every field. `invalid`: fix and call again.
- Code then computes the amount (paid − refunded, or the supplier's non-refundable figure; plus the change fee for an
  interruption; within the caps) and the deadlines (loss date + 20 days to notify, + 90 days for the documents), fills
  eClaimsLine's claim form (the form function: name, contact, card, benefit, date of loss, amount, reason, booking),
  hands the page over for the user to review and press **Submit claim** (never you), reads the claim number from the
  confirmation page, and sets the reminders (7 days before the documents deadline, a mail check every 10 days).
- Its JSON result has the status (`handed_over`: filled, waiting for the user's Submit; `submitted`: the claim number),
  the amount and its arithmetic, the dates, the documents and `must_say`. Never say the claim was submitted unless the
  result has a claim number.
- By phone, when the portal cannot be reached: Sapphire Reserve 1-800-350-1697, Sapphire Preferred 1-800-350-1362
  (US; collect from abroad at the number in the Guide to Benefits).

## 3. The airline (or hotel / tour) first
The insurance pays only what the supplier does not refund. Alongside the claim, draft (gmail_create_draft) the request
to the airline's refunds / customer relations address: booking ref, flight, date, passengers, "unable to travel for
medical reasons", a refund of what is refundable (taxes and fees at least) and a **written statement of the
non-refundable amount** — no diagnosis, no card number. The user sends it (approval). Hotels and tours: the same.

## 4. Medical proof (sickness or injury)
The administrator's **physician statement**, completed and signed by the treating doctor (dates seen, unable to travel
on the departure date, whether the condition was treated in the 60 days before the booking). A dated doctor's letter
saying the same is usually accepted meanwhile. Medical details stay in the claim documents — never in memory_remember,
the profile, or the answer beyond "medically unable to travel". A companion or family member: their statement and the
relationship.

## 5. The claim packet (make_pdf, then send_file)
One PDF under reports/ ("CSR-claim-<airline>-<date>.pdf") with:
1. **Summary**: benefit (cancellation / interruption), travellers, trip (route, dates, booking refs), loss date,
   notify-by and documents-by dates, claim number if any, administrator contacts.
2. **Amounts**: a table per booking — paid (date, card …last 4 / points), refunded or credited, claimed; the total.
3. **Documents checklist** with status (have / asked / missing): claim form; physician statement; itinerary /
   e-ticket; card statement or transaction record showing the charge; the airline's refund / credit confirmation or
   its written refund policy and non-refundable statement; unused tickets or vouchers; receipts for extra costs
   (interruption); other insurance declared.
4. **Letters**: the airline refund request (and the hotel's) as drafted.
5. **Timeline**: what happened when, and what is still to do, with owners (user / OMuse).
Send it to the chat; the user attaches it when uploading.

## 6. Keep it moving
The reminders are set by code (in the result). When the administrator asks for more, help produce exactly that; when
it denies, read the reason (wrong benefit, pre-existing condition, trip over 60 days, missing airline letter) and draft
the appeal with the missing proof.

## 7. Answer
Write it from claim_file's JSON result, with every fact in its `must_say`: the benefit that applies and the two dates
first, then the amount and how it was computed, the status (filled and waiting for the user's Submit, or the claim
number), the documents the user must get (doctor, airline) with the status of each, the airline letter drafted, the
packet and the reminders. Plain about limits: the administrator decides; pre-existing conditions, voluntary changes of
plan, travel against medical advice and trips longer than 60 days are excluded.

## Facts to rely on (October 2026 — verify in the user's current Guide to Benefits)
- Limits: $10,000 per covered traveller, $20,000 per trip, $40,000 per 12 months per account; interruption adds up to
  $250 emergency ground transport. Same limits on Sapphire Preferred; different phone number.
- Covered: prepaid non-refundable fares, hotels, tours and other common-carrier travel paid wholly or partly with the
  card or with Ultimate Rewards from the account; newer Guides limit the payout to the amount actually charged to the
  card / redeemed in points.
- Trip Interruption also covers the change fee or fare difference paid to return home early (the airline's charge to
  move the flight): add it to the claimed amount with the unrefunded prepaid nights and fares.
- Covered reasons include sickness, injury or death of the cardholder, a travelling companion or an immediate family
  member; physician-imposed quarantine; severe weather; jury duty or a subpoena; a terrorist action or hijacking.
- Excluded: pre-existing conditions (advice, diagnosis or treatment in the 60 days before the booking), a change of
  plans, travel against a physician's advice, trips for medical treatment, trips longer than 60 days, the supplier's
  own cancellation (unless weather) or insolvency, war.
- Offsets: anything refunded or credited by the supplier, and other insurance, come off first.

## Remember what you learn (for next time)
Whatever the user tells you here is worth keeping, so they are never asked twice — but each kind has its place:
- **Profile fields** (name as on the card and the ticket, address, phone, emails, the card's nickname): profile_suggest
  (field, value) — standard fields or `custom:<label>` (e.g. `custom:travel insurer`); the user confirms it on the
  Memory page.
- **Preferences and facts** ("books flights on the Sapphire Reserve", "claim 2026-10 with Card Benefit Services,
  number …"): memory_remember with its domain (trip / account).
- **Secrets** — the card number, security code, passwords and one-time codes: never into memory, files or the answer;
  the card goes into the Vault (Memory → Profile → Vault) and browser_fill_secret uses it from then on. Medical
  letters: in the claim documents only.
