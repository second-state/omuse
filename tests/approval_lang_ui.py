"""The approval dialog speaks one language: in the English UI a vault-fill approval (Sentinel's reason and field values
are bilingual on the wire, e.g. "…输入框（每次使用都需要你批准）(fills a vault value — approved per use)" and
"号码 number") shows English only; in the Chinese UI, Chinese only."""
import asyncio
import os
import re
import sys

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from playwright.async_api import async_playwright  # noqa: E402

from tests import skills_common as K  # noqa: E402

K.setup(mail=[("Singapore Airlines <noreply@singaporeair.example>", "Your e-ticket: SQ 231 SYD-SIN 2026-11-02",
               "Booking ref 9QW3PL. SQ 231 departs Sydney 2026-11-02 10:30, arrives Singapore 16:20.", 1),
              ("Marina Bayfront Hotel <stay@marinabayfront.example>", "Booking confirmed: Marina Bayfront Hotel",
               "Confirmation MB-55120. Check-in 2026-11-02, 3 nights. 10 Bayfront Avenue, Singapore 018956.", 1)])
K.set_profile(name_en="LUCAS LU", birthday="1985-04-12", nationality="Australia")
K.vault_item("Passport Lucas", "id_document", {"number": "E1234567X", "expiry": "2031-05-01"}, domains="shop.test")
t = K.run("SGAC fill in my Singapore arrival card for this trip. Health: nothing to declare.", approve=False)
K.check("a vault-fill approval is pending", t["status"] == "WAITING_APPROVAL", (t["status"], t.get("error")))
HAN = re.compile(r"[一-鿿]")


async def dialog_text(lang):
    async with async_playwright() as p:
        br = await p.chromium.launch()
        K.c.put(K.B + "/api/settings", json={"ui_language": lang}, headers=K.H)
        ctx = await br.new_context(viewport={"width": 1400, "height": 1000})
        await ctx.add_init_script(f"try {{ localStorage.setItem('omuse_ui', '{lang}') }} catch (e) {{}}")
        pg = await ctx.new_page()
        await pg.goto(K.B + "/#chat")
        await pg.wait_for_selector(".approval", timeout=15000)
        txt = await pg.locator(".approval").first.inner_text()
        await pg.keyboard.press("Escape")
        await pg.goto(K.B + f"/#chat/{t['_conv']}")
        await pg.reload()
        await pg.wait_for_timeout(1500)
        card = await pg.locator(".thread, #thread, main").first.inner_text()
        await br.close()
        return txt + "\n---CARD---\n" + "\n".join(l for l in card.splitlines() if "Sentinel" in l)


en = asyncio.run(dialog_text("en"))
K.check("English UI: the approval dialog has no Chinese", not HAN.search(en), [l for l in en.splitlines() if HAN.search(l)])
card = en.split("---CARD---")[-1]
K.check("English UI: the chat card names the action in English", "Fill from vault" in card and not HAN.search(card), card)
K.check("English UI: the reason and the value read in English",
        "Fills a vault value — approved per use" in en and "Once only" in en and "number" in en, en[:600])
zh = asyncio.run(dialog_text("zh"))
K.check("Chinese UI: the reason is the Chinese half", "每次使用都需要你批准" in zh and "fills a vault value" not in zh, zh[:600])
K.c.put(K.B + "/api/settings", json={"ui_language": "en"}, headers=K.H)
K.finish()
