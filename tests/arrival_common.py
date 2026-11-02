"""Arrival cards on the REAL official sites with a real model, from the user's point of view: the user's install, their
part in the browser view and the checks for tests/arrival_real.py, tests/arrival_cards*_real.py and
tests/arrival_address*_real.py (see tests/user_common.py for the rule: only what the user sees is checked).

Every card opens its real official site and stops at the first step the user owns — a Cloudflare / Turnstile check, a
sign-in to the user's own account, the passport photo, the emailed code when no mailbox is connected, or the user's
own review and Submit. Nothing is ever submitted or paid: the test never presses Submit, Pay or Register, never uploads
a passport, and never solves or gets around a bot check (a site that blocks automation is handed over at the block).

The user: JORDAN LU, an Australian — clearly fake test data (profile: name as on the passport, birthday, gender
Female, nationality, country of birth, phone, home address in Sydney; email michael@michaelyuan.com, the test inbox);
the Vault holds a fake passport (number and expiry, no site restriction). The trip (the flight arriving in two days, no
return flight, and the hotel) is in the request. The Taiwan card and the UK ETA verify the email with a code: their
suites connect the test inbox as a generic IMAP account when OMUSE_E2E_IMAP_* are set (read-only use: the agent reads
the code mail; nothing is sent or deleted) — without them only the email-code part is skipped (a SKIP line) and the
hand-over is at the email step.

What the user does and sees for a card (`fill_card`): they ask Main's chat to fill it in (holiday trip). If the answer
asks something first, they answer once. When the agent asks them to take over, they look at the page in the browser
view and hand it back without pressing anything. Checked (state and facts, never the model's words): the Travel bot did
it; no approval dialog at all; a takeover card came and the page handed over is on the official site, inside the card's
form; a form code fills is handed over on its FINAL page — the one with the user's Submit (read from the page's
elements; a multi-page form shows only its last pieces there, the whole card shows only after Submit, which is the
user's) — and a site that stops earlier is handed over at that step; nothing is submitted (the block code appends says
the card waits on the user); the passport number never appears in the chat. No per-field value check (user's
decision, 2-Nov-2026)."""
from __future__ import annotations

import datetime as dt
import json
import os
import re
from dataclasses import dataclass
from urllib.parse import urlsplit

from tests import user_common as U
from tests.user_common import check

ARRIVE = dt.date.today() + dt.timedelta(days=2)
DEPART = ARRIVE + dt.timedelta(days=7)
PASSPORT = "E1234567X"
EXPIRY = "2031-05-01"
EMAIL = "michael@michaelyuan.com"
PROFILE = {"name_en": "JORDAN LU", "gender": "Female", "birthday": "1985-04-12", "nationality": "Australia",
           "birthplace": "Australia", "email_personal": EMAIL, "phone": "+61 400 000 000",
           "address_home": "12 Harbour St, Sydney NSW 2000, Australia", "home_city": "SYD", "home_country": "AU"}
OUT = "/repo-out/arrival" if os.path.isdir("/repo-out") else ""
IMAP = {k: os.environ.get(f"OMUSE_E2E_{k}", "") for k in ("IMAP_HOST", "IMAP_PORT", "IMAP_USER", "IMAP_PASSWORD",
                                                          "SMTP_HOST", "SMTP_PORT")}


@dataclass
class Card:
    key: str
    ask: str            # what the user types into Main's chat (the trip details follow it, see `details`)
    form: str           # the official site (its host is where the hand-over must be)
    flight: str
    origin: tuple[str, str]   # (city, airport code) the flight comes from
    dest: tuple[str, str]
    hotel: tuple[str, str]    # (name, address)
    # the first step that is the user's on the real site (mapped 2026-11-02): form = the filled form for their review,
    # check = a Cloudflare / Turnstile check, signin = their own account's sign-in, photo = the passport photo upload,
    # email = the emailed verification code
    handover: str = "form"
    # after the user's step code fills a form on (Taiwan's traveller form after the emailed code)
    then_form: bool = False
    # the site verifies the email with a code (agent/email_code.rs): with a mailbox, code reads and types it
    email_code: bool = False
    trip: bool = True         # the request carries the trip (the UK ETA asks no trip)
    # the address of the form flow on the real site: the page handed over is inside it (not the home page)
    path: str = ""
    # the flight that leaves the country, for a card that requires it (Taiwan's itinerary step: the departure by air
    # needs its flight code and number); "" = one way
    leave: str = ""
    # the stay's phone number, for a card that requires it (Cambodia's "Hotel contact number"); "" = not told
    hotel_phone: str = ""

    def flight_text(self) -> str:
        o, d = self.origin, self.dest
        if self.leave:
            return (f"I fly {self.flight} from {o[0]} ({o[1]}) to {d[0]} ({d[1]}), arriving on {ARRIVE.isoformat()}, and "
                    f"fly back on {self.leave} on {DEPART.isoformat()}.")
        return f"I fly {self.flight} from {o[0]} ({o[1]}) to {d[0]} ({d[1]}), arriving on {ARRIVE.isoformat()}, one way."

    def details(self) -> str:
        """The trip as the user tells it: the flight and the hotel."""
        phone = f" (the hotel's phone: {self.hotel_phone})" if self.hotel_phone else ""
        return f"{self.flight_text()} I'm staying at the {self.hotel[0]}, {self.hotel[1]}{phone}."


