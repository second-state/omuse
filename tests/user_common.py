"""Shared driver for the real-model e2e suites, from the user's point of view (`tests/run_in_docker.sh real`).

The rule: a real-model suite does what the user does and checks facts and state — never the model's words to decide or
to verify a step (CLAUDE.md, "Skills: code decides": no natural language from the model anywhere in a decision flow).
A check reads the answer's FACTS (a reference number, a date, an amount, a link, a file) or the WORLD: the web UI (the
approval dialog's tool, a takeover card, the browser view's address, choice cards, file cards), the files the user
downloads, their accounts and the sites they use (the mailbox, Drive, a website's page), and the task's record (its
events: tool calls by name, choices, takeovers; its status). Nothing in a suite reads a sentence the model wrote to
pick the next step or to say a step was taken.

The world is set up first like any user's install (mail in the local IMAP server, the calendar, Drive, the Vault, the
profile, Settings → Models pointed at the real model through tests/model_proxy.py); then a headless browser opens the
app and the suite talks to it through `User`:

    async with User() as u:
        a = await u.ask("What is the cheapest flight from Singapore to Tokyo on December 3?")
        check("…", "SGD" in a.text and a.links, a.text)

`ask` types the message into a new chat (or the current one with `new_chat=False`), presses Enter, and then acts as the
user until the answer is on the page: it approves each approval dialog (or denies it, see `approve=`) and, when the
agent asks the user to take over the browser, opens the browser view, lets the suite look at the page (`on_takeover`)
and hands it back. It returns the answer's text and links, the file cards, the approval dialogs' texts and the hand-over
requests, and the time from Enter to the answer (one TIMING line)."""
from __future__ import annotations

import asyncio
import os
import re
import sys
import time
import traceback
from dataclasses import dataclass, field
from typing import Awaitable, Callable

import httpx

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from playwright.async_api import Page, async_playwright  # noqa: E402

from tests import skills_common as K  # noqa: E402
from tests import travel_common as T  # noqa: E402

B, H, c = T.B, T.H, T.c
APP = B + "/"
check = T.check
SUITE = os.path.splitext(os.path.basename(sys.argv[0]))[0]
UA = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0 Safari/537.36"
PRICE = re.compile(r"(US\$|S\$|HK\$|NT\$|A\$|\$|USD|SGD|JPY|¥|€|£)\s?\d[\d,]*")
# the tools an approval dialog can be about that send something in the user's name (the user who asked for drafts
# denies these; see `tool_of`)
MAIL_SEND_TOOLS = frozenset({"gmail_send", "gmail_reply", "gmail_forward", "slack_send_message"})
# the tool calls that mean the agent asked the user into the browser (a takeover card follows in the chat)
TAKEOVER_CALLS = frozenset({"browser_request_takeover"})
TAKEOVER_EVENTS = frozenset({"takeover", "takeover_requested", "auto_handover"})
# the tool calls that write into a web form (the agent filled something before answering)
FORM_CALLS = frozenset({"browser_fill_form", "browser_type", "browser_fill_secret", "browser_click", "browser_click_at", "browser_upload"})


def tool_of(dialog: str) -> str:
    """The tool an approval dialog is about: `User.ask` puts it first in the dialog's text ("gmail_send 🛡 …")."""
    return (dialog.split() or [""])[0]

if not T.REAL:
    print(f"{SUITE} needs a real model (OMUSE_E2E_REAL=1, see tests/user_common.py)")
    sys.exit(1)


def setup(*, mail: list | None = None, calendar: bool = False, travel_connector: bool = False, vault: bool = True,
          profile: dict | None = None, language: str = "en", auto_approve: bool = True) -> None:
    """The user's world (fixtures, not checks): mail, calendar, an optional travel MCP connector, the Vault's card and
    passport, profile fields, Settings → Models on the real model through the timing proxy, and Settings → Agent's
    "auto-approve browser actions" as a fresh install has it (on: nothing in the browser asks; `auto_approve=False` for
    a user who turned it off)."""
    os.makedirs("/tmp/claude-0", exist_ok=True)
    K.setup(mail=mail, calendar=calendar, language=language)
    c.put(K.S + "/connections/browser", json={"config": {"auto_approve": auto_approve}}, headers=H)
    c.put(B + "/api/settings", json={"language": language, "ui_language": language, "profile_reminder": False}, headers=H)
    T.start_proxy()
    if travel_connector:
        T.connect_travel()
    if vault:
        T.seed_vault()
    for k, v in (profile or {}).items():
        c.put(B + "/api/profile", json={"key": k, "value": v}, headers=H)
    T.use_proxy_model()


