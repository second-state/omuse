"""credit-card-picker skill: the user's cards come from the profile (names only), the market table is read from the
skill's own folder with load_skill(file=…), the value per card is computed with calculate, the answer quotes the rate
and the verification date from the catalog, and no browser is used."""
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from tests import skills_common as K  # noqa: E402
K.BOT = "cards"   # the Credit cards bot has this suite's skills (crate::bots)

K.setup()
K.set_profile(**{"custom:credit cards": "Chase Sapphire Reserve; Citi Double Cash", "custom:card market": "US"})
t = K.run("WHICHCARD which card should I use for a USD 300 dinner at a restaurant in New York tonight?")
K.trace(t)
names = K.names(t)
K.check("task completes", t["status"] == "COMPLETED", (t["status"], t.get("error")))
K.check("the credit-card-picker skill was loaded", K.loaded(t, "credit-card-picker"), names)
pg = K.results(t, "profile_get")
K.check("the user's cards came from the profile", pg and "Sapphire Reserve" in str(pg[0].get("preview")), pg)
loads = [x for x in K.calls(t) if x["name"] == "load_skill" and x["args"].get("file")]
K.check("the market table was read from the skill's folder", loads and loads[0]["args"]["file"] == "cards/us.md", loads)
tbl = [r for r in K.results(t, "load_skill") if "Credit cards — US" in str(r.get("preview"))]
K.check("the table came back with the catalog header", tbl and tbl[0].get("ok"), K.results(t, "load_skill"))
K.check("the maths went through calculate", "calculate" in names and K.results(t, "calculate")[0].get("ok"), K.results(t, "calculate"))
K.check("no browser", not any(n.startswith("browser_") for n in names), names)
res = t.get("result") or ""
K.check("the pick is one of the user's cards with the catalog's dining rate", "Sapphire Reserve" in res and "dining 3x" in res, res[:600])
K.check("the answer carries the catalog's verification date and the 'terms change' caveat", "Catalog verified 2026-" in res and "terms change" in res, res[:600])
K.check("no card number anywhere", not any(len(w) >= 15 and w.isdigit() for w in res.replace("-", " ").split()), res[:300])
K.finish()
