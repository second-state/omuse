"""Arrival cards with a real model, from the user's point of view: the shared world, the user's part in the browser
view and the checks for tests/arrival_real.py and tests/arrival_cards*_real.py (see tests/user_common.py for the rule:
only what the user sees is checked).

The user: JORDAN LU, an Australian (profile: name as on the passport, birthday, gender Female, nationality, email,
phone, home address in Sydney); the Vault holds the passport (number and expiry, no site restriction). For each card the
mailbox has the e-ticket of a flight arriving in two days (no return flight) and a hotel booking without a check-out
date. The official sites are the stand-ins at their real addresses (tests/https_standins.py, tests/pages): realistic
forms with a sex radio group, masked DD/MM/YYYY date fields, a nationality list, the trip and accommodation sections,
health and declaration Yes/No questions, a CAPTCHA the agent cannot fill (UK ETA: a face photo instead), and a Submit
that goes to the confirmation page with every value in the address.

What the user does and sees for a card (`fill_card`): they ask Main's chat to fill it in (holiday trip). If the answer
asks something first, they answer once. When the agent asks them to take over, they read the request, look at the page
in the browser view, scroll to the bottom, type the CAPTCHA, press Submit and hand the browser back (Visit Japan Web:
they first log in to their account when asked). Checked: the Travel bot did it; no approval dialog at all; the request
to take over says what is left and does not claim the card was submitted; the page handed over is the official form,
not its confirmation; the confirmation page's address holds the right values (name, sex from the profile, birth date,
nationality, passport number and expiry DD/MM/YYYY from the Vault, flight, arrival date, last and next city = where the
flight came from, departure = arrival + 7 days, the hotel, purpose, every health and declaration answer No); the final
answer quotes the reference the confirmation page shows; the passport number never appears in the chat."""
from __future__ import annotations

import datetime as dt
import os
import re
from dataclasses import dataclass
from urllib.parse import parse_qs, urlsplit

from tests import skills_common as K
from tests import user_common as U
from tests.user_common import check

ARRIVE = dt.date.today() + dt.timedelta(days=2)
DEPART = ARRIVE + dt.timedelta(days=7)
PASSPORT = "E1234567X"
EXPIRY = "2031-05-01"
PROFILE = {"name_en": "JORDAN LU", "gender": "Female", "birthday": "1985-04-12", "nationality": "Australia",
           "email_personal": "jordan.lu@example.com", "phone": "+61 400 000 000",
           "address_home": "12 Harbour St, Sydney NSW 2000, Australia"}
OUT = "/repo-out/arrival" if os.path.isdir("/repo-out") else ""
NAME_KEYS = ("name", "full_name", "family_name", "given_name", "first_name", "middle_name", "last_name", "surname")


def dmy(d: dt.date | str) -> str:
    d = dt.date.fromisoformat(d) if isinstance(d, str) else d
    return d.strftime("%d/%m/%Y")


@dataclass
class Card:
    key: str
    ask: str            # what the user types into Main's chat
    form: str           # the official form's address
    done: str           # the confirmation page's file name
    ref: str            # the reference the confirmation page shows
    flight: str
    airline: str
    origin: tuple[str, str]   # (city, airport code) the flight comes from
    dest: tuple[str, str]
    hotel: tuple[str, str]    # (name, address)
    captcha: str = ""
    login: bool = False       # Visit Japan Web: the user logs in to their account first
    leftover: str = r"captcha|security (?:code|check)|characters|image"   # what the takeover request must name
    fields_url: str = ""      # the page whose named fields the form has, when it is not `form` (Korea: the terms page first)
    trip: bool = True         # the form has the trip and accommodation sections

    def mail(self) -> list[tuple]:
        dep = ARRIVE - dt.timedelta(days=1) if self.key == "uk" else ARRIVE
        o, d = self.origin, self.dest
        return [
            (f"{self.airline} <noreply@{self.airline.lower().replace(' ', '')}.example>",
             f"Your e-ticket: {self.flight} {o[1]}-{d[1]} {ARRIVE.isoformat()}",
             f"E-ticket receipt. Booking ref 9QW3PL. Passenger: LU/JORDAN. {self.flight} departs {o[0]} ({o[1]}) "
             f"{dep.isoformat()} 09:10, arrives {d[0]} ({d[1]}) {ARRIVE.isoformat()} 18:40. One way, economy, 30 kg baggage.", 1),
            (f"{self.hotel[0]} <reservations@stay.example>", f"Booking confirmed: {self.hotel[0]}",
             f"Dear Jordan Lu, your reservation is confirmed. Confirmation HB-55120. Arrival {ARRIVE.isoformat()}, "
             f"check-in from 15:00. {self.hotel[0]}, {self.hotel[1]}. We look forward to welcoming you.", 1),
        ]


