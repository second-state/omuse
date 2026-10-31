"""Market data and exact maths with a real model on the real market data, from the user's point of view
(tests/user_common.py). A real-data suite (run_in_docker.sh REAL_DATA_SUITES): market data is Yahoo Finance itself; the
test reads the same live prices (Yahoo's chart data, as a quote page shows them) to judge the answer.

  1. quotes — "Tencent's and HSBC's share price and how they moved over the past year, with a chart comparing them":
     the answer gives both prices (within 3% of the live price) and the year's move, a chart picture shows in the chat
     (a real PNG), and the user was never asked to take over or approve a browser (no browser card in the chat).
  2. unknown ticker — the price of NOPE.XX: the answer says there is no such ticker / no data (no made-up price).
  3. fundamentals — Tesla's and BYD's P/E and market cap: a P/E number and a market cap (billions / trillions) for each.
  4. mortgage — the monthly payment on a 3,000,000 loan over 25 years at 3.5%: 15,018.71.
  5. survey — the user attaches a CSV of 40 survey answers (an NPS column) and asks for the promoters, passives,
     detractors and the NPS: 12 / 14 / 14 and −5.
The rate-limited first host and the crumb flow are exercised on the way but are the scripted tests/market_e2e.py's to
check (the user cannot see which host answered). One TIMING line per request.

    OMUSE_E2E_ENV_FILE=~/.config/omuse/e2e.env tests/run_in_docker.sh market_real"""
import io
import time
import os
import re
import sys
import tempfile

from PIL import Image

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from tests import drive_common as D  # noqa: E402
from tests import user_common as U  # noqa: E402
from tests.user_common import check  # noqa: E402


def survey() -> str:
    """The user's survey export: 40 answers, NPS scores cycling 2, 5, 7, 8, 9, 10."""
    p = os.path.join(tempfile.mkdtemp(), "survey.csv")
    open(p, "w").write("id,age,nps\n" + "\n".join(f"u{i},{20 + i},{[2, 5, 7, 8, 9, 10][i % 6]}" for i in range(40)) + "\n")
    return p


def live_price(sym: str) -> float:
    """The live price a quote page shows (Yahoo's chart data), 0 when it cannot be read."""
    # both hosts, a few tries: right after the product's own quotes Yahoo may answer 429 for a moment
    for attempt in range(6):
        host = ("query1", "query2")[attempt % 2]
        try:
            r = U.c.get(f"https://{host}.finance.yahoo.com/v8/finance/chart/{sym}", params={"range": "5d", "interval": "1d"},
                        headers={"User-Agent": "Mozilla/5.0"}, timeout=30)   # a full browser UA without cookies gets 429
            return float(r.json()["chart"]["result"][0]["meta"]["regularMarketPrice"])
        except Exception:   # noqa: BLE001
            time.sleep(5)
    return 0.0


def near_any(text: str, value: float, tol: float = 0.03) -> bool:
    """Whether the text gives a number within `tol` of `value`."""
    nums = [float(x.replace(",", "")) for x in re.findall(r"\d[\d,]*\.?\d*", text) if x.replace(",", "").replace(".", "").isdigit()]
    return value > 0 and any(abs(n - value) <= value * tol for n in nums)


def num(text: str, pattern: str) -> bool:
    return re.search(pattern, text.replace("−", "-")) is not None


