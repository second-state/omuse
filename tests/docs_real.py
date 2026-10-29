"""Documents with a real model (see tests/real_common.py): a plain request for a one-page PDF sent to the main bot, which
keeps it (documents are general work). The model must write the packing list, print it to PDF on this machine
(make_pdf), and hand it over (send_file, after make_pdf): a file card in the conversation, and the file behind it (the
download API the scripted suites use, /api/files/raw) is a real one-page PDF whose text is a packing list for Tokyo in
December. No browser. One TIMING line.

    OMUSE_E2E_ENV_FILE=~/.config/omuse/e2e.env OMUSE_E2E_PLANNER=xai/grok-4.7 OMUSE_E2E_EXECUTOR=openai-codex/gpt-6-luna \\
        tests/run_in_docker.sh docs_real"""
import io
import json
import os
import sys

from pypdf import PdfReader

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from tests import real_common as R  # noqa: E402


def file_cards(conv: str) -> list[dict]:
    out = []
    for m in R.c.get(f"{R.B}/api/conversations/{conv}", headers=R.H).json()["messages"]:
        if m["role"] == "system":
            try:
                j = json.loads(m["content"])
            except ValueError:
                continue
            if j.get("type") == "file":
                out.append(j)
    return out


R.setup()
info = R.ask("Make me a one-page PDF packing list for a 5-day business trip to Tokyo in December.")
R.routed("packing", info, "main")
R.finished("packing", info)
names = info["names"]
R.check("packing: make_pdf, then send_file", "make_pdf" in names and "send_file" in names
        and names.index("make_pdf") < len(names) - 1 - names[::-1].index("send_file"), names)
sent = [str((x.get("args") or {}).get("path", "")) for x in info["calls"] if x["name"] == "send_file"]
pdfs = [p for p in sent if p.lower().endswith(".pdf")]
R.check("packing: the file handed over is a PDF", bool(pdfs), sent)
cards = file_cards(info["conv_id"]) if info["conv_id"] else []
R.check("packing: a file card for the PDF in the conversation", any(".pdf" in json.dumps(c).lower() for c in cards), cards)
if pdfs:
    d = R.c.get(R.B + "/api/files/raw", params={"path": pdfs[-1], "download": 1}, headers=R.H)
    R.check("packing: the PDF downloads", d.status_code == 200 and d.content[:5] == b"%PDF-", (d.status_code, d.content[:80]))
    if d.status_code == 200 and d.content[:5] == b"%PDF-":
        rd = PdfReader(io.BytesIO(d.content))
        txt = "".join(p.extract_text() or "" for p in rd.pages)
        print(f"INFO PDF {len(d.content)} bytes, {len(rd.pages)} page(s): {txt[:400]!r}")
        R.check("packing: one page, as asked", len(rd.pages) == 1, len(rd.pages))
        R.check("packing: the PDF is a packing list for Tokyo in December (Tokyo, and warm or business clothes)",
                R.mentions(txt, "Tokyo") and R.mentions(txt, "coat", "jacket", "sweater", "scarf", "suit", "layers", "thermal"), txt[:600])
R.no_browser("packing", info)
R.timing("packing", info)
R.finish()