@dataclass
class Answer:
    text: str = ""
    links: list[str] = field(default_factory=list)
    files: list[dict] = field(default_factory=list)        # {"name", "href"} of each file card
    approvals: list[str] = field(default_factory=list)     # each approval dialog's text, as the user read it
    takeovers: list[str] = field(default_factory=list)     # each "take over the browser" request
    bot_link: str = ""                                     # "Continue in ✈️ Travel →" when a bot answered
    seconds: float = 0.0
    timed_out: bool = False

    def has_prices(self, n: int = 1) -> bool:
        """At least n prices: amounts with their currency, or — in a table whose header names the currency, e.g.
        "Price for 3 nights (SGD)" — plain amounts under it."""
        if len(PRICE.findall(self.text)) >= n:
            return True
        cur = re.search(r"\((?:US\$|S\$|\$|USD|SGD|JPY|EUR|GBP|HKD)\)", self.text)
        return bool(cur) and len(re.findall(r"(?<![\d.:/-])\d{2,6}(?:\.\d\d)?(?![\d:/-])", self.text[cur.end():])) >= n

    def broken_links(self) -> list[str]:
        """Links in the answer that lead nowhere for the user: an old stand-in address (https://link.omuse.local/…) that
        reached the page (earlier versions handed links to the model as stand-ins; none should ever show)."""
        return [x for x in self.links + re.findall(r"https?://link\.omuse\.local/\S*", self.text) if "link.omuse.local" in x]

    def mentions(self, *words: str) -> bool:
        low = self.text.lower()
        return any(w.lower() in low for w in words)


def opens(url: str) -> tuple[bool, str]:
    """The link opens a page for the user: a 2xx after redirects whose title is not a bot check."""
    try:
        r = httpx.get(url, headers={"User-Agent": UA}, follow_redirects=True, timeout=30)
    except httpx.HTTPError as e:
        return False, str(e)[:120]
    title = (re.search(r"<title[^>]*>([^<]*)", r.text, re.I) or [None, ""])[1].strip()
    bot = any(w in title.lower() for w in ("bot or not", "just a moment", "captcha", "access denied"))
    return r.status_code < 400 and not bot, f"{r.status_code} {title[:80]}"


TakeoverFn = Callable[[Page, str], Awaitable[None]]
ApproveFn = Callable[[str], bool]


