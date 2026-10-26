"""Video generation (make_video) through the OpenAI-style Videos API: the job is started, polled and downloaded, the
clip lands in videos/<title>.mp4 and is shown in the chat with a player. The video endpoint and its key are OPTIONAL
and separate from the model's: here the user's own model set points the videos at a second provider
(/vid/v1 on the fake server) with its own key, so the request, the polls and the download must carry THAT key while
the chat requests carry the model key. Chat options (service_tier, reasoning_effort) never reach the Videos API.
Then: blank endpoint and key = the model endpoint and key; a declined prompt is explained; the per-task budget holds;
Settings → test models reports the video model; the keys never come back from the API."""
import sys
import time

import httpx

B = "http://127.0.0.1:8080"
LLM = "http://127.0.0.1:8090"
H = {"X-Persona-UI": "1"}
c = httpx.Client(timeout=60, trust_env=False)
fails = []


def check(name, cond, info=""):
    print(("PASS " if cond else "FAIL ") + name, "" if cond else str(info)[:800], flush=True)
    if not cond:
        fails.append(name)


def run(msg, timeout=90):
    r = c.post(B + "/api/chat", headers=H, json={"message": msg}).json()
    t = {}
    for _ in range(int(timeout / 0.3)):
        t = c.get(f"{B}/api/tasks/{r['task_id']}").json()
        if t["status"] in ("COMPLETED", "FAILED", "CANCELLED"):
            break
        time.sleep(0.3)
    return r, t


def events(t, kind):
    return [e["data"] for e in t.get("events", []) if e["type"] == kind]


def vcalls():
    return c.get(LLM + "/video_calls").json()


# ---- own model, videos from a separate provider with its own key
r = c.put(B + "/api/settings", headers=H, json={"language": "en", "model_override": True, "model_base_url": LLM + "/v1",
                                                 "model_api_key": "own-key", "model_name": "fake",
                                                 "video_base_url": LLM + "/vid/v1/", "video_api_key": "vid-key", "video_model": "sora-2",
                                                 "fast_mode": True, "reasoning_effort": "high"})
s = r.json().get("settings", {})
check("own model with a separate video endpoint and key saved (URL trimmed, keys only as *_set)",
      r.status_code == 200 and s.get("video_base_url") == LLM + "/vid/v1" and s.get("video_api_key_set") is True
      and "video_api_key" not in s and "model_api_key" not in s, r.text[:400])
g = c.get(B + "/api/settings", headers=H).json()
check("the form shows the video endpoint and model, the key only as set",
      g["model_user"]["video_base_url"] == LLM + "/vid/v1" and g["model_user"]["video_model"] == "sora-2"
      and g["model_user"]["video_api_key_set"] is True and "video_api_key" not in g["model_user"], g["model_user"])

n0 = len(vcalls())
r, t = run("VIDEOGEN make a short clip of a paper boat")
names = [e["data"]["name"] for e in t.get("events", []) if e["type"] == "tool_call"]
check("the task made a clip", t["status"] == "COMPLETED" and names.count("make_video") == 1 and "VIDEO DONE" in str(t.get("result")),
      (t["status"], t.get("error"), names, str(t.get("result"))[:200]))
vc = vcalls()[n0:]
posts = [x for x in vc if x["url"] == "videos"]
polls = [x for x in vc if x["url"] == "poll"]
content = [x for x in vc if x["url"] == "content"]
check("the Videos API got one job request, polls and one download — all at the video endpoint",
      len(posts) == 1 and len(polls) >= 2 and len(content) == 1 and all(x["prefix"] == "/vid/v1" for x in vc), vc)
body = posts[0]["body"] if posts else {}
check("job request: the video model, seconds and size; no service_tier, no reasoning_effort",
      body.get("model") == "sora-2" and body.get("prompt", "").startswith("a paper boat") and body.get("seconds") == "4"
      and body.get("size") == "1280x720" and "service_tier" not in body and "reasoning_effort" not in body, body)
check("every video call carried the video endpoint's key, not the model key",
      vc and all(x["auth"] == "Bearer vid-key" for x in vc), [x["auth"] for x in vc])
last = c.get(LLM + "/calls").json()["last"] or {}
check("the task's chat requests used the model key and did carry the chat options",
      c.get(LLM + "/last_auth").json()["authorization"] == "Bearer own-key" and last.get("service_tier") == "fast"
      and last.get("reasoning_effort") == "high", (c.get(LLM + "/last_auth").json(), {k: last.get(k) for k in ("service_tier", "reasoning_effort")}))
ev = events(t, "video")
check("a video event names the clip, model, size and length",
      len(ev) == 1 and ev[0]["paths"] == ["videos/paper_boat.mp4"] and ev[0]["model"] == "sora-2" and ev[0]["size"] == "1280x720"
      and ev[0]["seconds"] == 4, ev)
raw = c.get(B + "/api/files/raw", params={"path": "videos/paper_boat.mp4"}, headers=H)
check("the clip is in the workspace as an MP4", raw.status_code == 200 and raw.content[4:8] == b"ftyp"
      and raw.headers.get("content-type", "").startswith("video/"), (raw.status_code, raw.headers.get("content-type"), raw.content[:16]))