CARDS = {
    "sg": Card("sg", "Please fill in my SG Arrival Card for my trip to Singapore — it's a holiday.",
               "https://eservices.ica.gov.sg/sgarrivalcard/", "sgac_done", "DE202611020123", "SQ 231", "Singapore Airlines",
               ("Sydney", "SYD"), ("Singapore", "SIN"), ("Marina Bayfront Hotel", "10 Bayfront Avenue, Singapore 018956"),
               # the real ICA form (rebuilt stand-in, 2026-10-31): no CAPTCHA; the user ticks the declaration and submits
               leftover=r"declar|submit|review"),
    "th": Card("th", "Please fill in my Thailand Digital Arrival Card (TDAC) for my trip — it's a holiday.",
               "https://tdac.immigration.go.th/", "tdac_done", "TH7D3K9Q2", "TG 476", "Thai Airways",
               ("Sydney", "SYD"), ("Bangkok", "BKK"), ("Siam Riverside Hotel", "88 Charoen Krung Road, Bang Rak, Bangkok 10500"), "M4XP8"),
    "my": Card("my", "Please fill in my Malaysia Digital Arrival Card (MDAC) for my trip — it's a holiday.",
               "https://imigresen-online.imi.gov.my/mdac/main", "mdac_done", "MDAC-2611-48213", "MH 122", "Malaysia Airlines",
               ("Sydney", "SYD"), ("Kuala Lumpur", "KUL"), ("Bukit Bintang Grand Hotel", "22 Jalan Bukit Bintang, 55100 Kuala Lumpur"), "K9TW3",
               leftover=r"captcha|puzzle|slide|slider|verif|submit"),   # the real MDAC ends in a slider puzzle
    "kr": Card("kr", "Please fill in my Korea e-Arrival Card for my trip to Seoul — it's a holiday.",
               "https://www.e-arrivalcard.go.kr/", "kr_arrival_done", "KR-EA-77120934", "KE 122", "Korean Air",
               ("Sydney", "SYD"), ("Seoul Incheon", "ICN"), ("Hotel Namsan Gardens", "15 Toegye-ro 20-gil, Jung-gu, Seoul 04637"), "P3HN7",
               # the real site (rebuilt stand-in, 2026-10-31): landing → terms → the form; Submit asks for the emailed code
               leftover=r"captcha|code|verif|submit", fields_url="https://www.e-arrivalcard.go.kr/portal/apply/individual.do"),
    "jp": Card("jp", "Please do my Visit Japan Web registration for my trip to Tokyo — it's a holiday.",
               "https://vjw-lp.digital.go.jp/", "vjw_done", "JP-5521-9087", "NH 880", "ANA",
               ("Sydney", "SYD"), ("Tokyo Haneda", "HND"), ("Shinjuku Granbell Inn", "2-14-5 Kabukicho, Shinjuku-ku, Tokyo 160-0021"), "W8FD2",
               login=True),
    "cn": Card("cn", "Please fill in my China online arrival card for my trip to Shanghai — it's a holiday.",
               "https://s.nia.gov.cn/ArrivalCardFillingPC/", "cn_arrival_done", "CN-ARC-20261102-6613", "MU 562", "China Eastern",
               ("Sydney", "SYD"), ("Shanghai Pudong", "PVG"), ("Bund Riverside Hotel", "99 Zhongshan East 1st Road, Huangpu District, Shanghai"), "T6QZ4",
               # the real site (rebuilt stand-in, 2026-10-31): the passport-page upload is the user's ("captcha" = the
               # document code standing in for it), then the agent fills the form; Submit is the user's
               leftover=r"submit|upload|photo|passport page|review",
               fields_url="https://s.nia.gov.cn/ArrivalCardFillingPC/entry-registation-form"),
    "tw": Card("tw", "Please fill in my Taiwan Arrival Card for my trip to Taipei — it's a holiday.",
               "https://twac.immigration.gov.tw/", "twac_done", "TWAC-90417736", "CI 52", "China Airlines",
               ("Sydney", "SYD"), ("Taipei Taoyuan", "TPE"), ("Xinyi Park Hotel", "6 Songshou Road, Xinyi District, Taipei 110"), "H2VK9",
               # the real TWAC (rebuilt stand-in, 2026-10-31): the user verifies their email (the code is the "captcha"),
               # then the agent fills the form; the declaration and SUBMIT are the user's
               leftover=r"declar|submit|review", fields_url="https://twac.immigration.gov.tw/submit"),
    "id": Card("id", "Please fill in my All Indonesia arrival card for my trip to Bali — it's a holiday.",
               "https://allindonesia.imigrasi.go.id/", "id_arrival_done", "ALLIND-332190", "GA 715", "Garuda Indonesia",
               ("Sydney", "SYD"), ("Denpasar Bali", "DPS"), ("Seminyak Lagoon Resort", "Jl. Kayu Aya No. 8, Seminyak, Bali 80361"),
               leftover=r"certif|declar|submit"),   # the real site (rebuilt stand-in, 2026-10-31): no CAPTCHA, the user certifies
    "ph": Card("ph", "Please do my Philippines eTravel registration for my trip to Manila — it's a holiday.",
               "https://etravel.gov.ph/", "etravel_done", "ETRV-PH-55820117", "PR 210", "Philippine Airlines",
               ("Sydney", "SYD"), ("Manila", "MNL"), ("Makati Palm Suites", "1200 Ayala Avenue, Makati City, Metro Manila"), "B7JQ3"),
    "kh": Card("kh", "Please fill in my Cambodia e-Arrival Card for my trip to Phnom Penh — it's a holiday.",
               "https://arrival.gov.kh/", "kh_arrival_done", "KH-EAC-640219", "SQ 166", "Singapore Airlines",
               ("Singapore", "SIN"), ("Phnom Penh", "PNH"), ("Riverside Palace Hotel", "51 Sisowath Quay, Phnom Penh"), "N4CX6"),
    "uk": Card("uk", "Please apply for my UK ETA for my trip to London — it's a holiday. I work as a software engineer.",
               "https://www.gov.uk/eta", "uk_eta_done", "4402-1187-6631", "QF 1", "Qantas",
               ("Sydney", "SYD"), ("London Heathrow", "LHR"), ("Covent Garden Court Hotel", "8 Bedford Street, London WC2E 9ED"), "",
               leftover=r"photo|picture|selfie|pay|fee", trip=False),
}


