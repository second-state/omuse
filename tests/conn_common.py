"""Helpers for the real-model suites of the connectors and the everyday chat features (apps_real, automations_real,
telegram_real, mcp_real, phone_real, memory_real, chat_real), on top of tests/user_common.py and tests/shop_common.py.

Two kinds of helpers, kept apart:
* the world (fixtures, never checks): the user's accounts connected the way an install has them (Notion, Slack, the
  Telegram bot, the phone lines), colleagues posting in Slack or editing Notion, hotel sites that answer slowly;
* what the user sees outside OMuse, read the way the user's own apps show it: their Telegram chat with the bot (the
  fake Telegram API's log of what the bot sent to the user's chat, and the user tapping a button or typing a message),
  the Slack channel, the Notion page and database, the notes in their notes app (the MCP server's own search), the
  restaurant's phone, and OMuse's own pages (Memory, Automations, Connections, the chat list).
Nothing here reads OMuse's insides (no tool calls, events, audit or internal APIs)."""
from __future__ import annotations

import json
import re
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import urlparse

import httpx

from tests import user_common as U

B, H = U.B, U.H
S = B + "/sentinel/api"
c = httpx.Client(timeout=60, trust_env=False)


def wait_for(pred, timeout: float = 60, every: float = 1.5):
    """Poll `pred()` until it returns something truthy (or the time is up); returns the last value."""
    end = time.monotonic() + timeout
    v = pred()
    while not v and time.monotonic() < end:
        time.sleep(every)
        v = pred()
    return v


# ================================================================================================ the chat
async def ask(u: U.User, text: str, *, label: str = "", new_chat: bool = True, approve=True, on_takeover=None,
              timeout: float = 900) -> U.Answer:
    """`User.ask` (tests/user_common.py) for requests the user may deny: the same steps as the user, but a "Review"
    button that disappears while the user reaches for it (the card re-renders as "resolved" once the decision is in)
    is simply gone, not a hang. Takeovers are handed to `User.ask`'s own handling only through `on_takeover`-free
    requests; use `User.ask` for takeovers."""
    pg = u.page
    if new_chat:
        await u.new_chat()
    a = U.Answer()
    before = await pg.locator(".msg.assistant:not(.live)").count()
    await pg.fill("#chatInput", text)
    t0 = time.monotonic()
    await pg.keyboard.press("Enter")
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        modal = pg.locator(".modal-back .approval")
        if await modal.count() and await modal.first.locator("button.btn.approve:disabled").count():
            await pg.wait_for_timeout(500)
            continue
        if await modal.count():
            try:
                txt = " ".join((await modal.first.inner_text(timeout=3000)).split())
                yes = approve(txt) if callable(approve) else approve
                await modal.first.locator("button.btn.approve" if yes else "button.btn.danger").first.click(timeout=5000)
                a.approvals.append(txt)
            except Exception:   # noqa: BLE001 — the dialog closed meanwhile
                pass
            await pg.wait_for_timeout(800)
            continue
        review = pg.locator(".sys-card button.btn.approve")
        if await review.count():
            try:
                await review.first.click(timeout=3000)
            except Exception:   # noqa: BLE001 — the card turned into "resolved" meanwhile
                pass
            await pg.wait_for_timeout(500)
            continue
        cards = pg.locator(".sys-card.takeover a.btn")
        if await cards.count():
            a.takeovers.append(" ".join((await pg.locator(".sys-card.takeover").last.inner_text()).split()))
            break
        done = await pg.evaluate(
            f"() => document.querySelectorAll('.msg.assistant:not(.live)').length > {before} && !document.querySelector('.msg.assistant.live')")
        if done:
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
    print(f"TIMING {U.SUITE}/{name} {a.seconds:.0f} s")
    print(f"INFO {name} answer: {' '.join(a.text.split())[:900]}")
    print(f"INFO {name} links: {a.links[:8]}  files: {a.files[:4]}  approvals: {[x[:300] for x in a.approvals]}  takeovers: {a.takeovers}")
    return a


