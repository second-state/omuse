"""Documents with a real model, from the user's point of view (tests/user_common.py). In Main's chat the user asks for a
one-page PDF packing list for a business trip to Tokyo in December. What the user sees: a PDF file card in the chat;
it downloads (Download on the card), it is a real PDF of one page, and its text is a packing list for Tokyo in winter
on business. One TIMING line.

    OMUSE_E2E_ENV_FILE=~/.config/omuse/e2e.env tests/run_in_docker.sh docs_real"""
import io
import os
import sys

from pypdf import PdfReader

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from tests import user_common as U  # noqa: E402
from tests.user_common import check  # noqa: E402


def has(text: str, *words: str) -> bool:
    low = text.lower()
    return any(w in low for w in words)


async def main():
    async with U.User() as u:
        a = await u.ask("Make me a one-page PDF packing list for a 5-day business trip to Tokyo in December.", label="packing")
        pdfs = [f for f in a.files if ".pdf" in (f["name"] + f["href"]).lower()]   # the card's name, or its Download link
        check("packing: a PDF file card is in the chat", bool(pdfs), a.files)
        if not pdfs:
            return
        data = await u.download(pdfs[-1]["href"])
        check("packing: the PDF downloads", data[:5] == b"%PDF-", data[:80])
        if data[:5] != b"%PDF-":
            return
        rd = PdfReader(io.BytesIO(data))
        txt = "".join(p.extract_text() or "" for p in rd.pages)
        print(f"INFO PDF {len(data)} bytes, {len(rd.pages)} page(s): {' '.join(txt.split())[:500]!r}")
        check("packing: one page, as asked", len(rd.pages) == 1, len(rd.pages))
        check("packing: the PDF is a packing list for Tokyo", has(txt, "tokyo") and has(txt, "pack", "checklist", "list"), txt[:600])
        check("packing: … for December (warm clothes)", has(txt, "coat", "jacket", "sweater", "scarf", "layers", "thermal", "gloves"), txt[:600])
        check("packing: … for business (suit, business cards or laptop)", has(txt, "suit", "business card", "laptop", "blazer", "dress shirt"), txt[:600])


U.setup()
U.run(main)
