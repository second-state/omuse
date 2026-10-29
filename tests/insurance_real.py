"""FWD travel insurance with a real model (tests/real_common.py): a plain request to Main to buy FWD travel insurance, the
trip in mail. FWD Singapore's site is the stand-in served inside the test container at its real address
(tests/https_standins.py: https://www.fwd.com.sg, tests/pages/fwd_sg_*.html). Main must
hand it to the Travel insurance bot, which loads fwd-travel-insurance, takes the trip from mail with the profile, gets
the quote, compares the plans, confirms the purchase (purchase_confirm with the Vault card), fills the application —
card and passport only through the Vault — and hands over for Pay, never pressing it; after the user hands the
browser back (without paying), the answer compares the plans with their premiums. If the bot asks for a missing detail
before opening the quote page, one follow-up answers it in the same conversation. One TIMING line per request.

Not in run_in_docker.sh's default list (it needs a real model, billed):
    OMUSE_E2E_ENV_FILE=~/.config/omuse/e2e.env tests/run_in_docker.sh insurance_real"""
import datetime as dt
import json
import re
import os
import sys
import time

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from tests import real_common as R  # noqa: E402
from tests import skills_common as K  # noqa: E402

D1 = (dt.date.today() + dt.timedelta(days=24)).isoformat()
D2 = (dt.date.today() + dt.timedelta(days=31)).isoformat()
PASSPORT_NO, CARD_NO = "K1234567Z", ("4111 1111 1111 1111", "4111111111111111")
MAIL = [("Singapore Airlines <noreply@singaporeair.example>", f"Booking confirmed — SQ 636 Singapore to Tokyo, {D1}",
         f"Passenger TAN WEI MING. SQ 636 SIN 10:30 → HND 18:25 on {D1}; SQ 637 HND → SIN on {D2}. Booking reference 7XKQ2P.", 5)]


def events(info: dict) -> list[dict]:
    return [e for x in info["tasks"] for e in R.task(x).get("events", [])]


def typed(info: dict) -> str:
    return " | ".join(str((x.get("args") or {}).get("text")) for x in info["calls"] if x["name"] == "browser_type")


R.setup(mail=MAIL, vault=False, profile={"name_en": "TAN WEI MING", "birthday": "1985-04-12", "email_personal": "weiming@example.com",
                                         "phone": "+65 9123 4567", "address_home": "10 Anson Road, Singapore 079903"})
CARD = K.vault_item("Visa Wei Ming", "card", {"number": CARD_NO[0], "expiry": "12/29", "cvc": "123", "holder": "TAN WEI MING"},
                    domains="fwd.com.sg")
PASSPORT = K.vault_item("Passport Wei Ming", "id_document", {"number": PASSPORT_NO, "name": "TAN WEI MING", "expiry": "2031-05-01",
                                                             "country": "SG"}, domains="fwd.com.sg")

first = R.ask("I need travel insurance from FWD for my trip next month, just me — the flight booking is in my email. "
              "Get me the cheapest plan with at least SGD 10,000 trip cancellation "
              "and fill in the application with my card from the Vault; I'll press Pay myself.", timeout=1500)
R.timing("fwd", first)
info = first
if not first["takeover"] and not [n for n in first["names"] if n.startswith("browser_")]:
    # the bot asked something before opening the quote page: answer once, in the same conversation, and let it go on
    R.routed("fwd question", first, "insurance")
    info = R.ask("Everything is in my email (the Singapore Airlines booking) and my profile (name, birthday, contact). "
                 "Only the flights are paid so far, no pre-existing conditions. Please go ahead.", conv=first["conv_id"], timeout=1500)
    R.timing("fwd follow-up", info)
    for k in ("calls", "names", "approvals", "tasks"):
        info[k] = first[k] + info[k]
R.routed("fwd", info, "insurance")
R.finished("fwd", info, takeover=True)
end = info["kid"] or info["main"]
R.check("fwd: the fwd-travel-insurance skill was loaded",
        any(x["name"] == "load_skill" and "fwd-travel-insurance" in json.dumps(x.get("args")) for x in info["calls"]), info["names"])