# ================================================================================================ Notion & Slack
NOTION, NOTION_TOKEN = "http://127.0.0.1:8094", "ntn_testtoken1234567890abcdef"
Q4_PAGE = "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee"
TASKS_DB = "11111111-2222-4333-8444-555555555555"
SLACK_BOT, SLACK_USER = "xoxb-test-token-123456", "xoxp-test-token-123456"
_NH = {"Authorization": "Bearer " + NOTION_TOKEN, "Notion-Version": "2022-06-28"}


def connect_notion_and_slack() -> None:
    """The world: the user's Notion workspace ("Lucas's Notion": the Q4 Plan page and the Tasks database) and Slack
    (team Acme, the bot in #general but not in #sales) are connected with their tokens, as on the Connections page."""
    c.post(NOTION + "/_reset")
    r = c.post(S + "/connections/notion/credential", headers=H, json={"token": NOTION_TOKEN})
    U.check("world: Notion connected", r.status_code == 200, r.text[:300])
    r = c.post(S + "/connections/slack/credential", headers=H, json={"token": SLACK_BOT})
    U.check("world: Slack connected", r.status_code == 200, r.text[:300])


def _title(p: dict) -> str:
    for v in p.get("properties", {}).values():
        if v.get("type") == "title":
            return "".join(x.get("plain_text", "") for x in v["title"])
    return ""


def notion_tasks() -> list[dict]:
    """The rows of the user's Notion Tasks database as Notion shows them: {"name", "status", "due", "archived"}."""
    r = c.post(f"{NOTION}/v1/databases/{TASKS_DB}/query", headers=_NH, json={}).json()
    out = []
    for p in r.get("results", []):
        pr = p["properties"]
        st = (pr.get("Status") or {}).get("status") or (pr.get("Status") or {}).get("select") or {}
        due = (pr.get("Due") or {}).get("date") or {}
        out.append({"name": _title(p), "status": st.get("name", ""), "due": due.get("start", ""), "archived": p.get("archived", False)})
    return out


def notion_page(pid: str = Q4_PAGE) -> dict:
    """A Notion page as the user sees it: {"title", "archived", "text"} (its blocks' text)."""
    p = c.get(f"{NOTION}/v1/pages/{pid}", headers=_NH).json()
    blocks = c.get(f"{NOTION}/v1/blocks/{pid}/children", headers=_NH).json().get("results", [])
    text = "\n".join("".join(x.get("plain_text", "") for x in b.get(b["type"], {}).get("rich_text", [])) for b in blocks)
    return {"title": _title(p), "archived": p.get("archived", False), "text": text}


def notion_colleague_sets_status(row: str, status: str) -> None:
    """The world: a colleague changes a task's status in Notion."""
    ids = {"Write launch post": "cccccccc-0000-4000-8000-000000000000", "Fix login bug": "cccccccc-0000-4000-8000-000000000001"}
    c.post(NOTION + "/_notion/edit", json={"id": ids[row], "status": status})


def slack_channel(channel: str = "C1") -> list[dict]:
    """A Slack channel as the user's Slack shows it, oldest first: {"who", "text", "bot"} ("C1" is #general)."""
    r = c.post(NOTION + "/api/conversations.history", headers={"Authorization": "Bearer " + SLACK_USER}, json={"channel": channel})
    msgs = list(reversed(r.json().get("messages", [])))
    return [{"who": "OMuse" if m.get("bot_id") else {"U2": "John"}.get(m.get("user"), m.get("user")), "text": m.get("text", ""),
             "bot": bool(m.get("bot_id"))} for m in msgs]


def slack_colleague_posts(text: str, channel: str = "C1") -> None:
    """The world: John posts in a channel."""
    c.post(NOTION + "/_slack/post", json={"channel": channel, "text": text, "user": "U2"})


# ================================================================================================ Telegram
TG = "http://127.0.0.1:8091"
CHAT = 555


def tg_log() -> dict:
    return c.get(TG + "/_log").json()


