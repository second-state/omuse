"""Scripted (fake-model) remainder of the 0.2.12 suite (PDF form filling): only what a real-model run cannot do — a
model that names a field the form does not have. pdf_form_fill reports that field instead of guessing, points out the
required fields left empty, and the original stays untouched. (The choice-card and web-watch cases went with the test
pages they needed.)"""
import os
import shutil
import sys
import time

import httpx

B = "http://127.0.0.1:8080"
H = {"X-Persona-UI": "1"}
FIXTURES = os.path.join(os.path.dirname(__file__), "fixtures")
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

# ================================================================ 3. PDF form filling
os.makedirs(os.path.join(WS, "forms"), exist_ok=True)
shutil.copy(os.path.join(FIXTURES, "permission_slip.pdf"), os.path.join(WS, "forms", "permission_slip.pdf"))
t, conv = run("PDFFORM fill in Able's aquarium permission slip")
check("pdf: task completes", t["status"] == "COMPLETED", t["status"])
ff = results(t, "pdf_form_fields")
check("fields are listed with types and options", ff and '"lunch"' in ff[0]["preview"] and '"radio"' in ff[0]["preview"]
      and "vegetarian" in ff[0]["preview"], ff[:1])
fl = results(t, "pdf_form_fill")
# the slip has no /Sig field: a value for "signature" is an unknown field and is never written (a /Sig field would be
# listed as left_for_the_user_to_sign — decided by the PDF field type, never by a name)
check("a filled copy is written; the signature is left for the user, never written",
      fl and fl[0]["ok"] and "permission_slip-filled.pdf" in fl[0]["preview"] and "unknown field 'signature'" in fl[0]["preview"],
      fl[:1])
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