CARDS = {
    "sg": Card("sg", "Please fill in my SG Arrival Card for my trip to Singapore — it's a holiday.",
               "https://eservices.ica.gov.sg/sgarrivalcard/", "SQ 231",
               ("Sydney", "SYD"), ("Singapore", "SIN"), ("Marina Bay Sands", "10 Bayfront Avenue, Singapore 018956"), path="/arrivalcard/sgac/"),
    # the home page's Cloudflare Turnstile comes before any field: the hand-over is at the check
    "th": Card("th", "Please fill in my Thailand Digital Arrival Card (TDAC) for my trip — it's a holiday.",
               "https://tdac.immigration.go.th/", "TG 476",
               ("Sydney", "SYD"), ("Bangkok", "BKK"), ("Siam Riverside Hotel", "88 Charoen Krung Road, Bang Rak, Bangkok 10500"),
               handover="check"),
    "my": Card("my", "Please fill in my Malaysia Digital Arrival Card (MDAC) for my trip — it's a holiday.",
               "https://imigresen-online.imi.gov.my/mdac/main", "MH 122",
               ("Sydney", "SYD"), ("Kuala Lumpur", "KUL"), ("Bukit Bintang Grand Hotel", "22 Jalan Bukit Bintang, 55100 Kuala Lumpur"),
               path="registerMain"),
    # the terms page (consents, the representative email, Confirm), then the form: dates are three selects
    "kr": Card("kr", "Please fill in my Korea e-Arrival Card for my trip to Seoul — it's a holiday.",
               "https://www.e-arrivalcard.go.kr/", "KE 122",
               ("Sydney", "SYD"), ("Seoul Incheon", "ICN"), ("Hotel Namsan Gardens", "15 Toegye-ro 20-gil, Jung-gu, Seoul 04637"),
               path="/portal/apply/"),
    # the service moved to www.vjw.digital.go.jp: the sign-in to the user's account comes first (email put in)
    "jp": Card("jp", "Please do my Visit Japan Web registration for my trip to Tokyo — it's a holiday.",
               "https://www.vjw.digital.go.jp/", "NH 880",
               ("Sydney", "SYD"), ("Tokyo Haneda", "HND"), ("Shinjuku Granbell Inn", "2-14-5 Kabukicho, Shinjuku-ku, Tokyo 160-0021"),
               handover="signin"),
    # step 1 is the ID document type and the passport photo upload: the user's
    "cn": Card("cn", "Please fill in my China online arrival card for my trip to Shanghai — it's a holiday.",
               "https://s.nia.gov.cn/ArrivalCardFillingPC/", "MU 562",
               ("Sydney", "SYD"), ("Shanghai Pudong", "PVG"), ("Bund Riverside Hotel", "99 Zhongshan East 1st Road, Huangpu District, Shanghai"),
               handover="photo"),
    # step 1 is the email verification: with a mailbox code reads the code and the traveller's form is filled
    "tw": Card("tw", "Please fill in my Taiwan Arrival Card for my trip to Taipei — it's a holiday.",
               "https://twac.immigration.gov.tw/", "CI 52",
               ("Sydney", "SYD"), ("Taipei Taoyuan", "TPE"), ("Xinyi Park Hotel", "6 Songshou Road, Xinyi District, Taipei 110"),
               handover="email", email_code=True, then_form=True, leave="CI 51"),
    # four steps saved on Next: the page handed over is the declaration step (its Submit). The hotel is picked from the
    # site's own list (searched on its server by name): a real hotel on it (docs/sites/id-arrival-card.md)
    "id": Card("id", "Please fill in my All Indonesia arrival card for my trip to Bali — it's a holiday.",
               "https://allindonesia.imigrasi.go.id/", "GA 715",
               ("Sydney", "SYD"), ("Denpasar Bali", "DPS"), ("Alila Seminyak", "Jl. Taman Ganesha No. 9, Petitenget, Seminyak, Bali 80361"),
               path="/arrival-card-submission/"),
    # every registration starts with the sign-in to the user's eTravel account (Cloudflare check, email, password)
    "ph": Card("ph", "Please do my Philippines eTravel registration for my trip to Manila — it's a holiday.",
               "https://etravel.gov.ph/", "PR 210",
               ("Sydney", "SYD"), ("Manila", "MNL"), ("Makati Palm Suites", "1200 Ayala Avenue, Makati City, Metro Manila"),
               handover="signin"),
    # the Angular stepper's five steps: the page handed over is step 5, Confirm Declaration (its Submit). The travel step
    # requires the hotel's phone ("Hotel contact number"): told in the request (clearly fake)
    "kh": Card("kh", "Please fill in my Cambodia e-Arrival Card for my trip to Phnom Penh — it's a holiday.",
               "https://arrival.gov.kh/", "SQ 166",
               ("Singapore", "SIN"), ("Phnom Penh", "PNH"), ("Riverside Palace Hotel", "51 Sisowath Quay, Phnom Penh"),
               path="/e-arrival/", hotel_phone="+855 23 000 000"),
    # one question per page: the start pages, then the email and its security code; the photos are the user's
    "uk": Card("uk", "Please apply for my UK ETA for my trip to London — it's a holiday. I work as a software engineer.",
               # after the code the passport step is on confirm-your-identity.homeoffice.gov.uk: the Home Office's own
               "https://homeoffice.gov.uk/", "QF 1",
               ("Sydney", "SYD"), ("London Heathrow", "LHR"), ("Covent Garden Court Hotel", "8 Bedford Street, London WC2E 9ED"),
               handover="email", email_code=True, trip=False),
}


