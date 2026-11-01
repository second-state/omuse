"""Bots (crate::bots): the routing and hand-over guarantees a real-model run cannot see (scripted model). The use cases —
travel, city and card requests answered by the right bot with the "Continue in …" link, the bot's conversation showing
the question and the same answer, a follow-up landing in the same bot conversation, a general request kept in Main, a
chat started in a bot's own view answered there, an approval inside a bot shown in Main — are tested from the user's
side by tests/bots_real.py (a takeover inside a bot shown in Main: tests/arrival_real.py). What stays here, and why it
cannot be e2e:
  * the five bots with their skills and names (the bot definitions, exact);
  * the planner's routing decision and the hand-over coming from it, the fallbacks when routing fails or names a
    conversation that is not the bot's, Main refusing another bot's skill or tool (internal events and forced
    failures of the planner);
  * the main task waiting in WAITING_BOT and the bot's approval / takeover card in Main marked with the bot (internal
    states);
  * cancelling the main task cancels the bot's: the scripted bot waits for a takeover so the cancel is never a race
    with a real model finishing first;
  * an unknown bot id starts a Main conversation (the exact API)."""
import datetime as dt
import os
import sys
import time

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from tests import skills_common as K  # noqa: E402

B, S, H, c = K.B, K.S, K.H, K.c
DATE = (dt.date.today() + dt.timedelta(days=30)).isoformat()
K.setup()


def wait(tid, ok, timeout=60):
    t0, t = time.time(), {}
    while time.time() - t0 < timeout:
        t = c.get(f"{B}/api/tasks/{tid}").json()
        if ok(t):
            return t
        time.sleep(0.3)
    return t


def conv(cid):
    return c.get(f"{B}/api/conversations/{cid}", headers=H).json()


# ---------------------------------------------------------------- the bots and their skills
bots = c.get(B + "/api/bots", headers=H).json()["bots"]
ids = [b["id"] for b in bots]
K.check("five bots, Main first: main, travel, insurance, cards, photos", ids == ["main", "travel", "insurance", "cards", "photos"], ids)
sk = {b["id"]: set(b["skills"]) for b in bots}
K.check("each bot has its own skills; Main keeps the general ones and none of the travel ones",
        "flight-booking" in sk["travel"] and "flight-booking" not in sk["main"] and "news-search" in sk["main"] and "tokyo" in sk["travel"]
        and "sg-arrival-card" in sk["travel"] and "fwd-travel-insurance" in sk["insurance"] and "drive-video-edit" in sk["photos"]
        and "drive-video-edit" not in sk["main"] and "credit-card-picker" in sk["cards"] and "credit-card-picker" not in sk["main"],
        {k: sorted(v)[:4] for k, v in sk.items()})
K.check("bots may share a skill (pdf-forms: Main, Travel, Insurance & claims)", all("pdf-forms" in sk[b] for b in ("main", "travel", "insurance")))
K.check("every bot has a name in both languages, an icon and a description",
        all(b["name"] and b["name_zh"] and b["icon"] and b["description"] for b in bots), [(b["id"], b["name"], b["name_zh"]) for b in bots])

# ---------------------------------------------------------------- the main bot hands a flight question to Travel
t = K.run(f"BOTHAND cheapest flight Singapore to Tokyo on {DATE} DATE={DATE}", bot="main")
K.trace(t)
main_cid = t["_conv"]
ev = [e for e in t["events"] if e["type"] in ("handoff", "handoff_done")]
K.check("the main task handed over once (ask_bot → handoff event) and ended COMPLETED after the bot answered",
        t["status"] == "COMPLETED" and K.names(t) == ["ask_bot"] and [e["type"] for e in ev] == ["handoff", "handoff_done"], (t["status"], K.names(t), [e["type"] for e in ev], t["_seen"]))   # (the bot's search is code and quick: the poll may miss WAITING_BOT; the events show the hand-over)
ro = [e["data"] for e in t["events"] if e["type"] == "route"]
K.check("the planner routed the request (route event: travel) and the hand-over came from the routing, not the executor",
        [r.get("bot") for r in ro] == ["travel"] and (K.calls(t) or [{}])[0].get("routed") is True, (ro, K.calls(t)))
h = (ev[0]["data"] if ev else {})
kid_cid, kid_tid = h.get("conv_id"), h.get("task_id")
kid = c.get(f"{B}/api/tasks/{kid_tid}").json() if kid_tid else {}
K.check("the bot's task ran in its own conversation with the flight skill and the search", kid.get("status") == "COMPLETED"
        and K.work(kid) == ["flights_search", "present_choices"] and [w["tool"] for w in K.workflow(kid)] == ["travel_search"],
        (kid.get("status"), [x["name"] for x in K.calls(kid)]))   # the search and its choice card by code (the skill load is optional)
lm = [x["id"] for x in c.get(B + "/api/conversations?bot=main", headers=H).json()["conversations"]]
lt = [x["id"] for x in c.get(B + "/api/conversations?bot=travel", headers=H).json()["conversations"]]
K.check("each bot lists its own conversations", main_cid in lm and kid_cid not in lm and kid_cid in lt and main_cid not in lt, (lm[:3], lt[:3]))

# ---------------------------------------------------------------- the main bot has no travel skill of its own
t3 = K.run("BOTSKILL load the flight skill yourself", bot="main")
hs3 = [e["data"] for e in t3["events"] if e["type"] == "handoff"]
ab3 = [x for x in K.calls(t3) if x["name"] == "ask_bot"]
K.check("in Main, load_skill of a travel skill is handed to the Travel bot by code (ask_bot event with the request, handoff)",
        t3["status"] == "COMPLETED" and K.names(t3) == ["load_skill", "ask_bot"] and ab3 and ab3[0].get("for_skill") == "flight-booking"
        and ab3[0]["args"].get("bot") == "travel" and "BOTSKILL" in str(ab3[0]["args"].get("question"))
        and [h.get("bot") for h in hs3] == ["travel"] and (t3.get("result") or "").startswith("TRAVELBOT SKILL ANSWER"),
        (t3["status"], K.calls(t3), hs3, (t3.get("result") or "")[:200]))

