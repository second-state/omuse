"""Scripted OpenAI-compatible server for integration tests.

Behaviour is chosen from the latest user goal text:
 - planner calls (system prompt contains 'planning module') -> fixed JSON plan; the planner only runs when the executor
   asks (plan_task) or loads a skill marked `plan: on` — a plain answer or an ordinary skill makes no plan
 - 'BROWSE' goal -> browser_navigate to TEST_PAGE (default: example.com), then a purchase_confirm (an approval), final
 - 'REMEMBER' goal -> memory_remember then final
 - 'SCHEDULE' goal -> schedule_create then final
 - otherwise -> plain answer
"""
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
# the scripted translations (agent/runtime.rs `rewrite_in_english`): the start of a Chinese reply -> its English
TRANSLATIONS = [("最重要的邮件是 Google Workspace 发来的可疑登录安全告警，建议尽快检查。",
                 "Here is the most important email: the security alert from Google Workspace about a suspicious login.")]


def tc(name, args):
    return {"id": "call_" + uuid.uuid4().hex[:8], "type": "function", "function": {"name": name, "arguments": json.dumps(args)}}


def _fake_lang(text):
    """The fake model's reading of a message's language: "zh", "en" or "" (no letters). A real model reads it; the
    scripted one weighs Chinese characters against Latin letters (~4 letters per English word)."""
    text = re.sub(r"https?://\S+|[A-Z]{4,}[A-Z0-9_]*", " ", text)   # links and the suites' all-caps markers (LANGCHECK …)
    cjk_n = len(re.findall(r"[\u3400-\u9fff]", text))
    lat_n = len(re.findall(r"[A-Za-z]", text))
    if cjk_n + lat_n == 0:
        return ""
    return "zh" if cjk_n / (cjk_n + lat_n / 4) >= 0.3 else "en"


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
    request says `stream: true` — slowly (50 ms a piece) for STREAMSLOW and BOTSTREAM goals so a page can be watched filling up."""
    b = await req.json()
    out = await _chat(req, b)
    if isinstance(out, dict) and b.get("stream"):
        from fastapi.responses import StreamingResponse
        goal = "\n".join(str(m["content"]) for m in b.get("messages") or [] if m.get("role") == "user")
        return StreamingResponse(sse(out, 0.05 if "STREAMSLOW" in goal or "BOTSTREAM" in goal else 0.001), media_type="text/event-stream")
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
    if sys.startswith("You check an assistant's final reply"):   # agent/runtime.rs `review_answer`: the reply holds together
        # (the facts the user must read are not the review's: code appends them after it, agent/show.rs)
        body = str(msgs[-1]["content"])
        answer = body.split("The reply:\n", 1)[-1]
        # the language rule (the review gets the setting): a reply mostly in Chinese while the rule asks for English is
        # against it, unless the request asked for Chinese text to use as written (ZHPOEM stands in for the model
        # understanding the request; a real model reads it)
        request = body.split("The user's request:\n", 1)[-1].split("\n\nThe reply:", 1)[0]
        cjk_n = len(re.findall(r"[\u3400-\u9fff]", answer))
        lat_n = len(re.findall(r"[A-Za-z]", answer))
        zh_reply = cjk_n > 0 and cjk_n / (cjk_n + lat_n / 4) > 0.5
        lang_ok = not ("not written in English" in sys and zh_reply and "ZHPOEM" not in request)
        return reply(json.dumps({"ok": True, "problem": "", "language_ok": lang_ok}))
    if sys.startswith(("You rewrite a reply to the user from a tool's JSON result", "你根据工具的 JSON")):   # `reply_fixed`: the problem fixed
        body = str(msgs[-1]["content"])
        return reply(body.split("\n\nThe reply:\n", 1)[-1].split("\n\nProblem: ", 1)[0].rstrip())
    if sys.startswith("Rewrite the reply to the user in English"):   # `reply_in_language`: the review found it against the rule
        text = str(msgs[-1]["content"]).split("\n\nThe reply:\n", 1)[-1]
        for zh, en in TRANSLATIONS:
            if text.startswith(zh):
                return reply(en + text[len(zh):])
        return reply(text)   # a text the fake has no translation for comes back as it was (no language_fixed event)
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
    if sys.startswith("You classify an assistant's reply"):   # agent/runtime.rs `answer_traits`: wording stands in for meaning
        a = str(msgs[-1]["content"]).lower()
        a = a.split("the assistant's reply:", 1)[-1]   # the request comes first, the reply after it
        asks = "?" in a or re.search(r"\b(please (send|tell|confirm|let me know)|reply with)\b", a)
        return reply(json.dumps({
            "asks_defaults": bool(asks and re.search(r"departure date|next (city|port|destination)|onward|leav(e|ing) .{0,20}on|visa", a)),
            "asks_stay": bool(asks and re.search(r"where (will|do|are) you (stay|staying)|which (of your )?address|hotel|accommodation", a)),
            "tells_user_to_act": bool(re.search(r"\b(press|click|tap|submit|pay|complete|confirm)\b[^.]{0,60}\byourself\b|left [^.]{0,60}for you|"
                                                r"over to you|your turn|you (can|need to|should) (press|click|submit|pay|complete)", a)),
            "reconfirms": bool(re.search(r"\b(please )?confirm (that )?(you want|whether)|shall i go ahead|do you want me to (go ahead|proceed)", a)),
        }))
    if sys.startswith("You name conversations"):   # chat titles (runtime.rs `retitle`): JSON {"title", "lang"}
        last = next((m["content"] for m in reversed(msgs) if m["role"] == "user"), "")
        users = re.findall(r"User: (.*)", str(last))   # the latest request: deterministic, and the title follows the chat
        lang = _fake_lang(users[-1] if users else "") or "en"   # the fake model "reads" the latest message's language
        title_json = lambda t: reply(json.dumps({"title": t, "lang": lang}, ensure_ascii=False))   # noqa: E731
        if "很长很长" in str(last):   # resize_ui.py needs its long first-question title kept: "nothing usable" from the model
            return title_json("")
        for ask, title in ((SHOW_TRIP, "Tokyo trip this week"), (SHOW_CITY, "Moving to Shanghai")):
            if ask in str(last):   # marketing_shots.py: real-looking titles in the pictures
                return title_json(title)
        return title_json("Topic: " + (users[-1].split()[0][:16] if users and users[-1].split() else "chat"))
    if sys.startswith("You make one decision") and "Which language is the user's request written in?" in str(msgs[-1]["content"]):
        # Settings → Reply language = match (agent/lang.rs `request_lang`): the model reads the request's language
        ctx = str(msgs[-1]["content"]).split("Context:\n", 1)[-1].rsplit("\n\nChoices: ", 1)[0]
        return reply(json.dumps({"choice": _fake_lang(ctx) or "other"}))
    if sys.startswith("You read one price from a web page"):   # watch_price.rs `extract_price`: the page's price into JSON
        q = json.loads(msgs[-1]["content"])
        if "Zebra" in q.get("item", ""):   # an item the page does not sell: no price for it
            return reply(json.dumps({"price": None, "currency": "", "item": ""}))
        # the fake model "reads" the price after "Price:" (drawn in pieces: S$ 18 . 00); a real model reads the page
        m = re.search(r"Price:\s*(S\$)\s*(\d+)\s*\.?\s*(\d{2})", q["page"]["text"])
        if not m:
            return reply(json.dumps({"price": None, "currency": "", "item": ""}))
        return reply(json.dumps({"price": float(f"{m.group(2)}.{m.group(3)}"), "currency": m.group(1), "item": q.get("item", "")}))
    if sys.startswith("You read an identity document"):   # /api/profile/ocr (profile_onboarding_ui.py)
        return reply(json.dumps({"document_type": "passport", "number": "E1234567X", "surname": "LU", "given_names": "LUCAS", "name": "LU LUCAS",
                                 "sex": "M", "date_of_birth": "1985-04-12", "nationality": "Australia", "expiry": "2031-05-01", "country": "AUS"}))
    if sys.startswith("You look at a file"):   # file_look / attachment reading
        txt = json.dumps(msgs[-1]["content"], ensure_ascii=False)
        what = "a video contact sheet with numbered frames" if "contact sheet" in txt else "a red square labelled HELLO"
        return reply(f"VISION-SEEN: {what}")
    if sys.startswith("You read the user's own addresses"):   # places_fill.rs: the profile's place codes, once
        return reply(fake_places(msgs[-1]["content"]))
    if sys.startswith("You look at a screenshot"):   # the vision helper behind browser_look
        return reply(fake_vision(msgs[-1]["content"]))
    if sys.startswith(("You locate things", "This is a zoomed-in part", "A red circle with a crosshair")):   # browser_locate
        return reply(fake_locate(sys, msgs[-1]["content"]))
    if "planning module" in sys and SHOW_TRIP in msgs[-1]["content"]:   # marketing_shots.py
        return reply(json.dumps({"objective": "Put this week's Tokyo trip into one timeline", "steps": [
            {"id": "s1", "description": "Find the flight, hotel and restaurant bookings in mail", "tool_hint": "gmail", "risk": "read", "parallel": "p1"},
            {"id": "s2", "description": "Read this week's calendar", "tool_hint": "calendar", "risk": "read", "parallel": "p1"},
            {"id": "s3", "description": "Build the day-by-day timeline and flag gaps and conflicts", "tool_hint": "trip-today", "risk": "read"}]}))
    if "planning module" in sys and SHOW_CITY in msgs[-1]["content"]:
        return reply(json.dumps({"objective": "Brief a family moving to Shanghai on neighbourhoods and rents", "steps": [
            {"id": "s1", "description": "Load the Shanghai city guide", "tool_hint": "shanghai", "risk": "read"},
            {"id": "s2", "description": "Get today's USD/CNY rate", "tool_hint": "market_data", "risk": "read"},
            {"id": "s3", "description": "Compare neighbourhoods, rents and schools", "tool_hint": "shanghai", "risk": "read"}]}))
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
        if "CARDFACT" in msgs[-1]["content"]:   # memory_vault_e2e.py: two cards, one with its number typed in the chat
            items += [{"kind": "card", "fact": "Lucas holds a Citi Prestige card (USA)", "value": "Citi Prestige", "country": "USA"},
                      {"kind": "card", "fact": "Lucas holds a Chase Sapphire Reserve", "value": "Chase Sapphire Reserve", "country": ""},
                      {"kind": "card", "fact": "Lucas's card number", "value": "Visa 4111 1111 1111 1111"}]
        return reply(json.dumps({"items": items}))
    if sys.startswith("You pick which parts of a personal assistant's long-term memory"):   # context::relevance
        return reply(json.dumps(fake_relevance(str(msgs[-1]["content"])), ensure_ascii=False))
    if msgs and str(msgs[-1]["content"]).startswith("You sort a personal assistant's memory facts"):   # memory_tidy::classify
        return reply(json.dumps(fake_sort(str(msgs[-1]["content"])), ensure_ascii=False))
    if msgs and str(msgs[-1]["content"]).startswith("You are tidying"):
        ids = re.findall(r"^(fact_\w+) \| long \| .*Opened zipair", msgs[-1]["content"], re.M)
        quiet = re.findall(r"^(fact_\w+) \| long \| .*\| Lucas likes (?:the )?quiet hotels", msgs[-1]["content"], re.M)
        merge = [{"ids": quiet, "fact": "Lucas likes quiet hotels.", "category": "preference"}] if len(quiet) > 1 else []
        return reply(json.dumps({"demote": [{"id": i, "reason": "process log"} for i in ids], "merge": merge}))
    # the live state (agent/runtime.rs, prompts::live_state) is runtime data at the end, never the user's request
    users = [m["content"] for m in msgs if m["role"] == "user" and not str(m["content"]).startswith(("（系统）", "(System)", '{"state"'))]
    goal = users[-1] if users else ""
    state = live_state(msgs)
    tools_done = [m for m in msgs if m["role"] == "tool"]
    n = len(tools_done)
    last_tool = tools_done[-1]["content"] if tools_done else ""
    allu = "\n".join(str(m["content"]) for m in msgs if m["role"] == "user")
    tnames = [t["function"]["name"] for t in (b.get("tools") or [])]
    # the scripted model decides from the transcript's SHAPE, never from the words of a system note: the tools it was
    # offered (`tnames`), the tools it has called (`called`), the count of results (`n`), and whether an earlier final
    # answer of its own is already in the transcript (`finals`: the runtime put it back when it asked again)
    called = [c["function"]["name"] for m in msgs if m.get("role") == "assistant" for c in (m.get("tool_calls") or [])]
    finals = [m for m in msgs if m.get("role") == "assistant" and not m.get("tool_calls") and str(m.get("content") or "").strip()]
    if tools_done and str(tools_done[-1].get("tool_call_id") or "").startswith("res_"):
        head = ""
        if "BOTKID1" in goal and "You are the Travel bot" in sys:   # bots_e2e.py: the bot's answer Main shows and follows up on
            d = re.search(r"DATE=(20\d\d-\d\d-\d\d)", allu)
            head = f"TRAVELBOT ANSWER {d.group(1) if d else ''}: Singapore Airlines SQ638 SGD 620 direct; Cathay Pacific via HKG SGD 510.\n"
        return workflow_reply(tools_done[-1]["content"], head)
    bk = bots_flow(goal, sys, msgs, tools_done, n, allu)
    if bk is not None:
        return bk
    sk = travel_skill(goal, sys, msgs, tools_done, n, last_tool, tnames)
    if sk is not None:
        return sk
    if "FAKECLAIM" in goal:   # says it sent an email without sending anything (outcome check, roadmap batch 1)
        if finals:   # asked again (its claim is back in the transcript): the honest answer
            return reply("我还没有发送邮件：这个任务里没有执行发送，下面是准备好的草稿内容。")
        return reply("好的，邮件已发送给 Jennifer。")
    if "ZHPOEM" in goal:   # a Chinese request for a poem: the system prompt's exceptions line keeps it in Chinese
        if "text the user asked for in Chinese" in sys or "语言：简体中文" in sys:
            return reply("《雨季》\n午后的雷声敲开乌节路的天空，\n雨落在组屋的晾衣竿上，\n城市在水汽里慢慢醒来。")
        return reply("Monsoon: the afternoon thunder cracks the sky over Orchard Road, and the city wakes slowly in the steam.")
    if "ZHANSWER" in goal:   # a model that answers in Chinese although the setting is English
        if finals:   # asked again (its Chinese answer is back in the transcript): the English one
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
    if "STREAMTEST" in goal or "STREAMSLOW" in goal:   # stream_e2e.py: a long answer after hidden reasoning
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
    if "BUDGETPDF" in goal:   # a research task that reads pages as long as the page tools are offered (the step budget
        # is a tool-list limit: agent/runtime.rs `budget` withdraws them), then delivers
        if "browser_navigate" in tnames:
            return reply("", [tc("browser_navigate", {"url": PAGE + f"?p={n}"})])
        if "make_pdf" not in called and "make_pdf" in tnames:
            return reply("", [tc("make_pdf", {"markdown": "# 调研简报\n\n- Kuriya Dining 19:00\n- Shinzo 19:30", "output": "reports/brief.pdf"})])
        if "send_file" not in called and "send_file" in tnames:   # a second send of a file already in the chat is refused by code
            return reply("", [tc("send_file", {"path": "reports/brief.pdf", "note": "简报"})])
        return reply(f"BUDGET DONE after {n} tool results; tools offered at the end: {','.join(sorted(tnames))}")
    if "SHOPCART" in goal:   # the 2026-09-29 Amazon run: find a case on a long page, add it to the cart
        # a real model would reason from the tool results; this one follows the path the skill describes
        def ref_after(pattern, text):
            m = re.search(r"\[(e\d+)\][^\n]*" + pattern, text, re.I)
            return m.group(1) if m else None
        if n == 0:
            return reply("", [tc("browser_navigate", {"url": PAGE + "?k=iphone+17+pro+max+case"})])
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
    if "WEBLOOP" in goal:   # 2026-10-02 R4-04b: endless searching for a cleaner product list — the web budget
        # (guards.rs WEB_NUDGE = 18 web calls) ends it: the model wraps up once it has made that many, or as soon as
        # the search tool is no longer offered
        if n >= 18 or "web_search" not in tnames:
            return reply(f"WRAPPED after {n} calls")
        return reply("", [tc("web_search", {"query": f"anker power bank variant {n}"})])
    if "REMAKEXLSX" in goal:   # the 2026-10-02 itinerary run: the same Excel file re-made again and again
        if n < 6:
            return reply("", [tc("make_xlsx", {"output": "trip.xlsx", "sheets": [{"name": "Plan", "columns": ["Day", "Cost"],
                                                                                   "rows": [["D1", 100 + n]]}]})])
        return reply("REMAKE DONE:\n" + "\n=====\n".join(str(m["content"])[:400] for m in tools_done))
    if "MARKUPFINAL" in goal:   # the 2026-10-02 Tesla run: a reply that was only tool-call markup (cut off, so no call
        # could be read from it) — the runtime strips it and writes the answer with its tool-free reply call (rescue_final)
        if not b.get("tools"):
            return reply("MARKUP FIXED: plain answer")
        if n == 0:
            return reply("", [tc("browser_navigate", {"url": PAGE + "?m=0"})])
        return reply('<tool_call>\n<function=update_plan>\n<parameter=steps>\n[{"id": "s1", "status": "done"}]\n')
    if "SLOWTASK" in goal:   # a slow research task: reads pages while browser_navigate is offered (past 70% of the time
        # budget only the finishing tools are: agent/runtime.rs `budget`), then keeps notes until no tool is offered
        if not b.get("tools"):
            return reply(f"SLOW SUMMARY after {n} results; navigate withdrawn before the limit: {'files_write' in called}")
        import asyncio as _a
        await _a.sleep(4)
        if "browser_navigate" in tnames:
            return reply("", [tc("browser_navigate", {"url": PAGE + f"?page={n}"})])
        return reply("", [tc("files_write", {"path": f"notes/slow{n}.md", "content": f"notes {n}"})])
    if "STUCKLOOP" in goal:   # the 2026-10-02 HSBC run: the same Google Finance link, 3 copies per turn, forever
        if not b.get("tools"):
            return reply(f"STUCK SUMMARY after {n} tool results; no tools offered: {not tnames}")
        url = PAGE + "?quote=HSBC&window=5Y"
        return reply("", [tc("browser_navigate", {"url": url}), tc("browser_navigate", {"url": url}),
                          tc("browser_navigate", {"url": url})])
    if "SWITCHSRC" in goal:   # a source that keeps failing: after 3 failures the host is blocked, the agent switches site
        dead_seen = any(d.get("host") == "nosuch.test" for d in (state.get("dead_ends") or []))
        if n < 4:
            return reply("", [tc("browser_navigate", {"url": f"http://nosuch.test:8099/quote{n}"})])
        if n == 4:
            return reply("", [tc("browser_navigate", {"url": PAGE})])
        return reply(f"SWITCH OK dead_ends_in_prompt={dead_seen} last={last_tool[:120]}")
    if "LOOPFOREVER" in goal:   # a model that never ends (stop_ui.py stops it from the chat): a harmless new note every
        # turn, so no loop guard ends it; a short think per turn so the page can be watched
        import asyncio as _a
        await _a.sleep(0.4)
        return reply("", [tc("files_write", {"path": f"notes/loop{n}.md", "content": f"round {n}"})])
    if "LOOPFEED" in goal:   # a model stuck re-opening the same RSS feed (the 2026-09-28 news-brief run)
        if not b.get("tools"):
            return reply(f"LOOP SUMMARY after {n} tool calls: feed read, email not sent.")
        return reply("", [tc("browser_navigate", {"url": PAGE.rsplit('/', 1)[0] + "/feed.xml"})])
    if "LEARNSITE" in goal:      # batch 2: a habit learned by OMuse itself
        if n == 0:
            return reply("", [tc("memory_remember", {"fact": "shop.test allows guest checkout; the phone goes in as 8 digits.", "domain": "site",
                                                     "entity": "shop.test", "topics": ["shopping", "forms"], "scope": "topic"})])
        return reply(f"Learned. {last_tool[:200]}")
    if "BROWSE" in goal:
        if n == 0:
            return reply("", [tc("browser_navigate", {"url": PAGE})])
        if n == 1:   # an approval with no page of ours behind it: the purchase confirmation always asks the user
            return reply("", [tc("purchase_confirm", {"site": "example.com", "items": [{"name": "Test item", "qty": 1, "price": 1}],
                                                      "total": 1, "currency": "SGD"})])
        return reply(f"Browsed. Last: {last_tool[:300]}")
    if "TAKEOVERLOOP" in goal:   # the site is still blocked after a hand-back: asks once more, then gives up
        if n < 2:
            return reply("", [tc("browser_request_takeover", {"reason": "still blocked on zipair"})])
        return reply(f"Gave up: {last_tool[:200]}")
    if "TAKEOVER" in goal:
        if n == 0:
            return reply("", [tc("browser_request_takeover", {"reason": "please log in"})])
        return reply(f"After takeover: {last_tool[:200]}")
    if "REMEMBER" in goal:
        if n == 0:
            return reply("", [tc("memory_remember", {"fact": "Lucas likes quiet hotels", "category": "preference"})])
        return reply("好的，我记住了。")
    if "SCHEDULE" in goal:
        if n == 0:
            return reply("", [tc("schedule_create", {"name": "daily brief", "goal": "summarize email", "kind": "cron", "spec": "0 8 * * *"})])
        return reply(f"Created: {last_tool}")
    if "XMLTOOL" in goal:
        if n == 0:
            return reply('<tool_call>{"name": "files_write", "arguments": {"path": "notes/a.md", "content": "hello"}}</tool_call>')
        return reply("<think>hidden</think>Wrote file: " + last_tool)
    return reply("<think>reasoning here</think>你好！这是一个普通回答。")


def live_state(msgs):
    """The runtime's live state (`{"state": {...}}`, the last message when there is a plan or a dead end), or {}."""
    last = msgs[-1] if msgs else {}
    if last.get("role") == "user" and str(last.get("content") or "").startswith('{"state"'):
        try:
            return json.loads(last["content"])["state"]
        except (ValueError, KeyError):
            return {}
    return {}


