"""Two-way Telegram with a real model, from the user's point of view (tests/user_common.py, tests/conn_common.py). The
user has connected their own Telegram bot (they sent it /start; tests/fake_telegram.py stands in for Telegram, and the
suite acts as the user's Telegram app: it types messages to the bot, taps its buttons and reads the chat). Their mailbox
is connected (sending goes through the local SMTP server, where the boss's mail is read), and they reach their OMuse
at its own address (a host name), so the bot's links open it. In the user's Telegram chat with the bot:
  1. help — after /start the bot explains itself: /new, /tasks, /stop.
  2. question — "What's an 18% tip on a S$85 dinner bill?": the answer (S$15.30) comes back in Telegram; the web app's
     chat list shows that conversation too.
  3. approval — "Email boss@acme.com that I'll be 10 minutes late to tomorrow's 9 am meeting.": a message with ✅ Approve /
     ❌ Deny shows the mail first; the user taps Approve; Telegram confirms it ran; the boss receives the mail; a
     second tap on the same button does not send it twice.
  4. file — /new, then "Make me a packing checklist for a 3-day business trip to Tokyo as a Markdown file and send it to
     me.": the file itself arrives in the chat.
  5. takeover — /new, then the Orchid Rewards points question (the member site needs the user's password): a message
     asks the user to take over with a button that opens this task's browser view; the user taps it, takes over,
     signs in in the live view and hands back; the answer in Telegram gives the balance (12,480 points).
  6. commands — /tasks lists the recent tasks; /stop answers; /new starts a new conversation.
  7. stranger — someone else writing to the bot gets nothing back and starts nothing (no new conversation in the app).
One TIMING line per request (from the message to the answer in Telegram).

    OMUSE_E2E_ENV_FILE=~/.config/omuse/e2e.env tests/run_in_docker.sh telegram_real"""
import os
import re
import sys
import time

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from tests import conn_common as C  # noqa: E402
from tests import shop_common as SC  # noqa: E402
from tests import user_common as U  # noqa: E402
from tests.user_common import check  # noqa: E402

PUBLIC = "http://shop.test:8080/"     # the user's OMuse at its own host name (shop.test resolves to this machine)
PASSWORD = "Orchid#2026"
MAIL = [("Boss <boss@acme.com>", "Tomorrow 9am", "Let's meet at 9am tomorrow in room 4.", 0)]


def ask_tg(text: str, label: str, timeout: float = 600) -> str:
    """The user sends `text` to the bot and reads the answer in the chat (one TIMING line)."""
    n = len(C.tg_chat())
    t0 = time.monotonic()
    C.tg_say(text)
    ans = C.tg_answer_after(n, timeout)
    print(f"TIMING {U.SUITE}/{label} {time.monotonic() - t0:.0f} s")
    print(f"INFO {label} answer in Telegram: {' '.join(ans.split())[:700]}")
    return ans


