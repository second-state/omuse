"""Booking a flight, a hotel room or a Michelin table with a real model, from the user's point of view: the real-model
suites travel_flight_real.py, travel_hotel_real.py and travel_michelin_real.py.

The user's world: their travel agency's MCP connector added on Connections (tests/fake_travel.py: the agency's offers
and its booking site, shop.test:8097), their card and passport in the Vault, the real model in Settings → Models. In
the browser the user asks Main's chat to find and book; if the answer stops at the options they say "yes, book it".
Settings → Agent's "auto-approve browser actions" is on, as on a fresh install: nothing asks — not the purchase, not
the card or passport fills from the Vault. When asked to take over the browser they look at the booking page in the
browser view, check it is filled in and press Pay / Confirm in the live picture, then hand it back. Afterwards they
open "My trips" on the booking site.

Only what the user sees is checked: the options in the chat, that no approval dialog appeared, the takeover request,
the booking page (its address in the browser view, the page itself, the live picture's filled fields), the booking on
the site's "My trips" (what, when, for whom, the card and passport endings, the amount), the amount and the booking
reference in the answer, and that no card or passport number shows anywhere in the chat. What
the right booking is comes from the agency's own data (the same offers the agency's site would show the user)."""
from __future__ import annotations

import datetime as dt
import os
import re
import sys

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from tests import fake_travel as F  # noqa: E402  (the agency's data: what its site offers)
from tests import user_common as U  # noqa: E402
from tests.user_common import check  # noqa: E402

SITE = "http://shop.test:8097"
EMAIL = "lucas@example.com"
WHO = "I'm Lucas Lu, lucas@example.com, +65 8000 0000."
SECRETS = ("4111 1111 1111 1111", "4111111111111111", "4111-1111-1111-1111", "E1234567X")


def _day(n: int) -> dt.date:
    return dt.date.today() + dt.timedelta(days=n)


def _flight() -> dict:
    d = _day(21)
    direct = sorted((o for o in F.flights_search({"origin": "SIN", "destination": "TYO", "date": str(d)})["offers"] if o["stops"] == 0),
                    key=lambda o: o["fare_amount"])
    o = direct[0]
    return {"kind": "flight", "name": o["flight_no"], "label": o["title"], "amount": float(o["fare_amount"]), "currency": "SGD",
            "when": [str(d)], "party": "1 adult", "passport": True, "card": True, "pay": "Pay now",
            "ask": f"Find me the cheapest direct flight from Singapore to Tokyo on {d:%B} {d.day}, {d.year} for one adult. "
                   "I book through my travel agency, the Travel connector: use its flight search and its booking page, not "
                   f"the airline's site. {WHO}",
            "follow": "Book the cheapest direct one on my agency's booking page. My passport and card are in the vault."}


def _hotel() -> dict:
    ci, co = _day(28), _day(31)
    best = None
    for h in F.hotels_search({"city": "Tokyo", "check_in": str(ci), "check_out": str(co), "guests": 2, "min_stars": 4})["hotels"]:
        if h["stars"] != 4:
            continue
        for r in F.hotel_rooms({"hotel_id": h["hotel_id"], "check_in": str(ci), "check_out": str(co), "guests": 2})["rooms"]:
            if r["cancellation"].startswith("Free") and (best is None or r["total_amount"] < best[1]["total_amount"]):
                best = (h, r)
    h, r = best
    return {"kind": "hotel", "name": h["name"], "label": f"{h['name']} {r['room_type']}", "amount": float(r["total_amount"]),
            "currency": "SGD", "when": [str(ci), str(co)], "party": "2 guests", "passport": False,
            "card": r["payment"].startswith("Prepay"), "pay": "Confirm booking",
            "ask": f"Find the cheapest 4-star hotel in Tokyo with free cancellation, check-in {ci:%B} {ci.day}, check-out "
                   f"{co:%B} {co.day}, {co.year}, 2 guests. {WHO}",
            "follow": "Book the cheapest 4-star room with free cancellation for me."}


def _restaurant() -> dict:
    d = _day(21)
    r = F.michelin_search({"city": "Tokyo", "distinction": "2 stars"})["restaurants"][0]
    av = F.restaurant_availability({"restaurant_id": r["restaurant_id"], "date": str(d), "party_size": 2})
    slot = min(av["slots"], key=lambda s: abs(int(s["time"][:2]) * 60 + int(s["time"][3:]) - 19 * 60))
    dep = float(av["deposit_due_now"].split()[-1].replace(",", ""))
    return {"kind": "restaurant", "name": r["name"], "label": r["name"], "amount": dep, "currency": "JPY",
            "when": [str(d), f"Time: {slot['time']}"], "party": "Party size: 2", "passport": False, "card": dep > 0,
            "pay": "Confirm booking",
            "ask": f"Book a table for 2 at a 2-star Michelin restaurant in Tokyo on {d:%B} {d.day}, {d.year} around 7pm. "
                   "Lucas Lu, lucas@example.com, +65 8000 0000. Use my card in the vault for any deposit.",
            "follow": "Yes, book the 2-star restaurant at the time closest to 7pm, card from the vault for the deposit."}