@app.get("/calls_for")
async def calls_for(marker: str):
    """Executor requests whose conversation contains `marker`: the system prompt and the last message of each."""
    out = []
    for b in CALLS:
        ms = b.get("messages") or []
        if any(marker in str(m.get("content")) for m in ms if m.get("role") == "user") and ms and ms[0].get("role") == "system" \
                and "planning module" not in str(ms[0].get("content")) \
                and not str(ms[0].get("content")).startswith("You classify an assistant's reply") \
                and not str(ms[0].get("content")).startswith("You pick which parts of a personal assistant's long-term memory"):   # answer_traits, context::relevance
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


def fake_places(content):
    """Stand-in for the one-time place fill: the city and ISO country codes of the profile's addresses (a fixed table
    of the test addresses' cities)."""
    m = re.search(r"Addresses: (\{.*?\})\n", content, re.S)
    addrs = json.loads(m.group(1)) if m else {}
    table = [("hong kong", "HKG", "HK"), ("singapore", "SIN", "SG"), ("sydney", "SYD", "AU"), ("tokyo", "TYO", "JP"),
             ("bangkok", "BKK", "TH"), ("london", "LON", "GB"), ("new york", "NYC", "US")]
    out = {"home_city": "", "countries": {}}
    for k, v in addrs.items():
        hit = next((t for t in table if t[0] in str(v).lower()), None)
        out["countries"][k] = hit[2] if hit else ""
        if k == "address_home" and hit:
            out["home_city"] = hit[1]
    return json.dumps(out)


