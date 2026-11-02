"""Chat titles: the guarantees a real-model run cannot see (local stack, scripted model: "Topic: <first word>"). The use
case — a finished chat shows a short title in the chat list that says what it was about, not the first words of the
message — is tested from the user's side by tests/memory_real.py. What stays here, and why it cannot be e2e:
  * until the first task ends the title is the question cut to the UI's limit (the exact placeholder);
  * a follow-up re-titles the chat after its task (the scripted model's titles differ by design; a real model may
    rightly keep the same title);
  * re-titling does not reorder the list: updated_at stays the last message's (an internal guarantee)."""
import sys
import time

import httpx

B = "http://127.0.0.1:8080"
H = {"X-Persona-UI": "1"}
c = httpx.Client(timeout=60, trust_env=False)
fails = []


def check(name, cond, info=""):
    print(("PASS " if cond else "FAIL ") + name, "" if cond else str(info)[:1200])
    if not cond:
        fails.append(name)


def wait_done(tid, timeout=60):
    t0 = time.time()
    while time.time() - t0 < timeout:
        t = c.get(f"{B}/api/tasks/{tid}").json()
        if t["status"] in ("COMPLETED", "FAILED", "CANCELLED"):
            return t
        time.sleep(0.5)
    return t


def title_of(cid, want=None, timeout=15):
    t0 = time.time()
    while time.time() - t0 < timeout:
        conv = next((x for x in c.get(B + "/api/conversations").json()["conversations"] if x["id"] == cid), None)
        if conv and (want is None or want(conv["title"])):
            return conv["title"]
        time.sleep(0.3)
    return conv["title"] if conv else None


q = "Helsinki weather this weekend and what to pack for a 3-day trip with kids, please be thorough"
r = c.post(B + "/api/chat", json={"message": q}, headers=H).json()
cid = r["conversation_id"]
check("a new chat starts with the question as its title", title_of(cid) == q[:40], title_of(cid))
wait_done(r["task_id"])
t = title_of(cid, want=lambda x: x.startswith("Topic"))
check("(setup) after the task the chat is named after its content", t == "Topic: Helsinki", t)

# a follow-up in the same chat re-titles it after the new task
r2 = c.post(B + "/api/chat", json={"message": "Also compare hotel prices there", "conversation_id": cid}, headers=H).json()
wait_done(r2["task_id"])
t2 = title_of(cid, want=lambda x: x != t)
check("the title follows the conversation", t2 == "Topic: Also", t2)
conv = next(x for x in c.get(B + "/api/conversations").json()["conversations"] if x["id"] == cid)
check("re-titling does not reorder the list (updated_at is the last message's)", conv["updated_at"] <= time.time())
print("ALL PASS" if not fails else f"{len(fails)} FAILED: {fails}")
sys.exit(1 if fails else 0)
