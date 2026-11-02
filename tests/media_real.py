"""One picture and one short clip with a real provider, from the user's point of view (tests/user_common.py). The user's
own model set (Settings → Models) has the chat model plus a separate image and video endpoint with its own key — the
SAME gateway as in the e2e env file, with xai/grok-imagine-image and xai/grok-imagine-video (world setup). What the
user sees:
  - Settings → Models → Test models: the image and the video model are both listed by the gateway (✓ … (listed)).
  - picture — in a new chat they ask for a watercolour of a paper boat, landscape: one picture shows in the chat, it
    loads, it is landscape (wider than tall), and it downloads as a real PNG/JPEG.
  - edit — in the same chat: "give the boat a bright red sail": a second picture shows, it loads and downloads as a
    real PNG/JPEG, and it is a different picture from the first.
  - clip — in the same chat they ask for a 5-second video of that boat: a video player shows in the chat, the video
    plays and lasts a few seconds, it downloads as a real MP4, and no second picture appeared. (The test image's
    Chromium has no H.264 decoder: when its player cannot decode the clip, the downloaded file is read as a player would.)
The three requests are billed. One TIMING line per request.

Not in run_in_docker.sh's default list:
    OMUSE_E2E_ENV_FILE=~/.config/omuse/e2e.env bash tests/run_in_docker.sh media_real"""
import json
import os
import subprocess
import sys
import tempfile

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from tests import user_common as U  # noqa: E402
from tests.user_common import check  # noqa: E402

IMAGE_MODEL = os.environ.get("OMUSE_E2E_IMAGE_MODEL", "xai/grok-imagine-image")
VIDEO_MODEL = os.environ.get("OMUSE_E2E_VIDEO_MODEL", "xai/grok-imagine-video")
URL = os.environ["OMUSE_E2E_MODEL_URL"].rstrip("/")


def probe(data: bytes) -> tuple[float, int, str]:
    """A video file's length in seconds and width in pixels as a player reads them (0 when it does not play)."""
    with tempfile.NamedTemporaryFile(suffix=".mp4") as f:
        f.write(data)
        f.flush()
        r = subprocess.run(["ffprobe", "-v", "error", "-select_streams", "v:0", "-show_entries", "format=duration:stream=width",
                            "-of", "json", f.name], capture_output=True, text=True, timeout=60)
    try:
        j = json.loads(r.stdout)
        return float(j["format"]["duration"]), int((j.get("streams") or [{}])[0].get("width") or 0), r.stdout[:200]
    except (ValueError, KeyError):
        return 0.0, 0, (r.stdout + r.stderr)[:200]


async def main():
    async with U.User() as u:
        pg = u.page
        # Settings → Models → Test models
        await pg.goto(U.APP + "#settings")
        btn = pg.locator("button", has_text="Test models")
        await btn.wait_for(timeout=20000)
        await btn.click()
        try:
            await pg.wait_for_function("m => [...document.querySelectorAll('#models .small')].some(e => e.textContent.includes(m) && /[✓✕]/.test(e.textContent))",
                                       arg=VIDEO_MODEL, timeout=120000)
        except Exception:   # noqa: BLE001 — what the page shows then is checked below
            pass
        lines = [" ".join(t.split()) for t in await pg.locator("#models .small").all_inner_texts() if t.strip()[:1] in ("✓", "✕")]
        print(f"INFO Test models: {lines}")
        check(f"Test models: the image model {IMAGE_MODEL} is listed by the gateway", any(l.startswith("✓") and IMAGE_MODEL in l and "(listed)" in l for l in lines), lines)
        check(f"Test models: the video model {VIDEO_MODEL} is listed by the gateway", any(l.startswith("✓") and VIDEO_MODEL in l and "(listed)" in l for l in lines), lines)

        # 1. a picture
        a = await u.ask("Please draw a watercolour picture of a small paper boat on a calm pond at sunrise, landscape format.",
                        label="picture", timeout=400)
        pics = [m for m in await u.media() if m["kind"] == "image"]
        check("picture: one picture shows in the chat", len(pics) == 1, pics)
        if pics:
            p = pics[0]
            check("picture: it loads, and it is landscape (wider than tall)", p["width"] > 0 and p["width"] > p["height"], p)
            data = await u.download(p["src"])
            check("picture: it downloads as a real PNG/JPEG", len(data) > 20000 and (data[:4] == b"\x89PNG" or data[:3] == b"\xff\xd8\xff"),
                  (len(data), data[:8]))
        check("picture: the answer finished", not a.timed_out, a.seconds)

        # 2. the picture edited, in the same chat
        a = await u.ask("Now give the boat a bright red sail, same scene.", new_chat=False, label="edit", timeout=400)
        pics2 = [m for m in await u.media() if m["kind"] == "image"]
        check("edit: a second picture shows in the chat", len(pics2) == 2, pics2)
        if len(pics2) == 2 and pics:
            q = pics2[-1]
            check("edit: it loads", q["width"] > 0, q)
            data2 = await u.download(q["src"])
            check("edit: it downloads as a real PNG/JPEG, different from the first picture",
                  len(data2) > 20000 and (data2[:4] == b"\x89PNG" or data2[:3] == b"\xff\xd8\xff") and data2 != data, (len(data2), data2[:8]))
        check("edit: the answer finished", not a.timed_out, a.seconds)

        # 3. a clip of the same boat, in the same chat
        a = await u.ask("Now make a 5-second video of that paper boat drifting slowly across the pond.", new_chat=False, label="clip",
                        timeout=900)
        media = await u.media()
        vids = [m for m in media if m["kind"] == "video"]
        check("clip: one video player shows in the chat", len(vids) == 1, media)
        if vids:
            v = vids[0]
            data = await u.download(v["src"])
            check("clip: it downloads as a real MP4", len(data) > 50000 and data[4:8] == b"ftyp", (len(data), data[:12]))
            if v["error"] == 4:
                # the test image's Chromium cannot decode H.264 (no proprietary codecs): read the downloaded file the way the
                # user's own browser or player would
                secs, w, why = probe(data)
                print(f"INFO the test browser cannot decode the clip; the downloaded file: {w} px wide, {secs:.1f} s ({why})")
                check("clip: the video plays and lasts a few seconds (1–15 s)", w > 0 and 1 <= secs <= 15, why)
            else:
                check("clip: the video plays and lasts a few seconds (1–15 s)", v["width"] > 0 and 1 <= v["duration"] <= 15, v)
        check("clip: no further picture appeared", len([m for m in media if m["kind"] == "image"]) == len(pics2), media)
        check("clip: the answer finished", not a.timed_out, a.seconds)


U.setup(vault=False)
# the optional image / video endpoints and keys of the user's own model set: the gateway itself (world setup)
r = U.c.put(U.B + "/api/settings", headers=U.H, json={"model_override": True, "image_base_url": URL, "image_api_key": os.environ["OMUSE_E2E_MODEL_KEY"],
                                                       "image_model": IMAGE_MODEL, "video_base_url": URL,
                                                       "video_api_key": os.environ["OMUSE_E2E_MODEL_KEY"], "video_model": VIDEO_MODEL})
s = r.json().get("settings", {})
check("Settings → Models: the image and video endpoints and keys saved", r.status_code == 200 and s.get("image_base_url") == URL
      and s.get("video_base_url") == URL and s.get("image_api_key_set") and s.get("video_api_key_set"), r.status_code)
U.run(main)
