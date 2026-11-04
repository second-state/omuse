"""Skills page: every skill is on by default; the user turns some off (they leave the agent's instructions and
load_skill refuses them); only a GitHub link is accepted for an import; built-in skills cannot be deleted."""
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


def run(msg, bot="main"):
    r = c.post(B + "/api/chat", json={"message": msg, "bot": bot}, headers=H).json()
    t0 = time.time()
    while time.time() - t0 < 60:
        t = c.get(f"{B}/api/tasks/{r['task_id']}").json()
        if t["status"] in ("COMPLETED", "FAILED", "CANCELLED"):
            return t.get("result") or ""
        time.sleep(0.5)
    return ""


sk = c.get(B + "/api/skills").json()
names = [s["name"] for s in sk["skills"]]
check("built-in skills listed, all enabled by default", len(names) >= 9 and all(s["enabled"] and s["source"] == "builtin" for s in sk["skills"]), sk)
check("no OMUSE_SKILLS in the test stack", sk["env"] == "")
# each bot's instructions list the enabled skills of that bot (crate::bots); together the bots cover every skill
bots = {b["id"]: set(b["skills"]) for b in c.get(B + "/api/bots", headers=H).json()["bots"]}
check("together the bots have every enabled skill", set().union(*bots.values()) == set(names), sorted(set(names) - set().union(*bots.values())))
byname = {k["name"]: k.get("bots") for k in sk["skills"]}
check("each skill names the bots that may use it (the Skills page groups by them)", byname.get("tokyo") == ["travel"]
      and byname.get("pdf-forms") == ["main", "travel", "money"] and byname.get("news-search") == ["main"]
      and all(byname[n] for n in names), {n: byname.get(n) for n in ("tokyo", "pdf-forms", "news-search")})
listed = run("SKILLCHECK")
check("Main's instructions list Main's enabled skills", listed.startswith("SKILLS=") and set(listed[7:].split(",")) == bots["main"], (listed, sorted(bots["main"])))
listed = run("SKILLCHECK", bot="travel")
check("the Travel bot's instructions list its own", listed.startswith("SKILLS=") and set(listed[7:].split(",")) == bots["travel"], (listed, sorted(bots["travel"])))

# turn two off
off = ["online-shopping", "phone-call"]
r = c.put(B + "/api/skills", json={"disabled": off}, headers=H).json()
check("selection saved", [s["name"] for s in r["skills"] if not s["enabled"]] == off, r)
listed = run("SKILLCHECK")
check("disabled skills leave the agent's instructions", listed.startswith("SKILLS=") and not set(off) & set(listed[7:].split(",")) and "reply-email" in listed, listed)
out = run("LOADSKILL online-shopping", bot="shopping")   # the Shopping bot's skill
check("load_skill refuses a disabled skill", "turned off" in out, out)
out = run("LOADSKILL reply-email")
check("load_skill still works for an enabled one", "LOADED=---" in out and "reply-email" in out, out)

# import: only GitHub links (the import itself went with the fake GitHub server)
r = c.post(B + "/api/skills/import", json={"url": "https://gitlab.com/acme/skills"}, headers=H)
check("non-GitHub links are refused", r.status_code == 400, r.text[:200])
check("built-in skills cannot be deleted", c.delete(B + "/api/skills/reply-email", headers=H).status_code == 404)
aud = c.get(B + "/sentinel/api/audit", params={"limit": 400, "actor": "user"}).text
check("skills changes are audited", "skills.select" in aud)
c.put(B + "/api/skills", json={"disabled": []}, headers=H)
print("ALL PASS" if not fails else f"{len(fails)} FAILED: {fails}")
sys.exit(1 if fails else 0)
