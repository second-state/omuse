---
name: michelin-dining
description: 米其林餐厅：用 travel_search(kind=michelin) 查米其林星级/必比登餐厅和可订时段，用 travel_book 订位——城市、日期（YYYY-MM-DD）、时间、人数、星级、菜系作为参数；回复按结果 JSON 写 Find MICHELIN-starred and Bib Gourmand restaurants and book a table with travel_search / travel_book (kind=michelin) — the user's details as JSON arguments; code checks real availability (the next candidate when one is full), confirms a deposit and hands the Confirm click to the user.
form: on
---
# MICHELIN dining / 米其林订位

Code does the work; you turn the user's words into the tool's arguments and the tool's JSON result into the reply.

## 1. Call the tool
- Find / compare → `travel_search` with `kind: "michelin"`; book → `travel_book` (plus `pick` when the user chose one).
- `city`, `date` (YYYY-MM-DD; resolve relative dates from today), `time` (HH:MM, 24-hour; dinner is 19:00 when the
  user gave no time), `party`, `distinction` ("3 stars", "2 stars", "1 star", "stars" for any starred,
  "bib gourmand"), `cuisine`, `best: true` for the highest distinction rather than the cheapest, `requests` = dietary
  needs and the occasion (they go in the booking's requests field), `contact`, `card`.
- Distinctions: a Bib Gourmand is NOT a star; "Michelin-starred" means 1–3 Stars.

## 2. What the tool answers, and the reply
- `needs_input` / `invalid` / `running` as in the flight skill.
- `shown`: the restaurants (distinction, cuisine, price per person in local currency), the pick, the free time found
  (`pick.time`). `nothing_found` with `checked`: no table at those restaurants — offer another date or time.
- `booked`: reference, restaurant, date (weekday), time, party, the deposit charged and its refund rule, seating limit,
  dress code. The facts in `show` are appended to the reply by code. No reference → not confirmed.
