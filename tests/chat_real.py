"""A file handed over in the chat, with a real model, from the user's point of view (tests/user_common.py): "Write me a
packing list for a 3-day trip to Helsinki in November with two kids, as a Markdown file I can download." What the user
sees: a file card in the chat; clicking Download saves a .md file (its wording is the model's and is not checked).
One TIMING line.

    OMUSE_E2E_ENV_FILE=~/.config/omuse/e2e.env tests/run_in_docker.sh chat_real"""
import os
import re
import sys

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from tests import user_common as U  # noqa: E402
from tests.user_common import check  # noqa: E402


async def main():
    async with U.User() as u:
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


U.setup(vault=False)
U.run(main)
