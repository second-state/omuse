"""Documents with a real model, from the user's point of view (tests/user_common.py). In Main's chat the user asks for a
one-page PDF packing list for a business trip to Tokyo in December. What the user sees: a PDF file card in the chat;
it downloads (Download on the card) and it is a real PDF of one page (its wording is the model's and is not checked). One TIMING line.

    OMUSE_E2E_ENV_FILE=~/.config/omuse/e2e.env tests/run_in_docker.sh docs_real"""
import io
import os
import sys

from pypdf import PdfReader

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from tests import user_common as U  # noqa: E402
from tests.user_common import check  # noqa: E402


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


U.setup()
U.run(main)
