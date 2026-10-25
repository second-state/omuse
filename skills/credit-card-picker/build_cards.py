#!/usr/bin/env python3
"""Build the per-market card tables the credit-card-picker skill reads (cards/<market>.md) from cards/catalog.json,
and check the catalog on the way (duplicate cards, missing fields, unknown categories, stale verification dates).

    python3 skills/credit-card-picker/build_cards.py            # rebuild cards/*.md, exit 1 on a catalog problem
    python3 skills/credit-card-picker/build_cards.py --import ../personal_finance/crates/card_rewards/card_catalog.json
                                                                # merge that catalog's cards (by issuer + product) first

The JSON is the source of truth; never edit the .md files by hand. See KNOWLEDGE.md at the repository root."""
import datetime as dt
import json
import os
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
CATALOG = os.path.join(HERE, "cards", "catalog.json")
CATEGORIES = ["dining", "travel", "transit", "groceries", "gas", "online_retail", "streaming", "entertainment", "drugstore",
              "utilities", "insurance", "foreign", "other"]
STALE_DAYS = 400


def key(c):
    return (c["issuer"].strip().lower(), c["product"].strip().lower(), c.get("region", "").upper())


def trim(src):
    """Only the fields the skill needs, from a personal_finance catalog entry."""
    return {
        "issuer": src["issuer"], "product": src["product"], "aliases": src.get("aliases") or [],
        "region": src.get("region") or "US", "currency": src.get("currency") or "USD", "network": src.get("network") or "",
        "unit": src.get("unit") or "points", "point_value_cents": src.get("point_value_cents"),
        "annual_fee_cents": src.get("annual_fee_cents"), "fx_fee_pct": src.get("fx_fee_pct"),
        "rules": src.get("rules") or {}, "caps": src.get("caps") or {},
        "merchant_rules": [{"keywords": m.get("keywords") or [], "earn_rate": m.get("earn_rate"), "end_date": m.get("end_date"),
                            "notes": (m.get("notes") or "")[:160]} for m in (src.get("merchant_rules") or [])],
        "rule_notes": {k: (v or "").split(" Source")[0][:220] for k, v in (src.get("rule_notes") or {}).items()},
        "benefits": [(b.get("description") or b if isinstance(b, dict) else str(b))[:160] for b in (src.get("benefits") or [])][:6],
        "verified_at": src.get("verified_at") or "", "verified": bool(src.get("verified_at")),
        "source": src.get("source") or "",
    }


def load():
    return json.load(open(CATALOG, encoding="utf-8")) if os.path.exists(CATALOG) else {"as_of": "", "note": "", "cards": []}


def check(cat):
    problems, seen = [], set()
    for c in cat["cards"]:
        k = key(c)
        if k in seen:
            problems.append(f"duplicate card {c['issuer']} {c['product']} ({c.get('region')})")
        seen.add(k)
        for f in ("issuer", "product", "region", "currency", "unit", "rules"):
            if not c.get(f):
                problems.append(f"{c.get('issuer')} {c.get('product')}: missing {f}")
        for cat_name in c.get("rules", {}):
            if cat_name not in CATEGORIES:
                problems.append(f"{c['issuer']} {c['product']}: unknown category {cat_name}")
        if c.get("verified") and c.get("verified_at"):
            try:
                age = (dt.date.today() - dt.date.fromisoformat(c["verified_at"][:10])).days
                if age > STALE_DAYS:
                    problems.append(f"{c['issuer']} {c['product']}: verified {age} days ago — re-verify")
            except ValueError:
                problems.append(f"{c['issuer']} {c['product']}: bad verified_at {c['verified_at']}")
    return problems


def money(cents, cur):
    if cents is None:
        return "?"
    return f"{cur} {cents / 100:,.0f}"


