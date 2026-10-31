"""The Vault's fill secrecy and memory's exact write-time guarantees, which a real-model run cannot see (local stack,
scripted model). The use cases — a profile detail said in the chat waits under Pending until the user accepts it, a
one-off detail goes to Recent, cards named in the chat are in the Vault by name and the typed number nowhere, the memory
tidy preview then apply — are tested from the user's side by tests/memory_real.py. What stays here, and why it cannot
be e2e:
  * a Vault fill: one approval per fill, the masked value on the card, no standing grant, and the value never in the
    agent's results, snapshot, transcript, vision or the audit (secrecy guarantees on the system's insides);
  * cards: number, expiry and CVC stored as UNKNOWN, the issuing country a field of its own, no number anywhere in the
    profile, the same cards named again not added twice, vault.add audited (exact data guarantees);
  * tidy: the preview changes nothing, lists the merge and the demotion, and applies only once (exact guarantees)."""
import json
import sys
import time

import httpx

B = "http://127.0.0.1:8080"
H = {"X-Persona-UI": "1"}
c = httpx.Client(timeout=60, trust_env=False)
fails = []
SECRET = "KF8812345678"


def check(name, cond, info=""):
    print(("PASS " if cond else "FAIL ") + name, "" if cond else str(info)[:800])
    if not cond:
        fails.append(name)


def chat(msg):
    r = c.post(B + "/api/chat", json={"message": msg}, headers=H)
    r.raise_for_status()
    return r.json()["task_id"]


def wait(tid, states, timeout=90):
    t0 = time.time()
    while time.time() - t0 < timeout:
        t = c.get(f"{B}/api/tasks/{tid}").json()
        if t["status"] in states:
            return t
        time.sleep(0.5)
    return t


# ------------------------------------------------------------------ vault
for it in c.get(B + "/sentinel/api/vault", headers=H).json()["items"]:
    c.delete(B + "/sentinel/api/vault/" + it["id"], headers=H)
r = c.post(B + "/sentinel/api/vault", headers=H, json={"kind": "membership", "label": "KrisFlyer", "domains": "shop.test",
                                                        "values": {"number": SECRET, "name": "LU LIANG"}})
check("vault item saved", r.status_code == 200, r.text)
lst = c.get(B + "/sentinel/api/vault", headers=H).text
check("vault list never returns values", SECRET not in lst and "•••• 5678" in lst, lst[:300])
check("vault writes need the UI header", c.post(B + "/sentinel/api/vault", json={"kind": "other", "label": "x", "values": {"value": "12345"}}).status_code == 403)

tid = chat("VAULTFILL fill my membership number")
t = wait(tid, {"WAITING_APPROVAL", "COMPLETED", "FAILED"})
check("vault fill waits for approval", t["status"] == "WAITING_APPROVAL", t.get("error") or t["events"][-3:])
aps = [a for a in c.get(B + "/sentinel/api/approvals?status=pending", headers=H).json()["approvals"] if a["task_id"] == tid]
check("one approval for the fill", len(aps) == 1, aps)
if aps:
    ap = aps[0]
    s = json.dumps(ap, ensure_ascii=False)
    check("approval card shows label + masked value, not the number", "KrisFlyer" in s and "•••• 5678" in s and SECRET not in s, s[:600])
    r = c.post(f"{B}/sentinel/api/approvals/{ap['id']}/resolve", headers=H,
               json={"decision": "approve", "scope": "PERMANENT"}).json()   # scope is forced to ONCE
    check("approved fill executed", (r.get("result") or {}).get("status") == "ok", r)
    check("no standing grant for vault fills",
          not [g for g in c.get(B + "/sentinel/api/grants", headers=H).json()["grants"] if g["tool"] == "browser_fill_secret"])
t = wait(tid, {"COMPLETED", "FAILED"})
check("task completes", t["status"] == "COMPLETED", t["status"])
everything = json.dumps(t, ensure_ascii=False)
check("the agent never saw the number (results, snapshot, transcript)", SECRET not in everything, everything[-1200:])
look = [e for e in t["events"] if e["type"] == "tool_result" and e["data"]["name"] == "browser_look"]
check("vision works on a page holding a vault value (the value is hidden in the picture; the number is in no result)",
      look and look[0]["data"].get("ok") is True and SECRET not in json.dumps(look, ensure_ascii=False), look)
audit = c.get(B + "/sentinel/api/audit?limit=300", headers=H).text
check("audit never contains the number", SECRET not in audit)
it = c.get(B + "/sentinel/api/vault", headers=H).json()["items"][0]
check("use counted", it["uses"] == 1, it)