def world(mail: list | None) -> None:
    """The user's install: profile, the passport in the Vault (no site restriction), the mailbox (None: no mailbox)."""
    U.setup(mail=mail, vault=False, profile=PROFILE)
    if mail is None:
        disconnect_mail()
    K.vault_item("Passport Jordan", "id_document", {"number": PASSPORT, "name": "LU JORDAN", "expiry": EXPIRY, "country": "AU"}, domains="")


def disconnect_mail() -> None:
    """The user removes their mailbox on Connections (the no-mail case)."""
    gmail = lambda: next(x for x in K.c.get(K.S + "/connections", headers=K.H).json()["connections"] if x["name"] == "gmail")  # noqa: E731
    for acc in gmail().get("accounts", []):
        K.c.delete(f"{K.S}/connections/gmail/accounts/{acc['id']}", headers=K.H)
    check("no mailbox is connected any more", not gmail().get("accounts"), gmail().get("accounts"))


def fresh_trip(mail: list[tuple]) -> None:
    """Between two cards of a suite (a fixture, not a check): the mailbox holds only this trip, and what the agent
    remembered about the previous card's trip is forgotten — each card is its own user story, not a second trip on the
    same day."""
    K.seed_mail(mail)
    mem = K.c.get(K.B + "/api/memory", headers=K.H).json()
    ids = {f["id"] for v in mem.values() if isinstance(v, list) for f in v
           if isinstance(f, dict) and str(f.get("id", "")).startswith("fact_")}
    for fid in ids:
        K.c.delete(f"{K.B}/api/memory/{fid}", headers=K.H)


