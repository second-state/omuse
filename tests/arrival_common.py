"""Arrival cards with a real model, from the user's point of view: the user's install, their part in the browser view and
the checks for tests/arrival_real.py and tests/arrival_cards*_real.py (see tests/user_common.py for the rule: only what
the user sees is checked).

SKELETON — the stand-in pages are gone; each card is to be rebuilt on its REAL official site (the `TODO real URL` line of
each card below), stopping at the first step the user owns (sign-in, emailed code, CAPTCHA, photo, Submit / Pay).
Nothing is ever submitted: the test never presses Submit.

The user: JORDAN LU, an Australian (profile: name as on the passport, birthday, gender Female, nationality, email,
phone, home address in Sydney); the Vault holds the passport (number and expiry, no site restriction; clearly fake test
values). No mailbox: the trip (the flight arriving in two days, no return flight, and the hotel) is in the request.

What the user does and sees for a card (`fill_card`): they ask Main's chat to fill it in (holiday trip). If the answer
asks something first, they answer once. When the agent asks them to take over, they look at the page in the browser
view and hand it back without pressing anything (Visit Japan Web: the hand-over is the sign-in to their own account,
the user's step). Checked (state and facts, never the model's words): the Travel bot did it; no approval dialog at all;
a takeover card came and the page handed over is on the official site; the values the handed-over form holds (name,
sex from the profile, birth date, nationality, passport number and expiry from the Vault, flight, arrival date, last
and next city = where the flight came from, departure = arrival + 7 days, the hotel, every Yes / No question answered
No, the CAPTCHA left empty); the passport number never appears in the chat."""
from __future__ import annotations

import datetime as dt
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
PROFILE = {"name_en": "JORDAN LU", "gender": "Female", "birthday": "1985-04-12", "nationality": "Australia",
           "email_personal": "jordan.lu@example.com", "phone": "+61 400 000 000",
           "address_home": "12 Harbour St, Sydney NSW 2000, Australia"}
OUT = "/repo-out/arrival" if os.path.isdir("/repo-out") else ""


@dataclass
class Card:
    key: str
    ask: str            # what the user types into Main's chat (the trip details follow it, see `details`)
    form: str           # the official site (TODO real URL: confirm against the live site)
    flight: str
    origin: tuple[str, str]   # (city, airport code) the flight comes from
    dest: tuple[str, str]
    hotel: tuple[str, str]    # (name, address)
    login: bool = False       # Visit Japan Web: the hand-over is the sign-in to the user's own account
    trip: bool = True         # the form has the trip and accommodation sections
    departure: bool = True    # the form asks a departure date

    def flight_text(self) -> str:
        o, d = self.origin, self.dest
        return f"I fly {self.flight} from {o[0]} ({o[1]}) to {d[0]} ({d[1]}), arriving on {ARRIVE.isoformat()}, one way."

    def details(self) -> str:
        """The trip as the user tells it (no mailbox): the flight and the hotel."""
        return f"{self.flight_text()} I'm staying at the {self.hotel[0]}, {self.hotel[1]}."


