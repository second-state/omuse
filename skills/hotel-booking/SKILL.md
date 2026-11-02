---
name: hotel-booking
description: 订酒店：用 travel_search(kind=hotel) 查酒店比价，用 travel_book 预订——城市、区域、入住退房日期（YYYY-MM-DD）、人数、星级、预算等作为参数，代码搜索一次、按规则挑选酒店和房型、打开预订页交给 browser-use 技能（确认由用户点）；回复按结果 JSON 写 Search and compare hotels with travel_search(kind=hotel) and book with travel_book — the user's details as JSON arguments; code picks the hotel and the room by rule, opens the booking page for the browser-use skill (the Confirm click is the user's); you word the reply from the JSON result.

form: on
---
# Hotel booking / 订酒店

Code does the work; you turn the user's words into the tool's arguments and the tool's JSON result into the reply.

## 1. Call the tool — once, with every detail as an argument
- **Search / compare** → `travel_search` with `kind: "hotel"`; **book** ("book it", a choice card's "I choose: …") →
  `travel_book` with the same arguments plus `pick` = the hotel the user chose. Never `hotels_search`, `hotel_rooms`,
  `present_choices`, `purchase_confirm` or the browser yourself for this.
- `city` (the city's name), `area` (the area or landmark: "Ginza", "near Shinjuku station"), `date` = check-in and
  `check_out` as YYYY-MM-DD (resolve relative dates and "3 nights" from today in the live state), `party` = guests.
- `min_stars`, `max_price` (per night), `min_rating`, `free_cancellation: true` when the user needs it,
  `any_terms: true` for the cheapest rate whatever its conditions (a non-refundable rate is fine). `named`: a hotel
  the user named. `contact`, `card`, `mail_results_to`, `use_connector`, `next` as for flights.

## 2. What the tool answers
- `needs_input` → ask for exactly the missing fields, then call again with every field; `invalid` → fix and call
  again; `running` → code works; the JSON result follows as `travel_search_result` / `travel_book_result`.

## 3. Write the reply from the result
- The facts in `show` are appended to the reply by code.
- `shown`: the hotels (area, rating, price per night and total, cancellation, each `booking_url`), the pick and its
  rule, the room picked for it (`pick.room`: the cheapest with free cancellation unless any terms were fine),
  the Kayak `links.compare_url` once. `source: "web"` → these are web pages with "from" prices, not quotes.
- Check-in / check-out times and cancellation deadlines are in the hotel's local time.
- `nothing_found` / `not_confirmed` / `browser_use` / `mailed`: as in the flight skill —
  a booking is confirmed only with its reference; pay-at-hotel means nothing is charged now.
