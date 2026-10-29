"""City guides for Westerners (skills/<city>/SKILL.md): every guide is installed, has the agreed shape (orientation,
entry, money, apps, getting around, business, living, food, sights, timeline, sources, the hand-offs to the task
skills and the "remember" section), never tells the agent to use the browser, and answers from its own sheet plus a
data tool: the scripted agent loads the guide, takes the exchange rate from market_data and quotes the guide's own
"Getting around" line — no browser tool is called."""
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

# ---- installed, and answering without a browser
K.setup()
installed = {s["name"] for s in K.c.get(K.B + "/api/skills").json().get("skills", [])}
K.check("every city guide is installed and enabled", set(CITIES) <= installed, sorted(set(CITIES) - installed))
for skill, (sym, _a) in CITIES.items():
    t = K.run(f"CITYGUIDE {skill} {sym} I'm a first-time visitor from the US: how do I get around, and what's the exchange rate?")
    names = K.names(t)
    res = t.get("result") or ""
    sheet = open(os.path.join(ROOT, skill, "SKILL.md"), encoding="utf-8").read() if os.path.exists(os.path.join(ROOT, skill, "SKILL.md")) else ""
    line = re.search(r"Getting around: (.{20,60})", res)
    md = K.results(t, "market_data")
    rate = re.search(re.escape(f"{sym[:3]}/{sym[3:6]}") + r"[^0-9]*([0-9][0-9,]*\.[0-9]+)", str(md[0].get("preview")) if md else "")
    K.check(f"{skill}: guide loaded, rate from market_data, no browser tool",
            t["status"] == "COMPLETED" and K.loaded(t, skill) and "market_data" in names and not any(n.startswith("browser_") for n in names),
            (t["status"], names))
    K.check(f"{skill}: the answer quotes the guide's own sheet and the market_data rate",
            line and line.group(1)[:30] in sheet and rate and f"FX {sym}: {rate.group(1)}" in res, res[:300])
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
# loading a skill made for websites after the guide brings the browser back
t = K.run("CITYLIFT find me a table in Tokyo after reading the guide")
K.check("browser tools: hidden after the guide, back after a booking skill",
        "LIFT after_guide=False after_booking=True" in (t.get("result") or ""), t.get("result"))
K.finish()
