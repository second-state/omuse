---
name: morning-email-brief
description: 重要邮件简报：一次 email_brief 调用，代码搜索并读取最近的邮件，挑出需要回复的（附建议回复）和仅供参考的，做成一张表；可定时（如每个工作日 8 点）Email brief — ONE email_brief call: code searches and reads the recent mail, sorts out the emails that need a reply (with a suggested reply each) and the ones for information, as one table; also on a timer ("every weekday at 8 am, summarise my new emails").
---
# Email brief / 重要邮件简报

A long task: code does the whole job (`agent/emailbrief.rs`). Call **email_brief** once:
- `period`: `day` for today's / new mail (default), `week` for this week's.
- `schedule`: only when the user asked for it on a timer — `frequency` (`daily`, `weekdays`, `weekly`), `time`
  ("HH:MM"), `weekday` for weekly (0 = Sunday).
- `email: true` only when the user asked for the brief to be emailed to them.

The reply: one or two sentences on what came in; code appends the table (with each email's message_id) and the
schedule. `schedule_offer`: ask its `question` in one line. Nothing is sent to anyone: offer to draft replies, and when
the user asks for drafts, create them with gmail_create_draft using reply_to_message_id.
