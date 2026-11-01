"""Helpers for the real-model suites of everyday use cases (shopping_real, watch_real, support_chat_real,
pdf_forms_real, brief_real, gcal_real, assistant_real), on top of tests/user_common.py. Everything here acts as the user
does or reads what the user sees: the app's pages (Settings, Automations, Connections, Memory), the toasts and dialogs,
the browser view at a takeover, the user's own calendar and the mail the school receives. The world (shop pages, the
user's calendar, the school's mail server) is set up here too; that is never a check."""
from __future__ import annotations

import os
import re
import time

import httpx

from tests import user_common as U

SHOP = "http://shop.test:8099"
PAGES = os.path.join(os.path.dirname(os.path.abspath(__file__)), "pages")
c = httpx.Client(timeout=60, trust_env=False)


# ---------------------------------------------------------------------------------------------------- the world
def write_deal_page(price: str) -> None:
    """Kaya Mobile's page for the Aurora Glitter Case (kaya_aurora.html) at `price` ("25.00"): the price drawn the way
    big shops draw it (Amazon's markup: a screen-reader copy, then symbol, whole and fraction in separate pieces), with
    other products' prices around it."""
    whole, frac = price.split(".")
    bullets = "".join(f"<li>{b}</li>" for b in ("Military-grade drop protection, tested from 3 m", "MagSafe compatible, strong magnets",
                                                 "Translucent matte back that does not yellow", "Raised edges for the screen and the cameras"))
    html = f"""<!doctype html><html lang="en"><head><meta charset="utf-8"><title>Aurora Glitter Case for iPhone 17 Pro Max : Kaya Mobile</title>
<meta name="viewport" content="width=device-width, initial-scale=1">
<style>body{{font-family:Arial,sans-serif;margin:0;color:#111}} header{{background:#0f2a3a;color:#fff;padding:10px 16px}} main{{padding:16px 32px;max-width:900px}}
.a-price{{font-size:26px;font-weight:bold}} .a-price-symbol,.a-price-fraction{{font-size:14px;vertical-align:top}} .rel{{border-top:1px solid #ddd;margin-top:24px;padding-top:8px;color:#444}}</style></head>
<body><header><b>Kaya Mobile</b></header><main>
<p class="rel">Customers also viewed: Aurora Mini Case S$9.90</p>
<h1>Aurora Glitter Case for iPhone 17 Pro Max, MagSafe Compatible</h1>
<a href="/kaya_search.html">Visit the Aurora store</a><p>4.7 out of 5 stars · 1,203 ratings</p>
<p>Price: <span class="a-price"><span class="a-offscreen" style="position:absolute;opacity:0">S${price}</span><span aria-hidden="true"><span class="a-price-symbol">S$</span><span class="a-price-whole">{whole}<span class="a-price-decimal">.</span></span><span class="a-price-fraction">{frac}</span></span></span></p>
<p>FREE delivery in 2–3 working days · In stock</p><ul>{bullets}</ul>
<div class="rel"><h2>Related items</h2><p>Basic Clear Case S$12.90</p><p>Aurora Mini Case S$9.90</p></div></main></body></html>"""
    with open(os.path.join(PAGES, "kaya_aurora.html"), "w") as f:
        f.write(html)


GCAL = "http://127.0.0.1:8094/cal/calendars/primary/events"
GTOKEN = {"Authorization": "Bearer gat-1"}


def calendar_events() -> list[dict]:
    """The events in the user's Google Calendar, as Google lists them to the user (with the user's token)."""
    return c.get(GCAL, headers=GTOKEN).json().get("items", [])


def calendar_add(summary: str, start: str, end: str) -> None:
    """The user already has this event in their Google Calendar (the world: ISO times with the offset)."""
    c.post(GCAL, headers=GTOKEN, json={"summary": summary, "start": {"dateTime": start}, "end": {"dateTime": end}})