SCENARIOS = {"flight": _flight, "hotel": _hotel, "restaurant": _restaurant}


def _amounts(text: str) -> list[float]:
    return [float(x.replace(",", "")) for x in re.findall(r"\d[\d,]*(?:\.\d+)?", text)]


def _has_amount(text: str, amount: float) -> bool:
    return any(abs(x - amount) < 0.01 for x in _amounts(text))


async def _read(u: U.User, url: str) -> str:
    """The page at `url` in a tab of the user's own browser, as text."""
    p = await u.ctx.new_page()
    try:
        await p.goto(url, wait_until="load", timeout=30000)
        return " ".join((await p.inner_text("body")).split())
    finally:
        await p.close()


async def _bottom_layout(u: U.User, url: str, selectors: dict[str, str]) -> dict[str, tuple[float, float, float, float]]:
    """Where each element sits in the view once the page at `url` is scrolled to the end (the browser view's size)."""
    ctx = await u._br.new_context(viewport={"width": u.VIEW[0], "height": u.VIEW[1]})
    try:
        p = await ctx.new_page()
        await p.goto(url, wait_until="load", timeout=30000)
        await p.evaluate("() => window.scrollTo(0, document.documentElement.scrollHeight)")
        await p.wait_for_timeout(300)
        out = {}
        for name, sel in selectors.items():
            if await p.locator(sel).count():
                b = await p.locator(sel).first.bounding_box()
                if b:
                    out[name] = (b["x"], b["y"], b["width"], b["height"])
        return out
    finally:
        await ctx.close()


async def _scroll(u: U.User, dy: int) -> None:
    pg = u.page
    box = await pg.locator(".bview img").bounding_box()
    await pg.mouse.move(box["x"] + box["width"] / 2, box["y"] + box["height"] / 2)
    for _ in range(4):
        await pg.mouse.wheel(0, dy)
        await pg.wait_for_timeout(300)
    await pg.wait_for_timeout(2500)   # the live picture refreshes


async def _press(u: U.User, x: float, y: float) -> None:
    """The user clicks the live picture at the page's point (x, y), scrolling the app first when that point of the
    picture is below the window (U.User.click_view clicks where the picture would be, even off screen)."""
    img = u.page.locator(".bview img")
    box = await img.bounding_box()
    k = box["width"] / u.VIEW[0]
    await img.click(position={"x": x * k, "y": y * k})
    await u.page.wait_for_timeout(2500)


