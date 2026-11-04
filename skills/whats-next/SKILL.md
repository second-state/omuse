---
name: whats-next
description: 接下来的安排：从所有已连接的邮箱和日历整理今天（及之后两天）的航班、酒店、会议和要去的地方，逐项调研（航班动态与备选、当地新闻、地图、见面的人、会议背景、入境卡、退税、晚上的活动和电影），做成一份 HTML 报告；定时运行时直接发到你的邮箱 What's next — today's plan (and the next two days) from every connected mailbox and calendar, researched item by item (flight status and backups, local news, maps, the people you meet, the meeting's background, arrival cards, tax refunds, evening events and films), as one HTML report; on a schedule it is emailed to you.
browser: off
---
# What's next / 接下来的安排

Use this for "what's next", "what's my day", "brief me on today", "prepare me for tomorrow's trip", and for a daily
morning briefing ("every morning at 7"). Code does the work through three tools; you turn text into JSON and write
the words.

1. **whats_next** — call it first (with `every_day_at` "HH:MM" only when the user asked for it every day). Code reads
   every connected calendar (today to the day after tomorrow) and the booking and invitation emails.
2. **whats_next_items** — from the result's `emails` and `events`, give every item from today to the day after
   tomorrow, one each: flights (number, IATA airports, the arrival city and country, the departure country), hotel
   check-ins and check-outs, meetings (with every participant other than the user: name, email, organization),
   reservations, events, places to be. Times are local where it happens. A later email about the same booking wins;
   leave out cancelled ones. `location` is where the user is today.
   - Asked in the chat, code shows today's list and the result says `confirm`: ask the user its question in one line.
     Their reply: call whats_next_items again with the same items, `confirmed: true`, and anything they added as
     items with `source: user` (code puts those in the calendar). Nothing to add is an answer too.
   - On a schedule nothing is asked: code goes straight on.
3. **whats_next_publish** — from the researched result: the day's summary (two or three sentences), a short note per
   item by its number (what to know or do — from the emails, the earlier emails with the participants, the searches),
   each of today's flights' status as the status search shows it (`unknown` when it does not say), and a one-line
   public profile of each person from their search results (role and company only; nothing private, nothing
   guessed). Code lays out the report, links it, and mails it on a schedule.

Never invent an item, a time or a person: what is not in the mail or the calendar is not in the plan.
