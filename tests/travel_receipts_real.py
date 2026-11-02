"""Photos on the road with a real model, from the user's point of view (tests/user_common.py, tests/travel2_common.py):
a menu read for an allergy and a trip's receipts turned into an expense report. The pictures and the statement are the
user's own, made by the test.

1. menu — the user has a severe peanut allergy and is in Tokyo. They photograph a ramen shop's menu, attach it with ＋
   and ask the Travel bot to check it, write a card in Japanese for the staff, and remember the allergy. Checked: the
   Memory page shows the allergy (as a profile suggestion to confirm, or saved).
2. expenses — a business trip to Tokyo two weeks ago. The user attaches the photo of a paper receipt and the card
   statement (CSV) and asks for the expense report in JPY. Checked: the chat offers an Excel file and a PDF. If the bot
   first asks a question, the user answers it once.
(What the model reads off the pictures is its wording, not checked.) No request asks the user to take over a browser.
One TIMING line per request.

    OMUSE_E2E_ENV_FILE=~/.config/omuse/e2e.env bash tests/run_in_docker.sh travel_receipts_real"""
import datetime as dt
import os
import re
import sys

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from tests import claims_common as C  # noqa: E402
from tests import travel2_common as T2  # noqa: E402
from tests import user_common as U  # noqa: E402
from tests.user_common import check  # noqa: E402

D = dt.date.today() - dt.timedelta(days=14)
D1, D3 = D + dt.timedelta(days=1), D + dt.timedelta(days=3)
HERE = "/tmp/claude-0/travel_receipts"
MENU = """<html><body style="font-family:'Noto Sans CJK JP',sans-serif;background:#f6efe1;margin:0;padding:22px;color:#2a1a0a">
<h1 style="text-align:center;margin:0 0 4px">麺処 さくら</h1><p style="text-align:center;margin:0 0 16px">お品書き</p>
<table style="width:100%;font-size:19px;border-collapse:collapse">
<tr><td>醤油ラーメン</td><td style="text-align:right">¥900</td></tr>
<tr><td style="font-size:14px;color:#664" colspan=2>鶏ガラ醤油スープ・チャーシュー・メンマ・ねぎ</td></tr>
<tr><td>味噌ラーメン</td><td style="text-align:right">¥950</td></tr>
<tr><td style="font-size:14px;color:#664" colspan=2>北海道味噌・もやし・コーン・バター</td></tr>
<tr><td>担々麺</td><td style="text-align:right">¥1,100</td></tr>
<tr><td style="font-size:14px;color:#664" colspan=2>ごまだれ・ラー油・ひき肉・砕き落花生（ピーナッツ）入り</td></tr>
<tr><td>塩ラーメン</td><td style="text-align:right">¥900</td></tr>
<tr><td style="font-size:14px;color:#664" colspan=2>あっさり塩スープ・鶏チャーシュー・柚子</td></tr>
<tr><td>焼き餃子（6個）</td><td style="text-align:right">¥450</td></tr>
<tr><td>杏仁豆腐</td><td style="text-align:right">¥400</td></tr>
</table><p style="font-size:13px;margin-top:16px">※アレルギーのある方はスタッフにお申し付けください。</p></body></html>"""
RECEIPT = f"""<html><body style="font-family:'Noto Sans CJK JP',monospace;background:#fff;margin:0;padding:20px;color:#111;font-size:16px">
<h2 style="text-align:center;margin:0">鮨 星空</h2><p style="text-align:center;margin:2px 0">Sushi Hoshizora</p>
<p style="text-align:center;margin:2px 0;font-size:13px">東京都中央区銀座4-2-8 TEL 03-0000-1234</p>
<p style="text-align:center;margin:10px 0">領収書 RECEIPT</p>
<p>{D1:%Y年%m月%d日} 21:12　No. 004417</p>
<table style="width:100%">
<tr><td>おまかせコース × 2</td><td style="text-align:right">¥15,000</td></tr>
<tr><td>日本酒 (1合)</td><td style="text-align:right">¥1,364</td></tr>
<tr><td>小計</td><td style="text-align:right">¥16,364</td></tr>
<tr><td>消費税 (10%)</td><td style="text-align:right">¥1,636</td></tr>
<tr><td><b>合計</b></td><td style="text-align:right"><b>¥18,000</b></td></tr>
<tr><td>VISA ****1111</td><td style="text-align:right">¥18,000</td></tr>
</table><p style="text-align:center;font-size:13px">ありがとうございました</p></body></html>"""
STATEMENT = ("date,description,amount,currency\n"
             f"{D1.isoformat()},SUSHI HOSHIZORA TOKYO,18000,JPY\n"
             f"{D1.isoformat()},AGODA HOTEL GINZA GRAND,78000,JPY\n")


async def menu(u: U.User) -> None:
    pic = await T2.photo(u, MENU, os.path.join(HERE, "menu.png"))
    await u.open_bot("Travel")
    await T2.attach(u, [pic])
    a = await u.ask("I have a severe peanut allergy and I'm at this ramen place in Tokyo. Can you check the menu in the photo for me, and "
                    "write me a card in Japanese I can show the staff? Please remember my allergy for next time.",
                    label="menu", new_chat=False)
    T2.answered("menu", a)
    mem = await C.page_text(u, "memory")
    print(f"INFO Memory page: {' '.join(mem.split())[:900]}")
    check("menu: the Memory page shows the peanut allergy (to confirm or saved)", re.search(r"peanut|ピーナッツ|落花生", mem, re.I), mem[:900])


async def expenses(u: U.User) -> None:
    pic = await T2.photo(u, RECEIPT, os.path.join(HERE, "receipt-hoshizora.png"), width=420)
    csv = os.path.join(HERE, "card-statement.csv")
    open(csv, "w").write(STATEMENT)
    await u.open_bot("Travel")
    await T2.attach(u, [pic, csv])
    ask = (f"Please build the expense report for my Tokyo business trip, {T2.day(D)} to {T2.day(D3)}, in JPY. Purpose: client meetings; "
           "no company policy to apply. The restaurant receipt photo and my card statement are attached (the hotel was JPY 78,000 through Agoda).")
    a = await u.ask(ask, label="expenses", new_chat=False, timeout=1200)
    if not a.files and not a.timed_out:   # no file yet: the bot asked first (state, not its words)
        a = await u.ask("That's everything — go ahead with what you have and give me the Excel and the PDF.", label="expenses go", new_chat=False,
                        timeout=1200)
    T2.answered("expenses", a)
    # every file the chat's file cards let the user download (a card may carry several)
    files = await u.page.evaluate("() => [...document.querySelectorAll('.bubble.filecard a[href*=\"download=1\"]')]"
                                  ".map(x => ({name: x.getAttribute('download') || x.textContent, href: x.href}))")
    print(f"INFO expenses downloads: {files}")
    xl = [x for x in files if re.search(r"\.xlsx\b", x["name"] + x["href"], re.I)]
    pdf = [x for x in files if re.search(r"\.pdf\b", x["name"] + x["href"], re.I)]
    check("expenses: the chat offers an Excel file and a PDF", xl and pdf, files)


async def main():
    async with U.User() as u:
        await menu(u)
        await expenses(u)


U.setup(profile={"name_en": "LUCAS LU", "email_work": "lucas@acme.example", "company": "Acme Pte Ltd"})
U.run(main)
