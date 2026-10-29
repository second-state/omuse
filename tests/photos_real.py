"""Photos & videos with a real model (see tests/real_common.py): a plain request to the main bot — make a short video
from the Kyoto trip clips in Google Drive and put it back in the same folder — handed to the Photos & videos bot, which
loads the drive-video-edit skill, downloads the clips from the fake Drive (tests/fake_apps.py), edits them locally
(real ffmpeg) and uploads one real MP4 back into the clips' folder with an approval; no browser. If the bot asks first (which clips, what
order), one follow-up answers it in the same conversation. One TIMING line per request.

    OMUSE_E2E_ENV_FILE=~/.config/omuse/e2e.env tests/run_in_docker.sh photos_real"""
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from tests import drive_common as D  # noqa: E402
from tests import real_common as R  # noqa: E402

R.setup()
D.disconnect_all()
D.reset_drive()
D.connect_drive()

info = R.ask("Make a short highlights video from my Kyoto trip clips in Google Drive and put it back in the same folder.")
R.timing("video", info)
if "edit_video" not in info["names"] and info["main"].get("status") == "COMPLETED":
    # the bot asked first (which clips, what order): answer once, in the same conversation, and let it go on
    R.routed("video question", info, "photos")
    first = info
    info = R.ask("Yes — use both clips, arrival first, then the temple. Go ahead and upload it to the same folder.",
                 conv=first["conv_id"])
    R.timing("video follow-up", info)
    for k in ("calls", "names", "approvals", "tasks"):
        info[k] = first[k] + info[k]
R.routed("video", info, "photos")
R.finished("video", info)
names = info["names"]
R.check("video: the drive-video-edit skill was loaded", any(x["name"] == "load_skill" and (x.get("args") or {}).get("name") == "drive-video-edit"
                                                           for x in info["calls"]), names)
R.check("video: the clips were found in Drive and both downloaded", R.used(info, "drive_search")
        and {str((x.get("args") or {}).get("file_id")) for x in info["calls"] if x["name"].endswith("drive_download")} >= {"vid-1", "vid-2"},
        [(x["name"], x.get("args")) for x in info["calls"] if x["name"].endswith(("drive_search", "drive_download"))])
R.check("video: edited locally with edit_video", "edit_video" in names, names)
ap = [a for a in info["approvals"] if a["tool"].endswith("drive_upload")]
R.check("video: the upload asked for approval (approved here)", bool(ap), info["approvals"])
up = D.drive_state()["uploads"]
R.check("video: the fake Drive received a real MP4 in the Kyoto trip folder", len(up) >= 1 and all(u["parents"] == ["fld-trip"] for u in up)
        and any(u["mime"] == "video/mp4" and u["size"] > 2000 and u["head"].endswith("66747970") for u in up), up)
R.check("video: one upload only", len(up) == 1, [u["name"] for u in up])
R.check("video: the answer gives the Drive link", "drive.google.com" in info["result"], info["result"][:500])
R.no_browser("video", info)
D.disconnect_all()
R.finish()
