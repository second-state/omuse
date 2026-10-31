"""Scripted (fake-model) remainder of the 0.2.12 suite (grounded choice cards, web page watches, PDF form filling): only
what a real-model run cannot do.

The use cases are tested from the user's side by real-model suites: choice cards (verified, picked by the user) in
tests/shopping_real.py, a price watch (Automations, notifications on each new drop, none on the same price) in
tests/watch_real.py, a PDF form filled from an email and sent back in tests/pdf_forms_real.py. What stays here needs a
model that misbehaves on purpose, a forced failure, or an internal guarantee the user cannot see:
* present_choices refuses a made-up price and a page that was never read (a real model is not made to lie);
* watch_create refuses a keyword that is not on the page and a price the model claims but the watch does not read,
  leaving nothing behind; a notify-only watch fires without any agent run or model call;
* a watch whose page cannot be loaded reports the error and backs off (an unreachable host, forced);
* pdf_form_fill reports a field the form does not have instead of guessing, and the original stays untouched."""
import json
import os
import shutil
import sys
import time

import httpx

B = "http://127.0.0.1:8080"
H = {"X-Persona-UI": "1"}
PAGES = os.path.join(os.path.dirname(__file__), "pages")
WS = "/tmp/claude-0/persona-test/workspace"
c = httpx.Client(timeout=120, trust_env=False)
fails = []


def check(name, cond, info=""):
    print(("PASS " if cond else "FAIL ") + name, "" if cond else str(info)[:1500])
    if not cond:
        fails.append(name)


def run(msg, conv=None, timeout=150):
    r = c.post(B + "/api/chat", json={"message": msg, "conversation_id": conv}, headers=H).json()
    t0 = time.time()
    while time.time() - t0 < timeout:
        t = c.get(f"{B}/api/tasks/{r['task_id']}").json()
        if t["status"] in ("COMPLETED", "FAILED", "CANCELLED", "WAITING_APPROVAL"):
            return t, r["conversation_id"]
        time.sleep(0.5)
    return t, r["conversation_id"]


def results(t, name):
    return [e["data"] for e in t["events"] if e["type"] == "tool_result" and e["data"]["name"] == name]


c.put(B + "/api/settings", json={"language": "zh", "ui_language": "zh"}, headers=H)

# ================================================================ 1. grounded choice cards
t, conv = run("CHOICES find me a nice iPhone case and let me pick")
check("choices: task completes", t["status"] == "COMPLETED", t["status"])
pc = results(t, "present_choices")
check("a made-up price is refused", pc and not pc[0]["ok"] and "S$9.90" in pc[0]["preview"], pc[:1])
check("a page that was never read is refused", pc and "other.test/titanium" in pc[0]["preview"] and "not read" in pc[0]["preview"], pc[:1])
check("exact excerpts from the pages read are accepted", len(pc) > 1 and pc[1]["ok"], pc[1:2])
msgs = c.get(f"{B}/api/conversations/{conv}", headers=H).json()["messages"]
cards = [json.loads(m["content"]) for m in msgs if m["role"] == "system" and '"choices"' in m["content"]]
check("one verified card set is stored in the chat (the refused one is not)", len(cards) == 1 and cards[0]["verified"]
      and len(cards[0]["options"]) == 2, cards)


# ================================================================ 2. web page watch
watch = os.path.join(PAGES, "watch.html")


def page(price):
    whole, frac = price.split(".")
    with open(watch, "w") as f:   # drawn the way big shops draw prices: symbol, whole and fraction in separate pieces
        f.write(f"<!doctype html><meta charset=utf-8><title>Aurora case</title><p>Customers also viewed: Aurora Mini Case S$9.90</p><h1>Aurora Glitter Case for iPhone 17 Pro Max</h1>"
                f"<a href='/store'>Visit the Aurora Store</a><p>4.6 · 1,203 ratings</p>"
                f"<ul>{'<li>Military grade drop protection, MagSafe compatible, translucent matte back</li>' * 12}</ul>"
                f"<p>Price: <span class='a-price'><span class='a-offscreen' style='position:absolute;opacity:0'>S${price}</span>"
                f"<span aria-hidden='true'><span>S$</span><span>{whole}</span><span>.</span><span>{frac}</span></span></span></p>"
                f"<p>Other: Basic Case S$12.90</p><h2>Related</h2><p>Aurora Mini Case S$9.90</p>")


page("25.00")
t, conv = run("WATCHPRICE tell me when the Aurora case is below S$20")
wc = results(t, "watch_create")
check("a keyword that isn't on the page -> no watch, and the agent is told what prices it did see",
      wc and not wc[0]["ok"] and "NOT created" in wc[0]["preview"] and "25" in wc[0]["preview"], wc[:1])