# ------------------------------------------------------------------ the user's hands in the browser view
async def _img_box(u: U.User) -> tuple[dict, float]:
    """The live picture's box in the app and the agent page's viewport height (CSS px at 1280 wide)."""
    img = u.page.locator(".bview img")
    # the picture is replaced as the page refreshes: measure one that has loaded (as a user's eye waits for it)
    for _ in range(40):
        await u.view_picture()
        box = await img.bounding_box()
        nat = await img.evaluate("i => [i.naturalWidth, i.naturalHeight]")
        if box and nat[0] > 0:
            break
        await u.page.wait_for_timeout(250)
    return box, nat[1] * u.VIEW[0] / max(nat[0], 1)


async def click_at(u: U.User, x: float, y: float) -> None:
    """The user clicks the live picture at the page's viewport point (x, y)."""
    pg = u.page
    box, _ = await _img_box(u)
    k = box["width"] / u.VIEW[0]
    vh = (pg.viewport_size or {"height": 1400})["height"]
    if not 0 <= box["y"] + y * k <= vh - 10:   # bring that part of the picture into the window (no wheel: it goes to the page)
        await pg.evaluate("dy => window.scrollBy(0, dy)", box["y"] + y * k - vh / 2)
        await pg.wait_for_timeout(300)
        box, _ = await _img_box(u)
    await pg.mouse.click(box["x"] + x * k, box["y"] + y * k)
    await pg.wait_for_timeout(1200)


async def scroll_page(u: U.User, dy: float) -> None:
    """The user turns the mouse wheel over the live picture (scrolls the agent's page)."""
    pg = u.page
    box, _ = await _img_box(u)
    await pg.mouse.move(box["x"] + box["width"] / 2, box["y"] + min(box["height"], 300) / 2)
    for _ in range(4):
        await pg.mouse.wheel(0, dy)
        await pg.wait_for_timeout(400)
    await pg.wait_for_timeout(1500)


async def where(u: U.User, url: str, selectors: dict[str, str], *, bottom: bool, vh: float) -> dict[str, tuple[float, float, float, float]]:
    """Where each element sits in the viewport of the page at `url` (the browser view's size), scrolled to the bottom or
    the top — the way the user's eye finds the field in the picture."""
    ctx = await u._br.new_context(viewport={"width": u.VIEW[0], "height": int(round(vh))}, ignore_https_errors=True)
    try:
        p = await ctx.new_page()
        await p.goto(url, wait_until="load", timeout=30000)
        if bottom:
            await p.evaluate("window.scrollTo(0, document.documentElement.scrollHeight)")
            await p.wait_for_timeout(300)
        out = {}
        for name, sel in selectors.items():
            # the visible one (a multi-step form keeps the other steps' fields hidden in the page)
            loc = p.locator(sel).locator("visible=true")
            if await loc.count():
                b = await loc.first.evaluate("e => { const r = e.getBoundingClientRect(); return [r.x, r.y, r.width, r.height]; }")
                out[name] = tuple(b)
        return out
    finally:
        await ctx.close()


async def form_fields(u: U.User, url: str) -> list[str]:
    """The named fields the official form shows (what the user can read on it)."""
    ctx = await u._br.new_context(ignore_https_errors=True)
    try:
        p = await ctx.new_page()
        await p.goto(url, wait_until="load", timeout=30000)
        return await p.evaluate("() => [...new Set([...document.querySelectorAll('form [name]')].map(e => e.name))]")
    finally:
        await ctx.close()