def connect_telegram() -> None:
    """The world: the user created a bot with BotFather, sent it /start, and saved its token and their chat on the
    Connections page (the chat found from that /start)."""
    tg_say("/start")
    d = c.post(S + "/connections/telegram/detect", json={"bot_token": "123:ABC"}, headers=H).json()
    chat = str((d.get("chats") or [{}])[0].get("chat_id") or CHAT)
    r = c.post(S + "/connections/telegram/credential", json={"bot_token": "123:ABC", "chat_id": chat}, headers=H)
    U.check("world: Telegram connected", r.status_code == 200, r.text[:300])


def tg_say(text: str, chat: int = CHAT, name: str = "Lucas") -> None:
    """The user (or someone else, by chat id) writes to the bot in Telegram."""
    c.post(TG + "/_push", json={"message": {"message_id": int(time.time() * 1000) % 10**9, "chat": {"id": chat, "type": "private",
                                                                                                  "first_name": name}, "text": text}})


def _plain(text: str) -> str:
    import html
    return html.unescape(re.sub(r"<[^>]+>", "", text or ""))


def tg_chat(chat: int = CHAT) -> list[dict]:
    """The user's Telegram chat with the bot: every message the bot sent there, in order, as Telegram shows it —
    {"id", "text" (plain), "buttons": [{"text", "data" | "url"}]} — with later edits applied."""
    log = tg_log()
    edits = {}
    for e in log["edits"]:
        edits[e.get("message_id")] = e
    out = []
    for m in log["sent"]:
        if str(m.get("chat_id")) != str(chat):
            continue
        e = edits.get(m["message_id"], {})
        kb = (e.get("reply_markup") if "reply_markup" in e else m.get("reply_markup")) or {}
        if isinstance(kb, str):
            try:
                kb = json.loads(kb)
            except ValueError:
                kb = {}
        buttons = [{"text": b.get("text", ""), "data": b.get("callback_data", ""), "url": b.get("url", "")}
                   for row in kb.get("inline_keyboard", []) for b in row]
        out.append({"id": m["message_id"], "text": _plain(e.get("text") or m.get("text", "")), "buttons": buttons})
    return out


def tg_files(chat: int = CHAT) -> list[dict]:
    """The files the bot sent to the user's chat: {"name", "caption", "size", "head" (the first bytes, as text)}."""
    return [d for d in tg_log()["docs"] if str(d.get("chat_id")) == str(chat)]


def tg_popups() -> list[str]:
    """The little notices Telegram pops up after the user taps a button (answerCallbackQuery texts)."""
    return [a.get("text", "") for a in tg_log()["answers"]]


def tg_tap(msg: dict, button: str) -> None:
    """The user taps the inline button whose label contains `button` under a message of the bot."""
    b = next(x for x in msg["buttons"] if button.lower() in x["text"].lower())
    c.post(TG + "/_push", json={"callback_query": {"id": f"cb{time.time():.6f}", "data": b["data"],
                                                   "message": {"message_id": msg["id"], "chat": {"id": CHAT}, "text": msg["text"]}}})


def tg_wait(pred, after: int = 0, timeout: float = 600) -> list[dict]:
    """Wait until the user's chat (the messages after the first `after`) satisfies `pred(messages)`; returns them."""
    return wait_for(lambda: (lambda ms: ms if pred(ms) else None)(tg_chat()[after:]), timeout=timeout, every=2) or tg_chat()[after:]


def tg_answer_after(after: int, timeout: float = 600) -> str:
    """The bot's answer to a request: the text after the "✅ Done" (or Failed / Cancelled) message that follows the
    first `after` messages ('' when none came)."""
    heads = ("Done", "Failed", "Cancelled")

    def done(ms):
        i = next((k for k, m in enumerate(ms) if any(h in m["text"] for h in heads) and len(m["text"]) < 40), None)
        return i is not None and len(ms) > i + 1
    ms = tg_wait(done, after, timeout)
    i = next((k for k, m in enumerate(ms) if any(h in m["text"] for h in heads) and len(m["text"]) < 40), None)
    if i is None:
        return ""
    time.sleep(2)   # a long answer comes in several messages
    ms = tg_chat()[after:]
    return "\n".join(m["text"] for m in ms[i + 1:] if not m["buttons"] or not any(b["data"] for b in m["buttons"]))


