"""Official sites, served locally inside the test container: eservices.ica.gov.sg and www.fwd.com.sg resolve to
127.0.0.1 (docker --add-host, tests/run_in_docker.sh) and this server answers them over HTTPS on 443 with a certificate
from a throw-away test CA that only the container's Chromium trusts (its NSS database). The pages are the stand-ins in
tests/pages, at the real sites' paths, without their "test stand-in" notes: with a real model the agent must see the
official site — it rightly refuses to type a passport or a card into a page that says it is a test copy.

    python3 tests/https_standins.py            # creates the CA and the certificate once, then serves on :443

Never on the host: it is started by tests/run_local.sh only when OMUSE_STANDINS=1 (set inside the container)."""
from __future__ import annotations

import http.server
import os
import re
import ssl
import subprocess
import sys
from urllib.parse import urlsplit

PAGES = os.path.join(os.path.dirname(os.path.abspath(__file__)), "pages")
DIR = "/tmp/claude-0/standins"
HOSTS = ["eservices.ica.gov.sg", "www.fwd.com.sg"]
# the real sites' paths (what the skills navigate to) → the stand-in pages; any other path is the same-named file
ROUTES = {
    "eservices.ica.gov.sg": {"/": "sgac.html", "/sgarrivalcard": "sgac.html", "/sgarrivalcard/": "sgac.html"},
    "www.fwd.com.sg": {"/": "fwd_sg_quote.html", "/travel-insurance/online-quote/": "fwd_sg_quote.html",
                       "/travel-insurance/online-quote": "fwd_sg_quote.html",
                       "/claim-online/travel.html": "fwd_sg_claim.html"},
}
NOTE = re.compile(r'<p class="note">[^<]*(?:test|stand-in)[^<]*</p>\s*', re.I)
TITLE_TEST = re.compile(r"\s*\(test\)\s*(?=</title>)", re.I)


def certificate() -> tuple[str, str]:
    """The CA (for the NSS database) and the server certificate for HOSTS, made once per container."""
    os.makedirs(DIR, exist_ok=True)
    ca_key, ca, key, csr, crt = (os.path.join(DIR, n) for n in ("ca.key", "ca.pem", "site.key", "site.csr", "site.pem"))
    if not os.path.exists(crt):
        run = lambda *a: subprocess.run(["openssl", *a], check=True, capture_output=True)  # noqa: E731
        run("req", "-x509", "-newkey", "rsa:2048", "-nodes", "-days", "30", "-keyout", ca_key, "-out", ca,
            "-subj", "/CN=OMuse test stand-ins CA")
        run("req", "-newkey", "rsa:2048", "-nodes", "-keyout", key, "-out", csr, "-subj", f"/CN={HOSTS[0]}")
        ext = os.path.join(DIR, "san.ext")
        open(ext, "w").write("subjectAltName=" + ",".join(f"DNS:{h}" for h in HOSTS) + "\n")
        run("x509", "-req", "-in", csr, "-CA", ca, "-CAkey", ca_key, "-CAcreateserial", "-days", "30",
            "-out", crt, "-extfile", ext)
    return ca, crt


class Handler(http.server.SimpleHTTPRequestHandler):
    def __init__(self, *a, **kw):
        super().__init__(*a, directory=PAGES, **kw)

    def do_GET(self):  # noqa: N802
        host = (self.headers.get("Host") or "").split(":")[0].lower()
        path = urlsplit(self.path).path
        name = ROUTES.get(host, {}).get(path) or path.lstrip("/")
        f = os.path.join(PAGES, os.path.basename(name))
        if host not in ROUTES or not os.path.isfile(f):
            self.send_error(404)
            return
        body = open(f, "rb").read()
        if f.endswith(".html"):
            body = TITLE_TEST.sub("", NOTE.sub("", body.decode("utf-8"))).encode("utf-8")
        self.send_response(200)
        self.send_header("Content-Type", self.guess_type(f))
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, fmt, *args):
        sys.stderr.write("%s %s\n" % (self.headers.get("Host"), fmt % args))


if __name__ == "__main__":
    _, crt = certificate()
    srv = http.server.ThreadingHTTPServer(("0.0.0.0", 443), Handler)
    ctx = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
    ctx.load_cert_chain(crt, os.path.join(DIR, "site.key"))
    srv.socket = ctx.wrap_socket(srv.socket, server_side=True)
    srv.serve_forever()