class Takeover:
    """The user's part when the agent asks them to take over: log in (Visit Japan Web) or check, CAPTCHA and Submit."""

    def __init__(self, u: U.User, card: Card, label: str):
        self.u, self.card, self.label = u, card, label
        self.requests: list[str] = []
        self.form_url = ""
        self.after_submit = ""
        self.logged_in = False

    async def __call__(self, pg, request: str) -> None:
        u, card = self.u, self.card
        self.requests.append(request)
        url = await u.view_url()
        print(f"INFO {self.label} takeover: {request!r} url {url!r}")
        await u.take_over()
        _, vh = await _img_box(u)
        if card.login and urlsplit(url).path in ("", "/", "/vjw.html") and "vjw-lp.digital.go.jp" in url:
            await scroll_page(u, -3000)
            at = await where(u, url, {"email": "#email", "password": "#password", "login": "button[type=submit]"}, bottom=False, vh=vh)
            for k, text in (("email", PROFILE["email_personal"]), ("password", "Sakura-Blossom-26")):
                x, y, w, h = at[k]
                await click_at(u, x + w / 2, y + h / 2)
                # the agent may have filled the email already: the user types over what is there
                await pg.keyboard.press("Control+a")
                await pg.keyboard.press("Backspace")
                await pg.keyboard.type(text, delay=30)
                await pg.wait_for_timeout(800)
            x, y, w, h = at["login"]
            await click_at(u, x + w / 2, y + h / 2)
            for _ in range(10):
                if "vjw_entry" in await u.view_url():
                    self.logged_in = True
                    break
                await pg.wait_for_timeout(1000)
            print(f"INFO {self.label} logged in: {self.logged_in} url {await u.view_url()!r}")
            return
        if card.done in url:   # already the confirmation: nothing for the user to do
            self.form_url = self.form_url or url
            return
        self.form_url = url
        if OUT:
            await u.view_picture(os.path.join(OUT, f"{U.SUITE}-{card.key}-handed-over.png"))
        await scroll_page(u, 3000)
        at = await where(u, url, {"captcha": "input[name=captcha]", "submit": "button[type=submit]",
                                  "declaration": "input[name=declaration], input[name=declarationCheck]"}, bottom=True, vh=vh)
        if "declaration" in at:   # the declaration the user reads and ticks before Submit (the SG Arrival Card)
            x, y, w, h = at["declaration"]
            await click_at(u, x + w / 2, y + h / 2)
            await pg.wait_for_timeout(500)
        if card.captcha and "captcha" in at:
            x, y, w, h = at["captcha"]
            await click_at(u, x + w / 2, y + h / 2)
            await pg.keyboard.press("Control+a")
            await pg.keyboard.press("Backspace")
            await pg.keyboard.type(card.captcha, delay=40)
            await pg.wait_for_timeout(1500)
        if OUT:
            await u.view_picture(os.path.join(OUT, f"{U.SUITE}-{card.key}-before-submit.png"))
        if "submit" in at:
            x, y, w, h = at["submit"]
            await click_at(u, x + w / 2, y + h / 2)
        for i in range(12):
            self.after_submit = await u.view_url()
            if card.done in self.after_submit:
                break
            if i == 5 and card.captcha and "captcha" in at:   # the click missed: Enter in the code field submits too
                x, y, w, h = at["captcha"]
                await click_at(u, x + w / 2, y + h / 2)
                await pg.keyboard.press("Enter")
            await pg.wait_for_timeout(1000)
        if OUT:
            await u.view_picture(os.path.join(OUT, f"{U.SUITE}-{card.key}-after-submit.png"))
        print(f"INFO {self.label} after Submit the URL bar shows {self.after_submit!r}")


ASKED_DEFAULTS = re.compile(r"\b(gender|sex|symptom|fever|criminal|convict|refused|deport|declare|customs|other names|passport number)\b", re.I)
_SUBMITTED = re.compile(r"\b(has been|have been|was|successfully|already) submitted\b", re.I)
_NEGATED = re.compile(r"\b(no|not|nothing|never|neither|nor|none)\b|n['’]t\b", re.I)


class _Claims:
    """"…has been submitted" said as done — not "No registration has been submitted" (the sentence negates it)."""

    @staticmethod
    def search(text: str):
        for m in _SUBMITTED.finditer(text or ""):
            start = max(text.rfind(".", 0, m.start()), text.rfind("\n", 0, m.start())) + 1
            if not _NEGATED.search(text[start:m.start()]):
                return m
        return None


CLAIMS_SUBMITTED = _Claims()


def norm(s: str) -> str:
    return re.sub(r"[\s-]", "", s or "").upper()


