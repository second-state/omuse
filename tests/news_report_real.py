"""News research reports with a real model, from the user's point of view (tests/user_common.py). The subject is made
up — Lumora, a tidal-power company (tests/fake_apps.py NEWS: six articles dated within the last week, found only
through the browser's news search: the fake DuckDuckGo answers them for date-filtered queries) — so every fact in a
report comes from the pages read. The user's mailbox is connected; its outgoing mail lands in a local SMTP sink.

1. report — in Main: "Research the latest news on Lumora tidal power and make me a newspaper-style report. Send me a
   new edition every week." Checked: the answer links the HTML and the PDF under /artifacts/; both open for the user;
   the HTML is the newspaper (masthead, the lead, the stories with their sources) with facts from the articles
   (40 MW, €120m, Kestrel, Ofgem …); the PDF is a PDF; the searches were the browser's (date-filtered queries reached
   the engine); the answer offers to email the edition; the Tasks tab lists a weekly task about Lumora.
2. email it — "email it" back in the report's chat (a question first is fine: the user says which report and where):
   the user's own address receives the edition with the PDF attached.
3. scheduled edition — the user presses Run now on the task (the full Automations page): a new edition is published
   in the task's chat with its links, /artifacts/ lists both editions, and the edition is emailed to the user (the
   schedule asks for it: "send me") or the answer offers to email it.
One TIMING line per request.

    OMUSE_E2E_ENV_FILE=~/.config/omuse/e2e.env tests/run_in_docker.sh news_report_real"""
import os
import re
import sys

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from tests import conn_common as C  # noqa: E402
from tests import skills_common as K  # noqa: E402
from tests import shop_common as SC  # noqa: E402
from tests import user_common as U  # noqa: E402
from tests.user_common import check  # noqa: E402

FACTS = [r"40\s?MW", r"€\s?120|120\s?million", r"Kestrel", r"Ofgem|£\s?310", r"Pentland|Orkney", r"Paimpol|Brittany"]
SMTP = SC.SchoolMail()
SMTP.start()


async def report_links(u: U.User) -> list[str]:
    """Every link of the last answer, as the browser resolves it (the report's links are /artifacts/… on this host)."""
    return await u.page.locator(".msg.assistant:not(.live) .bubble:not(.filecard)").last.evaluate(
        "b => [...b.querySelectorAll('a[href]')].map(x => x.href)")


async def opens(u: U.User, url: str) -> tuple[int, str, bytes]:
    r = await u.ctx.request.get(url)
    return r.status, r.headers.get("content-type", ""), await r.body()