check("the price the model saw (30) differs from what the watch reads (25) -> refused",
      len(wc) > 1 and not wc[1]["ok"] and "not the 30" in wc[1]["preview"], wc[1:2])
check("watch: created by the agent on the third try, reporting what it read",
      t["status"] == "COMPLETED" and len(wc) > 2 and wc[2]["ok"] and "S$25.00" in wc[2]["preview"], wc[2:3])
sch = [s for s in c.get(B + "/api/schedules", headers=H).json()["schedules"] if s["name"] == "Aurora case price"]
check("it is a notify-only web watch", sch and "web.page" in sch[0]["spec"] and '"notify"' in sch[0]["spec"], sch)
check("only one watch was made (the refused one left nothing behind)", len(sch) == 1, sch)
check("the watch card shows what it read (S$25.00, not the related item's S$9.90)",
      sch and "S$25.00" in (sch[0]["trigger"].get("_seen") or ""), sch[:1])
sid = sch[0]["id"] if sch else ""


def poll():
    return c.post(f"{B}/api/schedules/{sid}/poll", json={}, headers=H).json()


def notes():
    return [n for n in c.get(B + "/api/notifications", headers=H).json().get("notifications", []) if "Aurora case price" in n["title"]]


p1 = poll()
check("the next check sees the same price (no alert)", p1.get("ok") and not p1.get("fired") and not notes(), (p1, notes()))
page("18.00")
p2 = poll()
check("price drops below the threshold -> one notification", p2.get("fired") and len(notes()) == 1 and "18" in notes()[0]["body"],
      (p2, notes()))
check("the Basic Case price elsewhere on the page is ignored (keyword)", "12.9" not in notes()[0]["body"], notes()[:1])
tasks_run = [x for x in c.get(B + "/api/tasks?limit=50", headers=H).json().get("tasks", []) if x.get("schedule_id") == sid]
check("notify-only: no agent run, no model call", not tasks_run, tasks_run)
# failures back off (and don't alert)
spec = json.loads(sch[0]["spec"])
spec["params"]["url"] = "http://nonexistent-host.invalid/"
c.put(f"{B}/api/schedules/{sid}", json={"spec": spec}, headers=H)
p5 = poll()
s5 = [s for s in c.get(B + "/api/schedules", headers=H).json()["schedules"] if s["id"] == sid][0]
check("a failed check is reported on the watch", not p5.get("ok") and s5["trigger"].get("_error"), (p5, s5["trigger"]))
check("…and the next check is pushed back (backoff)", s5["next_run"] > time.time() + spec["every"] * 60 * 1.5, (s5["next_run"] - time.time()))
os.remove(watch)

# ================================================================ 3. PDF form filling
os.makedirs(os.path.join(WS, "forms"), exist_ok=True)
shutil.copy(os.path.join(PAGES, "permission_slip.pdf"), os.path.join(WS, "forms", "permission_slip.pdf"))
t, conv = run("PDFFORM fill in Able's aquarium permission slip")
check("pdf: task completes", t["status"] == "COMPLETED", t["status"])
ff = results(t, "pdf_form_fields")
check("fields are listed with types and options", ff and '"lunch"' in ff[0]["preview"] and '"radio"' in ff[0]["preview"]
      and "vegetarian" in ff[0]["preview"], ff[:1])
fl = results(t, "pdf_form_fill")
check("a filled copy is written; the signature is left for the user, never written",
      fl and fl[0]["ok"] and "permission_slip-filled.pdf" in fl[0]["preview"] and "left_for_the_user_to_sign" in fl[0]["preview"]
      and "signature" in fl[0]["preview"], fl[:1])
check("required/empty fields are pointed out (emergency phone)", fl and "emergency_phone" in fl[0]["preview"], fl[:1])
from pypdf import PdfReader  # noqa: E402


def pdf_values(path):
    """Form field values as the PDF stores them (checkbox / radio states without the leading slash)."""
    return {name: str(f.get("/V", "") or "").lstrip("/") for name, f in (PdfReader(path).get_fields() or {}).items()}


vals = pdf_values(os.path.join(WS, "forms", "permission_slip-filled.pdf"))
check("the filled PDF really contains the values", vals.get("student_name") == "Able Lu" and vals.get("consent") == "Yes"
      and vals.get("lunch") == "vegetarian" and vals.get("tshirt") == "L", vals)
orig = pdf_values(os.path.join(WS, "forms", "permission_slip.pdf"))
check("the original stays untouched", orig.get("student_name") == "" and orig.get("consent") == "Off", orig)
sf = results(t, "send_file")
check("the filled copy is handed to the user to check", sf and sf[0]["ok"], sf[:1])

print("\n" + ("ALL PASS" if not fails else f"{len(fails)} FAILED: {fails}"))
sys.exit(1 if fails else 0)
