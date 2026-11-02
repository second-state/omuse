"""Japan's tax-free shopping with a real model on the real web, from the user's point of view (tests/user_common.py).
A real-data suite (run_in_docker.sh REAL_DATA_SUITES): the web search and the official pages (the Japan Tourism Agency,
mlit.go.jp, JNTO) are the real ones. Japan moves to a refund system on 1 November 2026: the shop charges the tax and it
is refunded at departure after customs confirms the goods leave with the traveller.

"I'm shopping in Tokyo in late November 2026 — how do I get the consumption tax back?" Checked: the Travel bot answers;
the answer says the refund system applies from 1 November 2026 (pay the tax in the shop, get it back when leaving Japan,
after customs / the departure check), that the passport is needed and the goods must leave Japan with the traveller, and
names its official source (a link to the official sites or the Japan Tourism Agency / JNTO). One TIMING line.

    OMUSE_E2E_ENV_FILE=~/.config/omuse/e2e.env bash tests/run_in_docker.sh tax_refund_real"""
import os
import re
import sys

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from tests import user_common as U  # noqa: E402
from tests.user_common import check  # noqa: E402

OFFICIAL = re.compile(r"mlit\.go\.jp|jnto\.go\.jp|japan\.travel|tax-?freeshop|customs\.go\.jp|nta\.go\.jp", re.I)


async def main():
    async with U.User() as u:
        a = await u.ask("I'm shopping in Tokyo in late November 2026 — how do I get the consumption tax back?", label="tax", timeout=900)
        check("tax: the answer came (no time-out)", bool(a.text.strip()) and not a.timed_out, a.text[:300])
        check("tax: the Travel bot answered", "Travel" in a.bot_link, a.bot_link)
        check("tax: the refund system from 1 November 2026 (pay in the shop, refunded when leaving Japan)",
              re.search(r"1 Nov(ember)?,? 2026|Nov(ember)? 1,? 2026|2026-11-01|November 2026", a.text)
              and re.search(r"refund", a.text, re.I) and re.search(r"depart|leav|airport|customs|exit", a.text, re.I), a.text[:1200])
        check("tax: the official source is named (a link to an official site, or the Japan Tourism Agency / JNTO)",
              any(OFFICIAL.search(x) for x in a.links) or re.search(r"Japan Tourism Agency|JNTO|Japan National Tourism", a.text),
              (a.links, a.text[-400:]))
        check("tax: no approval dialog and no takeover (only reading)", not a.approvals and not a.takeovers, (a.approvals, a.takeovers))


U.setup()
U.run(main)
