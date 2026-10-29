"""The official sites' stand-ins inside the test container (tests/https_standins.py): the browser opens
https://eservices.ica.gov.sg/sgarrivalcard/ and https://www.fwd.com.sg/travel-insurance/online-quote/ over HTTPS with
the test CA trusted, gets the stand-in pages at the real paths, and sees no "test" note or title on them."""
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from tests import drive_common as D  # noqa: E402
from tests.skills_common import check, finish  # noqa: E402


def page(url):
    r = D.act("browser_navigate", {"url": url}, task="t-standins")
    return str(r.get("result") or r)


ica = page("https://eservices.ica.gov.sg/sgarrivalcard/")
check("eservices.ica.gov.sg opens over HTTPS (test CA trusted) with the arrival card form",
      "SG Arrival Card" in ica and "Passport number" in ica and "ERR_CERT" not in ica, ica[:400])
check("the ICA stand-in carries no test note or title", "(test)" not in ica and "stand-in" not in ica.lower(), ica[:400])
fwd = page("https://www.fwd.com.sg/travel-insurance/online-quote/")
check("www.fwd.com.sg's quote path serves the quote page", "Get a travel insurance quote" in fwd and "ERR_CERT" not in fwd, fwd[:400])
check("the FWD stand-in carries no test note or title", "stand-in" not in fwd.lower() and "made up" not in fwd.lower()
      and "(test)" not in fwd, fwd[:400])
nf = page("https://www.fwd.com.sg/no-such-page.html")
check("an unknown path is a 404, not another page", "Get a travel insurance quote" not in nf, nf[:200])
finish()
