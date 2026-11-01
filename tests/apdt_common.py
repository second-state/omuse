"""The world of tests/apdt_real.py (Hong Kong's air passenger departure tax refund, skills/hk-apdt-refund): the trip's
documents as the airlines send them (PDF boarding passes and an e-ticket receipt with the departure tax), the user's
passport photo, and what the user can read back from the official site's stand-in (tests/pages/apdt*.html) and from
the app's browser view."""
from __future__ import annotations

import base64
import datetime as dt
import io
import json
import os
import re
from urllib.parse import parse_qs, urlsplit

SITE = "https://app.apdtrefund.hk/hkTaxMobile/"
TODAY = dt.date.today()
ARRIVE = TODAY - dt.timedelta(days=3)    # SQ 890 Singapore → Hong Kong
DEPART = TODAY - dt.timedelta(days=2)    # CX 500 Hong Kong → Tokyo, the next calendar day, on a separate ticket
PASSPORT = "E1234567X"
CARD = "4111111111111111"


def _d(d: dt.date) -> str:
    return d.strftime("%d %b %Y").upper()


# ------------------------------------------------------------------ documents
def pdf(title: str, lines: list[str]) -> bytes:
    """A one-page text PDF (Helvetica), as an airline's mail attachment: readable by any PDF reader and text extractor."""
    def esc(s: str) -> str:
        return s.replace("\\", "\\\\").replace("(", "\\(").replace(")", "\\)")
    ops = ["BT", "/F2 16 Tf", "50 790 Td", f"({esc(title)}) Tj", "/F1 11 Tf", "0 -26 Td"]
    for ln in lines:
        ops += [f"({esc(ln)}) Tj", "0 -16 Td"]
    ops.append("ET")
    stream = "\n".join(ops).encode("latin-1")
    objs = [b"<< /Type /Catalog /Pages 2 0 R >>",
            b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
            b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 595 842] /Contents 4 0 R /Resources << /Font << /F1 5 0 R /F2 6 0 R >> >> >>",
            b"<< /Length %d >>\nstream\n" % len(stream) + stream + b"\nendstream",
            b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>",
            b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica-Bold /Encoding /WinAnsiEncoding >>"]
    out, offs = io.BytesIO(), []
    out.write(b"%PDF-1.4\n%\xe2\xe3\xcf\xd3\n")
    for i, o in enumerate(objs, 1):
        offs.append(out.tell())
        out.write(b"%d 0 obj\n" % i + o + b"\nendobj\n")
    x = out.tell()
    out.write(b"xref\n0 %d\n0000000000 65535 f \n" % (len(objs) + 1))
    for o in offs:
        out.write(b"%010d 00000 n \n" % o)
    out.write(b"trailer\n<< /Size %d /Root 1 0 R >>\nstartxref\n%d\n%%%%EOF\n" % (len(objs) + 1, x))
    return out.getvalue()


def boarding_pass_sq890() -> bytes:
    return pdf("SINGAPORE AIRLINES  -  BOARDING PASS", [
        "Name: LU/LUCAS MR", "Flight: SQ 890", "From: SINGAPORE (SIN) Changi Terminal 3", "To: HONG KONG (HKG)",
        f"Date: {_d(ARRIVE)}", "Departs: 09:40   Arrives: 13:35 (local time)", "Boarding time: 09:00   Gate: B6   Seat: 41A",
        "Class: Economy   Seq: 112", "Booking reference: 6PZK4T", "E-ticket: 618 2345611122",
        "M1LU/LUCAS MR          E6PZK4T SINHKGSQ 0890 112Y041A0112 100"])


def boarding_pass_cx500() -> bytes:
    return pdf("CATHAY PACIFIC  -  BOARDING PASS", [
        "Name: LU/LUCAS MR", "Flight: CX 500", "From: HONG KONG (HKG) Terminal 1", "To: TOKYO NARITA (NRT)",
        f"Date: {_d(DEPART)}", "Departs: 09:05   Arrives: 14:30 (local time)", "Boarding time: 08:25   Gate: 23   Seat: 54K",
        "Class: Economy   Seq: 087", "Booking reference: 7HKQ2P", "E-ticket: 160 2345678901",
        "M1LU/LUCAS MR          E7HKQ2P HKGNRTCX 0500 087Y054K0087 100"])


def eticket_cx500() -> bytes:
    issued = DEPART - dt.timedelta(days=20)
    return pdf("CATHAY PACIFIC  -  ELECTRONIC TICKET ITINERARY / RECEIPT", [
        "Passenger: LU/LUCAS MR", "Ticket number: 160 2345678901", "Booking reference: 7HKQ2P",
        f"Date of issue: {_d(issued)}   Place of issue: CATHAY PACIFIC WEBSITE", "",
        "Flight   From                 To                    Date          Dep    Arr    Class  Status",
        f"CX 500   HONG KONG (HKG)      TOKYO NARITA (NRT)    {_d(DEPART)}   09:05  14:30  V      OK", "",
        "Fare: HKD 2,860.00", "Taxes, fees and charges:",
        "  HK   HKD 120.00   Hong Kong Air Passenger Departure Tax",
        "  G3   HKD  90.00   Hong Kong Airport Construction Fee",
        "  I5   HKD  70.00   Hong Kong Airport Security Charge",
        "  YR   HKD 180.00   Carrier-imposed surcharge",
        "Total: HKD 3,320.00", "Form of payment: VISA XXXXXXXXXXXX1111", "",
        "This receipt is your proof of payment of the taxes shown above."])