def connect_mailbox() -> bool:
    """The test inbox as a generic IMAP account (Connections → Mail), when OMUSE_E2E_IMAP_* are set — the user's own
    mailbox, used read-only (the agent reads the code mail; nothing is sent or deleted). The password is never
    printed. False (and a SKIP line) when the variables are missing or the account does not connect."""
    if not (IMAP["IMAP_HOST"] and IMAP["IMAP_USER"] and IMAP["IMAP_PASSWORD"]):
        print("SKIP the email-code part: no OMUSE_E2E_IMAP_* in the env file (the hand-over is at the email step)")
        return False
    port = int(IMAP["IMAP_PORT"] or 993)
    sport = int(IMAP["SMTP_PORT"] or 465)
    body = {"email": IMAP["IMAP_USER"], "provider": "custom", "app_password": IMAP["IMAP_PASSWORD"],
            "imap_host": IMAP["IMAP_HOST"], "imap_port": port, "imap_security": "ssl" if port == 993 else "starttls",
            "smtp_host": IMAP["SMTP_HOST"] or IMAP["IMAP_HOST"], "smtp_port": sport,
            "smtp_security": "ssl" if sport == 465 else "starttls"}
    r = U.c.post(U.SENTINEL + "/connections/gmail/credential", json=body, headers=U.H)
    ok = r.status_code < 300
    print(f"INFO mailbox {IMAP['IMAP_USER']} connected: {ok} ({r.status_code})")
    if not ok:
        print("SKIP the email-code part: the test inbox did not connect (the hand-over is at the email step)")
    return ok


def world(*, mailbox: bool = False) -> bool:
    """The user's install: profile, the fake passport in the Vault (no site restriction); the test inbox only for the
    cards that verify the email by a code (`mailbox`). Returns whether a mailbox is connected."""
    U.setup(vault=False, profile=PROFILE)
    U.vault_item("Passport Jordan", "id_document", {"number": PASSPORT, "name": "LU JORDAN", "expiry": EXPIRY, "country": "AU"})
    return connect_mailbox() if mailbox else False


