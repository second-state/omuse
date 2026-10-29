"""travel-health-emergency skill (lost passport): the insurer's assistance line from the policy mail, the embassy's
page READ for hours, documents and its emergency line, the user's files searched for a passport copy, and numbered
steps whose phone numbers all come from those sources."""
import os
import re
import sys

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from tests import skills_common as K  # noqa: E402
K.BOT = "insurance"   # the Insurance & claims bot has this suite's skills (crate::bots)

MAIL = [
    ("TravelSure <policy@travelsure.example>", "Your travel insurance policy TI-2026-77",
     "Policy TI-2026-77. 24-hour assistance: +65 6000 0000. Lost passport: replacement costs and extra nights covered up to SGD 1,000.", 5),
]
K.setup(mail=MAIL)
t = K.run("LOSTPASSPORT I lost my passport in Tokyo this morning — what do I do?")
K.trace(t)
names = K.names(t)
K.check("task completes", t["status"] == "COMPLETED", (t["status"], t.get("error")))
K.check("the travel-health-emergency skill was loaded", K.loaded(t, "travel-health-emergency"), names)
K.check("the policy was found in mail", "gmail_search" in names and "+65 6000 0000" in str(K.results(t, "gmail_search")), K.results(t, "gmail_search"))
K.check("the embassy page was read, not navigated", "browser_read" in names and "browser_navigate" not in names, names)
K.check("the user's files were searched for a passport copy", "memory_search" in names, names)
res = t.get("result") or ""
K.check("numbered steps: police report, embassy, insurer", res.startswith("1.") and "Police report" in res and "Embassy" in res and "assistance" in res, res[:600])
seen = "\n".join(str(r.get("preview")) for r in [e["data"] for e in t["events"] if e["type"] == "tool_result"])
nums = re.findall(r"\+\d[\d ]{6,}\d", res)
K.check("every phone number in the answer comes from a source the agent read", nums and all(n in seen for n in nums), (nums, seen[:300]))
K.check("the embassy's hours and documents are the page's", "09:00–12:00" in res and "police report" in res.lower() and "two photos" in res, res[:600])
K.finish()