# ------------------------------------------------------------------ cards named in the chat
# credit cards named in the chat go into the Vault by name (the issuing country a field of its own, optional) —
# number, expiry and CVC UNKNOWN — never into the profile;
# one entry per card, and a number typed in the chat is never stored
def vault_cards():
    return {i["label"]: i for i in c.get(B + "/sentinel/api/vault", headers=H).json()["items"] if i["kind"] == "card"}


for name in ("Citi Prestige", "Chase Sapphire Reserve"):
    if name in vault_cards():
        c.delete(B + "/sentinel/api/vault/" + vault_cards()[name]["id"], headers=H)
tid = chat("CARDFACT I pay with my Citi Prestige from the US, and I also have a Chase Sapphire Reserve 4111 1111 1111 1111")
wait(tid, {"COMPLETED", "FAILED"})
time.sleep(3)
cards = vault_cards()
m = c.get(B + "/api/memory").json()
check("each card the user named is in the Vault by name, its number, expiry and CVC UNKNOWN",
      all(n in cards and cards[n]["masked"] == "UNKNOWN" and cards[n]["missing"] == ["number", "expiry", "cvc"]
          for n in ("Citi Prestige", "Chase Sapphire Reserve")), cards)
check("the issuing country is a field of its own, when the user said it (optional)",
      cards.get("Citi Prestige", {}).get("country") == "USA" and cards.get("Chase Sapphire Reserve", {}).get("country") == "", cards)
check("no card number stored anywhere", not any("4111" in k for k in cards) and "4111" not in str(m["pending"]) and "4111" not in str(m["profile"]), list(cards))
check("cards are not put in the profile", not any("card" in k.lower() for k in m["profile"]) and not any("card" in p["key"].lower() for p in m["pending"]), m["pending"])
tid = chat("CARDFACT I pay with my Citi Prestige from the US, and I also have a Chase Sapphire Reserve 4111 1111 1111 1111")
wait(tid, {"COMPLETED", "FAILED"})
time.sleep(3)
check("the same cards named again are not added twice", len([k for k in vault_cards() if k in ("Citi Prestige", "Chase Sapphire Reserve")]) == 2
      and len(vault_cards()) == len(cards), list(vault_cards()))
ev = c.get(B + "/sentinel/api/audit?limit=80").json()["events"]
check("adding a card is audited (vault.add by the agent)", any(e["action"] == "vault.add" and e["actor"] == "agent" for e in ev))
for n in ("Citi Prestige", "Chase Sapphire Reserve"):
    c.delete(B + "/sentinel/api/vault/" + vault_cards()[n]["id"], headers=H)

# ------------------------------------------------------------------ tidy: preview, then apply exactly that
c.post(B + "/api/memory", json={"fact": "Opened zipair and clicked search", "category": "other"}, headers=H)
c.post(B + "/api/memory", json={"fact": "Lucas likes quiet hotels!", "category": "preference"}, headers=H)
c.post(B + "/api/memory", json={"fact": "Lucas likes the quiet hotels", "category": "preference"}, headers=H)
before = c.get(B + "/api/memory").json()


def run_job(body):
    r = c.post(B + "/api/memory/consolidate", json=body, headers=H)
    assert r.status_code == 200, r.text
    for _ in range(120):
        j = c.get(B + "/api/memory").json()
        if not j["job"].get("running"):
            return j
        time.sleep(0.5)
    return j


j = run_job({"dry_run": True})
prev = j["runs"][0]
check("preview stored", prev["kind"] == "dry_run" and prev["lines"], prev)
check("preview changes nothing", len(j["facts"]) == len(before["facts"]), (len(j["facts"]), len(before["facts"])))
rep = c.get(f"{B}/api/memory/runs/{prev['id']}").json()["report"]
check("preview lists the merge and the demotion", rep["plan"]["local"]["dups"] and rep["plan"]["llm"]["demote"], rep["plan"])
j = run_job({"from_run": prev["id"]})
facts = [f["fact"] for f in j["facts"]]
check("apply merged the duplicate", sum("quiet hotels" in f for f in facts) == 1, facts)
check("apply demoted the process note", any("Opened zipair" in f["fact"] for f in j["recent"]) and not any("Opened zipair" in f for f in facts))
check("preview marked applied", any(r["id"] == prev["id"] and r["applied"] for r in j["runs"]), j["runs"][:2])
r = c.post(B + "/api/memory/consolidate", json={"from_run": prev["id"]}, headers=H)
time.sleep(1)
check("a preview applies only once", "已执行" in (c.get(B + "/api/memory").json()["job"].get("error") or ""), c.get(B + "/api/memory").json()["job"])

print("\nFAILED:" if fails else "\nALL PASSED", fails)
sys.exit(1 if fails else 0)