def fresh_trip() -> None:
    """Between two cards of a suite (not a check): what the agent remembered about the previous card's trip is
    forgotten — each card is its own user story, not a second trip on the same day."""
    mem = U.c.get(U.B + "/api/memory", headers=U.H).json()
    ids = {f["id"] for v in mem.values() if isinstance(v, list) for f in v
           if isinstance(f, dict) and str(f.get("id", "")).startswith("fact_")}
    for fid in ids:
        U.c.delete(f"{U.B}/api/memory/{fid}", headers=U.H)


def inputs_of_page() -> dict:
    """The inputs of the page handed over, with their ids, names, placeholders and the label above them (the browser
    service's element read) — saved next to the picture for mapping a form; never a check."""
    st = U.c.get(U.BROWSER + "/state", headers=U.BT).json()
    tid = st.get("takeover_task") or st.get("view_task") or ""
    item = {"name": "inputs", "sel": "input, select, textarea, [role=combobox]", "hidden": True, "max": 200,
            "fields": {"id": {"attr": "id"}, "name": {"attr": "name"}, "type": {"attr": "type"},
                       "placeholder": {"attr": "placeholder"}, "fcn": {"attr": "formcontrolname"},
                       "label": {"closest": "div:has(label)", "sel": "label"}}}
    buttons = {"name": "buttons", "sel": "button, input[type=submit], input[type=button], [role=button]", "hidden": True,
               "max": 200, "fields": {"id": {"attr": "id"}, "class": {"attr": "class"}, "type": {"attr": "type"},
                                      "name": {"attr": "name"}, "value": {"attr": "value"}, "text": {},
                                      "title": {"attr": "title"}}}
    try:
        return U.c.post(U.BROWSER + "/agent/read_elements", json={"task_id": tid, "read": [item, buttons]}, headers=U.BT).json()
    except Exception as e:   # noqa: BLE001 — a debugging artifact only
        return {"error": str(e)[:200]}


class Takeover:
    """The user's part when the agent asks them to take over: look at the page handed over (its URL, the values its form
    holds, its text) and hand it back — never pressing anything."""

    def __init__(self, u: U.User, card: Card, label: str):
        self.u, self.card, self.label = u, card, label
        self.requests: list[str] = []
        self.form_url = ""
        self.form: list[dict] = []
        self.final: list[str] = []
        self.step: list[str] = []

    async def __call__(self, pg, request: str) -> None:
        self.requests.append(request)
        url = await self.u.view_url()
        print(f"INFO {self.label} takeover: {request!r} url {url!r}")
        if self.form_url:   # a later hand-over in the same run: the first one is the one checked
            return
        self.form_url = url
        self.form = await self.u.view_form()
        d = declared(self.card.key)
        self.final = present([d["final"]] if d["final"] else [])
        self.step = present(d["steps"])
        print(f"INFO {self.label}: declared final page {d['final']}, user steps {d['steps']}; on the page: final {self.final}, "
              f"step {self.step}")
        if OUT:
            os.makedirs(OUT, exist_ok=True)
            await self.u.view_picture(os.path.join(OUT, f"{U.SUITE}-{self.card.key}-handed-over.png"))
            # not a check: the page's inputs (ids, names, labels) for mapping the form further
            with open(os.path.join(OUT, f"{U.SUITE}-{self.card.key}-inputs.json"), "w") as fh:
                json.dump(inputs_of_page(), fh, indent=1, ensure_ascii=False)


# fields with a default the agent never asks about — checked on a choice card's structured question and options
# (present_choices), never on the model's prose
ASKED_DEFAULTS = re.compile(r"\b(gender|sex|symptom|fever|criminal|convict|refused|deport|declare|customs|other names|passport number)\b", re.I)


# the card's skill in the driver's table (crates/omuse-runtime/src/agent/driver.rs `CARDS`) by the suite's key
SKILLS = {"sg": "sg-arrival-card", "my": "my-arrival-card", "kr": "kr-arrival-card", "id": "id-arrival-card",
          "tw": "tw-arrival-card", "th": "th-arrival-card", "cn": "cn-arrival-card", "jp": "jp-visit-japan-web",
          "kh": "kh-arrival-card", "ph": "ph-etravel", "uk": "uk-eta"}
DRIVER = next((p for p in ("/repo/crates/omuse-runtime/src/agent/driver.rs",
                           os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))),
                                        "crates/omuse-runtime/src/agent/driver.rs")) if os.path.exists(p)), "")
