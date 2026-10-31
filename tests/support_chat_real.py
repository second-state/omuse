"""Customer-service chat widgets with a real model, from the user's point of view (tests/user_common.py,
tests/shop_common.py). Three companies' sites (tests/pages, http://shop.test:8099) each answer the user's question only
in their chat — the pages themselves do not say it:
  * Lumen Notes' pricing page: a Tidio-style assistant (Lyra) whose launcher and panel live in an open shadow root;
  * Parcelly Home's contact page: a Zendesk-style messaging window in an iframe;
  * BrightFit's membership page: a cross-origin iframe (another host) whose chat bubble has no name and whose message
    box is an unnamed contenteditable — found by looking at the page.
In Main's chat the user asks each question and names the site. Checked: the answer gives what the company's chat
replied (Lumen: the free Starter plan with 3 notebooks and 50 AI summaries a month; Parcelly: furniture in 6–8
working days, assembly S$39; BrightFit: a freeze of up to 2 months a year at S$15 a month).
The third question is asked with "Auto-approve browser actions" off (Settings): the message the agent types into the
chat needs the user's approval, and the dialog shows the user what will be sent (the question about pausing).
One TIMING line per request.

    OMUSE_E2E_ENV_FILE=~/.config/omuse/e2e.env tests/run_in_docker.sh support_chat_real"""
import os
import re
import sys

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from tests import shop_common as S  # noqa: E402
from tests import user_common as U  # noqa: E402
from tests.user_common import check  # noqa: E402


async def main():
    async with U.User() as u:
        # ---- 1. a widget in a shadow root
        a = await u.ask(f"Does Lumen Notes have a free plan, and what's included? Please ask the assistant in the chat on their pricing page: "
                        f"{S.SHOP}/lumen_pricing.html", label="shadow")
        check("shadow: the answer came (no time-out)", bool(a.text.strip()) and not a.timed_out, a.text[:300])
        check("shadow: the chat's reply — a free Starter plan", re.search(r"starter", a.text, re.I) and re.search(r"free", a.text, re.I), a.text[:700])
        check("shadow: … with 3 notebooks and 50 AI summaries a month",
              re.search(r"\b(3|three) notebooks", a.text, re.I) and re.search(r"\b50\b[^.\n]{0,20}summar", a.text, re.I), a.text[:700])

        # ---- 2. a messaging window in an iframe
        a = await u.ask(f"How long does Parcelly Home take to deliver a sofa, and is assembly extra? Ask their support chat: {S.SHOP}/parcelly_contact.html",
                        label="iframe")
        check("iframe: the answer came (no time-out)", bool(a.text.strip()) and not a.timed_out, a.text[:300])
        check("iframe: the chat's reply — 6–8 working days", re.search(r"\b6\s?(–|-|to)\s?8\b[^.\n]{0,20}(working|business)? ?days", a.text, re.I),
              a.text[:700])
        check("iframe: … and assembly at S$39", re.search(r"(S\$|SGD)\s?39\b", a.text) and re.search(r"assembl", a.text, re.I), a.text[:700])

        # ---- 3. an unnamed bubble in a cross-origin iframe, with auto-approve off: the user approves the message
        await S.set_auto(u, False)
        a = await S.ask(u, "Can I pause my BrightFit membership while I'm travelling for a month? Please ask the chat bubble on their site: "
                           f"{S.SHOP}/brightfit_plans.html", label="bubble")
        await S.set_auto(u, True)
        check("bubble: the answer came (no time-out)", bool(a.text.strip()) and not a.timed_out, a.text[:300])
        check("bubble: the chat's reply — a freeze of up to 2 months a year", re.search(r"freez", a.text, re.I)
              and re.search(r"\b(2|two) months", a.text, re.I), a.text[:700])
        check("bubble: … at S$15 a month", re.search(r"(S\$|SGD)\s?15\b", a.text), a.text[:700])
        check("bubble: with auto-approve off the user was asked to approve the message to the chat", bool(a.approvals), a.approvals)
        check("bubble: … and the dialog shows what will be sent (the question about pausing)",
              any(re.search(r"pause|freez|hold|suspend|travel", x, re.I) for x in a.approvals), a.approvals)


U.setup(mail=[], vault=False)
U.run(main)
