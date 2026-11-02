"""The model plumbing of the real-model suites (tests/user_common.py): a real model through Settings → Models → "use my
own model" (endpoint OMUSE_E2E_MODEL_URL, key OMUSE_E2E_MODEL_KEY, executor OMUSE_E2E_EXECUTOR, planner
OMUSE_E2E_PLANNER; OMUSE_E2E_MODELS=alt picks OMUSE_E2E_ALT_EXECUTOR / OMUSE_E2E_ALT_PLANNER), always reached through
tests/model_proxy.py (port 8098; started here when it is not running) so model time can be taken out of the wall time
(tests/exec_time.py), the Vault's test card and passport (clearly fake values), PASS / FAIL lines and the exit code. The
model override is always switched off at the end."""
from __future__ import annotations

import os
import subprocess
import sys
import time

import httpx

# "today" in the suites is the app's today: the test stack runs on Asia/Singapore (tests/run_local.sh), so a date taken
# in UTC is yesterday's from 16:00 UTC on (claims_real 2026-10-30: an e-ticket "today" that the app read as yesterday)
os.environ["TZ"] = "Asia/Singapore"
time.tzset()

B = "http://127.0.0.1:8080"
PROXY = "http://127.0.0.1:8098"
FAKE_LLM = "http://127.0.0.1:8090"
H = {"X-Persona-UI": "1"}
REAL = os.environ.get("OMUSE_E2E_REAL", "") not in ("", "0", "false")
c = httpx.Client(timeout=120, trust_env=False)
fails: list[str] = []
CARD = {"label": "Visa Lucas", "kind": "card", "values": {"number": "4111 1111 1111 1111", "expiry": "12/29", "cvc": "123",
                                                         "holder": "LUCAS LU"}}
PASSPORT = {"label": "Passport Lucas", "kind": "id_document", "values": {"number": "E1234567X", "name": "LU LUCAS",
                                                                         "expiry": "2031-05-01", "country": "SG"}}


def check(name, cond, info=""):
    print(("PASS " if cond else "FAIL ") + name, "" if cond else str(info)[:1500], flush=True)
    if not cond:
        fails.append(name)


# ------------------------------------------------------------------ stack setup
def _up(url: str) -> bool:
    try:
        c.get(url, timeout=3)
        return True
    except httpx.HTTPError:
        return False


def _wait_up(url: str, secs: float = 20) -> bool:
    t0 = time.time()
    while time.time() - t0 < secs:
        if _up(url):
            return True
        time.sleep(0.3)
    return False


def models() -> tuple[str, str]:
    alt = os.environ.get("OMUSE_E2E_MODELS", "") == "alt"
    ex = os.environ.get("OMUSE_E2E_ALT_EXECUTOR" if alt else "OMUSE_E2E_EXECUTOR", "")
    pl = os.environ.get("OMUSE_E2E_ALT_PLANNER" if alt else "OMUSE_E2E_PLANNER", "")
    return ex, pl


def start_proxy() -> None:
    """model_proxy.py on 8098 with the right upstream: the real endpoint (and key) or the fake model."""
    want = os.environ.get("OMUSE_E2E_MODEL_URL", "").rstrip("/") if REAL else FAKE_LLM
    if _up(PROXY + "/_info"):
        if c.get(PROXY + "/_info").json().get("upstream", "").rstrip("/") == want:
            c.post(PROXY + "/_reset")
            return
        subprocess.run(["bash", "-c", "fuser -k 8098/tcp >/dev/null 2>&1; pkill -f 'model_proxy:app' >/dev/null 2>&1"], check=False)
        time.sleep(1)
    env = {**os.environ, "MODEL_PROXY_UPSTREAM": want, "MODEL_PROXY_KEY": os.environ.get("OMUSE_E2E_MODEL_KEY", "") if REAL else ""}
    root = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
    log = open("/tmp/claude-0/model_proxy.log" if os.path.isdir("/tmp/claude-0") else os.devnull, "a")
    subprocess.Popen([sys.executable, "-m", "uvicorn", "tests.model_proxy:app", "--port", "8098", "--log-level", "warning"],
                     cwd=root, env={**env, "PYTHONPATH": root}, stdout=log, stderr=log, start_new_session=True)
    if not _wait_up(PROXY + "/_info"):
        raise SystemExit("model_proxy did not start on 8098")


def use_proxy_model() -> None:
    """Settings → Models → use my own model, pointed at the proxy (the endpoint the app sees in both modes)."""
    ex, pl = models() if REAL else ("fake", "")
    body = {"model_override": True, "model_base_url": PROXY + "/v1", "model_api_key": "via-proxy", "model_name": ex,
            "planner_model": pl}
    r = c.put(B + "/api/settings", json=body, headers=H)
    check("Settings → Models: own model through the timing proxy", r.status_code == 200 and r.json()["settings"]["model_override"] is True,
          (r.status_code, r.text[:300]))


def model_off() -> None:
    r = c.put(B + "/api/settings", json={"model_override": False}, headers=H)
    check("own-model override switched off again", r.status_code == 200 and r.json()["settings"]["model_override"] is False, r.text[:200])


def seed_vault() -> dict:
    have = {i["label"]: i["id"] for i in c.get(B + "/sentinel/api/vault", headers=H).json().get("items", [])}
    out = {}
    for item in (CARD, PASSPORT):
        if item["label"] not in have:
            r = c.post(B + "/sentinel/api/vault", json=item, headers=H)
            have[item["label"]] = (r.json().get("item") or r.json()).get("id")
        out[item["kind"]] = have[item["label"]]
    return out


def finish() -> None:
    try:
        model_off()
    finally:
        print("\n" + ("ALL PASS" if not fails else f"{len(fails)} FAILED: {fails}"), flush=True)
        sys.exit(1 if fails else 0)
