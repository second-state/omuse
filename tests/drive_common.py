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


# ---------------------------------------------------------------- the real-model Drive suites (drive_*_real.py)
# What the user sees in their own Google Drive (the fake Drive stands in for it), read through the Drive API with the
# user's token, never the fake's /_state.
_UTOKEN = {"Authorization": "Bearer gat-1"}


def my_drive(*, trashed: bool = False) -> list[dict]:
    """The user's files (no folders) as Drive lists them: in My Drive (`trashed=False`) or in the Bin (`trashed=True`):
    {"id", "name", "mimeType", "size", "parents", "webViewLink"}."""
    q = f"trashed = {'true' if trashed else 'false'}"
    rows = c.get(F + "/drive/files", params={"q": q, "pageSize": 1000}, headers=_UTOKEN).json().get("files", [])
    return [{k: f.get(k) for k in ("id", "name", "mimeType", "size", "parents", "webViewLink")} for f in rows
            if f.get("mimeType") != "application/vnd.google-apps.folder"]


def image_model_settings(url: str, key: str, model: str) -> dict:
    """Settings → Models: the user's own image endpoint, key and model (world setup, as tests/media_real.py does)."""
    r = c.put(B + "/api/settings", headers=H, json={"model_override": True, "image_base_url": url, "image_api_key": key, "image_model": model})
    return r.json().get("settings", {})


async def attach(u, *paths: str) -> list[dict]:
    """The user clicks ＋ in the chat (a tests/user_common.User), picks files and waits until each chip is uploaded or
    failed: [{"name", "bad", "text"}] as the chips show them."""
    pg = u.page
    async with pg.expect_file_chooser() as fc:
        await pg.locator("button.attach").click()
    await (await fc.value).set_files(list(paths))
    await pg.wait_for_function("n => { const c = [...document.querySelectorAll('.chip-att')]; "
                               "return c.length >= n && c.every(x => !/%$/.test(x.querySelector('.small').textContent.trim())); }",
                               arg=len(paths), timeout=60000)
    chips = await pg.evaluate("() => [...document.querySelectorAll('.chip-att')].map(c => ({name: c.querySelector('.nm').textContent, "
                              "bad: c.classList.contains('bad'), text: c.innerText.replace(/\\s+/g, ' ')}))")
    print(f"INFO chips: {chips}")
    return chips
