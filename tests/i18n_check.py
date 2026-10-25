"""The UI speaks one language at a time.

* Every Chinese UI string in app.js is wrapped in T()/Tf() and has an English (i18n.js) and a Traditional (i18n_tw.js)
  translation; the Traditional table has no keys the English one lacks.
* English values contain no Chinese.
* A Chinese UI never shows the English half of a bilingual source string: `bizh` below mirrors `biZh` in app.js (the
  reducer every Chinese string goes through); after it, a key or a Traditional value may only keep Latin words that
  are product / protocol names or identifiers (ZH_TERMS / ZH_IDENT, also copied from app.js)."""
import json
import re
import sys

sys.path.insert(0, '/tmp/claude-0')
src = open('web/app.js', encoding='utf-8').read()
en = json.loads(re.search(r'window\.OMUSE_EN = (\{.*\});', open('web/i18n.js', encoding='utf-8').read(), re.S).group(1))
tw = json.loads(re.search(r'window\.OMUSE_TW = (\{.*\});', open('web/i18n_tw.js', encoding='utf-8').read(), re.S).group(1))
CJK = '一-鿿'
keys = set()
for m in re.finditer(r"\bT\((['`])((?:\\.|(?!\1).)*)\1\)", src):
    keys.add(m.group(2))
for m in re.finditer(r'\bTf\(("(?:\\.|[^"\\])*")', src):
    keys.add(json.loads(m.group(1)))
missing = sorted(k for k in keys if re.search(f'[{CJK}]', k) and k not in en)
missing_tw = sorted(k for k in keys if re.search(f'[{CJK}]', k) and k not in tw)
stale_tw = sorted(k for k in tw if k not in en)   # the Traditional table mirrors the English one
# CJK literals not wrapped: strip T(...)/Tf(...) strings, comments and regexes, then look for CJK quotes
body = re.sub(r"\bT\((['`])(?:\\.|(?!\1).)*\1\)", "", src)
body = re.sub(r'\bTf\("(?:\\.|[^"\\])*"', "", body)
body = "\n".join(l for l in body.splitlines() if "i18n-ok" not in l)
body = re.sub(r"//[^\n]*", "", body)
unwrapped = [l.strip()[:100] for l in body.splitlines() if re.search(f"['\"`][^'\"`\\n]*[{CJK}]", l)
             and 'DAYS' not in l and "'每周" not in l and '每天' not in l and '工作日' not in l]
# English values must be English
cjk_en = sorted(k for k, v in en.items() if re.search(f'[{CJK}]', v))

# ---------------------------------------------------------------- the Chinese reducer, as app.js has it
TERMS = set(re.search(r"const ZH_TERMS = new Set\(\((.*?)\)\.split\(' '\)\);", src, re.S).group(1)
            .replace("' +\n  '", "").strip("'").split(" "))
IDENT = re.compile(r"\{\d+\}|v?\d[\d\.]*|[A-Z]{1,5}\d*|[\w.+-]+@[\w.-]*|https?://\S*|#\w+|\d+%|[+\-*/×=…→•]|[.,;:!?()]|/\w+|\w+[:_/.-]\S*|xox[bp]-|ntn_|[\w-]+\.(com|eu|org|io|md|toml)|SHA-\d+")


def is_term(seg):
    s = seg.strip(" ·,，。.;；:：()（）/-—–")
    if not s:
        return True
    return all(t in TERMS or IDENT.fullmatch(t) for t in re.split(r"[\s/,·]+", s) if t)


def bizh(s):
    if not re.search(f"[{CJK}]", s) or not re.search(r"[A-Za-z]{2,}", s):
        return s
    t = re.sub(rf"([「“][^」”]*?[{CJK}][^」”{CJK}]*?)\s+([A-Za-z][^」”{CJK}]*)([」”])",
               lambda m: m.group(0) if is_term(m.group(2)) else m.group(1) + m.group(3), s)
    t = re.sub(rf"\s*[(（]([^()（）{CJK}]*[A-Za-z]{{2,}}[^()（）{CJK}]*)[)）]", lambda m: m.group(0) if is_term(m.group(1)) else "", t)
    t = re.sub(rf"([{CJK}）)」》…。！？!?:：、])\s*([A-Za-z][^{CJK}]*)$", lambda m: m.group(0) if is_term(m.group(2)) else m.group(1), t)
    t = re.sub(rf"([{CJK}）)」》：:，,])\s*([A-Za-z][A-Za-z'&\-]*(?: [A-Za-z'&\-]+)*)\s*(?=[{CJK}（(「《“→·])",
               lambda m: m.group(0) if is_term(m.group(2)) else m.group(1), t)
    t = re.sub(r"子 Agent", "子智能体", t)
    t = re.sub(r"\bAgent\b", "智能体", t)
    t = re.sub(rf"([{CJK}])\s+(智能体)", r"\1\2", t)
    t = re.sub(rf"(智能体)\s+([{CJK}])", r"\1\2", t)
    return re.sub(r"\s+([，。；：、）」])", r"\1", t).strip()


def residue(z):
    out = []
    for tok in re.split(r"[\s，。；：、（）()「」“”《》｜|]+", z):
        if not tok or tok in TERMS or IDENT.fullmatch(tok):
            continue
        if re.fullmatch(r"[A-Za-z][A-Za-z'&\-]*", tok):
            out.append(tok)
    return out


mixed_zh = sorted((k, residue(bizh(k))) for k in keys if re.search(f'[{CJK}]', k) and residue(bizh(k)))
mixed_tw = sorted((k, residue(bizh(v))) for k, v in tw.items() if k in keys and residue(bizh(v)))
# the reducer must not eat Chinese: every key keeps all its Chinese characters
only_cjk = lambda s: re.sub(f'[^{CJK}]', '', s).replace('智能体', '')   # noqa: E731 — Agent → 智能体 is the one addition
lost = sorted(k for k in keys if re.search(f'[{CJK}]', k) and only_cjk(bizh(k)) != only_cjk(k))
print(f"{len(keys)} keys, {len(missing)} missing translations, {len(missing_tw)} missing Traditional, {len(stale_tw)} stale Traditional, "
      f"{len(unwrapped)} unwrapped lines, {len(cjk_en)} English values with Chinese, {len(mixed_zh)} mixed Chinese keys, "
      f"{len(mixed_tw)} mixed Traditional values, {len(lost)} keys losing Chinese")
for k in missing: print("MISSING", repr(k))
for k in missing_tw: print("MISSING_TW", repr(k))
for k in stale_tw: print("STALE_TW", repr(k))
for l in unwrapped: print("UNWRAPPED", l)
for k in cjk_en: print("CJK_IN_EN", repr(en[k]))
for k, w in mixed_zh: print("MIXED_ZH", repr(bizh(k)), w)
for k, w in mixed_tw: print("MIXED_TW", repr(bizh(tw[k])), w)
for k in lost: print("LOST", repr(k), "->", repr(bizh(k)))
sys.exit(1 if missing or missing_tw or stale_tw or unwrapped or cjk_en or mixed_zh or mixed_tw or lost else 0)
