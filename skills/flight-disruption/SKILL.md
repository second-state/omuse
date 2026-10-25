---
name: flight-disruption
description: 航班延误/取消/错过转机：查实时状态、改签和备选航班（优先旅行数据连接器）、航空公司和当地的赔偿与照顾规则、保险理赔要留的证明（延误证明、收据）、监控直到起飞并通知 Flight delays, cancellations and missed connections — live status, rebooking options (data connector first), what the airline and local rules give you, the proof an insurance claim needs, and a watch until departure.
---
# Flight disruption / 航班延误与取消

## 1. Find the flight (mail first)
gmail_search `(<flight no> OR <airline>) (e-ticket OR itinerary OR booking OR delayed OR cancelled OR schedule change)
newer_than:60d` and calendar_list_events for the day: booking reference, passenger names, route, times, fare class,
connecting flights, whether the ticket is one booking or separate tickets (matters for missed connections).

## 2. Live status (data first, browser second)
- A connected data tool wins: look for `mcp_<server>__flight_status` / `flights_search` and use it.
- Else browser_read (no clicking) the airline's own flight status page, or FlightAware / Flightradar24 for the flight
  number, and the airport's departures page. Quote the new time, gate, and the airline's wording ("delayed",
  "cancelled"); never estimate a new departure time yourself.
- If the user is at the airport, also check the inbound aircraft when the page shows it (a late inbound = a later
  delay than announced).

## 3. Options
- **Rebooking**: the airline rebooks for free on its own cancellations and long delays; show the alternatives it
  offers (its app / site, or the airport desk and the phone line — phone_call if configured, with the user's approval).
  To compare, use the data connector (`flights_search` same route, same day and next morning) and present_choices;
  book a new ticket only if the user asks (then the `flight-booking` skill, approvals as usual).
- **Separate tickets / self-transfer**: the next carrier owes nothing; say so plainly and look for the cheapest fix.
- **Care while waiting**: meals, hotel and transport rules differ by airline and country. There is no EU261-style
  law across Asia; rights come from the airline's conditions of carriage and some national rules (mainland China's
  CAAC rules on long delays, Japan's and Korea's consumer guidelines, Taiwan's CAA). browser_read the airline's
  "delays and cancellations" page and quote it with the date checked; keep every receipt. EU261 still applies to
  flights departing the EU / UK and to EU carriers flying in.
- **Refund vs voucher**: on a cancellation the user may take a refund to the original payment instead of a voucher
  — mention it when the airline page offers a voucher first.

## 4. Insurance (ask once, then help)
If a travel-insurance email exists (gmail_search `insurance (policy OR certificate OR travel)`), read the delay
benefit: usually a fixed amount per block of 6 or 8 hours, or expenses up to a cap, and it needs a **delay
certificate** from the airline (ask at the desk or via the airline's online form), boarding passes and receipts.
Write the checklist, draft the certificate request (gmail_create_draft to the airline) and, after the trip, the
claim email. If no policy was found, say that delays, missed connections and lost luggage are among the most common
claims and ask whether they are covered (credit cards often include delay cover) — one line, no selling.

## 5. Keep watching
When the flight is still hours away: watch_create on the status page (every 15–30 min, "change") or schedule_create
checks until the new departure, and notify_user on any change. Add the new time to the calendar event
(calendar_update_event, approved by the user) and tell the people affected only if the user asks (gmail_send /
slack_send_message go through approval).

## 6. Answer
Status line first (flight, old → new time, cause if published), then the options with times and prices, what the
airline owes (quoted, with source and date), the insurance checklist, and what you set up to keep watching. Say what
was not verifiable (e.g. the airline site is blocked for automated browsers → suggest the app and the phone number).

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
