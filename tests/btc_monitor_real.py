"""Monitoring a price and its news with a real model on the real web and the real market data, from the user's point
of view (tests/user_common.py). A real-data suite (run_in_docker.sh REAL_DATA_SUITES): the news comes from the real
web search and pages, the price from the real market data (Yahoo Finance), so what the report says is the live news's
and is not checked — only that the update came, carries the price, opens and is scheduled.

1. monitor — Main's first example, in Main's chat: "Monitor BTC price and news. Send me an update with research report
   every day at 8am." No mailbox is connected (nothing is ever mailed in e2e). Checked, all in what code appends after
   the answer and on the app's pages: the BTC price (BTC-USD, within 3% of the live price the market data gives now);
   the first update's report (the HTML edition under /artifacts/, which opens and is the newspaper with the price under
   its masthead); delivered here in the chat and not emailed; Automations lists the daily task at 08:00 and the Tasks
   tab its next run at 08:00.
One TIMING line per request.

    OMUSE_E2E_ENV_FILE=~/.config/omuse/e2e.env tests/run_in_docker.sh btc_monitor_real"""
import os
import re
import sys
import time

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from tests import conn_common as C  # noqa: E402
from tests import shop_common as SC  # noqa: E402
from tests import user_common as U  # noqa: E402
from tests.user_common import check  # noqa: E402

REQUEST = "Monitor BTC price and news. Send me an update with research report every day at 8am."


def live_price(sym: str) -> float:
    """The live price the market data gives now (Yahoo's chart data), 0 when it cannot be read."""
    for attempt in range(6):
        host = ("query1", "query2")[attempt % 2]
        try:
            r = U.c.get(f"https://{host}.finance.yahoo.com/v8/finance/chart/{sym}", params={"range": "5d", "interval": "1d"},
                        headers={"User-Agent": "Mozilla/5.0"}, timeout=30)
            return float(r.json()["chart"]["result"][0]["meta"]["regularMarketPrice"])
        except Exception:   # noqa: BLE001
            time.sleep(5)
    return 0.0


def numbers(text: str) -> list[float]:
    return [float(x.replace(",", "")) for x in re.findall(r"\d[\d,]*\.\d+|\d[\d,]{3,}", text)]


async def main():
    async with U.User() as u:
        a = await u.ask(REQUEST, label="monitor", timeout=1500)
        check("monitor: the answer came (no time-out)", bool(a.text.strip()) and not a.timed_out, a.text[:300])

        # the price: the block code appends carries BTC-USD's price from the market data
        line = next((ln for ln in a.block.splitlines() if "BTC-USD" in ln), "")
        live = live_price("BTC-USD")
        print(f"INFO price line: {line!r} live {live}")
        check("monitor: the appended block gives the BTC-USD price, within 3% of the live price",
              bool(line) and live > 0 and any(abs(n - live) <= live * 0.03 for n in numbers(line)), (line, live))

        # the first update's report: the HTML edition opens and is the newspaper, with the price under its masthead
        html = next((x for x in a.block_links if "/artifacts/" in x and x.endswith(".html")), "")
        check("monitor: the appended block links the update's report (HTML edition under /artifacts/)",
              bool(html) and "HTML edition" in a.block, (a.block, a.block_links))
        if html:
            r = await u.ctx.request.get(html)
            page = (await r.body()).decode("utf-8", "replace")
            check("monitor: the report opens for the user", r.status == 200 and r.headers.get("content-type", "").startswith("text/html"),
                  (r.status, r.headers.get("content-type")))
            check("monitor: it is the newspaper (masthead, lead, sources) with the BTC price under the masthead",
                  'class="masthead"' in page and "<h2>" in page and "Sources" in page
                  and 'class="markets"' in page and "BTC-USD" in page, page[:400])

        # "send me" with no mailbox connected: delivered here, nothing emailed
        check("monitor: delivered in the chat (no mailbox connected), not emailed",
              a.shows("Delivered") and "Emailed to" not in a.block, a.block)

        # the daily 08:00 task
        card = await SC.automation_card(u, "News report")
        print(f"INFO Automations card: {card!r}")
        check("monitor: Automations lists the daily task at 08:00",
              bool(card) and re.search(r"\b0?8:00\b|\b8\s?(am|a\.m\.)", card, re.I)
              and re.search(r"daily|every day|each day|\* \* \*", card, re.I), card)
        tasks = await C.page_text(u, "timers")
        print(f"INFO Tasks tab: {tasks[:600]!r}")
        check("monitor: the Tasks tab lists it with its next run at 08:00",
              re.search(r"News report", tasks) and re.search(r"Next run[^A-Za-z]{0,40}\b0?8:00", tasks), tasks[:600])


U.setup()
U.run(main)
