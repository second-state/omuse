"""payments-connectivity-asia skill: exchange rates from market data (never from memory), no browser for general
advice, and a per-country checklist that names the local wallet and transit card."""
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from tests import skills_common as K  # noqa: E402

K.setup()
t = K.run("PAYASIA I'm going to Tokyo and Shanghai next month — how should I pay and get online? I think in SGD.")
K.trace(t)
names = K.names(t)
K.check("task completes", t["status"] == "COMPLETED", (t["status"], t.get("error")))
K.check("the payments-connectivity-asia skill was loaded", K.loaded(t, "payments-connectivity-asia"), names)
md = K.results(t, "market_data")
K.check("the exchange rates came from market data", "market_data" in names and md and md[0].get("ok") and "SGD/JPY" in str(md[0].get("preview")), md)
K.check("no browser for general advice", not any(n.startswith("browser_") for n in names), names)
res = t.get("result") or ""
import re  # noqa: E402
got = re.search(r"SGD/JPY[^0-9]*([0-9]+\.[0-9]+)", str(md[0].get("preview")) if md else "")
K.check("the rate in the answer is the one market data returned", got and f"1 SGD = {got.group(1)} JPY" in res, (got.group(1) if got else None, res[:300]))
K.check("Japan: transit card and cash advice; China: Alipay / WeChat Pay set-up before departure; DCC warning",
        "Suica" in res and "Alipay" in res and "before" in res and "dynamic currency conversion" in res, res[:800])
K.check("no approval was needed", not t["_approvals"], t["_approvals"])
K.finish()
