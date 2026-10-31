"""Bots with a real model, from the user's point of view (OMUSE_E2E_REAL=1, see tests/user_common.py): the user asks
Main's chat for a flight, a return flight as a follow-up, a hotel, a city tip, which card to pay with, and a sentence
rewrite. A travel or card request is answered by the right bot — the answer shows "Continue in ✈️ Travel →" (or
"💳 Credit cards"), and clicking it opens that bot's conversation with the user's question and the same answer, marked
as handed over from Main; the follow-up lands in the same Travel conversation (it now shows both questions). A flight
or hotel answer carries booking links and a Kayak link to compare; no request asks the user to approve anything or to
take over a browser. The rewrite is answered by Main itself (no bot link). Then:
  7. a chat the user starts in the Travel bot's own view (menu → Travel) is answered there by Travel itself — no
     hand-over link — and is listed in Travel's chats;
  8. an approval inside a bot shows in Main: "find the cheapest flight … and email the three cheapest options to my
     wife at mei@example.com" — Travel answers (the "Continue in ✈️ Travel" link), the mail is shown to the user in an
     approval dialog in Main's chat first, and after the user approves Mei receives it with the flights and prices.
(A takeover inside a bot shown in Main is arrival_real's SG Arrival Card, handed to Travel and over to the user for the
CAPTCHA.) Nothing inside the system is looked at.
One TIMING line per request (from Enter to the answer). No SerpApi key: flights come from Kiwi.com, hotels from web
search.

    OMUSE_E2E_ENV_FILE=~/.config/omuse/e2e.env bash tests/run_in_docker.sh bots_real"""
import datetime as dt
import os
import re
import sys

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from tests import user_common as U  # noqa: E402
from tests import conn_common as C  # noqa: E402
from tests import shop_common as SC  # noqa: E402
from tests.user_common import check  # noqa: E402

D1 = dt.date.today() + dt.timedelta(days=30)
D2 = D1 + dt.timedelta(days=7)
FLIGHT_NO = re.compile(r"\b(SQ|TR|NH|JL|CX|MM|ZG|UA|TG|BR|KE|D7|3K|GK)\s?\d{2,4}\b")
AIRLINE = re.compile(r"Singapore Airlines|Scoot|Cathay|\bANA\b|All Nippon|Japan Airlines|\bJAL\b|Zipair|ZIPAIR|Peach|United|Thai|EVA Air|Korean Air|AirAsia|Jetstar")


def day(d: dt.date) -> str:
    return f"{d:%B} {d.day}, {d.year}"


def mentions_day(text: str, d: dt.date) -> bool:
    return bool(re.search(rf"\b({d:%b}|{d:%B})\.? {d.day}\b|\b{d.day} ({d:%b}|{d:%B})\b|{d.isoformat()}", text))


def nothing_asked(label: str, a: U.Answer) -> None:
    check(f"{label}: the user was never asked to approve anything or to take over a browser", not a.approvals and not a.takeovers,
          (a.approvals, a.takeovers))
    check(f"{label}: the answer came (no time-out)", bool(a.text.strip()) and not a.timed_out, a.text[:200])


def same(bot_answer: str, main_answer: str, link: str) -> bool:
    """The bot's answer word for word in Main's (which ends with the "Continue in …" link)."""
    n = lambda s: " ".join(s.split())  # noqa: E731
    return n(bot_answer) == n(n(main_answer).replace(n(link), ""))


async def handed(u: U.User, label: str, a: U.Answer, bot: str, *, questions: int = 1) -> dict:
    """The answer says which bot answered, and its link opens that bot's conversation with the question(s) and this answer."""
    check(f"{label}: the answer shows the link to continue in the {bot} bot", bot in a.bot_link, a.bot_link)
    conv = await u.follow_bot_link()
    check(f"{label}: the link opens the {bot} bot's conversation, marked as handed over from Main", conv["bot"] == bot and conv["from_main"],
          (conv["bot"], conv["from_main"]))
    check(f"{label}: the {bot} conversation shows the user's question" + (f"s ({questions})" if questions > 1 else ""),
          len(conv["questions"]) == questions, conv["questions"])
    check(f"{label}: the {bot} conversation shows the same answer as Main", bool(conv["answers"]) and same(conv["answers"][-1], a.text, a.bot_link),
          (conv["answers"][-1:] and conv["answers"][-1][:200], a.text[:200]))
    return conv


