"""tax-refund-shopping skill: the official rule page is READ, the answer quotes the new Japanese procedure from it (refund
at departure from 1 November 2026, terminal before check-in, 90-day window) with the date checked, and a departure-day
reminder is created."""
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from tests import skills_common as K  # noqa: E402
K.BOT = "travel"   # the Travel bot has this suite's skills (crate::bots)

K.setup()
before = {s["name"] for s in K.c.get(K.B + "/api/schedules").json().get("schedules", [])}
t = K.run("TAXREFUND I'm shopping in Tokyo in late November 2026 — how do I get the tax back?")
K.trace(t)
names = K.names(t)
K.check("task completes", t["status"] == "COMPLETED", (t["status"], t.get("error")))
K.check("the tax-refund-shopping skill was loaded", K.loaded(t, "tax-refund-shopping"), names)
K.check("the rule page was read, not navigated", "browser_read" in names and "browser_navigate" not in names, names)
after = {s["name"] for s in K.c.get(K.B + "/api/schedules").json().get("schedules", [])}
K.check("a departure-day reminder was created", "Tax refund before check-in" in after - before, after)
res = t.get("result") or ""
K.check("the answer carries the page's rules: 1 Nov 2026, before check-in, 90 days, all items present",
        "1 November 2026" in res and "before checking in" in res and "90 days" in res and "every item" in res, res[:600])
K.check("the date checked is stated", "2026-10-25" in res or "read today" in res, res[:600])
K.finish()
