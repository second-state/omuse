"""The hand-over on a real booking or shop page, as the user sees it (travel_book_real; on top of
tests/user_common.py).

A booking or an order is never finished in an e2e run: OMuse searches, code picks, code opens the REAL booking page (an
OTA's or an airline's page from the offer's booking link, a hotel seller's page, a shop's product page) and hands it to
the browser-use skill, which fills it with the profile's clearly fake data and asks the user to take over at the first
step that is theirs (sign-in, CAPTCHA, Pay / Place order). The suite takes over like the user, looks at the page and
hands it back WITHOUT pressing anything: nothing is paid, booked or ordered.

    seen = Handover()
    a = await u.ask("Book …", on_takeover=seen.look(u))
    check_handover(a, seen, "flight", sites=("kiwi.com",))

No Vault card is set up for these suites (U.setup(vault=False)): no card number, not even a test one, can reach a real
payment form."""
from __future__ import annotations

import os
import re
from dataclasses import dataclass, field
from urllib.parse import urlparse

from tests import user_common as U
from tests.user_common import check

# the user's profile: clearly fake test data (example.com mail, a 555 number, a test street)
FAKE = {"name_en": "OMUSE TESTER", "email_personal": "omuse.e2e.tester@example.com", "phone": "+1 415 555 0123",
        "birthday": "1990-01-15", "gender": "Male", "nationality": "Singapore",
        "address_home": "1 Test Street, Apt 2, San Francisco, CA 94105, USA"}
# what a page may hold after browser-use filled it: the profile's values (and parts of them)
FAKE_VALUES = {v.lower() for v in FAKE.values()} | {"omuse", "tester", "test street", "94105", "san francisco"}
CARD_LIKE = re.compile(r"(?<!\d)(?:\d[ -]?){13,19}(?!\d)")
SEARCH_HOSTS = ("google.", "bing.com", "duckduckgo.com", "search.brave.com")


def host(url: str) -> str:
    return (urlparse(url or "").hostname or "").lower()


def site(url: str) -> str:
    """The registrable part of a host, roughly: www.kiwi.com → kiwi.com, checkout.shopify.com → shopify.com."""
    parts = host(url).split(".")
    if len(parts) >= 3 and parts[-2] in ("co", "com", "net", "org") and len(parts[-1]) == 2:
        return ".".join(parts[-3:])
    return ".".join(parts[-2:])


@dataclass
class Handover:
    """What the user saw in the browser view at each hand-over: the address, the form fields, the page's words."""
    urls: list[str] = field(default_factory=list)
    forms: list[list[dict]] = field(default_factory=list)
    texts: list[str] = field(default_factory=list)

    def look(self, u: U.User, name: str = ""):
        async def on_takeover(_pg, _card: str) -> None:
            url = await u.view_url()
            self.urls.append(url)
            try:
                self.forms.append(await u.view_form())
            except Exception as e:   # noqa: BLE001 — a page the browser service cannot read shows no form
                print(f"INFO no form at the hand-over: {e!r}"[:200])
                self.forms.append([])
            self.texts.append(_page_text())
            print(f"INFO hand-over {len(self.urls)}: {url}")
            out = os.environ.get("OMUSE_E2E_OUT") and "/repo-out"
            if out and os.path.isdir(out):
                try:
                    await u.view_picture(f"{out}/shots/{U.SUITE}-{name or 'handover'}-{len(self.urls)}.png")
                except Exception:   # noqa: BLE001 — the picture is for reading the run, not a check
                    pass
        return on_takeover

    @property
    def last_url(self) -> str:
        return self.urls[-1] if self.urls else ""

    def filled(self) -> list[dict]:
        """The fields that hold a value (or are ticked) on the pages handed over."""
        return [f for form in self.forms for f in form if f["value"] or f["checked"]]


def _page_text() -> str:
    """The words of the page handed over (the browser service's snapshot), for INFO lines and the site's own state."""
    try:
        st = U.c.get(U.BROWSER + "/state", headers=U.BT).json()
        tid = st.get("takeover_task") or st.get("view_task") or ""
        snap = U.c.post(U.BROWSER + "/agent/snapshot", json={"task_id": tid, "max_chars": 20000}, headers=U.BT).json()
        return str(snap.get("snapshot") or "")
    except Exception:   # noqa: BLE001
        return ""


def check_handover(a: U.Answer, seen: Handover, label: str, *, sites: tuple[str, ...] = (), amount: bool = True) -> None:
    """The checks every booking / order hand-over gets, from what the user sees. `amount`: the block must carry the
    amount (a booking always: the search's price or the approved fare; an order only once a purchase was approved)."""
    check(f"{label}: the agent asked the user to take over the browser on the booking page (the first step that is theirs)",
          bool(a.takeovers) and bool(seen.urls), (a.takeovers, a.text[:300]))
    url = seen.last_url
    real = url.startswith("https://") and not any(h in host(url) for h in SEARCH_HOSTS + ("127.0.0.1", "localhost"))
    if sites:
        real = real and any(site(url) == s or host(url).endswith("." + s) for s in sites)
    check(f"{label}: the page handed over is the real booking site ({' / '.join(sites) or 'a booking site'}), not the app or a search page",
          real, url)
    # browser-use filled only the page: the profile's fake data, never a card number or another secret
    filled = seen.filled()
    print(f"INFO {label} filled at the hand-over: {[(f['label'][:30], f['value'][:40]) for f in filled][:40]}")
    secrets = [f for f in filled if f["value"] == "[hidden]" or CARD_LIKE.search(f["value"].replace("+", ""))
               and "phone" not in f["label"].lower() and "tel" not in f["label"].lower()]
    check(f"{label}: no card number or other secret was put into the page (there is no card in the Vault; payment is the user's)",
          not secrets, secrets)
    typed = [f for f in filled if f["role"] in ("textbox", "textarea", "searchbox") and f["value"]]
    ours = [f for f in typed if any(v in f["value"].lower() or f["value"].lower() in v for v in FAKE_VALUES if len(v) > 3)]
    print(f"INFO {label}: {len(typed)} typed field(s), {len(ours)} with the profile's fake data")
    # the facts code appended under the reply: the booking page and the amount
    links = [x for x in a.block_links if x.startswith("http") and host(x) != "127.0.0.1"]
    check(f"{label}: the block code appends after the answer links the booking page", bool(links), (a.block, a.block_links))
    if amount:
        check(f"{label}: the block code appends after the answer has the amount with its currency",
              bool(U.PRICE.search(a.block)) or bool(re.search(r"\b[A-Z]{3} \d[\d,]*(?:\.\d\d)?\b", a.block)), a.block)
    # nothing was paid or booked: the user pressed nothing, so the block carries no confirmation reference and the page
    # handed over is not a confirmation page
    check(f"{label}: nothing was booked or paid — no confirmation reference in the block, the page handed over is not a confirmation",
          # a reference code read off a page is "<label>: <value>"; code's own "⚠️ Not confirmed: no order number or
          # confirmation was found …" line says the opposite and does not count
          not re.search(r"(confirmation (number|code)|booking reference|order number)\s*:\s*\S", a.block, re.I)
          and not re.search(r"/(confirmation|thank[-_]?you|thanks|order-received)\b", url, re.I), (a.block, url))