class User:
    """One user in a headless browser on the app."""

    def __init__(self, *, viewport=(1280, 900)):
        self.viewport = viewport
        self.page: Page | None = None

    async def __aenter__(self) -> "User":
        self._pw = await async_playwright().start()
        self._br = await self._pw.chromium.launch()
        self.ctx = await self._br.new_context(viewport={"width": self.viewport[0], "height": self.viewport[1]}, locale="en-US",
                                              accept_downloads=True)
        self.page = await self.ctx.new_page()
        self.errors: list[str] = []
        self.page.on("pageerror", lambda e: self.errors.append(str(e)))
        await self.page.goto(APP + "#chat")
        await self.page.wait_for_selector("#chatInput", timeout=30000)
        return self

    async def __aexit__(self, *exc) -> None:
        await self._br.close()
        await self._pw.stop()

    async def new_chat(self) -> None:
        pg = self.page
        await pg.goto(APP + "#chat")
        await pg.wait_for_selector("#chatInput")
        if await pg.locator(".msg.user").count():
            await pg.click(".thread-head .btn.primary")   # New chat
            await pg.wait_for_function("() => !document.querySelector('.msg.user')", timeout=10000)

    async def ask(self, text: str, *, label: str = "", new_chat: bool = True, approve: ApproveFn | bool = True,
                  on_takeover: TakeoverFn | None = None, hand_back: bool = True, timeout: float = 900) -> Answer:
        """Send `text` and act as the user until the answer is on the page (see the module doc)."""
        pg = self.page
        if new_chat:
            await self.new_chat()
        a = Answer()
        before = await pg.locator(".msg.assistant:not(.live)").count()
        handled_takeovers = await pg.locator(".sys-card.takeover").count()
        await pg.fill("#chatInput", text)
        t0 = time.monotonic()
        await pg.keyboard.press("Enter")
        deadline = time.monotonic() + timeout
        waited_for_card = 0
        while time.monotonic() < deadline:
            # an approval dialog: the user reads it and decides
            modal = pg.locator(".modal-back .approval")
            if await modal.count() and await modal.first.locator("button.btn.approve:disabled").count():
                await pg.wait_for_timeout(500)   # the decision is on its way (its buttons are disabled): not a new dialog
                continue
            if await modal.count():
                # the dialog's text starts with the tool it is about (the mono label in its title): a suite decides on
                # the tool's name (`tool_of`), never on words in the body
                mono = modal.first.locator(".ttl .mono")
                tool = (await mono.first.text_content() or "").strip() if await mono.count() else ""
                txt = (tool + " " + " ".join((await modal.first.inner_text()).split())).strip()
                a.approvals.append(txt)
                yes = approve(txt) if callable(approve) else approve
                btn = modal.first.locator("button.btn.approve" if yes else "button.btn.danger")
                if not await btn.count():
                    btn = modal.first.locator("button", has_text="Approve" if yes else "Deny")
                await btn.first.click()
                await pg.wait_for_timeout(800)
                continue
            # an open approval card in the chat without its dialog: open it
            review = pg.locator(".sys-card button.btn.approve")
            if await review.count():
                try:   # the card may be redrawn under the click (a resolved approval's stale button): try again
                    await review.first.click(timeout=3000)
                except Exception:   # noqa: BLE001
                    pass
                await pg.wait_for_timeout(500)
                continue
            # a request to take over the browser: the user looks at the page and hands it back
            cards = pg.locator(".sys-card.takeover")
            if await cards.count() > handled_takeovers and await cards.last.locator("a.btn").count():
                handled_takeovers = await cards.count()
                a.takeovers.append(" ".join((await cards.last.inner_text()).split()))
                if not hand_back:
                    break
                back_to = pg.url
                await cards.last.locator("a.btn").click()
                await pg.wait_for_selector(".bview", timeout=20000)
                await pg.wait_for_timeout(1500)
                if on_takeover:
                    await on_takeover(pg, a.takeovers[-1])
                if await pg.locator("button.release:disabled").count():
                    await pg.locator(".bview button.take").click()   # "Hand back" needs the page taken over first
                    await pg.wait_for_selector(".bview button.release:not([disabled])", timeout=20000)
                await pg.locator("button.release").click()
                await pg.wait_for_timeout(1000)
                await pg.goto(back_to)
                await pg.wait_for_selector("#chatInput")
                continue
            done = await pg.evaluate(
                f"() => document.querySelectorAll('.msg.assistant:not(.live)').length > {before} && !document.querySelector('.msg.assistant.live')")
            if done:
                # a message can come a moment before the takeover card it goes with: that is not the answer yet while
                # the task is waiting for the user in the browser — wait for the card (up to 30 s), then carry on as
                # the user (the task's state decides, never the message's words)
                if await pg.locator(".sys-card.takeover").count() <= handled_takeovers and waited_for_card < 30 \
                        and await self.takeover_pending():
                    waited_for_card += 1
                    await pg.wait_for_timeout(1000)
                    continue
                break
            await pg.wait_for_timeout(1000)
        else:
            a.timed_out = True
        a.seconds = time.monotonic() - t0
        last = pg.locator(".msg.assistant:not(.live) .bubble").last
        if await last.count():
            a.text = await last.inner_text()
            a.links = await last.evaluate("b => [...b.querySelectorAll('a[href^=\"http\"]')].map(x => x.href)")
            a.bot_link = await last.evaluate("b => { const l = b.closest('.msg').querySelector('.handoff-link'); return l ? l.textContent : ''; }")
        a.files = await pg.evaluate("() => [...document.querySelectorAll('.bubble.filecard')].map(c => ({name: c.innerText.split('\\n').map(l => l.trim()).find(l => /[A-Za-z0-9]/.test(l)) || '', "
                                    "href: (c.querySelector('a[href]') || {}).href || ''}))")
        name = label or text[:30]
        print(f"TIMING {SUITE}/{name} {a.seconds:.0f} s")
        print(f"INFO {name} answer: {' '.join(a.text.split())[:900]}")
        print(f"INFO {name} links: {a.links[:8]}  files: {a.files[:4]}  approvals: {[x[:120] for x in a.approvals]}  takeovers: {a.takeovers}")
        return a

    async def download(self, href: str) -> bytes:
        """What the user gets when they click a file card's link."""
        r = await self.ctx.request.get(href)
        return await r.body()

    async def watch(self, text: str, *, label: str = "", new_chat: bool = True, every: float = 0.25,
                    timeout: float = 300) -> tuple[Answer, list[tuple[float, str]]]:
        """Send `text` and watch the reply bubble while the answer is being written: returns the final answer (like
        `ask`, without acting on approvals or takeovers) and what the live bubble showed over time — (seconds after
        Enter, the bubble's text) at every change."""
        pg = self.page
        if new_chat:
            await self.new_chat()
        before = await pg.locator(".msg.assistant:not(.live)").count()
        await pg.fill("#chatInput", text)
        t0 = time.monotonic()
        await pg.keyboard.press("Enter")
        samples: list[tuple[float, str]] = []
        a = Answer()
        while time.monotonic() - t0 < timeout:
            live = await pg.evaluate("() => { const b = document.querySelector('.msg.assistant.live:not(.wait) .bubble'); return b ? b.innerText : null; }")
            if live and (not samples or samples[-1][1] != live):
                samples.append((time.monotonic() - t0, live))
            done = await pg.evaluate(
                f"() => document.querySelectorAll('.msg.assistant:not(.live)').length > {before} && !document.querySelector('.msg.assistant.live')")
            if done:
                break
            await pg.wait_for_timeout(int(every * 1000))
        else:
            a.timed_out = True
        a.seconds = time.monotonic() - t0
        last = pg.locator(".msg.assistant:not(.live) .bubble").last
        if await last.count():
            a.text = await last.inner_text()
            a.links = await last.evaluate("b => [...b.querySelectorAll('a[href^=\"http\"]')].map(x => x.href)")
        name = label or text[:30]
        print(f"TIMING {SUITE}/{name} {a.seconds:.0f} s")
        print(f"INFO {name} answer: {' '.join(a.text.split())[:900]}")
        print(f"INFO {name} live bubble: {len(samples)} different texts, lengths {[len(s) for _, s in samples][:20]}")
        return a, samples

    async def media(self, timeout: float = 30) -> list[dict]:
        """The pictures and the video players the chat's file cards show, as the user sees them once loaded: {"kind":
        "image" | "video", "name", "src", "width", "height", "duration", "error"} — the picture's or the video's size in
        pixels and a video's length in seconds (0 when it did not load in `timeout` seconds); "error" is the player's
        MediaError code (4: this browser cannot decode it — the test image's Chromium has no H.264/AAC)."""
        pg = self.page
        sel = ".bubble.filecard img, .bubble.filecard video"
        for el in await pg.locator(sel).all():
            try:
                await el.scroll_into_view_if_needed(timeout=5000)
            except Exception:   # noqa: BLE001 — what is not shown stays unloaded and reads as 0 × 0
                pass
        try:
            await pg.wait_for_function(f"() => [...document.querySelectorAll('{sel}')].every(e => e.tagName === 'IMG' ? e.complete : e.readyState >= 1 || e.error)",
                                       timeout=timeout * 1000)
        except Exception:   # noqa: BLE001
            pass
        out = await pg.evaluate(f"""() => [...document.querySelectorAll('{sel}')].map(e => {{
            const img = e.tagName === 'IMG', card = e.closest('.filecard'), t = e.closest('[title]');
            return {{kind: img ? 'image' : 'video', name: e.alt || (t && t.title) || (card ? card.innerText.split('\\n')[0] : ''),
                     src: e.currentSrc || e.src, width: img ? e.naturalWidth : e.videoWidth, height: img ? e.naturalHeight : e.videoHeight,
                     duration: img ? 0 : (isFinite(e.duration) ? e.duration : 0), error: (!img && e.error) ? e.error.code : 0}};
        }})""")
        print(f"INFO media in the chat: {out}")
        return out

    async def open_bot(self, name: str) -> None:
        """The user clicks a bot in the menu by its name ("Travel", "Credit cards", …) and starts a new chat with it;
        then `ask(…, new_chat=False)` talks to that bot."""
        pg = self.page
        await pg.goto(APP + "#chat")
        await pg.wait_for_selector("#navlinks a.botlink", timeout=20000)
        await pg.locator("#navlinks a.botlink", has_text=name).first.click()
        await pg.wait_for_function("n => (document.querySelector('#viewTitle') || {}).textContent === n", arg=name, timeout=10000)
        await pg.wait_for_selector("#chatInput")
        if await pg.locator(".msg.user").count():
            await pg.click(".thread-head .btn.primary")   # New chat
            await pg.wait_for_function("() => !document.querySelector('.msg.user')", timeout=10000)

    async def follow_bot_link(self, *, back: bool = True) -> dict:
        """The user clicks the last answer's "Continue in <bot> →" link and reads the bot's conversation. Returns what it
        shows: {"bot": the view's title, "questions": the user's messages, "answers": the bot's answers, "from_main":
        whether it says it was handed over from Main}. With `back`, the user then returns to the chat they came from."""
        pg = self.page
        out = {"bot": "", "questions": [], "answers": [], "from_main": False}
        link = pg.locator(".msg.assistant .handoff-link").last
        if not await link.count():
            return out
        came_from = pg.url
        await link.click()
        try:
            await pg.wait_for_selector(".msg.user .bubble", timeout=20000)
            await pg.wait_for_selector(".msg.assistant:not(.live) .bubble", timeout=60000)
        except Exception:   # noqa: BLE001 — an empty conversation is what the user sees then
            pass
        await pg.wait_for_timeout(800)
        out["bot"] = (await pg.locator("#viewTitle").inner_text()).strip()
        out["questions"] = [" ".join(t.split()) for t in await pg.locator(".msg.user .bubble").all_inner_texts()]
        out["answers"] = await pg.locator(".msg.assistant:not(.live) .bubble").all_inner_texts()
        out["from_main"] = await pg.locator(".from-main").count() > 0
        print(f"INFO bot conversation ({out['bot']}): questions {[q[:80] for q in out['questions']]}, {len(out['answers'])} answers")
        if back:
            await pg.goto(came_from)
            await pg.wait_for_selector("#chatInput")
            await pg.wait_for_timeout(800)
        return out

    # ---------------------------------------------------------------- the browser view (#browser/<task>) as the user sees it
    # In `ask(on_takeover=…)` the page is the browser view: the user reads the URL bar, looks at the live picture of the
    # agent's page, takes over and clicks in it. Coordinates are the page's own (CSS pixels of the agent's 1280 × 800
    # viewport); `layout` tells where a field or a button sits, the way the user's eye finds it in the picture.
    VIEW = (1280, 800)

    async def take_over(self) -> None:
        """The user clicks "Take over" in the browser view: the selected tab (the one asking) is theirs until "Hand back"."""
        pg = self.page
        await pg.wait_for_selector(".bview button.take:not([disabled])", timeout=20000)
        await pg.locator(".bview button.take").click()
        await pg.wait_for_selector(".bview button.release:not([disabled])", timeout=20000)
        await pg.wait_for_timeout(2500)   # the live picture refreshes for the page now under the user's control

    async def view_url(self) -> str:
        """The address in the browser view's URL bar (the selected tab's page). The bar fills a moment after the view
        opens: wait for it (up to 15 s), as the user's eye does."""
        bar = self.page.locator(".bview .bbar input.url")
        for _ in range(30):
            v = await bar.input_value() if await bar.count() else ""
            if v:
                return v
            await self.page.wait_for_timeout(500)
        return ""

    async def view_picture(self, path: str | None = None) -> bytes:
        """The live picture of the page as the browser view shows it (PNG), saved to `path` when given."""
        import base64
        pg = self.page
        await pg.wait_for_function("() => { const i = document.querySelector('.bview img'); return i && i.complete && i.naturalWidth > 0; }",
                                   timeout=20000)
        url = await pg.evaluate("""() => { const i = document.querySelector('.bview img'), c = document.createElement('canvas');
            c.width = i.naturalWidth; c.height = i.naturalHeight; c.getContext('2d').drawImage(i, 0, 0); return c.toDataURL('image/png'); }""")
        png = base64.b64decode(url.split(",", 1)[1])
        if path:
            os.makedirs(os.path.dirname(path) or ".", exist_ok=True)
            open(path, "wb").write(png)
        return png

    async def view_ink(self, boxes: dict[str, tuple[float, float, float, float]], *, inset: float = 4, dark: int = 90) -> dict[str, float]:
        """How much is written inside each box of the live picture: the share of dark pixels (text, not a field's border
        or a grey placeholder) in each (x, y, w, h) box of the page, shrunk by `inset`. A filled-in field reads > 0."""
        await self.view_picture()
        return await self.page.evaluate("""([boxes, inset, dark, vw]) => {
            const i = document.querySelector('.bview img'), c = document.createElement('canvas');
            c.width = i.naturalWidth; c.height = i.naturalHeight; const g = c.getContext('2d'); g.drawImage(i, 0, 0);
            const k = i.naturalWidth / vw, out = {};
            for (const [name, [x, y, w, h]] of Object.entries(boxes)) {
              const X = Math.round((x + inset) * k), Y = Math.round((y + inset) * k);
              const W = Math.max(1, Math.round((w - 2 * inset) * k)), H = Math.max(1, Math.round((h - 2 * inset) * k));
              const d = g.getImageData(X, Y, W, H).data; let n = 0;
              for (let p = 0; p < d.length; p += 4) if (0.299 * d[p] + 0.587 * d[p + 1] + 0.114 * d[p + 2] < dark) n++;
              out[name] = n / (W * H);
            }
            return out; }""", [boxes, inset, dark, self.VIEW[0]])

    async def click_view(self, x: float, y: float) -> None:
        """The user clicks the live picture at the page's point (x, y) (they must have taken over)."""
        img = self.page.locator(".bview img")
        box = await img.bounding_box()
        k = box["width"] / self.VIEW[0]
        # a point low in the picture may lie below the window: scroll it into view first, as the user would
        vy = box["y"] + y * k
        vh = self.page.viewport_size["height"] if self.page.viewport_size else 900
        if vy > vh - 20 or vy < 0:
            await self.page.mouse.wheel(0, vy - vh / 2)
            await self.page.wait_for_timeout(400)
            box = await img.bounding_box()
        await self.page.mouse.click(box["x"] + x * k, box["y"] + y * k)
        await self.page.wait_for_timeout(2500)

    async def layout(self, url: str, selectors: dict[str, str]) -> dict[str, tuple[float, float, float, float]]:
        """Where each element (by CSS selector) sits on the page at `url` shown at the browser view's size, unscrolled:
        {name: (x, y, w, h)}; an element that is not there is left out."""
        ctx = await self._br.new_context(viewport={"width": self.VIEW[0], "height": self.VIEW[1]}, ignore_https_errors=True)
        try:
            p = await ctx.new_page()
            await p.goto(url, wait_until="load", timeout=30000)
            out = {}
            for name, sel in selectors.items():
                b = await p.locator(sel).first.bounding_box() if await p.locator(sel).count() else None
                if b:
                    out[name] = (b["x"], b["y"], b["width"], b["height"])
            return out
        finally:
            await ctx.close()

    async def thread_cards(self) -> list[str]:
        """The cards the open conversation shows between the messages (approvals asked and auto-approved, takeover
        requests, files), each as the user reads it."""
        return [" ".join(t.split()) for t in await self.page.locator(".msg .sys-card").all_inner_texts()]

    async def thread_text(self) -> str:
        """Everything the open conversation shows: the messages and the cards, as text."""
        return " ".join((await self.page.locator(".msg").all_inner_texts()))

    # ---------------------------------------------------------------- the task's record (state, never the model's words)
    async def conv_id(self) -> str:
        """The open conversation's id (the app's own state; "" before the first message)."""
        try:
            return await self.page.evaluate("() => (typeof S !== 'undefined' && S.conv) || ''") or ""
        except Exception:   # noqa: BLE001 — a page in between views
            return ""

    async def tasks(self) -> list[dict]:
        """The tasks run for the open conversation (the bots' child tasks included), newest last."""
        return conv_tasks(await self.conv_id())

    async def events(self) -> list[dict]:
        """Every event of the open conversation's tasks (tool_call / tool_result / choices / handoff / auto_handover …),
        the bots' child tasks included, in order."""
        return [e for t in await self.tasks() for e in t.get("events", [])]

    async def calls(self, *names: str) -> list[dict]:
        """The tool calls of the open conversation's tasks, by name ({"name", "args", "call_id"}); all when no name."""
        return tool_calls(await self.events(), *names)

    async def takeover_happened(self) -> bool:
        """The agent asked the user into the browser in this conversation (a takeover call or an automatic hand-over)."""
        ev = await self.events()
        return bool(tool_calls(ev, *TAKEOVER_CALLS)) or any(e.get("type") in TAKEOVER_EVENTS for e in ev)

    async def takeover_pending(self) -> bool:
        """A task of the open conversation is waiting for the user in the browser right now."""
        return any(t.get("status") == "WAITING_EXTERNAL" or (t.get("waiting") or {}).get("type") in ("takeover", "takeover_requested")
                   for t in await self.tasks())

    async def choices(self) -> list[dict]:
        """The choice cards the open conversation holds (present_choices, as structured data: {"question", "options":
        [{"label", "details", …}], "kind", "verified"}), oldest first."""
        return conv_choices(await self.conv_id())


