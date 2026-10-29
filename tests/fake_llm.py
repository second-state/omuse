"""Scripted OpenAI-compatible server for integration tests.

Behaviour is chosen from the latest user goal text:
 - planner calls (system prompt contains 'planning module') -> fixed JSON plan; the planner only runs when the executor
   asks (plan_task) or loads a skill marked `plan: on` — a plain answer or an ordinary skill makes no plan
 - 'SEND' goal  -> calls gmail_send, then final answer
 - 'BROWSE' goal -> browser_navigate to TEST_PAGE, click the Submit button, final
 - 'INJECT' goal -> browser_navigate to injection page, then try gmail_send to attacker
 - 'REMEMBER' goal -> memory_remember then final
 - 'SCHEDULE' goal -> schedule_create then final
 - otherwise -> plain answer
"""
import datetime as _dtm
import json
import os
import re
import time
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


# ---- the Videos API (videogen_e2e.py): POST /videos starts a job, GET /videos/{id} polls it (done after two polls),
# GET /videos/{id}/content is the clip. Served twice: at /v1 (the model endpoint) and at /vid/v1 (a separate video
# provider, so a suite can check that the video endpoint and ITS key are used). Every call records its Authorization.
VIDEO_CALLS = []
VIDEO_JOBS = {}


def _mp4():
    # a tiny but well-formed MP4 header (ftyp box) — enough for a file card, a mime by extension and a download
    return b"\x00\x00\x00\x18ftypmp42\x00\x00\x00\x00mp42isom" + b"\x00\x00\x00\x08free"


def _video_app(prefix: str):
    from fastapi.responses import JSONResponse, Response

    @app.post(prefix + "/videos")
    async def videos_create(req: Request):
        b = await req.json()
        vid = "video_" + uuid.uuid4().hex[:10]
        VIDEO_CALLS.append({"url": "videos", "prefix": prefix, "body": b, "auth": req.headers.get("authorization") or ""})
        if not b.get("model"):
            return JSONResponse({"error": {"message": "Missing 'model' field"}}, status_code=400)
        if b.get("aspect_ratio"):
            return JSONResponse({"error": {"message": "Unknown parameter: 'aspect_ratio'."}}, status_code=400)
        if "storm" in str(b.get("prompt", "")).lower():   # a prompt the safety policy declines
            return JSONResponse({"error": {"code": "content_policy_violation", "message": "rejected as a result of our safety system"}},
                                status_code=400)
        VIDEO_JOBS[vid] = {"polls": 0, "seconds": b.get("seconds", "4"), "size": b.get("size", "1280x720")}
        return {"id": vid, "object": "video", "model": b["model"], "status": "queued", "progress": 0,
                "seconds": VIDEO_JOBS[vid]["seconds"], "size": VIDEO_JOBS[vid]["size"]}

    @app.get(prefix + "/videos/{vid}")
    def videos_get(vid: str, req: Request):
        VIDEO_CALLS.append({"url": "poll", "prefix": prefix, "id": vid, "auth": req.headers.get("authorization") or ""})
        j = VIDEO_JOBS.get(vid)
        if not j:
            return JSONResponse({"error": {"message": "no such video"}}, status_code=404)
        j["polls"] += 1
        st = "completed" if j["polls"] >= 2 else "in_progress"
        return {"id": vid, "object": "video", "status": st, "progress": 100 if st == "completed" else 50,
                "seconds": j["seconds"], "size": j["size"], "error": None}

    @app.get(prefix + "/videos/{vid}/content")
    def videos_content(vid: str, req: Request):
        VIDEO_CALLS.append({"url": "content", "prefix": prefix, "id": vid, "auth": req.headers.get("authorization") or ""})
        j = VIDEO_JOBS.get(vid)
        if not j or j["polls"] < 2:
            return JSONResponse({"error": {"code": "video_not_completed", "message": "not completed"}}, status_code=409)
        return Response(_mp4(), media_type="video/mp4")

    if prefix != "/v1":
        @app.get(prefix + "/models")
        def vid_models(req: Request):
            VIDEO_CALLS.append({"url": "models", "prefix": prefix, "auth": req.headers.get("authorization") or ""})
            return {"data": [{"id": "fake-chat"}, {"id": "sora-2"}, {"id": "fake-video-gen"}]}


_video_app("/v1")
_video_app("/vid/v1")


@app.get("/video_calls")
def video_calls():
    return VIDEO_CALLS


@app.get("/last_auth")
def last_auth():
    return {"authorization": AUTH[-1] if AUTH else None, "calls": len(AUTH)}


@app.post("/v1/chat/completions")
async def chat(req: Request):
    """The scripted reply; as `data:` chunks (content in small pieces, tool calls in two fragments, usage last) when the
    request says `stream: true` — slowly (50 ms a piece) for STREAMSLOW goals so a page can be watched filling up."""
    b = await req.json()
    out = await _chat(req, b)
    if isinstance(out, dict) and b.get("stream"):
        from fastapi.responses import StreamingResponse
        goal = "\n".join(str(m["content"]) for m in b.get("messages") or [] if m.get("role") == "user")
        return StreamingResponse(sse(out, 0.05 if "STREAMSLOW" in goal else 0.001), media_type="text/event-stream")
    return out


async def sse(out, delay):
    import asyncio
    msg = out["choices"][0]["message"]
    fr = out["choices"][0].get("finish_reason")

    def chunk(delta, finish=None):
        return "data: " + json.dumps({"id": "chatcmpl-fake", "object": "chat.completion.chunk", "choices": [
            {"index": 0, "delta": delta, "finish_reason": finish}]}, ensure_ascii=False) + "\n\n"
    yield chunk({"role": "assistant"})
    content = msg.get("content") or ""
    for i in range(0, len(content), 16):
        yield chunk({"content": content[i:i + 16]})
        await asyncio.sleep(delay)
    for i, t in enumerate(msg.get("tool_calls") or []):
        args = t["function"]["arguments"]
        half = len(args) // 2
        yield chunk({"tool_calls": [{"index": i, "id": t["id"], "type": "function", "function": {"name": t["function"]["name"], "arguments": args[:half]}}]})
        yield chunk({"tool_calls": [{"index": i, "function": {"arguments": args[half:]}}]})
    yield chunk({}, fr)
    yield "data: " + json.dumps({"id": "chatcmpl-fake", "object": "chat.completion.chunk", "choices": [], "usage": out.get("usage") or {}}) + "\n\n"
    yield "data: [DONE]\n\n"


