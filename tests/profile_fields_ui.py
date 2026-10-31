"""Memory → Profile: custom fields and extra addresses without limit — "+ Add field" opens another row, every filled row is
saved, a saved field has a remove button, and removing one deletes just that key. The page's layout: the pending changes
are the whole top row, the Vault is beside the profile. A credit card is added on the Vault card by its name alone ("+ Add
credit card"): its number, expiry and CVC are UNKNOWN, and the card says so until the user adds them."""
import asyncio
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from playwright.async_api import async_playwright  # noqa: E402

from tests import skills_common as K  # noqa: E402
from tests.skills_common import check, finish  # noqa: E402

B = K.B + "/"
K.c.put(K.B + "/api/settings", json={"language": "en", "ui_language": "en", "profile_reminder": False}, headers=K.H)
for k in ("custom:Airline", "custom:Seat", "custom:Hotel chain", "address:Beach house", "preferred_car_rental"):
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
        labels = await card.locator("label.field > span").all_inner_texts()
        check("the passport name comes first and the other name right after it", labels[:2] == ["Name as on passport", "Other name"] and "Chinese name" not in labels, labels[:4])
        # the two standard preference fields are dropdowns: alliances + airlines, groups + brands, blank first; a hint points at custom fields
        air = card.locator("select[data-key=preferred_airline]")
        hot = card.locator("select[data-key=preferred_hotel_brand]")
        air_opts = await air.locator("option").all_inner_texts()
        hot_opts = await hot.locator("option").all_inner_texts()
        air_groups = await air.locator("optgroup").evaluate_all("els => els.map(e => e.label)")
        hot_groups = await hot.locator("optgroup").evaluate_all("els => els.map(e => e.label)")
        check("Preferred airline is a dropdown: blank, then Alliances (Star Alliance…) and Airlines (United, ANA…)", await air.count() == 1 and air_opts[0] == "—"
              and air_groups == ["Alliances", "Airlines"] and all(x in air_opts for x in ("Star Alliance", "oneworld", "United", "ANA")) and len(air_opts) > 30, (air_groups, air_opts[:6]))
        check("Preferred hotel brand is a dropdown: blank, then Hotel groups (IHG One Rewards…) and Brands (Holiday Inn…)", await hot.count() == 1 and hot_opts[0] == "—"
              and hot_groups == ["Hotel groups / loyalty programmes", "Brands"] and all(x in hot_opts for x in ("IHG One Rewards", "Marriott Bonvoy", "Holiday Inn", "Hyatt Regency")), (hot_groups, hot_opts[:6]))
        car = card.locator("select[data-key=preferred_car_rental]")
        car_opts = await car.locator("option").all_inner_texts()
        car_groups = await car.locator("optgroup").evaluate_all("els => els.map(e => e.label)")
        check("Preferred car rental is a dropdown: blank, then Rental companies (Hertz, Avis, Sixt…)", await car.count() == 1 and car_opts[0] == "—"
              and car_groups == ["Rental companies"] and all(x in car_opts for x in ("Hertz", "Avis", "Sixt", "Toyota Rent a Car")), (car_groups, car_opts[:6]))
        hint = await card.locator("#pref-hint").inner_text()
        check("the hint explains airline or alliance, brand or group, and custom fields for anything else", "Star Alliance" in hint and "IHG" in hint and "Hertz" in hint and "Not on the list" in hint and "custom field" in hint, hint)
        await air.select_option("Star Alliance")
        await hot.select_option("Holiday Inn")
        await car.select_option("Hertz")
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
        check("saving reports the four new fields and the three dropdown choices", "7" in toast, toast)
        prof = K.c.get(K.B + "/api/memory").json().get("profile", {})
        check("the dropdown choices are saved as the standard fields", prof.get("preferred_airline") == "Star Alliance" and prof.get("preferred_hotel_brand") == "Holiday Inn"
              and prof.get("preferred_car_rental") == "Hertz", {k: prof.get(k) for k in ("preferred_airline", "preferred_hotel_brand", "preferred_car_rental")})
        await pg.wait_for_function("() => document.querySelectorAll('#profile .removable').length >= 4", timeout=10000)
        keys = await pg.evaluate("() => [...document.querySelectorAll('#profile .removable')].map(e => e.dataset.key)")
        check("all three custom fields and the address are saved, each with a remove button",
              all(k in keys for k in ("custom:Airline", "custom:Seat", "custom:Hotel chain", "address:Beach house")), keys)
        pg.on("dialog", lambda d: asyncio.ensure_future(d.accept()))
        await pg.locator("#profile .removable[data-key='custom:Seat'] button").click()
        # the page is drawn again after the removal: wait for the rows to be back, without the removed one
        await pg.wait_for_function("() => document.querySelector(\"#profile .removable[data-key='custom:Airline']\") && !document.querySelector(\"#profile .removable[data-key='custom:Seat']\")", timeout=10000)
        keys = await pg.evaluate("() => [...document.querySelectorAll('#profile .removable')].map(e => e.dataset.key)")
        check("removing one field deletes just that key", "custom:Seat" not in keys and "custom:Airline" in keys and "address:Beach house" in keys, keys)
        await pg.reload()
        await pg.wait_for_selector("#profile .removable")
        check("after a reload the dropdowns show the saved choices", await pg.locator("#profile select[data-key=preferred_airline]").input_value() == "Star Alliance"
              and await pg.locator("#profile select[data-key=preferred_hotel_brand]").input_value() == "Holiday Inn"
              and await pg.locator("#profile select[data-key=preferred_car_rental]").input_value() == "Hertz")
        keys = await pg.evaluate("() => [...document.querySelectorAll('#profile .removable')].map(e => e.dataset.key)")
        check("after a reload the fields are still there", sorted(keys) == sorted(["custom:Airline", "custom:Hotel chain", "address:Beach house"]), keys)
        lay = await pg.evaluate("""() => { const card = document.querySelector('#profile'); const grid = card.querySelector('.grid2');
            const r = k => card.querySelector(`.removable[data-key='${k}']`); const nf = card.querySelector('.newfield[data-kind=address]');
            const a = r('address:Beach house'), c = r('custom:Airline');
            return { inGrid: !!grid.querySelector('.removable'), addrW: a.getBoundingClientRect().width, newW: nf.getBoundingClientRect().width,
                     addrBeforeNew: !!(a.compareDocumentPosition(nf) & Node.DOCUMENT_POSITION_FOLLOWING), addrName: a.querySelector('.fieldname').textContent,
                     customAfterAddr: !!(a.compareDocumentPosition(c) & Node.DOCUMENT_POSITION_FOLLOWING) }; }""")
        check("saved addresses and custom fields are full-width rows above the entry rows, not cells of the fixed-field grid",
              not lay["inGrid"] and abs(lay["addrW"] - lay["newW"]) < 2 and lay["addrBeforeNew"] and lay["addrName"] == "Beach house" and lay["customAfterAddr"], lay)
        # the layout: pending changes across the top, the Vault beside the profile
        await pg.wait_for_selector("#memProfileVault #addCardBtn", timeout=10000)
        box = await pg.evaluate("""() => { const q = s => document.querySelector(s);
            const pend = q('#memPending'), prof = q('#profile'), grid = q('#memProfileVault');
            const cards = grid ? [...grid.children] : [];
            const vault = cards.find(e => e.querySelector('#addCardBtn'));
            const b = e => e ? e.getBoundingClientRect() : { top: -1, left: -1, width: -1 };
            return { pendTop: b(pend).top, pendW: b(pend).width, mainW: b(grid).width, profTop: b(prof).top,
                     profLeft: b(prof).left, vaultTop: b(vault).top, vaultLeft: b(vault).left, vaultIsVault: !!vault,
                     gridKids: cards.map(e => e.className + '#' + e.id) }; }""")
        check("the pending changes are the whole top row", box["pendTop"] < box["profTop"] and abs(box["pendW"] - box["mainW"]) < 2, box)
        check("the Vault is beside the profile", box["vaultIsVault"] and abs(box["vaultTop"] - box["profTop"]) < 2 and box["vaultLeft"] > box["profLeft"], box)
        # a credit card by its name alone
        for it in K.c.get(K.B + "/sentinel/api/vault", headers=K.H).json()["items"]:
            if it["label"] in ("Citi Prestige", "UOB One"):
                K.c.delete(K.B + "/sentinel/api/vault/" + it["id"], headers=K.H)
        await pg.locator("#addCardBtn").click()
        form_label = pg.locator("#memProfileVault input[placeholder*='Citi Prestige']")
        form_country = pg.locator("#memProfileVault input.cardcountry")
        check("+ Add credit card opens the form for a card: its name, and the issuing country as a field of its own (optional)",
              await form_label.count() == 1 and await form_country.is_visible() and "optional" in (await form_country.get_attribute("placeholder") or ""))
        await form_label.fill("Citi Prestige")
        await form_country.fill("USA")
        await pg.locator("#memProfileVault > .card").nth(1).get_by_role("button", name="Save", exact=True).click()
        await pg.wait_for_selector(".vaultitem[data-kind=card] .namedcard", timeout=10000)
        items = [i for i in K.c.get(K.B + "/sentinel/api/vault", headers=K.H).json()["items"] if i["label"] == "Citi Prestige"]
        check("the card is in the Vault by its name and country: number, expiry and CVC UNKNOWN",
              len(items) == 1 and items[0]["kind"] == "card" and items[0]["country"] == "USA" and items[0]["masked"] == "UNKNOWN"
              and items[0]["missing"] == ["number", "expiry", "cvc"], items)
        row = await pg.locator(".vaultitem[data-kind=card]", has_text="Citi Prestige").inner_text()
        check("the Vault card shows the card's country, and that it is a name only, to add the number before paying with it",
              "USA" in row and "UNKNOWN" in row and "add the number" in row, row)
        # the country is optional: a card by its name alone
        await pg.locator("#addCardBtn").click()
        await form_label.fill("UOB One")
        await pg.locator("#memProfileVault > .card").nth(1).get_by_role("button", name="Save", exact=True).click()
        await pg.wait_for_selector(".vaultitem[data-kind=card]:has-text('UOB One')", timeout=10000)
        uob = [i for i in K.c.get(K.B + "/sentinel/api/vault", headers=K.H).json()["items"] if i["label"] == "UOB One"]
        check("a card without a country is saved too (the country is optional)", len(uob) == 1 and uob[0]["country"] == "", uob)
        items += uob
        for it in items:
            K.c.delete(K.B + "/sentinel/api/vault/" + it["id"], headers=K.H)
        if os.path.isdir("/repo-out"):   # OMUSE_E2E_OUT: a picture of the card for a visual check
            await pg.emulate_media(color_scheme="dark")
            await card.screenshot(path="/repo-out/profile_card.png")
            await pg.emulate_media(color_scheme="light")
            shot = [K.vault_item("Citi Prestige", "card", {}, country="USA"), K.vault_item("UOB One", "card", {})]
            await pg.goto(B + "#memory")
            await pg.wait_for_selector("#memProfileVault .vaultitem")
            await pg.screenshot(path="/repo-out/memory_page.png", full_page=False)
            for i in shot:
                K.c.delete(K.B + "/sentinel/api/vault/" + i, headers=K.H)
        check("no page errors", not errs, errs)
        await br.close()


asyncio.run(main())
for k in ("custom:Airline", "custom:Hotel chain", "address:Beach house", "preferred_car_rental"):
    K.c.put(K.B + "/api/profile", json={"key": k, "value": ""}, headers=K.H)
finish()
