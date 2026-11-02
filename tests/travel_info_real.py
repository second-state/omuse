"""Entry rules, getting around and paying in Asia with a real model on the real web, from the user's point of view
(tests/user_common.py, tests/travel2_common.py). A real-data suite (run_in_docker.sh REAL_DATA_SUITES): the web search is
the real one and the official sites are the real ones (Thailand's e-Visa and TDAC sites, ICA, JR East), market data is
Yahoo's. The checks judge what a careful answer from those live pages must carry, not fixed figures that change.

1. entry — the profile says the user is Australian. "Bangkok next week, then Singapore: what do I need to enter each
   country? Set reminders for what I must file." Checked: the block gives Thailand's visa-free stay with its number of
   days; the answer links the TDAC's official site (tdac.immigration.go.th) and the SG Arrival Card's (ica.gov.sg); the
   Automations page lists the reminders.
2. narita — "landing at Narita T1 at 7 tomorrow with two big suitcases: how to Tokyo Station, times and price?" Checked:
   the Narita Express with at least two departure times and a yen fare, named from the operator (JR East).
3. pay — "Tokyo and Shanghai next month, how do I pay and stay online; I think in SGD". Checked: today's SGD → JPY and
   SGD → CNY rates in the block (within the real range, with where they come from).
No request asks the user to take over a browser. One TIMING line per request.

    OMUSE_E2E_ENV_FILE=~/.config/omuse/e2e.env bash tests/run_in_docker.sh travel_info_real"""
import datetime as dt
import os
import re
import sys

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from tests import claims_common as C  # noqa: E402
from tests import travel2_common as T2  # noqa: E402
from tests import user_common as U  # noqa: E402
from tests.user_common import check  # noqa: E402

ARRIVE = dt.date.today() + dt.timedelta(days=7)
ON = ARRIVE + dt.timedelta(days=4)


async def entry(u: U.User) -> None:
    await u.open_bot("Travel")
    a = await u.ask(f"I'm flying to Bangkok on {T2.day(ARRIVE)} and then on to Singapore on {T2.day(ON)}. What do I need to enter each "
                    "country? Please set reminders for anything I have to file before I arrive.", label="entry", new_chat=False)
    T2.answered("entry", a)
    links = " ".join(a.links) + " " + a.text
    check("entry: the block code appends after the answer gives Thailand's visa-free stay with its number of days (official table)",
          re.search(r"Visa-free stay in Thailand[^\n]*: up to \d{2,3} days", a.block) is not None, (a.block, a.text[:300]))
    check("entry: Thailand — the TDAC's official link", "tdac.immigration.go.th" in links, (a.links, a.text[:1500]))
    check("entry: Singapore — the SG Arrival Card's official link", "ica.gov.sg" in links, (a.links, a.text[:1500]))
    autos = await C.page_text(u, "schedules")
    print(f"INFO Automations page: {' '.join(autos.split())[:800]}")
    check("entry: the Automations page lists a reminder for the Thai arrival card (TDAC)", re.search(r"TDAC|Thai", autos), autos[:600])
    check("entry: … and one for the SG Arrival Card", re.search(r"SG Arrival|SGAC|Singapore", autos), autos[:600])


async def narita(u: U.User) -> None:
    await u.open_bot("Travel")
    a = await u.ask("I land at Narita Terminal 1 tomorrow at about 7 in the morning with two big suitcases. How do I get to Tokyo Station — "
                    "what time are the trains and how much does it cost?", label="narita", new_chat=False)
    T2.answered("narita", a)
    line = next((x for x in a.block.splitlines() if re.search(r"Narita Express|N'?EX", x)), "")
    times = re.findall(r"\b(?:[01]?\d|2[0-3]):[0-5]\d\b", line.split(":", 1)[-1])
    check("narita: the block code appends after the answer gives the Narita Express with at least two departure times (JR East's timetable)",
          len(set(times)) >= 2, (times, a.block))
    check("narita: a yen fare, from the operator (JR East)", re.search(r"(¥|JPY|yen)\s?\d[\d,]{2,}|\d[\d,]{2,}\s?(yen|JPY)", a.text, re.I)
          and re.search(r"JR East|jreast", a.text + " ".join(a.links), re.I), (a.links, a.text[:1200]))


async def pay(u: U.User) -> None:
    await u.open_bot("Travel")
    a = await u.ask("I'm going to Tokyo and Shanghai next month. How should I pay for things and stay online in each? "
                    "I think in Singapore dollars — what are the exchange rates?", label="pay", new_chat=False)
    T2.answered("pay", a)
    # today's rates, computed in code (fx_convert) and appended after the answer with the market data's date
    for pair, cur, real in (("SGD → JPY", "JPY", (95, 135)), ("SGD → CNY", "CNY", (4.8, 6.0))):
        line = next((x for x in a.block.splitlines() if f"in {cur}:" in x), "")
        rate = any(real[0] <= v <= real[1] for v in T2.numbers(line))
        check(f"pay: the block code appends after the answer gives today's {pair} rate (within {real[0]}–{real[1]})", rate, (line, a.block))
    check("pay: the block code appends after the answer says where the rates come from and when (market data of …)",
          "market data of" in a.block, a.block)


async def main():
    async with U.User() as u:
        await entry(u)
        await narita(u)
        await pay(u)


U.setup(profile={"name_en": "OLIVIA HARRIS", "nationality": "Australia", "email_personal": "olivia@example.com"})
U.run(main)