async def _chat(req: Request, b: dict):
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
    if sys.startswith("## Route"):   # the main bot's router (agent/bots.rs `route`): main, unless bots_e2e asks
        return route_reply(str(msgs[-1]["content"]))
    # retry policy (retry_e2e.py): the first calls of a task fail, then it works
    goal_txt = "\n".join(str(m["content"]) for m in msgs if m["role"] == "user")   # the goal (the last user message is the status note)
    m = re.search(r"RETRY(429|503)x(\d)", goal_txt)
    if m and b.get("tools"):   # the executor's own calls (they carry the tool list; titles and memory extraction do not)
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
    if sys.startswith("You read an identity document"):   # /api/profile/ocr (profile_onboarding_ui.py)
        return reply(json.dumps({"document_type": "passport", "number": "E1234567X", "surname": "LU", "given_names": "LUCAS", "name": "LU LUCAS",
                                 "sex": "M", "date_of_birth": "1985-04-12", "nationality": "Australia", "expiry": "2031-05-01", "country": "AUS"}))
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
        if n == 0:   # a comparison of three candidates is multi-step work: the executor asks for a plan first
            return reply("", [tc("plan_task", {"objective": "Compare three hotels", "notes": "one sub-agent per hotel"})])
        if n == 1:
            if "PARALLELBATCH" in goal:
                return reply("", [tc("delegate", {"tasks": jobs})])
            return reply("", [tc("delegate", j) for j in jobs])
        # a summary, not the raw <subagent_report> tags (angle-bracket markup is stripped from answers)
        found = re.findall(r"SUBREPORT (Hotel\w+)", "\n".join(str(m["content"]) for m in tools_done))
        return reply("PARALLEL RESULT: " + ", ".join(found))
    bk = bots_flow(goal, sys, msgs, tools_done, n, allu)
    if bk is not None:
        return bk
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
    if "STREAMTEST" in goal or "STREAMSLOW" in goal:   # stream_e2e.py / stream_ui.py: a long answer after hidden reasoning
        if "STREAMCALL" in goal and n == 0:   # a turn with commentary AND a tool call first
            return reply("Let me look at the workspace first.", [tc("files_list", {"path": ""})])
        return reply("<think>the user wants a story; I will keep it short and warm</think>" + STREAM_STORY)
    if "fold test" in goal and n == 0:   # chat_fold_ui.py: a task with a plan, so the finished card has something to fold
        return reply("", [tc("plan_task", {"objective": "fold test"})])
    if "LANGCHECK" in goal:   # report what language the agent's instructions are in (and the planner's, via plan_task)
        if n == 0:
            return reply("", [tc("plan_task", {"objective": "LANGCHECK"})])
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
    if "WEBREAD" in goal:   # web_search then browser_read several pages in one call
        if n == 0:
            return reply("", [tc("web_search", {"query": "olares one"})])
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
        return reply("", [tc("web_search", {"query": f"anker power bank variant {n}"})])
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
            return reply("", [tc("plan_task", {"objective": "Triage this week's inbox and prepare replies"})])
        if n == 1:
            return reply("", [tc("notion_search", {"query": "plan"})])
        if n == 2:
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
            return reply("", [tc("plan_task", {"objective": "Share the Q4 plan with the team on Slack"})])
        if n == 1:
            return reply("", [tc("notion_get_page", {"page_id": "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee"})])
        if n == 2:
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
                        "tools": "\n".join(str(m.get("content")) for m in ms if m.get("role") == "tool"),
                        "stream": b.get("stream"), "stream_options": b.get("stream_options"), "has_tools": bool(b.get("tools"))})
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


def _book_at(text):
    """The first airline website in a flights result's book_at (Kiwi.com data has no link)."""
    m = re.search(r'book_at\\?"?: ?\[\\?"([a-z0-9.-]+)', text)
    return m.group(1) if m else "?"


_TRAVEL_SEEN = {}   # (kind, goal) -> the first search's parsed result, for turns after compression trimmed it


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
    # once the booking button was pressed, answer from the confirmation and the approved purchase summary — the first
    # search result may have been trimmed by context compression by then (a long skill sheet plus page snapshots)
    if has("browser_click"):
        after = "\n".join(r for n, _, r in calls if n in ("browser_click", "browser_snapshot"))
        m = re.search(r"TRV-[A-Z0-9]{7}", after)
        pc = next((a for n, a, _ in calls if n == "purchase_confirm"), None)
        if m:
            what = (pc or {}).get("items", [{}])[0].get("name", kind)
            total = f" total {(pc or {}).get('currency', '')} {(pc or {}).get('total', '')}" if pc else ""
            return reply(f"Booked. Booking reference: {m.group(0)}. {what}{total}. Confirmation goes to {TRAVELLER['email']}.")
        if not has("browser_snapshot"):
            return reply("", [tc("browser_snapshot", {})])
        return reply("TRAVEL FAIL: no booking reference after the click: " + after[-800:])
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
        # remember the offers: later in the task context compression may have trimmed the first search result
        if data.get("offers"):
            _TRAVEL_SEEN[(kind, goal)] = data
        data = data if data.get("offers") else _TRAVEL_SEEN.get((kind, goal), {})
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
        # the button from the newest page text (every browser call returns one; compression may have trimmed the
        # navigate result's tail, where the button is); none found → a fresh snapshot first
        pages = [r for nm, _, r in reversed(calls) if nm.startswith("browser_")]
        ref = next((x for x in (_snap_ref(pg, button, "button") for pg in pages) if x), None)
        if not ref:
            if not has("browser_snapshot"):
                return reply("", [tc("browser_snapshot", {})])
            return reply("TRAVEL FAIL: no " + button + " button on the booking page")
        return reply("", [tc("browser_click", {"ref": ref})])
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
STREAM_STORY = ("Once upon a time, in a small town by the sea, there lived a lighthouse keeper named Mara. " * 6 +
                "Every evening she climbed the one hundred and twelve steps, lit the lamp, and watched the fishing boats come home. "
                "One stormy night a boat did not return, and Mara kept the lamp burning until dawn, when the boat finally appeared on the horizon. "
                "The fishermen said the light had guided them home. THE END.")
SHOW_TRIP = "What's on my Tokyo trip this week? Check my mail and calendar."
SHOW_CITY = "We're moving to Shanghai for work with two kids. Where do expat families live, and what does rent cost?"
SHOW_SGAC = "Fill in my Singapore arrival card for Sunday's flight."


