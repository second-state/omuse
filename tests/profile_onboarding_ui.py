"""First login: the profile is empty, so the first chat view of a session goes straight to Memory → Profile, where an
ID photo read by the vision model saves name, birthday and nationality to the profile and the document to the Vault (the
photo is not kept, the number never reaches the profile); the card then lists the stored ID with "Add another ID" instead
of asking again. Until then the chat shows a banner unless the user says "later"; a complete profile ends both."""
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
        # not filled in yet → the chat shows the banner; "Later" dismisses it
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
        # the ID photo: the empty fields are saved at once, the document goes to the Vault and is listed on the card
        K.c.put(B + "api/settings", json={"profile_reminder": True}, headers=K.H)
        await pg.goto(B + "#memory/profile")
        await pg.wait_for_selector("#profile")
        await pg.locator("#profile input[type=file]").set_input_files(PNG_PATH)
        await pg.wait_for_selector("text=saved to the Vault", timeout=15000)
        txt = await pg.locator("#profile").inner_text()
        K.check("the card says the fields were saved and the ID went to the Vault, masked, and the photo is not kept",
                "Read and saved 3 field(s)" in txt and "••••567X" in txt and "E1234567X" not in txt and "not kept" in txt, txt[-500:])
        prof = K.c.get(B + "api/memory", headers=K.H).json()["profile"]
        K.check("name, birthday and nationality are saved to the profile (no Save press needed)",
                (prof.get("name_en"), prof.get("birthday"), prof.get("nationality")) == ("LU LUCAS", "1985-04-12", "Australia"), prof)
        items = K.c.get(K.S + "/vault", headers=K.H).json().get("items", [])
        K.check("the vault item exists (number stored there only)", any(i["label"] == "Passport LU" and i["kind"] == "id_document" for i in items)
                and "E1234567X" not in str(prof), [i["label"] for i in items])
        K.check("the card lists the stored ID right away", await pg.locator("#idDocs .iddoc", has_text="Passport LU").count() == 1)
        stt = K.c.get(B + "api/profile/status", headers=K.H).json()
        K.check("the profile is complete", stt["complete"] is True, stt)
        # after a reload: the card shows the ID and offers to add another — it does not ask for an ID again
        await pg.reload()
        await pg.wait_for_selector("#idDocs .iddoc")
        box = await pg.locator("#idBox").inner_text()
        K.check("after a reload the card shows Your IDs with the passport and an Add another ID button, not the upload prompt",
                "Your IDs" in box and "Passport LU" in box and "Add another ID" in box and "Upload ID photo" not in box and "Complete your profile" not in box, box)
        # the same passport again: not stored twice
        await pg.locator("#profile input[type=file]").set_input_files(PNG_PATH)
        await pg.wait_for_selector("text=already in the Vault", timeout=15000)
        items = K.c.get(K.S + "/vault", headers=K.H).json().get("items", [])
        K.check("uploading the same ID again does not add a second vault item", sum(i["label"] == "Passport LU" for i in items) == 1, [i["label"] for i in items])
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
