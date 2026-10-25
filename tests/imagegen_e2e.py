"""Image generation and editing get only the Images API fields: fast mode (service_tier) and reasoning_effort are chat
options and must never reach /images/generations or /images/edits, while the chat requests of the same task carry them.
Runs with the user's own model (fast mode on, reasoning high, an image model on the fake endpoint)."""
import sys
import time

import httpx

B = "http://127.0.0.1:8080"
LLM = "http://127.0.0.1:8090"
H = {"X-Persona-UI": "1"}
c = httpx.Client(timeout=60, trust_env=False)
fails = []


def check(name, cond, info=""):
    print(("PASS " if cond else "FAIL ") + name, "" if cond else str(info)[:800], flush=True)
    if not cond:
        fails.append(name)


r = c.put(B + "/api/settings", headers=H, json={"language": "en", "model_override": True, "model_base_url": LLM + "/v1",
                                                 "model_api_key": "own-key", "model_name": "fake", "image_model": "gpt-image-1",
                                                 "fast_mode": True, "reasoning_effort": "high"})
s = r.json().get("settings", {})
check("own model on with fast mode and reasoning high", r.status_code == 200 and s.get("fast_mode") is True
      and s.get("reasoning_effort") == "high", r.text[:300])
n0 = c.get(LLM + "/calls").json()["n"]
tid = c.post(B + "/api/chat", headers=H, json={"message": "IMAGEGEN draw a fox, then give it a scarf"}).json()["task_id"]
t = {}
for _ in range(200):
    t = c.get(f"{B}/api/tasks/{tid}").json()
    if t["status"] in ("COMPLETED", "FAILED", "CANCELLED"):
        break
    time.sleep(0.3)
names = [e["data"]["name"] for e in t.get("events", []) if e["type"] == "tool_call"]
check("the task made a picture and edited it", t["status"] == "COMPLETED" and "make_image" in names and "edit_image" in names,
      (t["status"], t.get("error"), names))
imgs = c.get(LLM + "/image_calls").json()
gens = [x for x in imgs if x["url"] == "generations"]
edits = [x for x in imgs if x["url"] == "edits"]
check("the Images API got a generation and an edit request", gens and edits, imgs)
check("generation request: the image model, no service_tier, no reasoning_effort",
      gens and gens[0]["body"].get("model") == "gpt-image-1" and all("service_tier" not in x["body"] and "reasoning_effort" not in x["body"] for x in gens),
      gens)
check("edit request: no service_tier, no reasoning_effort", edits and all("service_tier" not in x["body"] and "reasoning_effort" not in x["body"] for x in edits),
      edits)
last = c.get(LLM + "/calls").json()
check("the task's chat requests did carry them", last["n"] > n0 and (last["last"] or {}).get("service_tier") == "fast"
      and (last["last"] or {}).get("reasoning_effort") == "high", {k: (last["last"] or {}).get(k) for k in ("service_tier", "reasoning_effort")})
c.put(B + "/api/settings", headers=H, json={"model_override": False})
print("\n" + ("ALL PASS" if not fails else f"{len(fails)} FAILED: {fails}"))
sys.exit(1 if fails else 0)
