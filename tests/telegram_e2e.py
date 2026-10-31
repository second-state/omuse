"""Telegram: the guarantees a real-model run cannot see (local stack + fake Telegram API on 8091, scripted model). The
use cases — help after /start, a question answered in Telegram and listed in the app, an approval tapped in Telegram
(and a second tap that does not run it twice), a file sent to the chat, a takeover link that opens the task's browser
view, /tasks, /stop, /new, a stranger ignored — are tested from the user's side by tests/telegram_real.py. What stays
here, and why it cannot be e2e:
  * the chat id is detected from the user's /start (the Connections page's detect call: the exact API);
  * the "working on it" message is edited with the progress (an internal detail of how the chat is updated);
  * an approval resolved from Telegram is recorded as approved via telegram in the audit (internal)."""
import sys, time, httpx
B, TG = "http://127.0.0.1:8080", "http://127.0.0.1:8091"
H = {"X-Persona-UI": "1"}
c = httpx.Client(timeout=60, trust_env=False)
fails = []
def check(name, cond, info=""):
    print(("PASS " if cond else "FAIL ") + name, "" if cond else str(info)[:400]); (None if cond else fails.append(name))
def log(): return c.get(TG + "/_log").json()
def texts(): return [m["text"] for m in log()["sent"]]
def say(text, chat=555):
    c.post(TG + "/_push", json={"message": {"message_id": 1, "chat": {"id": chat, "type": "private", "first_name": "Lucas"}, "text": text}})
def wait_for(pred, timeout=40):
    t0 = time.time()
    while time.time() - t0 < timeout:
        if pred(): return True
        time.sleep(0.5)
    return False

# chat id detection + configure
say("/start")
d = c.post(B + "/sentinel/api/connections/telegram/detect", json={"bot_token": "123:ABC"}, headers=H).json()
check("detect chat id from /start", d.get("chats") and d["chats"][0]["chat_id"] == "555", d)
r = c.post(B + "/sentinel/api/connections/telegram/credential", json={"bot_token": "123:ABC", "chat_id": "555"}, headers=H)
check("telegram credential saved + welcome sent", r.status_code == 200 and any("已连接 Telegram" in t for t in texts()), r.text)
wait_for(lambda: c.get(B + "/sentinel/api/telegram/status").json().get("running"), 20)
st = c.get(B + "/sentinel/api/telegram/status").json()
check("bot polling", st.get("running") and st.get("configured"), st)

say("你好")
check("(setup) a plain message gets its answer", wait_for(lambda: any("这是一个普通回答" in t for t in texts())), texts()[-4:])
check("progress message edited", len(log()["edits"]) >= 1)

say("/new")  # fresh conversation so the fake LLM sees BROWSE as the goal
time.sleep(2)
say("打开购物网站帮我下单 BROWSE")
ok = wait_for(lambda: any(m.get("reply_markup") and "ap:" in str(m["reply_markup"]) for m in log()["sent"]), 60)
check("(setup) approval pushed with inline buttons", ok, texts()[-3:])
appr = [m for m in log()["sent"] if m.get("reply_markup") and "ap:" in str(m["reply_markup"])]
if appr:
    m = appr[-1]
    cb = m["reply_markup"]["inline_keyboard"][0][0]["callback_data"]
    c.post(TG + "/_push", json={"callback_query": {"id": "cb1", "data": cb, "message": {"message_id": m["message_id"], "chat": {"id": 555}, "text": m["text"]}}})
    wait_for(lambda: any(a.get("callback_query_id") == "cb1" for a in log()["answers"]))
    aid = cb.split(":")[1]
    ap = [a for a in c.get(B + "/sentinel/api/approvals").json()["approvals"] if a["id"] == aid]
    check("approval resolved via telegram", ap and ap[0]["status"] == "approved", ap and ap[0]["status"])
    ev = c.get(B + "/sentinel/api/audit?limit=50").json()["events"]
    check("audit records via=telegram", any(e["action"] == "approval.approve" and '"via": "telegram"' in str(e.get("detail")) or
                                            (e["action"] == "approval.approve" and (e.get("detail") or {}).get("via") == "telegram") for e in ev))
c.post(B + "/sentinel/api/browser/release", json={}, headers=H)
print(f"\n{len(fails)} failures", fails)
sys.exit(1 if fails else 0)
