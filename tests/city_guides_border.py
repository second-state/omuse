"""python3 tests/city_guides_border.py — write each city guide's "## Arrival card and tax" section from OMuse's code tables (driver.rs CARDS, tax_refund.rs
SCHEMES, apdt.rs HOME), so the guides say what the tools answer. Run from the repo root; re-running replaces the section."""
import re
import sys

ROOT = sys.argv[1] if len(sys.argv) > 1 else "."
drv = open(f"{ROOT}/crates/omuse-runtime/src/agent/driver.rs").read()
tax = open(f"{ROOT}/crates/omuse-runtime/src/agent/tax_refund.rs").read()
apdt = open(f"{ROOT}/crates/omuse-runtime/src/agent/apdt.rs").read()


def field(block, name):
    m = re.search(rf'\b{name}: "((?:[^"\\]|\\.)*)"', block)
    return m.group(1).replace('\\"', '"') if m else ""


cards = {}
for block in drv.split("    CardSpec {")[1:]:
    block = block.split("\n    },")[0]
    w = re.search(r"window_days: (\d+)", block)
    cards[field(block, "country")] = {"skill": field(block, "skill"), "name": field(block, "name"),
                                      "url": field(block, "start_url"), "window": int(w.group(1)) if w else 0}
schemes = {}
body = tax.split("pub static SCHEMES")[1].split("\n];")[0]
for block in body.split("    Scheme {")[1:]:
    links = re.findall(r'\("((?:[^"\\]|\\.)*)", "((?:[^"\\]|\\.)*)"\)', block)
    schemes[field(block, "iso")] = {"country": field(block, "country"), "scheme": field(block, "scheme"),
                                    "running": "running: true" in block, "who": field(block, "who"),
                                    "minimum": field(block, "minimum"), "time_limit": field(block, "time_limit"),
                                    "links": links}
APDT_HOME = re.search(r'const HOME: &str = "([^"]+)"', apdt).group(1)
CHECKED = re.search(r'pub const CHECKED: &str = "([^"]+)"', tax).group(1)

CITIES = {
    "bangkok": ("Thailand", "TH"), "beijing": ("China", "CN"), "chengdu": ("China", "CN"),
    "guangzhou": ("China", "CN"), "hanoi": ("Vietnam", "VN"), "ho-chi-minh-city": ("Vietnam", "VN"),
    "hong-kong": ("Hong Kong", "HK"), "kuala-lumpur": ("Malaysia", "MY"), "osaka": ("Japan", "JP"),
    "seoul": ("Korea", "KR"), "shanghai": ("China", "CN"), "shenzhen": ("China", "CN"),
    "singapore": ("Singapore", "SG"), "taipei": ("Taiwan", "TW"), "tokyo": ("Japan", "JP"),
}
START = "## Arrival card and tax"


def section(city: str) -> str:
    country, iso = CITIES[city]
    out = [START, "",
           f"From OMuse's own tables (the same facts `arrival_card_fill` and `tax_refund_rules` answer with; tax table "
           f"checked {CHECKED}). Asked how, give the requirement and the official link; file only when the user asks.", ""]
    c = cards.get(country)
    if c:
        # the wording of `driver::card_info` (what the arrival-card tool answers a question with)
        when = (f"within {c['window']} days before you arrive" if c["window"] else "any time before you arrive")
        out.append(f"- **Arrival card — what to file**: {c['name']} ({country}). **When**: {when}. **You need**: your "
                   f"passport, the flight that arrives and its date, where you stay (the hotel's name and address) and the "
                   f"day you leave. **Official site** (free): {c['url']}. OMuse fills it in when asked (`{c['skill']}`).")
    else:
        out.append(f"- **Arrival card**: none to file before arriving in {country} (no card in OMuse's arrival-card "
                   f"table); visas and entry rules: `entry-requirements`.")
    s = schemes.get(iso)
    if s:
        links = "; ".join(f"{label}: {url}" for label, url in s["links"][:2])
        if s["running"]:
            out.append(f"- **Shopping tax refund (visitors)** — {s['scheme']}. Who: {s['who']}. Minimum: {s['minimum']}. "
                       f"Time limit: {s['time_limit']}. Official: {links}. The steps and what to carry: the Money bot "
                       f"(`tax-refund-shopping`).")
        else:
            out.append(f"- **Shopping tax refund (visitors)** — {s['scheme']}." + (f" Official: {links}." if links else ""))
    if iso == "HK":
        out.append(f"- **Airport departure tax refund (APDT)**: a passenger who arrived in Hong Kong and flew out on the "
                   f"same or the next calendar day can get the HK$120 / HK$200 Air Passenger Departure Tax on the ticket "
                   f"back; apply within 28 days after the departure flight on the official platform {APDT_HOME} — the "
                   f"Money bot (`hk-apdt-refund`) gives the rule and the deadline, and applies when asked.")
    return "\n".join(out) + "\n"


def write(city: str) -> None:
    p = f"{ROOT}/skills/{city}/SKILL.md"
    s = open(p).read()
    sec = section(city)
    if START in s:
        i = s.index(START)
        j = s.find("\n## ", i + len(START))
        s = s[:i] + sec + ("\n" + s[j + 1:] if j != -1 else "")
    else:
        anchor = "## Local events"
        assert anchor in s, city
        s = s.replace(anchor, sec + "\n" + anchor, 1)
    open(p, "w").write(s)


for city in CITIES:
    write(city)
print("cards", sorted(cards), "schemes", sorted(schemes))
