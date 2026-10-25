---
name: hotel-requests
description: 酒店事务：提前入住/延迟退房、房型与特殊要求、改期或取消（看清免费取消期限）、前一天再确认、降价时免费取消再订；从邮件找预订，用邮件草稿或电话联系酒店 Hotel requests — early check-in / late check-out, room and special requests, date changes and cancellations (mind the free-cancellation deadline), reconfirmation the day before, rebooking when the price drops; bookings from mail, contact by email draft or phone.
---
# Hotel requests / 酒店事务

## 1. Find the booking
gmail_search `(hotel OR booking OR reservation OR confirmation) (<hotel> OR <city>) newer_than:90d` → confirmation
number, dates, room type, rate and **cancellation policy** (free until when, in which time zone — the hotel's local
time), prepaid or pay-at-hotel, booked direct or through an OTA (Agoda / Booking / Trip.com / Expedia: changes go
through the OTA, not the hotel). Hotel phone and email from the confirmation mail (not from a web search).

## 2. The request
- **Early check-in / late check-out / high floor / twin beds / crib / adjoining rooms / allergy / anniversary**:
  one polite email (gmail_create_draft to the hotel, with the confirmation number, dates, guest name, the ask, and a
  fallback such as "or luggage storage if the room isn't ready"), 1–3 days before arrival; the user sends it (approval).
  Or phone_call when configured — hotels answer requests fastest by phone; the call needs the user's approval.
  Loyalty status (memory_search "Marriott", "Hilton", "IHG", "Hyatt", "Accor") earns these for free — mention the
  number in the email, never the password.
- **Change dates / rooms / guests**: through the OTA's "manage booking" (browser_navigate to the link in the mail;
  clicks that change or cancel a booking are approved one by one — say what you are about to change: booking number,
  hotel, old → new) or by email for direct bookings. Compare the new rate first; a cancel-and-rebook is sometimes
  cheaper than a change.
- **Cancel**: check the deadline in the hotel's time zone and the penalty; cancel through the same channel as the
  booking; keep the cancellation number; the refund timeline (prepaid OTA: 7–14 days to the card). Never cancel
  anything the user did not name explicitly.
- **Reprice**: for free-cancellation bookings, a data connector (`hotel_rooms` for the same hotel and dates) or a
  browser_read of the same OTA search can show a lower rate — if lower by a margin that matters, offer "cancel and
  rebook" with both numbers; do it only when the user says yes, rebook first, cancel second.
- **Reconfirm**: the day before arrival (especially late arrivals after 18:00, which some hotels release), a short
  email or call: "arriving ~23:00, please hold the room"; add the hotel address in the local script to the calendar
  event for the taxi (calendar_update_event).

## 3. Problems at the hotel
Overbooked ("walked") → the hotel must find comparable accommodation and transport; no room ready at check-in → luggage
storage, a time, or an upgrade; noise, cleanliness, safety → ask for a room change in writing to the front desk;
unexpected charges on the folio → ask for an itemised bill (keep it for `travel-expense-report`); card holds that
don't release → the bank, with the folio. Draft the message in the hotel's language and English.

## 4. Answer
The booking found (confirmation number, dates, policy with its deadline and time zone), what you drafted or changed,
what still needs the user (send, approve, call), and the reminders set. Never invent policy terms — quote the mail.

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
