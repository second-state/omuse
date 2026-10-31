"""Drive clean-up with a real model, from the user's point of view (tests/user_common.py). The user's Google Drive is
connected (the fake Drive of tests/fake_apps.py stands in for it): a 16 GB quota, a 2.5 GB backup-2022.zip never opened
since 2022, a 900 MB old-render.mov last opened in 2023, a 4 GB wedding-raw.mov opened this month, a few small files, a
large sheet someone else shared with them and a clip already in the Bin.

  1. In Main's chat: "My Google Drive is almost full — what could I delete?" The answer gives the storage numbers
     (16 GB), lists the big and long-unused files (backup-2022.zip and old-render.mov at least) with their sizes, does
     not offer the sheet shared with them, and asks the user which to remove — and nothing has left their Drive yet
     (the Bin holds only what was there before).
  2. In the same chat the user picks two of them by name. Afterwards their Drive shows exactly those two in the Bin
     (with the clip that was there), wedding-raw.mov and everything else still in My Drive, and the answer says they
     are in the Bin (recoverable for 30 days) and how much space that frees (about 3.4 GB).
One TIMING line per request.

    OMUSE_E2E_ENV_FILE=~/.config/omuse/e2e.env tests/run_in_docker.sh drive_cleanup_real"""
import os
import re
import sys

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from tests import drive_common as D  # noqa: E402
from tests import user_common as U  # noqa: E402
from tests.user_common import check  # noqa: E402

CHOSEN = {"backup-2022.zip", "old-render.mov"}


def names(rows: list[dict]) -> set[str]:
    return {f["name"] for f in rows}


async def main():
    live0, bin0 = names(D.my_drive()), names(D.my_drive(trashed=True))
    print(f"INFO My Drive before: {sorted(live0)}; Bin: {sorted(bin0)}")
    async with U.User() as u:
        a = await u.ask("My Google Drive is almost full. What's taking up the space, and what could I delete?", label="cleanup-list", timeout=600)
        live1, bin1 = names(D.my_drive()), names(D.my_drive(trashed=True))
        check("list: the answer finished", not a.timed_out, a.seconds)
        check("list: the storage numbers (the 16 GB plan)", re.search(r"\b16(\.0+)?\s?GB\b|\b16\s?GiB\b", a.text) is not None, a.text[:600])
        check("list: the big, long-unused files are listed (backup-2022.zip, old-render.mov)", a.mentions("backup-2022") and a.mentions("old-render"),
              a.text[:1200])
        check("list: … with their sizes", re.search(r"\d(\.\d+)?\s?(GB|MB|GiB|MiB)", a.text) is not None, a.text[:1200])
        check("list: the sheet shared with the user is not offered", not a.mentions("team-roadmap"), a.text[:1200])
        check("list: the user is asked which to remove", "?" in a.text and a.mentions("which", "all", "bin", "trash", "delete", "remove"),
              a.text[-500:])
        check("list: nothing has left My Drive yet, the Bin is as before", live1 == live0 and bin1 == bin0, (live0 - live1, bin1 - bin0))

        b = await u.ask("Please move backup-2022.zip and old-render.mov to the bin. Keep everything else, including wedding-raw.mov.",
                        new_chat=False, label="cleanup-trash", timeout=600)
    live2, bin2 = names(D.my_drive()), names(D.my_drive(trashed=True))
    print(f"INFO My Drive after: {sorted(live2)}; Bin: {sorted(bin2)}")
    check("trash: the answer finished", not b.timed_out, b.seconds)
    check("trash: exactly the two chosen files went to the Bin", bin2 - bin0 == CHOSEN, sorted(bin2 - bin0))
    check("trash: everything else is still in My Drive (wedding-raw.mov too)", live2 == live0 - CHOSEN and "wedding-raw.mov" in live2,
          sorted(live0 - live2))
    check("trash: the answer says they are in the Bin and can be restored for 30 days", b.mentions("bin", "trash") and "30" in b.text, b.text[:600])
    check("trash: the answer says how much space that frees (about 3.4 GB, or the MB)",
          re.search(r"\b3[.,][3-5]\d*\s?(GB|GiB)|\b3,?[3-5]\d\d(\.\d+)?\s?(MB|MiB)", b.text) is not None, b.text[:600])


U.setup(vault=False)
D.disconnect_all()
D.reset_drive()
D.connect_drive()
U.run(main)