class SchoolMail:
    """The mail server the user's mailbox sends through (SMTP on 127.0.0.1:1025): what each recipient receives."""

    def __init__(self) -> None:
        self.received: list[dict] = []

    async def handle_DATA(self, server, session, envelope):   # noqa: N802 (aiosmtpd's name)
        import email
        import email.policy
        msg = email.message_from_bytes(envelope.content, policy=email.policy.default)
        part = msg.get_body(preferencelist=("plain", "html"))
        self.received.append({"to": list(envelope.rcpt_tos), "subject": str(msg["Subject"] or ""),
                              "body": part.get_content() if part else "",
                              "attachments": [(a.get_filename() or "", a.get_payload(decode=True) or b"") for a in msg.iter_attachments()]})
        return "250 OK"

    def start(self) -> None:
        from aiosmtpd.controller import Controller
        from aiosmtpd.smtp import AuthResult

        def auth(server, session, envelope, mechanism, data):
            return AuthResult(success=True)
        self.ctl = Controller(self, hostname="127.0.0.1", port=1025, authenticator=auth, auth_require_tls=False)
        self.ctl.start()


# ---------------------------------------------------------------------------------------------------- the app, as the user uses it
async def watch_toasts(u: U.User) -> None:
    """From now on every toast the app shows is remembered (they disappear after a few seconds); `toasts` reads them."""
    await u.page.evaluate("""() => { window.__toasts = window.__toasts || [];
        if (window.__toastObs) return;
        window.__toastObs = new MutationObserver(ms => ms.forEach(m => m.addedNodes.forEach(n => { if (n.innerText) window.__toasts.push(n.innerText); })));
        window.__toastObs.observe(document.getElementById('toasts'), {childList: true}); }""")


async def toasts(u: U.User) -> list[str]:
    return await u.page.evaluate("() => window.__toasts || []")


async def set_auto(u: U.User, on: bool) -> None:
    """Settings → "Auto-approve actions": the user ticks or unticks the box (it saves at once, with a toast)."""
    pg = u.page
    back = pg.url
    await pg.goto(U.APP + "#settings")
    box = pg.locator("label.toggle", has_text="Auto-approve actions").locator("input[type=checkbox]")
    await box.wait_for(timeout=20000)
    if await box.is_checked() != on:
        await box.click()
        toast = "approved automatically" if on else "need your approval"
        await pg.wait_for_function("t => document.body.innerText.includes(t)", arg=toast, timeout=10000)
    await pg.goto(back if "#chat" in back else U.APP + "#chat")
    await pg.wait_for_selector("#chatInput")
    await pg.wait_for_timeout(1500)


async def set_number(u: U.User, label: str, value: int) -> str:
    """Settings: the user types `value` into the field labelled `label` and presses Save settings; returns the toast."""
    pg = u.page
    back = pg.url
    await pg.goto(U.APP + "#settings")
    inp = pg.locator("label.field", has_text=label).locator("input").first
    await inp.wait_for(timeout=20000)
    await inp.fill(str(value))
    await watch_toasts(u)
    await pg.locator("button.btn.primary", has_text="Save settings").click()
    await pg.wait_for_timeout(2000)
    said = " | ".join(await toasts(u))
    await pg.reload()
    await inp.wait_for(timeout=20000)
    shown = await inp.input_value()
    await pg.goto(back if "#chat" in back else U.APP + "#chat")
    await pg.wait_for_selector("#chatInput")
    await pg.wait_for_timeout(1500)
    print(f"INFO Settings → {label}: saved {value}, shows {shown!r}, toast {said!r}")
    return shown


async def app_page(u: U.User, view: str, wait: str = "main") -> str:
    """What one of the app's own pages shows (`schedules` Automations, `memory`, `connections`, …), as text."""
    pg = u.page
    await pg.goto(U.APP + "#" + view)
    try:
        await pg.wait_for_selector(wait, timeout=15000)
    except Exception:   # noqa: BLE001 — an empty page reads as empty
        pass
    await pg.wait_for_timeout(2500)
    return await pg.locator("#view, main, body").first.inner_text()


def _cards(pg):
    """The Automations page's own cards (a watch, a trigger, a schedule): the innermost cards with a Check now / Run now."""
    return pg.locator(".card.stack:not(:has(.card))", has=pg.locator("button", has_text=re.compile("Check now|Run now")))


async def automation_cards(u: U.User) -> list[str]:
    """Every watch, trigger and schedule card on Automations, as the user reads them."""
    if "#schedules" not in u.page.url:
        await u.page.goto(U.APP + "#schedules")
        await u.page.wait_for_timeout(2500)
    return [" ".join(t.split()) for t in await _cards(u.page).all_inner_texts()]


async def automation_card(u: U.User, words: str) -> str:
    """The Automations card (trigger, watch or schedule) whose text has `words`, as the user reads it ('' if none)."""
    pg = u.page
    await pg.goto(U.APP + "#schedules")
    await pg.wait_for_timeout(2500)
    for t in await automation_cards(u):
        if words.lower() in t.lower():
            return " ".join(t.split())
    return ""


