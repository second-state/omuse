"""City guides with a real model (OMUSE_E2E_REAL=1, see travel_common.py): three questions a Western visitor or expat
asks. The model must pick the right guide by itself (load_skill), answer from it — exchange rates from market_data when
it needs one — and never open the browser (no browser_* call, sub-agents included); the answer must carry the guide's
facts.

    OMUSE_E2E_ENV_FILE=~/.config/omuse/e2e.env OMUSE_E2E_MODELS=alt tests/run_in_docker.sh city_guides_real"""
import json
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from tests import travel_common as T  # noqa: E402

if not T.REAL:
    print("city_guides_real needs a real model (OMUSE_E2E_REAL=1)")
    sys.exit(1)

CASES = [
    ("shanghai", "I'm moving to Shanghai for work next month with my wife and two kids. Which neighbourhoods do expat "
                 "families usually live in, and roughly what does a family flat rent for?",
     ["Pudong", "Jinqiao", "Gubei", "Former French Concession", "Minhang", "Hongqiao"], ["¥", "RMB", "yuan", "CNY"]),
    ("tokyo", "First time in Tokyo next week, flying in from the US to Haneda. How do I get to Shinjuku, and will I "
              "need cash or can I use my credit card everywhere?",
     ["Keikyu", "Monorail", "Limousine", "Shinagawa", "Hamamatsucho"], ["cash", "Suica", "PASMO"]),
    # (visa questions belong to entry-requirements, which reads the official page on purpose)
    ("hanoi", "I'm an American spending a week in Hanoi. Which area should I stay in, how do I get around the city, "
              "and how much is 300 US dollars in dong right now?",
     ["Hoàn Kiếm", "Hoan Kiem", "Old Quarter", "Tây Hồ", "Tay Ho", "Ba Đình"], ["₫", "VND", "dong"]),
]

T.setup()
tasks_all = []
for skill, prompt, places, money in CASES:
    info = T.run(prompt, timeout=600)
    tasks = info["tasks"]
    tasks_all += tasks
    final = T.task(tasks[-1])
    ev = [e for tid in tasks for e in T.task(tid).get("events", [])]
    calls = [e["data"] for e in ev if e["type"] == "tool_call"]
    names = [c["name"] for c in calls]
    for c in calls:
        print(f"INFO {skill} call {c['name']} {json.dumps(c.get('args'), ensure_ascii=False)[:140]}")
    res = str(final.get("result") or "")
    print(f"INFO {skill} answer: {res[:600]!r}")
    loaded = [c for c in calls if c["name"] == "load_skill"]
    T.check(f"{skill}: the task completed", final.get("status") == "COMPLETED", (final.get("status"), final.get("error")))
    T.check(f"{skill}: the model loaded the {skill} guide by itself", any((c.get("args") or {}).get("name") == skill for c in loaded),
            [c.get("args") for c in loaded])
    T.check(f"{skill}: no browser at all (sub-agents included)", not any(n.startswith("browser_") for n in names), names)
    T.check(f"{skill}: the answer carries the guide's facts", any(p in res for p in places) and any(m in res for m in money), res[:500])
    if skill == "hanoi":
        T.check("hanoi: the exchange rate came from market_data", "market_data" in names, names)
    T.timing(f"city_guides_real/{skill}", info, len(names), sum(1 for n in names if n.startswith("browser_")))
os.makedirs(os.path.dirname(T.LAST), exist_ok=True)
json.dump({"suite": "city_guides_real", "tasks": tasks_all}, open(T.LAST, "w"))
T.finish()
