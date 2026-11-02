"""Attachments in the chat with a real model, from the user's point of view (tests/user_common.py). The files are the
user's own, made by the test (nothing from a stand-in page or a fixture).

  1. refused — the user tries to attach a program (setup.exe): its chip shows it failed, and they remove it.
  2. attach — with ＋ the user attaches a Word report, a picture and a Markdown note to one message and asks what they
     say. What they see: the three files uploaded (no failed chip) and shown above their message, and an answer.
(What the answer says about each file is the model's wording and is not checked.) One TIMING line per request.

    OMUSE_E2E_ENV_FILE=~/.config/omuse/e2e.env tests/run_in_docker.sh attach_media_real"""
import io
import os
import sys
import tempfile
import zipfile

from PIL import Image

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from tests import user_common as U  # noqa: E402
from tests.user_common import check  # noqa: E402


def files_to_attach() -> dict[str, str]:
    """The user's own files on their computer."""
    d = tempfile.mkdtemp()
    out = {}
    W = 'xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"'
    buf = io.BytesIO()
    with zipfile.ZipFile(buf, "w") as z:
        z.writestr("word/document.xml", f'<w:document {W}><w:body><w:p><w:r><w:t>Q3 quarterly report: revenue grew 12% to 4.2 million dollars, '
                                        'driven by the Asia launch.</w:t></w:r></w:p></w:body></w:document>')
    png = io.BytesIO()
    Image.new("RGB", (320, 240), (220, 20, 90)).save(png, "PNG")
    for name, data in (("Q3 report.docx", buf.getvalue()), ("product.png", png.getvalue()),
                       ("notes.md", b"# Team notes\n\nThe team offsite is on 14 November in Lisbon, at the Pestana hotel.\n"),
                       ("setup.exe", b"MZ\x90\x00" + b"\x00" * 60)):
        p = os.path.join(d, name)
        open(p, "wb").write(data)
        out[name] = p
    return out


async def main():
    f = files_to_attach()
    async with U.User() as u:
        pg = u.page
        # 1. a program is refused at ＋ (first, in a fresh chat: the user removes it before sending anything)
        await u.new_chat()
        chips = await u.attach(f["setup.exe"])
        check("refused: the program's chip shows it failed", chips and chips[0]["bad"], chips)
        await pg.locator(".chip-att button.x").first.click()
        check("refused: … and the user removed it", await pg.locator(".chip-att").count() == 0)

        # 2. three files attached to one message
        chips = await u.attach(f["Q3 report.docx"], f["product.png"], f["notes.md"])
        check("attach: the three files uploaded (no failed chip)", len(chips) == 3 and not any(c["bad"] for c in chips), chips)
        a = await u.ask("What do these three files say? One short line for each.", new_chat=False, label="attach", timeout=600)
        strip = await pg.locator(".msg.user .att-strip > *").count()
        check("attach: the three files show above the user's message", strip == 3, strip)
        check("attach: an answer came", not a.timed_out and bool(a.text.strip()), a.text[:300])


U.setup(vault=False)
U.run(main)