def check_values(label: str, card: Card, q: dict, fields: list[str], *, flight: str | None = None, hotel: str | None = None,
                 accom: tuple[str, ...] = ("HOTEL",)) -> None:
    """The confirmation page's address: what the form held when the user pressed Submit. A field the agent left blank
    is the user's to fill in before submitting (they review the page anyway): it counts as filled with the user's own
    value and is listed, and only a wrong value — or a form left mostly blank — fails."""
    has = set(fields)
    city, code = card.origin if card.trip else ("", "")
    mine = {"name": "JORDAN LU", "full_name": "JORDAN LU", "family_name": "LU", "last_name": "LU", "surname": "LU", "given_name": "JORDAN", "given_names": "JORDAN",
            "first_name": "JORDAN", "sex": "F", "dob": "12/04/1985", "passport_no": PASSPORT, "passport_expiry": dmy(EXPIRY),
            "nationality": "AUS", "passport_country": "AUS", "country_residence": "AUS", "email": PROFILE["email_personal"],
            "phone": "+61 400 000 000", "flight_no": flight or card.flight, "mode": "AIR", "arrival_date": dmy(ARRIVE),
            "last_city": city, "next_city": city, "departure_date": dmy(DEPART), "accommodation": hotel or (card.hotel[0] if card.trip else ""),
            "accom_type": accom[0], "purpose": "HOLIDAY", "home_address": "12 Harbour St", "occupation": "engineer"}
    mine.update({k: "No" for k in fields if k.startswith(("h_", "d_"))})
    blank = [k for k in fields if k in mine and mine[k] and not str(q.get(k, "")).strip()]
    print(f"INFO {label}: left blank for the user to fill in before submitting: {blank}")
    check(f"{label}: at most 3 fields left blank for the user ({len(blank)})", len(blank) <= 3, blank)
    q = {**q, **{k: mine[k] for k in blank}}
    names = [k for k in NAME_KEYS if k in has]
    joined = " ".join(q.get(k, "") for k in names).upper()
    fam = next((q.get(k, "") for k in ("family_name", "last_name", "surname") if k in has), None)
    check(f"{label}: the form held the name as on the passport (from the profile)",
          "JORDAN" in joined and re.search(r"\bLU\b", joined) is not None and (fam is None or fam.strip().upper() == "LU"),
          {k: q.get(k) for k in names})
    check(f"{label}: sex = Female (from the profile), birth date 12/04/1985 in the field's DD/MM/YYYY order",
          q.get("sex") == "F" and q.get("dob") == "12/04/1985", {k: q.get(k) for k in ("sex", "dob")})
    check(f"{label}: the passport number and its expiry (01/05/2031, DD/MM/YYYY) from the Vault",
          norm(q.get("passport_no", "")) == PASSPORT and ("passport_expiry" not in has or q.get("passport_expiry") == dmy(EXPIRY)),
          {"passport_no": "(right)" if norm(q.get("passport_no", "")) == PASSPORT else q.get("passport_no"), "passport_expiry": q.get("passport_expiry")})
    nat = [k for k in ("nationality", "passport_country", "country_residence") if k in has]
    check(f"{label}: nationality / country of issue = Australia; email and phone from the profile",
          all(q.get(k) == "AUS" for k in nat) and q.get("email", "").lower() == PROFILE["email_personal"]
          and ("phone" not in has or re.sub(r"\D", "", q.get("phone", "")).endswith("400000000")),   # Korea asks no phone
          {k: q.get(k) for k in (*nat, "email", "phone")})
    if card.trip:
        fl = norm(flight or card.flight)
        check(f"{label}: the flight, mode Air and arrival date ({dmy(ARRIVE)}) from the e-ticket",
              norm(q.get("flight_no", "")) == fl and q.get("mode") == "AIR" and q.get("arrival_date") == dmy(ARRIVE),
              {k: q.get(k) for k in ("flight_no", "mode", "arrival_date")})
        city, code = card.origin
        here = lambda v: city.upper() in (v or "").upper() or code in (v or "").upper()   # noqa: E731
        if "last_city" in has:   # some forms ask only for the country (MDAC)
            check(f"{label}: last city = {city}, where the flight came from",
                  here(q.get("last_city")), q.get("last_city"))
        if "next_city" in has:
            check(f"{label}: next city = {city} too (the city the flight came from, no onward flight)",
                  here(q.get("next_city")), q.get("next_city"))
        if "departure_date" in has:   # China's card asks none unless a departure is confirmed
            check(f"{label}: departure date = arrival + 7 days ({dmy(DEPART)}), no return flight in the mail",
                  q.get("departure_date") == dmy(DEPART), q.get("departure_date"))
        h = hotel or card.hotel[0]
        word = max(h.split(), key=len).lower()
        check(f"{label}: the accommodation is {h}, type {' or '.join(accom)}, purpose Holiday",
              word in q.get("accommodation", "").lower() and ("accom_type" not in has or q.get("accom_type") in accom)
              and ("purpose" not in has or q.get("purpose") == "HOLIDAY"),   # some forms ask no purpose (MDAC)
              {k: q.get(k) for k in ("accommodation", "accom_type", "purpose")})
    else:
        check(f"{label}: home address from the profile, occupation from the request",
              "harbour" in q.get("home_address", "").lower() and "engineer" in q.get("occupation", "").lower(),
              {k: q.get(k) for k in ("home_address", "occupation")})
    health = [k for k in fields if k.startswith("h_")]
    decl = [k for k in fields if k.startswith("d_")]
    if health:
        check(f"{label}: every health question answered No without asking", all(q.get(k) == "No" for k in health),
              {k: q.get(k) for k in health})
    if decl:   # some forms ask none (MDAC)
        check(f"{label}: every declaration question answered No without asking", all(q.get(k) == "No" for k in decl),
              {k: q.get(k) for k in decl})
    if card.captcha:
        check(f"{label}: the CAPTCHA is the one the user typed", q.get("captcha", "").upper() == card.captcha, q.get("captcha"))