def fwd_insurance(goal, msgs):
    """fwd-travel-insurance: FWDBUY (Singapore quote → plan → application → hand-over for Pay), FWDCLAIM (Singapore
    delay claim on FWD's online form → hand-over for Submit), FWDHKCLAIM (bolttech's PDF claim form → email draft)."""
    import datetime as _dt
    calls = _travel_calls(msgs)
    names = [c[0] for c in calls]

    def has(name):
        return name in names

    def result(name):
        return next((r for n, _, r in reversed(calls) if n == name), "")

    def page():   # the newest page text (every browser call returns one)
        return next((r for n, _, r in reversed(calls) if n.startswith("browser_") and "[e" in r), "")

    inc = (re.search(r"INC=(20\d\d-\d\d-\d\d)", goal) or re.search(r"(20\d\d-\d\d-\d\d)", goal + " 2026-10-20")).group(1)
    d30 = (_dt.date.fromisoformat(inc) + _dt.timedelta(days=30)).isoformat()
    if not has("load_skill"):
        return reply("", [tc("load_skill", {"name": "fwd-travel-insurance"})])

    if "FWDBUY" in goal:
        d1, d2 = re.findall(r"(20\d\d-\d\d-\d\d)", goal)[:2]
        if not has("gmail_search"):
            return reply("", [tc("gmail_search", {"query": f"(flight OR e-ticket OR hotel OR booking) (Tokyo OR Japan) newer_than:90d"}),
                              tc("profile_get", {"fields": ["name_en", "birthday", "email_personal", "phone"]})])
        if not has("browser_navigate"):
            return reply("", [tc("browser_navigate", {"url": PAGES + "/fwd_sg_quote.html"})])
        if not has("browser_type"):
            pg = page()
            return reply("", [tc("browser_type", {"ref": _snap_ref(pg, lbl) or "e1", "text": v})
                              for lbl, v in [("Destination", "Japan"), ("Departure date", d1), ("Return date", d2)]])
        if not has("browser_click"):
            return reply("", [tc("browser_click", {"ref": _snap_ref(page(), "Get quote", "button") or "e1"})])
        if not has("present_choices"):
            quote = result("browser_click")
            plans = re.findall(r"(Premium|Business|First)\W+SGD (\d+\.\d\d)\W+SGD ([\d,]+)\W+SGD ([\d,]+)", quote)
            if len(plans) != 3:
                return reply("FWD FAIL: no plans on the quote page: " + quote[-1200:])
            return reply("", [tc("present_choices", {"question": "Which FWD plan?", "kind": "comparison", "options": [
                {"label": p, "details": [f"SGD {pr}", f"Overseas medical SGD {med}", f"Trip cancellation SGD {can}"],
                 "source_url": PAGES + "/fwd_sg_quote.html",
                 "note": "Cheapest with at least SGD 10,000 trip cancellation — recommended." if p == "Business" else ""}
                for p, pr, med, can in plans]})])
        if not has("vault_list"):
            return reply("", [tc("vault_list", {})])
        vl = result("vault_list")
        card, passport = _vault_id(vl, "card"), _vault_id(vl, "id_document")
        if not has("purchase_confirm"):
            return reply("", [tc("purchase_confirm", {"site": "shop.test", "items": [{"name": f"FWD Business travel insurance, Japan, {d1} to {d2}, 1 traveller",
                                                                                      "qty": 1, "price": 47.0}],
                                                      "shipping": 0, "total": 47.0, "currency": "SGD", "delivery": "Policy by email",
                                                      "card_item_id": card, "note": "Single trip, Business plan"})])
        if sum(n == "browser_navigate" for n in names) < 2:
            return reply("", [tc("browser_navigate", {"url": PAGES + "/fwd_sg_apply.html?plan=Business&premium=47.00"})])
        pg = page()
        if sum(n == "browser_type" for n in names) < 4:
            return reply("", [tc("browser_type", {"ref": _snap_ref(pg, lbl) or "e1", "text": v}) for lbl, v in
                              [("Full name", "TAN WEI MING"), ("Date of birth", "1985-04-12"), ("Email", "weiming@example.com"), ("Mobile number", "+65 9123 4567")]])
        fills = [a for n, a, _ in calls if n == "browser_fill_secret"]
        todo = [("Passport number", passport, "number"), ("Card number", card, "number"), ("Expiry", card, "expiry"),
                ("CVC", card, "cvc"), ("Name on card", card, "holder")]
        if len(fills) < len(todo):
            lbl, item, field = todo[len(fills)]
            return reply("", [tc("browser_fill_secret", {"ref": _snap_ref(pg, lbl) or "e1", "item_id": item, "field": field})])
        if not has("browser_request_takeover"):
            return reply("", [tc("browser_request_takeover", {"reason": "The FWD application is filled in (Business plan, SGD 47.00) — check it and press Pay now yourself"})])
        return reply("**FWD Singapore** (FWD Singapore Pte. Ltd. is the insurer) — quote for Japan, "
                     f"{d1} to {d2}, 1 traveller:\n\n| Plan | Premium | Trip cancellation |\n|---|---|---|\n| Premium | SGD 34.00 | SGD 5,000 |\n"
                     "| **Business** | **SGD 47.00** | **SGD 10,000** |\n| First | SGD 68.00 | SGD 15,000 |\n\n"
                     "**Business** is the cheapest plan with at least SGD 10,000 trip cancellation. The application is filled in "
                     "(name, date of birth, email, mobile; passport and card from your Vault) — check it and press **Pay now**; I did not pay.\n\n"
                     "Overseas emergencies: **+65 6322 2072**.")

    if "FWDCLAIM" in goal:
        if not has("gmail_search"):
            return reply("", [tc("gmail_search", {"query": "(FWD OR bolttech) (policy OR certificate) travel newer_than:365d"}),
                              tc("gmail_search", {"query": "(Singapore Airlines OR SQ 636) (delay OR delayed OR cancelled OR irregularity OR baggage) newer_than:60d"}),
                              tc("gmail_search", {"query": "(receipt OR invoice OR clinic OR hospital) newer_than:60d"}),
                              tc("profile_get", {"fields": ["name_en", "email_personal", "phone", "address_home"]})])
        if not has("gmail_get_message"):
            delay = next((r for n, a, r in calls if n == "gmail_search" and "SQ 636" in str(a)), "")
            return reply("", [tc("gmail_get_message", {"message_id": _msg_id(delay)})])
        if not has("gmail_save_attachment"):
            msg = result("gmail_get_message")
            fn = (re.search(r'"filename": "([^"]+\.pdf)"', msg) or re.search(r"(delay_certificate_SQ636\.pdf)", msg + " delay_certificate_SQ636.pdf")).group(1)
            return reply("", [tc("gmail_save_attachment", {"message_id": re.search(r'"id": "([^"]+)"', msg).group(1) if '"id"' in msg else "1",
                                                           "filename": fn})])
        if not has("calculate"):
            return reply("", [tc("calculate", {"expression": "(17*60+45 - (10*60+30)) / 60"})])
        if not has("browser_navigate"):
            return reply("", [tc("browser_navigate", {"url": PAGES + "/fwd_sg_claim.html"})])
        if not has("vault_list"):
            return reply("", [tc("vault_list", {})])
        pg = page()
        if not has("browser_type"):
            vals = [("Policy number", "TI-SG-2026-55123"), ("Full name", "TAN WEI MING"), ("Email", "weiming@example.com"),
                    ("Mobile number", "+65 9123 4567"), ("Date of incident", inc),
                    ("What happened", f"Flight SQ 636 SIN-HND on {inc} was scheduled at 10:30 and left at 17:45: a delay of 7 hours 15 minutes (late inbound aircraft)."),
                    ("Amount claimed", "100.00")]
            return reply("", [tc("browser_type", {"ref": _snap_ref(pg, lbl) or "e1", "text": v}) for lbl, v in vals])
        if not has("browser_fill_secret"):
            return reply("", [tc("browser_fill_secret", {"ref": _snap_ref(pg, "NRIC or passport number") or "e1",
                                                         "item_id": _vault_id(result("vault_list"), "id_document"), "field": "number"})])
        if not has("browser_upload"):
            saved = re.search(r"(attachments/[^\s\"\\]+\.pdf)", result("gmail_save_attachment"))
            return reply("", [tc("browser_upload", {"ref": _snap_ref(pg, "Supporting documents") or "e1",
                                                    "path": saved.group(1) if saved else "attachments/delay_certificate_SQ636.pdf"})])
        if not has("browser_request_takeover"):
            return reply("", [tc("browser_request_takeover", {"reason": "The FWD travel claim is filled in with the delay certificate attached — check it and press Submit claim yourself"})])
        if not has("schedule_create"):
            return reply("", [tc("schedule_create", {"name": f"FWD claim SQ 636 delay — check mail weekly (notify by {d30})", "kind": "cron", "spec": "0 9 * * 1",
                                                     "goal": "Check Gmail for any email from FWD about the travel delay claim on policy TI-SG-2026-55123; notify me of news"})])
        if not has("memory_remember"):
            return reply("", [tc("memory_remember", {"fact": f"FWD Singapore travel delay claim, policy TI-SG-2026-55123 (Business): SQ 636 of {inc}, delayed 7 h 15 min, SGD 100 claimed; tell FWD by {d30}.",
                                                     "domain": "trip"})])
        return reply(f"**FWD Singapore's online claim form** — tell FWD by **{d30}** (30 days after the delay on {inc}).\n\n"
                     "- **Policy:** TI-SG-2026-55123, Business plan.\n"
                     "- **Delay:** SQ 636 was scheduled at 10:30 and left at 17:45 — **7 hours 15 minutes**. The plan pays SGD 100 for every 6 hours, "
                     "so **SGD 100** is claimed (calculated: 7.25 hours → one full 6-hour block).\n"
                     "- **Documents:** the policy email and Singapore Airlines' delay certificate (uploaded to the form). Keep boarding passes.\n"
                     "- **Filled:** policy number, name, email, mobile, date, what happened, the amount; passport number from your Vault. "
                     "**You press Submit claim** — I did not.\n- **Reminders:** a weekly mail check for FWD's reply.\n\n"
                     "Overseas emergencies: +65 6322 2072; in Singapore +65 6820 8888. FWD decides the claim.")

    if "FWDHKCLAIM" in goal:
        form_url = PAGES + "/travel-insurance-clm-form-fillable.pdf"
        if not has("gmail_search"):
            return reply("", [tc("gmail_search", {"query": "(FWD OR bolttech) (policy OR certificate) travel newer_than:365d"}),
                              tc("gmail_search", {"query": "(Cathay Pacific OR CX 543) (delay OR delayed OR cancelled OR irregularity OR baggage) newer_than:60d"}),
                              tc("gmail_search", {"query": "(receipt OR invoice OR clinic OR hospital) newer_than:60d"}),
                              tc("profile_get", {"fields": ["name_en", "email_personal", "phone", "address_home"]})])
        if not has("gmail_get_message"):
            delay = next((r for n, a, r in calls if n == "gmail_search" and "CX 543" in str(a)), "")
            return reply("", [tc("gmail_get_message", {"message_id": _msg_id(delay)})])
        if not has("gmail_save_attachment"):
            msg = result("gmail_get_message")
            fn = (re.search(r'"filename": "([^"]+\.pdf)"', msg) or re.search(r"(delay_letter_CX543\.pdf)", msg + " delay_letter_CX543.pdf")).group(1)
            return reply("", [tc("gmail_save_attachment", {"message_id": re.search(r'"id": "([^"]+)"', msg).group(1) if '"id"' in msg else "1",
                                                           "filename": fn})])
        if not has("calculate"):
            return reply("", [tc("calculate", {"expression": "floor(((15*60+20) - (9*60+5)) / 60 / 6) * 300"})])
        if not has("browser_navigate"):
            return reply("", [tc("browser_navigate", {"url": form_url})])
        form = re.search(r'"path": "(downloads/[^"]+\.pdf)"', result("browser_navigate").replace('\\"', '"'))
        if not form:
            return reply("FWD FAIL: the claim form was not saved: " + result("browser_navigate")[:600])
        form = form.group(1)
        if not has("pdf_form_fields"):
            return reply("", [tc("pdf_form_fields", {"path": form})])
        if not has("pdf_form_fill"):
            return reply("", [tc("pdf_form_fill", {"path": form, "values": {
                "fill_17": "MT-HK-2026-00881", "Full name": "CHAN TAI MAN",
                "Correspondence address": "Flat 12B, 88 Robinson Road, Mid-Levels, Hong Kong", "Email address": "taiman@example.com",
                "fill_12": f"{inc} 09:05, Hong Kong International Airport",
                "fill_11": f"{inc} 09:05", "fill_12_2": "HKG", "fill_13_2": "NRT", "fill_14_2": "CX 543",
                "fill_15_2": f"{inc} 15:20", "fill_16_2": "HKG", "fill_17_2": "NRT", "fill_18_2": "CX 543",
                "fill_1": "6 hours 15 minutes", "fill_7": "HKD 300", "fill_8": "Cathay Pacific"}})])
        filled = re.search(r"(downloads/[^\s\"\\]+-filled\.pdf)", result("pdf_form_fill"))
        filled = filled.group(1) if filled else form.replace(".pdf", "-filled.pdf")
        if not has("send_file"):
            return reply("", [tc("send_file", {"path": filled, "note": "bolttech claim form, filled — add your HKID, bank account and signature"})])
        if not has("gmail_create_draft"):
            saved = re.search(r"(attachments/[^\s\"\\]+\.pdf)", result("gmail_save_attachment"))
            return reply("", [tc("gmail_create_draft", {
                "to": "claims.hk@bolttechinsurance.com",
                "subject": f"Travel insurance claim — policy MT-HK-2026-00881 — flight delay CX 543 {inc}",
                "body": ("Dear bolttech Claims,\n\nPolicyholder: CHAN TAI MAN. Policy: MT-HK-2026-00881 (MyTravel, sold through FWD).\n"
                         f"Cathay Pacific CX 543 Hong Kong to Tokyo Narita on {inc} was scheduled at 09:05 and departed at 15:20, a delay of 6 hours 15 minutes. "
                         "I claim the travel delay benefit of HKD 300.\n\nAttached: the claim form and Cathay Pacific's delay confirmation.\n\nKind regards,\nChan Tai Man"),
                "attachments": [filled, saved.group(1) if saved else "attachments/delay_letter_CX543.pdf"]})])
        if not has("schedule_create"):
            return reply("", [tc("schedule_create", {"name": "bolttech claim CX 543 delay — check mail weekly", "kind": "cron", "spec": "0 9 * * 1",
                                                     "goal": "Check Gmail for any email from bolttech about the travel delay claim on policy MT-HK-2026-00881; notify me of news"})])
        if not has("memory_remember"):
            return reply("", [tc("memory_remember", {"fact": f"bolttech (FWD HK MyTravel) travel delay claim, policy MT-HK-2026-00881: CX 543 of {inc}, delayed 6 h 15 min, HKD 300 claimed; draft to claims.hk@bolttechinsurance.com.",
                                                     "domain": "trip"})])
        return reply("**bolttech's claim form by email** — your MyTravel plan is sold by FWD but underwritten by **bolttech Insurance (Hong Kong)**, "
                     "so bolttech handles the claim. The time limit could not be checked: the policy wording is not in your mail, so send it as soon as possible.\n\n"
                     "- **Delay:** CX 543 was scheduled at 09:05 and left at 15:20 — **6 hours 15 minutes**: one full 6-hour block × HKD 300 = **HKD 300**.\n"
                     "- **Form:** filled with your policy number, name, address, email, the schedule, the hours and the amount (in the chat). "
                     "**You add** your HKID number, bank account for the direct credit and your signature.\n"
                     "- **Email:** a draft to claims.hk@bolttechinsurance.com with the form and Cathay Pacific's delay letter attached — sign the form, "
                     "replace it in the draft and send it.\n- **Reminders:** a weekly mail check for bolttech's reply.\n\n"
                     "bolttech claims enquiries: +852 3123 3344. Emergency assistance: +852 2000 0100 (from your policy). bolttech decides the claim.")
    return None


