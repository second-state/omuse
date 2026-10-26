"""Shared bits of the Google Drive suites: connect Drive (and the calendar) with the one-click OMuse Google sign-in
against the fake Google in tests/fake_apps.py, reset the fake Drive, read its state."""
import json
import os
import sys

import httpx

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from tests import skills_common as K  # noqa: E402

B = K.B
S = B + "/sentinel/api"
F = "http://127.0.0.1:8094"
H = K.H
RT = {"X-Persona-Runtime": "rt-test"}
GM = "/tmp/claude-0/persona-test/google_managed.json"
c = httpx.Client(timeout=120, trust_env=False)


def conn(name):
    return next(x for x in c.get(S + "/connections", headers=H).json()["connections"] if x["name"] == name)


def tools():
    return {t["function"]["name"] for t in c.get(B + "/internal/catalog", headers=RT).json()["tools"]}


def act(tool, args, task="t-drive"):
    return c.post(B + "/internal/act", json={"tool": tool, "args": args, "task_id": task, "call_id": "x"}, headers=RT).json()


def managed_client(on=True):
    os.makedirs(os.path.dirname(GM), exist_ok=True)
    open(GM, "w").write(json.dumps({"client_id": "omuse-test.apps.googleusercontent.com", "client_secret": "GOCSPX-test-secret-123",
                                    "relay": F + "/relay/callback"}) if on else "{}")


def connect_drive(services=("calendar", "drive")):
    """One-click sign-in; returns the result page text."""
    managed_client(True)
    r = c.post(S + "/connections/drive/google/start", json={"origin": B, "services": list(services)}, headers=H).json()
    page = c.get(r["auth_url"], follow_redirects=True)
    return page.text


def reset_drive():
    c.post(F + "/drive/_reset")


def drive_state():
    return c.get(F + "/drive/_state").json()


def disconnect_all():
    c.delete(S + "/connections/drive/credential", headers=H)
    c.delete(S + "/connections/calendar/credential", headers=H)
