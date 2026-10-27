"""Streamed answers against the real gateway (OMUSE_E2E_REAL=1, the e2e env file): one short request straight to the
endpoint (no proxy, so the SSE framing is the provider's own), and the page's event stream must carry answer_delta
events ending in the stored answer. Some providers behind a gateway hand the whole event stream over in one piece
(the gateway's `openai/…` route does; `openai-codex/…` and `xai/…` stream as they write): a tiny probe request tells
which, and only a streaming provider must produce several deltas, each a longer prefix of the next, well before the
task finishes. Two short completions are billed.

Not in run_in_docker.sh's default list:
    OMUSE_E2E_ENV_FILE=~/.config/omuse/e2e.env bash tests/run_in_docker.sh stream_real
    OMUSE_E2E_MODELS=alt OMUSE_E2E_ENV_FILE=~/.config/omuse/e2e.env bash tests/run_in_docker.sh stream_real"""
import json
import os
import threading
import time

import httpx

from tests import travel_common as T

if os.environ.get("OMUSE_E2E_REAL") != "1":
    print("stream_real needs a real model (OMUSE_E2E_REAL=1)")
    raise SystemExit(0)
URL = os.environ["OMUSE_E2E_MODEL_URL"].rstrip("/")
KEY = os.environ["OMUSE_E2E_MODEL_KEY"]
EX, _ = T.models()
c, B, H = T.c, T.B, T.H
EV = []
STOP = threading.Event()


def listen():
    with httpx.Client(timeout=None, trust_env=False) as cl, cl.stream("GET", B + "/api/stream", headers=H) as r:
        for line in r.iter_lines():
            if STOP.is_set():
                break
            if line.startswith("data:"):
                try:
                    EV.append({"t": time.time(), **json.loads(line[5:])})
                except ValueError:
                    pass


def provider_streams() -> bool:
    """Whether the endpoint delivers a streamed reply in more than one piece (a short probe, no proxy)."""
    body = {"model": EX, "messages": [{"role": "user", "content": "Count from one to forty in words."}], "stream": True, "max_tokens": 120, "temperature": 0}
    n = 0
    with httpx.Client(timeout=120, trust_env=False) as cl, cl.stream("POST", URL + "/chat/completions", json=body,
                                                                    headers={"Authorization": "Bearer " + KEY, "Accept-Encoding": "identity"}) as r:
        for _ in r.iter_raw():
            n += 1
    return n > 1


STREAMS = provider_streams()
print(f"INFO {EX}: the provider {'streams as it writes' if STREAMS else 'hands the stream over in one piece'}")
threading.Thread(target=listen, daemon=True).start()
c.put(B + "/api/settings", json={"language": "en", "stream_answers": True, "model_override": True, "model_base_url": URL,
                                 "model_api_key": KEY, "model_name": EX, "planner_model": ""}, headers=H)
try:
    t0 = time.time()
    r = c.post(B + "/api/chat", json={"message": "Write a short story of about 150 words about a lighthouse keeper. Just write it, no tools."}, headers=H).json()
    tid = r["task_id"]
    t = {}
    while time.time() - t0 < 180:
        t = T.task(tid)
        if t["status"] in ("COMPLETED", "FAILED", "CANCELLED"):
            break
        time.sleep(0.5)
    done = time.time()
    deltas = [e for e in EV if e.get("kind") == "answer_delta" and e.get("task_id") == tid and not e.get("calls")]
    texts = [d["text"] for d in deltas]
    print(f"INFO model {EX}: {len(deltas)} deltas, first after {deltas[0]['t'] - t0:.1f} s, task done after {done - t0:.1f} s, answer {len(t.get('result') or '')} chars"
          if deltas else f"INFO model {EX}: no deltas; task {t.get('status')} {t.get('error')}")
    T.check("the task completed", t.get("status") == "COMPLETED" and len(t.get("result") or "") > 200, (t.get("status"), t.get("error"), (t.get("result") or "")[:120]))
    T.check("the deltas are each a longer prefix of the next", deltas and all(texts[i + 1].startswith(texts[i]) for i in range(len(texts) - 1)), [len(x) for x in texts][:12])
    if STREAMS:
        T.check("a streaming provider: several deltas, the first well before the task finished", len(deltas) >= 3 and len(texts[-1]) > len(texts[0])
                and deltas[0]["t"] < done - 1.0, (len(deltas), deltas[0]["t"] - t0 if deltas else None, done - t0))
    T.check("the last delta is the stored answer, no think tags", texts and texts[-1].strip() == (t.get("result") or "").strip() and "<think>" not in texts[-1],
            ((texts[-1][-80:] if texts else None), (t.get("result") or "")[-80:]))
finally:
    T.model_off()
    STOP.set()
T.finish()
