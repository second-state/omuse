"""The Money bot's questions with a real model, from the user's point of view (tests/user_common.py). A real-data suite
(run_in_docker.sh REAL_DATA_SUITES): the answers come from code's checked tables (tax_refund_rules, payment_options) and
the HK departure tax refund's rule (apdt_refund with `file: false`); no form is filled. Each request is asked in Main's
chat, as the user would, and Main hands it to Money. Smoke checks on what code appends (the `show` block), never on the
model's wording:

1. tokyo — the Money bot's example "I'm shopping in Tokyo in late November 2026 — how do I get the consumption tax
   back?": Money answers; the block has Japan's scheme (the refund at departure from 1 November 2026) and the official
   Japan Customs link.
2. hk — the example "I flew out of Hong Kong last week — can I get the airport departure tax back?": Money answers with
   the rule — the block has "Apply by" and the refund platform's link; nothing is filed (no takeover, no approval).
3. china — "How do I pay for things in Shanghai as a foreigner? I have a US Visa card." Money answers; the block has
   Alipay's and WeChat's official store links and the 3% fee rule.
One TIMING line per request.

    OMUSE_E2E_ENV_FILE=~/.config/omuse/e2e.env bash tests/run_in_docker.sh money_real"""
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from tests import user_common as U  # noqa: E402
from tests.user_common import check  # noqa: E402


def by_money(a: U.Answer, step: str) -> None:
    check(f"{step}: the answer came (no time-out)", bool(a.text.strip()) and not a.timed_out, a.text[:300])
    check(f"{step}: the Money bot answered", "Money" in a.bot_link, a.bot_link)
    check(f"{step}: nothing filed — no approval dialog and no takeover", not a.approvals and not a.takeovers,
          (a.approvals, a.takeovers))


async def main():
    async with U.User() as u:
        a = await u.ask("I'm shopping in Tokyo in late November 2026 — how do I get the consumption tax back?",
                        label="tokyo", timeout=900)
        by_money(a, "tokyo")
        check("tokyo: the block gives Japan's refund at departure from 1 November 2026 and Japan Customs' page",
              "1 November 2026" in a.block and any("customs.go.jp" in x for x in a.block_links), (a.block[:800], a.block_links))

        a = await u.ask("I flew out of Hong Kong last week — can I get the airport departure tax back?", label="hk", timeout=900)
        by_money(a, "hk")
        check("hk: the block gives the deadline and the refund platform's link",
              "Apply by" in a.block and any("apdtrefund.hk" in x for x in a.block_links), (a.block[:800], a.block_links))

        a = await u.ask("How do I pay for things in Shanghai as a foreigner? I have a US Visa card.", label="china", timeout=900)
        by_money(a, "china")
        check("china: the block links Alipay's and WeChat's official app pages and gives the 3% fee rule",
              any("id333206289" in x or "AlipayGphone" in x for x in a.block_links)
              and any("id414478124" in x or "com.tencent.mm" in x for x in a.block_links) and "3%" in a.block,
              (a.block[:800], a.block_links))


U.setup()
U.run(main)
