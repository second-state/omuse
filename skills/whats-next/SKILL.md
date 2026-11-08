---
name: whats-next
description: 接下来的安排：一次 whats_next 调用，代码从所有已连接的邮箱和日历整理今天（及之后两天）的航班、酒店、会议和要去的地方，逐项调研（航班动态与备选、当地新闻、地图、见面的人、会议背景、入境卡、退税、晚上的活动和电影），做成一份 HTML 报告；定时运行时直接发到你的邮箱 What's next — ONE whats_next call: code makes today's plan (and the next two days) from every connected mailbox and calendar, researches it item by item (flight status and backups, local news, maps, the people you meet, the meeting's background, arrival cards, tax refunds, evening events and films) and lays it out as one HTML report; on a schedule it is emailed to you.
browser: off
---
# What's next / 接下来的安排

Use this for "what's next", "what's my day", "brief me on today", "prepare me for tomorrow's trip", and for a daily
morning briefing ("every morning at 7"). A long task: code does all of it (`agent/whatsnext.rs`).

1. Call **whats_next** (with `every_day_at` "HH:MM" only when the user asked for it every day).
2. Asked in the chat, the result first shows today's list and says `confirm`: ask its `question` in one line.
3. The user's reply — whatever it is, "nothing to add" too — calls **whats_next** again with `confirmed: true` and
   anything they added as `additions` (one item each: kind, title, date, start, end, place). Code puts the additions
   in the calendar, researches everything and makes the report.
4. The reply: a sentence or two on the day; code appends the report's link. `schedule_offer`: ask its `question`.

Never invent an item, a time or a person.