def fake_locate(sys, content):
    """Stand-in vision model for browser_locate: finds the target by its colour (blue chat bubble / yellow message box)
    and answers the JSON decision — {"choice": <grid cell>}, {"choice": <zoomed cell number>} or {"on_item": bool} for
    the red marker — exercising the real geometry."""
    import base64, io
    from PIL import Image
    parts = content if isinstance(content, list) else []
    text = " ".join(p.get("text", "") for p in parts if p.get("type") == "text")
    find = text.split("Choices:")[0].lower()
    url = next((p["image_url"]["url"] for p in parts if p.get("type") == "image_url"), "")
    img = Image.open(io.BytesIO(base64.b64decode(url.split(",", 1)[1]))).convert("RGB")
    want = (0, 102, 255) if "bubble" in find else (255, 235, 59) if "message" in find else None
    marker = sys.startswith("A red circle")
    none = json.dumps({"on_item": False} if marker else {"choice": "NONE"})
    if not want:
        return none
    W, H = img.size
    px = img.load()
    xs = ys = cnt = 0
    for y in range(0, H, 2):
        for x in range(0, W, 2):
            r, g, b = px[x, y]
            if abs(r - want[0]) < 45 and abs(g - want[1]) < 45 and abs(b - want[2]) < 45:
                xs += x; ys += y; cnt += 1
    if cnt < 8:
        return none
    cx, cy = xs / cnt, ys / cnt
    if sys.startswith("You locate things"):
        return json.dumps({"choice": "ABCDEFGH"[min(7, int(cx / (W / 8)))] + str(min(6, int(cy / (H / 6)) + 1))})
    if sys.startswith("This is a zoomed-in part"):
        return json.dumps({"choice": str(min(5, int(cy / (H / 6))) * 8 + min(7, int(cx / (W / 8))) + 1)})
    rx = ry = rn = 0
    for y in range(0, H, 2):
        for x in range(0, W, 2):
            r, g, b = px[x, y]
            if r > 210 and g < 50 and b < 50:
                rx += x; ry += y; rn += 1
    if not rn:
        return none
    d = ((rx / rn - cx) ** 2 + (ry / rn - cy) ** 2) ** 0.5
    return json.dumps({"on_item": d < 25})


