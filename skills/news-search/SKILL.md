---
name: news-search
description: 新闻与时事：最近几天发生了什么、某公司/人物/事件的最新消息、公告和结果，带日期和来源 News and current events — what happened in the last days, the latest on a company, person or event, announcements and results — with dates and sources.
---
# News search / 新闻搜索

**No plan for the search.** One `news_search` call (several in the same turn when the question has distinct topics),
then the answer. Never call plan_task to search news.

1. **One call.** `news_search(query, days)`: `query` = the subject in a few words ("Olares OS release", "Singapore
   GST budget 2027", "SQ 638 incident"); `days` = how far back — 1 for "today", 7 for "this week" (the default), 30
   for "this month". It answers from Tavily's news search (dated articles with snippets) when its key is set, else from
   web search; the result's `news_fallback` says when it was web search — then check each item's date yourself.
2. **Read only when the snippets are not enough.** browser_read the 1–3 most relevant articles in ONE call, preferring
   the original source (the company's announcement, the official statement, a major outlet) over aggregators.
3. **Answer.**
   - Lead with what happened, newest first, each item with its **date** (as published, in the user's language) and its
     **source** (outlet name, linked).
   - Keep to the period asked; drop anything older and say when nothing in the period was found.
   - Separate fact from opinion and from rumour ("reported by …", "unconfirmed").
   - Never invent a date, number or quote that is not in what you read.