def route_reply(user):
    """The router's JSON (bots_e2e.py): BOTHAND / BOTAPPR / BOTWAIT go to Travel booking with the kid's marker as the
    question, BOTFOLLOW to the conversation the history names, BOTROUTEFAIL gets no JSON (the executor decides with
    ask_bot), BOTROUTEBAD names a bot that does not exist; everything else stays in Main."""
    req = user.split("The user's new request:\n", 1)[-1]
    date = (re.search(r"DATE=(20\d\d-\d\d-\d\d)", req) or re.search(r"(20\d\d-\d\d-\d\d)", "x 2026-12-20")).group(1)
    if "BOTROUTEFAIL" in req:
        return reply("I am not sure.")
    if "BOTROUTEBAD" in req:
        return reply(json.dumps({"bot": "no-such-bot", "question": "x"}))
    slow = " BOTSLOW" if "BOTSLOW" in req else ""
    if slow:   # bots_ui.py: the routing takes a while, so Main's timer must be longer than the bot's by that much
        time.sleep(2)
    for marker, kid in (("BOTHAND", f"BOTKID1 cheapest flight Singapore to Tokyo on {date} DATE={date}"),
                        ("BOTAPPR", "BOTKIDAPPR book the SQ638 for SGD 620"), ("BOTWAIT", "BOTKIDWAIT log in to the airline site")):
        if marker in req:
            return reply(json.dumps({"bot": "travel", "question": kid + slow}))
    if "BOTFOLLOW" in req:
        m = re.search(r"\[answered by the travel bot in conversation (conv_\w+)\]", user)
        return reply(json.dumps({"bot": "travel", "question": "BOTKID2 and the return flight a week later",
                                 "conversation_id": m.group(1) if m else ""}))
    if "BOTCONVBAD" in req:   # a conversation that is not that bot's: the code starts a new one
        return reply(json.dumps({"bot": "travel", "question": f"BOTKID1 checked conv DATE={date}", "conversation_id": "conv_nope"}))
    return reply('```json\n{"bot": "main"}\n```')


