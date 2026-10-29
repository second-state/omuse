"""Bots (crate::bots): the main bot and the specialised ones, each with its own skills and conversations; the main bot
hands a request to a specialised bot (ask_bot), waits (WAITING_BOT) and shows its answer word for word with a link to
the bot's conversation. A follow-up goes back to the same bot conversation; an approval or a takeover inside the bot
shows in the main conversation too; cancelling the main task cancels the bot's."""
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
K.check("seven bots, Main first: main, travel, cities, arrival, insurance, cards, photos", ids == ["main", "travel", "cities", "arrival", "insurance", "cards", "photos"], ids)
sk = {b["id"]: set(b["skills"]) for b in bots}
K.check("each bot has its own skills; Main keeps the general ones and none of the travel ones",
        "flight-booking" in sk["travel"] and "flight-booking" not in sk["main"] and "news-search" in sk["main"] and "tokyo" in sk["cities"]
        and "sg-arrival-card" in sk["arrival"] and "fwd-travel-insurance" in sk["insurance"] and "drive-video-edit" in sk["photos"]
        and "drive-video-edit" not in sk["main"] and "credit-card-picker" in sk["cards"] and "credit-card-picker" not in sk["main"],
        {k: sorted(v)[:4] for k, v in sk.items()})
K.check("bots may share a skill (pdf-forms: Main, Arrival & entry, Travel insurance)", all("pdf-forms" in sk[b] for b in ("main", "arrival", "insurance")))
K.check("every bot has a name in both languages, an icon and a description",
        all(b["name"] and b["name_zh"] and b["icon"] and b["description"] for b in bots), [(b["id"], b["name"], b["name_zh"]) for b in bots])

# ---------------------------------------------------------------- the main bot hands a flight question to Travel booking
t = K.run(f"BOTHAND cheapest flight Singapore to Tokyo on {DATE} DATE={DATE}", bot="main")
K.trace(t)
main_cid = t["_conv"]
ev = [e for e in t["events"] if e["type"] in ("handoff", "handoff_done")]
K.check("the main task handed over once (ask_bot → handoff event) and ended COMPLETED after the bot answered",
        t["status"] == "COMPLETED" and K.names(t) == ["ask_bot"] and [e["type"] for e in ev] == ["handoff", "handoff_done"]
        and "WAITING_BOT" in t["_seen"], (t["status"], K.names(t), [e["type"] for e in ev], t["_seen"]))
h = (ev[0]["data"] if ev else {})
kid_cid, kid_tid = h.get("conv_id"), h.get("task_id")
kid = c.get(f"{B}/api/tasks/{kid_tid}").json() if kid_tid else {}
K.check("the bot's task ran in its own conversation with the flight skill and the search", kid.get("status") == "COMPLETED"
        and [x["name"] for x in K.calls(kid)] == ["load_skill", "flights_search"], (kid.get("status"), [x["name"] for x in K.calls(kid)]))
K.check("the main answer is the bot's answer word for word", (t.get("result") or "").startswith(f"TRAVELBOT ANSWER {DATE}") and t["result"] == kid.get("result"),
        ((t.get("result") or "")[:120], (kid.get("result") or "")[:120]))
mc = conv(main_cid)
last = [m for m in mc["messages"] if m["role"] == "assistant"][-1]
K.check("the main conversation's answer carries the link to the bot's conversation (meta.handoff)",
        (last.get("meta") or {}).get("handoff", {}).get("bot") == "travel" and last["meta"]["handoff"]["conv_id"] == kid_cid, last.get("meta"))
kc = conv(kid_cid)
K.check("the bot's conversation belongs to Travel booking and points back to the main conversation; its first message says where it came from",
        kc["conversation"]["bot_id"] == "travel" and kc["conversation"]["parent_conv"] == main_cid
        and kc["messages"][0]["role"] == "user" and kc["messages"][0]["content"].startswith("BOTKID1")
        and (kc["messages"][0].get("meta") or {}).get("from", {}).get("conv_id") == main_cid, (kc["conversation"], kc["messages"][:1]))
lm = [x["id"] for x in c.get(B + "/api/conversations?bot=main", headers=H).json()["conversations"]]
lt = [x["id"] for x in c.get(B + "/api/conversations?bot=travel", headers=H).json()["conversations"]]
K.check("each bot lists its own conversations", main_cid in lm and kid_cid not in lm and kid_cid in lt and main_cid not in lt, (lm[:3], lt[:3]))

# ---------------------------------------------------------------- a follow-up goes back to the same bot conversation
t2 = K.run(f"BOTFOLLOW and the return a week later? DATE={DATE}", conv=main_cid)
ev2 = [e["data"] for e in t2["events"] if e["type"] == "handoff"]
K.check("the follow-up was handed to the same Travel booking conversation (the history named it)", t2["status"] == "COMPLETED"
        and ev2 and ev2[0]["conv_id"] == kid_cid and [x["args"].get("conversation_id") for x in K.calls(t2)] == [kid_cid], (t2["status"], ev2, (t2.get("result") or "")[:200]))
K.check("the bot kept its context: it answered from its own earlier answer", "the outbound was SQ638" in (t2.get("result") or ""), (t2.get("result") or "")[:200])
K.check("the bot's conversation now has both questions", [m["content"][:7] for m in conv(kid_cid)["messages"] if m["role"] == "user"] == ["BOTKID1", "BOTKID2"])

# ---------------------------------------------------------------- the main bot has no travel skill of its own
t3 = K.run("BOTSKILL load the flight skill yourself", bot="main")
K.check("in Main, load_skill of a travel skill is refused and points to ask_bot", t3["status"] == "COMPLETED"
        and "belongs to the bot(s) travel" in (t3.get("result") or "") and "ask_bot" in (t3.get("result") or ""), (t3.get("result") or "")[:300])

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

# ---------------------------------------------------------------- a chat started in a bot's view belongs to that bot
r = c.post(B + "/api/chat", json={"message": "BOTKID1 straight to the travel bot DATE=" + DATE, "bot": "travel"}, headers=H).json()
t5 = K.wait_end(r["task_id"])
K.check("a conversation started in the Travel booking view is the bot's, and the bot answers there itself (no hand-over)",
        conv(r["conversation_id"])["conversation"]["bot_id"] == "travel" and t5["status"] == "COMPLETED"
        and [x["name"] for x in K.calls(t5)] == ["load_skill", "flights_search"], (t5["status"], [x["name"] for x in K.calls(t5)]))
r = c.post(B + "/api/chat", json={"message": "hello", "bot": "no-such-bot"}, headers=H).json()
K.c.post(f"{B}/api/tasks/{r['task_id']}/cancel", headers=H)
K.check("an unknown bot id starts a Main conversation", conv(r["conversation_id"])["conversation"]["bot_id"] == "main")
K.finish()
