"""Web research with a real model, from the user's point of view (tests/user_common.py). In Main's chat the user asks
for the pros and cons of heat pumps versus gas boilers for a small UK house, with links to the sources. A real-data suite
(run_in_docker.sh REAL_DATA_SUITES): the web search and the pages are the real ones. What the user sees: an answer that cites its sources as links that open — none of them is a dead link (a 404 or a site that does not exist), which
would mean a source the agent never opened. One TIMING line.

    OMUSE_E2E_ENV_FILE=~/.config/omuse/e2e.env tests/run_in_docker.sh research_real"""
import os
import re
import sys

import httpx

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from tests import user_common as U  # noqa: E402
from tests.user_common import check  # noqa: E402

URL = re.compile(r"https?://[^\s)\]>\"'`,]+")


def dead(url: str) -> str:
    """Why the link is dead for the user ('' when the page is there, even if it shows a bot check or asks to sign in)."""
    try:
        r = httpx.get(url, headers={"User-Agent": U.UA}, follow_redirects=True, timeout=30)
    except httpx.ConnectError as e:
        return f"no such site: {str(e)[:100]}"
    except httpx.HTTPError as e:
        return f"did not load: {str(e)[:100]}"
    return f"{r.status_code}" if r.status_code in (404, 410) else ""


async def main():
    async with U.User() as u:
        # (one literal: this request is also Main's example in the UI — bots/main.md, checked by bots.rs tests)
        a = await u.ask("Research the pros and cons of heat pumps versus gas boilers for heating a small house in the UK. Search the web and give me a short summary with links to your sources.",
                        label="heatpump")
        cited = sorted({x.rstrip(".;:") for x in a.links + URL.findall(a.text) if "127.0.0.1:8080" not in x})
        print(f"INFO cited {cited}")
        check("heatpump: the answer cites its sources as links", bool(cited), a.text[-600:])
        status = {x: dead(x) for x in cited}
        for x, why in status.items():
            ok, title = U.opens(x)
            print(f"INFO source {x}: {'dead ' + why if why else 'there'}; opens for a browser: {ok} ({title})")
        check("heatpump: no cited source is a dead link (404 or no such site)", not [x for x, w in status.items() if w],
              {x: w for x, w in status.items() if w})
        # (no check that third-party sources open: news and research sites block automated requests with 403 / Cloudflare,
        # which is not our code)


U.setup()
U.run(main)
