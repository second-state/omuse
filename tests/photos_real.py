"""Photos & videos with a real model, from the user's point of view (tests/user_common.py). The user's Google Drive is
connected (the fake Drive of tests/fake_apps.py stands in for it) and holds a "Kyoto trip" folder with two short clips
(kyoto-arrival.mp4, kyoto-temple.mp4). In Main's chat the user asks for a short highlights video from those clips, put
back in the same folder. What the user sees: the answer comes from the Photos & videos bot (its link in the answer);
they are asked to approve the upload to Drive (and approve it); afterwards their "Kyoto trip" folder holds exactly one
new video, a real MP4 that plays (it has a video stream and lasts a few seconds), the two clips are still there, and the
answer links that new file in Drive. If the bot asks first (which clips, what order), the user answers once in the same
chat. One TIMING line per request.

    OMUSE_E2E_ENV_FILE=~/.config/omuse/e2e.env tests/run_in_docker.sh photos_real"""
import json
import os
import re
import subprocess
import sys
import tempfile

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from tests import drive_common as D  # noqa: E402
from tests import user_common as U  # noqa: E402
from tests.user_common import check  # noqa: E402

CLIPS = {"kyoto-arrival.mp4", "kyoto-temple.mp4"}


def plays(data: bytes) -> tuple[float, str]:
    """The video's length in seconds as a player reads it (0 when it does not play), and what ffprobe says."""
    with tempfile.NamedTemporaryFile(suffix=".mp4") as f:
        f.write(data)
        f.flush()
        r = subprocess.run(["ffprobe", "-v", "error", "-show_entries", "format=duration:stream=codec_type", "-of", "json", f.name],
                           capture_output=True, text=True, timeout=60)
    try:
        j = json.loads(r.stdout)
        has_video = any(s.get("codec_type") == "video" for s in j.get("streams", []))
        return (float(j["format"]["duration"]) if has_video else 0.0), r.stdout[:300]
    except (ValueError, KeyError):
        return 0.0, (r.stdout + r.stderr)[:300]


async def main():
    before = {f["name"] for f in U.drive_folder("Kyoto trip")}
    print(f"INFO the Kyoto trip folder before: {sorted(before)}")
    async with U.User() as u:
        a = await u.ask("Make a short highlights video from my Kyoto trip clips in Google Drive and put it back in the same folder.", label="video")
        answers = [a]
        new = [f for f in U.drive_folder("Kyoto trip") if f["name"] not in before]
        if not new and not a.timed_out:
            # the bot asked first (which clips, what order): the user answers once, in the same chat
            answers.append(await u.ask("Yes — use both clips, arrival first, then the temple. Go ahead and upload it to the same folder.",
                                       new_chat=False, label="video-followup"))
    dialogs = [d for x in answers for d in x.approvals]
    text = "\n".join(x.text for x in answers)
    links = [l for x in answers for l in x.links]
    check("video: answered by the Photos & videos bot (its link in the answer)", any("Photos" in x.bot_link for x in answers),
          [x.bot_link for x in answers])
    check("video: the user was asked to approve the upload to Drive (and approved it)", any(re.search(r"drive|upload", d, re.I) for d in dialogs),
          dialogs)
    folder = U.drive_folder("Kyoto trip")
    print(f"INFO the Kyoto trip folder after: {[(f['name'], f['mimeType'], f['size']) for f in folder]}")
    new = [f for f in folder if f["name"] not in before]
    check("video: the user's Kyoto trip folder holds exactly one new file", len(new) == 1, [f["name"] for f in new])
    check("video: the two clips are still in the folder", CLIPS <= {f["name"] for f in folder}, [f["name"] for f in folder])
    vids = [f for f in new if (f["mimeType"] or "").startswith("video/") or f["name"].lower().endswith(".mp4")]
    check("video: the new file is a video", bool(vids), new)
    if vids:
        data = U.drive_file(vids[0]["id"])
        secs, why = plays(data)
        print(f"INFO {vids[0]['name']}: {len(data)} bytes, {secs:.1f} s")
        check("video: it is a real MP4 that plays (a video stream, 1 to 60 seconds)", data[4:8] == b"ftyp" and 1 <= secs <= 60, (data[:12], why))
        check("video: the answer links the new video in Drive", any(vids[0]["webViewLink"] in l or vids[0]["id"] in l for l in links),
              (vids[0]["webViewLink"], links, text[:300]))


U.setup(auto_approve=False)  # this suite checks the approval dialogs: the user turned "Auto-approve actions" off (the default approves everything)
D.disconnect_all()
D.reset_drive()
D.connect_drive()
U.run(main)
