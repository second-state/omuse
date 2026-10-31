"""Roadmap batch 3 (ledger), the parts a real run cannot check, on the local stack (tests/run_local.sh), driven through
Sentinel's internal API (no model): an approved purchase confirmation becomes a ledger entry with its amount, delivery,
card and approval; the order number from the confirmation page moves it to "placed"; a backfill creates nothing twice;
3 approved Slack posts to one channel give a standing-approval suggestion (never for purchases or order changes) that
only covers that destination once accepted; the Trust page lists the order. Placing an order and requesting its
cancellation as the user (the final click is always the user's, in a takeover) is tests/refunds_real.py."""
import asyncio
import sys
import time

import httpx

B, LLM = "http://127.0.0.1:8080", "http://127.0.0.1:8090"
H = {"X-Persona-UI": "1"}
c = httpx.Client(timeout=90, trust_env=False)
fails = []


def check(name, cond, info=""):
    print(("PASS " if cond else "FAIL ") + name, "" if cond else str(info)[:700], flush=True)
    if not cond:
        fails.append(name)


def task(tid):
    return c.get(f"{B}/api/tasks/{tid}").json()


def wait(pred, timeout=90):
    t0 = time.time()
    while time.time() - t0 < timeout:
        v = pred()
        if v:
            return v
        time.sleep(1)
    return None


def pending(tid):
    return [a for a in c.get(B + "/sentinel/api/approvals?status=pending").json()["approvals"] if a["task_id"] == tid]


def ledger():
    return c.get(B + "/sentinel/api/ledger", headers=H).json()["orders"]


def resolve(aid, decision):
    return c.post(f"{B}/sentinel/api/approvals/{aid}/resolve", json={"decision": decision}, headers=H).json()


RT = {"X-Persona-Runtime": "rt-test"}


def act(tool, args, tsk):
    return c.post(B + "/internal/act", headers=RT, json={"task_id": tsk, "call_id": "c" + str(time.time()), "tool": tool, "args": args}).json()


# ---- 1. purchase → ledger (the confirmation is approved by the user; the order number comes from the page afterwards)
tid = "ledger-t1"
res = act("purchase_confirm", {"site": "shop.test", "items": [{"name": "Kiprun socks", "qty": 1, "price": 4.9}], "shipping": 0,
                               "total": 4.9, "currency": "SGD", "delivery": "Home delivery"}, tid)
check("the purchase confirmation asks the user (auto-approve is off in this stack)", res.get("status") == "approval_required", res)
aid = res.get("approval_id")
if aid:
    resolve(aid, "approve")
L = wait(lambda: [o for o in ledger() if o["task_id"] == tid])
check("an approved purchase becomes a ledger entry with amount, delivery and who approved it",
      L and abs(L[0]["total"] - 4.9) < 0.01 and L[0]["currency"] == "SGD" and L[0]["approval_id"] == aid
      and L[0]["delivery"] == "Home delivery" and L[0]["merchant"] == "shop.test", L)
r = c.post(B + "/internal/ledger/confirm", headers=RT, json={"task_id": tid, "order_number": "OM-77821",
                                                            "evidence": {"kind": "confirmation", "text": "Order OM-77821 placed"}}).json()
L = [o for o in ledger() if o["task_id"] == tid]
check("the confirmation page's order number moves it to \"placed\"", L and L[0]["status"] == "placed" and L[0]["order_number"] == "OM-77821", (r, L))

# ---- backfill is harmless when everything is in already
r = c.post(B + "/api/ledger/backfill", json={}, headers=H).json()
check("backfill creates nothing twice", r.get("created") == [], r)


# ---- 5. fewer approvals: 3 approved Slack posts to #general → a suggestion; accepted → the 4th asks nothing
c.post("http://127.0.0.1:8094/_reset")
c.post(B + "/sentinel/api/connections/slack/credential", headers=H, json={"token": "xoxb-test-token-123456"})



for i in range(3):
    res = act("slack_send_message", {"channel": "#general", "text": f"note {i}"}, f"slack-t{i}")
    if res.get("status") == "approval_required":
        resolve(res["approval_id"], "approve")
sg = c.get(B + "/sentinel/api/grant_suggestions", headers=H).json()["suggestions"]
check("after 3 approvals of the same action, OMuse suggests (does not switch on) a standing approval",
      len(sg) == 1 and sg[0]["tool"] == "slack_send_message" and sg[0]["count"] == 3, sg)
check("purchases and cancels are never suggested", not any(x["tool"] in ("purchase_confirm", "browser_click") for x in sg), sg)
res = act("slack_send_message", {"channel": "#general", "text": "before accepting"}, "slack-t3")
check("…and until accepted it still asks", res.get("status") == "approval_required", res)
if res.get("approval_id"):
    resolve(res["approval_id"], "deny")
if sg:
    c.post(B + "/sentinel/api/grant_suggestions", json={"key": sg[0]["key"], "accept": True, "days": 30}, headers=H)
res = act("slack_send_message", {"channel": "#general", "text": "after accepting"}, "slack-t4")
check("accepted: the same action now goes through without an approval card", res.get("status") == "ok", res)
res = act("slack_send_message", {"channel": "#random", "text": "other channel"}, "slack-t5")
check("…only for that destination (another channel still asks)", res.get("status") == "approval_required", res)

# ---- 4. Trust page
async def ui():
    from playwright.async_api import async_playwright
    async with async_playwright() as p:
        b = await p.chromium.launch()
        pg = await (await b.new_context(viewport={"width": 1400, "height": 1200})).new_page()
        await pg.goto(B + "/#trust")
        await pg.wait_for_selector("text=OM-77821", timeout=20000)
        await pg.screenshot(path="/tmp/claude-0/sc/ledger_page.png", full_page=True)
        txt = await pg.locator(".card", has_text="Ledger").inner_text()
        await b.close()
        return txt
try:
    txt = asyncio.run(ui())
    check("Trust page lists the order with its status", "OM-77821" in txt and "shop.test" in txt, txt[:300])
except Exception as e:
    check("Trust page lists the order with its status", False, repr(e))

print()
print(("%d FAILED: %s" % (len(fails), fails)) if fails else "ALL PASS")
sys.exit(1 if fails else 0)
