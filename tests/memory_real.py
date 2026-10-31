"""Personal context with a real model, from the user's point of view (tests/user_common.py, tests/conn_common.py): what
OMuse remembers, how the Memory page sorts it, and the right facts used for a request.
  1. tell — in Main's chat the user says, one message each: their running shoes are EU 43; on shop.test the phone field
     takes 8 digits without +65; their wife Mei collects jazz vinyl; they prefer business class on long flights; and a
     standing rule ("when I say 'prepare for a trip', first check my mail for the flight and hotel"); each answer is
     about what the user asked to remember. Memory →
     Long-term: the Preferences chip shows the shoe size and business class, Site habits shows shop.test, People shows
     Mei, Rules shows the trip rule. The Entities card lists Mei.
  2. try it — Memory → "Try it" with "help me pick running shoes on shop.test": it lists the shoe size and the shop's
     habit, not business class or the jazz records; with "Suggest a birthday present for my wife" it brings up Mei's
     jazz records.
  3. used — in a new chat: "Suggest a birthday present for my wife." The answer is about jazz vinyl (the wording
     "my wife" found Mei); and "Which shoe size should I order for running shoes?" — EU 43, without asking.
  4. profile — "My new mobile number is +65 9000 1111, and I have a dentist appointment this Friday at 4 pm.": Memory →
     Pending suggests the number for the profile (not applied until the user says so); the user presses Accept;
     Profile → Mobile phone shows +65 9000 1111. The one-off dentist appointment is not a long-term fact.
  5. cards — "I pay with my Citi Prestige from the US, and I also have a Chase Sapphire Reserve 4111 1111 1111 1111.":
     the Vault lists both cards by name; the number the user typed is nowhere on the Memory page.
  6. tidy — the user types two near-identical facts and a process note into Memory → Remember, presses Memory tidy →
     Preview, then Apply: the duplicate is merged (one "quiet hotels" fact left) and nothing the user said in 1 is lost.
  7. title — the chat list shows short titles that say what each chat was about (not the first words of the message).
One TIMING line per chat request.

    OMUSE_E2E_ENV_FILE=~/.config/omuse/e2e.env tests/run_in_docker.sh memory_real"""
import os
import re
import sys

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from tests import conn_common as C  # noqa: E402
from tests import user_common as U  # noqa: E402
from tests.user_common import check  # noqa: E402

TELL = [
    ("shoes", "Please remember: my running shoes are EU size 43."),
    ("shop", "Please remember: on shop.test the phone number field takes 8 digits, without +65."),
    ("mei", "Please remember: my wife Mei collects jazz vinyl records."),
    ("cabin", "Please remember: I prefer business class on long flights."),
    ("rule", "Please remember this rule: whenever I say 'prepare for a trip', first check my mail for the flight and the hotel."),
]
KEY = {"shoes": ("43",), "shop": ("shop.test", "8 digits", "8-digit", "+65"), "mei": ("Mei", "jazz", "vinyl"),
       "cabin": ("business",), "rule": ("prepare for a trip", "flight", "hotel")}
ASK_ONCE = re.compile(r"\?\s*$|what (is|'s) your (shoe )?size|which size do you", re.I)


async def remember_box(u: U.User, fact: str) -> None:
    """Memory → Long-term: the user types a fact into the box and presses Remember."""
    await C.memory_view(u)
    pg = u.page
    card = pg.locator(".card", has=pg.locator("h3", has_text="Long-term")).first
    await card.locator("input[type=text]").first.fill(fact)
    await card.locator("button.btn.primary", has_text="Remember").click()
    await pg.wait_for_timeout(2000)