async def fill_card(u: U.User, card: Card, *, label: str = "", ask: str = "", flight: str | None = None,
                    hotel: str | None = None, follow_up: str = "", accom: tuple[str, ...] = ("HOTEL",)) -> list[U.Answer]:
    """One card from the user's side (module doc): ask, answer one question if any, take over, submit, check."""
    label = label or card.key
    to = Takeover(u, card, label)
    fields = await form_fields(u, card.fields_url or card.form.replace("vjw-lp.digital.go.jp/", "vjw-lp.digital.go.jp/vjw_entry.html"))
    a = await u.ask(ask or card.ask, label=label, on_takeover=to, timeout=1500)
    approvals, answers = list(a.approvals), [a]
    if not to.form_url and not a.timed_out:
        # the answer asked something before the form was handed over: the user answers once, in the same chat
        # only the questions count: "I filled in your passport number" is no question
        asked = [x for x in re.split(r"(?<=[.?!])\s+|\n", a.text) if "?" in x or re.search(r"\b(please (tell|provide|confirm|send|let me know))\b", x, re.I)]
        check(f"{label}: the question did not ask for anything that has a default (health, declarations, sex, passport)",
              not any(ASKED_DEFAULTS.search(x) for x in asked), asked or a.text[:500])
        a = await u.ask(follow_up or "It's a holiday, travelling alone. My e-ticket and hotel booking are in my email — please take "
                        "everything else from there and from my profile, and go ahead and fill in the form.",
                        label=f"{label} follow-up", new_chat=False, on_takeover=to, timeout=1500)
        approvals += a.approvals
        answers.append(a)
    check(f"{label}: the answer came (no time-out)", bool(a.text.strip()) and not a.timed_out, a.text[:300])
    check(f"{label}: the Travel bot did it (the link to continue in Travel)", "Travel" in (answers[0].bot_link or a.bot_link),
          [x.bot_link for x in answers])
    check(f"{label}: no approval dialog at all (browser auto-approve, the passport fill included)", not approvals, approvals)
    if card.login:
        check(f"{label}: the agent asked the user to take over to log in to their account, and they did",
              to.logged_in and any(re.search(r"log[- ]?in|sign[- ]?in|account", r, re.I) for r in to.requests), to.requests)
    finish = [r for r in to.requests if not re.search(r"log[- ]?in|sign[- ]?in", r, re.I) or re.search(card.leftover, r, re.I)]
    check(f"{label}: the user was asked to take over to finish the form, naming what is left ({card.leftover})",
          bool(to.form_url) and any(re.search(card.leftover, r, re.I) for r in finish), to.requests)
    check(f"{label}: nothing claims the card was submitted before the user did",
          not any(CLAIMS_SUBMITTED.search(r) for r in to.requests) and not any(CLAIMS_SUBMITTED.search(x.text) for x in answers[:-1]),
          to.requests)
    host = urlsplit(card.form).hostname
    check(f"{label}: the page handed over is the official form ({host}), not its confirmation (the agent did not submit)",
          host in to.form_url and card.done not in to.form_url, to.form_url)
    check(f"{label}: the user's Submit reaches the confirmation page", card.done in to.after_submit, to.after_submit)
    if card.done in to.after_submit:
        q = {k: v[0] for k, v in parse_qs(urlsplit(to.after_submit).query, keep_blank_values=True).items()}
        check_values(label, card, q, fields, flight=flight, hotel=hotel, accom=accom)
    check(f"{label}: after the hand-back the answer quotes the confirmation's reference ({card.ref})",
          norm(card.ref) in norm(a.text), a.text[:500])
    shown = await u.thread_text() + " " + " ".join(approvals) + " " + " ".join(to.requests)
    check(f"{label}: the passport number never appears in the chat (messages, cards, requests, the answer)",
          PASSPORT not in shown and PASSPORT not in a.text, [m.start() for m in re.finditer(PASSPORT, shown)][:3])
    return answers


