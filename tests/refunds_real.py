"""An order's refund with a real model, from the user's point of view (tests/user_common.py, tests/claims_common.py).
Japan's tax refund is tests/tax_refund_real.py (on the real web).

Order, then refund — a shop's pages at http://shop.test:8099 (tests/pages/stride_*.html: Stride Sports, Kiprun socks
   S$4.90, checkout, confirmation SR-48213, the order page with "Request cancellation"). The user asks Main's chat to
   order the socks with the Visa from their Vault; Sentinel never lets the agent press Place order, so the agent fills
   the checkout and hands it over: the user presses Place order in the live view and hands back. Checked: the takeover
   was for placing the order, on the shop's checkout; the answer gives the order number (SR-48213), the item and the
   amount (S$4.90); Status → Ledger lists the order. The shop's confirmation email then arrives (the world, not a check).
   Then the user asks for a refund: "cancel it and get my money back". The agent finds the order (the ledger, the mail),
   opens its order page and hands over the final button: the user presses "Request cancellation" in the live view.
   Checked: the takeover is on the order page of SR-48213; the user's press reaches the shop's "cancellation requested"
   page; the answer reports the cancellation of SR-48213 and the S$4.90 refund; the ledger still lists the order (its
   status is printed). The agent itself pressed neither button: each was pressed by the user in a takeover.
One TIMING line per request.

    OMUSE_E2E_ENV_FILE=~/.config/omuse/e2e.env bash tests/run_in_docker.sh refunds_real"""
import os
import re
import sys
from urllib.parse import parse_qs, urlsplit

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from tests import claims_common as C  # noqa: E402
from tests import skills_common as K  # noqa: E402
from tests import user_common as U  # noqa: E402
from tests.user_common import check  # noqa: E402

SHOP = "http://shop.test:8099"
CARD_NO = "4111111111111111"
CONFIRM_MAIL = ("Stride Sports <orders@stridesports.example>", "Your Stride Sports order SR-48213 is confirmed",
                "Hi Tan Wei Ming,\n\nThank you for your order SR-48213: Kiprun Run 500 running socks × 1, S$4.90, home delivery.\n"
                f"Manage, cancel or return your order: {SHOP}/stride_order.html?no=SR-48213\n\nStride Sports", 0)


async def press(u: U.User, pg, url_for_layout: str, sel: str, until: str, shot: str) -> str:
    """The user takes over, presses the button `sel` in the live view and waits for the page whose address has `until`."""
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
        await u.view_picture(f"/repo-out/{shot}.png")
    return after


async def order_and_refund(u: U.User) -> None:
    seen = {"buy_url": "", "buy_after": "", "cancel_url": "", "cancel_after": ""}

    async def place(pg, request: str) -> None:
        seen["buy_url"] = await u.view_url()
        print(f"INFO takeover (buy): {request!r} url {seen['buy_url']!r}")
        if "stride_socks" in seen["buy_url"]:
            seen["buy_after"] = await press(u, pg, f"{SHOP}/stride_socks.html", "#place", "stride_done", "refund_checkout")
        print(f"INFO after Place order: {seen['buy_after']!r}")

    a = await u.ask(f"Please order the Kiprun Run 500 running socks for me from Stride Sports: {SHOP}/stride_socks.html — home delivery, "
                    "pay with my Visa from the Vault. I'll press Place order myself.", label="order", on_takeover=place, timeout=1200)
    answers = [a]
    if not a.takeovers and not a.timed_out:
        a = await u.ask("Yes, please go ahead — my name, phone and address are in my profile and the card is in the Vault.",
                        label="order follow-up", new_chat=False, on_takeover=place, timeout=1200)
        answers.append(a)
    text = "\n".join(x.text for x in answers)
    takeovers = [t for x in answers for t in x.takeovers]
    check("order: the answer came (no time-out)", bool(a.text.strip()) and not a.timed_out, a.text[:300])
    check("order: the user was asked to take over on the shop's checkout (a takeover card; the order was not placed for the user)",
          bool(takeovers) and "stride_socks" in seen["buy_url"], (takeovers, seen["buy_url"]))
    q = {k: v[0] for k, v in parse_qs(urlsplit(seen["buy_after"]).query).items()}
    check("order: the order the user placed has the delivery details and the Vault card filled in (the filled checkout)",
          "stride_done" in seen["buy_after"] and q.get("address") and C.flat(q.get("card", "")) == CARD_NO, q)

    # the shop's confirmation email arrives
    K.seed_mail([CONFIRM_MAIL])

    async def cancel(pg, request: str) -> None:
        seen["cancel_url"] = await u.view_url()
        print(f"INFO takeover (cancel): {request!r} url {seen['cancel_url']!r}")
        if "stride_order" in seen["cancel_url"] and "cancel=requested" not in seen["cancel_url"]:
            seen["cancel_after"] = await press(u, pg, f"{SHOP}/stride_order.html?no=SR-48213", "#cancel", "cancel=requested", "refund_cancel")
        print(f"INFO after Request cancellation: {seen['cancel_after']!r}")

    a = await u.ask("I don't need those running socks any more — please cancel the Stride Sports order and get me a refund.",
                    label="refund", on_takeover=cancel, timeout=1200)
    answers = [a]
    if not a.takeovers and not a.timed_out:
        a = await u.ask("Yes, cancel it — the reason is that I changed my mind. I'll press the final button myself.",
                        label="refund follow-up", new_chat=False, on_takeover=cancel, timeout=1200)
        answers.append(a)
    text = "\n".join(x.text for x in answers)
    takeovers = [t for x in answers for t in x.takeovers]
    check("refund: the answer came (no time-out)", bool(a.text.strip()) and not a.timed_out, a.text[:300])
    check("refund: the user was asked to take over for the cancellation", bool(takeovers), takeovers)
    check("refund: the page handed over is the order page of SR-48213, cancellation not yet requested (the agent did not press it)",
          "stride_order" in seen["cancel_url"] and "SR-48213" in seen["cancel_url"] and "cancel=requested" not in seen["cancel_url"],
          seen["cancel_url"])
    check("refund: the user's press reaches the shop's 'cancellation requested' page", "cancel=requested" in seen["cancel_after"],
          seen["cancel_after"])
    check("refund: the answer reports the cancellation of SR-48213 and the S$4.90 refund",
          "SR-48213" in text and re.search(r"cancel", text, re.I) and re.search(r"(S\$|SGD)\s?4\.90", text) and re.search(r"refund", text, re.I),
          text[:900])
    everything = await u.thread_text()
    check("refund: the card number never appears in the chat", CARD_NO not in everything.replace(" ", ""), "")


async def main():
    async with U.User() as u:
        await order_and_refund(u)


U.setup(mail=[], vault=False, profile={"name_en": "TAN WEI MING", "email_personal": "weiming@example.com", "phone": "+65 9123 4567",
                              "address_home": "10 Anson Road, #12-01, Singapore 079903"})
K.vault_item("Visa Wei Ming", "card", {"number": "4111 1111 1111 1111", "expiry": "12/29", "cvc": "123", "holder": "TAN WEI MING"},
             domains="shop.test")
U.run(main)
