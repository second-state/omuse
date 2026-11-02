"""The facts code appends to an answer (crates/omuse-runtime/src/agent/show.rs), read the same way by every suite.

After the model's reply, code appends the `show` items of the run's code tools (a confirmation number, a booking
reference, an amount, an artifact or file link, a deadline, an emergency line, the visa-free days …) as one block:
a Markdown rule (`---`) and then one line per item, "- Label: value". The block is everything after the answer's LAST
rule, so a suite finds it without reading the model's prose:

    block = appended_block(answer_markdown)        # fake suites: the task's `result` (Markdown)
    shows(block, "TH7D3K9Q2")                       # a value (or a link) is in the block

Real-model suites read the rendered page instead (tests/user_common.py `Answer.block` / `Answer.block_links`: the text
and links after the bubble's last `<hr>`)."""
from __future__ import annotations

import re

MARK = "\n---\n"


def appended_block(markdown: str) -> str:
    """The block code appended after the reply (everything after the last rule), "" when there is none."""
    text = (markdown or "").replace("\r\n", "\n")
    return text.rsplit(MARK, 1)[1].strip() if MARK in text else ""


def block_links(block: str) -> list[str]:
    """The link targets of a Markdown block ("[text](url)")."""
    return re.findall(r"\]\(([^)\s]+)\)", block or "")


def shows(block: str, *needles: str, links: list[str] | None = None) -> bool:
    """Every needle is in the block's text or among its links (a link may be given whole or as its path)."""
    targets = list(links or []) + block_links(block)
    return all(n and (n in block or any(n in t for t in targets)) for n in needles)


def item_needles(item: dict) -> list[str]:
    """What the rendered line of one `show` item must contain: its value, its link (a file by its path) or its
    currency; the label too when the item has one (English labels: the suites run with English settings)."""
    kind = item.get("kind")
    out = [" ".join(str(item.get("label") or "").split())]
    if kind == "link":
        url = str(item.get("url") or "").strip()
        out.append(url.replace(" ", "%20").replace("(", "%28").replace(")", "%29"))
    elif kind == "file":
        # every byte but ASCII letters and digits percent-encoded, as code writes it (NON_ALPHANUMERIC)
        path = str(item.get("path") or "").strip().lstrip("/").encode()
        out.append("".join(chr(b) if chr(b).isascii() and chr(b).isalnum() else f"%{b:02X}" for b in path))
    elif kind == "amount":
        out.append(str(item.get("currency") or "").strip())
    elif kind in ("reference", "fact"):
        out.append(" ".join(str(item.get("value") or "").split()))
    elif kind == "block":
        out.append((str(item.get("value") or "").strip().splitlines() or [""])[0])
    return [n for n in out if n]


def shows_items(markdown: str, items: list[dict]) -> bool:
    """The answer's appended block carries every `show` item of a result."""
    block = appended_block(markdown)
    return all(shows(block, *item_needles(i)) for i in items or [])
