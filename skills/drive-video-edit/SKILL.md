---
name: drive-video-edit
description: 剪辑 Google Drive 里的旅行视频：把一次旅行的几段拼成精华短片、截取片段、做成竖屏或 720p、加标题字幕、配音乐、变速或去声音，结果放回 Drive 并在对话里播放 Edit the trip videos kept in Google Drive — join the clips of a trip into a highlights cut, cut a section, make a phone-sized or 720p version, add a title caption, music, speed changes or mute, show the result in the chat and put it back in Drive.
browser: off
---
# Edit videos from Drive / 剪辑 Drive 里的视频

Use this for "combine the clips in my Trip folder", "cut the first 10 seconds off", "make a vertical version for
Instagram", "put a title on it", "add this music", "join these two videos and upload the result". All editing is
local with `edit_video` (ffmpeg); the files come from and go back to Drive with the Drive tools. Never open
drive.google.com or an online editor in the browser.

## Steps

1. **Find the clips** — `drive_search` with `kind=video` and what the user said (`query`, `folder_id`, `order_by=
   modified`). If several could match and the user was not specific, list them (name, length, size, modified) and ask
   which ones and in what order. One question, then act.
2. **Bring them in** — `drive_download` each chosen file once (they land under `drive/`). Files over 200 MB cannot be
   downloaded: say so and suggest a smaller export or a shorter list. Several downloads go in one turn.
3. **Check** — `edit_video op=info` on the inputs when lengths or sizes matter (how long the result will be, mixed
   resolutions). Mixed sizes are fine: concat scales everything to the first clip.
4. **Edit, one op per call, chaining the outputs** (the output path of one call is the `path` of the next):
   - join: `concat` with `paths` in order → `trim` for a section → `resize` with `preset=vertical` / `720p` / `1080p`
     / `square` → `caption` with `text` (position top / bottom / center; `start` / `end` seconds for a title only at the
     beginning) → `add_audio` with a workspace audio file → `speed` / `mute` as asked → `gif` for a short preview.
   - Give the final step a `title` so the file name makes sense ("Trip-to-Kyoto-highlights").
   - Every result is shown in the chat by default; for intermediate steps pass `send=false` so the user only sees the
     final cut. Describe the result in one sentence.
5. **Put it back** — `drive_upload` the final file (path from the last op) into the folder the clips came from
   (`folder_id`), or a new folder from `drive_create_folder` when the user asked for one. Approval is asked by
   Sentinel — just call it. Tell the user the Drive link.
6. Offer the next change (shorter, different music, a different caption) instead of re-doing everything.

## Rules

- Music: the user must provide the audio file (upload or a Drive file you download); never fetch music from the web.
- Do not re-download files you already have under `drive/`; do not re-run an op whose output already exists.
- Long videos take time: say what you are doing while ffmpeg runs, and do not run two edits of the same file at once.
- If `edit_video` fails with an ffmpeg error, read the message, fix the arguments (wrong path, end before start, a
  factor out of range) and try once more; do not loop.