def bots_flow(goal, sys, msgs, tools_done, n, allu):
    """bots_e2e.py: the main bot hands a request to the Travel booking bot (ask_bot) and shows its answer; a follow-up
    goes back to the same bot conversation; an approval and a takeover inside the bot."""
    main = "## Bots" in sys
    travel = "You are the Travel booking bot" in sys
    date = (re.search(r"DATE=(20\d\d-\d\d-\d\d)", allu) or re.search(r"(20\d\d-\d\d-\d\d)", "x 2026-12-20")).group(1)
    if main and n == 0:
        for marker, kid in (("BOTHAND", f"BOTKID1 cheapest flight Singapore to Tokyo on {date} DATE={date}"),
                            ("BOTAPPR", "BOTKIDAPPR book the SQ638 for SGD 620"), ("BOTWAIT", "BOTKIDWAIT log in to the airline site")):
            if marker in goal:
                return reply("", [tc("ask_bot", {"bot": "travel", "question": kid})])
        if "BOTFOLLOW" in goal:
            hist = "\n".join(str(m["content"]) for m in msgs if m["role"] in ("user", "assistant"))
            m = re.search(r"\[answered by the travel bot in conversation (conv_\w+)\]", hist)
            if not m:
                return reply("NO HANDOFF IN HISTORY")
            return reply("", [tc("ask_bot", {"bot": "travel", "conversation_id": m.group(1),
                                             "question": "BOTKID2 and the return flight a week later"})])
        if "BOTSKILL" in goal:   # the main bot has no travel skill: load_skill is refused and points to ask_bot
            return reply("", [tc("load_skill", {"name": "flight-booking"})])
        if "BOTTOOL" in goal:    # nor the travel searches: the tool is not offered, and a call is refused
            offered = any((x.get("function") or {}).get("name") == "flights_search" for x in (CALLS[-1].get("tools") or []))
            return reply("", [tc("flights_search", {"origin": "SIN", "destination": "TYO", "date": date, "offered": offered})])
        if "BOTROUTEFAIL" in goal or "BOTROUTEBAD" in goal:   # no routing decision: the executor hands over itself
            return reply("", [tc("ask_bot", {"bot": "travel", "question": f"BOTKID1 executor hand-over DATE={date}"})])
    if main and "BOTTOOL" in goal:
        return reply("MAIN SAW: " + str(tools_done[-1]["content"])[:200])
    if main and "BOTSKILL" in goal:
        return reply("MAIN SAW: " + str(tools_done[-1]["content"])[:200])
    if travel and "BOTKID1" in goal:
        if n == 0:
            return reply("", [tc("load_skill", {"name": "flight-booking"})])
        if n == 1:
            return reply("", [tc("flights_search", {"origin": "SIN", "destination": "TYO", "date": date, "cabin": "economy", "currency": "SGD"})])
        if "BOTSLOW" in goal:   # bots_ui.py: the bot works a while, so its own timer shows it
            time.sleep(2)
        return reply(f"TRAVELBOT ANSWER {date}: Singapore Airlines SQ638 SGD 620 direct; Cathay Pacific via HKG SGD 510.")
    if travel and "BOTKID2" in goal:
        earlier = "TRAVELBOT ANSWER" in allu or any("TRAVELBOT ANSWER" in str(m["content"]) for m in msgs if m["role"] == "assistant")
        return reply("TRAVELBOT FOLLOW-UP: " + ("the outbound was SQ638, so the return is SQ637 a week later." if earlier else "NO CONTEXT"))
    if travel and "BOTKIDAPPR" in goal:
        if n == 0:
            return reply("", [tc("purchase_confirm", {"site": "shop.test", "items": [{"name": "Flight SQ638 SIN-NRT", "price": 620}],
                                                      "total": 620, "currency": "SGD"})])
        return reply("TRAVELBOT BOOKED: the purchase of SQ638 was approved.")
    if travel and "BOTKIDWAIT" in goal:
        if n == 0:
            return reply("", [tc("browser_request_takeover", {"reason": "please log in to the airline site"})])
        return reply("TRAVELBOT DONE after the takeover.")
    return None


