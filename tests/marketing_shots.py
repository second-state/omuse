"""The README / Olares Market images (deploy/market/assets): real screenshots of the current UI, framed with the OMuse
brand. Not a test suite (it checks nothing); it runs in the test image against the scripted model, whose showcase
scenarios (SHOW_* in tests/fake_llm.py) give presentable English answers:

    OMUSE_E2E_OUT=/tmp/market bash tests/run_in_docker.sh marketing_shots
    cp /tmp/market/market/*.webp /tmp/market/market/overview.png deploy/market/assets/

Writes featured.webp, promote-1…7 *.webp (1440×900) and overview.png (the collage) to <OMUSE_E2E_OUT>/market."""
import asyncio
import base64
import io
import os
import sys
import time

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from PIL import Image  # noqa: E402
from playwright.async_api import async_playwright  # noqa: E402

from tests import skills_common as K  # noqa: E402

OUT = os.path.join("/repo-out" if os.path.isdir("/repo-out") else (sys.argv[1] if len(sys.argv) > 1 else "/tmp/claude-0"), "market")
SHOTS = os.path.join(OUT, "shots")
os.makedirs(SHOTS, exist_ok=True)
ICON = next(p for p in ("/repo/deploy/market/assets/omuse-icon.png",
                        os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))), "deploy/market/assets/omuse-icon.png"))
            if os.path.exists(p))
B = K.B + "/"
# the showcase prompts the scripted model knows (SHOW_* in tests/fake_llm.py)
SHOW_TRIP = "What's on my Tokyo trip this week? Check my mail and calendar."
SHOW_CITY = "We're moving to Shanghai for work with two kids. Where do expat families live, and what does rent cost?"
SHOW_SGAC = "Fill in my Singapore arrival card for Sunday's flight."


# ------------------------------------------------------------------ the world in the pictures
def seed():
    K.c.put(B + "api/settings", headers=K.H, json={"language": "en", "ui_language": "en", "theme": "paper",
                                                    "instance_name": "OMuse", "user_name": "Lucas"})
    K.seed_mail([
        ("Singapore Airlines <noreply@singaporeair.example>", "Your e-ticket: SQ 636 / SQ 637 Singapore – Tokyo Haneda",
         "Booking ref 9QW3PL. SQ 636 SIN 10:30 → HND 18:25 on 18 Nov 2026. SQ 637 HND 11:05 → SIN on 22 Nov 2026.", 6),
        ("Hotel Ginza Grand <reservations@ginzagrand.example>", "Booking confirmed: Hotel Ginza Grand 18–21 Nov",
         "Confirmation 88812345. 8-7-6 Ginza, Chuo City, Tokyo. Check-in 18 Nov 15:00, check-out 21 Nov 11:00.", 5),
        ("OpenTable <reservations@opentable.example>", "Your table at Sushi Hoshizora, Fri 20 Nov 19:00",
         "Reservation OT-55812 for 2 guests.", 3),
        ("Singapore Airlines <noreply@singaporeair.example>", "Your e-ticket: SQ 231 SYD-SIN",
         "Booking ref 7HK2QL. SQ 231 departs Sydney 1 Nov 10:30, arrives Singapore 16:20.", 1),
        ("Marina Bayfront Hotel <stay@marinabayfront.example>", "Booking confirmed: Marina Bayfront Hotel",
         "Confirmation MB-55120. Check-in 1 Nov, 3 nights. 10 Bayfront Avenue, Singapore 018956.", 1),
    ])
    K.c.post(K.S + "/password", headers=K.H, json={"current": "local-test", "new": "marketing-shots-1"})
    K.connect_mail()
    K.connect_calendar()
    K.connect_travel()
    K.set_profile(name_en="LUCAS LU", birthday="1985-04-12", nationality="Australia", email_personal="lucas@example.com",
                  phone="+61 400 000 000")
    K.vault_item("Passport Lucas", "id_document", {"number": "E1234567X", "name": "LU LUCAS", "expiry": "2031-05-01", "country": "AU"},
                 domains="ica.gov.sg,shop.test")


