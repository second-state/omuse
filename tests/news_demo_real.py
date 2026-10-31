"""One-off demo (not in the gate): the user asks Main for a newspaper-style news report on PCCW's stock, with the real
search engines in the browser; the published HTML and PDF are copied to OMUSE_E2E_OUT/artifacts.

    OMUSE_E2E_ENV_FILE=~/.config/omuse/e2e.env OMUSE_E2E_OUT=<dir> bash tests/run_in_docker.sh news_demo_real"""
# run_local: OMUSE_E2E_SEARCH_ENGINES=
import os
import shutil
import sys

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from tests import user_common as U  # noqa: E402
from tests.user_common import check  # noqa: E402

OUT = "/repo-out" if os.path.isdir("/repo-out") else ""
WS = "/tmp/claude-0/persona-test/workspace"


async def main():
    async with U.User() as u:
        a = await u.ask("Research the latest news on PCCW stock (0008.HK) and make me a newspaper-style report.",
                        label="pccw", timeout=1500)
        # the report's links are relative (/artifacts/…): read every link of the answer
        a.links = await u.page.locator(".msg.assistant:not(.live) .bubble").last.evaluate(
            "b => [...b.querySelectorAll('a[href]')].map(x => x.href)")
        print(f"INFO answer: {a.text}")
        print(f"INFO links: {a.links}  files: {a.files}")
        check("the answer came", bool(a.text.strip()) and not a.timed_out, a.text[:300])
        check("the answer links the published HTML and PDF", any("/artifacts/" in x and x.endswith(".html") for x in a.links)
              and any("/artifacts/" in x and x.endswith(".pdf") for x in a.links), a.links)
    if OUT and os.path.isdir(os.path.join(WS, "artifacts")):
        shutil.copytree(os.path.join(WS, "artifacts"), os.path.join(OUT, "artifacts"), dirs_exist_ok=True)


U.setup(mail=[])
U.run(main)