# marketing_shots.py: natural prompts (no test markers in the pictures) with presentable answers
STREAM_STORY = ("Once upon a time, in a small town by the sea, there lived a lighthouse keeper named Mara. " * 6 +
                "Every evening she climbed the one hundred and twelve steps, lit the lamp, and watched the fishing boats come home. "
                "One stormy night a boat did not return, and Mara kept the lamp burning until dawn, when the boat finally appeared on the horizon. "
                "The fishermen said the light had guided them home. THE END.")
SHOW_TRIP = "What's on my Tokyo trip this week? Check my mail and calendar."
SHOW_CITY = "We're moving to Shanghai for work with two kids. Where do expat families live, and what does rent cost?"


def route_reply(user):
    """The router's JSON (bots_e2e.py): BOTHAND / BOTAPPR / BOTWAIT go to Travel with the kid's marker as the
    question, BOTROUTEFAIL gets no JSON (the executor decides with
    ask_bot), BOTROUTEBAD names a bot that does not exist; everything else stays in Main."""
    req = user.split("The user's new request:\n", 1)[-1]
    date = (re.search(r"DATE=(20\d\d-\d\d-\d\d)", req) or re.search(r"(20\d\d-\d\d-\d\d)", "x 2026-12-20")).group(1)
    if "BOTROUTEFAIL" in req:
        return reply("I am not sure.")
    if "BOTROUTEBAD" in req:
        return reply(json.dumps({"bot": "no-such-bot", "question": "x"}))
    # BOTSLOW / BOTSTREAM (bots_ui.py) go on to the bot: it works a while / streams its answer slowly
    slow = "".join(" " + m for m in ("BOTSLOW", "BOTSTREAM") if m in req)
    if "BOTSLOW" in req:   # bots_ui.py: the routing takes a while, so Main's timer must be longer than the bot's by that much
        time.sleep(2)
    for marker, kid in (("BOTHAND", f"BOTKID1 cheapest flight Singapore to Tokyo on {date} DATE={date}"),
                        ("BOTAPPR", "BOTKIDAPPR book the SQ638 for SGD 620"), ("BOTWAIT", "BOTKIDWAIT log in to the airline site")):
        if marker in req:
            if marker == "BOTHAND" and "BOTSTREAM" in req:   # bots_ui.py: a question (the model answers, streamed) — a dated search would be code's
                kid = "BOTKID1 which airlines fly Singapore to Tokyo direct?"
            return reply(json.dumps({"bot": "travel", "question": kid + slow}))
    if "BOTCONVBAD" in req:   # a conversation that is not that bot's: the code starts a new one
        return reply(json.dumps({"bot": "travel", "question": f"BOTKID1 checked conv DATE={date}", "conversation_id": "conv_nope"}))
    return reply('```json\n{"bot": "main"}\n```')


