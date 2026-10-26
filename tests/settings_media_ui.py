"""Settings → Models → "Use my own model": the optional image / video endpoint, key and model fields. They sit under the
required three, in English in the English UI; blank means the endpoint and key above. Saving a video endpoint with its
key shows the key only as "saved" after a reload, "Clear" drops the key (the endpoint stays), and "Test models" lists a
Video row."""
import asyncio
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from playwright.async_api import async_playwright  # noqa: E402

from tests import skills_common as K  # noqa: E402

B = K.B + "/"
LLM = "http://127.0.0.1:8090"
K.c.put(B + "api/settings", json={"language": "en", "ui_language": "en", "model_override": False}, headers=K.H)


async def main():
    async with async_playwright() as p:
        br = await p.chromium.launch()
        ctx = await br.new_context(viewport={"width": 1400, "height": 1100})
        await ctx.add_init_script("try { localStorage.setItem('omuse_ui', 'en') } catch (e) {}")
        pg = await ctx.new_page()
        errs = []
        pg.on("pageerror", lambda e: errs.append(str(e)))
        await pg.goto(B + "#settings")
        await pg.wait_for_selector("#models")
        await pg.locator("#models label.toggle", has_text="Use my own model").locator("input").click()
        box = pg.locator("#modelOverride")
        txt = await box.inner_text()
        K.check("the own-model section has image and video endpoint, key and model fields, in English",
                all(x in txt for x in ("Image", "Image endpoint (optional, blank = Base URL)", "Image endpoint API key (optional)",
                                       "Video", "Video endpoint (optional, blank = Base URL)", "Video endpoint API key (optional)"))
                and not any("一" <= ch <= "鿿" for ch in txt), txt[:600])
        field = lambda label: box.locator("label.field", has_text=label).locator("input")  # noqa: E731
        K.check("the media key fields say blank = the key above", await field("Video endpoint API key").get_attribute("placeholder") == "Blank = the API key above")
        await field("Base URL *").fill(LLM + "/v1")
        await field("API key").first.fill("own-key")
        await field("LLM").first.fill("fake")
        await field("Video").first.fill("sora-2")
        await field("Video endpoint (optional").fill(LLM + "/vid/v1/")
        await field("Video endpoint API key").fill("vid-key")
        await pg.locator("button", has_text="Save settings").click()
        await pg.wait_for_selector(".toast")
        g = K.c.get(B + "api/settings", headers=K.H).json()
        K.check("saved: the video endpoint (trimmed) and model on the own set, the key only as set",
                g["settings"]["model_override"] is True and g["model_user"]["video_base_url"] == LLM + "/vid/v1"
                and g["model_user"]["video_model"] == "sora-2" and g["model_user"]["video_api_key_set"] is True
                and g["model_user"]["image_base_url"] == "" and g["model_user"]["image_api_key_set"] is False, g["model_user"])
        await pg.reload()
        await pg.wait_for_selector("#modelOverride:not([hidden])")
        K.check("after a reload the video key shows as saved, the image key still as blank = above",
                await field("Video endpoint API key").get_attribute("placeholder") == "Saved; leave blank to keep it"
                and await field("Image endpoint API key").get_attribute("placeholder") == "Blank = the API key above")
        # Test models: a Video row
        await pg.locator("button", has_text="Test models").click()
        await pg.wait_for_selector("#models .small:has-text('Video (sora-2)')", timeout=30000)
        rows = await pg.locator("#models .stack .small").all_inner_texts()
        vid = [r for r in rows if r.startswith(("✓ Video", "✕ Video"))]
        K.check("Test models lists the video model, reachable at its own endpoint", vid and vid[0].startswith("✓ Video (sora-2)") and "not generated" in vid[0], rows)
        # Clear the video key: the endpoint stays, the key is gone
        await box.locator("label.field", has_text="Video endpoint API key").locator("button", has_text="Clear").click()
        await pg.wait_for_selector(".toast")
        g = K.c.get(B + "api/settings", headers=K.H).json()
        K.check("Clear drops the video key and keeps the endpoint", g["model_user"]["video_api_key_set"] is False and g["model_user"]["video_base_url"] == LLM + "/vid/v1", g["model_user"])
        K.check("no page errors", not errs, errs)
        await br.close()


asyncio.run(main())
K.c.put(B + "api/settings", json={"model_override": False}, headers=K.H)
K.finish()
