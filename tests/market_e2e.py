"""market_data / stock_fundamentals when Yahoo Finance's first host is rate-limited: the fake Yahoo of tests/fake_apps.py
answers 429 on host 1 for every request, so quotes, history and valuations must come from host 2 (valuations through its
crumb flow). Scripted because a real run cannot produce or see this: the outage is injected, and which host answered is
invisible to the user. Everything a user can see — quotes, a year's moves, a chart, an unknown ticker, fundamentals, the
mortgage maths, a survey's NPS — is tests/market_real.py's (real model, from the user's side)."""
import sys
import time

import httpx

B = "http://127.0.0.1:8080"
H = {"X-Persona-UI": "1"}
c = httpx.Client(timeout=60, trust_env=False)
fails = []


def check(name, cond, info=""):
    print(("PASS " if cond else "FAIL ") + name, "" if cond else str(info)[:1200])
    if not cond:
        fails.append(name)


def run(msg, timeout=120):
    r = c.post(B + "/api/chat", json={"message": msg}, headers=H).json()
    t0 = time.time()
    while time.time() - t0 < timeout:
        t = c.get(f"{B}/api/tasks/{r['task_id']}").json()
        if t["status"] in ("COMPLETED", "FAILED", "CANCELLED"):
            return t
        time.sleep(0.5)
    return t


t = run("MARKETTEST 腾讯和汇丰的走势")
out = t.get("result") or ""
check("quotes task completes with host 1 rate-limited", t["status"] == "COMPLETED", (t["status"], t.get("error")))
check("quote and history served by host 2 (host 1 answers 429)", "0700.HK · Tencent Holdings Limited" in out and "closes:" in out
      and out.count(", 20") > 20, out[:600])
check("name lookup served by host 2", "0005.HK · HSBC Holdings plc" in out, out[:900])

t = run("FUNDTEST 特斯拉和比亚迪的估值和财报")
out = t.get("result") or ""
check("fundamentals task completes with host 1 rate-limited", t["status"] == "COMPLETED", (t["status"], t.get("error")))
check("valuations via host 2's crumb flow", "TSLA · Tesla, Inc." in out and "市盈率 P/E (TTM) 334.07" in out
      and "市值 market cap 1.40T USD" in out, out[:900])
check("financials via host 2", "季度财报 quarterly" in out and "2026-06-30" in out and "年度财报 annual" in out, out[:2500])
print("ALL PASS" if not fails else f"{len(fails)} FAILED: {fails}")
sys.exit(1 if fails else 0)