def run_cards(keys: list[str]) -> None:
    """A suite of cards: one chat per card, the mailbox holding that trip's e-ticket and hotel booking."""
    # OMUSE_E2E_ONLY=kr (run_in_docker.sh): only these cards, to rerun one with its trace
    only = os.environ.get("OMUSE_E2E_ONLY", "").strip()
    if only:
        keys = [k for k in keys if k in only.split(",")]
    world(CARDS[keys[0]].mail())

    async def main():
        async with U.User(viewport=(1440, 1500)) as u:
            for i, key in enumerate(keys):
                if i:
                    fresh_trip(CARDS[key].mail())
                await fill_card(u, CARDS[key])

    U.run(main)


PARIS = ARRIVE + dt.timedelta(days=30)
EU_MAIL = [
    ("Qantas <noreply@qantas.example>", f"Your e-ticket: QF 33 SYD-CDG {PARIS.isoformat()}",
     f"E-ticket receipt. Booking ref 7XK2LM. Passenger: LU/JORDAN. QF 33 departs Sydney (SYD) {(PARIS - dt.timedelta(days=1)).isoformat()} "
     f"21:40, arrives Paris (CDG) {PARIS.isoformat()} 07:15. One way, economy.", 1),
    ("Hôtel Rive Gauche <reservations@stay.example>", "Booking confirmed: Hôtel Rive Gauche",
     f"Dear Jordan Lu, your reservation is confirmed. Arrival {PARIS.isoformat()}. Hôtel Rive Gauche, 12 Rue Jacob, 75006 Paris.", 1),
]


async def etias(u: U.User) -> None:
    """EU / Schengen: nothing to fill while ETIAS is not live — the agent checks the official status and says so."""
    fresh_trip(EU_MAIL)
    a = await u.ask("I'm flying to Paris next month. Do I need to fill in an arrival card or ETIAS or anything before the trip?",
                    label="etias", timeout=900)
    t = a.text
    check("etias: the answer came (no time-out)", bool(t.strip()) and not a.timed_out, t[:300])
    check("etias: the Travel bot answered (the link to continue in Travel)", "Travel" in a.bot_link, a.bot_link)
    check("etias: it says ETIAS is not in operation yet, so there is nothing to apply for now",
          re.search(r"ETIAS", t) is not None and re.search(r"not (yet )?(in operation|live|operational|launched|running|open|available|required)"
                                                          r"|isn'?t (yet )?(live|operational|in operation|required|open)|no(t)? (need|application)|"
                                                          r"(doesn'?t|does not|don'?t|do not) (yet )?(need|have to|apply)|expected (to (start|launch) )?in 2027|"
                                                          r"(start|launch)\w* (in|late|from) .{0,12}2027", t, re.I) is not None, t[:700])
    check("etias: it explains EES (registration at the border, no form)", re.search(r"\bEES\b|Entry/?Exit System", t, re.I) is not None, t[:700])
    check("etias: it points to the official EU site", any("europa.eu" in x for x in a.links) or "europa.eu" in t, (a.links, t[:300]))
    check("etias: no form, no takeover and no approval dialog", not a.takeovers and not a.approvals, (a.takeovers, a.approvals))
