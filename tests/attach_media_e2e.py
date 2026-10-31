"""What tests/attach_media_real.py (real model, from the user's side) cannot do: the forced security refusals of the
upload API that the UI never sends — an upload without the UI header, a message attaching a path that was not uploaded
(../runtime.db). The photos and video saved from a page, attaching files with ＋, the agent reading them and looking
again later, a video attachment and a refused program are tests/attach_media_real.py's."""
import sys

import httpx

B = "http://127.0.0.1:8080"
H = {"X-Persona-UI": "1"}
c = httpx.Client(timeout=120, trust_env=False)
fails = []


def check(name, cond, info=""):
    print(("PASS " if cond else "FAIL ") + name, "" if cond else str(info)[:800])
    if not cond:
        fails.append(name)


# ------------------------------------------------------------------ the upload API's refusals
r = c.put(f"{B}/api/upload", params={"name": "a.txt"}, content=b"hi")
check("upload needs the UI header", r.status_code == 403, r.status_code)
r = c.post(B + "/api/chat", json={"message": "x", "attachments": ["../runtime.db"]}, headers=H)
check("only uploaded files can be attached", r.status_code == 400, r.text)

print("\nFAILED:" if fails else "\nALL PASSED", fails)
sys.exit(1 if fails else 0)
