---
name: drive-cleanup
description: 清理 Google Drive、为下一次旅行的照片视频腾出空间：找出占空间的大文件和很久没打开的文件（备份、旧素材、重复导出、下载的安装包），列成清单让用户勾选，确认后一次移到回收站并报告释放的空间 Free up Google Drive before the next trip's photos and videos — find the large and long-unused files (backups, old raw footage, duplicate exports, installers), show a list the user confirms, move the batch to the bin in one approved step and report the space freed.
browser: off
---
# Drive clean-up / 清理 Google Drive

Use this for "my Drive is full", "find big files I don't need", "what's taking up space", "clean up old files",
"free up 10 GB". Everything is done with the Drive tools; never open drive.google.com in the browser.

## Rules

- **Nothing is deleted without the user's explicit yes, and nothing is ever deleted permanently.** `drive_trash`
  moves files to the Drive bin, where they stay 30 days. Say so. If the user wants the space back at once, tell them
  to empty the bin themselves (Drive → Bin → Empty bin); do not try to do it.
- Only the user's own files count against their storage: search with the default `mine=true`. Files shared with
  them cannot be trashed by them anyway.
- Never trash something because its name looks unimportant. The user decides; you only propose and sort.
- Ask before acting only once: show the list, get the answer, act. Do not re-confirm each file.

## Steps

1. `drive_storage` — the quota (used / limit / bin). Tell the user the numbers in one line.
2. Find candidates, in parallel in ONE turn:
   - `drive_search` with `min_size_mb=100` (or what the user said), `order_by=size`, `max_results=50`: the big files.
   - `drive_search` with `unused_days=365` (or what the user said), `min_size_mb=20`, `order_by=size`: big and not
     opened for a long time (`last_viewed` empty = never opened by the user).
   - If the user named a kind ("old videos", "backups"): add `kind=video` or `query=backup` searches.
3. Merge by id. Rank by size, mark what makes each a candidate (size, last opened, kind). Drop anything modified in
   the last 30 days unless the user asked for recent files too.
4. Show the list in chat as a numbered table: name, kind, size (MB/GB), last opened (or "never"), modified, a short
   link. Give the total that would be freed. Then ask one question: "Move all of these to the bin, or only some
   (numbers)?" Stop and wait for the answer.
5. On the user's yes: ONE `drive_trash` call with the chosen `file_ids` and the matching `names` (same order; this is
   what the approval card shows). Approval is asked by Sentinel — just call it. Up to 50 per call; more → several
   calls, each after the previous one returned.
6. Report: how many moved, the MB freed (from the tool's `freed_mb`), any `failed` entries with the reason, and the
   reminder that the bin keeps them for 30 days and emptying it frees the space right away.

## What not to do

- Do not download files to "check" them; names, sizes and dates are enough for a proposal. If the user wants to know
  what a file is, `drive_get` gives the details and the link.
- Do not search again and again with slightly different filters; two or three searches in one turn are enough.
- Do not call `drive_trash` for files the user did not pick, and never a second time for the same ids.
