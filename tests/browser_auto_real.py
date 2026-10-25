"""Auto-approve browser actions, with a real model on a real website (OMUSE_E2E_REAL=1, see travel_common.py).

The task reads the user's hotel booking from mail and then opens the hotel's address on openstreetmap.org. Reading
mail taints the task, and a URL that carries parameters on a site the task has not used yet is exactly what Sentinel's
data-egress check stops for approval — so the second step always needs one (the request names the site but not its
domain: a domain written in the request is trusted and would skip the check). With the setting on, the task must never
wait: the action is recorded as approved by "auto" and the answer names the location. With the setting off, the same
request must stop at that step (the suite then approves it by hand so the task can finish).

    OMUSE_E2E_ENV_FILE=~/.config/omuse/e2e.env OMUSE_E2E_MODELS=alt tests/run_in_docker.sh browser_auto_real"""
import json
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from tests import skills_common as K  # noqa: E402
from tests import travel_common as T  # noqa: E402

if not T.REAL:
    print("browser_auto_real needs a real model (OMUSE_E2E_REAL=1)")
    sys.exit(1)

B, H, c = T.B, T.H, T.c
# the site is named, but not its domain: a domain written in the request counts as trusted and skips the egress check
PROMPT = ("Find my Tokyo hotel booking in my email, then look the hotel's address up on OpenStreetMap (the free map site, "
          "use its search page with the address in the URL) and tell me the hotel name, address and whether the map found it. "
          "Research only.")
MAIL = [
    ("Hotel Ginza Grand <reservations@ginzagrand.example>", "Booking confirmed: Hotel Ginza Grand 20-23 Nov 2026",
     "Confirmation 88812345 for Lucas Lu. Hotel Ginza Grand, 8-7-6 Ginza, Chuo City, Tokyo 104-0061, Japan. Check-in 20 Nov 2026 15:00.", 2),
]


def set_auto(on: bool) -> None:
    r = c.put(B + "/sentinel/api/connections/browser", json={"config": {"auto_approve": on}}, headers=H)
    T.check(f"auto-approve browser actions set to {on}", r.status_code == 200, r.text[:200])


def trace(tasks):
    for tid in tasks:
        for e in T.task(tid).get("events", []):
            d = e.get("data") or {}
            if e["type"] == "tool_call":
                print(f"INFO call {d.get('name')} {json.dumps(d.get('args'), ensure_ascii=False)[:160]}")
            elif e["type"] == "tool_result":
                print(f"INFO  -> ok={T.ok(d)} {'auto ' if d.get('approved') == 'auto' else ''}{str(d.get('preview') or d.get('error') or '')[:140]!r}")
            elif e["type"] in ("plan", "waiting", "approval_auto", "approval_resolved", "site_blocked", "error", "retry"):
                print(f"INFO {e['type']} {json.dumps(d, ensure_ascii=False)[:300]}")


K.seed_mail(MAIL)
K.connect_mail()
T.start_proxy()
T.use_proxy_model()
c.put(B + "/api/settings", json={"language": "en"}, headers=H)
try:
    # ---- on: the egress-checked page load is approved by auto, shown, and the task never waits
    set_auto(True)
    info = T.run(PROMPT, timeout=600)
    tasks = info["tasks"]
    trace(tasks)
    final = T.task(tasks[-1])
    ev = [e for tid in tasks for e in T.task(tid).get("events", [])]
    names = [e["data"]["name"] for e in ev if e["type"] == "tool_call" and not (e["data"] or {}).get("sub")]
    auto = [e["data"] for e in ev if e["type"] == "approval_auto"]
    waits = [e["data"] for e in ev if e["type"] == "waiting" and (e["data"] or {}).get("type") == "approval"]
    T.check("on: the task completed", final.get("status") == "COMPLETED", (final.get("status"), final.get("error"), str(final.get("result"))[:300]))
    T.check("on: the booking was read from mail first", any(n.startswith("gmail_") for n in names), names)
    T.check("on: the map site was opened with the address in the URL",
            any("openstreetmap.org" in json.dumps((e["data"] or {}).get("args")) for e in ev if e["type"] == "tool_call"), names)
    T.check("on: the task never waited for an approval", not waits and not info["approvals"], (waits, info["approvals"]))
    T.check(f"on: the egress-checked browser action was approved by auto ({len(auto)})", len(auto) >= 1
            and all(str(a.get("tool", "")).startswith("browser_") for a in auto), (auto, names))
    hist = c.get(B + "/sentinel/api/approvals?status=resolved&limit=50", headers=H).json()["approvals"]
    rec = [a for a in hist if a["task_id"] in tasks]
    T.check("on: each auto-approval is on record, approved, decided by auto", rec and all(a["status"] == "approved" and a.get("decided_by") == "auto" for a in rec), rec[:3])
    res = str(final.get("result") or "")
    T.check("on: the answer names the hotel from the mail", "Ginza Grand" in res, res[:400])
    T.timing("browser_auto_real/on", info, len(names), sum(1 for n in names if n.startswith("browser_")))

    # ---- off: the same request stops at that page load until the user approves
    set_auto(False)
    info2 = T.run(PROMPT, timeout=600)
    tasks2 = info2["tasks"]
    trace(tasks2)
    final2 = T.task(tasks2[-1])
    ev2 = [e for tid in tasks2 for e in T.task(tid).get("events", [])]
    manual = [a for a in info2["approvals"] if str(a["tool"]).startswith("browser_")]
    T.check(f"off: the same work asked for approval ({len(manual)}, approved here by the suite)", len(manual) >= 1,
            (info2["approvals"], [e["data"]["name"] for e in ev2 if e["type"] == "tool_call"]))
    T.check("off: no auto-approval happened", not [e for e in ev2 if e["type"] == "approval_auto"], [e["type"] for e in ev2])
    T.check("off: approved by hand, the task completed", final2.get("status") == "COMPLETED", (final2.get("status"), final2.get("error")))
    T.timing("browser_auto_real/off", info2, len([e for e in ev2 if e["type"] == "tool_call"]),
             len([e for e in ev2 if e["type"] == "tool_call" and e["data"]["name"].startswith("browser_")]))
    os.makedirs(os.path.dirname(T.LAST), exist_ok=True)
    json.dump({"suite": "browser_auto_real", "tasks": tasks + tasks2, "conv_id": info2["conv_id"]}, open(T.LAST, "w"))
finally:
    set_auto(False)
    T.finish()
