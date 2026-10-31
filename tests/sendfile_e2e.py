"""Files handed to the user: the guarantees a real-model run cannot see (local stack, scripted model). The use cases — a
file card in the web chat whose Download saves the file (tests/chat_real.py), the file itself sent to the user's
Telegram chat when the conversation came from Telegram (tests/telegram_real.py) — are tested from the user's side.
What stays here, and why it cannot be e2e:
  * the exact HTTP headers: Download is an attachment named after the file, opening a plain file is inline;
  * a file from a web chat is not also pushed to Telegram (an internal routing guarantee);
  * active content (HTML) in the workspace is never rendered on the app's own origin (forced download, sandbox CSP, no
    text/html), and a path outside the workspace is refused (forced security cases)."""
import os, sys, time
import httpx

B = "http://127.0.0.1:8080"
TG = "http://127.0.0.1:8091"
H = {"X-Persona-UI": "1"}
WS = "/tmp/claude-0/persona-test/workspace"
c = httpx.Client(timeout=60, trust_env=False)
fails = []


def check(name, cond, info=""):
    print(("PASS " if cond else "FAIL ") + name, "" if cond else info)
    if not cond:
        fails.append(name)


def wait(tid, timeout=60):
    t0 = time.time()
    while time.time() - t0 < timeout:
        t = c.get(f"{B}/api/tasks/{tid}").json()
        if t["status"] in ("COMPLETED", "FAILED", "CANCELLED"):
            return t
        time.sleep(0.5)
    return t


# 1 web chat: the file card's download endpoint
r = c.post(B + "/api/chat", json={"message": "SENDFILE make me a brief"}, headers=H).json()
t = wait(r["task_id"])
conv = c.get(f"{B}/api/conversations/{r['conversation_id']}").json()
files = [m for m in conv["messages"] if m["role"] == "system" and '"type": "file"' in m["content"]]
check("(setup) the file card is in the conversation", t["status"] == "COMPLETED" and len(files) == 1 and "reports/brief.md" in files[0]["content"],
      (t["status"], conv["messages"][-3:]))
d = c.get(B + "/api/files/raw", params={"path": "reports/brief.md", "download": 1})
check("download returns the file as an attachment", d.status_code == 200 and "Hello from OMuse" in d.text
      and "attachment" in d.headers.get("content-disposition", "") and "brief.md" in d.headers.get("content-disposition", ""),
      (d.status_code, dict(d.headers)))
i = c.get(B + "/api/files/raw", params={"path": "reports/brief.md"})
check("open (inline) still works for plain files", i.status_code == 200 and "inline" in i.headers.get("content-disposition", ""),
      dict(i.headers))
check("not sent to Telegram (web chat)", not c.get(TG + "/_log").json().get("docs"), c.get(TG + "/_log").json().get("docs"))

# 2 HTML in the workspace is never rendered on the app origin
os.makedirs(WS + "/downloads", exist_ok=True)
open(WS + "/downloads/evil.html", "w").write("<script>fetch('/api/settings')</script>")
e = c.get(B + "/api/files/raw", params={"path": "downloads/evil.html"})
check("HTML is forced to download + sandboxed", "attachment" in e.headers.get("content-disposition", "")
      and "sandbox" in e.headers.get("content-security-policy", "") and "html" not in e.headers.get("content-type", ""),
      dict(e.headers))
bad = c.get(B + "/api/files/raw", params={"path": "../../etc/passwd"})
check("path traversal refused", bad.status_code == 404, bad.status_code)

print("ALL PASS" if not fails else f"{len(fails)} FAILED: {fails}")
sys.exit(1 if fails else 0)
