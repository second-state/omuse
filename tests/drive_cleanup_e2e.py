"""drive-cleanup skill: the quota and the two searches (big files; big and long-unused files) go in ONE turn, the agent
shows a numbered list and asks before anything happens (nothing trashed, no approval), then the user's answer in the
same conversation leads to ONE drive_trash call for the chosen files — approved on a card that lists their names —
and the report says what was moved, the MB freed and that the bin keeps them 30 days. Nothing is ever deleted
permanently; the browser is never used."""
import json

from tests import drive_common as D
from tests import skills_common as K
K.BOT = "photos"   # the Photos & videos bot has this suite's skills (crate::bots)
from tests.skills_common import check, finish

D.disconnect_all()
D.reset_drive()
D.connect_drive()
K.c.put(D.B + "/api/settings", json={"language": "en"}, headers=D.H)

t = K.run("DRIVECLEAN my Drive is full — what can go?")
names = K.names(t)
check("task 1 completed with a list and a question, no trash yet", t["status"] == "COMPLETED" and "CLEANUP LIST" in str(t.get("result"))
      and "Move all of these to the bin, or only some" in str(t.get("result")) and "drive_trash" not in names and not t["_approvals"],
      (t["status"], names, str(t.get("result"))[:300]))
check("the skill was loaded; quota and both searches in one turn; no browser", names[:4] == ["load_skill", "drive_storage", "drive_search", "drive_search"]
      and not any(n.startswith("browser_") for n in names), names)
check("...the three reads ran concurrently", ["drive_storage", "drive_search", "drive_search"] in K.batches(t), K.batches(t))
lists = [x for x in D.drive_state()["calls"] if "list" in x]
big_q = [x for x in lists if "viewedByMeTime" not in x["list"]]
old_q = [x for x in lists if "viewedByMeTime" in x["list"]]
check("the big-file search asked Drive for the user's own files by size; the unused search added the never-opened clause",
      len(big_q) == 1 and len(old_q) == 1 and all(x["orderBy"] == "quotaBytesUsed desc" and "'me' in owners" in x["list"] and "trashed = false" in x["list"] for x in lists)
      and "not viewedByMeTime >" in old_q[0]["list"], lists)
check("the answer names the three candidates (big own files; the shared sheet and the trashed clip are out), marks the recent one, promises no deletion",
      "3 candidates" in str(t["result"]) and all(x in str(t["result"]) for x in ("backup-2022.zip", "old-render.mov", "wedding-raw.mov", "never opened", "recent"))
      and "team-roadmap" not in str(t["result"]) and "already-gone" not in str(t["result"]) and "permanently" not in str(t["result"]), str(t["result"])[:400])
check("nothing was trashed (only the seeded bin item is in the bin)", D.drive_state()["trashed"] == ["trash-1"], D.drive_state()["trashed"])

t2 = K.run("DRIVECLEAN yes, move 1 and 2 to the bin", conv=t["_conv"])
names2 = K.names(t2)
check("task 2: one drive_trash call for the two chosen files, then the report", t2["status"] == "COMPLETED" and names2.count("drive_trash") == 1
      and "CLEANUP DONE" in str(t2.get("result")), (t2["status"], names2, str(t2.get("result"))[:300]))
ap = [a for a in t2["_approvals"] if a["tool"] == "drive_trash"]
check("the trash batch was approved once, with the file names on the card", len(ap) == 1 and ap[0]["args"]["names"] == ["backup-2022.zip", "old-render.mov"], t2["_approvals"])
pend = K.c.get(D.S + "/approvals?status=pending", headers=D.H).json()["approvals"]
check("no approval left behind", not [a for a in pend if a["tool"].startswith("drive_")], pend)
st = D.drive_state()
check("the fake Drive moved exactly those two files to the bin", st["trashed"] == ["big-1", "big-2", "trash-1"], st["trashed"])
import re
pv = (K.results(t2, "drive_trash") or [{}])[0].get("preview", "")
freed = re.search(r'"freed_mb": ([0-9.]+)', pv)
check("the tool reported the MB freed and the 30-day note", '"count": 2' in pv and freed and 3400 < float(freed.group(1)) < 3500 and "30" in pv, pv[:400])
ev = K.c.get(D.S + "/audit?limit=100", headers=D.H).json()
ev = ev.get("events") or ev.get("entries") or ev
check("the approval card summary named the batch", any("drive_trash" in json.dumps(e) and "backup-2022.zip" in json.dumps(e) for e in ev))
D.disconnect_all()
finish()