def row(c):
    rates = ", ".join(f"{k} {v:g}x" for k, v in sorted(c["rules"].items(), key=lambda kv: -kv[1]) if k != "other")
    other = c["rules"].get("other")
    rates = (rates + (f", other {other:g}x" if other is not None else "")).strip(", ")
    caps = "; ".join(f"{k}: {v}" for k, v in (c.get("caps") or {}).items()) or "—"
    merch = "; ".join(f"{'/'.join(m['keywords'][:2])} {m['earn_rate']:g}x" + (f" until {m['end_date']}" if m.get("end_date") else "")
                      for m in c.get("merchant_rules", [])[:4]) or "—"
    pv = c.get("point_value_cents")
    unit = c["unit"] + (f" @ {pv:g}¢" if isinstance(pv, (int, float)) and c["unit"] != "cashback" else "")
    fx = f"{c['fx_fee_pct']:g}%" if isinstance(c.get("fx_fee_pct"), (int, float)) else "?"
    ver = c["verified_at"][:10] if c.get("verified") and c.get("verified_at") else "UNVERIFIED (seed)"
    name = c["product"] + (f" ({', '.join(c['aliases'][:2])})" if c.get("aliases") else "")
    return f"| {c['issuer']} | {name} | {c.get('network') or '?'} | {unit} | {money(c.get('annual_fee_cents'), c['currency'])} | {fx} | {rates or '—'} | {caps} | {merch} | {ver} |"


def build(cat):
    by = {}
    for c in cat["cards"]:
        by.setdefault(c["region"].upper(), []).append(c)
    for region, cards in by.items():
        cards.sort(key=lambda c: (c["issuer"].lower(), c["product"].lower()))
        lines = [f"# Credit cards — {region} ({len(cards)} cards; catalog as of {cat.get('as_of') or '?'})", "",
                 "Earn rates are per canonical category (dining, travel, transit, groceries, gas, online_retail, streaming, "
                 "entertainment, drugstore, utilities, insurance, foreign, other); caps and merchant rules override them; "
                 "a row marked UNVERIFIED is a seed from general knowledge — verify on the issuer's page before relying on it. "
                 "Point values are cents per point at a plain redemption. Generated by build_cards.py from cards/catalog.json — do not edit.", "",
                 "| Issuer | Card (aliases) | Network | Unit | Annual fee | FX fee | Earn rates | Caps | Merchant rules | Verified |",
                 "|---|---|---|---|---|---|---|---|---|---|"]
        lines += [row(c) for c in cards]
        notes = []
        for c in cards:
            rn = {k: v for k, v in (c.get("rule_notes") or {}).items() if v}
            if rn or c.get("benefits"):
                notes.append(f"- **{c['issuer']} {c['product']}**: " + " ".join(f"{k}: {v}" for k, v in list(rn.items())[:3])
                             + (" Benefits: " + "; ".join(c["benefits"][:3]) if c.get("benefits") else ""))
        if notes:
            lines += ["", "## Notes (conditions that change the rate)", ""] + notes
        open(os.path.join(HERE, "cards", f"{region.lower()}.md"), "w", encoding="utf-8").write("\n".join(lines) + "\n")
        print(f"cards/{region.lower()}.md: {len(cards)} cards")


def main():
    cat = load()
    if "--import" in sys.argv:
        src = json.load(open(sys.argv[sys.argv.index("--import") + 1], encoding="utf-8"))
        have = {key(c): i for i, c in enumerate(cat["cards"])}
        n_new = n_upd = 0
        for s in src["cards"]:
            t = trim(s)
            k = key(t)
            if k in have:
                cat["cards"][have[k]] = t
                n_upd += 1
            else:
                cat["cards"].append(t)
                n_new += 1
        cat["as_of"] = src.get("as_of") or cat.get("as_of")
        cat["note"] = "Imported from personal_finance card_catalog.json and hand-maintained; see KNOWLEDGE.md."
        json.dump(cat, open(CATALOG, "w", encoding="utf-8"), ensure_ascii=False, indent=1)
        print(f"imported: {n_new} new, {n_upd} updated")
    problems = check(cat)
    for p in problems:
        print("PROBLEM", p)
    build(cat)
    sys.exit(1 if [p for p in problems if "re-verify" not in p] else 0)


if __name__ == "__main__":
    main()