def wait_status(tid, want, timeout=90):
    t0 = time.time()
    while time.time() - t0 < timeout:
        t = K.c.get(f"{K.B}/api/tasks/{tid}").json()
        if t["status"] in want:
            return t
        time.sleep(0.3)
    return t


# ------------------------------------------------------------------ screenshots
async def shoot(pg, name, hash_, settle=1500, click=None):
    await pg.goto(B + "#" + hash_)
    await pg.reload()
    await pg.wait_for_timeout(settle)
    if click:
        loc = pg.locator(click)
        if await loc.count():
            await loc.first.click()
            await pg.wait_for_timeout(800)
    path = os.path.join(SHOTS, name + ".png")
    await pg.screenshot(path=path)
    print("shot", name)
    return path


async def screenshots():
    seed()
    # the model chip shows the model's name: the scripted model under the name of the Olares default
    K.c.put(B + "api/settings", headers=K.H, json={"model_override": True, "model_base_url": "http://127.0.0.1:8090/v1",
                                                    "model_api_key": "local", "model_name": "Qwen3.8-27B"})
    trip = K.run(SHOW_TRIP)
    city = K.run(SHOW_CITY)
    print("tasks", trip["status"], city["status"])
    out = {}
    async with async_playwright() as p:
        br = await p.chromium.launch()
        desk = await br.new_context(viewport={"width": 1440, "height": 900}, device_scale_factor=2, locale="en-US")
        pg = await desk.new_page()
        await pg.goto(B)
        await pg.evaluate("localStorage.setItem('omuse_ui', 'en')")
        out["chat"] = await shoot(pg, "chat", f"chat/{trip['_conv']}", settle=2500)
        out["city"] = await shoot(pg, "city", f"chat/{city['_conv']}", settle=2500)
        out["connections"] = await shoot(pg, "connections", "connections")
        phone = await br.new_context(viewport={"width": 390, "height": 844}, device_scale_factor=3, locale="en-US",
                                     is_mobile=True, has_touch=True)
        pp = await phone.new_page()
        await pp.goto(B)
        await pp.evaluate("localStorage.setItem('omuse_ui', 'en')")
        out["phone_city"] = await shoot(pp, "phone_city", f"chat/{city['_conv']}", settle=2500)
        out["phone_trip"] = await shoot(pp, "phone_trip", f"chat/{trip['_conv']}", settle=2500)
        # the arrival card last: its approval dialog opens over whatever page is showing
        sg = K.run(SHOW_SGAC, approve=False)
        print("sgac", sg["status"])
        await pg.goto(B + "#chat/" + sg["_conv"])
        await pg.wait_for_selector(".approval", timeout=15000)
        await pg.wait_for_timeout(1200)
        out["approval"] = os.path.join(SHOTS, "approval.png")
        await pg.screenshot(path=out["approval"])
        for a in K.c.get(K.S + "/approvals?status=pending", headers=K.H).json()["approvals"]:
            K.c.post(f"{K.S}/approvals/{a['id']}/resolve", json={"decision": "approve", "scope": "ONCE"}, headers=K.H)
        wait_status(sg["id"], ("WAITING_EXTERNAL", "COMPLETED", "FAILED"))
        out["takeover"] = await shoot(pg, "takeover", f"chat/{sg['_conv']}", settle=3000)
        K.hand_back()
        wait_status(sg["id"], ("COMPLETED", "FAILED"))
        out["audit"] = await shoot(pg, "audit", "activity")
        await br.close()
    return out


# ------------------------------------------------------------------ the frames
def uri(path, width=None):
    im = Image.open(path)
    if width and im.width > width:
        im = im.resize((width, round(im.height * width / im.width)), Image.LANCZOS)
    buf = io.BytesIO()
    im.convert("RGB").save(buf, "JPEG", quality=92) if path.endswith(".png") and "icon" not in path else im.save(buf, "PNG")
    kind = "png" if "icon" in path else "jpeg"
    return f"data:image/{kind};base64," + base64.b64encode(buf.getvalue()).decode()


