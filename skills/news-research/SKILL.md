---
name: news-research
description: 深度新闻研究报告：就任何主题在浏览器里搜索和阅读最新新闻，写成报纸版式的 HTML 和 PDF 报告（放在 /artifacts/），并可按天/周定时生成、发到用户的 Gmail Deep news research on any subject — searches and reads the latest news in the browser, writes a newspaper-style report as HTML and PDF (published under /artifacts/), and delivers new editions on a timer (daily, weekly …), optionally to the user's Gmail. Use it for "research the news on X", "a report / briefing / newsletter on X", "send me a weekly news report on X"; for a quick "what's the latest on X" use news-search.
steps: 30
---
# News research report / 新闻研究报告

The user names a subject; you research its latest news **in the browser** and publish a newspaper-style report. Code
lays out the paper (the template in this skill), names the files and puts them under `/artifacts/`; you find, read and
write.

1. **Search in the browser, several angles.** 2–4 `browser_news_search(query, days)` calls in ONE turn: the subject
   itself plus its main angles (companies, places, policy, markets, people). `days` = the period — 1 for a daily
   report, 7 for weekly (the default), 30 for monthly. Never use web_search or news_search here: the browser's news
   search is the source.
2. **Read the articles.** `browser_read` the 4–8 most relevant, most recent articles (up to 4 URLs per call, calls in
   the same turn), preferring original reporting and official statements over aggregators. Skip anything older than
   the period. Use only what you read: never invent a fact, number, date or quote.
3. **Write the edition and publish it** with ONE `news_report` call:
   - `subject` as the user put it; `title` a paper's name ("The Battery Weekly"); `period` ("Week of 2 November 2026").
   - `stories`: 4–10, the most important first (it becomes the lead). Each has a `headline`, a 2–5 sentence `summary`
     in your own words, the `source` (publication), the article's `url` and its `date`.
   - `insights`: 3–6 points across the stories — trends, what changed, what to watch, what it means for the user.
4. **Delivery on a timer.** When the user asked for the report regularly ("daily", "weekly", "every Monday"), create it
   with `schedule_create`: kind `cron`, spec from their words (daily `0 8 * * *`, weekly `0 8 * * 1`, or the time they
   named), goal `News report on <subject>: research the latest news with the news-research skill and publish the
   newspaper report` — add ` and email it to me` when they asked to get it by email. A scheduled run follows the same
   steps; when its goal says to email it, send the edition with `gmail_send` to the user's own address (the first
   connected mailbox) with the PDF and HTML as `attachments` (the `pdf_path` / `html_path` from news_report).
5. **Answer** in a few lines: the edition's headline points, the two links (`html_url`, `pdf_url`) as given, and the
   schedule when one was made. If the user replies "email it" later, send the latest edition the same way (its link
   `/artifacts/…` is its path).
