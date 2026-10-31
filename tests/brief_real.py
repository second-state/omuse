"""A research brief within the user's step limit, an Excel export and a site that blocks robots — with a real model, from
the user's point of view (tests/user_common.py, tests/shop_common.py). Workspace Weekly's four guides to co-working
spaces in Singapore (tests/pages/cowork_*.html at http://shop.test:8099) list nine spaces with their area, the monthly
price of a dedicated desk and the hourly price of a meeting room; DeskHub's booking page (tests/pages/deskhub_rooms.html,
on its own host http://opentable.test:8099) answers every visitor with an Akamai "Access Denied" page.

1. brief — in Settings the user lowers "Max steps" to 8, then asks for a one-page PDF brief on co-working
   spaces for a 5-person team from the four guides. Checked: the answer is delivered, not cut off at the limit (no
   "didn't finish"); a PDF file card in the chat; the PDF the user downloads names at least six of the nine spaces with
   their desk prices from the guides. The step limit is put back afterwards.
2. excel — in the same chat: "also send me those spaces as an Excel sheet with the monthly desk price as a number".
   Checked: an .xlsx file card; the workbook the user opens has a row per space (at least six) whose desk price is a
   number equal to the guide's.
3. blocked — in a new chat the user asks for a meeting room on DeskHub, or else a shortlist from the CBD guide.
   Checked: the answer tells the user DeskHub blocked access (or could not be opened) and gives meeting rooms from the
   guide with their hourly prices — it does not claim to have booked anything.
One TIMING line per request.

    OMUSE_E2E_ENV_FILE=~/.config/omuse/e2e.env tests/run_in_docker.sh brief_real"""
import io
import os
import re
import sys

from pypdf import PdfReader

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from tests import shop_common as S  # noqa: E402
from tests import user_common as U  # noqa: E402
from tests.user_common import check  # noqa: E402

WALL = "http://opentable.test:8099/deskhub_rooms.html"   # DeskHub's own site (another host than the guides)
GUIDES = [f"{S.SHOP}/cowork_{x}.html" for x in ("cbd", "east", "north", "west")]
DESK = {"The Foundry": 650, "Harbourline": 590, "Atrium 71": 720, "Kopi Desk": 380, "Eastside Works": 340, "Greenroom": 420,
        "Nexus Woodlands": 300, "LaunchPad Labs": 450, "Clementi Commons": 360}
ROOM_CBD = {"The Foundry": 60, "Harbourline": 45, "Atrium 71": 80}
CUT = re.compile(r"didn.t finish|did not finish|step limit|没有做完", re.I)


def norm(s: str) -> str:
    return re.sub(r"\s+", " ", s or "").lower()


async def main():
    async with U.User() as u:
        # ---- 1. a brief within 8 steps
        old = await S.set_number(u, "Max steps", 8)
        check("brief: Settings shows the new step limit (8)", old == "8", old)
        a = await u.ask("I'm choosing a co-working space for my 5-person team in Singapore. Using these four Workspace Weekly guides, make me a "
                        "one-page PDF brief that compares the spaces: name, area, the monthly price of a dedicated desk, and one line on each. "
                        + " ".join(GUIDES), label="brief")
        check("brief: the answer came (no time-out)", bool(a.text.strip()) and not a.timed_out, a.text[:300])
        check("brief: the answer is not cut off at the step limit", not CUT.search(a.text), a.text[:500])
        pdfs = [f for f in a.files if ".pdf" in (f["name"] + f["href"]).lower()]
        check("brief: a PDF file card is in the chat", bool(pdfs), (a.files, a.text[:400]))
        txt = ""
        if pdfs:
            data = await u.download(pdfs[-1]["href"])
            if data[:5] == b"%PDF-":
                txt = " ".join(p.extract_text() or "" for p in PdfReader(io.BytesIO(data)).pages)
        print(f"INFO brief PDF text: {' '.join(txt.split())[:900]!r}")
        named = [n for n in DESK if n.lower() in norm(txt)]
        priced = [n for n in named if re.search(rf"{DESK[n]}\b", txt)]
        check("brief: the PDF names at least six of the nine spaces from the guides", len(named) >= 6, named)
        check("brief: … with their desk prices from the guides", len(priced) >= 6, (named, priced))
        await S.set_number(u, "Max steps", 40)

        # ---- 2. the same spaces in Excel
        b = await u.ask("Thanks. Also send me those spaces as an Excel sheet, with the monthly desk price as a number.", label="excel", new_chat=False)
        xl = [f for f in b.files if ".xlsx" in (f["name"] + f["href"]).lower()]
        check("excel: an Excel file card is in the chat", bool(xl), (b.files, b.text[:400]))
        rows: dict[str, list] = {}
        if xl:
            from openpyxl import load_workbook
            data = await u.download(xl[-1]["href"])
            try:
                ws = load_workbook(io.BytesIO(data), data_only=True).active
                for r in ws.iter_rows(values_only=True):
                    line = " ".join(str(x) for x in r if x is not None)
                    for n in DESK:
                        if n.lower() in line.lower():
                            rows[n] = list(r)
            except Exception as e:   # noqa: BLE001 — not a workbook the user can open
                print(f"INFO not a workbook: {e!r} {data[:60]!r}")
        print(f"INFO workbook rows: {rows}")
        numeric = [n for n, r in rows.items() if any(isinstance(x, (int, float)) and abs(x - DESK[n]) < 0.01 for x in r)]
        check("excel: the workbook has a row for at least six of the spaces", len(rows) >= 6, sorted(rows))
        check("excel: … each with the desk price as a number equal to the guide's", len(numeric) >= 6 and len(numeric) == len(rows), (numeric, rows))

        # ---- 3. a site that blocks robots
        c = await u.ask(f"Can you book a meeting room for 6 people next Tuesday 2–4 pm on DeskHub ({WALL})? If that doesn't work, "
                        f"just give me a shortlist of meeting rooms from this guide with their hourly prices: {GUIDES[0]}", label="blocked")
        check("blocked: the answer came (no time-out)", bool(c.text.strip()) and not c.timed_out, c.text[:300])
        check("blocked: the answer says DeskHub blocked access (or could not be opened)",
              re.search(r"DeskHub", c.text, re.I) and re.search(r"block|denied|access|couldn.t|could not|unable|not able", c.text, re.I), c.text[:700])
        rooms = [n for n, p in ROOM_CBD.items() if n.lower() in norm(c.text) and re.search(rf"\$\s?{p}\b|{p}\s?(/|per|an) ?h", c.text)]
        check("blocked: … and gives meeting rooms from the guide with their hourly prices", len(rooms) >= 2, (rooms, c.text[:900]))
        check("blocked: … without claiming a booking", not re.search(r"\b(I('ve| have)? booked|is booked|booking (is )?confirmed)\b", c.text, re.I),
              c.text[:600])


U.setup(mail=[], vault=False)
U.run(main)