_MARK = re.compile(r'mark(_hidden)?\(\s*"((?:[^"\\]|\\.)*)"(?:,\s*"((?:[^"\\]|\\.)*)")?\s*,?\s*\)')


def declared(key: str) -> dict:
    """The elements the code declares for a card (the same table the machine checks before its hand-over): the final
    page's Submit `{sel, text}` (None when the form lies behind a user's step) and the user steps' markers."""
    src = open(DRIVER).read() if DRIVER else ""
    for block in src.split("    CardSpec {")[1:]:
        if f'skill: "{SKILLS[key]}"' not in block:
            continue
        fin = re.search(r"final_page: (None|Some\((.*?)\)),\n", block, re.S)
        steps = re.search(r"user_step_at: &\[(.*?)\],\n", block, re.S)
        unq = lambda x: (x or "").replace('\\"', '"')
        def marks(text):
            return [{"sel": unq(m.group(2)), "text": unq(m.group(3)), "hidden": bool(m.group(1))} for m in _MARK.finditer(text or "")]
        final = marks(fin.group(2))[0] if fin and fin.group(2) else None
        return {"final": final, "steps": marks(steps.group(1) if steps else "")}
    return {"final": None, "steps": []}


def present(marks: list[dict]) -> list[str]:
    """Which of these declared elements the page handed over shows (by the code's own selectors and texts)."""
    if not marks:
        return []
    st = U.c.get(U.BROWSER + "/state", headers=U.BT).json()
    tid = st.get("takeover_task") or st.get("view_task") or ""
    items = [{"name": f"m{i}", "sel": m["sel"], "hidden": m["hidden"], "max": 5} for i, m in enumerate(marks)]
    try:
        el = U.c.post(U.BROWSER + "/agent/read_elements", json={"task_id": tid, "read": items}, headers=U.BT).json()
    except Exception:   # noqa: BLE001
        return []
    got = el.get("elements") or {}
    out = []
    for i, m in enumerate(marks):
        vals = [str(v).strip() for v in got.get(f"m{i}") or []]
        if vals and (not m["text"] or any(v.lower() == m["text"].lower() for v in vals)):
            out.append(f'{m["sel"]} "{m["text"]}"'.strip())
    return out


async def fill_card(u: U.User, card: Card, *, label: str = "", ask: str = "",
                    follow_up: str = "", mailbox: bool = False) -> list[U.Answer]:
    """One card from the user's side (module doc): ask, answer one question if any, look at the page handed over,
    check — never Submit."""
    label = label or card.key
    to = Takeover(u, card, label)
    a = await u.ask(ask or f"{card.ask} {card.details()}", label=label, on_takeover=to, timeout=1500)
    approvals, answers = list(a.approvals), [a]
    if not to.form_url and not a.timed_out:
        # the first turn ended with no hand-over: the agent asked something first (state: no takeover card, no
        # takeover call). The user answers once, in the same chat.
        cards = await u.choices()
        if cards:   # a choice card: its question and options are structured — none of them a field with a default
            check(f"{label}: the choice card did not ask for anything that has a default (health, declarations, sex, passport)",
                  not ASKED_DEFAULTS.search(U.choice_text(cards)), U.choice_text(cards)[:500])
        a = await u.ask(follow_up or "It's a holiday, travelling alone. Please take everything else from my profile and go ahead and "
                        "fill in the form.", label=f"{label} follow-up", new_chat=False, on_takeover=to, timeout=1500)
        approvals += a.approvals
        answers.append(a)
    check(f"{label}: the answer came (no time-out)", bool(a.text.strip()) and not a.timed_out, a.text[:300])
    check(f"{label}: the Travel bot did it (the link to continue in Travel)", "Travel" in (answers[0].bot_link or a.bot_link),
          [x.bot_link for x in answers])
    check(f"{label}: no approval dialog at all (browser auto-approve, the passport fill included)", not approvals, approvals)
    host = urlsplit(card.form).hostname or ""
    check(f"{label}: the user was asked to take over on the official site ({host})",
          bool(to.requests) and host in to.form_url, (to.requests, to.form_url))
    if to.form_url:
        card_checks(label, card, to, a, mailbox=mailbox)
    shown = await u.thread_text() + " " + " ".join(approvals) + " " + " ".join(to.requests)
    check(f"{label}: the passport number never appears in the chat (messages, cards, requests, the answer)",
          PASSPORT not in shown and PASSPORT not in a.text, [m.start() for m in re.finditer(PASSPORT, shown)][:3])
    return answers