async def press_on_card(u: U.User, words: str, button: str) -> None:
    """The user presses `button` ("Check now", "Run now", "Results") on the Automations card with `words`."""
    pg = u.page
    if "#schedules" not in pg.url:
        await pg.goto(U.APP + "#schedules")
        await pg.wait_for_timeout(2500)
    card = _cards(pg).filter(has_text=words).first
    await card.locator("button", has_text=button).first.click()
    await pg.wait_for_timeout(1500)


async def ask(u: U.User, text: str, *, label: str = "", new_chat: bool = True, on_approval=None, send=None, timeout: float = 900) -> U.Answer:
    """Like `User.ask` for a request whose approval dialogs the user reads closely and may edit: for each dialog
    `on_approval(modal, info)` is awaited with the dialog (a Playwright locator) and what it shows — {"text": its text,
    "values": {label: the value in each editable box}} — and returns True to approve (after any edits it made) or False
    to deny. Without `on_approval` every dialog is approved. `Answer.approvals` gets each dialog's text with its values.
    `send`: how the user sends the message instead of typing `text` (e.g. pressing Choose on a choice card)."""
    pg = u.page
    if new_chat:
        await u.new_chat()
    a = U.Answer()
    before = await pg.locator(".msg.assistant:not(.live)").count()
    t0 = time.monotonic()
    if send:
        await send()
    else:
        await pg.fill("#chatInput", text)
        await pg.keyboard.press("Enter")
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        modal = pg.locator(".modal-back .approval")
        if await modal.count() and not await modal.first.locator("button.btn.approve:disabled").count():
            m = modal.first
            txt = " ".join((await m.inner_text()).split())
            values = await m.evaluate("""m => Object.fromEntries([...m.querySelectorAll('dd input, label.field textarea')].map(e => {
                const lab = e.closest('dd') ? e.closest('dd').previousElementSibling.innerText : e.closest('label').innerText;
                return [lab.trim(), e.value]; }))""")
            a.approvals.append(txt + " " + " ".join(f"[{k}: {v}]" for k, v in values.items()))
            yes = await on_approval(m, {"text": txt, "values": values}) if on_approval else True
            await m.locator("button.btn.approve" if yes else "button.btn.danger").first.click()
            await pg.wait_for_timeout(1000)
            continue
        if await modal.count():
            await pg.wait_for_timeout(500)
            continue
        review = pg.locator(".sys-card button.btn.approve")
        if await review.count():
            # the user presses Review on the approval card in the chat; the thread may be redrawn while a task waits
            # (2026-10-30: a normal click kept finding the button detached for 30 s), so the press is one event
            try:
                await review.first.dispatch_event("click", timeout=5000)
            except Exception:   # noqa: BLE001 — redrawn in between: look again
                pass
            await pg.wait_for_timeout(800)
            continue
        done = await pg.evaluate(
            f"() => document.querySelectorAll('.msg.assistant:not(.live)').length > {before} && !document.querySelector('.msg.assistant.live')")
        if done:
            break
        await pg.wait_for_timeout(700)
    else:
        a.timed_out = True
    a.seconds = time.monotonic() - t0
    last = pg.locator(".msg.assistant:not(.live) .bubble").last
    if await last.count():
        a.text = await last.inner_text()
        a.links = await last.evaluate("b => [...b.querySelectorAll('a[href^=\"http\"]')].map(x => x.href)")
    a.files = await pg.evaluate("() => [...document.querySelectorAll('.bubble.filecard')].map(c => ({name: c.innerText.split('\\n').map(l => l.trim()).find(l => /[A-Za-z0-9]/.test(l)) || '', "
                                "href: (c.querySelector('a[href]') || {}).href || ''}))")
    name = label or text[:30]
    print(f"TIMING {U.SUITE}/{name} {a.seconds:.0f} s")
    print(f"INFO {name} answer: {' '.join(a.text.split())[:900]}")
    print(f"INFO {name} links: {a.links[:8]}  files: {a.files[:4]}  approvals: {[x[:300] for x in a.approvals]}")
    return a


