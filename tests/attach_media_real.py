"""Media in and out of the chat with a real model, from the user's point of view (tests/user_common.py).

  1. gallery — the user asks Main to open the OMG Beauty best-sellers page (http://shop.test:8099/gallery.html: a
     lipstick photo, a serum photo, a brand film) and send them the product photos and the video. What they see: the
     pictures and a video player in the chat; the pictures load and are the page's own (the lipstick and the serum
     pictures, as downloaded), the video downloads as the page's 2-second film.
  2. attach — with ＋ the user attaches a Word report, a picture and a Markdown note to one message and asks what they
     say. What they see: the three files above their message, and an answer drawn from each (the revenue figure in the
     report, the offsite city in the note, the lipstick picture).
  3. later — in the same chat they ask what colour the picture was: the answer names its colour (a pink/red).
  4. video — in a new chat they attach a short clip and ask what is in it: the answer describes the colour bars.
  5. refused — they try to attach a program (setup.exe): its chip shows it failed, and they remove it.
One TIMING line per request.

    OMUSE_E2E_ENV_FILE=~/.config/omuse/e2e.env tests/run_in_docker.sh attach_media_real"""
import io
import json
import os
import subprocess
import sys
import tempfile
import zipfile

from PIL import Image

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from tests import drive_common as D  # noqa: E402
from tests import user_common as U  # noqa: E402
from tests.user_common import check  # noqa: E402

PAGES = os.path.join(os.path.dirname(os.path.abspath(__file__)), "pages")
PINK = ("pink", "magenta", "crimson", "raspberry", "fuchsia", "red", "rose", "cerise", "berry")


def files_to_attach() -> dict[str, str]:
    """The user's own files on their computer."""
    d = tempfile.mkdtemp()
    out = {}
    W = 'xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"'
    buf = io.BytesIO()
    with zipfile.ZipFile(buf, "w") as z:
        z.writestr("word/document.xml", f'<w:document {W}><w:body><w:p><w:r><w:t>Q3 quarterly report: revenue grew 12% to 4.2 million dollars, '
                                        'driven by the Asia launch.</w:t></w:r></w:p></w:body></w:document>')
    for name, data in (("Q3 report.docx", buf.getvalue()), ("product.png", open(os.path.join(PAGES, "lipstick.png"), "rb").read()),
                       ("notes.md", b"# Team notes\n\nThe team offsite is on 14 November in Lisbon, at the Pestana hotel.\n"),
                       ("clip.mp4", open(os.path.join(PAGES, "promo.mp4"), "rb").read()), ("setup.exe", b"MZ\x90\x00" + b"\x00" * 60)):
        p = os.path.join(d, name)
        open(p, "wb").write(data)
        out[name] = p
    return out


def video_seconds(data: bytes) -> float:
    with tempfile.NamedTemporaryFile(suffix=".mp4") as f:
        f.write(data)
        f.flush()
        r = subprocess.run(["ffprobe", "-v", "error", "-show_entries", "format=duration", "-of", "json", f.name], capture_output=True, text=True, timeout=60)
    try:
        return float(json.loads(r.stdout)["format"]["duration"])
    except (ValueError, KeyError):
        return 0.0


def same_picture(data: bytes, page_file: str) -> bool:
    """The downloaded picture is the page's own: the same bytes, or the same picture once decoded (size and colours)."""
    ref = open(os.path.join(PAGES, page_file), "rb").read()
    if data == ref:
        return True
    try:
        a, b = Image.open(io.BytesIO(data)).convert("RGB"), Image.open(io.BytesIO(ref)).convert("RGB")
    except Exception:   # noqa: BLE001 — not a picture
        return False
    return a.size == b.size and a.resize((8, 8)).tobytes() == b.resize((8, 8)).tobytes()


async def main():
    f = files_to_attach()
    async with U.User() as u:
        pg = u.page
        # 1. the page's photos and video into the chat
        a = await u.ask("Open http://shop.test:8099/gallery.html (the OMG Beauty best sellers) and send me the product photos and the "
                        "brand video from that page here in the chat.", label="gallery", timeout=600)
        media = await u.media()
        pics = [m for m in media if m["kind"] == "image"]
        vids = [m for m in media if m["kind"] == "video"]
        check("gallery: the answer finished", not a.timed_out, a.seconds)
        check("gallery: the photos show in the chat and load", len(pics) >= 2 and all(m["width"] > 0 for m in pics), pics)
        got = [await u.download(m["src"]) for m in pics]
        check("gallery: … they are the page's lipstick and serum pictures", any(same_picture(x, "lipstick.png") for x in got)
              and any(same_picture(x, "serum.jpg") for x in got), [len(x) for x in got])
        check("gallery: a video player shows in the chat", len(vids) == 1, media)
        if vids:
            v = await u.download(vids[0]["src"])
            secs = video_seconds(v)
            check("gallery: the video downloads as the page's 2-second film", v[4:8] == b"ftyp" and 1.5 <= secs <= 2.5, (len(v), secs))

        # 5. a program is refused at ＋ (first, in a fresh chat: the user removes it before sending anything)
        await u.new_chat()
        chips = await D.attach(u, f["setup.exe"])
        check("refused: the program's chip shows it failed", chips and chips[0]["bad"], chips)
        await pg.locator(".chip-att button.x").first.click()
        check("refused: … and the user removed it", await pg.locator(".chip-att").count() == 0)

        # 2. three files attached to one message
        chips = await D.attach(u, f["Q3 report.docx"], f["product.png"], f["notes.md"])
        check("attach: the three files uploaded (no failed chip)", len(chips) == 3 and not any(c["bad"] for c in chips), chips)
        a = await u.ask("What do these three files say? One short line for each.", new_chat=False, label="attach", timeout=600)
        strip = await pg.locator(".msg.user .att-strip > *").count()
        check("attach: the three files show above the user's message", strip == 3, strip)
        check("attach: the report's revenue figure (12%)", "12" in a.text and a.mentions("revenue", "%"), a.text[:800])
        check("attach: the note's offsite (Lisbon, 14 November)", a.mentions("Lisbon"), a.text[:800])
        check("attach: the picture (a lipstick)", a.mentions("lipstick"), a.text[:800])

        # 3. a later question about the picture, in the same chat
        a = await u.ask("What colour was the picture I attached?", new_chat=False, label="later", timeout=400)
        check("later: the answer names the picture's colour", a.mentions(*PINK), a.text[:500])

        # 4. a video attachment
        await u.new_chat()
        await D.attach(u, f["clip.mp4"])
        a = await u.ask("What is in this video?", new_chat=False, label="video", timeout=400)
        check("video: the answer describes the clip (colour bars / a test pattern)", a.mentions("bar", "test pattern", "test card", "stripe")
              and a.mentions("colo", "cyan", "magenta", "yellow", "red", "green", "blue"), a.text[:500])


U.setup(vault=False)
U.run(main)
