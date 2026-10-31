"""A file handed over in the chat, and a comparison researched side by side, with a real model, from the user's point of
view (tests/user_common.py, tests/conn_common.py).
  1. file — "Write me a packing list for a 3-day trip to Helsinki in November with two kids, as a Markdown file I can
     download.": a file card in the chat; clicking Download saves a .md file whose text is a packing list (warm clothes,
     the kids' things).
  2. compare — five hotels' own websites (served at http://shop.test:8115, each taking 20 s to answer — slow sites, the
     world) and the user asks to compare their price per night, guest rating and distance to Raffles Place MRT and
     recommend one. Checked: every hotel with its price and rating from its site; and the answer arrives before the five
     pages could have been loaded one after another (5 × 20 s), i.e. the sites were read side by side.
One TIMING line per request.

    OMUSE_E2E_ENV_FILE=~/.config/omuse/e2e.env tests/run_in_docker.sh chat_real"""
import os
import re
import sys

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from tests import conn_common as C  # noqa: E402
from tests import user_common as U  # noqa: E402
from tests.user_common import check  # noqa: E402

DELAY = 20   # under the 30 s a page read may take
HOTELS = [   # path, name, price per night (S$), rating, metres to Raffles Place MRT
    ("/harbourview", "Harbourview Inn", 210, "4.4", 300),
    ("/orchidgarden", "Orchid Garden Hotel", 185, "4.1", 650),
    ("/marinapearl", "Marina Pearl Suites", 265, "4.7", 150),
    ("/tanjongloft", "Tanjong Loft", 159, "3.9", 900),
    ("/quayside", "Quayside Residence", 232, "4.5", 400),
]


def page(name: str, price: int, rating: str, metres: int) -> str:
    return (f"<!doctype html><html><head><meta charset='utf-8'><title>{name} — Official site</title></head><body>"
            f"<header><h1>{name}</h1><nav>Rooms · Dining · Location · Book</nav></header><main>"
            f"<p>Welcome to {name}, a hotel in Singapore's Central Business District.</p>"
            f"<h2>Rooms &amp; rates</h2><p>Deluxe King room: <b>S${price} per night</b> (taxes included), free cancellation until 48 hours before arrival.</p>"
            f"<h2>Guest rating</h2><p>Rated <b>{rating} out of 5</b> by our guests.</p>"
            f"<h2>Location</h2><p>{metres} m walk to Raffles Place MRT station.</p></main></body></html>")


async def main():
    base = C.serve_slow_sites({p: (DELAY, page(n, pr, r, m)) for p, n, pr, r, m in HOTELS})
    async with U.User() as u:
        # ---- 1. a file to download
        a = await u.ask("Write me a packing list for a 3-day trip to Helsinki in November with two kids, as a Markdown file I can download.",
                        label="file")
        md = [f for f in a.files if re.search(r"\.md\b", f["name"], re.I)] or a.files
        check("file: a file card shows in the chat", bool(md), a.files)
        if md:
            pg = u.page
            card = pg.locator(".bubble.filecard").last
            link = card.locator("a[download]").first
            name, text = "", ""
            if await link.count():
                async with pg.expect_download() as dl:
                    await link.click()
                f = await dl.value
                name = f.suggested_filename
                text = open(await f.path(), encoding="utf-8", errors="replace").read()
            else:
                text = (await u.download(md[0]["href"])).decode("utf-8", "replace")
            print(f"INFO downloaded {name!r}: {' '.join(text.split())[:400]!r}")
            check("file: Download saves a Markdown file", name.lower().endswith(".md"), name)
            check("file: the file is a packing list (warm clothes, things for the kids)",
                  re.search(r"jacket|coat|gloves|hat|thermal|boots", text, re.I) and re.search(r"kid|child", text, re.I), text[:400])

        # ---- 2. four slow hotel sites compared
        urls = "\n".join(f"- {base}{p}" for p, *_ in HOTELS)
        a = await u.ask("Compare these five hotels for my 2 nights in Singapore next week — price per night, guest rating and distance to "
                        f"Raffles Place MRT, from their own websites — and recommend one:\n{urls}", label="compare", timeout=900)
        for _, n, pr, r, _m in HOTELS:
            check(f"compare: {n} with its price (S${pr}) and rating ({r})", a.mentions(n) and str(pr) in a.text and r in a.text, a.text[:800])
        check("compare: a recommendation", a.mentions("recommend", "best", "pick", "choose", "go with"), a.text[-400:])
        check(f"compare: the answer came in {a.seconds:.0f} s — before the sites could have been read one after another "
              f"({len(HOTELS)} × {DELAY} s), so they were read side by side", a.seconds < len(HOTELS) * DELAY, a.seconds)


U.setup(vault=False)
U.run(main)
