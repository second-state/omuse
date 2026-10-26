"""drive-video-edit skill: find the clips in Drive, download them (one turn), join them with edit_video (quiet), put a
title on the result, show that one cut in the chat, upload it back into the same Drive folder with an approval. The
edits are real ffmpeg runs on real MP4s from the fake Drive; the uploaded file is a real MP4."""
import json

from tests import drive_common as D
from tests import skills_common as K
from tests.skills_common import check, finish

D.disconnect_all()
D.reset_drive()
D.connect_drive()
K.c.put(D.B + "/api/settings", json={"language": "en"}, headers=D.H)

t = K.run("DRIVEVIDEO join the two kyoto clips from my Drive, put a title on it and save it back", timeout=240)
names = K.names(t)
check("the task completed", t["status"] == "COMPLETED" and "VIDEO EDIT DONE" in str(t.get("result")), (t["status"], t.get("error"), names, str(t.get("result"))[:300]))
check("skill → search → two downloads → concat → caption → upload, no browser",
      names == ["load_skill", "drive_search", "drive_download", "drive_download", "edit_video", "edit_video", "drive_upload"], names)
dl = [r.get("preview", "") for r in K.results(t, "drive_download")]
check("downloads landed under drive/ and are marked untrusted", len(dl) == 2 and all("Drive file saved: drive/kyoto-" in x and "untrusted" in x for x in dl), dl)
for p in ("drive/kyoto-arrival.mp4", "drive/kyoto-temple.mp4"):
    raw = K.c.get(D.B + "/api/files/raw", params={"path": p}, headers=D.H)
    check(f"{p} is a real MP4 in the workspace", raw.status_code == 200 and raw.content[4:8] == b"ftyp" and len(raw.content) > 2000, (raw.status_code, len(raw.content)))
ev = [e["data"] for e in t.get("events", []) if e["type"] == "video"]
check("two video events: the concat and the caption, 4 seconds long, 320x240",
      [e["op"] for e in ev] == ["concat", "caption"] and all(e["size"] == "320x240" and e["seconds"] == 4 for e in ev), ev)
concat_out = ev[0]["paths"][0] if ev else ""
final_out = ev[1]["paths"][0] if len(ev) > 1 else ""
check("file names follow the titles", concat_out == "videos/kyoto.mp4" and final_out == "videos/Kyoto_highlights.mp4", (concat_out, final_out))
raw = K.c.get(D.B + "/api/files/raw", params={"path": final_out}, headers=D.H)
check("the final cut is a real MP4", raw.status_code == 200 and raw.content[4:8] == b"ftyp" and len(raw.content) > 5000, (raw.status_code, len(raw.content)))
conv = K.c.get(f"{D.B}/api/conversations/{t['_conv']}", headers=D.H).json()
cards = [m["content"] for m in conv["messages"] if m["role"] == "system" and '"type": "file"' in m["content"]]
check("only the final cut was shown in the chat (the concat step was quiet)", len(cards) == 1 and "Kyoto_highlights.mp4" in cards[0] and "video/mp4" in cards[0], cards)
ap = [a for a in t["_approvals"] if a["tool"] == "drive_upload"]
check("the upload asked for approval once, naming the file and the folder", len(ap) == 1 and ap[0]["args"]["path"] == final_out and ap[0]["args"]["folder_id"] == "fld-trip", t["_approvals"])
st = D.drive_state()
up = st["uploads"]
check("the fake Drive received one real MP4 in the Kyoto folder", len(up) == 1 and up[0]["name"] == "Kyoto_highlights.mp4" and up[0]["parents"] == ["fld-trip"]
      and up[0]["mime"] == "video/mp4" and up[0]["size"] > 5000 and up[0]["head"].endswith("66747970"), up)
upr = (K.results(t, "drive_upload") or [{}])[0].get("preview", "")
check("the upload result carries the Drive link for the user", '"uploaded": true' in upr and "https://drive.google.com/" in upr, upr[:300])
check("each clip was downloaded once; no second search", names.count("drive_download") == 2 and names.count("drive_search") == 1, names)
D.disconnect_all()
finish()
