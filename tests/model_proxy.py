"""OpenAI-compatible timing proxy for the e2e suites (port 8098).

Forwards /v1/* to MODEL_PROXY_UPSTREAM (e.g. https://gateway.example/v1 or http://127.0.0.1:8090) and records, for
every request, its start / end on the monotonic clock (shared by all processes of the machine, so a test can compare
them with its own time.monotonic()), the model, a purpose guess, the status and whether the app will retry it.

* MODEL_PROXY_KEY set: the request's Authorization header is replaced by `Bearer $MODEL_PROXY_KEY` (the app's own
  key, if any, never reaches the upstream). Unset: the app's header is passed through (the fake model checks it).
* Streaming (`"stream": true`) and plain requests; the upstream's status, body and Retry-After header are passed back
  unchanged, so the app's own retry logic runs. A 429/503 or any response with Retry-After is recorded with retry=True.
* GET /_intervals → {"intervals": [...], "now": monotonic}; POST /_reset clears them; GET /_info shows the upstream
  (never the key).

    MODEL_PROXY_UPSTREAM=http://127.0.0.1:8090 python3 -m uvicorn tests.model_proxy:app --port 8098
"""
from __future__ import annotations

import json
import os
import time
from urllib.parse import urlparse

import httpx
from fastapi import FastAPI, Request
from fastapi.responses import JSONResponse, Response, StreamingResponse

app = FastAPI()
UPSTREAM = os.environ.get("MODEL_PROXY_UPSTREAM", "http://127.0.0.1:8090").rstrip("/")
KEY = os.environ.get("MODEL_PROXY_KEY", "")
BASE = UPSTREAM[:-3] if UPSTREAM.endswith("/v1") else UPSTREAM     # /v1/... is appended to this
INTERVALS: list[dict] = []
_local = (urlparse(UPSTREAM).hostname or "") in ("127.0.0.1", "localhost", "::1")
client = httpx.AsyncClient(timeout=httpx.Timeout(900, connect=30), trust_env=not _local)
PASS_BACK = ("content-type", "retry-after", "x-request-id", "openai-processing-ms")


def _purpose(body: dict) -> str:
    msgs = body.get("messages") or []
    sys = str(msgs[0].get("content") or "") if msgs and msgs[0].get("role") == "system" else ""
    last = str(msgs[-1].get("content") or "") if msgs else ""
    if "planning module" in sys:
        return "planner"
    if sys.startswith("You name conversations"):
        return "title"
    if sys.startswith(("You look at", "You locate things", "This is a zoomed-in part", "A red circle")):
        return "vision"
    if last.startswith(("Extract durable facts", "Sort what the USER says", "You are tidying")):
        return "memory"
    if body.get("tools"):
        return "executor"
    return "other"


def _headers(req: Request) -> dict:
    h = {"content-type": req.headers.get("content-type", "application/json"), "accept": req.headers.get("accept", "*/*")}
    if KEY:
        h["authorization"] = f"Bearer {KEY}"
    elif req.headers.get("authorization"):
        h["authorization"] = req.headers["authorization"]
    return h


def _record(rec: dict, status: int, headers) -> None:
    ra = headers.get("retry-after") if headers is not None else None
    rec.update({"end": time.monotonic(), "end_wall": time.time(), "status": status, "retry_after": ra,
                "retry": status in (429, 503) or bool(ra)})


@app.api_route("/v1/{path:path}", methods=["GET", "POST"])
async def forward(path: str, req: Request):
    raw = await req.body()
    body: dict = {}
    if raw:
        try:
            body = json.loads(raw)
        except ValueError:
            body = {}
    rec = {"id": len(INTERVALS) + 1, "path": "/v1/" + path, "start": time.monotonic(), "start_wall": time.time(),
           "model": body.get("model", ""), "purpose": _purpose(body) if path.endswith("chat/completions") else path,
           "stream": bool(body.get("stream")), "status": None, "end": None, "retry": False}
    INTERVALS.append(rec)
    url = f"{BASE}/v1/{path}"
    if req.url.query:
        url += "?" + req.url.query
    try:
        if rec["stream"]:
            up = await client.send(client.build_request(req.method, url, content=raw, headers=_headers(req)), stream=True)
            if up.status_code >= 400:
                data = await up.aread()
                await up.aclose()
                _record(rec, up.status_code, up.headers)
                return Response(data, status_code=up.status_code, headers={k: v for k, v in up.headers.items() if k in PASS_BACK})

            async def gen():
                try:
                    async for chunk in up.aiter_raw():
                        yield chunk
                finally:
                    await up.aclose()
                    _record(rec, up.status_code, up.headers)
            return StreamingResponse(gen(), status_code=up.status_code,
                                     headers={k: v for k, v in up.headers.items() if k in PASS_BACK and k != "content-type"},
                                     media_type=up.headers.get("content-type", "text/event-stream"))
        up = await client.request(req.method, url, content=raw if raw else None, headers=_headers(req))
        _record(rec, up.status_code, up.headers)
        return Response(up.content, status_code=up.status_code, headers={k: v for k, v in up.headers.items() if k in PASS_BACK})
    except httpx.HTTPError as e:
        _record(rec, 599, None)
        rec["error"] = f"{type(e).__name__}: {e}"[:300]
        return JSONResponse({"error": {"message": f"model proxy: upstream unreachable ({type(e).__name__})"}}, status_code=502)


@app.get("/_intervals")
async def intervals():
    return {"intervals": INTERVALS, "now": time.monotonic(), "now_wall": time.time()}


@app.post("/_reset")
async def reset():
    INTERVALS.clear()
    return {"ok": True}


@app.get("/_info")
async def info():
    return {"upstream": UPSTREAM, "key_set": bool(KEY), "requests": len(INTERVALS)}


@app.get("/")
async def root():
    return {"ok": True, "upstream": UPSTREAM}
