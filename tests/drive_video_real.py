"""Video edit from Drive with a real model, from the user's point of view (tests/user_common.py). The user's Google Drive
is connected (the fake Drive of tests/fake_apps.py stands in for it) and holds a "Kyoto trip" folder with two 2-second
clips (kyoto-arrival.mp4, kyoto-temple.mp4). In Main's chat the user asks to join them (arrival first), put the title
"Kyoto 2026" on the result, show it in the chat and save it back to the same folder. If the agent asks first, the user
answers once in the same chat.

What the user sees:
  - one video player in the chat (only the final cut, no intermediate steps); the video downloads as a real MP4 that
    plays for about the two clips' length (3–5 s);
  - the title is on the picture: the first second of the cut differs visibly from the same moment of the arrival clip
    (the words drawn over it), and the end (the temple clip) is there too;
  - their "Kyoto trip" folder holds exactly one new video — the same cut (3–5 s) — and the two clips; the answer links
    the new file in Drive.
(The test image's Chromium cannot decode H.264: the video is read from the downloaded file as the user's own player
would.) One TIMING line per request.

    OMUSE_E2E_ENV_FILE=~/.config/omuse/e2e.env tests/run_in_docker.sh drive_video_real"""
import json
import os
import subprocess
import sys
import tempfile

from PIL import Image, ImageChops

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from tests import drive_common as D  # noqa: E402
from tests import user_common as U  # noqa: E402
from tests.user_common import check  # noqa: E402

CLIPS = {"kyoto-arrival.mp4", "kyoto-temple.mp4"}


def probe(data: bytes) -> tuple[float, int, int, str]:
    """(seconds, width, height) as a player reads the video (0 when it does not play), and what ffprobe says."""
    with tempfile.NamedTemporaryFile(suffix=".mp4") as f:
        f.write(data)
        f.flush()
        r = subprocess.run(["ffprobe", "-v", "error", "-select_streams", "v:0", "-show_entries", "format=duration:stream=width,height",
                            "-of", "json", f.name], capture_output=True, text=True, timeout=60)
    try:
        j = json.loads(r.stdout)
        s = (j.get("streams") or [{}])[0]
        return float(j["format"]["duration"]), int(s.get("width") or 0), int(s.get("height") or 0), r.stdout[:200]
    except (ValueError, KeyError):
        return 0.0, 0, 0, (r.stdout + r.stderr)[:200]


def frame(data: bytes, at: float, size: tuple[int, int]) -> Image.Image | None:
    """The picture the player shows at `at` seconds, at `size`."""
    with tempfile.TemporaryDirectory() as d:
        src, out = os.path.join(d, "v.mp4"), os.path.join(d, "f.png")
        open(src, "wb").write(data)
        subprocess.run(["ffmpeg", "-loglevel", "error", "-y", "-ss", str(at), "-i", src, "-frames:v", "1", out], timeout=60)
        if not os.path.exists(out):
            return None
        return Image.open(out).convert("L").resize(size)


def changed(a: Image.Image, b: Image.Image) -> float:
    """The share of pixels that differ clearly (more than re-encoding noise) between two pictures."""
    d = ImageChops.difference(a, b).point(lambda v: 255 if v > 60 else 0)
    return sum(1 for v in d.tobytes() if v) / (a.size[0] * a.size[1])


async def main():
    before = {f["name"] for f in U.drive_folder("Kyoto trip")}
    arrival = next(f for f in U.drive_folder("Kyoto trip") if f["name"] == "kyoto-arrival.mp4")
    temple = next(f for f in U.drive_folder("Kyoto trip") if f["name"] == "kyoto-temple.mp4")
    async with U.User() as u:
        a = await u.ask("Join the two clips in my Google Drive folder \"Kyoto trip\" into one video — the arrival clip first, then the "
                        "temple — and put the title \"Kyoto 2026\" on it. Show me the video here and save it back to the same folder.",
                        label="video", timeout=900)
        answers = [a]
        new = [f for f in U.drive_folder("Kyoto trip") if f["name"] not in before]
        if not new and not a.timed_out:
            answers.append(await u.ask("Yes, go ahead: both clips, arrival first, title \"Kyoto 2026\", and upload it to the same folder.",
                                       new_chat=False, label="video-followup", timeout=900))
        media = await u.media()
        vids = [m for m in media if m["kind"] == "video"]
        shown = await u.download(vids[-1]["src"]) if vids else b""
    links = [l for x in answers for l in x.links]
    text = "\n".join(x.text for x in answers)
    check("video: the answer finished", not any(x.timed_out for x in answers), [x.seconds for x in answers])
    check("video: one video player in the chat (only the final cut)", len(vids) == 1, media)
    secs, w, h, why = probe(shown) if shown else (0.0, 0, 0, "")
    print(f"INFO the video in the chat: {len(shown)} bytes, {w}x{h}, {secs:.1f} s")
    check("video: the video in the chat downloads as a real MP4 that plays for the two clips' length (3–5 s)",
          shown[4:8] == b"ftyp" and w > 0 and 3 <= secs <= 5.5, (shown[:12], why))
    if w and h:
        size = (w, h)
        start, orig_start = frame(shown, 0.5, size), frame(U.drive_file(arrival["id"]), 0.5, size)
        end, orig_end = frame(shown, secs - 0.6, size), frame(U.drive_file(temple["id"]), 1.4, size)
        if start and orig_start:
            diff = changed(start, orig_start)
            print(f"INFO the first half second differs from the arrival clip in {diff:.1%} of the picture")
            check("video: the title is on the picture at the start (words over the arrival clip)", diff >= 0.003, diff)
        else:
            check("video: the title is on the picture at the start (words over the arrival clip)", False, "no frame")
        if end and orig_end:
            check("video: the cut ends with the temple clip (its last moment looks like the temple clip's)", changed(end, orig_end) < 0.3,
                  changed(end, orig_end))
    folder = U.drive_folder("Kyoto trip")
    print(f"INFO the Kyoto trip folder after: {[(f['name'], f['mimeType'], f['size']) for f in folder]}")
    new = [f for f in folder if f["name"] not in before]
    check("video: the two clips are still in the folder", CLIPS <= {f["name"] for f in folder}, [f["name"] for f in folder])
    check("video: the folder holds exactly one new file, a video", len(new) == 1 and ((new[0]["mimeType"] or "").startswith("video/")
          or new[0]["name"].lower().endswith(".mp4")), new)
    if new:
        data = U.drive_file(new[0]["id"])
        s2, w2, _, why2 = probe(data)
        check("video: the file in Drive is the cut (a real MP4, 3–5 s)", data[4:8] == b"ftyp" and w2 > 0 and 3 <= s2 <= 5.5, why2)
        check("video: the answer links the new video in Drive", any(new[0]["webViewLink"] in l or new[0]["id"] in l for l in links)
              or new[0]["id"] in text, (new[0]["webViewLink"], links, text[-400:]))


U.setup(vault=False)
D.disconnect_all()
D.reset_drive()
D.connect_drive()
U.run(main)
