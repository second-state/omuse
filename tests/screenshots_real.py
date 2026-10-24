"""Screenshots of a finished travel run (meant for real-model runs): the chat of that conversation (desktop 1400x1000 and
mobile 390x844), the approvals history, the task's plan / steps view and the task's browser tab.

Run in the same stack right after a travel suite (it wrote /tmp/claude-0/travel_last.json):

    python3 tests/screenshots_real.py <output-dir> [--state /tmp/claude-0/travel_last.json] [--base http://127.0.0.1:8080]

Times are hidden like in tests/screenshots.py so two runs compare cleanly.
"""
import argparse
import asyncio
import json
import os

from playwright.async_api import async_playwright

HIDE = "time, .ts, .time, .muted.small.ts { visibility: hidden !important }"


async def shot(pg, url, path, wait_for=None, settle=1500, click=None):
    await pg.goto(url)
    if click:   # a view that opens from a button (the approvals drawer has no URL of its own)
        await pg.wait_for_selector(click, timeout=10000)
        await pg.wait_for_timeout(1000)
        await pg.click(click)
        await pg.wait_for_timeout(800)
        for d in await pg.query_selector_all("details:not([open])"):   # expand the drawer's decision history
            if "Recent decisions" in (await d.inner_text()):
                await d.evaluate("d => d.open = true")
    if wait_for:
        try:
            await pg.wait_for_selector(wait_for, timeout=10000)
        except Exception:   # the view still gets photographed; a missing selector is visible in the picture
            pass
    await pg.wait_for_timeout(settle)
    await pg.add_style_tag(content=HIDE)
    await pg.screenshot(path=path, full_page=False)
    print("shot", os.path.basename(path), flush=True)


async def main(out, state, base):
    st = json.load(open(state))
    suite, conv, tasks = st.get("suite", "travel"), st.get("conv_id"), st.get("tasks") or []
    tid = tasks[-1] if tasks else ""
    os.makedirs(out, exist_ok=True)
    for f in os.listdir(out):   # this suite's pictures from an earlier run would mix with the new ones
        if f.startswith(suite + "_") and f.endswith(".png"):
            os.remove(os.path.join(out, f))
    p = lambda name: os.path.join(out, f"{suite}_{name}.png")   # noqa: E731
    async with async_playwright() as pw:
        b = await pw.chromium.launch()
        ctx = await b.new_context(viewport={"width": 1400, "height": 1000}, locale="en-US", device_scale_factor=1,
                                  reduced_motion="reduce")
        pg = await ctx.new_page()
        await shot(pg, f"{base}/#chat/{conv}", p("chat_desktop"), "#chatInput", 2500)
        ap = await ctx.new_page()   # its own page: the drawer would stay open over the next views
        await shot(ap, f"{base}/#chat/{conv}", p("approvals"), None, 1500, click="#approvalBell")
        await ap.close()
        for i, t in enumerate(tasks):
            await shot(pg, f"{base}/#tasks/{t}", p(f"task_steps{'' if i == len(tasks) - 1 else '_' + str(i + 1)}"), "#taskDetail", 2000)
        await shot(pg, f"{base}/#browser/{tid}", p("browser_task"), None, 2500)
        mob = await b.new_context(viewport={"width": 390, "height": 844}, locale="en-US", device_scale_factor=1,
                                  reduced_motion="reduce")
        mp = await mob.new_page()
        await shot(mp, f"{base}/#chat/{conv}", p("chat_mobile"), "#chatInput", 2500)
        await b.close()
    print("screenshots in", out)


if __name__ == "__main__":
    ap = argparse.ArgumentParser()
    ap.add_argument("out")
    ap.add_argument("--state", default="/tmp/claude-0/travel_last.json")
    ap.add_argument("--base", default="http://127.0.0.1:8080")
    a = ap.parse_args()
    # show the real model in the header chip while photographing (the suite switched its override off at the end)
    import httpx
    alt = os.environ.get("OMUSE_E2E_MODELS", "") == "alt"
    ex = os.environ.get("OMUSE_E2E_ALT_EXECUTOR" if alt else "OMUSE_E2E_EXECUTOR", "")
    pl = os.environ.get("OMUSE_E2E_ALT_PLANNER" if alt else "OMUSE_E2E_PLANNER", "")
    hc = httpx.Client(timeout=30, trust_env=False, headers={"X-Persona-UI": "1"})
    base = a.base.rstrip("/")
    if ex:
        hc.put(base + "/api/settings", json={"model_override": True, "model_base_url": "http://127.0.0.1:8098/v1",
                                             "model_api_key": "via-proxy", "model_name": ex, "planner_model": pl})
    try:
        asyncio.run(main(a.out, a.state, base))
    finally:
        if ex:
            hc.put(base + "/api/settings", json={"model_override": False})
