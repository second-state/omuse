---
name: news-research
description: 深度新闻研究报告：用一次 news_gather 调用（代码搜索和读取最新新闻、写成报纸、生成 HTML 和 PDF、按天/周/月定时、发到用户的邮箱），也用于"监控比特币价格和新闻" Deep news research on any subject — ONE news_gather call with the whole request as JSON: code searches and reads the latest news, writes the edition, lays out a newspaper-style report as HTML and PDF under /artifacts/, makes the schedule (daily, weekly, monthly) and emails it to the user's own address when asked. Use it for "research the news on X", "a report / briefing / newsletter on X", "send me a weekly news report on X", "monitor the BTC price and news, send me an update every day" (with markets); for a quick "what's the latest on X" use news-search.
steps: 30
---
# News research report / 新闻研究报告

A long task: code does the whole job (`crates/omuse-runtime/src/agent/news.rs`). You turn the user's words into ONE
`news_gather` call and its JSON result into the reply.

## `news_gather` — the whole request as JSON
- `subject`: what the news is about (a company, topic, place or person), without "latest news", "report", "weekly".
- `window`: `day` for a daily report, `month` for a monthly one, else `week`.
- `markets`: when the user asks to monitor / track / watch a price with the news (a cryptocurrency, a stock, an index,
  a currency pair, gold), its Yahoo Finance symbols (`BTC-USD`, `AAPL`, `^GSPC`, `GC=F`).
- `schedule`: `daily` / `weekly` / `monthly` only when the user asked for regular editions ("every week" = weekly);
  `weekday` (0 = Sunday) and `hour` only when the user named them.
- `email: true` when the user asked for the edition to be sent to them ("send me", "email me").
- Ask the user only for the subject, and only when there is none (`needs_input`).

## The reply
- `published`: say what the edition covers and its lead in a sentence or two; code appends the links, the prices, the
  schedule and the mail. `schedule_offer`: ask its `question` in one line.
- `nothing_found`: say no recent article was found and that no edition was published. `error`: say why.

## "Email it" later in the chat
- `news_mail` with `files` = the edition's `/artifacts/news/…` PDF and HTML links from the earlier reply (and its
  `title`). Code mails them to the user's own connected address.