def card_checks(label: str, card: Card, to: Takeover, a: U.Answer, *, mailbox: bool) -> None:
    """The page handed over, by the elements the code declares for the card (driver.rs `CARDS`: the final page's Submit,
    the user steps' markers — the same the machine checks before its hand-over). A form code fills is handed over on its
    final page (a multi-page form shows only its last pieces there; the whole card shows only after Submit, the
    user's); a site that stops earlier is handed over at that step. Nothing is submitted."""
    if card.path:
        check(f"{label}: the page handed over is inside the card's form ({card.path}), not the site's home page",
              card.path in to.form_url, to.form_url)
    if card.handover == "email":
        # (b) handed over at the email step (its declared marker on the page) with the line code appends saying the
        # code could not be read — no mailbox, no mail in time, no code; or (a) the code was read and typed: the page
        # handed over is the declared final page (Taiwan) or past the email step (the UK ETA's identity pages)
        not_read = bool(to.step) and a.shows("could not read the verification code")
        read = bool(to.final) if card.then_form else (not to.step and bool(to.form_url))
        print(f"INFO {label}: email code {'read and typed' if read else 'not read (said so)' if not_read else '?'}")
        check(f"{label}: either the emailed code was read and the run went on to "
              f"{'the declared final page' if card.then_form else 'past the email step'}, or it was handed over at the "
              f"email step saying the code could not be read", read or not_read,
              (to.final, to.step, to.form_url, a.block[:200]))
        if not mailbox:
            check(f"{label}: no mailbox — handed over at the email step, saying so", not_read, (to.step, a.block[:200]))
    elif card.handover == "form":
        check(f"{label}: handed over on the form's final page (the declared Submit / last step is on the page)",
              bool(to.final), (to.final, to.form_url))
    else:
        check(f"{label}: handed over at the user's step ({card.handover}): its declared marker is on the page",
              bool(to.step), (to.step, to.form_url))
    # nothing submitted: the block code appends says the card waits on the user (or stopped early)
    check(f"{label}: nothing submitted — the block code appends says the card waits on the user",
          a.shows("Waiting on your step") or a.shows("Not confirmed") or a.shows("Nothing has been submitted")
          or a.shows("nothing has been submitted") or a.shows("Your step"), a.block[:300])


# cards not run for now, with why (each prints a SKIP line; the card and its form function stay). Indonesia and
# Cambodia, deferred in 0.6.0, run again since 0.6.1 (walked end to end on the live sites: docs/sites/)
DEFERRED: dict[str, str] = {}


def run_cards(keys: list[str], *, mailbox: bool = False) -> None:
    """A suite of cards: one chat per card."""
    # OMUSE_E2E_ONLY=kr (run_in_docker.sh): only these cards, to rerun one with its trace
    only = os.environ.get("OMUSE_E2E_ONLY", "").strip()
    if only:
        keys = [k for k in keys if k in only.split(",")]
    for k in [k for k in keys if k in DEFERRED]:
        print(f"SKIP {k}: {DEFERRED[k]}")
    keys = [k for k in keys if k not in DEFERRED]
    connected = world(mailbox=mailbox)

    async def main():
        async with U.User(viewport=(1440, 1500)) as u:
            for i, key in enumerate(keys):
                if i:
                    fresh_trip()
                await fill_card(u, CARDS[key], mailbox=connected)

    U.run(main)


async def etias(u: U.User) -> None:
    """EU / Schengen: nothing to fill while ETIAS is not live — the agent checks the official status and says so."""
    fresh_trip()
    a = await u.ask("I'm flying to Paris next month. Do I need to fill in an arrival card or ETIAS or anything before the trip?",
                    label="etias", timeout=900)
    t = a.text
    check("etias: the answer came (no time-out)", bool(t.strip()) and not a.timed_out, t[:300])
    check("etias: the Travel bot answered (the link to continue in Travel)", "Travel" in a.bot_link, a.bot_link)
    check("etias: the block code appends after the answer says ETIAS is not in operation yet, nothing to apply for now",
          a.shows("ETIAS is not in operation yet"), (a.block, t[:300]))
    check("etias: the block code appends after the answer points to the official EU site", a.shows("europa.eu"), (a.block, a.block_links))
    check("etias: no form, no takeover and no approval dialog", not a.takeovers and not a.approvals, (a.takeovers, a.approvals))