def passport_jpg() -> bytes:
    """The photo page of the user's passport, as a phone photo of it would show it (JPEG)."""
    from PIL import Image, ImageDraw, ImageFont
    im = Image.new("RGB", (1200, 820), (226, 232, 222))
    g = ImageDraw.Draw(im)
    f = lambda n: ImageFont.load_default(size=n)   # noqa: E731
    g.rectangle((0, 0, 1199, 819), outline=(120, 130, 120), width=4)
    g.text((40, 28), "PASSPORT   PASSEPORT", font=f(34), fill=(30, 60, 40))
    g.text((640, 28), "AUSTRALIA", font=f(34), fill=(30, 60, 40))
    g.rectangle((40, 110, 360, 520), fill=(200, 205, 200), outline=(90, 90, 90), width=2)
    g.ellipse((130, 170, 270, 330), fill=(170, 150, 130))
    g.rectangle((110, 330, 290, 520), fill=(60, 70, 90))
    rows = [("Type / Type", "P"), ("Country code / Code du pays", "AUS"), ("Passport No. / No. du passeport", PASSPORT),
            ("Surname / Nom", "LU"), ("Given names / Prenoms", "LUCAS"), ("Nationality / Nationalite", "AUSTRALIAN"),
            ("Date of birth / Date de naissance", "12 APR 1985"), ("Sex / Sexe", "M"),
            ("Date of issue / Date de delivrance", "02 MAY 2021"), ("Date of expiry / Date d'expiration", "01 MAY 2031")]
    y = 110
    for k, v in rows:
        g.text((400, y), k, font=f(17), fill=(80, 90, 80))
        g.text((400, y + 19), v, font=f(26), fill=(10, 10, 10))
        y += 50 if k.startswith(("Type", "Country")) else 44
    g.rectangle((0, 640, 1199, 819), fill=(240, 240, 236))
    g.text((40, 670), "P<AUSLU<<LUCAS<<<<<<<<<<<<<<<<<<<<<<<<<<<<<<<", font=f(36), fill=(0, 0, 0))
    g.text((40, 730), f"{PASSPORT}5AUS8504124M3105017<<<<<<<<<<<<<<04", font=f(36), fill=(0, 0, 0))
    buf = io.BytesIO()
    im.save(buf, "JPEG", quality=88)
    return buf.getvalue()


def mail() -> list:
    """The user's mailbox: the airlines' mails with their PDF attachments (skills_common.seed_mail tuples)."""
    a, d = ARRIVE.isoformat(), DEPART.isoformat()
    return [
        ("Singapore Airlines <boardingpass@singaporeair.example>", f"Your boarding pass: SQ 890 Singapore to Hong Kong on {a}",
         f"Dear Mr Lu,\n\nYou are checked in for SQ 890 from Singapore (SIN) to Hong Kong (HKG) on {a}, departing 09:40. "
         "Your boarding pass is attached. Booking reference 6PZK4T.\n\nSingapore Airlines", 4,
         [("BoardingPass_SQ890_LU_LUCAS.pdf", boarding_pass_sq890())]),
        ("Cathay Pacific <eticket@cathaypacific.example>", f"Your e-ticket receipt: CX 500 Hong Kong to Tokyo Narita, {d}",
         f"Dear Mr Lucas Lu,\n\nThank you for booking with Cathay Pacific. Booking reference 7HKQ2P, ticket 160 2345678901. "
         f"CX 500 Hong Kong (HKG) 09:05 to Tokyo Narita (NRT) 14:30 on {d}. Your electronic ticket itinerary / receipt, "
         "with the fare, taxes and charges you paid, is attached.\n\nCathay Pacific", 22,
         [("ETKT_1602345678901_Receipt.pdf", eticket_cx500())]),
        ("Cathay Pacific <checkin@cathaypacific.example>", f"Boarding pass: CX 500 Hong Kong to Tokyo Narita, {d}",
         f"Dear Mr Lu,\n\nYou are checked in for CX 500 on {d}. Please find your boarding pass attached. "
         "Gate closes 20 minutes before departure.\n\nCathay Pacific", 3,
         [("BoardingPass_CX500_LU_LUCAS.pdf", boarding_pass_cx500())]),
        ("Tokyo Bay Hotel <stay@tokyobayhotel.example>", f"Reservation confirmed: Tokyo Bay Hotel, check-in {d}",
         f"Confirmation TB-88213. Check-in {d}, 4 nights. 1-2-3 Kaigan, Minato-ku, Tokyo.", 10),
    ]