def conv_tasks(cid: str) -> list[dict]:
    """The tasks of conversation `cid` with their events, the bots' child tasks (handoff events) included, in order."""
    if not cid:
        return []
    try:
        d = c.get(B + f"/api/conversations/{cid}").json()
    except Exception:   # noqa: BLE001
        return []
    ids = [m["task_id"] for m in d.get("messages", []) if m.get("task_id")]
    ids += list((d.get("tasks") or {}).keys())
    out, seen = [], set()
    queue = [i for i in ids if i]
    while queue:
        tid = queue.pop(0)
        if tid in seen:
            continue
        seen.add(tid)
        try:
            t = c.get(B + f"/api/tasks/{tid}").json()
        except Exception:   # noqa: BLE001
            continue
        if not isinstance(t, dict) or not t.get("id"):
            continue
        out.append(t)
        for e in t.get("events", []):
            if e.get("type") == "handoff" and (e.get("data") or {}).get("task_id"):
                queue.append(e["data"]["task_id"])
    out.sort(key=lambda t: str(t.get("created_at") or ""))
    return out


def tool_calls(events: list[dict], *names: str) -> list[dict]:
    """The tool_call events' data ({"name", "args", "call_id"}) with one of `names` (all when none is given)."""
    return [e.get("data") or {} for e in events
            if e.get("type") == "tool_call" and (not names or (e.get("data") or {}).get("name") in names)]