# ================================================================================================ MCP
MCP = "http://127.0.0.1:8093"
MCP_TOKEN = "test-mcp-token"


def notes(query: str = "") -> list[dict]:
    """The notes in the user's notes app, found by `query` (its own search, the way the user looks a note up)."""
    h = {"Authorization": "Bearer " + MCP_TOKEN, "Accept": "application/json, text/event-stream"}
    r = c.post(MCP + "/mcp", headers=h, json={"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
        "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "notes-app", "version": "1"}}})
    h["Mcp-Session-Id"] = r.headers.get("mcp-session-id", "s")
    r = c.post(MCP + "/mcp", headers=h, json={"jsonrpc": "2.0", "id": 2, "method": "tools/call",
                                                "params": {"name": "notes_search", "arguments": {"query": query}}})
    data = next((ln[5:] for ln in r.text.splitlines() if ln.startswith("data:") and '"result"' in ln), "")
    try:
        return json.loads(data)["result"]["structuredContent"]["results"]
    except (ValueError, KeyError, TypeError):
        return []


# ================================================================================================ phone
VOICE, DIAL = "http://127.0.0.1:8095", "http://127.0.0.1:8096"


def connect_telnyx(daily_limit: int = 20) -> None:
    """The world: the user's Telnyx line (their number +1 979 347 1777, OpenAI Realtime voice) is set up on Connections."""
    c.post(VOICE + "/reset")
    cfg = {"owner_name": "Lucas Lu", "from_number": "+1 979 347 1777", "connection_id": "conn-1", "public_url": "http://127.0.0.1:8083",
           "telnyx_api_key": "KEYtest", "openai_api_key": "sk-test", "allowed_prefixes": "+65, +1", "max_minutes": 5,
           "daily_limit": daily_limit}
    r = c.post(S + "/connections/phone/credential", json=cfg, headers=H).json()
    U.check("world: the Telnyx line is set up", r.get("ready"), r)


def connect_dialmcp() -> None:
    """The world: the user signed in to DialMCP from Connections → Phone (their verified number +1 669 222 9512)."""
    c.post(DIAL + "/_reset")
    cb = B + "/sentinel/api/oauth/callback"
    au = c.post(S + "/connections/phone/dialmcp/start", json={"redirect_uri": cb}, headers=H).json().get("auth_url", "")
    page = c.get(au, follow_redirects=True)
    U.check("world: DialMCP signed in", page.status_code == 200 and "DialMCP" in page.text, page.text[:200])
    c.put(S + "/connections/phone", json={"config": {"provider": "auto"}}, headers=H)


def restaurant_rang() -> list[str]:
    """The numbers the Telnyx line's calls rang (what the people called would notice: their phone rang)."""
    return [d.get("to", "") for d in c.get(VOICE + "/state").json()["dials"]]


def dialmcp_rang() -> list[str]:
    """The numbers DialMCP's calls rang."""
    return [p.get("to", "") for p in c.get(DIAL + "/_state").json()["placed"]]


# ================================================================================================ OMuse's own pages
async def chat_titles(u: U.User, bot_home: str = "#chat") -> list[str]:
    """The titles in the chat list (newest first), as the user reads them."""
    pg = u.page
    await pg.goto(U.APP + bot_home)
    await pg.wait_for_selector(".convlist", timeout=20000)
    await pg.wait_for_timeout(1500)
    return [" ".join(t.split()) for t in await pg.locator(".convlist .conv .ct").all_inner_texts()]


async def memory_view(u: U.User) -> None:
    pg = u.page
    await pg.goto(U.APP + "#memory")
    await pg.wait_for_selector("table.memtable, .card", timeout=20000)
    await pg.wait_for_timeout(2000)


async def memory_card(u: U.User, heading: str) -> str:
    """The text of the Memory page's card whose heading has `heading` ("Pending", "Learned", "Long-term", "Entities",
    "Recent", "Vault", "Profile", …)."""
    await memory_view(u)
    card = u.page.locator(".card", has=u.page.locator("h3", has_text=heading)).first
    if not await card.count():
        return ""
    return " ".join((await card.inner_text()).split())


async def memory_domain(u: U.User, domain: str) -> list[str]:
    """Memory → Long-term: the user clicks the domain chip `domain` ("Preferences", "People", "Sites", "Rules", …) and
    reads the facts the table then shows."""
    await memory_view(u)
    pg = u.page
    card = pg.locator(".card", has=pg.locator("h3", has_text="Long-term")).first
    chip = card.locator("button.chip", has_text=re.compile("^" + re.escape(domain), re.I)).first
    if not await chip.count():
        return []
    await chip.click()
    await pg.wait_for_timeout(600)
    rows = [" ".join(t.split()) for t in await card.locator("table.memtable tbody tr").all_inner_texts()]
    await card.locator("button.chip", has_text=re.compile("^All", re.I)).first.click()
    return rows


async def memory_chips(u: U.User) -> list[str]:
    await memory_view(u)
    card = u.page.locator(".card", has=u.page.locator("h3", has_text="Long-term")).first
    return [" ".join(t.split()) for t in await card.locator("button.chip").all_inner_texts()]


async def memory_try(u: U.User, request: str) -> str:
    """Memory → "Try it": the user types a request and presses Preview; returns what the card then lists."""
    await memory_view(u)
    pg = u.page
    card = pg.locator(".card", has=pg.locator("h3", has_text="Try it")).first
    await card.locator("input").fill(request)
    await card.locator("button.btn.primary").click()
    # the model reads the request first: wait, as the user does, until the card stops showing that it is working
    await pg.wait_for_timeout(500)
    for _ in range(60):
        if not await card.locator("[data-loading]").count():
            break
        await pg.wait_for_timeout(500)
    return " ".join((await card.inner_text()).split())


async def memory_pending(u: U.User) -> str:
    """Memory → Pending: what the suggested changes say, with the value in each one's box."""
    await memory_view(u)
    pg = u.page
    card = pg.locator(".card", has=pg.locator("h3", has_text="Pending")).first
    if not await card.count():
        return ""
    vals = [await x.input_value() for x in await card.locator("input").all()]
    return " ".join((await card.inner_text()).split()) + " " + " ".join(vals)


async def memory_accept_pending(u: U.User, words: str) -> bool:
    """Memory → Pending: the user presses Accept on the suggested change whose text has `words`."""
    await memory_view(u)
    pg = u.page
    card = pg.locator(".card", has=pg.locator("h3", has_text="Pending")).first
    rows = card.locator(".stack", has=pg.locator("button.btn.approve"))
    for i in range(await rows.count()):
        r = rows.nth(i)
        txt = (await r.inner_text()) + " " + " ".join([await x.input_value() for x in await r.locator("input").all()])
        if words.lower() in txt.lower():
            await r.locator("button.btn.approve").first.click()
            await pg.wait_for_timeout(1500)
            return True
    return False


async def profile_value(u: U.User, label: str) -> str:
    """Memory → Profile: the value in the field labelled `label` ("Mobile", "Phone", …)."""
    await memory_view(u)
    pg = u.page
    f = pg.locator("#profile label.field", has_text=label).first
    if not await f.count():
        f = pg.locator("label.field", has_text=label).first
    if not await f.count():
        return ""
    el = f.locator("input, select").first
    return await el.input_value() if await el.count() else ""


async def page_text(u: U.User, view: str) -> str:
    """What one of the app's pages shows (`connections`, `schedules`, `memory`, …), as text."""
    pg = u.page
    await pg.goto(U.APP + "#" + view)
    await pg.wait_for_timeout(3000)
    return " ".join((await pg.locator("#view").inner_text()).split())


# ================================================================================================ slow hotel sites
SLOW_PORT = 8115


class _Slow(BaseHTTPRequestHandler):
    pages: dict[str, tuple[float, str]] = {}

    def do_GET(self):   # noqa: N802
        p = urlparse(self.path)
        hit = self.pages.get(p.path)
        if not hit:
            self.send_response(404)
            self.end_headers()
            return
        delay, html = hit
        time.sleep(delay)
        body = html.encode()
        self.send_response(200)
        self.send_header("Content-Type", "text/html; charset=utf-8")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, *a):   # quiet
        pass


def serve_slow_sites(pages: dict[str, tuple[float, str]]) -> str:
    """The world: hotel websites that take `delay` seconds to answer, at http://shop.test:8115/<path>. Returns the base."""
    _Slow.pages = pages
    srv = ThreadingHTTPServer(("0.0.0.0", SLOW_PORT), _Slow)
    threading.Thread(target=srv.serve_forever, daemon=True).start()
    return f"http://shop.test:{SLOW_PORT}"



# ================================================================================================ mail
def mail_arrives(frm: str, subject: str, body: str) -> None:
    """The world: a new mail lands in the user's inbox (the local IMAP account), next to what is there."""
    import email.utils
    import imaplib
    from tests import skills_common as K
    m = imaplib.IMAP4("127.0.0.1", 1143)
    m.login(K.USER, K.PW)
    now = time.time()
    raw = (f"From: {frm}\r\nTo: {K.USER}\r\nSubject: {subject}\r\nDate: {email.utils.formatdate(now, localtime=True)}\r\n"
           f"Message-ID: <arr{int(now * 1000)}@example.com>\r\nMIME-Version: 1.0\r\nContent-Type: text/plain; charset=utf-8\r\n"
           f"Content-Transfer-Encoding: 8bit\r\n\r\n{body}\r\n").encode()
    m.append("INBOX", None, imaplib.Time2Internaldate(now), raw)
    m.logout()


# ================================================================================================ Automations
async def answers_in(u: U.User, words: str, button: str = "Results") -> int:
    """How many answers the card's run conversation (`button`: "Results" / "View runs") shows now."""
    from tests import shop_common as SC
    await SC.press_on_card(u, words, button)
    await u.page.wait_for_timeout(2000)
    n = await u.page.locator(".msg.assistant:not(.live) .bubble:not(.filecard)").count()
    await u.page.goto(U.APP + "#schedules")
    await u.page.wait_for_timeout(1500)
    return n


async def run_results(u: U.User, words: str, button: str = "Results", prev: int = 0, timeout: float = 600) -> tuple[str, list[str]]:
    """After a run started from Automations: the user opens the card's `button` ("Results" / "View runs") and waits
    for the run's answer. Any approval the run asks for is denied (the user asked for a summary, not for something
    to be sent). `prev`: the answers the conversation already had (an earlier run's). Returns (the new answer, the
    approval dialogs' texts)."""
    from tests import shop_common as SC
    pg = u.page
    denied: list[str] = []
    await SC.press_on_card(u, words, button)
    end = time.monotonic() + timeout
    seen = 0
    while time.monotonic() < end:
        modal = pg.locator(".modal-back .approval")
        if await modal.count() and not await modal.first.locator("button.btn.approve:disabled").count():
            denied.append(" ".join((await modal.first.inner_text()).split()))
            await modal.first.locator("button.btn.danger").first.click()
            await pg.wait_for_timeout(1000)
            continue
        review = pg.locator(".sys-card button.btn.approve")
        if await review.count() and len(denied) < 3:
            await review.first.click()
            await pg.wait_for_timeout(800)
            continue
        n = await pg.locator(".msg.assistant:not(.live) .bubble:not(.filecard)").count()
        live = await pg.locator(".msg.assistant.live").count()
        busy = await pg.locator(".taskcard:not(.folded)").count()
        if n > prev and not live and not busy:
            return await pg.locator(".msg.assistant:not(.live) .bubble:not(.filecard)").last.inner_text(), denied
        await pg.wait_for_timeout(3000)
        seen += 1
        if n <= prev and seen % 5 == 0:   # the conversation fills in when the run writes to it: look again
            await pg.goto(U.APP + "#schedules")
            await pg.wait_for_timeout(1500)
            await SC.press_on_card(u, words, button)
    return "", denied
