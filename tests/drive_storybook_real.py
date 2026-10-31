"""Photo story book from Drive with a real model, from the user's point of view (tests/user_common.py). The user's Google
Drive is connected (the fake Drive of tests/fake_apps.py stands in for it) and holds a "Mia birthday" folder with three
photos (mia-cake.jpg, mia-balloons.jpg, mia-friends.jpg); their own image model is set in Settings → Models (the gateway
of the e2e env file, xai/grok-imagine-image, as in tests/media_real.py). In Main's chat they ask for a short PDF story
book for Mia from those photos, with the cake photo turned into a watercolour picture, put back in the same folder.
If the agent asks first (which photos, what tone), the user answers once in the same chat.

What the user sees:
  - the restyled picture shows in the chat (a picture that loads);
  - a PDF card in the chat; the PDF downloads, has several pages (2 or more), carries the pictures (3 or more images)
    and a story that names Mia;
  - their "Mia birthday" folder in Drive holds one new PDF — the same book (2+ pages, 3+ pictures) — and the three photos;
    the answer links the new file in Drive.
(make_pdf's Markdown has no page break, so the user does not ask for one page per photo.) Billed (the chat model and
one or two image edits). One TIMING line per request.

    OMUSE_E2E_ENV_FILE=~/.config/omuse/e2e.env tests/run_in_docker.sh drive_storybook_real"""
import io
import os
import sys

from pypdf import PdfReader

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from tests import drive_common as D  # noqa: E402
from tests import user_common as U  # noqa: E402
from tests.user_common import check  # noqa: E402

PHOTOS = {"mia-cake.jpg", "mia-balloons.jpg", "mia-friends.jpg"}
IMAGE_MODEL = os.environ.get("OMUSE_E2E_IMAGE_MODEL", "xai/grok-imagine-image")


def book(data: bytes) -> tuple[int, int, str]:
    """A PDF as a reader shows it: (pages, pictures, text)."""
    rd = PdfReader(io.BytesIO(data))
    pics = 0
    for p in rd.pages:
        try:
            pics += len(p.images)
        except Exception:   # noqa: BLE001 — a picture pypdf cannot decode still counts as not seen
            pass
    return len(rd.pages), pics, " ".join("".join(p.extract_text() or "" for p in rd.pages).split())


async def main():
    before = {f["name"] for f in U.drive_folder("Mia birthday")}
    print(f"INFO the Mia birthday folder before: {sorted(before)}")
    async with U.User() as u:
        a = await u.ask("My Google Drive has a folder \"Mia birthday\" with the photos from my daughter Mia's 6th birthday party. "
                        "Make a short PDF story book for her from those photos, with a few sentences of story for each photo, "
                        "and turn the cake photo into a watercolour picture for the book. Put the finished book back in the same folder.",
                        label="storybook", timeout=1200)
        answers = [a]
        new = [f for f in U.drive_folder("Mia birthday") if f["name"] not in before]
        if not new and not a.timed_out:
            answers.append(await u.ask("Yes, use all three photos, a warm tone for a 6-year-old. Go ahead, make the book and upload it to the folder.",
                                       new_chat=False, label="storybook-followup", timeout=1200))
        media = await u.media()
        pdfs = [f for f in answers[-1].files if ".pdf" in (f["name"] + f["href"]).lower()]   # the card's name or its Download link
        data = await u.download(pdfs[-1]["href"]) if pdfs else b""
    text = "\n".join(x.text for x in answers)
    links = [l for x in answers for l in x.links]
    check("storybook: the answer finished", not any(x.timed_out for x in answers), [x.seconds for x in answers])
    pics = [m for m in media if m["kind"] == "image"]
    check("storybook: the restyled picture shows in the chat (it loads)", any(m["width"] > 0 for m in pics), pics)
    check("storybook: a PDF card is in the chat", bool(pdfs), answers[-1].files)
    if pdfs:
        check("storybook: the PDF downloads", data[:5] == b"%PDF-", data[:40])
        if data[:5] == b"%PDF-":
            pages, n_pics, txt = book(data)
            print(f"INFO the book: {len(data)} bytes, {pages} pages, {n_pics} pictures: {txt[:600]!r}")
            check("storybook: several pages (2 or more)", pages >= 2, pages)
            check("storybook: the pictures are in the book (3 or more)", n_pics >= 3, n_pics)
            check("storybook: the story is about Mia", "mia" in txt.lower(), txt[:400])
    folder = U.drive_folder("Mia birthday")
    print(f"INFO the Mia birthday folder after: {[(f['name'], f['mimeType'], f['size']) for f in folder]}")
    new = [f for f in folder if f["name"] not in before]
    check("storybook: the three photos are still in the folder", PHOTOS <= {f["name"] for f in folder}, [f["name"] for f in folder])
    books = [f for f in new if f["mimeType"] == "application/pdf" or f["name"].lower().endswith(".pdf")]
    check("storybook: the folder holds one new PDF", len(books) == 1, [f["name"] for f in new])
    if books:
        data = U.drive_file(books[0]["id"])
        pages, n_pics, _ = book(data) if data[:5] == b"%PDF-" else (0, 0, "")
        check("storybook: the PDF in Drive is the book (2+ pages, 3+ pictures)", pages >= 2 and n_pics >= 3, (len(data), pages, n_pics))
        check("storybook: the answer links the new book in Drive", any(books[0]["webViewLink"] in l or books[0]["id"] in l for l in links)
              or books[0]["id"] in text, (books[0]["webViewLink"], links, text[-400:]))


U.setup(vault=False)
D.disconnect_all()
D.reset_drive()
D.connect_drive()
s = D.image_model_settings(os.environ["OMUSE_E2E_MODEL_URL"].rstrip("/"), os.environ["OMUSE_E2E_MODEL_KEY"], IMAGE_MODEL)
check("Settings → Models: the image endpoint and key saved", bool(s.get("image_api_key_set")) and s.get("image_model") == IMAGE_MODEL,
      {k: v for k, v in s.items() if k.startswith("image_") and "key" not in k})
U.run(main)
