"""Emergencies abroad with a real model, from the user's point of view (tests/user_common.py, tests/claims_common.py).
The user is Australian, in Tokyo. A real-data suite (run_in_docker.sh REAL_DATA_SUITES): the web search and the
Australian Embassy's pages (https://japan.embassy.gov.au) are the real ones. The mailbox has the user's travel insurance policy (SafeTrip Australia
ST-AU-448120, 24-hour emergency assistance +61 2 8055 1700; lost passport: replacement and extra nights up to AUD 1,500).

1. passport — "I lost my passport in Tokyo this morning — what do I do?" Checked: the Insurance & claims bot answers;
   numbered steps; the police report, passport photos, the embassy's opening hours and its number or the 24-hour
   Consular Emergency Centre (+61 2 6261 3305 / +81 3 5232 4111 on the embassy's site); the insurer's assistance line
   from the mail; and every international phone number in the answer is one the mail, the profile, the embassy's
   numbers or a page the answer links gives (the test reads those linked pages itself).
2. illness — "I've had a high fever since last night in my Tokyo hotel — where can I see an English-speaking doctor, and
   will my insurance pay?" Checked: the same bot; an English-speaking clinic or hospital in Tokyo with its number; the
   insurer's assistance line (call before paying large bills); and again only phone numbers from those sources
   (Japan's 119 is a local number and allowed).
One TIMING line per request.

    OMUSE_E2E_ENV_FILE=~/.config/omuse/e2e.env bash tests/run_in_docker.sh emergency_real"""
import os
import re
import sys

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from tests import claims_common as C  # noqa: E402
from tests import user_common as U  # noqa: E402
from tests.user_common import check  # noqa: E402

ASSIST = "+61 2 8055 1700"
MAIL = [("SafeTrip Australia <policy@safetrip.example>", "Your SafeTrip travel insurance — policy ST-AU-448120",
         "Dear Lucas Lu,\n\nPolicy ST-AU-448120 covers your trip to Japan this month (Comprehensive plan).\n"
         f"24-hour emergency assistance (call before you pay any hospital bill): {ASSIST}.\n"
         "Medical expenses overseas: unlimited. Lost or stolen passport: replacement costs and extra accommodation up to AUD 1,500 — "
         "a police report is required.\n\nSafeTrip Australia", 9)]
# the phone numbers the user's own sources give (profile, the policy mail) and the embassy's (its site's contact page)
KNOWN = set(C.phones(f"{ASSIST} +61 400 000 000 +61 2 6261 3305 +81 3 5232 4111"))


def linked_numbers(links: list[str]) -> set[str]:
    """The phone numbers on the pages the answer links, as the user would read them (the live pages)."""
    out = set()
    for url in links[:8]:
        try:
            r = U.c.get(url, follow_redirects=True, timeout=30, headers={"User-Agent": U.UA})
            out |= set(C.phones(r.text))
        except Exception:   # noqa: BLE001
            pass
    return out


def unknown_numbers(text: str, links: list[str]) -> list[str]:
    allowed = KNOWN | linked_numbers(links)
    return [n for n in C.phones(text) if n not in allowed]


async def main():
    async with U.User() as u:
        a = await u.ask("I lost my passport in Tokyo this morning — what do I do?", label="passport", timeout=900)
        check("passport: the answer came (no time-out)", bool(a.text.strip()) and not a.timed_out, a.text[:300])
        check("passport: the Insurance & claims bot answered", "nsurance" in a.bot_link, a.bot_link)
        steps = await C.numbered_steps(u)
        check("passport: numbered steps (at least 3)", steps >= 3, (steps, a.text[:600]))
        check("passport: the police report comes into it", re.search(r"police", a.text, re.I), a.text[:600])
        check("passport: the embassy's documents (passport photos)", re.search(r"photo", a.text, re.I), a.text[:1200])
        check("passport: the embassy's opening / consular hours",
              re.search(r"\d{1,2}(:\d\d)?\s?(am|pm)?\s?[–-]\s?\d{1,2}(:\d\d)?\s?(am|pm)?|hours", a.text, re.I), a.text[:1200])
        check("passport: the embassy's emergency line or number from its page",
              # the local form a user dials in Japan (03-5232-4111) is the same line as +81 3 5232 4111
              {"62613305", "52324111"} & {p[-8:] for p in C.phones(a.text)}, C.phones(a.text))
        check("passport: the insurer's 24-hour assistance line from the policy mail", "61280551700" in C.phones(a.text), C.phones(a.text))
        check("passport: every international phone number in the answer comes from the mail or the embassy's pages",
              not unknown_numbers(a.text, a.links), unknown_numbers(a.text, a.links))

        a = await u.ask("I've had a high fever since last night in my hotel in Tokyo. Where can I see an English-speaking doctor, "
                        "and will my insurance pay for it?", label="illness", timeout=900)
        check("illness: the answer came (no time-out)", bool(a.text.strip()) and not a.timed_out, a.text[:300])
        check("illness: the Insurance & claims bot answered", "nsurance" in a.bot_link, a.bot_link)
        check("illness: an English-speaking clinic or hospital in Tokyo",
              re.search(r"(clinic|hospital|medical cent(er|re))", a.text, re.I) and re.search(r"English", a.text, re.I), a.text[:1200])
        check("illness: with a Tokyo phone number (+81 3 … / 03-…)",
              any(n.startswith("813") for n in C.phones(a.text)) or re.search(r"\b03[- ]\d{4}[- ]\d{4}\b", a.text), C.phones(a.text))
        check("illness: the insurer's 24-hour assistance line from the policy mail", "61280551700" in C.phones(a.text), C.phones(a.text))
        check("illness: every international phone number in the answer comes from the mail or the embassy's pages",
              not unknown_numbers(a.text, a.links), unknown_numbers(a.text, a.links))


U.setup(mail=MAIL, profile={"name_en": "LUCAS LU", "birthday": "1985-04-12", "nationality": "Australia",
                            "email_personal": "lucas@example.com", "phone": "+61 400 000 000"})
U.run(main)
