"""getting-around-asia skill: the operator's timetable page is READ (one browser_read, no clicking), and the answer's
times and fares are the page's, in a per-leg table with a recommendation."""
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from tests import skills_common as K  # noqa: E402
K.BOT = "travel"   # the Travel bot has this suite's skills (crate::bots)

K.setup()
t = K.run("GETAROUND how do I get from Narita to Tokyo station tomorrow morning with two big suitcases?")
K.trace(t)
names = K.names(t)
K.check("task completes", t["status"] == "COMPLETED", (t["status"], t.get("error")))
K.check("the getting-around-asia skill was loaded", K.loaded(t, "getting-around-asia"), names)
K.check("the timetable was read, not clicked through", names.count("browser_read") == 1 and "browser_navigate" not in names and "browser_click" not in names, names)
page = str(K.results(t, "browser_read"))
K.check("the operator page came back", "Narita Express" in page and "3,250" in page, page[:300])
res = t.get("result") or ""
K.check("the answer's times and fares are the page's", "07:44" in res and "¥3,250" in res and "08:41" in res, res[:600])
K.check("a recommendation fit for big suitcases", "Narita Express" in res and "suitcase" in res, res[:600])
K.check("no approval was needed", not t["_approvals"], t["_approvals"])
K.finish()
