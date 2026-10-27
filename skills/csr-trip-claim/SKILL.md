---
name: csr-trip-claim
description: 用 Chase Sapphire Reserve（或 Preferred）刷的机票/酒店/行程，因生病、受伤等原因没去成或中途中断：判断该报行程取消还是行程中断、20 天/90 天时限、先向航空公司要退款和书面说明、医生证明、在 chasecardbenefits.com 立案（登录和最终提交交给用户）、整理理赔材料包 PDF、设提醒 Trip cancellation / interruption claim on the Chase Sapphire Reserve (or Preferred) card — you fell sick or were hurt and missed a flight, hotel or tour paid with the card: which benefit applies, the 20-day and 90-day clocks, the airline refund and letter first, the physician statement, the claim started at chasecardbenefits.com (login and final submit are the user's), a claim packet PDF, and reminders until it is paid.
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

## 2. The clock (state the dates, do not just say "soon")
- **Notify the administrator within 20 days** of the cancellation or interruption (or as soon as reasonably possible).
  Loss date = the day the trip was cancelled / the departure that was missed. Compute: loss date + 20 days.
- **Documents within 90 days** of the loss date. Compute: loss date + 90 days. The administrator may extend when it
  was not reasonably possible; late is still better than never — if the 20 days are over, file today and say so.
- Verify the current terms in the Guide to Benefits (chase.com/cardbenefits, the user's card) before quoting: the
  figures below are as of October 2026 and change.

## 3. Gather (mail first, in ONE turn)
- gmail_search `(e-ticket OR itinerary OR booking OR confirmation) (<airline> OR <hotel> OR <route>) newer_than:180d` —
  booking reference, passenger names, dates, fare, fare rules ("non-refundable", change fee), how it was paid (Sapphire
  …4321 / Ultimate Rewards points), the booking date (pre-existing-condition look-back counts from it).
- gmail_search `(Chase OR Sapphire) (transaction OR charged OR statement OR receipt) <amount or merchant>` — the
  charge: amount, date, last 4 digits; for an award ticket the taxes and fees paid with the card.
- gmail_search `(<airline>) (refund OR credit OR voucher OR cancellation OR cancelled)` — what the airline already
  gave back or offered.
- gmail_search `insurance (policy OR certificate OR travel) newer_than:365d` — any other policy (it must be declared;
  the card pays what the others do not).
- calendar_list_events around the departure; profile_get (name, address, phone, email as the administrator wants them).
- Then the arithmetic with `calculate`: paid − refunded or credited = the amount to claim, per traveller; cap
  $10,000 per traveller, $20,000 per trip, $40,000 per 12 months.

## 4. The airline (or hotel / tour) first
The insurance pays only what the supplier does not refund. Before or alongside the claim:
- Ask the airline to cancel the ticket (if not yet done) and refund what is refundable (government taxes and fees on
  most tickets, the full fare on refundable fares) or issue a travel credit, and to send a **written statement of the
  non-refundable amount / its refund policy** (the administrator asks for it). Many airlines waive change fees for a
  documented illness — ask.
- Draft it: gmail_create_draft to the airline's refunds / customer relations address (booking ref, flight, date,
  passengers, "unable to travel for medical reasons", what is requested, no diagnosis, no card number). The user
  sends it (approval).
- Hotels and tours: same request to the property or operator; keep the written reply.

## 5. Medical proof (sickness or injury)
- The administrator's **physician statement**: completed and signed by the treating doctor — patient, dates seen,
  nature of the illness or injury (general terms), the date the patient became unable to travel and that they were
  medically unable to travel on the departure date, and whether the condition was treated in the 60 days before the
  booking (the pre-existing-condition look-back). A dated doctor's letter saying the same is usually accepted while the
  form is being completed. Plus the consultation receipt or discharge paper.
- Ask the user once: doctor / clinic, date of the visit, whether they already have a letter (upload it; file_look to
  read what it says, nothing more). Medical details stay in the claim documents — never in memory_remember, the
  profile, or the answer beyond "medically unable to travel".
- For a companion or family member: their physician statement and the relationship.

## 6. Start the claim (the administrator's portal or the phone)
- Online: browser_navigate to **chasecardbenefits.com** (Card Benefit Services' eClaims). Flow: register or sign in
  with the card → "File a claim" → the card → "Trip Cancellation" (or "Trip Interruption") → the questions (loss date,
  reason, travellers, suppliers, amounts) → a **claim number** → upload the documents (or email / fax them later with the
  claim number on every page). Fill what is known from step 3; the card number only through browser_fill_secret from
  the Vault (approval). **The sign-in and the final submission are the user's**: when the form is complete, or when
  the portal asks for a login, password, one-time code or the card's security code, browser_request_takeover with
  the reason ("review the claim and press Submit"; "sign in"). Never press Submit yourself.
- By phone, or when the portal blocks automation: Sapphire Reserve 1-800-350-1697, Sapphire Preferred 1-800-350-1362
  (US; collect from abroad at the number in the Guide to Benefits). The administrator asks a few questions and sends
  the claim form and the physician statement. phone_call only if configured and the user asks.
- Record the claim number (memory_remember, domain=trip, with the loss date and the two deadlines — no card number).

## 7. The claim packet (make_pdf, then send_file)
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

## 8. Keep it moving
- schedule_create: a reminder 7 days before the documents deadline, and a check every 10 days ("any email from Card
  Benefit Services / eclaimsline / Chase about claim <number>?" via gmail_search, notify_user on news) until the claim
  is paid or closed. Typical processing is several weeks after the documents are complete.
- When the administrator asks for more, help produce exactly that; when it denies, read the reason (wrong benefit,
  pre-existing condition, trip over 60 days, missing airline letter) and draft the appeal with the missing proof.

## 9. Answer
First line: the benefit that applies and the two dates (notify by …, documents by …). Then the amount to claim and how
it was computed, what the airline is being asked for, the documents the user must get (doctor, airline) with the
status of each, how the claim is being started (portal step reached / phone), the claim number if any, the packet,
and the reminders set. Plain about limits: the administrator decides; pre-existing conditions (treated in the 60 days
before booking), voluntary changes of plan, travel against medical advice and trips longer than 60 days are excluded.

## Facts to rely on (October 2026 — verify in the user's current Guide to Benefits)
- Limits: $10,000 per covered traveller, $20,000 per trip, $40,000 per 12 months per account; interruption adds up to
  $250 emergency ground transport. Same limits on Sapphire Preferred; different phone number.
- Covered: prepaid non-refundable fares, hotels, tours and other common-carrier travel paid wholly or partly with the
  card or with Ultimate Rewards from the account; newer Guides limit the payout to the amount actually charged to the
  card / redeemed in points.
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
