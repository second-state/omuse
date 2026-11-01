"""City guides for Westerners (skills/<city>/SKILL.md), the part a real-model run cannot check. That every installed guide
answers its own city's questions from its content is checked from the user's side by city_guides_real.py and
city_guides2_real.py (all 15 cities). What stays scripted, and why:
- the files' shape (orientation, entry, money, apps, getting around, business, living, food, sights, timeline, sources,
  the hand-offs to the task skills, the "remember" section, size, dates): a static check of the skill files, not a use case;
- every guide is installed and enabled: the skills list, which a user never reads as a whole;
- a model that calls the browser anyway after loading a guide is refused by the runtime and the refusal is audited: a
  forced security refusal — a real model follows the guide and never makes that call;
- the browser tools are hidden after a guide and back after a booking skill: an exact internal guarantee (the tools offered
  to the model), not visible to the user."""
import os
import re
import sys

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from tests import skills_common as K  # noqa: E402
K.BOT = "travel"   # the Travel bot has this suite's skills (crate::bots)

# the skills folder: next to tests/ in a checkout; in the Docker runner the repository is mounted at /repo
ROOT = next(d for d in (os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))), "skills"), "/repo/skills")
            if os.path.isdir(os.path.join(d, "shenzhen")))
CITIES = {   # skill → (currency pair for market_data, arrival / entry skill it hands off to)
    "beijing": ("USDCNY=X", "cn-arrival-card"), "shanghai": ("USDCNY=X", "cn-arrival-card"),
    "shenzhen": ("USDCNY=X", "cn-arrival-card"), "guangzhou": ("USDCNY=X", "cn-arrival-card"),
    "chengdu": ("USDCNY=X", "cn-arrival-card"), "tokyo": ("USDJPY=X", "jp-visit-japan-web"),
    "osaka": ("USDJPY=X", "jp-visit-japan-web"), "seoul": ("USDKRW=X", "kr-arrival-card"),
    "singapore": ("USDSGD=X", "sg-arrival-card"), "hong-kong": ("USDHKD=X", "arrival-cards"),
    "taipei": ("USDTWD=X", "tw-arrival-card"), "bangkok": ("USDTHB=X", "th-arrival-card"),
    "kuala-lumpur": ("USDMYR=X", "my-arrival-card"), "ho-chi-minh-city": ("USDVND=X", "entry-requirements"),
    "hanoi": ("USDVND=X", "entry-requirements"),
}
SECTIONS = ["## How to use this skill", "## Orientation", "## Money", "## Getting around", "## Food", "## Timeline",
            "## Sources", "## Remember what you learn (for next time)"]
HANDOFFS = ["payments-connectivity-asia", "getting-around-asia", "dining-dietary-translate", "travel-health-emergency",
            "trip-today", "entry-requirements"]

# ---- the files themselves
for skill, (_sym, arrival) in CITIES.items():
    path = os.path.join(ROOT, skill, "SKILL.md")
    if not os.path.exists(path):
        K.check(f"{skill}: SKILL.md exists", False, path)
        continue
    txt = open(path, encoding="utf-8").read()
    front = re.match(r"---\nname: (\S+)\ndescription: ([^\n]+)\nbrowser: off\n---\n", txt)
    K.check(f"{skill}: frontmatter names the skill, describes it for Westerners and turns the browser off",
            front and front.group(1) == skill and "Westerners" in front.group(2) and "use for any question" in front.group(2),
            txt[:300])
    missing = [s for s in SECTIONS if s not in txt]
    K.check(f"{skill}: all sections present (entry, apps, business, living, sights too)", not missing
            and re.search(r"^## (Entering|Getting in)", txt, re.M) and re.search(r"^## .*(apps|online)", txt, re.M | re.I)
            and re.search(r"^## .*(Business|business)", txt, re.M) and "## Living here" in txt and re.search(r"^## What to see", txt, re.M),
            missing)
    K.check(f"{skill}: hands off to the task skills, incl. {arrival}", all(h in txt for h in HANDOFFS + [arrival]),
            [h for h in HANDOFFS + [arrival] if h not in txt])
    K.check(f"{skill}: no browser use — answers from the sheet and data tools",
            "**No browser.**" in txt and "market_data" in txt and not re.search(r"browser_(search|read|navigate|click)", txt),
            re.findall(r"browser_\w+", txt))
    K.check(f"{skill}: facts are dated (2025/2026) and the size is a briefing (10–26 KB)",
            "2026" in txt and 10_000 <= len(txt.encode()) <= 26_000, len(txt.encode()))
    K.check(f"{skill}: secrets stay in the Vault", "Never store passport, card or ID numbers" in txt)

# ---- installed; a browser call after a guide is refused
K.setup()
installed = {s["name"] for s in K.c.get(K.B + "/api/skills").json().get("skills", [])}
K.check("every city guide is installed and enabled", set(CITIES) <= installed, sorted(set(CITIES) - installed))
# a model that calls the browser anyway after loading a guide: refused by the runtime, audited, the answer still comes
t = K.run("CITYGUIDE tokyo USDJPY=X TRYBROWSER how do I get around Tokyo?")
bs = K.results(t, "browser_read")
K.check("browser call after a guide: refused by the runtime (skill policy), nothing fetched",
        t["status"] == "COMPLETED" and bs and bs[0].get("ok") is False and "browser is off" in str(bs[0].get("preview") or bs[0].get("error")),
        bs)
rows = K.c.get(K.S + "/audit?limit=300", headers=K.H).json()["events"]
K.check("the refusal is in the audit log as a skill policy decision",
        any(r.get("resource") == "skill_policy" and r.get("decision") == "DENY" for r in rows),
        [(r.get("action"), r.get("resource"), r.get("decision")) for r in rows][:15])
# loading a skill made for websites after the guide brings the browser back (a question about it, not a request to
# search or book: a search or a booking would be run by the booking machine, with no model turn to look at the tools)
t = K.run("CITYLIFT what do Tokyo restaurants expect about tipping, after reading the guide")
K.check("browser tools: hidden after the guide, back after a booking skill",
        "LIFT after_guide=False after_booking=True" in (t.get("result") or ""), t.get("result"))
K.finish()
