"""Memory → Profile: custom fields and extra addresses without limit — "+ Add field" opens another row, every filled row is
saved, a saved field has a remove button, and removing one deletes just that key."""
import asyncio
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from playwright.async_api import async_playwright  # noqa: E402

from tests import skills_common as K  # noqa: E402
from tests.skills_common import check, finish  # noqa: E402

B = K.B + "/"
K.c.put(K.B + "/api/settings", json={"language": "en", "ui_language": "en", "profile_reminder": False}, headers=K.H)
for k in ("custom:Airline", "custom:Seat", "custom:Hotel chain", "address:Beach house"):
    K.c.put(K.B + "/api/profile", json={"key": k, "value": ""}, headers=K.H)


async def main():
    async with async_playwright() as p:
        br = await p.chromium.launch()
        pg = await br.new_page(viewport={"width": 1280, "height": 1000})
        errs = []
        pg.on("pageerror", lambda e: errs.append(str(e)))
        await pg.goto(B + "#memory/profile")
        await pg.wait_for_selector("#profile")
        card = pg.locator("#profile")
        check("one empty custom row and one empty address row to start", await card.locator(".newfield[data-kind=custom]").count() == 1
              and await card.locator(".newfield[data-kind=address]").count() == 1)
        await card.locator("button", has_text="Add field").click()
        await card.locator("button", has_text="Add field").click()
        rows = card.locator(".newfield[data-kind=custom]")
        check("+ Add field opens more rows (three now)", await rows.count() == 3)
        for i, (lab, val) in enumerate((("Airline", "Singapore Airlines"), ("Seat", "aisle"), ("Hotel chain", "Marriott"))):
            await rows.nth(i).locator("input").nth(0).fill(lab)
            await rows.nth(i).locator("input").nth(1).fill(val)
        await card.locator("button", has_text="Add address").click()
        arow = card.locator(".newfield[data-kind=address]").nth(1)
        await arow.locator("input").nth(0).fill("Beach house")
        await arow.locator("input").nth(1).fill("1 Shell Lane, Sentosa 098000")
        await card.locator("button", has_text="Save profile").click()
        await pg.wait_for_selector(".toast")
        toast = await pg.locator(".toast").inner_text()
        check("saving reports the four new fields", "4" in toast, toast)
        await pg.wait_for_function("() => document.querySelectorAll('#profile .removable').length >= 4", timeout=10000)
        keys = await pg.evaluate("() => [...document.querySelectorAll('#profile .removable')].map(e => e.dataset.key)")
        check("all three custom fields and the address are saved, each with a remove button",
              all(k in keys for k in ("custom:Airline", "custom:Seat", "custom:Hotel chain", "address:Beach house")), keys)
        pg.on("dialog", lambda d: asyncio.ensure_future(d.accept()))
        await pg.locator("#profile .removable[data-key='custom:Seat'] button").click()
        await pg.wait_for_function("() => !document.querySelector(\"#profile .removable[data-key='custom:Seat']\")", timeout=10000)
        keys = await pg.evaluate("() => [...document.querySelectorAll('#profile .removable')].map(e => e.dataset.key)")
        check("removing one field deletes just that key", "custom:Seat" not in keys and "custom:Airline" in keys and "address:Beach house" in keys, keys)
        await pg.reload()
        await pg.wait_for_selector("#profile .removable")
        keys = await pg.evaluate("() => [...document.querySelectorAll('#profile .removable')].map(e => e.dataset.key)")
        check("after a reload the fields are still there", sorted(keys) == sorted(["custom:Airline", "custom:Hotel chain", "address:Beach house"]), keys)
        check("no page errors", not errs, errs)
        await br.close()


asyncio.run(main())
for k in ("custom:Airline", "custom:Hotel chain", "address:Beach house"):
    K.c.put(K.B + "/api/profile", json={"key": k, "value": ""}, headers=K.H)
finish()
