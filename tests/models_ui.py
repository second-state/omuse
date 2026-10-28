"""Settings → Models: "Use my own model" is a toggle; off = the section is closed and the system's model is in force,
on = the section opens with endpoint, API key and LLM required (the browser refuses to save without them); the saved
values come back after a reload, the key only as "saved"; off again closes the section and the system model is back.
Fast mode (service_tier "fast") and reasoning effort: the system's from config.toml (tests/run_local.sh: fast, low),
shown read-only and not changeable through the API; with the own model the user's choice goes into each request."""
import asyncio, sys
from playwright.async_api import async_playwright

B = "http://127.0.0.1:8080/"
fails = []


def check(name, cond, info=""):
    print(("PASS " if cond else "FAIL ") + name, "" if cond else info)
    if not cond:
        fails.append(name)


async def chat_body(c, H):
    """Send one chat message and return the body of the last request the (fake) model received."""
    import time
    tid = c.post(B + "api/chat", json={"message": "hello"}, headers=H).json()["task_id"]
    for _ in range(100):
        if c.get(B + f"api/tasks/{tid}").json()["status"] in ("COMPLETED", "FAILED", "CANCELLED"):
            break
        time.sleep(0.3)
    return c.get("http://127.0.0.1:8090/calls").json()["last"] or {}


