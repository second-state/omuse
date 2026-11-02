"""Scripted (fake-model) remainder of the 0.2.10 shopping suite: only what a real-model run cannot do.

The use case — shopping on a long product page (find / look / scroll, choose, add to cart without an approval, buying
handed over to the user) — is tested from the user's side by tests/shopping_real.py. What stays here needs a forced
condition: the configured vision model cannot take images (Settings → Models with a text-only vision model), and
browser_look must then fail cleanly with a hint instead of hanging or inventing what it saw. A real model run cannot
make the vision endpoint refuse images on purpose."""
import sys
import time

import httpx

B = "http://127.0.0.1:8080"
H = {"X-Persona-UI": "1"}
c = httpx.Client(timeout=90, trust_env=False)
fails = []


def check(name, cond, info=""):
    print(("PASS " if cond else "FAIL ") + name, "" if cond else str(info)[:1200])
    if not cond:
        fails.append(name)


def run(msg, timeout=120):
    tid = c.post(B + "/api/chat", json={"message": msg}, headers=H).json()["task_id"]
    t0 = time.time()
    while time.time() - t0 < timeout:
        t = c.get(f"{B}/api/tasks/{tid}").json()
        if t["status"] in ("COMPLETED", "FAILED", "CANCELLED", "WAITING_APPROVAL"):
            return t
        time.sleep(0.5)
    return t


def results(t, name):
    return [e["data"] for e in t["events"] if e["type"] == "tool_result" and e["data"]["name"] == name]


c.put(B + "/api/settings", json={"language": "en"}, headers=H)

# ---------------------------------------------------------------- the configured vision model can't take images
sys_url = c.get(B + "/api/settings", headers=H).json()["settings"]["model_base_url"]
c.put(B + "/api/settings", json={"model_override": True, "model_base_url": sys_url, "model_api_key": "k", "model_name": "fake", "vision_model": "text-only"}, headers=H)
t = run("SHOPCART open Amazon and add an iPhone case")
looks = results(t, "browser_look")
check("without a vision model browser_look fails cleanly with a hint", looks and not looks[0]["ok"] and "Vision model" in looks[0]["preview"], looks[:1])
c.put(B + "/api/settings", json={"model_override": False, "language": "zh"}, headers=H)

print("\n" + ("ALL PASS" if not fails else f"{len(fails)} FAILED: {fails}"))
sys.exit(1 if fails else 0)