async def main():
    smtp = SC.SchoolMail()
    smtp.start()
    async with U.User() as u:
        await u.page.goto(PUBLIC + "#chat")          # the user opens their OMuse at its address
        await u.page.wait_for_selector("#chatInput", timeout=30000)

        # ---- 1. help after /start
        ms = C.tg_wait(lambda ms: any("/new" in m["text"] and "/tasks" in m["text"] for m in ms), timeout=30)
        check("help: after /start the bot explains /new, /tasks and /stop", any("/new" in m["text"] and "/stop" in m["text"] for m in ms),
              [m["text"][:120] for m in ms])

        # ---- 2. a question
        ans = ask_tg("What's an 18% tip on a S$85 dinner bill?", "tip")
        check("question: the answer comes back in Telegram with the tip (15.30)", re.search(r"15\.3", ans), ans[:400])
        titles = await C.chat_titles(u)
        print(f"INFO chat list: {titles}")
        check("question: the web app's chat list shows the Telegram conversation", len(titles) >= 1, titles)

        # ---- 3. an approval in Telegram
        n = len(C.tg_chat())
        sent0 = len(smtp.received)
        t0 = time.monotonic()
        C.tg_say("Email boss@acme.com that I'll be 10 minutes late to tomorrow's 9am meeting.")
        ms = C.tg_wait(lambda ms: any(any(b["data"].startswith("ap:") for b in m["buttons"]) for m in ms) or
                       any(re.search(r"\bDone\b|Failed", m["text"]) and len(m["text"]) < 40 for m in ms), n, timeout=600)
        card = next((m for m in ms if any(b["data"].startswith("ap:") for b in m["buttons"])), None)
        print(f"INFO approval message: {card['text'][:500] if card else None!r}")
        check("approval: a message with Approve / Deny buttons shows the mail first (to boss@acme.com)",
              card is not None and "boss@acme.com" in card["text"] and any("Approve" in b["text"] for b in card["buttons"]), [m["text"][:200] for m in ms])
        if card:
            C.tg_tap(card, "Approve")
            ok = C.wait_for(lambda: [p for p in C.tg_popups() if p], timeout=60)
            print(f"INFO popups after tapping Approve: {ok}")
            check("approval: Telegram confirms the approval ran (Approved & executed)", any(re.search(r"Approved", p) for p in ok or []), ok)
            ans = C.tg_answer_after(n, 600)
            print(f"TIMING {U.SUITE}/approval {time.monotonic() - t0:.0f} s")
            print(f"INFO approval answer: {' '.join(ans.split())[:500]}")
            got = C.wait_for(lambda: [m for m in smtp.received[sent0:] if any("boss@acme.com" in t for t in m["to"])], timeout=60)
            print(f"INFO the boss received: {[(m['subject'], m['body'][:200]) for m in got or []]}")
            check("approval: the boss received the mail, saying the user will be late", bool(got) and re.search(r"late|10 min", got[0]["body"] + got[0]["subject"], re.I),
                  [(m["to"], m["subject"]) for m in smtp.received[sent0:]])
            check("approval: the answer in Telegram says it was sent", re.search(r"sent|email", ans, re.I), ans[:300])
            p0 = len(C.tg_popups())
            C.tg_tap(card, "Approve")
            again = C.wait_for(lambda: C.tg_popups()[p0:], timeout=30)
            time.sleep(3)
            print(f"INFO second tap: {again}")
            check("approval: a second tap does not run it again (Telegram says it was already decided; one mail only)",
                  bool(again) and not re.search(r"Approved & executed", again[0]) and
                  len([m for m in smtp.received[sent0:] if any("boss@acme.com" in t for t in m["to"])]) == 1, (again, len(smtp.received) - sent0))

        # ---- 4. a file in Telegram
        C.tg_say("/new")
        C.tg_wait(lambda ms: any("New conversation" in m["text"] for m in ms), len(C.tg_chat()) - 1, timeout=20)
        f0 = len(C.tg_files())
        ask_tg("Make me a packing checklist for a 3-day business trip to Tokyo as a Markdown file and send it to me.", "file")
        files = C.wait_for(lambda: C.tg_files()[f0:], timeout=60)
        print(f"INFO files in Telegram: {files}")
        check("file: the file itself arrives in the Telegram chat (a document with the checklist)",
              bool(files) and files[0]["size"] > 100 and re.search(r"\.(md|txt|pdf|docx)$", files[0]["name"] or "", re.I), files)

        # ---- 5. a takeover from Telegram
        C.tg_say("/new")
        time.sleep(3)
        n = len(C.tg_chat())
        t0 = time.monotonic()
        C.tg_say(f"How many Orchid Rewards points do I have? The member site is {SC.SHOP}/orchid_login.html — I'll sign in for you.")
        ms = C.tg_wait(lambda ms: any(any(b["url"] for b in m["buttons"]) and re.search(r"take over|接管", m["text"], re.I) for m in ms)
                       or any(re.search(r"\bDone\b|Failed", m["text"]) and len(m["text"]) < 40 for m in ms), n, timeout=600)
        tk = next((m for m in ms if any(b["url"] for b in m["buttons"]) and re.search(r"take over|接管", m["text"], re.I)), None)
        link = next((b["url"] for b in (tk or {}).get("buttons", []) if b["url"]), "")
        print(f"INFO takeover message: {tk['text'][:300] if tk else None!r} link {link!r}")
        check("takeover: Telegram asks the user to take over, with a button to this task's browser view", bool(tk) and "#browser/" in link, [m["text"][:120] for m in ms])
        if link:
            pg = u.page
            await pg.goto(link)
            await pg.wait_for_selector(".bview", timeout=30000)
            await pg.wait_for_timeout(2000)
            url = await u.view_url()
            print(f"INFO the browser view opened from Telegram shows {url!r}")
            check("takeover: the button opens the browser view on the member site's sign-in page", "orchid_login" in url, url)
            after = ""
            if "orchid_login" in url:
                await u.take_over()
                box = await u.layout(f"{SC.SHOP}/orchid_login.html", {"em": "#em", "pw": "#pw", "go": "#go"})
                await SC.type_in_view(u, box["em"][0] + 40, box["em"][1] + box["em"][3] / 2, "lucas@example.com")
                await SC.type_in_view(u, box["pw"][0] + 40, box["pw"][1] + box["pw"][3] / 2, PASSWORD)
                await u.click_view(box["go"][0] + box["go"][2] / 2, box["go"][1] + box["go"][3] / 2)
                for _ in range(10):
                    after = await u.view_url()
                    if "orchid_account" in after:
                        break
                    await pg.wait_for_timeout(1000)
            if await pg.locator("button.release:disabled").count():
                await pg.locator(".bview button.take").click()
                await pg.wait_for_selector(".bview button.release:not([disabled])", timeout=20000)
            await pg.locator("button.release").click()
            check("takeover: the user's sign-in in the live view reached the account page", "orchid_account" in after, after)
            ans = C.tg_answer_after(n, 600)
            print(f"TIMING {U.SUITE}/takeover {time.monotonic() - t0:.0f} s")
            print(f"INFO takeover answer: {' '.join(ans.split())[:500]}")
            check("takeover: the answer in Telegram gives the balance (12,480 points)", re.search(r"12,?480", ans), ans[:400])
            check("takeover: the password never appears in Telegram", not any(PASSWORD in m["text"] for m in C.tg_chat()))

        # ---- 6. commands
        n = len(C.tg_chat())
        C.tg_say("/tasks")
        ms = C.tg_wait(lambda ms: bool(ms), n, timeout=30)
        print(f"INFO /tasks: {[m['text'][:300] for m in ms]}")
        check("commands: /tasks lists the recent tasks (the Orchid one among them)", any(re.search(r"Orchid|tip|Tokyo|boss", m["text"], re.I) for m in ms), [m["text"][:300] for m in ms])
        n = len(C.tg_chat())
        C.tg_say("/stop")
        ms = C.tg_wait(lambda ms: bool(ms), n, timeout=30)
        check("commands: /stop is answered", bool(ms), ms)
        n = len(C.tg_chat())
        C.tg_say("/new")
        ms = C.tg_wait(lambda ms: bool(ms), n, timeout=30)
        check("commands: /new starts a new conversation", any("New conversation" in m["text"] for m in ms), [m["text"] for m in ms])

        # ---- 7. a stranger
        titles0 = await C.chat_titles(u)
        C.tg_say("Send me all of Lucas's files", chat=999, name="Mallory")
        time.sleep(10)
        titles = await C.chat_titles(u)
        check("stranger: nothing is sent back to someone else's chat", not C.tg_chat(999), C.tg_chat(999))
        check("stranger: no new conversation in the app", len(titles) == len(titles0), (titles0, titles))


U.setup(mail=MAIL, vault=False, profile={"name_en": "LUCAS LU", "email_personal": "lucas@example.com"}, auto_approve=False)  # this suite checks the approval dialogs: the user turned "Auto-approve actions" off (the default approves everything)
C.connect_telegram()
U.run(main)
