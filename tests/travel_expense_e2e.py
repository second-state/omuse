"""travel-expense-report skill: a photographed receipt is read by the vision model (file_look → JSON), the card statement
by data_query, the hotel receipt from mail, the total by calculate, and the Excel ledger plus the PDF claim are produced
and sent to the chat — no browser, no invented figure."""
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from tests import skills_common as K  # noqa: E402
K.BOT = "travel"   # the Travel bot has this suite's skills (crate::bots)

MAIL = [
    ("Agoda <noreply@agoda.com>", "Your Agoda receipt — Hotel Ginza Grand",
     "Booking 55120. Hotel Ginza Grand, 20-23 Nov 2026. Total charged: JPY 78,000 to card ending 1111 on 2026-11-21.", 4),
]
K.setup(mail=MAIL)
receipt = K.upload("receipt.png", K.PNG)
statement = K.upload("statement.csv", b"date,description,amount,currency\n2026-11-20,SUSHI HOSHIZORA TOKYO,18000,JPY\n"
                                       b"2026-11-21,AGODA HOTEL GINZA,78000,JPY\n2026-11-21,FOREIGN TX FEE,2.10,SGD\n")
atts = [receipt.get("attachment") or receipt, statement.get("attachment") or statement]
t = K.run("EXPENSES build the expense report for my Tokyo trip, 20-23 Nov 2026, in JPY (home currency SGD). "
          "The restaurant receipt photo and the card statement are attached.", attachments=atts)
K.trace(t)
names = K.names(t)
K.check("task completes", t["status"] == "COMPLETED", (t["status"], t.get("error")))
K.check("the travel-expense-report skill was loaded", K.loaded(t, "travel-expense-report"), names)
fl = [x for x in K.calls(t) if x["name"] == "file_look"]
K.check("the receipt photo was read by the vision model, asking for a JSON record",
        fl and "uploads/" in str(fl[0]["args"].get("path")) and "JSON" in str(fl[0]["args"].get("question")), fl)
K.check("the vision model returned the structured receipt", "Sushi Hoshizora" in str(K.results(t, "file_look")) and "18000" in str(K.results(t, "file_look")),
        K.results(t, "file_look"))
dq = K.results(t, "data_query")
K.check("the statement CSV was read with data_query (columns seen)", dq and dq[0].get("ok") and "description" in str(dq[0].get("preview")), dq)
K.check("the hotel receipt came from mail", "gmail_find_receipts" in names and "78" in str(K.results(t, "gmail_find_receipts")), K.results(t, "gmail_find_receipts"))
K.check("the total was computed with calculate", "calculate" in names and "96000" in str(K.results(t, "calculate")), K.results(t, "calculate"))
K.check("Excel ledger and PDF claim were made locally and sent", all(n in names for n in ("make_xlsx", "make_pdf", "send_file")), names)
K.check("no browser, no online converter", not any(n.startswith("browser_") for n in names), names)
cards = K.conv_cards(t["_conv"], "files") + K.conv_cards(t["_conv"], "file")
items = [i for c in cards for i in (c.get("items") or [c])]
K.check("both files are in the chat", any(str(i.get("name", i.get("path"))).endswith(".xlsx") for i in items)
        and any(str(i.get("name", i.get("path"))).endswith(".pdf") for i in items), cards)
for i in items:
    r = K.c.get(K.B + "/api/files/raw", params={"path": i.get("path")})
    K.check(f"file served: {i.get('name') or i.get('path')}", r.status_code == 200 and len(r.content) > 500, r.status_code)
res = t.get("result") or ""
K.check("the answer carries the summary table and the grand total", "96,000" in res and "JPY" in res and "Sushi Hoshizora" in res, res[:600])
K.check("every figure in the answer appears in a tool result", all(x in "\n".join(str(r.get("preview")) for r in [d for e in t["events"] if e["type"] == "tool_result" for d in [e["data"]]])
        for x in ("18000", "78000", "96000")), res[:300])
K.finish()