def bots_flow(goal, sys, msgs, tools_done, n, allu):
    """bots_e2e.py: the main bot hands a request to the Travel bot (ask_bot) and shows its answer; a follow-up
    goes back to the same bot conversation; an approval and a takeover inside the bot."""
    main = "## Bots" in sys
    travel = "You are the Travel bot" in sys
    date = (re.search(r"DATE=(20\d\d-\d\d-\d\d)", allu) or re.search(r"(20\d\d-\d\d-\d\d)", "x 2026-12-20")).group(1)
    if main and n == 0:
        for marker, kid in (("BOTHAND", f"BOTKID1 cheapest flight Singapore to Tokyo on {date} DATE={date}"),
                            ("BOTAPPR", "BOTKIDAPPR book the SQ638 for SGD 620"), ("BOTWAIT", "BOTKIDWAIT log in to the airline site")):
            if marker in goal:
                return reply("", [tc("ask_bot", {"bot": "travel", "question": kid})])
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
    if travel and "BOTSKILL" in goal:   # Main's load_skill of the flight skill, handed over by code (agent/exec.rs)
        return reply("TRAVELBOT SKILL ANSWER: handed over with the flight skill's request.")
    if travel and "BOTKID1" in goal:
        if n == 0:
            if "BOTSLOW" in goal:   # bots_ui.py: the bot works a while (its one model call, choosing the skill; code runs the rest)
                time.sleep(2)
            if "BOTSTREAM" in goal:   # bots_ui.py: a model-written answer streams into Main (code's answers are not streamed)
                return reply(f"TRAVELBOT ANSWER {date}: Singapore Airlines SQ638 SGD 620 direct; Cathay Pacific via HKG SGD 510.")
            return reply("", [tc("load_skill", {"name": "flight-booking"})])
        if n == 1:   # the search is ONE workflow call; code runs it (the reply to its result: `workflow_reply`)
            return reply("", [tc("travel_search", {"kind": "flight", "origin": "SIN", "destination": "TYO", "date": date})])
        return reply(f"TRAVELBOT ANSWER {date}: Singapore Airlines SQ638 SGD 620 direct; Cathay Pacific via HKG SGD 510.")
    if travel and "BOTKIDAPPR" in goal:
        if n == 0:
            return reply("", [tc("purchase_confirm", {"site": "www.singaporeair.com", "items": [{"name": "Flight SQ638 SIN-NRT", "price": 620}],
                                                      "total": 620, "currency": "SGD"})])
        return reply("TRAVELBOT BOOKED: the purchase of SQ638 was approved.")
    if travel and "BOTKIDWAIT" in goal:
        if n == 0:
            return reply("", [tc("browser_request_takeover", {"reason": "please log in to the airline site"})])
        return reply("TRAVELBOT DONE after the takeover.")
    return None


