---
name: news-research
description: 深度新闻研究报告：就任何主题在浏览器里搜索和阅读最新新闻，写成报纸版式的 HTML 和 PDF 报告（放在 /artifacts/），并可按天/周定时生成、发到用户的 Gmail Deep news research on any subject — searches and reads the latest news in the browser, writes a newspaper-style report as HTML and PDF (published under /artifacts/), and delivers new editions on a timer (daily, weekly …), optionally to the user's Gmail. Use it for "research the news on X", "a report / briefing / newsletter on X", "send me a weekly news report on X"; for a quick "what's the latest on X" use news-search.
steps: 30
---
# News research report / 新闻研究报告

Code runs this skill from start to end (`crates/omuse-runtime/src/agent/news.rs`): once the skill is loaded for a request
for an edition, no step is the model's to choose.

1. **The subject** is read from the user's words ("news on X", "a report about X"); one extraction step only when the
   words do not say it. The period comes from the words too: 1 day for a daily report, 30 for monthly, else 7.
2. **One browser news search** (`browser_news_search`) of the subject; a second, wider one only when the first finds no
   article. The search results are the edition's sources.
3. **The articles**: the first six distinct links of the results, read with `browser_read` in one turn.
4. **The edition's text** is the only part the model writes: it gets the articles as JSON and returns JSON (a title,
   one story per article by its number — headline and a summary that keeps the article's facts — and insights). Code
   adds each story's link, publication and date, keeps every article read (an article left out or not reported gets
   its own opening), and publishes with `news_report`: the newspaper (masthead, lead, stories, sources) as HTML and PDF
   under `/artifacts/`.
5. **Delivery**: a request for regular editions ("every week") gets its schedule from code after the edition; a
   scheduled run whose goal says to email it is emailed by code to the user's own address with the PDF and HTML; "email
   it" in the report's chat sends the latest edition. The answer (the lead, the other headlines, the two links, the
   schedule, the mail) is written by code.
