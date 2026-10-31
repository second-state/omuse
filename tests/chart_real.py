"""Charts and diagrams with a real model, from the user's point of view (tests/user_common.py).

  1. line chart — the user gives a year of monthly sales figures and asks for a line chart. What they see: one picture
     in the chat that loads, a real PNG of a chart's size (wider than 400 px), downloadable.
  2. flowchart — in the same chat, a diagram of their expense approval process (submit → manager → finance → paid):
     a second picture in the chat, a real PNG.
  3. report — the chart and the flowchart in a PDF report and in a Word document: a PDF card and a Word card in the
     chat; the PDF downloads with the pictures in it, the Word file opens (a .docx) with its pictures inside.
  4. bad request — a bar chart of four quarters with only three values: the answer points out the missing value (or
     asks for it) instead of drawing a chart that invents it.
One TIMING line per request.

    OMUSE_E2E_ENV_FILE=~/.config/omuse/e2e.env tests/run_in_docker.sh chart_real"""
import io
import os
import re
import sys
import zipfile

from PIL import Image
from pypdf import PdfReader

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from tests import user_common as U  # noqa: E402
from tests.user_common import check  # noqa: E402

SALES = "Jan 120, Feb 135, Mar 150, Apr 142, May 160, Jun 175, Jul 190, Aug 185, Sep 170, Oct 200, Nov 215, Dec 240"


def png_size(data: bytes) -> tuple[int, int]:
    try:
        im = Image.open(io.BytesIO(data))
        return im.size if im.format == "PNG" else (0, 0)
    except Exception:   # noqa: BLE001
        return 0, 0


async def main():
    async with U.User() as u:
        # 1. a line chart
        a = await u.ask(f"Here are my shop's monthly sales for 2026 in thousands of dollars: {SALES}. Draw a line chart of them.",
                        label="line", timeout=400)
        pics = [m for m in await u.media() if m["kind"] == "image"]
        check("line: the answer finished", not a.timed_out, a.seconds)
        check("line: one picture shows in the chat and loads", len(pics) == 1 and pics[0]["width"] > 0, pics)
        if pics:
            w, h = png_size(await u.download(pics[0]["src"]))
            check("line: it downloads as a real PNG of a chart's size (over 400 px wide)", w > 400 and h > 200, (w, h))

        # 2. a flowchart
        a = await u.ask("Now draw a flowchart of our expense approval process: the employee submits the claim, the manager approves it, "
                        "finance checks the receipts, then the money is paid.", new_chat=False, label="flow", timeout=400)
        pics = [m for m in await u.media() if m["kind"] == "image"]
        check("flow: a second picture shows in the chat and loads", len(pics) == 2 and all(p["width"] > 0 for p in pics), pics)
        if len(pics) == 2:
            w, h = png_size(await u.download(pics[1]["src"]))
            check("flow: it downloads as a real PNG", w > 200 and h > 100, (w, h))

        # 3. both in a PDF and a Word document
        a = await u.ask("Put the sales chart and the flowchart into a short PDF report, and also give me the same report as a Word document.",
                        new_chat=False, label="report", timeout=600)
        # every Download link of the chat's file cards (a card may carry several files), named as the link saves them
        cards = await u.page.evaluate("() => [...document.querySelectorAll('.bubble.filecard a[download]')].map(a => ({name: a.getAttribute('download') || a.textContent, href: a.href}))")
        pdf = [c for c in cards if ".pdf" in c["name"].lower()]
        docx = [c for c in cards if ".docx" in c["name"].lower()]
        check("report: a PDF card and a Word card are in the chat", bool(pdf) and bool(docx), cards)
        if pdf:
            data = await u.download(pdf[-1]["href"])
            n = 0
            if data[:5] == b"%PDF-":
                for p in PdfReader(io.BytesIO(data)).pages:
                    try:
                        n += len(p.images)
                    except Exception:   # noqa: BLE001
                        pass
            check("report: the PDF downloads with both pictures in it", data[:5] == b"%PDF-" and n >= 2, (data[:8], n))
        if docx:
            data = await u.download(docx[-1]["href"])
            try:
                z = zipfile.ZipFile(io.BytesIO(data))
                media = [x for x in z.namelist() if x.startswith("word/media/")]
                ok = "word/document.xml" in z.namelist()
            except zipfile.BadZipFile:
                media, ok = [], False
            check("report: the Word file opens (a .docx) with both pictures inside", ok and len(media) >= 2, (len(data), media))
        names = [c["name"] for c in cards]
        check("report: each file is shown once", len(names) == len(set(names)), names)

        # 4. a request that does not add up
        a = await u.ask("Make a bar chart of our quarterly profit: the labels are Q1, Q2, Q3 and Q4 and the values are 10, 20 and 30.",
                        label="bad", timeout=400)
        pics = [m for m in await u.media() if m["kind"] == "image"]
        flagged = a.mentions("Q4") and re.search(r"missing|only three|only 3|three values|3 values|no value|not given|not provided|didn.t give|"
                                                 r"provide|what (is|was|should)|what[’']s|which value|fourth", a.text, re.I) is not None
        check("bad: the answer points out the missing Q4 value (or asks for it)", flagged, a.text[:600])
        print(f"INFO bad: {len(pics)} picture(s) in that chat")


U.setup(vault=False)
U.run(main)