def travel_skill(goal, sys, msgs, tools_done, n, last_tool, tnames):
    """One scripted agent per skill. Returns None when the goal is not one of these scenarios."""
    if "focused sub-agent" in sys:
        return None
    tc_ = tc
    if str(goal).startswith(SHOW_TRIP):
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
    if str(goal).startswith(SHOW_CITY):
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
    if "SGAC" in goal or "SGACPICK" in goal or str(goal).startswith(SHOW_SGAC):   # sg-arrival-card: gather, fill, hand the Submit to the user (or click it when told to)
        if n == 0:
            return reply("", [tc_("load_skill", {"name": "sg-arrival-card"})])
        if n == 1:
            return reply("", [tc_("gmail_search", {"query": "(Singapore OR SIN OR Changi) (flight OR e-ticket) newer_than:60d"}),
                              tc_("gmail_search", {"query": "(hotel OR booking) Singapore newer_than:60d"})])
        if n == 3:
            return reply("", [tc_("profile_get", {"fields": ["name_en", "birthday", "nationality", "email_personal", "phone",
                                                              "address_home", "address_work", "address:*"]})])
        # the accommodation: a hotel booking in mail wins; else the profile address in Singapore (one → use it, several →
        # ask in the chat which, unless the user already picked one); else ask
        hotel = "Marina Bayfront Hotel" if n >= 3 and "Marina Bayfront Hotel" in str(tools_done[2]["content"]) else None
        prof = str(tools_done[3]["content"]) if n > 3 else ""
        sg_addr = [(lab, val) for lab, val in re.findall(r"Address — ([^(]+?) \(address:[^)]*\): (.+)", prof) if "Singapore" in val]
        if n > 3 and not hotel:
            if "SGACPICK" in goal:
                sg_addr = [x for x in sg_addr if x[0].lower() in goal.lower()] or sg_addr[:1]
            if len(sg_addr) > 1 and n == 4:
                return reply("You have " + str(len(sg_addr)) + " addresses in Singapore in your profile: " +
                             " / ".join(f"**{lab}** ({val})" for lab, val in sg_addr) + ". Which one are you staying at? Reply with the name and I'll fill the card.")
            if not sg_addr and n == 4:
                return reply("WAITING FOR DATA: where are you staying in Singapore (hotel name or address)? Or upload the booking confirmation.")
        stay = hotel or (sg_addr[0][1] if sg_addr else "?")
        if n == 4:
            return reply("", [tc_("vault_list", {})])
        if n == 5:
            return reply("", [tc_("browser_navigate", {"url": PAGES + "/sgac.html"})])
        if n == 6:
            page = last_tool
            vals = [("Full name", "LUCAS LU"), ("Date of birth", "1985-04-12"), ("Nationality", "Australia"),
                    ("Arrival flight", "SQ 231"), ("Arrival date", (re.search(r"SQ 231 SYD-SIN (20\d\d-\d\d-\d\d)", str(tools_done[1]["content"])) or re.search(r"(20\d\d-\d\d-\d\d)", "x 2026-11-02")).group(1)),
                    ("Accommodation", stay)]
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
        used = "" if hotel else " (your profile address in Singapore)"
        return reply(f"NOT SUBMITTED: the SG Arrival Card is still open with every field filled (LUCAS LU · 1985-04-12 · Australia · SQ 231 · "
                     f"{stay}{used} · passport from the vault). Press Submit in the browser tab when you are ready, or ask me again.")
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
    if "BRAVETRAVEL" in goal:   # flight-booking without the travel tools, the Web search API on: snippets, no browser pages
        api = re.search(r"Web search \(web_search, always available\): search services connected", sys)
        if n == 0:
            return reply("", [tc_("load_skill", {"name": "flight-booking"})])
        if n == 1:
            if not api:
                return reply("NO_SEARCH_API")
            # one data tool each, in ONE turn: flights_search (Kiwi.com), hotels_search (web search without the key), and
            # web_search only for the car (no data tool for cars)
            # the trip date comes from the request (DATE=…), so it never falls in the past
            d1 = _dtm.date.fromisoformat(re.search(r"DATE=(\d{4}-\d\d-\d\d)", goal).group(1))
            d3 = d1 + _dtm.timedelta(days=3)
            return reply("", [tc_("flights_search", {"origin": "SIN", "destination": "NRT", "date": d1.isoformat(), "adults": 1, "cabin": "economy", "sort": "price", "currency": "SGD"}),
                              tc_("hotels_search", {"city": "Tokyo", "area": "Ginza", "check_in": d1.isoformat(), "check_out": d3.isoformat(), "min_stars": 4}),
                              tc_("web_search", {"query": f"Tokyo Narita airport car rental {d1.day} {d1:%b %Y} compact", "max_results": 5})])
        allt = "\n".join(str(m["content"]) for m in tools_done)
        srcs = re.findall(r"web search \(([^)]+)\)", allt)
        kiwi = "Kiwi.com" in allt and "kiwi.com/u/" not in allt.lower()
        return reply(f"SEARCHED VIA {sorted(set(srcs))}: flights SIN–NRT 20 Nov — {'Kiwi.com data (information only): SQ638 09:25 → 17:15 SGD 620 direct, CX via HKG SGD 510' if kiwi else 'from search snippets'}, "
                     f"hotels near Ginza ({'web search' if 'hotels_fallback' in allt else 'Google Hotels'}); compact car at Narita (Brave result 1). "
                     f"Book the flight on {_book_at(allt)} (the airline's own site) or with the agency you choose — never through Kiwi.")
    if "KIWIFLIGHTS" in goal:   # flight-booking with no key: flights_search (Kiwi.com data) ONLY; book on the airline's site from book_at
        date = (re.search(r"DATE=(20\d\d-\d\d-\d\d)", goal) or re.search(r"(20\d\d-\d\d-\d\d)", "x 2026-11-20")).group(1)
        if n == 0:
            return reply("", [tc_("load_skill", {"name": "flight-booking"})])
        if n == 1:
            return reply("", [tc_("flights_search", {"origin": "SIN", "destination": "NRT", "date": date, "adults": 1, "cabin": "economy", "sort": "price", "currency": "SGD"})])
        allt = "\n".join(str(m["content"]) for m in tools_done)
        kiwi = "Kiwi.com" in allt and "kiwi.com/u/" not in allt.lower()
        return reply(f"FLIGHTS {date} SIN → NRT (flight data{' from Kiwi.com, information only' if kiwi else ''}): cheapest direct Singapore Airlines SQ638 09:25 → 17:15, SGD 620, 1 checked bag; "
                     f"cheapest overall Cathay Pacific via HKG SGD 510 (one stop). Book it on {_book_at(allt)} (the airline's own site, from the offer), "
                     "or tell me the agency you prefer — say the word and I open the booking page.")
    if "BIZFLIGHTS" in goal:   # flight-booking: the request's cabin (business), stop limit (direct) and city code (TYO) go into ONE flights_search
        date = (re.search(r"DATE=(20\d\d-\d\d-\d\d)", goal) or re.search(r"(20\d\d-\d\d-\d\d)", "x 2026-11-20")).group(1)
        if n == 0:
            return reply("", [tc_("load_skill", {"name": "flight-booking"})])
        if n == 1:
            # the instructions carry the profile's standing preferences: the preferred airline (or alliance) goes into the search
            pm = re.search(r"- Preferred airline: ([^\n]+)", sys)
            args = {"origin": "TYO", "destination": "SIN", "date": date, "adults": 1, "cabin": "business", "stops": "nonstop", "sort": "price", "currency": "SGD"}
            if pm:
                args["airlines"] = [x.strip() for x in pm.group(1).split(",")]
            return reply("", [tc_("flights_search", args)])
        allt = "\n".join(str(m["content"]) for m in tools_done)
        m = re.search(r'\\?"cabin\\?": \\?"([a-z ]+)\\?"', allt)
        pref = ' — your preferred airline (flagged preferred in the data)' if '"preferred": true' in allt or '\\"preferred\\": true' in allt else ''
        return reply(f"BUSINESS CLASS, nonstop, {date} Tokyo (all airports) → Singapore, searched as {m.group(1) if m else '?'} / nonstop only: "
                     f"Singapore Airlines SQ638 09:25 → 17:15, SGD 2108 (business, 1 checked bag){pref}. Book on singaporeair.com or with your agency — say the word.")
    if "ZEROCAP" in goal:   # a model filling defaults: max_price 0 and adults 0 — the tool must search every price for 1 adult
        date = (re.search(r"DATE=(20\d\d-\d\d-\d\d)", goal) or re.search(r"(20\d\d-\d\d-\d\d)", "x 2026-11-20")).group(1)
        if n == 0:
            return reply("", [tc_("flights_search", {"origin": "SIN", "destination": "TYO", "date": date, "adults": 0, "max_price": 0,
                                                     "cabin": "economy", "stops": "any", "sort": "price", "currency": "SGD"})])
        return reply(f"{date} Singapore → Tokyo, any price: Cathay Pacific via HKG SGD 510; Singapore Airlines SQ638 SGD 620 direct.")
    if "PREFNONE" in goal:   # the preferred airline goes into the ONE search; it has no offer → answer and ask before searching all airlines
        date = (re.search(r"DATE=(20\d\d-\d\d-\d\d)", goal) or re.search(r"(20\d\d-\d\d-\d\d)", "x 2026-11-20")).group(1)
        if n == 0:
            return reply("", [tc_("load_skill", {"name": "flight-booking"})])
        if n == 1:
            pm = re.search(r"- Preferred airline: ([^\n]+)", sys)
            args = {"origin": "SIN", "destination": "TYO", "date": date, "cabin": "economy", "stops": "any", "sort": "price", "currency": "SGD"}
            if pm:
                args["airlines"] = [x.strip() for x in pm.group(1).split(",")]
            return reply("", [tc_("flights_search", args)])
        allt = "\n".join(str(m["content"]) for m in tools_done)
        none = re.search(r'\\?"preferred_found\\?": 0', allt) is not None
        return reply(f"{date} Singapore → Tokyo: {'no Emirates flight came back on this route' if none else 'Emirates found'}; "
                     "the search also returned Singapore Airlines SQ638 SGD 620 (direct) and Cathay Pacific via HKG SGD 510. Shall I search all airlines?")
    if "FLEXFLIGHTS" in goal:   # a range of dates: ONE flights_search with date + date_to, no plan
        d1 = re.search(r"FROM=(20\d\d-\d\d-\d\d)", goal).group(1); d2 = re.search(r"TO=(20\d\d-\d\d-\d\d)", goal).group(1)
        if n == 0:
            return reply("", [tc_("load_skill", {"name": "flight-booking"})])
        if n == 1:
            return reply("", [tc_("flights_search", {"origin": "SIN", "destination": "TYO", "date": d1, "date_to": d2, "cabin": "economy", "sort": "price", "currency": "SGD"})])
        allt = "\n".join(str(m["content"]) for m in tools_done)
        days = len(set(re.findall(r'\\?"date\\?": \\?"(20\d\d-\d\d-\d\d)', allt)))
        return reply(f"Cheapest across {d1} to {d2}: Cathay Pacific via HKG SGD 510 ({days} days searched in one call).")
    if "BOOKIT" in goal:   # "book it" after the flight was found earlier in this conversation: straight to the booking page, no plan, no search
        url = (re.search(r"URL=(\S+)", goal) or re.search(r"(http\S+)", "x http://shop.test:8097/book/flight/none")).group(1)
        names_so_far = [n for n, _, _ in _travel_calls(msgs)]
        calls = _travel_calls(msgs)
        res_of = lambda name: next((r for n, _, r in calls if n == name), "")   # noqa: E731
        if "vault_list" not in names_so_far:
            return reply("", [tc_("vault_list", {})])
        vl = res_of("vault_list")
        card_id, passport_id = _vault_id(vl, "card"), _vault_id(vl, "id_document")
        if "browser_navigate" not in names_so_far:
            return reply("", [tc_("browser_navigate", {"url": url})])
        page = res_of("browser_navigate")
        if "browser_type" not in names_so_far:
            fields = [("Full name", TRAVELLER["name"]), ("Email", TRAVELLER["email"]), ("Phone", TRAVELLER["phone"])]
            typed = [tc_("browser_type", {"ref": _snap_ref(page, lbl), "text": v}) for lbl, v in fields if _snap_ref(page, lbl)]
            return reply("", typed or [tc_("browser_request_takeover", {"reason": "no form fields found; please book by hand"})])
        fills = [a for n, a, _ in calls if n == "browser_fill_secret"]
        if passport_id and not any(a.get("item_id") == passport_id for a in fills):
            return reply("", [tc_("browser_fill_secret", {"ref": _snap_ref(page, "Passport number") or "e1", "item_id": passport_id, "field": "number"})])
        amount = float((re.search(r"AMOUNT=([\d.]+)", goal) or re.search(r"([\d.]+)", "0")).group(1))
        if card_id and "purchase_confirm" not in names_so_far:
            # Sentinel fills a card only after the user approved the purchase summary
            return reply("", [tc_("purchase_confirm", {"site": url.split("/")[2].split(":")[0], "items": [{"name": "SQ638 SIN-NRT, 1 adult", "qty": 1, "price": amount}],
                                                       "shipping": 0, "total": amount, "currency": "SGD", "delivery": "E-ticket to lucas@example.com", "card_item_id": card_id})])
        if card_id and not any(a.get("item_id") == card_id for a in fills):
            boxes = [("Card number", "number"), ("Expiry", "expiry"), ("CVC", "cvc"), ("Name on card", "holder")]
            return reply("", [tc_("browser_fill_secret", {"ref": _snap_ref(page, lbl), "item_id": card_id, "field": f}) for lbl, f in boxes if _snap_ref(page, lbl)])
        if "browser_request_takeover" not in names_so_far:
            return reply("", [tc_("browser_request_takeover", {"reason": "The booking page is filled in (traveller, passport, card). Check it and press Pay now."})])
        return reply("The booking page for SQ638 is open with your details, passport and card filled in — press Pay now to complete it, or tell me to.")
    if "TRAVELSEARCH" in goal:   # flight-booking with the built-in Flights & hotels tools (Google data through SerpApi)
        date = (re.search(r"DATE=(20\d\d-\d\d-\d\d)", goal) or re.search(r"(20\d\d-\d\d-\d\d)", "x 2026-11-20")).group(1)
        import datetime as _dt
        out = (_dt.date.fromisoformat(date) + _dt.timedelta(days=3)).isoformat()
        if n == 0:
            return reply("", [tc_("load_skill", {"name": "flight-booking"})])
        if n == 1:
            return reply("", [tc_("flights_search", {"origin": "Singapore", "destination": "Tokyo", "date": date, "adults": 1, "cabin": "economy", "sort": "price", "currency": "SGD"})])
        if n == 2:
            m = re.search(r'offer_id\\?"?: \\?"(fl_[0-9a-f]+)', str(last_tool)) or re.search(r"(fl_[0-9a-f]{10})", str(last_tool))
            return reply("", [tc_("flight_offer", {"offer_id": m.group(1) if m else "fl_missing", "currency": "SGD"})])
        if n == 3:
            return reply("", [tc_("hotels_search", {"city": "Tokyo", "area": "Ginza", "check_in": date, "check_out": out, "guests": 2, "min_stars": 4, "currency": "SGD"})])
        if n == 4:
            m = re.search(r"(ht_[0-9a-f]{10})", str(last_tool))
            return reply("", [tc_("hotel_rooms", {"hotel_id": m.group(1) if m else "ht_missing", "check_in": date, "check_out": out, "guests": 2, "currency": "SGD"})])
        allt = "\n".join(str(m["content"]) for m in tools_done)
        seller = re.search(r'book_with\\?"?: \\?"([^"\\]+)', allt)
        return reply(f"FLIGHTS {date} Singapore → Tokyo (Google Flights): cheapest direct SQ 638 09:25 → 17:15, SGD 620, sold by {seller.group(1) if seller else '?'} "
                     f"(Trip.com has it at SGD 612); cheapest overall Cathay via HKG SGD 510 (one stop, 12 h 35). "
                     f"HOTELS Ginza (Google Hotels): Hotel Ginza Grand 4★ rating 4.4, SGD 210/night, SGD 630 for 3 nights, free cancellation on Booking.com; "
                     f"the hotel's own site has a Deluxe Twin at SGD 199/night. Book the flight on singaporeair.com and the room on the hotel site — say the word.")
    if any(k in goal for k in ("FWDBUY", "FWDCLAIM", "FWDHKCLAIM")):
        return fwd_insurance(goal, msgs)
    if "CSRCLAIM" in goal:   # csr-trip-claim: mail in one turn, the arithmetic, the airline letter, the packet, the portal, hand-over
        import datetime as _dt
        loss = (re.search(r"LOSS=(20\d\d-\d\d-\d\d)", goal) or re.search(r"(20\d\d-\d\d-\d\d)", "x 2026-10-20")).group(1)
        d = _dt.date.fromisoformat(loss)
        d20, d90 = (d + _dt.timedelta(days=20)).isoformat(), (d + _dt.timedelta(days=90)).isoformat()
        pdf = f"reports/CSR-claim-UA837-{loss}.pdf"
        if n == 0:
            return reply("", [tc_("load_skill", {"name": "csr-trip-claim"})])
        if n == 1:
            return reply("", [tc_("gmail_search", {"query": "(e-ticket OR eTicket OR itinerary OR confirmation) (United OR UA 837 OR SFO) newer_than:180d"}),
                              tc_("gmail_search", {"query": "(Chase OR Sapphire) (transaction OR charged OR statement OR receipt) UNITED"}),
                              tc_("gmail_search", {"query": "United (refund OR credit OR voucher OR cancellation OR cancelled)"}),
                              tc_("profile_get", {"fields": ["name_en", "email_personal", "phone", "address_home"]})])
        if n == 5:
            return reply("", [tc_("calculate", {"expression": "1840.00 - 86.40"})])
        if n == 6:
            return reply("", [tc_("gmail_create_draft", {"to": "refunds@united.example",
                                                          "subject": f"Refund request — confirmation ABC123, UA 837 of {loss} — unable to travel for medical reasons",
                                                          "body": f"Dear United Refunds,\n\nI was medically unable to travel on UA 837 (SFO–NRT) on {loss}, confirmation ABC123, passenger LUCAS LU. "
                                                                  "Thank you for refunding the taxes and fees. Please (1) confirm in writing the non-refundable fare amount and your refund policy for this fare, "
                                                                  "which my card's trip cancellation insurance requires, and (2) advise whether a travel credit or a medical waiver of the fare is available.\n\nKind regards,\nLucas Lu"})])
        if n == 7:
            md = (f"# Trip Cancellation claim — Chase Sapphire Reserve\n\n**Traveller:** LUCAS LU  \n**Trip:** UA 837 SFO → NRT, {loss}, confirmation ABC123  \n"
                  f"**Date of loss:** {loss}  \n**Notify the administrator by:** {d20}  \n**Documents by:** {d90}  \n"
                  "**Administrator:** Card Benefit Services — chasecardbenefits.com — 1-800-350-1697\n\n"
                  "## Amounts\n\n| Booking | Paid | Refunded | Claimed |\n|---|---|---|---|\n| UA 837 Basic Economy (Sapphire Reserve …4321) | USD 1,840.00 | USD 86.40 taxes and fees | **USD 1,753.60** |\n\n"
                  "## Documents\n\n- [ ] Claim form (from the portal)\n- [ ] Physician statement — your doctor (asked)\n- [x] eTicket receipt ABC123 (mail)\n- [x] Card transaction USD 1,840.00 (Chase alert)\n"
                  "- [x] United refund confirmation: USD 86.40 taxes and fees, fare non-refundable (mail)\n- [ ] United written refund policy / non-refundable statement (requested)\n- [x] No other travel insurance\n\n"
                  "## Letters\n\nRefund request to United (draft in Gmail).\n\n## Timeline\n\n- Booking paid; fell ill; missed UA 837 on " + loss + "\n- Today: claim started on the portal, airline asked for the letter\n- Next: doctor's statement, upload documents before " + d90 + "\n")
            return reply("", [tc_("make_pdf", {"markdown": md, "output": pdf})])
        if n == 8:
            return reply("", [tc_("send_file", {"path": pdf, "note": "Your claim packet"})])
        if n == 9:
            return reply("", [tc_("browser_navigate", {"url": PAGES + "/eclaims.html"})])
        if n == 10:
            page = last_tool
            vals = [("Cardholder name", "LUCAS LU"), ("Date of loss", loss), ("Amount claimed", "1753.60"), ("Reason", "sickness")]
            return reply("", [tc_("browser_type", {"ref": _snap_ref(page, lab) or "e1", "text": v}) for lab, v in vals])
        if n == 14:
            return reply("", [tc_("browser_request_takeover", {"reason": "Review the claim, sign in with your Chase login if the portal asks, and press Submit yourself; hand the browser back when the claim number shows"})])
        if n == 15:
            return reply("", [tc_("schedule_create", {"name": f"CSR claim UA 837 — documents due {d90}", "kind": "cron", "spec": "0 9 * * 1",
                                                       "goal": f"Check Gmail for any message from Card Benefit Services, eclaimsline or Chase about the trip cancellation claim for UA 837 of {loss}; notify me of news; remind me that the documents are due by {d90}"})])
        if n == 16:
            return reply("", [tc_("memory_remember", {"fact": f"Chase Sapphire Reserve trip cancellation claim for UA 837 (ABC123) of {loss}: started with Card Benefit Services; notify by {d20}, documents by {d90}; USD 1,753.60 claimed.", "domain": "trip"})])
        return reply(f"**Trip Cancellation** is the benefit that applies (you never left home). Notify the administrator by **{d20}**; documents by **{d90}**.\n\n"
                     "**Amount to claim: USD 1,753.60** — you paid USD 1,840.00 on the Sapphire Reserve (…4321); United refunded USD 86.40 in taxes and fees; the fare is non-refundable.\n\n"
                     "- **Airline:** a refund request and a request for the written non-refundable statement is drafted to United (confirmation ABC123) — approve it to send.\n"
                     "- **Doctor:** the physician statement saying you were medically unable to travel on " + loss + " — I can prepare the form once the portal issues it.\n"
                     "- **Claim:** the form on the administrator's portal (Card Benefit Services, chasecardbenefits.com) is filled with your name, the date of loss and the amount; I handed the browser to you to sign in and press Submit. By phone: 1-800-350-1697.\n"
                     f"- **Packet:** {pdf} is in the chat — the checklist and the letters.\n- **Reminders:** a weekly check for the administrator's mail and the documents deadline.\n\n"
                     "The administrator decides; pre-existing conditions (treated in the 60 days before booking) and trips over 60 days are excluded.")
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
                calls.append(tc_("browser_read", {"urls": ["https://www.tokyometro.jp/en/ticket/regular/"]}))
            return reply("", calls)
        sheet = str(tools_done[0]["content"])
        line = next((l.strip() for l in sheet.split("## Getting around", 1)[-1].splitlines() if l.strip().startswith("- ")), "")
        rate = re.search(re.escape(f"{sym[:3]}/{sym[3:6]}") + r"[^0-9]*([0-9][0-9,]*\.[0-9]+)", str(tools_done[1]["content"]))
        return reply(f"CITY ANSWER ({skill}). Getting around: {line[2:200]} | FX {sym}: {rate.group(1) if rate else '?'}")
    if "DRIVECLEAN" in goal:   # drive_cleanup_e2e.py: quota + two searches in one turn, a list the user confirms, one trash batch
        def ids_in(txt):
            return [m for m in re.findall(r'"id": "([a-z]+-\d+)"', txt)]
        if "yes" in goal.lower():   # the follow-up: the user picked numbers 1 and 2 → one drive_trash call with names
            if n == 0:
                return reply("", [tc_("drive_trash", {"file_ids": ["big-1", "big-2"], "names": ["backup-2022.zip", "old-render.mov"]})])
            return reply("CLEANUP DONE: " + str(last_tool)[:300])
        if n == 0:
            return reply("", [tc_("load_skill", {"name": "drive-cleanup"})])
        if n == 1:
            return reply("", [tc_("drive_storage", {}),
                              tc_("drive_search", {"min_size_mb": 100, "order_by": "size", "max_results": 50}),
                              tc_("drive_search", {"unused_days": 365, "min_size_mb": 20, "order_by": "size"})])
        big = str(tools_done[2]["content"]); unused = str(tools_done[3]["content"])
        names = re.findall(r'"name": "([^"]+)"', big)
        return reply(f"CLEANUP LIST ({len(set(ids_in(big) + ids_in(unused)))} candidates: {', '.join(names)}). "
                     "1. backup-2022.zip 2560 MB never opened; 2. old-render.mov 900 MB last opened 2023-02-01; 3. wedding-raw.mov 4096 MB opened 2026-10-01 (recent). "
                     "Move all of these to the bin, or only some (numbers)?")
    if "DRIVEVIDEO" in goal:   # drive_video_e2e.py: search → download both → concat (quiet) → caption → upload back
        def out_path(txt):
            m = re.search(r"(videos/[^\s（(]+\.mp4)", str(txt))
            return m.group(1) if m else "videos/missing.mp4"
        if n == 0:
            return reply("", [tc_("load_skill", {"name": "drive-video-edit"})])
        if n == 1:
            return reply("", [tc_("drive_search", {"kind": "video", "query": "kyoto", "order_by": "modified"})])
        if n == 2:
            ids = re.findall(r'"id": "(vid-\d+)"', str(last_tool))
            return reply("", [tc_("drive_download", {"file_id": i}) for i in sorted(ids)])
        if n == 4:
            paths = re.findall(r"(drive/[^\s（(]+\.mp4)", "\n".join(str(x["content"]) for x in tools_done))
            return reply("", [tc_("edit_video", {"op": "concat", "paths": sorted(set(paths)), "title": "kyoto", "send": False})])
        if n == 5:
            return reply("", [tc_("edit_video", {"op": "caption", "path": out_path(last_tool), "text": "Kyoto 2026", "position": "top",
                                                 "start": 0, "end": 2, "title": "Kyoto highlights"})])
        if n == 6:
            return reply("", [tc_("drive_upload", {"path": out_path(last_tool), "folder_id": "fld-trip"})])
        return reply("VIDEO EDIT DONE: " + str(last_tool)[:300])
    if "DRIVEBOOK" in goal:   # drive_storybook_e2e.py: photos → download → one edit → the PDF → upload back
        if n == 0:
            return reply("", [tc_("load_skill", {"name": "drive-photo-storybook"})])
        if n == 1:
            return reply("", [tc_("drive_search", {"kind": "image", "folder_id": "fld-bday", "order_by": "name"})])
        if n == 2:
            ids = re.findall(r'"id": "(img-\d+)"', str(last_tool))
            return reply("", [tc_("drive_download", {"file_id": i}) for i in sorted(ids)])
        if n == 5:
            return reply("", [tc_("edit_image", {"path": "drive/mia-cake.jpg", "prompt": "soft watercolour children's book style", "title": "cake-watercolour"})])
        if n == 6:
            md = ("# Mia's Birthday\n\nA story book\n\n## The cake\n\n![cake](images/cake-watercolour.png)\n\nMia blew out five candles.\n\n"
                  "## Balloons\n\n![balloons](drive/mia-balloons.jpg)\n\nBlue balloons floated to the ceiling.\n\n"
                  "## Friends\n\n![friends](drive/mia-friends.jpg)\n\nEveryone sang.\n")
            return reply("", [tc_("make_pdf", {"markdown": md, "output": "reports/Mia-birthday-storybook.pdf"})])
        if n == 7:   # make_pdf does not send: show the book, then put it back in Drive
            m = re.search(r"created locally: (\S+)", str(last_tool))
            return reply("", [tc_("send_file", {"path": m.group(1) if m else "reports/Mia-birthday-storybook.pdf", "note": "Mia's birthday story book"})])
        if n == 8:
            return reply("", [tc_("drive_upload", {"path": "reports/Mia-birthday-storybook.pdf", "folder_id": "fld-bday", "name": "Mia birthday storybook.pdf"})])
        return reply("STORYBOOK DONE: " + str(last_tool)[:300])
    if "VIDEOGEN" in goal:   # videogen_e2e.py: one short clip, shown in the chat
        if n == 0:
            prompt = "a violent storm at sea" if "storm" in goal else "a paper boat drifting across a calm pond at sunrise, gentle ripples"
            return reply("", [tc_("make_video", {"prompt": prompt, "seconds": 4, "aspect": "landscape", "title": "paper boat"})])
        if "VIDEOGEN twice" in goal and n == 1:   # a model that ignores the advice and asks for a second clip
            return reply("", [tc_("make_video", {"prompt": "the same boat, closer", "seconds": 4})])
        return reply("VIDEO DONE: " + str(last_tool)[:300])
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
