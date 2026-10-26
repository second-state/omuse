"""drive-photo-storybook skill: the photos of a Drive folder are listed and downloaded (one turn), one is restyled with
the image model, a PDF story book is laid out with the photos (make_pdf with images), shown in the chat and uploaded
back into the folder with an approval — a real PDF with the real JPEGs from the fake Drive."""
import json

from tests import drive_common as D
from tests import skills_common as K
from tests.skills_common import check, finish

D.disconnect_all()
D.reset_drive()
D.connect_drive()
LLM = "http://127.0.0.1:8090"
K.c.put(D.B + "/api/settings", json={"language": "en", "model_override": True, "model_base_url": LLM + "/v1", "model_api_key": "own-key",
                                     "model_name": "fake", "image_model": "gpt-image-1"}, headers=D.H)

t = K.run("DRIVEBOOK make a story book from Mia's birthday photos", timeout=240)
names = K.names(t)
check("the task completed", t["status"] == "COMPLETED" and "STORYBOOK DONE" in str(t.get("result")), (t["status"], t.get("error"), names, str(t.get("result"))[:300]))
check("skill → photo search → three downloads → one edit → the PDF → upload, no browser",
      names == ["load_skill", "drive_search", "drive_download", "drive_download", "drive_download", "edit_image", "make_pdf", "send_file", "drive_upload"], names)
calls = D.drive_state()["calls"]
lists = [x for x in calls if "list" in x]
check("the search asked Drive for the images of the birthday folder, by name", lists and "mimeType contains 'image/'" in lists[0]["list"]
      and "'fld-bday' in parents" in lists[0]["list"] and lists[0]["orderBy"] == "name", lists)
check("the three photos were downloaded once each", sorted(x["download"] for x in calls if "download" in x) == ["img-1", "img-2", "img-3"], calls)
for p in ("drive/mia-cake.jpg", "drive/mia-balloons.jpg", "drive/mia-friends.jpg"):
    raw = K.c.get(D.B + "/api/files/raw", params={"path": p}, headers=D.H)
    check(f"{p} is a real JPEG in the workspace", raw.status_code == 200 and raw.content[:3] == b"\xff\xd8\xff", (raw.status_code, raw.content[:4]))
raw = K.c.get(D.B + "/api/files/raw", params={"path": "images/cake-watercolour.png"}, headers=D.H)
check("the restyled photo exists (image model edit)", raw.status_code == 200 and raw.content[:4] == b"\x89PNG", raw.status_code)
pdfr = (K.results(t, "make_pdf") or [{}])[0].get("preview", "")
check("make_pdf made the book", "created locally: reports/Mia-birthday-storybook.pdf" in pdfr, pdfr[:200])
raw = K.c.get(D.B + "/api/files/raw", params={"path": "reports/Mia-birthday-storybook.pdf"}, headers=D.H)
check("the PDF is real and carries the photos (bigger than text alone)", raw.status_code == 200 and raw.content[:5] == b"%PDF-" and len(raw.content) > 20000, (raw.status_code, len(raw.content)))
ap = [a for a in t["_approvals"] if a["tool"] == "drive_upload"]
check("the upload asked for approval once, into the photos' folder with a readable name",
      len(ap) == 1 and ap[0]["args"]["folder_id"] == "fld-bday" and ap[0]["args"]["name"] == "Mia birthday storybook.pdf", t["_approvals"])
up = D.drive_state()["uploads"]
check("the fake Drive received the PDF in the birthday folder", len(up) == 1 and up[0]["name"] == "Mia birthday storybook.pdf" and up[0]["mime"] == "application/pdf"
      and up[0]["parents"] == ["fld-bday"] and up[0]["head"].startswith("255044462d") and up[0]["size"] > 20000, up)
conv = K.c.get(f"{D.B}/api/conversations/{t['_conv']}", headers=D.H).json()
cards = [m["content"] for m in conv["messages"] if m["role"] == "system" and '"type": "file"' in m["content"]]
check("the book (and the restyled photo) were shown in the chat", any("Mia-birthday-storybook.pdf" in x for x in cards), cards)
K.c.put(D.B + "/api/settings", json={"model_override": False}, headers=D.H)
D.disconnect_all()
finish()
