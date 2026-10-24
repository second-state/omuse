"""HTTP micro-benchmark of a running stack (run_local.sh): latency and throughput of the hot UI/API paths.

    python3 tests/perf_bench.py [base-url] [requests-per-endpoint] [concurrency]

Prints p50/p95/p99 latency and requests/second per endpoint plus a JSON summary line. Used to compare the Python
original with the Rust port under identical conditions (same container image family, same fake services).
"""
import asyncio
import json
import statistics
import sys
import time

import httpx

B = (sys.argv[1] if len(sys.argv) > 1 else "http://127.0.0.1:8080").rstrip("/")
N = int(sys.argv[2]) if len(sys.argv) > 2 else 400
CONC = int(sys.argv[3]) if len(sys.argv) > 3 else 8
H = {"X-Persona-UI": "1"}


async def bench(client, method, path, **kw):
    lat = []
    sem = asyncio.Semaphore(CONC)
    errors = 0

    async def one():
        nonlocal errors
        async with sem:
            t0 = time.perf_counter()
            try:
                r = await client.request(method, B + path, **kw)
                if r.status_code >= 400:
                    errors += 1
            except Exception:
                errors += 1
            lat.append(time.perf_counter() - t0)

    t0 = time.perf_counter()
    await asyncio.gather(*[one() for _ in range(N)])
    wall = time.perf_counter() - t0
    lat.sort()
    q = lambda p: lat[min(len(lat) - 1, int(p * len(lat)))] * 1000
    return {"path": path, "n": N, "errors": errors, "rps": round(N / wall, 1), "p50_ms": round(q(0.5), 2),
            "p95_ms": round(q(0.95), 2), "p99_ms": round(q(0.99), 2), "mean_ms": round(statistics.mean(lat) * 1000, 2)}


async def main():
    async with httpx.AsyncClient(timeout=60, trust_env=False, headers=H) as client:
        # warm up and create one task so task/conversation reads have data
        r = (await client.post(B + "/api/chat", json={"message": "你好"})).json()
        tid = r["task_id"]
        for _ in range(60):
            t = (await client.get(f"{B}/api/tasks/{tid}")).json()
            if t["status"] in ("COMPLETED", "FAILED"):
                break
            await asyncio.sleep(0.25)
        cases = [("GET", "/sentinel/api/health", {}), ("GET", "/", {}), ("GET", "/static/app.js", {"headers": {"Accept-Encoding": "gzip"}}),
                 ("GET", "/api/settings", {}), ("GET", f"/api/tasks/{tid}", {}), ("GET", "/api/tasks", {}),
                 ("GET", "/api/conversations", {}), ("GET", "/sentinel/api/approvals?status=pending", {}),
                 ("GET", "/sentinel/api/audit?limit=100", {}), ("GET", "/api/memory", {}), ("GET", "/api/schedules", {}),
                 ("GET", "/sentinel/api/connections", {}), ("GET", "/sentinel/api/browser/state", {})]
        out = []
        for m, p, kw in cases:
            res = await bench(client, m, p, **kw)
            out.append(res)
            print(f"{p:45s} rps={res['rps']:8.1f}  p50={res['p50_ms']:7.2f}ms  p95={res['p95_ms']:7.2f}ms  p99={res['p99_ms']:7.2f}ms  err={res['errors']}")
        # end-to-end: plain chat tasks (fake model) — measures the agent loop + proxy + DB, not the model
        t0 = time.perf_counter()
        tids = [(await client.post(B + "/api/chat", json={"message": "你好"})).json()["task_id"] for _ in range(10)]
        pending = set(tids)
        while pending and time.perf_counter() - t0 < 120:
            for t in list(pending):
                st = (await client.get(f"{B}/api/tasks/{t}")).json()["status"]
                if st in ("COMPLETED", "FAILED"):
                    pending.discard(t)
            await asyncio.sleep(0.1)
        chat = round(time.perf_counter() - t0, 2)
        print(f"10 plain chat tasks end-to-end: {chat}s")
        print("SUMMARY " + json.dumps({"endpoints": out, "chat10_s": chat}))


asyncio.run(main())