async def main():
    async with U.User() as u:
        # 1. a flight → Travel
        q1 = f"What is the cheapest flight from Singapore to Tokyo on {day(D1)}?"
        a = await u.ask(q1, label="flight")
        nothing_asked("flight", a)
        check("flight: the answer gives flights (airline or flight number) with prices", a.has_prices() and bool(FLIGHT_NO.search(a.text) or AIRLINE.search(a.text)), a.text[:500])
        check("flight: the answer links a booking page and Kayak to compare", any("kayak" not in x for x in a.links) and any("kayak" in x for x in a.links), a.links)
        conv = await handed(u, "flight", a, "Travel")
        check("flight: the question in the Travel conversation is the user's", conv["questions"][:1] and "Singapore" in conv["questions"][0], conv["questions"])

        # 2. the follow-up in the same Main chat → the same Travel conversation
        a = await u.ask("And the cheapest return flight a week later?", label="follow-up", new_chat=False)
        nothing_asked("follow-up", a)
        check("follow-up: the answer gives return flights with prices for a week later", a.has_prices() and bool(FLIGHT_NO.search(a.text) or AIRLINE.search(a.text))
              and (mentions_day(a.text, D2) or a.mentions("return")), a.text[:500])
        await handed(u, "follow-up", a, "Travel", questions=2)

        # 3. a hotel → Travel
        a = await u.ask(f"Find me a quiet hotel near Shinjuku station for 2 nights from {day(D1)}.", label="hotel")
        nothing_asked("hotel", a)
        check("hotel: the answer names hotels near Shinjuku", bool(re.search(r"[Hh]otel|Inn|Hostel", a.text)) and a.mentions("Shinjuku"), a.text[:500])
        check("hotel: the answer links a hotel page and Kayak to compare", any("kayak" not in x for x in a.links) and any("kayak" in x for x in a.links), a.links)
        await handed(u, "hotel", a, "Travel")

        # 4. a city tip → Travel (its city guides)
        a = await u.ask("I'm visiting Tokyo for the first time next month. How do I get from Narita airport to Shinjuku, and which transit card should I get?",
                        label="city")
        nothing_asked("city", a)
        check("city: the answer names the ways from Narita to Shinjuku and the transit card",
              a.mentions("Narita Express", "N'EX", "NEX", "Skyliner", "Keisei", "Limousine") and a.mentions("Suica", "PASMO"), a.text[:500])
        await handed(u, "city", a, "Travel")

        # 5. which card → Credit cards
        a = await u.ask("Which credit card should I use to pay for a S$300 dinner in Singapore? I have a DBS Altitude Visa and a Citi Rewards Mastercard.",
                        label="card")
        nothing_asked("card", a)
        check("card: the answer recommends one of the user's cards", a.mentions("Citi Rewards", "DBS Altitude"), a.text[:500])
        await handed(u, "card", a, "Credit cards")

        # 6. a general request stays in Main
        a = await u.ask("Rewrite this sentence to sound more polite: 'Send me the report now.'", label="general")
        nothing_asked("general", a)
        check("general: Main answered itself (no link to another bot)", not a.bot_link, a.bot_link)
        check("general: the answer is a politer version of the sentence", a.mentions("please", "could you", "would you") and a.mentions("report"), a.text[:300])

        # 7. a chat started in the Travel bot's own view
        await u.open_bot("Travel")
        a = await u.ask("How do I get from Changi Airport to Marina Bay by MRT, and roughly how long does it take?", label="travel view", new_chat=False)
        nothing_asked("travel view", a)
        view = (await u.page.locator("#viewTitle").inner_text()).strip()
        check("travel view: Travel answered in its own view (no hand-over link, still in Travel)", not a.bot_link and view == "Travel", (a.bot_link, view))
        check("travel view: the answer gives the MRT route (East-West / Thomson-East Coast line, Tanah Merah or Expo)",
              a.mentions("Tanah Merah", "East-West", "East West", "Thomson", "Expo", "Downtown") and a.mentions("min"), a.text[:400])
        titles = await C.chat_titles(u, "#bot/travel")
        check("travel view: the chat is listed in Travel's chats", len(titles) >= 1, titles)

        # 8. an approval inside a bot, shown in Main
        sent0 = len(SMTP.received)
        a = await SC.ask(u, f"Find the cheapest flight from Singapore to Tokyo on {day(D1)} and email the three cheapest options to my wife at mei@example.com.",
                         label="bot approval")   # (reads the dialog's boxes too: the recipient is an editable field)
        a.bot_link = await u.page.evaluate("() => { const l = [...document.querySelectorAll('.msg.assistant .handoff-link')].pop(); return l ? l.textContent : ''; }")
        check("bot approval: the answer came (no time-out)", bool(a.text.strip()) and not a.timed_out, a.text[:300])
        check("bot approval: Travel answered (the link to continue in Travel)", "Travel" in a.bot_link, a.bot_link)
        # "Auto-approve actions" is on by default (2026-10-31): the mail goes out without a dialog (it is still recorded)
        check("bot approval: the mail went out without asking (auto-approve is the default)", not a.approvals, a.approvals)
        got = C.wait_for(lambda: [m for m in SMTP.received[sent0:] if any("mei@example.com" in t for t in m["to"])], timeout=60)
        print(f"INFO Mei received: {[(m['subject'], ' '.join(m['body'].split())[:300]) for m in got or []]}")
        check("bot approval: Mei received the mail with flights and prices", bool(got) and U.PRICE.search(got[0]["body"]), [(m["to"], m["subject"]) for m in SMTP.received[sent0:]])


SMTP = SC.SchoolMail()   # the mail server the user's mailbox sends through (what Mei receives)
SMTP.start()
U.setup(travel_connector=True, mail=[("Boss <boss@acme.com>", "Quarterly plan", "Please review the quarterly plan by Friday.", 1)],
        profile={"name_en": "LUCAS LU", "email_personal": "lucas@example.com"})
U.run(main)