def conv_choices(cid: str) -> list[dict]:
    """The choice and question cards of conversation `cid` as the app stores them (system messages of type "choices"
    or "ask")."""
    import json
    if not cid:
        return []
    try:
        msgs = c.get(B + f"/api/conversations/{cid}").json().get("messages", [])
    except Exception:   # noqa: BLE001
        return []
    out = []
    for m in msgs:
        if m.get("role") != "system":
            continue
        try:
            j = json.loads(m.get("content") or "")
        except ValueError:
            continue
        # a choice card, or a question card (`ask_user`: the agent asked for named fields)
        if isinstance(j, dict) and j.get("type") in ("choices", "ask"):
            out.append(j)
    return out


def choice_text(cards: list[dict]) -> str:
    """The questions, labels and details of choice cards, flattened (to look a value up, never to decide a step)."""
    parts = []
    for card in cards:
        parts.append(str(card.get("question") or ""))
        for o in card.get("options") or []:
            parts.append(str(o.get("label") or ""))
            parts.extend(str(d) for d in (o.get("details") or []))
    return " ".join(parts)


def mailbox(folder: str = "INBOX") -> list[dict]:
    """The messages in a folder of the user's mailbox (the local IMAP account the test mail lives in: "INBOX", "Drafts",
    "Sent", …) as their mail app shows them: {"from", "to", "subject", "body", "in_reply_to"}. A folder that does not
    exist is empty."""
    import email
    import email.policy
    import imaplib
    m = imaplib.IMAP4("127.0.0.1", 1143)
    m.login(K.USER, K.PW)
    out = []
    try:
        if m.select(f'"{folder}"', readonly=True)[0] == "OK":
            _, ids = m.search(None, "ALL")
            for i in (ids[0] or b"").split():
                _, d = m.fetch(i, "(RFC822)")
                msg = email.message_from_bytes(d[0][1], policy=email.policy.default)
                part = msg.get_body(preferencelist=("plain", "html"))
                body = part.get_content() if part else ""
                out.append({"from": str(msg["From"] or ""), "to": str(msg["To"] or ""), "subject": str(msg["Subject"] or ""),
                            "body": body, "in_reply_to": str(msg["In-Reply-To"] or "")})
    finally:
        m.logout()
    return out