CARDS = {
    # TODO real URL: https://eservices.ica.gov.sg/sgarrivalcard/
    "sg": Card("sg", "Please fill in my SG Arrival Card for my trip to Singapore — it's a holiday.",
               "https://eservices.ica.gov.sg/sgarrivalcard/", "SQ 231",
               ("Sydney", "SYD"), ("Singapore", "SIN"), ("Marina Bayfront Hotel", "10 Bayfront Avenue, Singapore 018956")),
    # TODO real URL: https://tdac.immigration.go.th/arrival-card/
    "th": Card("th", "Please fill in my Thailand Digital Arrival Card (TDAC) for my trip — it's a holiday.",
               "https://tdac.immigration.go.th/", "TG 476",
               ("Sydney", "SYD"), ("Bangkok", "BKK"), ("Siam Riverside Hotel", "88 Charoen Krung Road, Bang Rak, Bangkok 10500")),
    # TODO real URL: https://imigresen-online.imi.gov.my/mdac/main
    "my": Card("my", "Please fill in my Malaysia Digital Arrival Card (MDAC) for my trip — it's a holiday.",
               "https://imigresen-online.imi.gov.my/mdac/main", "MH 122",
               ("Sydney", "SYD"), ("Kuala Lumpur", "KUL"), ("Bukit Bintang Grand Hotel", "22 Jalan Bukit Bintang, 55100 Kuala Lumpur")),
    # TODO real URL: https://www.e-arrivalcard.go.kr/
    "kr": Card("kr", "Please fill in my Korea e-Arrival Card for my trip to Seoul — it's a holiday.",
               "https://www.e-arrivalcard.go.kr/", "KE 122",
               ("Sydney", "SYD"), ("Seoul Incheon", "ICN"), ("Hotel Namsan Gardens", "15 Toegye-ro 20-gil, Jung-gu, Seoul 04637")),
    # TODO real URL: https://vjw-lp.digital.go.jp/
    "jp": Card("jp", "Please do my Visit Japan Web registration for my trip to Tokyo — it's a holiday.",
               "https://vjw-lp.digital.go.jp/", "NH 880",
               ("Sydney", "SYD"), ("Tokyo Haneda", "HND"), ("Shinjuku Granbell Inn", "2-14-5 Kabukicho, Shinjuku-ku, Tokyo 160-0021"),
               login=True),
    # TODO real URL: https://s.nia.gov.cn/ArrivalCardFillingPC/
    "cn": Card("cn", "Please fill in my China online arrival card for my trip to Shanghai — it's a holiday.",
               "https://s.nia.gov.cn/ArrivalCardFillingPC/", "MU 562",
               ("Sydney", "SYD"), ("Shanghai Pudong", "PVG"), ("Bund Riverside Hotel", "99 Zhongshan East 1st Road, Huangpu District, Shanghai"),
               departure=False),
    # TODO real URL: https://twac.immigration.gov.tw/
    "tw": Card("tw", "Please fill in my Taiwan Arrival Card for my trip to Taipei — it's a holiday.",
               "https://twac.immigration.gov.tw/", "CI 52",
               ("Sydney", "SYD"), ("Taipei Taoyuan", "TPE"), ("Xinyi Park Hotel", "6 Songshou Road, Xinyi District, Taipei 110")),
    # TODO real URL: https://allindonesia.imigrasi.go.id/
    "id": Card("id", "Please fill in my All Indonesia arrival card for my trip to Bali — it's a holiday.",
               "https://allindonesia.imigrasi.go.id/", "GA 715",
               ("Sydney", "SYD"), ("Denpasar Bali", "DPS"), ("Seminyak Lagoon Resort", "Jl. Kayu Aya No. 8, Seminyak, Bali 80361")),
    # TODO real URL: https://etravel.gov.ph/
    "ph": Card("ph", "Please do my Philippines eTravel registration for my trip to Manila — it's a holiday.",
               "https://etravel.gov.ph/", "PR 210",
               ("Sydney", "SYD"), ("Manila", "MNL"), ("Makati Palm Suites", "1200 Ayala Avenue, Makati City, Metro Manila")),
    # TODO real URL: https://arrival.gov.kh/
    "kh": Card("kh", "Please fill in my Cambodia e-Arrival Card for my trip to Phnom Penh — it's a holiday.",
               "https://arrival.gov.kh/", "SQ 166",
               ("Singapore", "SIN"), ("Phnom Penh", "PNH"), ("Riverside Palace Hotel", "51 Sisowath Quay, Phnom Penh")),
    # TODO real URL: https://www.gov.uk/eta (the application: https://apply-for-an-eta.homeoffice.gov.uk/)
    "uk": Card("uk", "Please apply for my UK ETA for my trip to London — it's a holiday. I work as a software engineer.",
               "https://www.gov.uk/eta", "QF 1",
               ("Sydney", "SYD"), ("London Heathrow", "LHR"), ("Covent Garden Court Hotel", "8 Bedford Street, London WC2E 9ED"),
               trip=False),
}


def world() -> None:
    """The user's install: profile, the passport in the Vault (no site restriction)."""
    U.setup(vault=False, profile=PROFILE)
    U.vault_item("Passport Jordan", "id_document", {"number": PASSPORT, "name": "LU JORDAN", "expiry": EXPIRY, "country": "AU"})


def fresh_trip() -> None:
    """Between two cards of a suite (not a check): what the agent remembered about the previous card's trip is
    forgotten — each card is its own user story, not a second trip on the same day."""
    mem = U.c.get(U.B + "/api/memory", headers=U.H).json()
    ids = {f["id"] for v in mem.values() if isinstance(v, list) for f in v
           if isinstance(f, dict) and str(f.get("id", "")).startswith("fact_")}
    for fid in ids:
        U.c.delete(f"{U.B}/api/memory/{fid}", headers=U.H)


class Takeover:
    """The user's part when the agent asks them to take over: look at the page handed over (its URL and the values its
    form holds) and hand it back — never pressing Submit."""

    def __init__(self, u: U.User, card: Card, label: str):
        self.u, self.card, self.label = u, card, label
        self.requests: list[str] = []
        self.form_url = ""
        self.form: list[dict] = []

    async def __call__(self, pg, request: str) -> None:
        self.requests.append(request)
        url = await self.u.view_url()
        print(f"INFO {self.label} takeover: {request!r} url {url!r}")
        if self.form_url:   # a later hand-over in the same run: the first one is the one checked
            return
        self.form_url = url
        self.form = await self.u.view_form()
        if OUT:
            await self.u.view_picture(os.path.join(OUT, f"{U.SUITE}-{self.card.key}-handed-over.png"))


# fields with a default the agent never asks about — checked on a choice card's structured question and options
# (present_choices), never on the model's prose
ASKED_DEFAULTS = re.compile(r"\b(gender|sex|symptom|fever|criminal|convict|refused|deport|declare|customs|other names|passport number)\b", re.I)


def norm(s: str) -> str:
    return re.sub(r"[\s-]", "", s or "").upper()


