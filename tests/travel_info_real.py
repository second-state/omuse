"""Entry rules, getting around and paying in Asia with a real model on the real web, from the user's point of view
(tests/user_common.py, tests/travel2_common.py). A real-data suite (run_in_docker.sh REAL_DATA_SUITES): the web search is
the real one and the official sites are the real ones (Thailand's e-Visa and TDAC sites, ICA, JR East), market data is
Yahoo's. The checks judge what a careful answer from those live pages must carry, not fixed figures that change.

1. entry — the profile says the user is Australian. "Bangkok next week, then Singapore: what do I need to enter each
   country? Set reminders for what I must file." Checked: the answer is for an Australian passport; Thailand: a
   visa-free stay with its number of days, the TDAC within the days before arrival with its official link (tdac.immigration.go.th);
   Singapore: the SG Arrival Card with its official link (ica.gov.sg); the Automations page lists the reminders.
2. narita — "landing at Narita T1 at 7 tomorrow with two big suitcases: how to Tokyo Station, times and price?" Checked:
   the Narita Express with at least two departure times and a yen fare, named from the operator (JR East), and a word on
   the suitcases.
3. pay — "Tokyo and Shanghai next month, how do I pay and stay online; I think in SGD". Checked: the per-country
   checklist (Suica / PASMO for Japan, Alipay or WeChat Pay set up before leaving for China, an eSIM, decline dynamic
   currency conversion) and today's SGD → JPY and SGD → CNY rates (within the real range, with where they come from).
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
    check("entry: the answer is for the user's nationality from the profile (Australian)", re.search(r"Australia", a.text), a.text[:900])
    check("entry: Thailand — a visa-free stay with its number of days",
          re.search(r"visa[- ]?(free|exempt)|exemption|without a visa|no visa", a.text, re.I) and re.search(r"\b\d{2,3}[ -]days?\b", a.text), a.text[:1500])
    check("entry: Thailand — the TDAC (within 3 days before arrival: the window or the filing date) with the official link",
          "TDAC" in a.text and (re.search(r"(3|three)[ -]days?", a.text) or T2.date_in(a.text, ARRIVE - dt.timedelta(days=3)))
          and "tdac.immigration.go.th" in links, (a.links, a.text[:1500]))
    check("entry: Singapore — the SG Arrival Card with the official link",
          re.search(r"SG Arrival Card|SGAC", a.text) and "ica.gov.sg" in links, (a.links, a.text[:1500]))
    autos = await C.page_text(u, "schedules")
    print(f"INFO Automations page: {' '.join(autos.split())[:800]}")
    check("entry: the Automations page lists a reminder for the Thai arrival card (TDAC)", re.search(r"TDAC|Thai", autos), autos[:600])
    check("entry: … and one for the SG Arrival Card", re.search(r"SG Arrival|SGAC|Singapore", autos), autos[:600])


async def narita(u: U.User) -> None:
    await u.open_bot("Travel")
    a = await u.ask("I land at Narita Terminal 1 tomorrow at about 7 in the morning with two big suitcases. How do I get to Tokyo Station — "
                    "what time are the trains and how much does it cost?", label="narita", new_chat=False)
    T2.answered("narita", a)
    times = re.findall(r"\b(?:[01]?\d|2[0-3]):[0-5]\d\b", a.text)
    check("narita: the Narita Express with at least two departure times", re.search(r"Narita Express|N'?EX", a.text) and len(set(times)) >= 2,
          (times, a.text[:1200]))
    check("narita: a yen fare, from the operator (JR East)", re.search(r"(¥|JPY|yen)\s?\d[\d,]{2,}|\d[\d,]{2,}\s?(yen|JPY)", a.text, re.I)
          and re.search(r"JR East|jreast", a.text + " ".join(a.links), re.I), (a.links, a.text[:1200]))
    check("narita: the answer speaks to the two big suitcases", re.search(r"suitcase|luggage|baggage", a.text, re.I), a.text[:1200])


async def pay(u: U.User) -> None:
    await u.open_bot("Travel")
    a = await u.ask("I'm going to Tokyo and Shanghai next month. How should I pay for things and stay online in each? "
                    "I think in Singapore dollars — what are the exchange rates?", label="pay", new_chat=False)
    T2.answered("pay", a)
    check("pay: Japan — a transit IC card (Suica / PASMO / ICOCA)", re.search(r"Suica|PASMO|ICOCA", a.text), a.text[:1500])
    check("pay: China — Alipay or WeChat Pay, set up before leaving",
          re.search(r"Alipay|WeChat Pay", a.text) and re.search(r"before (you )?(leav|depart|go|travel|fly|arriv)|at home|in advance|ahead", a.text, re.I),
          a.text[:1500])
    check("pay: staying online (eSIM / roaming / SIM)", re.search(r"eSIM|roaming|SIM card", a.text, re.I), a.text[:1500])
    check("pay: decline dynamic currency conversion (pay in the local currency)",
          re.search(r"dynamic currency conversion|\bDCC\b|in (the )?local currency|decline (the )?conversion|in yen\b|in yuan\b|in (JPY|CNY|RMB)\b",
                    a.text, re.I), a.text[:1500])
    # today's rates, live: within the real range and named with where they come from (market data, Yahoo, Wise, XE…)
    nums = T2.numbers(a.text)
    for pair, cur, word, real in (("SGD → JPY", "JPY", "yen", (95, 135)), ("SGD → CNY", "CNY", "yuan", (4.8, 6.0))):
        rate = any(real[0] <= v <= real[1] for v in nums) and re.search(rf"{cur}|{word}|RMB", a.text, re.I)
        check(f"pay: today's {pair} rate (within {real[0]}–{real[1]})", bool(rate), (nums[:20], a.text[:900]))
    check("pay: the rates say where they come from (market data, Yahoo, Wise, XE …) or when",
          re.search(r"market data|Yahoo|Wise|XE\b|Reuters|Bloomberg|Google Finance|OANDA|MAS|mid-market|as of|today", a.text, re.I), a.text[:900])


async def main():
    async with U.User() as u:
        await entry(u)
        await narita(u)
        await pay(u)


U.setup(profile={"name_en": "OLIVIA HARRIS", "nationality": "Australia", "email_personal": "olivia@example.com"})
U.run(main)