async def _book(kind: str) -> None:
    s = SCENARIOS[kind]()
    print(f"INFO the right booking (the agency's data): {s['label']} {s['currency']} {s['amount']:.2f} {s['when']} {s['party']}")
    review: dict = {}

    async def on_takeover(pg, request: str) -> None:
        """The user reviews the booking page in the browser view and pays (or confirms) in the live picture."""
        review["request"] = request
        addr = await u.view_url()
        review["address"] = addr
        check(f"{kind}: the browser view shows the booking site's page for the {kind}", f"shop.test:8097/book/{kind}/" in addr, addr)
        if "shop.test:8097/book/" not in addr:
            return
        page = await _read(u, addr)
        review["page"] = page
        print(f"INFO {kind}: the booking page: {page[:600]}")
        check(f"{kind}: the booking page is for {s['label']}, on {', '.join(s['when'])}, {s['party']}",
              s["name"] in page and all(w in page for w in s["when"]) and s["party"] in page, page[:600])
        check(f"{kind}: the booking page's total is {s['currency']} {s['amount']:,.2f}", _has_amount(page, s["amount"]), page[:600])
        await u.take_over()
        # the top of the form: the traveller's details are filled in
        await _scroll(u, -1500)
        top = await u.layout(addr, {"name": "#full_name", "email": "#email", "phone": "#phone"})
        ink = await u.view_ink(top)
        print(f"INFO {kind}: ink in the fields at the top: {ink}")
        await u.view_picture(os.path.join(os.environ.get("OMUSE_E2E_SHOTS", "/repo-out/shots"), f"review-{kind}-top.png"))
        check(f"{kind}: the page in the browser view has the traveller's name, email and phone filled in",
              len(ink) == 3 and all(v > 0.004 for v in ink.values()), ink)
        # the end of the form: the card, and the button
        await _scroll(u, 1500)
        bottom = await _bottom_layout(u, addr, {"passport": "#passport", "card_number": "#card_number", "card_expiry": "#card_expiry",
                                                "button": "button[type=submit]"})
        await u.view_picture(os.path.join(os.environ.get("OMUSE_E2E_SHOTS", "/repo-out/shots"), f"review-{kind}-bottom.png"))
        if s["passport"] and "passport" in bottom and bottom["passport"][1] > 0:
            ink = await u.view_ink({"passport": bottom["passport"]})
            print(f"INFO {kind}: ink in the passport field: {ink}")
            check(f"{kind}: the passport number is filled in on the page", ink.get("passport", 0) > 0.004, ink)
        if s["card"]:
            ink = await u.view_ink({k: bottom[k] for k in ("card_number", "card_expiry") if k in bottom})
            print(f"INFO {kind}: ink in the card fields: {ink}")
            check(f"{kind}: the card is filled in on the page", ink and all(v > 0.004 for v in ink.values()), ink)
        if "button" in bottom:
            x, y, w, h = bottom["button"]
            await _press(u, x + w / 2, y + h / 2)   # the user presses Pay now / Confirm booking
            review["pressed"] = True
            await u.view_picture(os.path.join(os.environ.get("OMUSE_E2E_SHOTS", "/repo-out/shots"), f"review-{kind}-after.png"))

    async with U.User() as u:
        a = await u.ask(s["ask"], label=f"{kind} ask", on_takeover=on_takeover, timeout=1200)
        approvals, takeovers = list(a.approvals), list(a.takeovers)
        options = await u.thread_text()
        trips = await _read(u, f"{SITE}/trips?email={EMAIL}")
        if "Booking reference" not in trips and not takeovers:
            # the user has seen the options and says which one to book
            a = await u.ask(s["follow"], label=f"{kind} book it", new_chat=False, on_takeover=on_takeover, timeout=1200)
            approvals += a.approvals
            takeovers += a.takeovers
            trips = await _read(u, f"{SITE}/trips?email={EMAIL}")
        chat = await u.thread_text()
        print(f"INFO {kind}: My trips: {trips[:900]}")

        # the options
        check(f"{kind}: the chat shows the option(s) with prices, {s['label']} among them",
              s["name"] in options and len(U.PRICE.findall(options)) >= 1, options[:800])
        # no dialogs: auto-approve is on (the purchase, the Vault fills and the clicks go through without asking)
        check(f"{kind}: no approval dialog asked the user (auto-approve is on)", not approvals, approvals)
        print(f"INFO {kind}: takeover requests: {takeovers}")
        # the last click is the user's: the filled page is handed over, the user presses it in the live picture
        check(f"{kind}: the filled booking page was handed over for the user to press {s['pay']}", bool(takeovers), takeovers)
        check(f"{kind}: the user found the button in the live picture and pressed {s['pay']}", review.get("pressed"), review.get("address"))
        # the booking site: My trips
        refs = re.findall(r"Booking reference: (TRV-[A-Z0-9]+)", trips)
        check(f"{kind}: My trips on the booking site shows one confirmed booking", len(refs) == 1, trips[:600])
        check(f"{kind}: the booking is {s['label']}, on {', '.join(s['when'])}, {s['party']}",
              s["name"] in trips and all(w in trips for w in s["when"]) and s["party"] in trips, trips[:900])
        check(f"{kind}: booked for Lucas Lu, {EMAIL}", re.search(r"Booked for: (Lucas Lu|LU LUCAS|Lu Lucas)", trips, re.I) and EMAIL in trips, trips[:900])
        if s["passport"]:
            check(f"{kind}: the passport from the Vault is on the booking (ending 567X)", "Passport: ending 567X" in trips, trips[:900])
        if s["card"]:
            check(f"{kind}: the Vault card paid (ending 1111)", "Card: ending 1111" in trips, trips[:900])
        if s["card"]:
            check(f"{kind}: the site charged {s['currency']} {s['amount']:,.2f} and the answer gives that amount",
                  _has_amount(trips, s["amount"]) and _has_amount(a.text, s["amount"]), (trips[-300:], a.text[:600]))
        else:
            check(f"{kind}: the booking's total is {s['currency']} {s['amount']:,.2f}, nothing charged now",
                  _has_amount(trips, s["amount"]) and "Card: none" in trips, trips[-300:])
        check(f"{kind}: the answer gives the booking reference", bool(refs) and refs[0] in a.text, (refs, a.text[:600]))
        # what must never show
        leaked = [x for x in SECRETS if x in chat]
        check(f"{kind}: no card or passport number in the chat", not leaked, leaked)


def main(kind: str) -> None:
    U.setup(travel_connector=True)
    U.run(lambda: _book(kind))