# ---------------------------------------------------------------- workflow tools (agent/booking.rs, docs/json-tools.md)
# The fake stands in for the model's understanding: a marker becomes ONE workflow call with the request's
# details as JSON arguments (the skill loaded first, as a model does); code runs every step; the result's JSON comes back
# as `<tool>_result` and the reply is written from it (`workflow_reply`).


def workflow_reply(content, head=""):
    """The reply written from a machine's JSON result: its status, its hints (the wording help; the `show` facts are
    appended by code after the reply), the next search offered."""
    try:
        res = json.loads(content)
    except ValueError:
        return reply("RESULT unreadable: " + str(content)[:200])
    lines = [f"RESULT {res.get('status')}"] + [str(x) for x in res.get("hints") or []]
    for item in res.get("next_items") or []:
        lines.append(f"Shall I search the {'rental car' if item == 'car' else item} now?")
    if res.get("next_question") == "search_all_airlines":
        lines.append("Shall I search all airlines?")
    return reply(head + "\n".join(lines))


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
    return None


# ---- memory: the two JSON readings a real model makes (context::relevance, memory_tidy::classify). The fake stands in
# for the model's understanding with word lists — a test double only; the product's code never reads words.
FAKE_TOPICS = [
    ("clothing", r"鞋|码|衣|shoe|size|shirt|sneaker"),
    ("shopping", r"买|购物|挑|结账|访客|shop|buy|cart|checkout|guest|order"),
    ("travel", r"机票|航班|酒店|出差|旅行|flight|hotel|trip|travel|business class|cabin"),
    ("dining", r"餐|吃|菜|restaurant|dinner|cuisine|food|vegetarian|drink"),
    ("gifts", r"礼物|生日|gift|present|birthday|vinyl|jazz"),
    ("documents", r"报告|简报|pdf|report|brief"),
    ("news", r"新闻|news"),
    ("email", r"邮件|email|mail"),
    ("forms", r"填|表|form|phone|电话"),
]