async def main():
    import httpx
    H = {"X-Persona-UI": "1"}
    c = httpx.Client(trust_env=False, timeout=30)
    c.put(B + "api/settings", json={"language": "zh", "ui_language": "zh", "model_override": False}, headers=H)
    sys_url = c.get(B + "api/settings", headers=H).json()["settings"]["model_base_url"]
    async with async_playwright() as p:
        b = await p.chromium.launch()
        ctx = await b.new_context(viewport={"width": 1400, "height": 1000}, locale="zh-CN")
        pg = await ctx.new_page()
        errs = []
        pg.on("pageerror", lambda e: errs.append(str(e)))
        await pg.goto(B + "#settings")
        await pg.wait_for_selector("#models")
        toggle = pg.locator("#models label.toggle", has_text="使用自己的模型").locator("input")
        box = pg.locator("#modelOverride")
        check("toggle off and the own-model section closed", not await toggle.is_checked() and await box.is_hidden())
        check("the system model is named", "fake" in await pg.locator("#systemModel").inner_text(), await pg.locator("#systemModel").inner_text())
        await toggle.click()
        check("the system's fast mode and reasoning are shown read-only", "开" in await pg.locator("#systemOptions").inner_text()
              and "低" in await pg.locator("#systemOptions").inner_text(), await pg.locator("#systemOptions").inner_text())
        r = c.put(B + "api/settings", json={"fast_mode": False, "reasoning_effort": "high"}, headers=H).json()["settings"]
        check("without the own model, fast mode and reasoning can't be changed (config.toml wins)",
              r["fast_mode"] is True and r["reasoning_effort"] == "low", r)
        body = await chat_body(c, H)
        check("the system model's request carries service_tier fast and reasoning_effort low",
              body.get("service_tier") == "fast" and body.get("reasoning_effort") == "low", {k: body.get(k) for k in ("service_tier", "reasoning_effort")})
        await pg.goto(B + "#settings")   # the chat above opened its conversation in the page
        await pg.reload()
        await pg.wait_for_selector("#models")
        await toggle.click()
        check("toggle on opens the section with 12 fields (endpoint, key, 5 models, the optional image/video endpoints and keys)", await box.is_visible() and await box.locator("input:not([type=checkbox])").count() == 12,
              await box.locator("input:not([type=checkbox])").count())
        fast = box.locator("label.toggle", has_text="快速模式").locator("input")
        effort = box.locator("select")
        check("...plus the fast-mode toggle and the reasoning choice, the user's own (off, model default)",
              await fast.is_visible() and not await fast.is_checked() and await effort.input_value() == "")
        check("the three required fields are marked", await box.locator(".req").count() == 3)
        save = pg.locator("button", has_text="保存设置")
        await save.click()
        await pg.wait_for_selector(".toast.err")
        t = await pg.locator(".toast.err").last.inner_text()
        check("saving with nothing filled is refused: endpoint first", "接口地址" in t, t)
        check("...and nothing was saved", c.get(B + "api/settings", headers=H).json()["settings"]["model_override"] is False)
        inputs = box.locator("input")
        await inputs.nth(0).fill(sys_url)
        await save.click()
        await pg.wait_for_timeout(300)
        t = await pg.locator(".toast.err").last.inner_text()
        check("then the API key", "API" in t and ("密钥" in t or "key" in t), t)
        await inputs.nth(1).fill("ui-key-1")
        await save.click()
        await pg.wait_for_timeout(300)
        t = await pg.locator(".toast.err").last.inner_text()
        check("then the LLM model name", "LLM" in t, t)
        await inputs.nth(2).fill("fake")
        await inputs.nth(3).fill("fake")   # planner
        await effort.select_option("high")
        await save.click()
        await pg.wait_for_selector(".toast:not(.err)")
        s = c.get(B + "api/settings", headers=H).json()
        check("saved: own model on, key stored but not returned", s["settings"]["model_override"] is True and s["settings"]["model_api_key_set"]
              and "model_api_key" not in s["settings"] and s["settings"]["planner_model"] == "fake", s["settings"])
        check("the own model's choice is saved: fast mode off, reasoning high", s["settings"]["fast_mode"] is False and s["settings"]["reasoning_effort"] == "high", s["settings"])
        check("the key field is cleared and says saved", await inputs.nth(1).input_value() == "" and "已保存" in (await inputs.nth(1).get_attribute("placeholder") or ""))
        body = await chat_body(c, H)
        check("the own model's request: no service_tier, reasoning_effort high",
              "service_tier" not in body and body.get("reasoning_effort") == "high", {k: body.get(k) for k in ("service_tier", "reasoning_effort")})
        await pg.goto(B + "#settings")
        await pg.reload()
        await pg.wait_for_selector("#models")
        toggle = pg.locator("#models label.toggle", has_text="使用自己的模型").locator("input")
        box = pg.locator("#modelOverride"); inputs = box.locator("input")
        check("after a reload the reasoning choice is still high", await box.locator("select").input_value() == "high")
        check("after a reload the toggle is on and the section open with the values", await toggle.is_checked() and await box.is_visible()
              and await inputs.nth(0).input_value() == sys_url and await inputs.nth(2).input_value() == "fake" and await inputs.nth(3).input_value() == "fake")
        check("the key is never shown", await inputs.nth(1).input_value() == "" and await inputs.nth(1).get_attribute("type") == "password")
        check("the header carries no model chip (several models answer a task; Settings names them)", await pg.locator("#modelChip").count() == 0)
        await toggle.click()
        check("toggle off closes the section", await box.is_hidden())
        await pg.locator("button", has_text="保存设置").click()
        await pg.wait_for_selector(".toast:not(.err)")
        s = c.get(B + "api/settings", headers=H).json()
        check("off: the system model is in force, the own values kept", s["settings"]["model_override"] is False and s["settings"]["planner_model"] == ""
              and s["model_user"]["planner_model"] == "fake" and s["model_user"]["api_key_set"], s)
        check("off: the system's fast mode and reasoning are back, the own choice kept",
              s["settings"]["fast_mode"] is True and s["settings"]["reasoning_effort"] == "low" and s["model_user"]["reasoning_effort"] == "high", s)
        await pg.reload()
        await pg.wait_for_selector("#models")
        check("after a reload the section stays closed", await pg.locator("#modelOverride").is_hidden())
        check("no page errors", not errs, errs)
        await b.close()
    c.put(B + "api/settings", json={"model_override": False}, headers=H)
    print("\n" + ("ALL PASS" if not fails else f"{len(fails)} FAILED: {fails}"))
    sys.exit(1 if fails else 0)

asyncio.run(main())