K.check("the planner kept that request in Main (route event: main)", [e["data"].get("bot") for e in t3["events"] if e["type"] == "route"] == ["main"],
        [e["data"] for e in t3["events"] if e["type"] == "route"])
t3 = K.run(f"BOTTOOL search the flight yourself DATE={DATE}", bot="main")
fc = [x for x in K.calls(t3) if x["name"] == "flights_search"]
K.check("Main is not offered the travel searches, and a call to one is refused and points to ask_bot", t3["status"] == "COMPLETED"
        and fc and fc[0]["args"].get("offered") is False and "belongs to the bot(s) travel" in (t3.get("result") or ""), (fc, (t3.get("result") or "")[:300]))
# (the bot's answer: code puts the flight's date as a heading above it, so "in", not "startswith")
for marker in ("BOTROUTEFAIL", "BOTROUTEBAD"):
    t4 = K.run(f"{marker} cheapest flight DATE={DATE}", bot="main")
    ro = [e["data"] for e in t4["events"] if e["type"] == "route"]
    hs = [e["data"] for e in t4["events"] if e["type"] == "handoff"]
    K.check(f"{marker}: no usable routing decision (route bot null), so the executor got ask_bot and handed over itself",
            t4["status"] == "COMPLETED" and ro and ro[0].get("bot") is None and hs and not K.calls(t4)[0].get("routed")
            and "TRAVELBOT ANSWER" in (t4.get("result") or ""), (t4["status"], ro, hs, (t4.get("result") or "")[:120]))
t4 = K.run(f"BOTCONVBAD cheapest flight DATE={DATE}", bot="main")
hs = [e["data"] for e in t4["events"] if e["type"] == "handoff"]
K.check("a conversation the planner names that is not that bot's (conv_nope) is dropped: a new Travel conversation",
        t4["status"] == "COMPLETED" and hs and hs[0]["conv_id"].startswith("conv_") and hs[0]["conv_id"] != "conv_nope"
        and "conversation_id" not in K.calls(t4)[0]["args"], (t4["status"], hs, K.calls(t4)[:1]))

# ---------------------------------------------------------------- an approval inside the bot shows in Main too
r = c.post(B + "/api/chat", json={"message": "BOTAPPR book the flight", "bot": "main"}, headers=H).json()
ptid, pcid = r["task_id"], r["conversation_id"]
p = wait(ptid, lambda x: x["status"] == "WAITING_BOT")
w = p.get("waiting") or {}
kt = wait(w.get("task_id", "x"), lambda x: x["status"] == "WAITING_APPROVAL")
K.check("the main task waits for the bot (WAITING_BOT, with the bot and its conversation) while the bot waits for approval",
        p["status"] == "WAITING_BOT" and w.get("type") == "bot" and w.get("bot") == "travel" and kt.get("status") == "WAITING_APPROVAL", (p["status"], w, kt.get("status")))
cards = [m for m in conv(pcid)["messages"] if m["role"] == "system" and '"approval"' in m["content"]]
K.check("the bot's approval card is in the main conversation too, marked with the bot", len(cards) == 1 and '"from_bot":"travel"' in cards[0]["content"].replace(" ", ""),
        [m["content"][:160] for m in cards])
aid = (kt.get("waiting") or {}).get("approval_id")
rr = c.post(f"{S}/approvals/{aid}/resolve", json={"decision": "approve", "scope": "ONCE"}, headers=H)
p = K.wait_end(ptid)
K.check("approved from Main: the bot finished and the main task ended with its answer", rr.status_code == 200 and p["status"] == "COMPLETED"
        and (p.get("result") or "").startswith("TRAVELBOT BOOKED"), (rr.status_code, p["status"], (p.get("result") or "")[:120]))

# ---------------------------------------------------------------- a takeover inside the bot; cancelling Main cancels the bot
r = c.post(B + "/api/chat", json={"message": "BOTWAIT log in for me", "bot": "main"}, headers=H).json()
ptid, pcid = r["task_id"], r["conversation_id"]
p = wait(ptid, lambda x: x["status"] == "WAITING_BOT")
ktid = (p.get("waiting") or {}).get("task_id", "x")
kt = wait(ktid, lambda x: x["status"] == "WAITING_EXTERNAL")
tk = [m for m in conv(pcid)["messages"] if m["role"] == "system" and '"takeover"' in m["content"]]
K.check("the bot's takeover request shows in the main conversation too", kt.get("status") == "WAITING_EXTERNAL" and len(tk) == 1, (kt.get("status"), len(tk)))
c.post(f"{B}/api/tasks/{ptid}/cancel", headers=H)
p = K.wait_end(ptid)
kt = K.wait_end(ktid, timeout=20)
K.check("cancelling the main task cancels the bot's task as well", p["status"] == "CANCELLED" and kt.get("status") == "CANCELLED", (p["status"], kt.get("status")))

# ---------------------------------------------------------------- an unknown bot
r = c.post(B + "/api/chat", json={"message": "hello", "bot": "no-such-bot"}, headers=H).json()
K.c.post(f"{B}/api/tasks/{r['task_id']}/cancel", headers=H)
K.check("an unknown bot id starts a Main conversation", conv(r["conversation_id"])["conversation"]["bot_id"] == "main")
K.finish()