async def main():
    async with U.User() as u:
        # ---- 1. the user tells OMuse about themselves
        for i, (lab, msg) in enumerate(TELL):
            a = await u.ask(msg, label=f"tell {lab}", new_chat=(i == 0))
            check(f"tell {lab}: the answer came (no time-out)", bool(a.text.strip()) and not a.timed_out, a.text[:200])
            check(f"tell {lab}: the answer is about what the user asked to remember", a.mentions(*KEY[lab]), a.text[:200])
        await u.page.wait_for_timeout(5000)
        print(f"INFO chips: {await C.memory_chips(u)}")
        pref = " ".join(await C.memory_domain(u, "Preferences"))
        site = " ".join(await C.memory_domain(u, "Site habits"))
        ppl = " ".join(await C.memory_domain(u, "People"))
        rules = " ".join(await C.memory_domain(u, "Rules"))
        print(f"INFO Preferences: {pref!r}\nINFO Site habits: {site!r}\nINFO People: {ppl!r}\nINFO Rules: {rules!r}")
        check("tell: Memory → Preferences shows the shoe size (43) and business class", "43" in pref and re.search(r"business", pref, re.I), pref)
        check("tell: Memory → Site habits shows the shop.test phone rule", "shop.test" in site, site)
        check("tell: Memory → People shows Mei and the jazz records", "Mei" in ppl and re.search(r"jazz|vinyl", ppl, re.I), ppl)
        check("tell: Memory → Rules shows the 'prepare for a trip' rule", re.search(r"prepare for a trip", rules, re.I), rules)
        ent = await C.memory_card(u, "People, places, accounts, sites")
        print(f"INFO Entities: {ent!r}")
        check("tell: the Entities card lists Mei", "Mei" in ent, ent)

        # ---- 2. Try it
        tried = await C.memory_try(u, "help me pick running shoes on shop.test")
        print(f"INFO Try it: {tried!r}")
        check("try it: the preview lists the shoe size and the shop's habit", "43" in tried and "shop.test" in tried, tried)
        check("try it: … and leaves out business class and the jazz records", not re.search(r"business class|jazz", tried, re.I), tried)

        tried = await C.memory_try(u, "Suggest a birthday present for my wife")
        print(f"INFO Try it (wife): {tried!r}")
        check("try it: 'a birthday present for my wife' brings up Mei's jazz records", re.search(r"jazz|vinyl", tried, re.I), tried)

        # ---- 3. the right facts used
        a = await u.ask("Suggest a birthday present for my wife.", label="gift")
        check("used: the gift ideas are about jazz vinyl (the wording 'my wife' found Mei)", a.mentions("vinyl", "jazz"), a.text[:500])
        a = await u.ask("Which shoe size should I order for running shoes?", label="size")
        check("used: the answer gives EU 43 without asking", "43" in a.text and not ASK_ONCE.search(a.text.strip()[-80:]), a.text[:300])

        # ---- 4. a profile detail said in the chat
        a = await u.ask("My new mobile number is +65 9000 1111, and I have a dentist appointment this Friday at 4 pm.", label="mobile")
        pend = ""
        for _ in range(8):
            pend = await C.memory_pending(u)
            if "9000" in pend:
                break
            await u.page.wait_for_timeout(5000)
        print(f"INFO Pending: {pend!r}")
        check("profile: Memory → Pending suggests the new mobile number for the profile", "9000" in pend, pend)
        recent, longterm = await C.memory_card(u, "Recent"), await C.memory_card(u, "Long-term")
        print(f"INFO Recent: {recent[:500]!r}")
        check("profile: the dentist appointment (a one-off) is not kept as a long-term fact", not re.search(r"dentist", longterm, re.I),
              (recent[:400], longterm[:400]))
        if "9000" in pend:
            ok = await C.memory_accept_pending(u, "9000")
            shown = await C.profile_value(u, "Mobile phone")
            print(f"INFO Profile → Mobile phone: {shown!r}")
            check("profile: after Accept, Profile → Mobile phone shows +65 9000 1111", ok and re.sub(r"\D", "", shown).endswith("6590001111"), shown)

        # ---- 5. cards named in the chat
        a = await u.ask("I pay with my Citi Prestige from the US, and I also have a Chase Sapphire Reserve 4111 1111 1111 1111.", label="cards")
        vault = ""
        for _ in range(6):
            vault = await C.memory_card(u, "Vault")
            if "Citi Prestige" in vault and "Sapphire" in vault:
                break
            await u.page.wait_for_timeout(5000)
        await C.memory_view(u)
        whole = " ".join((await u.page.locator("#view").inner_text()).split())
        print(f"INFO Vault: {vault[:600]!r}")
        check("cards: the Vault lists the Citi Prestige and the Chase Sapphire Reserve by name", "Citi Prestige" in vault and "Sapphire Reserve" in vault, vault[:600])
        check("cards: the card number the user typed is nowhere on the Memory page", not re.search(r"4111\s?1111", whole), "")

        # ---- 6. tidy
        for f in ("Lucas likes quiet hotels!", "Lucas likes the quiet hotels", "Opened zipair and clicked search"):
            await remember_box(u, f)
        pg = u.page
        await C.memory_view(u)
        tidy = pg.locator(".card", has=pg.locator("h3", has_text="Memory tidy")).first
        await tidy.locator("button", has_text="Preview").first.click()
        prev = ""
        for _ in range(60):
            await pg.wait_for_timeout(5000)
            await C.memory_view(u)
            tidy = pg.locator(".card", has=pg.locator("h3", has_text="Memory tidy")).first
            if await tidy.locator("button", has_text="Apply").count():
                prev = " ".join((await tidy.inner_text()).split())
                break
        print(f"INFO tidy preview: {prev[:800]!r}")
        check("tidy: Preview lists what it would change", bool(prev), prev)
        if prev:
            await tidy.locator("button", has_text="Apply").first.click()
            for _ in range(60):
                await pg.wait_for_timeout(5000)
                await C.memory_view(u)
                tidy = pg.locator(".card", has=pg.locator("h3", has_text="Memory tidy")).first
                if not await tidy.locator("button", has_text=re.compile("Running")).count():
                    break
            longterm = await C.memory_card(u, "Long-term")
            print(f"INFO Long-term after tidy: {longterm[:1500]!r}")
            check("tidy: one 'quiet hotels' fact is left", len(re.findall(r"quiet hotels", longterm, re.I)) == 1, longterm[:1500])
            check("tidy: what the user told OMuse is still there (43, shop.test, Mei, business class)",
                  all(w in longterm for w in ("43", "shop.test", "Mei")) and re.search(r"business", longterm, re.I), longterm[:1500])

        # ---- 7. titles
        titles = await C.chat_titles(u)
        print(f"INFO chat list: {titles}")
        firsts = {m[:40] for _, m in TELL} | {"Suggest a birthday present for my wife."[:40], "Which shoe size should I order for runni"}
        check("title: every chat in the list has a short title (at most 40 characters)", titles and all(0 < len(t) <= 40 for t in titles), titles)
        check("title: the titles say what the chats were about, not the first words of the message",
              titles and not any(t in firsts for t in titles), titles)
        check("title: the gift chat's title is about the gift", any(re.search(r"gift|present|birthday", t, re.I) for t in titles), titles)


U.setup(vault=False)
U.run(main)
