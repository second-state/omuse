"""A price watch with a real model, from the user's point of view (tests/user_common.py, tests/shop_common.py). Kaya
Mobile's page for the Aurora Glitter Case (tests/pages/kaya_aurora.html at http://shop.test:8099, written by
shop_common.write_deal_page) shows S$25.00, drawn the way big shops draw prices (symbol, whole and fraction in separate
pieces), with other products' prices around it (S$9.90, S$12.90).

1. In Main's chat: "Tell me when the Aurora Glitter Case drops below S$20." Checked: the answer reports the price the
   page shows now (S$25.00, not a related item's); Automations lists one watch on that page that says what it read
   (25.00). The notification is the watch's own: no question back, nothing bought.
2. The shop drops the price to S$18.00 (the world). On Automations the user presses "Check now" on the watch: a
   notification (🔔) with the new price (18) appears, and none quotes the S$12.90 of another product.
3. "Check now" again with the price unchanged: "Nothing new", no second notification.
4. The price drops again to S$16.00: "Check now" brings a new notification with 16.
One TIMING line.

    OMUSE_E2E_ENV_FILE=~/.config/omuse/e2e.env tests/run_in_docker.sh watch_real"""
import os
import re
import sys

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from tests import shop_common as S  # noqa: E402
from tests import user_common as U  # noqa: E402
from tests.user_common import check  # noqa: E402

URL = f"{S.SHOP}/kaya_aurora.html"


async def check_now(u: U.User) -> list[str]:
    """The user presses Check now on the watch; returns the toasts that appear within a few seconds."""
    n0 = len(await S.toasts(u))
    await S.press_on_card(u, "kaya_aurora", "Check now")
    await u.page.wait_for_timeout(6000)
    new = (await S.toasts(u))[n0:]
    print(f"INFO toasts after Check now: {new}")
    return new


async def main():
    S.write_deal_page("25.00")
    async with U.User() as u:
        a = await u.ask(f"Tell me when the Aurora Glitter Case on Kaya Mobile drops below S$20: {URL}", label="watch")
        check("watch: the answer came (no time-out)", bool(a.text.strip()) and not a.timed_out, a.text[:300])
        check("watch: the answer reports the price the page shows now (S$25.00)", re.search(r"(S\$|SGD)\s?25(\.00)?\b", a.text), a.text[:600])
        check("watch: … not a related item's price as the case's", not re.search(r"(currently|now|is)\s+(at\s+)?(S\$|SGD)\s?(9\.90|12\.90)", a.text, re.I),
              a.text[:600])
        card = await S.automation_card(u, "kaya_aurora")
        print(f"INFO Automations card: {card!r}")
        everything = await S.app_page(u, "schedules")
        check("watch: Automations lists a watch on the Aurora case's page", bool(card), everything[:1500])
        check("watch: … that says what it read (25.00)", "25.00" in card, card)
        watches = [x for x in await S.automation_cards(u) if "kaya_aurora" in x]
        check("watch: … only one watch on that page", len(watches) == 1, watches)

        await S.watch_toasts(u)
        S.write_deal_page("18.00")
        t = await check_now(u)
        bells = [x for x in t if "🔔" in x]
        check("drop: the price fell below S$20 → a notification with the new price (18)", any("18" in x for x in bells), t)
        check("drop: … about the case, not another product's S$12.90", bells and not any("12.9" in x for x in bells), bells)

        t = await check_now(u)
        check("same price: no second notification (Nothing new)", not [x for x in t if "🔔" in x], t)

        S.write_deal_page("16.00")
        t = await check_now(u)
        check("lower again: a new notification with 16", any("🔔" in x and "16" in x for x in t), t)


U.setup(mail=[], vault=False)
U.run(main)
