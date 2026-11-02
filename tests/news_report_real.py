"""News research reports with a real model on the real web, from the user's point of view (tests/user_common.py). A
real-data suite (run_in_docker.sh REAL_DATA_SUITES): the news comes from the real web search and the real pages, so what
a report says is the live news's and is not checked — only that it is published, opens and is scheduled.

1. report — in Main: "Research the latest news on tidal power in the UK and make me a newspaper-style report. Send me a
   new edition every week." Checked: the block code appends links the HTML and the PDF under /artifacts/ and names the
   next weekly edition; both open for the user; the HTML is the newspaper (masthead, the lead, the stories, sources);
   the PDF is a PDF; the Tasks tab lists a weekly task about it.
2. scheduled edition — the user presses Run now on the task (the full Automations page): a new edition is published in
   the task's chat with its links, and /artifacts/ lists both editions.
One TIMING line per request.

    OMUSE_E2E_ENV_FILE=~/.config/omuse/e2e.env tests/run_in_docker.sh news_report_real"""
import os
import re
import sys

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from tests import conn_common as C  # noqa: E402
from tests import shop_common as SC  # noqa: E402
from tests import user_common as U  # noqa: E402
from tests.user_common import check  # noqa: E402

SUBJECT = "tidal power in the UK"


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
        a = await u.ask(f"Research the latest news on {SUBJECT} and make me a newspaper-style report. "
                        "Send me a new edition every week.", label="report", timeout=1500)
        links = await report_links(u)
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
            check("report: the HTML opens for the user", st == 200 and ct.startswith("text/html"), (st, ct))
            check("report: it is the newspaper (masthead, lead, stories, sources)",
                  'class="masthead"' in page and "<h2>" in page and 'class="story"' in page and "Sources" in page, page[:300])
        if pdf:
            st, ct, body = await opens(u, pdf)
            check("report: the PDF opens for the user, and is a PDF", st == 200 and body[:5] == b"%PDF-", (st, ct, body[:8]))
        tasks = await C.page_text(u, "timers")
        check("report: the Tasks tab lists a weekly task about the subject",
              re.search(r"tidal", tasks, re.I) and re.search(r"Next run", tasks), tasks[:600])

        # ---- 2. a scheduled edition
        card = await SC.automation_card(u, "idal")
        check("scheduled: the task is on the Automations page", bool(card), card)
        if card:
            prev = await C.answers_in(u, "idal")
            await SC.press_on_card(u, "idal", "Run now")
            out, _ = await C.run_results(u, "idal", prev=prev, timeout=1500)
            run_links = await report_links(u)
            print(f"INFO scheduled: {' '.join(out.split())[:600]!r} links {run_links}")
            new_html = next((x for x in run_links if "/artifacts/" in x and x.endswith(".html")), "")
            check("scheduled: the run published a new edition with its links", bool(new_html) and new_html != html, run_links)
            st, _, body = await opens(u, U.APP.split("#")[0].rstrip("/") + "/artifacts/")
            listed = body.decode("utf-8", "replace").count("/artifacts/news/")
            check("scheduled: /artifacts/ lists both editions (HTML and PDF each)", st == 200 and listed >= 4, (st, listed))


U.setup()
U.run(main)
