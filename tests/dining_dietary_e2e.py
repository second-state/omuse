"""dining-dietary-translate skill: the menu photo is read by the vision model, the dish with the allergen is flagged, the
card is written in Japanese, the allergy is proposed for the profile (pending the user's confirmation), no browser."""
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from tests import skills_common as K  # noqa: E402
K.BOT = "cities"   # the City guides bot has this suite's skills (crate::bots)

K.setup()
menu = K.upload("menu.png", K.PNG)
t = K.run("DIETARY I have a severe peanut allergy. Make me a card in Japanese and check this menu for me.", attachments=[menu.get("attachment") or menu])
K.trace(t)
names = K.names(t)
K.check("task completes", t["status"] == "COMPLETED", (t["status"], t.get("error")))
K.check("the dining-dietary-translate skill was loaded", K.loaded(t, "dining-dietary-translate"), names)
fl = [x for x in K.calls(t) if x["name"] == "file_look"]
K.check("the menu photo was read by the vision model", fl and "uploads/" in str(fl[0]["args"].get("path")) and "menu" in str(fl[0]["args"].get("question")).lower(), fl)
K.check("no browser", not any(n.startswith("browser_") for n in names), names)
res = t.get("result") or ""
K.check("the card is in Japanese and names the allergy", "ピーナッツアレルギー" in res and "ありがとうございます" in res, res[:400])
K.check("the dish with peanuts is flagged, from the menu reading", "担々麺" in res and "peanut" in res.lower(), res[:600])
pend = K.c.get(K.B + "/api/memory").json().get("pending", [])
K.check("the allergy is proposed for the profile, pending the user's confirmation",
        any(p.get("key") == "custom:allergies" and "peanut" in str(p.get("value")) for p in pend), pend)
K.finish()