def date_forms(d: dt.date | str) -> set[str]:
    """The ways a form writes a date: ISO, DD/MM/YYYY, MM/DD/YYYY, YYYY/MM/DD, 12 Apr 1985, Apr 12, 1985 …"""
    d = dt.date.fromisoformat(d) if isinstance(d, str) else d
    return {d.isoformat(), f"{d:%d/%m/%Y}", f"{d:%m/%d/%Y}", f"{d:%Y/%m/%d}", f"{d:%d-%m-%Y}", f"{d:%d.%m.%Y}", f"{d:%Y.%m.%d}",
            f"{d.day} {d:%b} {d.year}", f"{d.day} {d:%B} {d.year}", f"{d:%b} {d.day}, {d.year}", f"{d:%B} {d.day}, {d.year}",
            f"{d:%d %b %Y}", f"{d:%d-%b-%Y}"}


def has_date(values: list[str], d: dt.date | str) -> bool:
    forms = {f.upper() for f in date_forms(d)}
    return any(v.strip().upper() in forms for v in values)


def check_values(label: str, card: Card, form: list[dict], *, hotel: str | None = None) -> None:
    """The values the handed-over form holds (what the user reviews before their own Submit). TODO real URL: tune the
    field lookups to the real form's labels once the card runs on its official site."""
    values = [f["value"] for f in form if f["value"]]
    ticked = [f["label"] for f in form if f["checked"]]
    joined = " ".join(values + ticked).upper()
    print(f"INFO {label}: the form holds {len(values)} values; ticked: {ticked[:20]}")
    check(f"{label}: the form was filled (several values at the hand-over)", len(values) >= 5, values[:20])
    check(f"{label}: the name as on the passport (from the profile)", "JORDAN" in joined and re.search(r"\bLU\b", joined) is not None, values[:12])
    check(f"{label}: sex = Female (from the profile)", re.search(r"\bFEMALE\b|\bF\b", joined) is not None, (values[:12], ticked))
    check(f"{label}: the birth date 1985-04-12 (in the field's own format)", has_date(values, PROFILE["birthday"]), values[:20])
    check(f"{label}: the passport number from the Vault", any(norm(v) == PASSPORT for v in values),
          "(right)" if any(norm(v) == PASSPORT for v in values) else "(missing)")
    check(f"{label}: the passport expiry 2031-05-01 from the Vault (in the field's own format)", has_date(values, EXPIRY), values[:20])
    check(f"{label}: nationality = Australia", re.search(r"AUSTRALIA|\bAUS\b|\bAU\b", joined) is not None, values[:20])
    if card.trip:
        check(f"{label}: the flight ({card.flight}) and the arrival date ({ARRIVE.isoformat()}) from the request",
              any(norm(v) == norm(card.flight) for v in values) and has_date(values, ARRIVE), values[:30])
        city, code = card.origin
        check(f"{label}: last / next city = {city}, where the flight came from", city.upper() in joined or re.search(rf"\b{code}\b", joined) is not None,
              values[:30])
        if card.departure:
            check(f"{label}: departure date = arrival + 7 days ({DEPART.isoformat()}), no return flight",
                  has_date(values, DEPART), values[:30])
        h = hotel or card.hotel[0]
        word = max(h.split(), key=len).upper()
        check(f"{label}: the accommodation is {h}", word in joined, values[:30])
    else:
        check(f"{label}: home address from the profile, occupation from the request",
              "HARBOUR" in joined and "ENGINEER" in joined, values[:30])
    yes = [f["label"] for f in form if f["role"] == "radio" and f["checked"] and re.fullmatch(r"\s*yes\s*", f["label"], re.I)]
    check(f"{label}: every Yes / No question answered No without asking (no Yes ticked)", not yes, yes)
    captcha = [f["value"] for f in form if re.search(r"captcha|verification code", f["label"], re.I) and f["value"]]
    check(f"{label}: the CAPTCHA was left empty for the user", not captcha, captcha)


async def fill_card(u: U.User, card: Card, *, label: str = "", ask: str = "", hotel: str | None = None,
                    follow_up: str = "") -> list[U.Answer]:
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
    if not card.login and to.form_url:   # Visit Japan Web stops at the user's own sign-in: no form yet
        check_values(label, card, to.form, hotel=hotel)
    shown = await u.thread_text() + " " + " ".join(approvals) + " " + " ".join(to.requests)
    check(f"{label}: the passport number never appears in the chat (messages, cards, requests, the answer)",
          PASSPORT not in shown and PASSPORT not in a.text, [m.start() for m in re.finditer(PASSPORT, shown)][:3])
    return answers


def run_cards(keys: list[str]) -> None:
    """A suite of cards: one chat per card."""
    # OMUSE_E2E_ONLY=kr (run_in_docker.sh): only these cards, to rerun one with its trace
    only = os.environ.get("OMUSE_E2E_ONLY", "").strip()
    if only:
        keys = [k for k in keys if k in only.split(",")]
    world()

    async def main():
        async with U.User(viewport=(1440, 1500)) as u:
            for i, key in enumerate(keys):
                if i:
                    fresh_trip()
                await fill_card(u, CARDS[key])

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
