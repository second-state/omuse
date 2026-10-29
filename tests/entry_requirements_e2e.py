"""entry-requirements skill: the nationality from the profile, the official rule pages READ (browser_read, no clicking
around), a reminder created for the arrival-card window, and an answer that carries the date checked."""
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from tests import skills_common as K  # noqa: E402
K.BOT = "travel"   # the Travel bot has this suite's skills (crate::bots)

K.setup()
K.set_profile(nationality="Australia")
before = {s["name"] for s in K.c.get(K.B + "/api/schedules").json().get("schedules", [])}
t = K.run("ENTRYREQ what do I need to enter Thailand and then Singapore next week?")
K.trace(t)
names = K.names(t)
K.check("task completes", t["status"] == "COMPLETED", (t["status"], t.get("error")))
K.check("the entry-requirements skill was loaded", K.loaded(t, "entry-requirements"), names)
K.check("the nationality came from the profile", "profile_get" in names and "Australia" in str(K.results(t, "profile_get")), K.results(t, "profile_get"))
K.check("official pages were read, not clicked through", "browser_read" in names and "browser_navigate" not in names, names)
page = str(K.results(t, "browser_read"))
K.check("the TDAC rule page was read (3-day window)", "within 3 days before arrival" in page, page[:400])
after = {s["name"] for s in K.c.get(K.B + "/api/schedules").json().get("schedules", [])}
K.check("a reminder for the arrival card was created", "File the Thailand TDAC" in after - before, after)
res = t.get("result") or ""
K.check("the answer is a table with the window, the official link and the date checked",
        "TDAC" in res and "within 3 days" in res and "tdac.immigration.go.th" in res and "Date checked" in res, res[:800])
K.check("Singapore is routed to the sg-arrival-card skill", "sg-arrival-card" in res, res[:800])
K.finish()
