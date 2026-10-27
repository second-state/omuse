"""Streamed answers: with Settings `stream_answers` on (default) the executor's call is `stream: true`, and the page's
event stream gets `answer_delta` events with the visible text so far while the model writes — never the
`<think>…</think>` reasoning, which is held back until the answer proper begins; a turn that carries tool calls is
flagged (`calls`) so its commentary is not shown as an answer; the last delta equals the stored answer and comes before
the task completes. With the setting off the request is plain and no delta is sent."""
import json
import threading

import httpx

from tests import skills_common as K
from tests.skills_common import check, finish

LLM = "http://127.0.0.1:8090"
EV = []
STOP = threading.Event()


def listen():
    with httpx.Client(timeout=None, trust_env=False) as cl, cl.stream("GET", K.B + "/api/stream", headers=K.H) as r:
        for line in r.iter_lines():
            if STOP.is_set():
                break
            if line.startswith("data:"):
                try:
                    EV.append(json.loads(line[5:]))
                except ValueError:
                    pass


th = threading.Thread(target=listen, daemon=True)
th.start()
K.c.put(K.B + "/api/settings", json={"language": "en", "stream_answers": True}, headers=K.H)

t = K.run("STREAMSLOW tell me a bedtime story")   # the scripted model streams 16 characters every 50 ms
deltas = [e for e in EV if e.get("kind") == "answer_delta" and e.get("task_id") == t["id"]]
texts = [d["text"] for d in deltas]
check("the task completed with the story", t["status"] == "COMPLETED" and "THE END." in (t.get("result") or ""), (t["status"], t.get("error")))
ex = K.c.get(LLM + "/calls_for", params={"marker": "STREAMSLOW tell me"}).json()
check("the executor's request streamed (stream: true, usage asked for); the helper calls after the answer did not",
      ex and all((c["stream"] is True and c["stream_options"] == {"include_usage": True}) == c["has_tools"] for c in ex)
      and any(c["has_tools"] for c in ex), [(c["has_tools"], c["stream"], c["stream_options"]) for c in ex])
check("many deltas arrived (at most ten a second), each a longer prefix of the next", 5 <= len(deltas) <= 40
      and all(texts[i + 1].startswith(texts[i]) for i in range(len(texts) - 1)) and len(texts[-1]) > len(texts[0]), (len(deltas), [len(x) for x in texts]))
check("the reasoning never showed", all("<think>" not in x and "keep it short" not in x for x in texts), [x[:80] for x in texts[:3]])
check("the last delta is the stored answer", texts and texts[-1].strip() == (t.get("result") or "").strip(), (texts[-1][-80:] if texts else None, (t.get("result") or "")[-80:]))
check("the deltas carry the conversation and are not stored as task events",
      all(d.get("conv_id") == t["_conv"] for d in deltas) and not any(e["type"] == "answer_delta" for e in t["events"]), [e["type"] for e in t["events"]])
# order: the deltas came before the task's final update
idx_final = next((i for i, e in enumerate(EV) if e.get("kind") == "task_update" and e["task"]["id"] == t["id"] and e["task"]["status"] == "COMPLETED"), None)
idx_last = max((i for i, e in enumerate(EV) if e.get("kind") == "answer_delta" and e.get("task_id") == t["id"]), default=None)
check("every delta came before the completed update", idx_final is not None and idx_last is not None and idx_last < idx_final, (idx_last, idx_final))

# a turn with commentary and a tool call: flagged, so the page does not show the commentary as an answer
t = K.run("STREAMTEST STREAMCALL tell me a bedtime story")
deltas = [e for e in EV if e.get("kind") == "answer_delta" and e.get("task_id") == t["id"]]
check("a tool-call turn: its text is flagged with calls=true", any(d["calls"] and "look at the workspace" in d["text"] for d in deltas), [(d["calls"], d["text"][:40]) for d in deltas][:4])
check("the answer turn follows with calls=false and the full story", deltas and not deltas[-1]["calls"] and deltas[-1]["text"].strip() == (t.get("result") or "").strip()
      and K.names(t) == ["files_list"], (K.names(t), deltas[-1]["calls"] if deltas else None))

# off: a plain request, no deltas
K.c.put(K.B + "/api/settings", json={"stream_answers": False}, headers=K.H)
t = K.run("STREAMTEST STREAMOFF tell me a bedtime story")
deltas = [e for e in EV if e.get("kind") == "answer_delta" and e.get("task_id") == t["id"]]
ex = K.c.get(LLM + "/calls_for", params={"marker": "STREAMOFF"}).json()
check("stream_answers off: the request is plain and nothing streams", t["status"] == "COMPLETED" and ex
      and all(c["stream"] is False and c["stream_options"] is None for c in ex) and not deltas, ([(c["stream"], c["stream_options"]) for c in ex], len(deltas)))
K.c.put(K.B + "/api/settings", json={"stream_answers": True}, headers=K.H)
STOP.set()
finish()