async def press(u: U.User, url_for_layout: str, sel: str, until: str, shot: str) -> str:
    """At a takeover: the user takes over, presses the button `sel` in the live view (where it sits on the page at
    `url_for_layout`) and waits for a page whose address has `until`. Returns the address the view then shows."""
    pg = u.page
    await u.take_over()
    where = await u.layout(url_for_layout, {"b": sel})
    after = await u.view_url()
    if "b" in where:
        x, y, w, h = where["b"]
        await u.click_view(x + w / 2, y + h / 2)
        for _ in range(12):
            after = await u.view_url()
            if until in after:
                break
            await pg.wait_for_timeout(1000)
    if os.path.isdir("/repo-out"):
        await u.view_picture(f"/repo-out/{U.SUITE}-{shot}.png")
    return after


async def type_in_view(u: U.User, x: float, y: float, text: str) -> None:
    """At a takeover: the user clicks the field at (x, y) in the live view and types `text` on their keyboard, over
    whatever is in it (the agent may have filled the email already)."""
    await u.click_view(x, y)
    await u.page.keyboard.press("Control+a")
    await u.page.keyboard.press("Backspace")
    await u.page.keyboard.type(text, delay=40)
    await u.page.wait_for_timeout(1200)


async def connect_google_calendar(u: U.User) -> str:
    """Connections → Google Calendar → "Sign in with Google": the user signs in on Google's page and comes back to
    OMuse. Returns what the page the user lands on says."""
    pg = u.page
    await pg.goto(U.APP + "#connections")
    card = pg.locator(".card", has=pg.locator("h3", has_text="Google Calendar")).first
    await card.wait_for(timeout=20000)
    await card.locator("button", has_text="Sign in with Google").first.click()
    await pg.wait_for_load_state("load")
    await pg.wait_for_timeout(2500)
    landed = await pg.locator("body").inner_text()
    print(f"INFO after Google sign-in: {pg.url} {' '.join(landed.split())[:200]!r}")
    return landed


async def ask_takeover(u: U.User, text: str, *, on_takeover, label: str = "", timeout: float = 900, **kw) -> U.Answer:
    """`User.ask` for a request that will need the user in the browser. When the task is still waiting for the user
    after `ask` returned (the takeover card came a moment after the message), the user opens the card, lets
    `on_takeover` act in the live view, hands back, and waits for the answer. Only the card (and the task's waiting
    state) starts this — never words in the message."""
    pg = u.page
    cards0 = await pg.locator(".sys-card.takeover").count()
    a = await u.ask(text, label=label, on_takeover=on_takeover, timeout=timeout, **kw)
    if a.takeovers or a.timed_out:
        return a
    for _ in range(15):   # the card follows the message while the task waits for the user
        if await pg.locator(".sys-card.takeover").count() > cards0:
            break
        if not await u.takeover_pending():
            return a
        await pg.wait_for_timeout(1000)
    card = pg.locator(".sys-card.takeover").last
    if not await card.count() or await pg.locator(".sys-card.takeover").count() <= cards0:
        return a
    request = " ".join((await card.inner_text()).split())
    link = card.locator("a.btn")
    if not await link.count():
        print(f"INFO the takeover card has no browser link to follow: {request[:200]!r}")
        return a
    before = await pg.locator(".msg.assistant:not(.live)").count()
    back = pg.url
    await link.first.click()
    await pg.wait_for_selector(".bview", timeout=20000)
    await pg.wait_for_timeout(1500)
    await on_takeover(pg, request)
    if await pg.locator("button.release:disabled").count():
        await u.take_over()
    await pg.locator("button.release").click()
    await pg.wait_for_timeout(1000)
    await pg.goto(back)
    await pg.wait_for_selector("#chatInput")
    t0 = time.monotonic()
    end = time.monotonic() + timeout
    while time.monotonic() < end:
        if await pg.evaluate(f"() => document.querySelectorAll('.msg.assistant:not(.live)').length > {before} && !document.querySelector('.msg.assistant.live')"):
            break
        await pg.wait_for_timeout(1000)
    b = U.Answer(takeovers=[request], seconds=a.seconds + time.monotonic() - t0)
    last = pg.locator(".msg.assistant:not(.live) .bubble").last
    b.text = await last.inner_text() if await last.count() else ""
    b.links = await last.evaluate("b => [...b.querySelectorAll('a[href^=\"http\"]')].map(x => x.href)") if await last.count() else []
    print(f"TIMING {U.SUITE}/{label or text[:30]} after the hand-back {b.seconds:.0f} s")
    print(f"INFO {label} answer after the hand-back: {' '.join(b.text.split())[:900]}")
    return b