# ------------------------------------------------------------------ what the site holds
def docsum(v: str) -> str:
    """The site's check value of a document / card number (the same function as tests/pages/apdt.js: APDT.sum)."""
    h = 5381
    for ch in v.upper().replace(" ", "").replace("(", "").replace(")", ""):
        h = ((h * 33) ^ ord(ch)) & 0xFFFFFFFF
    return f"{h:08x}"


def application(url: str) -> dict:
    """The application as the site's submit / face page carries it in its address (bizId, tsType and the t token)."""
    q = {k: v[0] for k, v in parse_qs(urlsplit(url).query).items()}
    out = {"page": urlsplit(url).path.rsplit("/", 1)[-1], "bizId": q.get("bizId", ""), "tsType": q.get("tsType", "")}
    t = q.get("t", "")
    if t:
        try:
            out.update(json.loads(base64.urlsafe_b64decode(t + "=" * (-len(t) % 4)).decode("utf-8")))
        except Exception as e:   # noqa: BLE001
            out["token_error"] = repr(e)
    return out


# ------------------------------------------------------------------ the user in the app's UI
async def attach(u, paths: list[str]) -> None:
    """The user clicks ＋ in the chat composer and picks files; waits until each chip shows its size (uploaded)."""
    pg = u.page
    await pg.locator(".composer input[type=file]").set_input_files(paths)
    await pg.wait_for_function(f"() => document.querySelectorAll('.composer .chip-att').length >= {len(paths)} && "
                               "[...document.querySelectorAll('.composer .chip-att .small')].every(e => /B$/.test(e.textContent.trim()))",
                               timeout=60000)


async def browser_tab(u, host: str, picture: str | None = None) -> str:
    """The user opens the browser view, picks the tab showing `host` and reads its URL bar (and keeps a picture)."""
    pg = u.page
    came_from = pg.url
    try:
        return await _read_tab(u, host, picture)
    finally:
        await pg.goto(came_from)
        await pg.wait_for_selector("#chatInput", timeout=20000)


async def _read_tab(u, host: str, picture: str | None) -> str:
    pg = u.page
    await pg.goto(u_app(u) + "#browser")
    await pg.wait_for_selector(".bview", timeout=20000)
    await pg.wait_for_timeout(2500)
    tabs = pg.locator(".btabs .btab", has=pg.locator(".host", has_text=host.replace("www.", "")))
    n = await tabs.count()
    if not n:
        print(f"INFO browser view tabs: {await pg.locator('.btabs').inner_text()!r}")
        return ""
    # one tab per request: the user looks at each of the site's tabs and keeps the one furthest along
    order = ["apdt.html", "", "apdt_upload.html", "apdt_refund.html", "apdt_confirm.html", "apdt_submit.html", "apdt_face.html"]
    best, best_i = -1, n - 1
    for i in range(n):
        await tabs.nth(i).click()
        await pg.wait_for_timeout(2000)
        page = urlsplit(await u.view_url()).path.rsplit("/", 1)[-1]
        rank = order.index(page) if page in order else 0
        print(f"INFO browser view tab {i}: {page!r}")
        if rank >= best:
            best, best_i = rank, i
    await tabs.nth(best_i).click()
    await pg.wait_for_timeout(2500)
    url = await u.view_url()
    try:
        await u.view_picture(picture)
    except Exception as e:   # noqa: BLE001 — the URL is what is checked; the picture is for whoever reads the run
        print(f"INFO browser view picture unavailable: {e!r}")
    return url


def u_app(u) -> str:
    from tests import user_common as U
    return U.APP


def write_files(folder: str) -> dict[str, str]:
    """The files the user has on their own device (the passport photo they attach in the chat)."""
    os.makedirs(folder, exist_ok=True)
    p = os.path.join(folder, "passport.jpg")
    open(p, "wb").write(passport_jpg())
    return {"passport": p}


async def type_card(u, request: str) -> None:
    """In the browser view the user takes over and types their card number into the focused Card Number field with the
    view's text box, presses the view's Tab button twice (to Validity Period) and types the expiry. What the user does
    follows the page in front of them (the refund page, where the card goes), never the words of the request."""
    pg = u.page
    url = await u.view_url()
    print(f"INFO takeover request: {request!r} url {url!r}")
    if "apdt_refund" not in url:
        return
    await u.take_over()
    box = pg.locator(".bview input[type=text]:not(.url)")

    async def send(text: str) -> None:
        await box.fill(text)
        await box.press("Enter")
        await pg.wait_for_timeout(800)

    await send(CARD)
    for _ in range(2):
        await pg.locator(".bview button", has_text=re.compile(r"^Tab$")).first.click()
        await pg.wait_for_timeout(500)
    await send("12/29")
    await pg.wait_for_timeout(1500)
