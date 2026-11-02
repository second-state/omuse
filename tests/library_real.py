"""The research library with a real model, from the user's point of view (tests/user_common.py).

  1. subject — on Memory → Library the user adds a subject ("Home NAS 2026": a four-bay NAS under 1,000 US dollars
     that runs Docker) and clicks Start research. The app opens the subject's own chat; the research answers there. What
     they see: the subject is listed as ready; its Report (📄) is a report on home NAS boxes that names several models
     (Synology, QNAP, UGREEN, TerraMaster, Asustor …), Docker, prices, and where they come from (its sources).
  2. follow-up — in the subject's chat they ask to add a section on how noisy each one is: the Report now has that
     section (noise / dB) and still lists the models it had.
  3. later — in a new chat with Main they ask which NAS from their research runs Docker for under 1,000 dollars: the
     answer names a model that is in their report.
A real-data suite (run_in_docker.sh REAL_DATA_SUITES): the research goes to the real web search and real pages. One TIMING line per request, and one for the research.

    OMUSE_E2E_ENV_FILE=~/.config/omuse/e2e.env tests/run_in_docker.sh library_real"""
import os
import re
import sys
import time

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from tests import user_common as U  # noqa: E402
from tests.user_common import check  # noqa: E402

TITLE = "Home NAS 2026"
BRANDS = ("synology", "qnap", "ugreen", "terramaster", "asustor", "wd ", "western digital")
MODEL = re.compile(r"\b(DS\d{3,4}\+?|DXP\d{4}\w*|TS-\d{3,4}\w*|F\d-\d{3}\w*|AS\d{4}\w*|AS\d{2}\d{2}T?)\b", re.I)


async def subject_row(u: U.User) -> dict:
    """The subject's row on Memory → Library as the user reads it: {"status", "meta", "text"}."""
    pg = u.page
    await pg.goto(U.APP + "#memory")
    await pg.wait_for_selector("#library", timeout=20000)
    row = pg.locator("#library .row", has=pg.locator("b", has_text=TITLE)).first
    if not await row.count():
        return {"status": "", "meta": "", "text": ""}
    return {"status": (await row.locator(".pill").first.inner_text()).strip(), "meta": (await row.locator(".small.faint").first.inner_text()).strip(),
            "text": " ".join((await row.inner_text()).split())}


async def report(u: U.User) -> str:
    """The user clicks 📄 Report on the subject's row and reads the report in the dialog (then closes it)."""
    pg = u.page
    await pg.goto(U.APP + "#memory")
    await pg.wait_for_selector("#library", timeout=20000)
    row = pg.locator("#library .row", has=pg.locator("b", has_text=TITLE)).first
    await row.locator("button", has_text="Report").first.click()
    dlg = pg.locator(".modal-back .modal").last
    await dlg.wait_for(timeout=20000)
    text = await dlg.inner_text()
    links = await dlg.evaluate("m => [...m.querySelectorAll('a[href^=\"http\"]')].map(a => a.href)")
    await dlg.locator("button.icon-btn").first.click()
    return text + "\n" + "\n".join(links)


async def main():
    async with U.User() as u:
        pg = u.page
        # 1. a new subject on Memory → Library
        await pg.goto(U.APP + "#memory")
        await pg.wait_for_selector("#library", timeout=20000)
        lib = pg.locator("#library")
        await lib.locator("summary", has_text="New subject").click()
        await lib.locator("input[type=text]").first.fill(TITLE)
        await lib.locator("textarea").first.fill("A four-bay home NAS for under 1,000 US dollars that can run Docker containers. Compare the "
                                                 "main models with their prices and say which to buy. Keep the report short: four or five "
                                                 "candidates.")
        t0 = time.monotonic()
        await lib.locator("button", has_text="Start research").click()
        await pg.wait_for_function("() => location.hash.startsWith('#chat/')", timeout=20000)
        conv_url = pg.url
        print(f"INFO the subject's chat: {conv_url}")
        # the research answers in the subject's chat; the user waits for it there
        done = False
        while time.monotonic() - t0 < 1100:
            if await pg.evaluate("() => document.querySelectorAll('.msg.assistant:not(.live) .bubble').length > 0 && !document.querySelector('.msg.assistant.live')"):
                done = True
                break
            await pg.wait_for_timeout(3000)
        print(f"TIMING {U.SUITE}/research {time.monotonic() - t0:.0f} s")
        answer = (await pg.locator(".msg.assistant:not(.live) .bubble").last.inner_text()) if done else ""
        print(f"INFO research answer: {' '.join(answer.split())[:700]}")
        check("subject: the research answered in the subject's own chat", done, time.monotonic() - t0)
        for _ in range(20):   # the row turns ready right after the answer
            row = await subject_row(u)
            if "ready" in row["status"].lower():
                break
            await pg.wait_for_timeout(1500)
        print(f"INFO the subject's row: {row}")
        check("subject: listed as ready on Memory → Library", "ready" in row["status"].lower(), row)
        rep = await report(u)
        low = rep.lower()
        print(f"INFO report ({len(rep)} chars): {' '.join(rep.split())[:900]}")
        brands = [b.strip() for b in BRANDS if b in low]
        models = {m.upper() for m in MODEL.findall(rep)}
        check("subject: the report compares several NAS models (2+ brands)", len(brands) >= 2, brands)
        check("subject: … covers Docker and prices", "docker" in low and re.search(r"(\$|usd|us\$)\s?\d", low) is not None, rep[:600])
        # the links themselves go to library/<slug>/sources.md, which the Report dialog does not show (the product writes them there)
        check("subject: … and says where its figures come from (its sources)", re.search(r"https?://|\bsources?\b", low) is not None, rep[-600:])

        # 2. a change asked in the subject's chat
        await pg.goto(conv_url)
        await pg.wait_for_selector("#chatInput")
        a = await u.ask("Please add a short section to the report on how noisy each of these NAS boxes is.", new_chat=False, label="noise", timeout=700)
        rep2 = await report(u)
        print(f"INFO report after ({len(rep2)} chars): {' '.join(rep2.split())[-700:]}")
        check("follow-up: the answer finished", not a.timed_out, a.seconds)
        check("follow-up: the report now has a section on noise", re.search(r"nois|\bdb\b|db\(a\)|dba", rep2.lower()) is not None
              and rep2.lower().count("nois") > low.count("nois"), rep2[-800:])
        check("follow-up: … and still lists the models it had", all(b in rep2.lower() for b in brands), brands)

        # 3. a later question in another chat draws on the library
        a = await u.ask("From my research library: which NAS that runs Docker for under 1,000 dollars did my research recommend? Just name it.",
                        label="later", timeout=500)
        named = {m.upper() for m in MODEL.findall(a.text)}
        check("later: the answer names a model from the user's report", bool(named & models) or (not models and any(b in a.text.lower() for b in brands)),
              (sorted(named), sorted(models), a.text[:400]))


U.setup(vault=False)
U.run(main)
