---
name: drive-photo-storybook
description: 用 Google Drive 里的旅行照片做故事书 / 相册 PDF：挑选和整理一次旅行或活动的照片，按需要修图（去背景、换风格、加字），为每页写几句故事或说明，排成 PDF，放回 Drive 并发到对话里 Turn the trip or event photos in Google Drive into a story book or album PDF — pick and order the photos, edit them where asked (restyle, remove things, add captions), write a short story or caption per page, lay it out as a PDF, show it in the chat and put it back in Drive.
browser: off

plan: on
---
# Photo story book from Drive / 用 Drive 照片做故事书

Use this for "make a story book from my Kyoto photos", "turn the photos in the Birthday folder into a picture book
for Mia", "an album PDF with captions", "a bedtime story with these pictures". Photos come from Drive with the Drive
tools, editing is `edit_image` / `caption_image` / `upscale_image`, the book is `make_pdf`, the result goes back with
`drive_upload`. Never open drive.google.com or an online album maker in the browser.

## Steps

1. **Find the photos** — `drive_search` with `kind=image` plus what the user said (`folder_id`, `query`,
   `modified_after`, `order_by=modified`). Up to ~30 photos; if there are many more, say so and ask whether to take the
   newest N or a folder. One question, then act.
2. **Look and choose** — `drive_download` the candidates (one turn for all), then `file_look` on each to know what is in
   it (people, places, mood). Pick a sequence that tells a story (arrival → highlights → ending), drop blurry or
   duplicate shots, and say in one line which you chose and why.
3. **Edit only where asked** — `edit_image` for a change the user wants (remove a passer-by, a watercolour look for a
   children's book, brighten), `upscale_image` for a small photo that will fill a page, `caption_image` when words must
   be ON the picture. Each edit is billed: do not restyle every photo unless the user asked for a styled book.
4. **Write the text** — the story or captions in the user's language, one or two short paragraphs per page for a story
   book, one line per photo for an album. Use what you saw in the photos and what the user told you (names, dates,
   places). For a child, short sentences and a warm tone; for an album, dates and places.
5. **Lay it out** — ONE `make_pdf` call: title page, then one photo per page as `![…](drive/…)` (or the edited path)
   followed by its text; landscape photos one per page, two portrait photos may share a page. Give it a title for the
   file name ("Kyoto-2026-storybook"), then `send_file` it so the user sees the book in the chat.
6. **Put it back** — `drive_upload` the PDF into the photos' folder (or a new "Story books" folder from
   `drive_create_folder`). Approval is asked — just call it. Tell the user the Drive link.
7. Offer changes: different order, more or fewer photos, another tone, a printable version.

## Rules

- Download each photo once; reuse `drive/…` paths across steps.
- Do not invent names or events the photos and the user did not give you; when unsure, write around it.
- Respect "no edits": an album is the photos as they are.
- Large originals: `make_pdf` handles photos of several MB; do not upscale photos that are already large.