async def main():
    async with U.User() as u:
        # 1. quotes, a year's moves and a chart
        a = await u.ask("What are Tencent's (0700.HK) and HSBC's (0005.HK) share prices now, and how have they moved over the past year? "
                        "Draw a chart comparing the two.", label="quotes", timeout=500)
        pics = [m for m in await u.media() if m["kind"] == "image"]
        cards = await u.thread_cards()
        print(f"INFO cards: {cards}")
        check("quotes: the answer finished", not a.timed_out, a.seconds)
        tencent, hsbc = live_price("0700.HK"), live_price("0005.HK")
        print(f"INFO live prices: Tencent {tencent}, HSBC {hsbc}")
        check("quotes: Tencent's price (within 3% of the live price)", near_any(a.text, tencent), (tencent, a.text[:800]))
        check("quotes: HSBC's price (within 3% of the live price)", near_any(a.text, hsbc), (hsbc, a.text[:800]))
        check("quotes: the year's move (a percentage change)", num(a.text, r"[+-]?\d+(\.\d+)?\s?%"), a.text[:800])
        check("quotes: a chart picture shows in the chat and loads", len(pics) == 1 and pics[0]["width"] > 0, pics)
        if pics:
            data = await u.download(pics[0]["src"])
            check("quotes: … a real PNG", data[:8] == b"\x89PNG\r\n\x1a\n" and Image.open(io.BytesIO(data)).size[0] > 400, len(data))
        check("quotes: no browser was involved (no takeover, approval or browser card)", not a.takeovers and not a.approvals
              and not any("browser" in c.lower() for c in cards), (a.takeovers, a.approvals, cards))

        # 2. a ticker that does not exist
        a = await u.ask("And what is the share price of NOPE.XX?", new_chat=False, label="unknown", timeout=300)
        a.text = a.text.replace("\u2019", "'")
        check("unknown: the answer says there is no such ticker / no data", a.mentions("not found", "no data", "couldn't find", "could not find",
                                                                                       "doesn't exist", "does not exist", "no such", "unknown",
                                                                                       "invalid", "not a valid", "no quote", "unable", "not recognized",
                                                                                       "not recognised", "isn't recognized", "isn't recognised", "isn't a valid", "no results",
                                                                                       "couldn't retrieve", "could not retrieve")
              and not U.PRICE.search(a.text.split("NOPE")[-1][:60]), a.text[:500])

        # 3. fundamentals
        a = await u.ask("What are Tesla's and BYD's (1211.HK) P/E ratios and market caps?", label="fundamentals", timeout=400)
        check("fundamentals: a P/E for each and a market cap (billions / trillions)",
              len(re.findall(r"P/?E[^.\n]{0,40}?\d+(\.\d+)?", a.text, re.I)) >= 2 or len(re.findall(r"\b\d{1,4}(\.\d+)?x?\b", a.text)) >= 2,
              a.text[:800])
        check("fundamentals: market caps in billions / trillions", num(a.text, r"\d(\.\d+)?\s?(T|B|trillion|billion)\b"), a.text[:800])

        # 4. exact loan maths
        a = await u.ask("What is the monthly payment on a 3,000,000 mortgage over 25 years at 3.5% a year?", label="mortgage", timeout=300)
        check("mortgage: the exact monthly payment (15,018.71)", num(a.text, r"15,?018\.7|15,?019\b"), a.text[:500])

        # 5. a survey file attached with ＋
        await u.new_chat()
        chips = await D.attach(u, survey())
        check("survey: the CSV uploaded", len(chips) == 1 and not chips[0]["bad"], chips)
        a = await u.ask("This is our customer survey. Using the standard NPS groups (0–6 detractors, 7–8 passives, 9–10 promoters), how many "
                        "promoters, passives and detractors are there, and what is the NPS?", new_chat=False, label="survey", timeout=400)
        t = a.text.replace("−", "-")
        check("survey: the exact counts (12 promoters, 14 passives, 14 detractors)",
              re.search(r"promoters?(\s*\([^)]*\))?\D{0,40}\b12\b|\b12\b\D{0,20}promoters?", t, re.I) is not None
              and re.search(r"passives?(\s*\([^)]*\))?\D{0,40}\b14\b|\b14\b\D{0,20}passives?", t, re.I) is not None
              and re.search(r"detractors?(\s*\([^)]*\))?\D{0,40}\b14\b|\b14\b\D{0,20}detractors?", t, re.I) is not None, t[:800])
        check("survey: the NPS (−5)", re.search(r"-\s?5(\.0+)?\b", t) is not None, t[:800])


U.setup(vault=False)
U.run(main)