CSS = """
* { box-sizing: border-box; margin: 0; padding: 0; }
body { width: 1440px; height: 900px; overflow: hidden; font-family: 'Noto Sans', 'Noto Sans CJK SC', 'DejaVu Sans', sans-serif;
       color: #0d2b22; background: linear-gradient(135deg, #f3faf6 0%, #e3f1ea 100%); }
.brand { position: absolute; left: 72px; top: 56px; display: flex; align-items: center; gap: 14px; font-size: 30px; font-weight: 700; color: #0f6e4f; }
.brand img { width: 52px; height: 52px; border-radius: 12px; }
h1 { font-size: 64px; line-height: 1.08; font-weight: 800; letter-spacing: -1px; }
p.sub { font-size: 25px; line-height: 1.45; color: #35574b; margin-top: 18px; }
.shot { position: absolute; border-radius: 14px; box-shadow: 0 30px 70px rgba(10, 60, 40, .22), 0 0 0 1px rgba(10, 60, 40, .08); overflow: hidden; background: #fff; }
.shot img { display: block; width: 100%; }
.phone { position: absolute; border-radius: 46px; background: #10231c; padding: 12px; box-shadow: 0 30px 70px rgba(10, 60, 40, .3); }
.phone img { display: block; width: 100%; border-radius: 34px; }
ul.ticks { list-style: none; margin-top: 30px; }
ul.ticks li { font-size: 24px; line-height: 1.75; color: #0d2b22; }
ul.ticks li::before { content: "✓"; color: #0f8a5f; font-weight: 800; margin-right: 12px; }
"""


def page(body, dark=False):
    bg = "body { background: radial-gradient(1200px 800px at 20% 30%, #138a62 0%, #0b5a40 55%, #083d2c 100%); color: #fff; }" if dark else ""
    return f"<!doctype html><html><head><meta charset='utf-8'><style>{CSS}{bg}</style></head><body>{body}</body></html>"


def promo(title, sub, shot, *, ticks=None, layout="top"):
    """layout top: headline above a wide screenshot; side: text left, screenshot right."""
    brand = f"<div class='brand'><img src='{uri(ICON)}'>OMuse</div>"
    tl = "".join(f"<li>{x}</li>" for x in ticks or [])
    if layout == "top":
        return page(f"""{brand}
<div style='position:absolute;left:72px;top:136px;width:1300px'><h1 style='font-size:58px'>{title}</h1><p class='sub'>{sub}</p></div>
<div class='shot' style='left:150px;top:330px;width:1140px'><img src='{uri(shot, 2280)}'></div>""")
    return page(f"""{brand}
<div style='position:absolute;left:72px;top:210px;width:560px'><h1>{title}</h1><p class='sub'>{sub}</p><ul class='ticks'>{tl}</ul></div>
<div class='shot' style='left:690px;top:140px;width:690px;height:620px'><img src='{uri(shot, 2360)}' style='width:1180px;max-width:none;margin-left:-436px'></div>""")


def phone_promo(title, sub, ticks, shot):
    brand = f"<div class='brand'><img src='{uri(ICON)}'>OMuse</div>"
    tl = "".join(f"<li>{x}</li>" for x in ticks)
    return page(f"""{brand}
<div style='position:absolute;left:72px;top:230px;width:720px'><h1>{title}</h1><p class='sub'>{sub}</p><ul class='ticks'>{tl}</ul></div>
<div class='phone' style='left:960px;top:70px;width:380px'><img src='{uri(shot, 1140)}'></div>""")


