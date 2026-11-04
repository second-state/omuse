"""Bots with a real model, from the user's point of view (OMUSE_E2E_REAL=1, see tests/user_common.py): which bot answers.
The user asks Main's chat for a flight, a return flight as a follow-up, a hotel, a city tip, which card to pay with, and
a sentence rewrite. A travel or card request is answered by the right bot — the answer shows "Continue in ✈️ Travel →"
(or "💳 Money"), and clicking it opens that bot's conversation with the user's question and the same answer,
marked as handed over from Main; the follow-up lands in the same Travel conversation (it now shows both questions). No
request asks the user to approve anything or to take over a browser. The rewrite is answered by Main itself (no bot
link). Then a chat the user starts in the Travel bot's own view (menu → Travel) is answered there by Travel itself — no
hand-over link — and is listed in Travel's chats. Only the routing is checked (what each answer says is the model's
and the live data's). One TIMING line per request (from Enter to the answer).

    OMUSE_E2E_ENV_FILE=~/.config/omuse/e2e.env bash tests/run_in_docker.sh bots_real"""
import datetime as dt
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from tests import user_common as U  # noqa: E402
from tests.user_common import check  # noqa: E402

D1 = dt.date.today() + dt.timedelta(days=30)


def day(d: dt.date) -> str:
    return f"{d:%B} {d.day}, {d.year}"


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
        conv = await handed(u, "flight", a, "Travel")
        check("flight: the question in the Travel conversation is the user's", conv["questions"][:1] and "Singapore" in conv["questions"][0], conv["questions"])

        # 2. the follow-up in the same Main chat → the same Travel conversation
        a = await u.ask("And the cheapest return flight a week later?", label="follow-up", new_chat=False)
        nothing_asked("follow-up", a)
        await handed(u, "follow-up", a, "Travel", questions=2)

        # 3. a hotel → Travel
        a = await u.ask(f"Find me a quiet hotel near Shinjuku station for 2 nights from {day(D1)}.", label="hotel")
        nothing_asked("hotel", a)
        await handed(u, "hotel", a, "Travel")

        # 4. a city tip → Travel (its city guides)
        a = await u.ask("I'm visiting Tokyo for the first time next month. How do I get from Narita airport to Shinjuku, and which transit card should I get?",
                        label="city")
        nothing_asked("city", a)
        await handed(u, "city", a, "Travel")

        # 5. which card → Money
        a = await u.ask("Which credit card should I use to pay for a S$300 dinner in Singapore? I have a DBS Altitude Visa and a Citi Rewards Mastercard.",
                        label="card")
        nothing_asked("card", a)
        await handed(u, "card", a, "Money")

        # 6. a general request stays in Main
        a = await u.ask("Rewrite this sentence to sound more polite: 'Send me the report now.'", label="general")
        nothing_asked("general", a)
        check("general: Main answered itself (no link to another bot)", not a.bot_link, a.bot_link)

        # 7. a chat started in the Travel bot's own view
        await u.open_bot("Travel")
        a = await u.ask("How do I get from Changi Airport to Marina Bay by MRT, and roughly how long does it take?", label="travel view", new_chat=False)
        nothing_asked("travel view", a)
        view = (await u.page.locator("#viewTitle").inner_text()).strip()
        check("travel view: Travel answered in its own view (no hand-over link, still in Travel)", not a.bot_link and view == "Travel", (a.bot_link, view))
        titles = await u.chat_titles("#bot/travel")
        check("travel view: the chat is listed in Travel's chats", len(titles) >= 1, titles)


U.setup(profile={"name_en": "LUCAS LU", "email_personal": "lucas@example.com"})
U.run(main)
