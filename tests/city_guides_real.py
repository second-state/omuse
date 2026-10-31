"""City guides with a real model, from the user's point of view (see tests/user_common.py): the user opens the Travel
bot from the menu and asks three questions a Western visitor or expat asks — where expat families live in Shanghai and
what a flat rents for, how to get from Haneda to Shinjuku and whether cash is needed in Tokyo, and where to stay and how
to get around in Hanoi plus what 300 US dollars are in dong right now. Each answer carries the guide's facts (the
neighbourhoods or routes it names, the money it talks in) and comes without the user being asked anything — no
approval dialog, no request to take over a browser; the Hanoi answer converts the 300 dollars into an amount of dong
(the market data in this test world is a year old — tests/fake_apps.py's Yahoo stand-in — so a model may rightly fall
back to the guide's rate; the exact rate is not pinned). Nothing inside the system is looked at.
One TIMING line per question (from Enter to the answer).

    OMUSE_E2E_ENV_FILE=~/.config/omuse/e2e.env bash tests/run_in_docker.sh city_guides_real"""
import os
import re
import sys

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from tests import user_common as U  # noqa: E402
from tests.user_common import check  # noqa: E402

CASES = [
    ("shanghai", "I'm moving to Shanghai for work next month with my wife and two kids. Which neighbourhoods do expat "
                 "families usually live in, and roughly what does a family flat rent for?",
     ["Pudong", "Jinqiao", "Gubei", "Former French Concession", "Minhang", "Hongqiao"], ["¥", "RMB", "yuan", "CNY"]),
    ("tokyo", "First time in Tokyo next week, flying in from the US to Haneda. How do I get to Shinjuku, and will I "
              "need cash or can I use my credit card everywhere?",
     ["Keikyu", "Monorail", "Limousine", "Shinagawa", "Hamamatsucho"], ["cash", "Suica", "PASMO"]),
    # (visa questions belong to entry-requirements, which reads the official page on purpose)
    ("hanoi", "I'm an American spending a week in Hanoi. Which area should I stay in, how do I get around the city, "
              "and how much is 300 US dollars in dong right now?",
     ["Hoàn Kiếm", "Hoan Kiem", "Old Quarter", "Tây Hồ", "Tay Ho", "Ba Đình"], ["₫", "VND", "dong"]),
]
# 300 US dollars in dong: millions ("₫7,845,000", "7.8 million dong", "VND 7,800,000–8,100,000", "~7.8M VND")
IN_DONG = re.compile(r"(₫|VND|đồng|dong)\s?\d{1,2}[,.]\d{3}[,.]\d{3}|\d{1,2}[,.]\d{3}[,.]\d{3}\s?(₫|VND|đồng|dong)|\d{1,2}(\.\d+)?\s?(million|M)\b", re.I)


async def main():
    async with U.User() as u:
        for name, prompt, places, money in CASES:
            await u.open_bot("Travel")
            a = await u.ask(prompt, label=name, new_chat=False)
            check(f"{name}: the answer came (no time-out)", bool(a.text.strip()) and not a.timed_out, a.text[:200])
            check(f"{name}: no approval dialog and no request to take over a browser", not a.approvals and not a.takeovers, (a.approvals, a.takeovers))
            check(f"{name}: the answer carries the guide's facts", a.mentions(*places) and a.mentions(*money), a.text[:600])
            if name == "hanoi":
                check("hanoi: the answer converts the 300 US dollars into dong (millions)", bool(IN_DONG.search(a.text)), a.text[-600:])


U.setup()
U.run(main)