def featured(desk, phone):
    chips = "".join(f"<span style='border:1.5px solid rgba(255,255,255,.45);border-radius:999px;padding:8px 20px;font-size:21px'>{c}</span>"
                    for c in ["Email", "Calendar", "Browser", "Travel", "City guides", "MCP", "Telegram"])
    return page(f"""
<img src='{uri(ICON)}' style='position:absolute;left:96px;top:190px;width:132px;height:132px;border-radius:30px;box-shadow:0 18px 40px rgba(0,0,0,.25)'>
<div style='position:absolute;left:96px;top:360px;width:640px'>
  <h1 style='font-size:104px;letter-spacing:-2px'>OMuse</h1>
  <p class='sub' style='color:#d8f1e6;font-size:28px'>Your private AI agent — it plans, does the work across your mail, calendar and the web, and asks before it acts.</p>
  <div style='display:flex;flex-wrap:wrap;gap:12px;margin-top:34px'>{chips}</div>
</div>
<div class='shot' style='left:790px;top:130px;width:1100px;border-radius:16px'><img src='{uri(desk, 2200)}'></div>""", dark=True)


async def render(frames):
    async with async_playwright() as p:
        br = await p.chromium.launch()
        pg = await br.new_page(viewport={"width": 1440, "height": 900}, device_scale_factor=1)
        made = []
        for name, html in frames:
            await pg.set_content(html, wait_until="load")
            await pg.wait_for_timeout(300)
            png = await pg.screenshot()
            im = Image.open(io.BytesIO(png)).convert("RGB")
            path = os.path.join(OUT, name + ".webp")
            im.save(path, "WEBP", quality=88, method=6)
            made.append((name, im))
            print("made", path)
        await br.close()
    # overview: every image in a two-column collage, as the Market listing shows them
    w, h, gap = 720, 450, 30
    rows = (len(made) + 1) // 2
    sheet = Image.new("RGB", (2 * w + 3 * gap, rows * (h + gap) + gap), (244, 247, 245))
    for i, (_n, im) in enumerate(made):
        sheet.paste(im.resize((w, h), Image.LANCZOS), (gap + (i % 2) * (w + gap), gap + (i // 2) * (h + gap)))
    sheet.save(os.path.join(OUT, "overview.png"), optimize=True)
    print("made overview.png")


def main():
    s = asyncio.run(screenshots())
    frames = [
        ("featured", featured(s["chat"], s["phone_city"])),
        ("promote-1-chat", promo("Tell it what you need. It plans — and does it.",
                                 "OMuse breaks a request into steps, runs independent ones in parallel and works through your mail, calendar, Notion, Slack and the web.",
                                 s["chat"])),
        ("promote-2-approval", promo("Nothing risky happens without you.",
                                     "Sending, paying, filling in your passport number: Sentinel, an independent guard, holds it until you approve. Secrets stay in the Vault.",
                                     s["approval"])),
        ("promote-3-travel", promo("Arrival cards, filled. You press Submit.",
                                   "Flight and hotel from your mail, your details from the profile, the passport from the Vault — the form is ready, you review it.",
                                   s["takeover"], layout="side",
                                   ticks=["11 countries' arrival cards", "Today's trip, gaps and conflicts", "Delays, lost bags, expense reports"])),
        ("promote-4-cityguides", promo("City guides for 15 Asian cities.",
                                       "Neighbourhoods, rents, schools, getting around and business customs for Westerners — answered from dated facts, without opening a browser.",
                                       s["city"])),
        ("promote-5-connections", promo("Your tools, your keys, your box.",
                                        "Mail (any IMAP), calendars, Notion, Slack, Telegram and any MCP server. Credentials are encrypted in the Sentinel vault.",
                                        s["connections"])),
        ("promote-6-audit", promo("Every action on a tamper-evident audit trail.",
                                  "Model calls, tool calls, approvals and policy decisions — hash-chained, on your own machine.",
                                  s["audit"])),
        ("promote-7-mobile", phone_promo("Runs at home. Answers in your pocket.",
                                         "Chat from your phone or Telegram, approve with one tap, and take over the browser when a login needs you.",
                                         ["Telegram remote control & one-tap approvals", "Live browser view with human takeover",
                                          "English or Chinese interface"], s["phone_trip"])),
    ]
    asyncio.run(render(frames))


if __name__ == "__main__":
    main()