def fake_topics(text):
    return [k for k, rx in FAKE_TOPICS if re.search(rx, text, re.I)]


def fake_relevance(body):
    req = body.split("The request:\n", 1)[-1].split("\nChoices:\n", 1)[0]
    try:
        ch = json.loads(body.split("\nChoices:\n", 1)[1])
    except Exception:
        return {"topics": [], "entities": [], "triggers": [], "facts": []}
    low = req.lower()
    rel = {"wife": r"wife|太太|老婆|妻子", "husband": r"husband|丈夫|老公"}
    ents = []
    for e in ch.get("entities") or []:
        names = [e["name"]] + list(e.get("aliases") or [])
        r = (e.get("relation") or "").lower()
        if any(n.lower() in low for n in names if len(n) >= 2) or any(re.search(rx, low) for k, rx in rel.items() if k in r or r in rx):
            ents.append(e["name"])
    return {"topics": fake_topics(req), "entities": ents,
            "triggers": [t for t in ch.get("triggers") or [] if t.lower() in low], "facts": []}


def fake_sort(body):
    out = []
    for line in body.split("\nFacts:\n", 1)[-1].splitlines():
        parts = [p.strip() for p in line.split(" | ", 3)]
        if len(parts) != 4:
            continue
        fid, dom, ent, fact = parts
        host = re.search(r"\b([a-z0-9-]+\.(?:test|sg|com|io))\b", fact, re.I)
        trig = re.search(r"(?:说|say|says)\s*[「“\"']([^」”\"']{2,30})[」”\"']", fact, re.I)
        if not dom:
            dom = "site" if host and re.search(r"网站|结账|checkout|guest|digits|位", fact, re.I) else "rule" if trig else "preference"
        out.append({"id": fid, "domain": dom, "topics": fake_topics(fact) or ["work"],
                    "scope": "trigger" if trig else "always" if re.search(r"assistant|OMuse|助手", fact) else "topic",
                    "trigger": trig.group(1) if trig else "", "entity": ent or (host.group(1) if host else ""),
                    "relation": "", "id_document": bool(re.search(r"passport|护照|出生|birth", fact, re.I))})
    return {"facts": out}
