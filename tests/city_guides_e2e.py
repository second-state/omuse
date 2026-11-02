"""City guides for Westerners (skills/<city>/SKILL.md), the part a real-model run cannot check. That every installed guide
answers its own city's questions from its content is checked from the user's side by city_guides_real.py and
city_guides2_real.py (all 15 cities). What stays scripted, and why:
- the files' front matter (the skill's name, browser off) and that no guide uses the browser: a static check of the
  skill files, not a use case;
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
# ---- the files themselves
for skill in CITIES:
    path = os.path.join(ROOT, skill, "SKILL.md")
    if not os.path.exists(path):
        K.check(f"{skill}: SKILL.md exists", False, path)
        continue
    txt = open(path, encoding="utf-8").read()
    front = re.match(r"---\nname: (\S+)\ndescription: ([^\n]+)\nbrowser: off\n---\n", txt)
    K.check(f"{skill}: frontmatter names the skill, describes it for Westerners and turns the browser off",
            front and front.group(1) == skill and "Westerners" in front.group(2) and "use for any question" in front.group(2),
            txt[:300])
    K.check(f"{skill}: no browser use — answers from the sheet and data tools",
            "**No browser.**" in txt and "market_data" in txt and not re.search(r"browser_(search|read|navigate|click)", txt),
            re.findall(r"browser_\w+", txt))

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
