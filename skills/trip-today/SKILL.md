---
name: trip-today
description: 行程助手：从邮件和日历整理今天/本次旅行的预订（航班、酒店、餐厅、门票、会议），按时间排好，找出缺口和冲突（没订酒店的晚上、城市之间没有交通、落地赶不上会议），提醒改签和取消期限 What's on today / this trip — bookings from mail and calendar in one timeline, gaps and conflicts (a night without a hotel, no transport between cities, a meeting before the flight lands), reconfirmations and deadlines.
---
# Trip today / 行程助手

Use this for "what's my plan today", "what have I booked for Tokyo", "do I have a hotel every night", "is there a gap in
my trip", "remind me what time my flight is". Everything comes from the user's own mail and calendar; no browsing is
needed unless a confirmation page or a live status must be checked.

## 1. Collect (mail + calendar, in ONE turn)
Run these together — they are independent (same-turn calls run at the same time):
- gmail_search for each kind, over all mailboxes, for the trip window (default: today ± 30 days; "this trip" = from the
  first outbound flight to the return): `(flight OR e-ticket OR itinerary OR boarding) newer_than:60d`,
  `(hotel OR booking OR reservation OR check-in) newer_than:60d`, `(train OR rail OR shinkansen OR KTX OR THSR OR ferry)`,
  `(restaurant OR table OR 订位 OR 予約)`, `(ticket OR admission OR tour OR Klook OR KKday OR GetYourGuide)`,
  `(visa OR arrival card OR SGAC OR TDAC OR MDAC)`, plus cancellations: `(cancelled OR canceled OR delayed OR rescheduled OR schedule change)`.
  Use max_results 30 and the mailbox's own language too (日本語 / 中文 subjects).
- calendar_list_events for the same window (if the calendar is connected).
- memory_search "trip", "hotel", "airline", "seat", "loyalty" for preferences and known bookings.
Read the full message (gmail_get_message) only for hits that look like confirmations; the metadata is enough to
discard newsletters. A later email about the same booking wins (change, cancellation, new time).

## 2. Build the timeline
One row per item: local date & time (in the place where it happens — flights in the airport's time zone, with the
time zone named), what, reference/booking number, place or route, status (confirmed / changed / cancelled / unpaid),
source (mail subject + date). Sort by time. Mark items that also exist in the calendar, and those that exist only in
mail (offer to add them: calendar_create_event, after the user says yes).

## 3. Check for gaps and conflicts (say them first)
- **Nights without a bed**: every night between arrival and departure needs a hotel / home / overnight transport.
- **Movement without transport**: a hotel in a different city from the previous day's events and no flight / train.
- **Timing**: the flight lands (plus 60–90 min for immigration and bags, 120 at a first entry with a visa) before a
  meeting, a train or a check-in cutoff; connections under the airport's minimum; check-out time vs the next departure.
- **Overlaps**: two things at the same time; a dinner reservation in another city.
- **Status**: cancelled, "payment required", "confirm within 24 h", schedule-change emails, unreplied booking requests.
- **Deadlines**: free-cancellation cutoffs (hotels), check-in windows (airlines, usually 24–48 h before), arrival
  cards that must be filed within 3 days of arrival (Singapore SGAC, Thailand TDAC, Korea e-Arrival) → the
  `entry-requirements` / `sg-arrival-card` skills.
- **Insurance reminder** (once per trip, one line): if the trip has flights and checked bags and no travel-insurance
  email was found, say that delays and lost luggage are among the most common claims and ask whether the user has a
  policy (don't sell anything).

## 4. Answer
- "Today" first: a short list in time order, then "Next" (tomorrow / next leg), then "Problems found" (or "no gaps").
- Every time with its time zone; every booking with its reference; nothing that is not in a mail or the calendar —
  write "not found" instead of assuming (no hotel found ≠ no hotel booked: say which search was run).
- Offer the follow-ups a tool can do: add to calendar, draft a reconfirmation email to the hotel / restaurant
  (gmail_create_draft), set a reminder (schedule_create) for check-in opening or a cancellation deadline.
- Never repeat passport numbers or card numbers in the answer.

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