_DRIVE, _GTOKEN = "http://127.0.0.1:8094/drive", {"Authorization": "Bearer gat-1"}


def drive_folder(name: str) -> list[dict]:
    """The files in the user's Google Drive folder `name`, as Drive lists them (the fake Drive of tests/fake_apps.py
    stands in for the user's Drive): {"id", "name", "mimeType", "size", "webViewLink"}; [] when there is no such folder."""
    q = f"name contains '{name}' and mimeType = 'application/vnd.google-apps.folder' and trashed = false"
    folders = c.get(_DRIVE + "/files", params={"q": q}, headers=_GTOKEN).json().get("files", [])
    if not folders:
        return []
    files = c.get(_DRIVE + "/files", params={"q": f"'{folders[0]['id']}' in parents and trashed = false"}, headers=_GTOKEN).json()
    return [{k: f.get(k) for k in ("id", "name", "mimeType", "size", "webViewLink")} for f in files.get("files", [])]


def drive_file(file_id: str) -> bytes:
    """A file's content as the user downloads it from their Drive."""
    return c.get(f"{_DRIVE}/files/{file_id}", params={"alt": "media"}, headers=_GTOKEN).content


def run(main: Callable[[], Awaitable[None]]) -> None:
    """Run the suite's `main`, count a crash as a failure (never a silent pass), switch the model back and finish."""
    try:
        asyncio.run(main())
    except Exception as e:   # noqa: BLE001
        traceback.print_exc()
        check("the run finished without an error", False, repr(e)[:500])
    finally:
        if T.fails:
            _trace()
        T.finish()


def _trace() -> None:
    """After a failure, what the agent did (TRACE lines for whoever fixes the product; never a check)."""
    try:
        for t in reversed(c.get(B + "/api/tasks", params={"limit": 12}).json().get("tasks", [])):
            ev = c.get(f"{B}/api/tasks/{t['id']}").json().get("events", [])
            for e in ev:
                if e["type"] in ("tool_call", "tool_result"):
                    d = e.get("data") or {}
                    print(f"TRACE {t['id'][-6:]} {e['type']} {d.get('name')} {str(d.get('args') or d.get('preview') or '')[:300]}")
                elif e["type"] in ("answer_traits", "arrival_check", "auto_handover", "choices_rejected", "takeover",
                                   "takeover_done", "error", "final"):
                    print(f"TRACE {t['id'][-6:]} {e['type']} {str(e.get('data') or '')[:300]}")
    except Exception as e:   # noqa: BLE001
        print(f"TRACE unavailable: {e!r}")