R.check("fwd: the trip looked up in mail", R.used(info, "gmail_search"), info["names"])
R.check("fwd: the quote page typed with the trip dates from mail", D1 in typed(info) and D2 in typed(info), typed(info))
pc = [a for a in info["approvals"] if a["tool"] == "purchase_confirm"]
R.check("fwd: purchase_confirm for the Business plan (SGD 47) with the Vault card", pc and abs(float((pc[0]["args"] or {}).get("total") or 0) - 47) < 0.01
        and (pc[0]["args"] or {}).get("card_item_id") == CARD, pc)
fills = [(x.get("args") or {}).get("item_id") for x in info["calls"] if x["name"] == "browser_fill_secret"]
R.check("fwd: card and passport filled only through the Vault, never typed", CARD in fills and PASSPORT in fills
        and PASSPORT_NO not in typed(info) and not any(n in typed(info).replace(" ", "") for n in ("4111111111111111", "1111111111")),
        (fills, typed(info)))


def pay_pressed() -> list:
    pages = [p.get("url", "") for p in R.c.get(R.B + "/sentinel/api/browser/state", headers=R.H).json().get("tasks", [])]
    done = [e["data"].get("name") for e in events(info) if e["type"] == "tool_result" and "fwd_sg_done" in str((e["data"] or {}).get("preview"))]
    print(f"INFO pages {pages}")
    return [u for u in pages if "fwd_sg_done" in u] + done


R.check("fwd: Pay never pressed (no page or tool result reached the done page)", not pay_pressed())
reason = str((end.get("waiting") or {}).get("reason") or "")
print(f"INFO takeover reason {reason[:300]!r}")
R.check("fwd: handed over for Pay (takeover), or the answer leaves Pay to the user",
        (info["takeover"] is not None and R.mentions(reason, "pay")) or (end.get("status") == "COMPLETED" and R.mentions(info["result"], "pay")),
        (end.get("status"), reason, info["result"][:400]))
final = info["result"]
if info["takeover"]:
    # the user looks at the application and hands the browser back without pressing Pay; the bot then answers
    K.hand_back()
    deadline = time.time() + 600
    while time.time() < deadline and R.task((info["kid"] or info["main"])["id"]).get("status") not in R.TERMINAL:
        R._approve(set(info["tasks"]), info["approvals"], info["excluded"])
        time.sleep(0.5)
    kid = R.task(info["kid"]["id"]) if info["kid"] else R.task(info["main"]["id"])
    final = str(kid.get("result") or "")
    print(f"INFO answer after the hand-back ({kid.get('status')}): {final[:900]!r}")
    R.check("fwd: after the hand-back the bot finished with an answer", kid.get("status") == "COMPLETED" and final.strip(),
            (kid.get("status"), kid.get("error")))
    R.check("fwd: still no Pay after the hand-back", not pay_pressed())
# the comparison: a present_choices card, the answer, or the bot's messages carry the plans with their premiums
choices = json.dumps([x.get("args") for x in info["calls"] if x["name"] == "present_choices"], ensure_ascii=False)
msgs = json.dumps(R.c.get(f"{R.B}/api/conversations/{info['kid_conv']}", headers=R.H).json().get("messages", []),
                  ensure_ascii=False) if info["kid_conv"] else ""
seen = [p for p in ("34", "47", "68") if re.search(rf"SGD\s*{p}(\.00)?\b", choices + final + msgs)]
R.check("fwd: the plans compared with their premiums from the page (at least two of three)", len(seen) >= 2, (seen, choices[:600]))
blob = final + str(info["main"].get("result") or "") + json.dumps(events(info), ensure_ascii=False) + msgs
R.check("fwd: no passport or card number in the answer or the trace", PASSPORT_NO not in blob and not any(n in blob for n in CARD_NO),
        [ln[:200] for ln in blob.splitlines() if PASSPORT_NO in ln or CARD_NO[0] in ln][:2])
R.finish()