conv = c.get(f"{B}/api/conversations/{r['conversation_id']}", headers=H).json()
cards = [m["content"] for m in conv["messages"] if m["role"] == "system" and '"type": "file"' in m["content"]]
check("the chat got one file card with the video (mime video/mp4 → a player)",
      len(cards) == 1 and "videos/paper_boat.mp4" in cards[0] and "video/mp4" in cards[0], cards)
check("the tool told the model not to send it again", "Don't send it again" in str(t.get("result")), str(t.get("result"))[:300])

# ---- the per-task budget (config: 2 clips): a third call in one task is refused without a request
r, t = run("VIDEOGEN twice a boat and then closer")
names = [e["data"]["name"] for e in t.get("events", []) if e["type"] == "tool_call"]
res = [e["data"] for e in t.get("events", []) if e["type"] == "tool_result"]
check("two clips in one task are within the budget", t["status"] == "COMPLETED" and names.count("make_video") == 2
      and len(events(t, "video")) == 2, (names, t.get("error")))
vc2 = vcalls()
n_posts = len([x for x in vc2 if x["url"] == "videos"])
check("the second clip got a new file name", events(t, "video")[1]["paths"][0].startswith("videos/") and
      events(t, "video")[1]["paths"][0] != "videos/paper_boat.mp4", events(t, "video"))

# ---- blank video endpoint and key: the model endpoint and the model key
c.put(B + "/api/settings", headers=H, json={"model_override": True, "video_base_url": "", "video_api_key": None, "video_model": "sora-2"})
g = c.get(B + "/api/settings", headers=H).json()
check("null clears the saved video key; blank endpoint = the model endpoint",
      g["model_user"]["video_api_key_set"] is False and g["model_user"]["video_base_url"] == "", g["model_user"])
n0 = len(vcalls())
r, t = run("VIDEOGEN another boat")
vc = vcalls()[n0:]
check("now the videos go to the model endpoint with the model key", t["status"] == "COMPLETED" and vc
      and all(x["prefix"] == "/v1" and x["auth"] == "Bearer own-key" for x in vc), [(x["prefix"], x["auth"]) for x in vc])

# ---- a declined prompt: explained once, no retry
n0 = len(vcalls())
r = c.post(B + "/api/chat", headers=H, json={"message": "VIDEOGEN storm at sea"})
t = {}
for _ in range(200):
    t = c.get(f"{B}/api/tasks/{r.json()['task_id']}").json()
    if t["status"] in ("COMPLETED", "FAILED", "CANCELLED"):
        break
    time.sleep(0.3)
res = [e["data"] for e in t.get("events", []) if e["type"] == "tool_result"]
posts = [x for x in vcalls()[n0:] if x["url"] == "videos"]
check("a prompt the safety policy declines is reported to the model once, with no retry",
      len(posts) == 1 and any("safety policy" in str(x) for x in res) and not events(t, "video"), (len(posts), res[:1]))

# ---- Settings → test models: the video row (reachability, nothing generated)
r = c.post(B + "/api/settings/test-models", headers=H, json={}).json()
by = {x["kind"]: x for x in r["results"]}
check("test-models: a named video model the endpoint does not list is reported (nothing generated)",
      by.get("video", {}).get("ok") is False and by["video"]["model"] == "sora-2" and "does not list sora-2" in by["video"].get("error", ""), by.get("video"))
c.put(B + "/api/settings", headers=H, json={"model_override": True, "video_base_url": LLM + "/vid/v1", "video_api_key": "vid-key"})
n0 = len(vcalls())
r = c.post(B + "/api/settings/test-models", headers=H, json={}).json()
by = {x["kind"]: x for x in r["results"]}
check("test-models reports the video model at its endpoint with its key (no clip generated)",
      by.get("video", {}).get("ok") is True and by["video"]["model"] == "sora-2" and "not generated" in by["video"].get("reply", "")
      and [x for x in vcalls()[n0:] if x["url"] == "models" and x["auth"] == "Bearer vid-key"]
      and not [x for x in vcalls()[n0:] if x["url"] == "videos"], (by.get("video"), vcalls()[n0:]))
n0 = len(vcalls())
r = c.post(B + "/api/settings/test-models", headers=H, json={"model_override": True, "video_base_url": LLM + "/vid/v1", "video_api_key": "form-key",
                                                            "video_model": ""}).json()
by = {x["kind"]: x for x in r["results"]}
vc = vcalls()[n0:]
check("the form's unsaved video endpoint and key are tested, with auto-pick from its model list",
      by.get("video", {}).get("ok") is True and by["video"].get("resolved_model") == "sora-2"
      and any(x["url"] == "models" and x["auth"] == "Bearer form-key" for x in vc), (by.get("video"), vc))

# ---- the system's models: no video model on the fake endpoint, said plainly; keys never logged
c.put(B + "/api/settings", headers=H, json={"model_override": False})
r, t = run("VIDEOGEN a boat with the system model")
res = [e["data"] for e in t.get("events", []) if e["type"] == "tool_result"]
check("with no video model configured the model is told to stop and point at Settings",
      any("video generation is not set up" in str(x) for x in res) and not events(t, "video"), res[:1])
ev = c.get(B + "/sentinel/api/audit?limit=100", headers=H).json()
ev = ev.get("events") or ev.get("entries") or ev
txt = str(ev)
check("the audit log never carries a key", "vid-key" not in txt and "own-key" not in txt and "form-key" not in txt)

print("\n" + ("ALL PASS" if not fails else f"{len(fails)} FAILED: {fails}"))
sys.exit(1 if fails else 0)
