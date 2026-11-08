"""Streamed answers with a real model, from the user's point of view (OMUSE_E2E_REAL=1, the e2e env file): in the browser
the user asks Main for a short story and watches the reply bubble while it is written. Only what the user sees is
checked: the text appears in the bubble bit by bit, well before the answer is finished, each piece is the beginning of
the final answer, and the final answer is what streamed — with no <think> tags anywhere. Then a request that needs a
tool first (a web search, then a short poem): what the model says while it calls the tool (the stream carries that text
before the call itself) is hidden the moment its tool call arrives, and the answer's own stream replaces it — the bubble
ends on the final answer's beginning, never on the commentary.

Settings → Models points straight at the endpoint (no timing proxy, so the stream is the provider's own). Some
providers behind a gateway hand the whole stream over in one piece (the gateway's `openai/…` route does; `openai-codex/…`
and `xai/…` stream as they write): a tiny probe request to the endpoint tells which (the user's world, not a check), and
only for a streaming provider must the text appear bit by bit. Two short completions are billed.

    OMUSE_E2E_ENV_FILE=~/.config/omuse/e2e.env bash tests/run_in_docker.sh stream_real
    OMUSE_E2E_MODELS=alt OMUSE_E2E_ENV_FILE=~/.config/omuse/e2e.env bash tests/run_in_docker.sh stream_real"""
import os
import re
import sys

import httpx

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from tests import travel_common as T  # noqa: E402
from tests import user_common as U  # noqa: E402
from tests.user_common import check  # noqa: E402

URL = os.environ["OMUSE_E2E_MODEL_URL"].rstrip("/")
KEY = os.environ["OMUSE_E2E_MODEL_KEY"]
EX, _ = T.models()


def provider_streams() -> bool:
    """Whether the endpoint delivers a streamed reply in more than one piece (a short probe, no proxy)."""
    body = {"model": EX, "messages": [{"role": "user", "content": "Count from one to forty in words."}], "stream": True, "max_tokens": 120, "temperature": 0}
    n = 0
    with httpx.Client(timeout=120, trust_env=False) as cl, cl.stream("POST", URL + "/chat/completions", json=body,
                                                                    headers={"Authorization": "Bearer " + KEY, "Accept-Encoding": "identity"}) as r:
        for _ in r.iter_raw():
            n += 1
    return n > 1


def norm(s: str) -> str:
    return " ".join(s.split())


THINK = re.compile(r"</?think>", re.I)


async def main():
    streams = provider_streams()
    print(f"INFO {EX}: the provider {'streams as it writes' if streams else 'hands the stream over in one piece'}")
    async with U.User() as u:
        a, seen = await u.watch("Write a short story of about 150 words about a lighthouse keeper. Just write it, no tools.",
                                label="story", timeout=240)
        final = norm(a.text)
        texts = [norm(t) for _, t in seen]
        print(f"INFO live bubble: first text after {seen[0][0]:.1f} s, answer after {a.seconds:.1f} s" if seen else "INFO the live bubble showed no text")
        check("the answer is on the page: a story of about 150 words", not a.timed_out and len(final.split()) >= 80, (a.timed_out, final[:200]))
        check("no <think> tags in the answer or in the bubble while it was written", not THINK.search(a.text) and not any(THINK.search(t) for t in texts),
              [t[:80] for t in texts if THINK.search(t)][:3] or a.text[:200])
        # the bubble shows the visible text so far: each piece is the beginning of the final answer (the last few
        # characters of a piece can still be half-rendered markdown)
        bad = [t for t in texts if not final.startswith(t[:-12] if len(t) > 24 else t[:12])]
        check("every text the bubble showed while writing is the beginning of the final answer", texts and not bad,
              ([b[:120] for b in bad][:3], final[:120]))
        if streams:
            check("the text appeared bit by bit: several growing pieces, the first well before the answer was finished",
                  len(texts) >= 3 and len(texts[-1]) > len(texts[0]) and seen[0][0] < a.seconds - 1.0,
                  ([len(t) for t in texts][:12], seen[0][0] if seen else None, a.seconds))
            check("what streamed is the final answer: the bubble got to (nearly) the end of it",
                  texts and len(texts[-1]) >= 0.7 * len(final), (len(texts[-1]) if texts else 0, len(final)))

        # a request with a tool call first: the commentary of that turn never shows as the answer
        a, seen = await u.watch("Search the web for how tall the Eiffel Tower is, then write a two-sentence poem that mentions its height.",
                                label="tool first", timeout=300)
        final = norm(re.sub(r"[*_`#]", "", a.text))
        texts = [norm(re.sub(r"[*_`#]", "", t)) for _, t in seen]   # the live bubble may show markdown before it is rendered
        check("tool first: the answer is on the page (not empty)", not a.timed_out and bool(final.strip()), (a.timed_out, final[:200]))
        # the model's commentary on a tool-calling turn streams before the tool call does, so it may show until the call
        # arrives; the page hides it then (`calls`), and the answer replaces it: whatever the bubble shows after the
        # last commentary is the answer's beginning (2026-11-07: "The Eiffel Tower is 330 metres…" with a browser_read,
        # then the poem). The answer may arrive in one piece and never be sampled (the gateway hands `openai/…` replies
        # over whole, 2026-11-08): then nothing follows the commentary, and that is not a failure.
        is_answer = lambda t: final.startswith(t[:-12] if len(t) > 24 else t[:12])
        last_other = max((i for i, t in enumerate(texts) if not is_answer(t)), default=-1)
        after = texts[last_other + 1:]
        check("tool first: once the answer shows in the bubble, nothing but the answer follows (the commentary of the tool-calling turn never comes back)",
              all(is_answer(t) for t in after),
              ([t[:120] for t in texts if not is_answer(t)][:3], [t[:80] for t in after][:3], final[:120]))
        check("tool first: no <think> tags", not THINK.search(a.text) and not any(THINK.search(t) for t in texts), a.text[:200])


U.setup(vault=False)
# the user's Settings → Models: their own endpoint, directly (the provider's own stream)
r = T.c.put(T.B + "/api/settings", json={"language": "en", "stream_answers": True, "model_override": True, "model_base_url": URL,
                                         "model_api_key": KEY, "model_name": EX, "planner_model": ""}, headers=T.H)
check("Settings → Models: the user's own model, straight to the endpoint", r.status_code == 200, r.text[:200])
U.run(main)
