"""First login: the profile is empty, so the first chat view of a session goes straight to Memory → Profile, where an
ID photo read by the vision model prefills name, birthday and nationality and offers the document number to the Vault
(the photo is not kept, the number never reaches the profile). Afterwards the chat shows a banner until the profile is
complete or the user says "later"; a complete profile ends both."""
import asyncio
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from playwright.async_api import async_playwright  # noqa: E402

from tests import skills_common as K  # noqa: E402

B = K.B + "/"
PNG_PATH = os.path.join(os.path.dirname(os.path.abspath(__file__)), "pages", "lipstick.png")
K.c.put(B + "api/settings", json={"language": "en", "ui_language": "en", "profile_reminder": True}, headers=K.H)
for k in ("name_en", "birthday", "nationality"):
    K.c.put(B + "api/profile", json={"key": k, "value": ""}, headers=K.H)
st = K.c.get(B + "api/profile/status", headers=K.H).json()
K.check("API: an empty profile is incomplete and the reminder is on", st == {"complete": False, "missing": ["name_en", "birthday", "nationality"], "reminder": True}, st)
r = K.c.put(B + "api/profile/ocr", content=open(PNG_PATH, "rb").read(), headers={**K.H, "Content-Type": "image/png"})
j = r.json()
K.check("API: the ID photo is read into profile fields and a vault item proposal",
        r.status_code == 200 and j["profile"] == {"name_en": "LU LUCAS", "birthday": "1985-04-12", "nationality": "Australia"}
        and j["document"]["values"]["number"] == "E1234567X" and j["document"]["label"] == "Passport LU", j)
K.check("API: the picture was not kept in the workspace", "lipstick" not in str(K.c.get(B + "api/files", headers=K.H).json()))
aud = K.c.get(K.S + "/audit?limit=20", headers=K.H).json()["events"]
ocr = [e for e in aud if e.get("action") == "profile.ocr"]
K.check("API: audited without the number", ocr and "E1234567X" not in str(ocr[0]) and "number" in str(ocr[0].get("detail")), ocr[:1])
K.check("API: a non-image is refused", K.c.put(B + "api/profile/ocr", content=b"hello", headers={**K.H, "Content-Type": "text/plain"}).status_code == 400)


async def main():
    async with async_playwright() as p:
        br = await p.chromium.launch()
        ctx = await br.new_context(viewport={"width": 1400, "height": 1000})
        await ctx.add_init_script("try { localStorage.setItem('omuse_ui', 'en') } catch (e) {}")
        pg = await ctx.new_page()
        errs = []
        pg.on("pageerror", lambda e: errs.append(str(e)))
        await pg.goto(B + "#chat")
        await pg.wait_for_selector("#profile", timeout=15000)
        location = await pg.evaluate("location.hash")
        K.check("first login: the chat view sends the user to Memory → Profile", location.startswith("#memory/profile"), location)
        K.check("...the profile card is highlighted and explains the ID photo", True
                and await pg.locator("#profile.flash").count() == 1 and "Upload ID photo" in await pg.locator("#profile").inner_text())
        # the ID photo prefills the form
        await pg.locator("#profile input[type=file]").set_input_files(PNG_PATH)
        await pg.wait_for_selector("text=Save to Vault", timeout=15000)
        name = await pg.locator("#profile label.field", has_text="Name as on passport").locator("input").input_value()
        bday = await pg.locator("#profile label.field", has_text="Date of birth").locator("input").input_value()
        nat = await pg.locator("#profile label.field", has_text="Nationality").locator("input").input_value()
        K.check("the photo prefills name, birthday and nationality (marked)", (name, bday, nat) == ("LU LUCAS", "1985-04-12", "Australia")
                and await pg.locator("#profile input.prefilled").count() == 3, (name, bday, nat))
        txt = await pg.locator("#profile").inner_text()
        K.check("the number is shown masked, offered to the Vault, and the photo is said not to be kept",
                "••••567X" in txt and "E1234567X" not in txt and "not kept" in txt, txt[-400:])
        await pg.locator("button", has_text="Save to Vault").click()
        await pg.wait_for_selector("text=Saved to the Vault")
        items = K.c.get(K.S + "/vault", headers=K.H).json().get("items", [])
        K.check("the vault item exists (number stored there only)", any(i["label"] == "Passport LU" and i["kind"] == "id_document" for i in items), [i["label"] for i in items])
        prof = K.c.get(B + "api/memory", headers=K.H).json()["profile"]
        K.check("nothing is saved to the profile until Save profile", not prof.get("name_en"), prof)
        # not saved yet → the chat still shows the banner; "Later" dismisses it
        await pg.goto(B + "#chat")
        await pg.wait_for_selector("#profileReminder", timeout=10000)
        K.check("the chat banner reminds while the profile is incomplete", await pg.locator("#profileReminder").count() == 1)
        await pg.locator("#profileReminder button", has_text="Later").click()
        await pg.wait_for_timeout(500)
        K.check("Later: the banner goes and the setting is off", await pg.locator("#profileReminder").count() == 0
                and K.c.get(B + "api/settings", headers=K.H).json()["settings"]["profile_reminder"] is False)
        await pg.reload()
        await pg.wait_for_selector(".thread")
        await pg.wait_for_timeout(800)
        K.check("...and stays gone after a reload", await pg.locator("#profileReminder").count() == 0 and (await pg.evaluate("location.hash")) == "#chat")
        # save the profile: complete → no nudge even with the reminder back on
        K.c.put(B + "api/settings", json={"profile_reminder": True}, headers=K.H)
        await pg.goto(B + "#memory/profile")
        await pg.wait_for_selector("#profile")
        await pg.locator("#profile input[type=file]").set_input_files(PNG_PATH)
        await pg.wait_for_selector("text=Save to Vault", timeout=15000)
        await pg.locator("button", has_text="Save profile").click()
        await pg.wait_for_selector(".toast")
        stt = K.c.get(B + "api/profile/status", headers=K.H).json()
        K.check("Save profile completes it", stt["complete"] is True and K.c.get(B + "api/memory", headers=K.H).json()["profile"]["birthday"] == "1985-04-12", stt)
        ctx2 = await br.new_context(viewport={"width": 1400, "height": 1000})
        await ctx2.add_init_script("try { localStorage.setItem('omuse_ui', 'en') } catch (e) {}")
        pg2 = await ctx2.new_page()
        await pg2.goto(B + "#chat")
        await pg2.wait_for_selector(".thread")
        await pg2.wait_for_timeout(800)
        K.check("a complete profile: a new session stays on the chat, no banner",
                (await pg2.evaluate("location.hash")) == "#chat" and await pg2.locator("#profileReminder").count() == 0)
        # the Memory tidy card: a run's report lines follow the UI language (they used to be stored in Chinese)
        K.c.post(B + "api/memory/consolidate", json={}, headers=K.H)
        for _ in range(60):
            if not K.c.get(B + "api/memory", headers=K.H).json()["job"].get("running"):
                break
            await asyncio.sleep(0.5)
        await pg2.goto(B + "#memory")
        await pg2.wait_for_selector("text=Memory tidy")
        tidy = await pg2.locator(".card", has_text="Memory tidy").inner_text()
        han = [l for l in tidy.splitlines() if any("\u4e00" <= ch <= "\u9fff" for ch in l)]
        K.check("English UI: the Memory tidy card and its run reports have no Chinese", "Long-term" in tidy and not han, han or tidy[:300])
        K.check("no page errors", not errs, errs)
        await br.close()


asyncio.run(main())
K.finish()