async def main():
    async with U.User() as u:
        # ---- 1. the report
        hits0 = C.c.get("http://127.0.0.1:8094/ddg/_hits").json()["n"]
        a = await u.ask("Research the latest news on Lumora tidal power and make me a newspaper-style report. "
                        "Send me a new edition every week.", label="report", timeout=1500)
        links = await report_links(u)
        # the chat the report is in (Main's latest chat): the user comes back to it
        report_chat = C.c.get(U.B + "/api/conversations", params={"kind": "chat", "bot": "main"}).json()["conversations"][0]["id"]
        print(f"INFO report links: {links}")
        check("report: the answer came (no time-out)", bool(a.text.strip()) and not a.timed_out, a.text[:300])
        # the editions' links are in the block code appends after the answer (gate 40: the model's prose left them out)
        html = next((x for x in a.block_links if "/artifacts/" in x and x.endswith(".html")), "")
        pdf = next((x for x in a.block_links if "/artifacts/" in x and x.endswith(".pdf")), "")
        check("report: the block code appends after the answer links the HTML and the PDF under /artifacts/",
              bool(html and pdf) and "HTML edition" in a.block and "PDF edition" in a.block, (a.block, a.block_links))
        check("report: the block code appends after the answer names the next weekly edition",
              a.shows("New edition: every"), a.block)
        if html:
            st, ct, body = await opens(u, html)
            page = body.decode("utf-8", "replace")
            text = re.sub(r"<[^>]+>", " ", page)
            check("report: the HTML opens for the user", st == 200 and ct.startswith("text/html"), (st, ct))
            check("report: it is the newspaper (masthead, lead, stories, sources)",
                  'class="masthead"' in page and "<h2>" in page and 'class="story"' in page and "Sources" in page, page[:300])
            got = [f for f in FACTS if re.search(f, text)]
            check("report: facts from the articles read (at least four of 40 MW, €120m, Kestrel, Ofgem, Orkney, Brittany)",
                  len(got) >= 4, got)
            check("report: the stories link their articles", text.count("Read the article") >= 3 and "shop.test:8094/news/" in page,
                  text.count("Read the article"))
        if pdf:
            st, ct, body = await opens(u, pdf)
            check("report: the PDF opens for the user, and is a PDF", st == 200 and body[:5] == b"%PDF-", (st, ct, body[:8]))
        hits = C.c.get("http://127.0.0.1:8094/ddg/_hits").json()
        check("report: the news was searched in the browser (date-filtered queries reached the engine)",
              hits["n"] > hits0 and any("[df=" in q for q in hits["queries"]), hits)
        tasks = await C.page_text(u, "timers")
        check("report: the Tasks tab lists a weekly task about Lumora",
              re.search(r"Lumora", tasks, re.I) and re.search(r"Next run", tasks), tasks[:600])

        # ---- 2. "email it", back in the report's chat
        sent0 = len(SMTP.received)
        await u.page.goto(U.APP + "#chat/" + report_chat)
        await u.page.wait_for_selector("#chatInput")
        await u.page.wait_for_timeout(1500)
        a2 = await u.ask("email it", label="email it", new_chat=False, timeout=600)
        mine = lambda: [m for m in SMTP.received[sent0:] if any(K.USER in t for t in m["to"])]   # noqa: E731
        got = C.wait_for(mine, timeout=30)
        if not got:
            # no mail arrived (state): asking first is fine, the user says which report and where
            a2 = await u.ask("The Lumora report you just made — send it to my own email address.", label="email it (which)",
                             new_chat=False, timeout=600)
            got = C.wait_for(mine, timeout=60)
        print(f"INFO mail: {[(m['to'], m['subject'], [n for n, _ in m['attachments']]) for m in SMTP.received[sent0:]]}")
        check("email it: the user's own address received the edition", bool(got), a2.text[:300])
        check("email it: … with the PDF attached",
              bool(got) and any(n.endswith(".pdf") and b[:5] == b"%PDF-" for n, b in got[0]["attachments"]),
              [n for n, _ in got[0]["attachments"]] if got else [])

        # ---- 3. a scheduled edition
        card = await SC.automation_card(u, "Lumora")
        check("scheduled: the task is on the Automations page", bool(card), card)
        if card:
            prev = await C.answers_in(u, "Lumora")
            mailed0 = len(SMTP.received)
            await SC.press_on_card(u, "Lumora", "Run now")
            out, _ = await C.run_results(u, "Lumora", prev=prev, timeout=1500)
            run_links = await report_links(u)
            print(f"INFO scheduled: {' '.join(out.split())[:600]!r} links {run_links}")
            new_html = next((x for x in run_links if "/artifacts/" in x and x.endswith(".html")), "")
            check("scheduled: the run published a new edition with its links", bool(new_html) and new_html != html, run_links)
            mailed = [m for m in SMTP.received[mailed0:] if any(K.USER in t for t in m["to"])
                      and any(n.endswith(".pdf") for n, _ in m["attachments"])]
            check("scheduled: the edition was emailed to the user with its PDF (the schedule asks for it: the SMTP sink has it)",
                  bool(mailed), (out[-300:], [(m["to"], m["subject"]) for m in SMTP.received[mailed0:]]))
            st, _, body = await opens(u, U.APP.split("#")[0].rstrip("/") + "/artifacts/")
            listed = body.decode("utf-8", "replace").count("/artifacts/news/")
            check("scheduled: /artifacts/ lists both editions (HTML and PDF each)", st == 200 and listed >= 4, (st, listed))


U.setup(mail=[])
U.run(main)
