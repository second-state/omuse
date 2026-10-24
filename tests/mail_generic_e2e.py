"""Non-Gmail mailboxes against a real IMAP server (Dovecot on 127.0.0.1:1143) and an SMTP server (aiosmtpd on :1025),
through a running local stack (tests/run_local.sh): the mailbox is connected with Sentinel's API and every mail operation
is the tool call the agent makes (/internal/act), approvals included.

Start Dovecot first (tests/dovecot.conf, see the docstring there); this script starts its own SMTP server, the fake
Microsoft sign-in (tests/fake_ms.py on :18999, [endpoints] ms_login) and seeds the mailbox. Covers: the connect flow,
search translation (folders, from:, is:unread, UTF-8 text, dates, OR), message / thread / attachment, drafts, send + Sent
copy, archive (Archive folder created), labels as folders (UTF-7 names), stars, read state, unsubscribe targets, and
Outlook (device-code sign-in, XOAUTH2 for IMAP and SMTP, token caching)."""
import base64
import email.utils
import imaplib
import sys
import threading
import time

import httpx

sys.path.insert(0, ".")
from aiosmtpd.controller import Controller  # noqa: E402
from aiosmtpd.smtp import AuthResult, LoginPassword  # noqa: E402

from tests import fake_ms  # noqa: E402

USER, PW = "tester@example.com", "secretpass123"
B, H, RT = "http://127.0.0.1:8080", {"X-Persona-UI": "1"}, {"X-Persona-Runtime": "rt-test"}
c = httpx.Client(timeout=60, trust_env=False)
fails = []


def check(name, cond, info=""):
    print(("PASS " if cond else "FAIL ") + name, "" if cond else str(info)[:600])
    if not cond:
        fails.append(name)


class Sink:
    def __init__(self):
        self.msgs = []
        self.xoauth = []

    async def auth_XOAUTH2(self, server, args):  # aiosmtpd custom AUTH mechanism
        import base64
        raw = base64.b64decode(args[1]).decode() if len(args) > 1 else ""
        self.xoauth.append(raw)
        ok = raw.startswith("user=me@outlook.com\x01auth=Bearer at-") and raw.endswith("\x01\x01")
        return AuthResult(success=ok, handled=False)

    async def handle_DATA(self, server, session, envelope):
        self.msgs.append((envelope.mail_from, envelope.rcpt_tos, envelope.content))
        return "250 OK"


def auth(server, session, envelope, mechanism, data):
    ok = isinstance(data, LoginPassword) and data.login.decode() == USER and data.password.decode() == PW
    return AuthResult(success=ok)


def seed():
    m = imaplib.IMAP4("127.0.0.1", 1143)
    m.login(USER, PW)
    for box in ("INBOX", "Sent", "Drafts", "Archive", "&XeVPXA-"):  # &XeVPXA- = 工作
        try:
            m.select(box)
            m.store("1:*", "+FLAGS", "(\\Deleted)")
            m.expunge()
        except Exception:
            pass
    m.select("INBOX")  # never delete the selected mailbox (Dovecot drops the connection)
    for box in ("Archive", "&XeVPXA-"):
        m.delete(box)
    now = email.utils.formatdate(localtime=True)
    old = email.utils.formatdate(time.time() - 40 * 86400, localtime=True)
    msgs = [
        ("\\Seen", f"From: Boss <boss@acme.com>\r\nTo: {USER}\r\nSubject: Quarterly plan\r\nDate: {now}\r\n"
                   "Message-ID: <q1@acme.com>\r\n\r\nPlease review the quarterly plan by Friday.\r\n"),
        ("", f"From: =?utf-8?b?6LSi5Yqh?= <finance@acme.com>\r\nTo: {USER}\r\nSubject: =?utf-8?b?5Y+R56Wo5oql6ZSA?=\r\n"
             f"Date: {now}\r\nMessage-ID: <f1@acme.com>\r\nContent-Type: text/plain; charset=utf-8\r\n"
             "Content-Transfer-Encoding: 8bit\r\n\r\n" + "请在月底前提交发票报销单。".encode().decode("latin-1") + "\r\n"),
        ("", f"From: Shop <news@shop.com>\r\nTo: {USER}\r\nSubject: Big sale today\r\nDate: {now}\r\nMessage-ID: <n1@shop.com>\r\n"
             "List-Unsubscribe: <https://shop.com/unsub/XYZ>, <mailto:leave@shop.com>\r\nList-Unsubscribe-Post: List-Unsubscribe=One-Click\r\n"
             "\r\n50% off everything.\r\n"),
        ("\\Seen", f"From: Old <old@acme.com>\r\nTo: {USER}\r\nSubject: Old news\r\nDate: {old}\r\nMessage-ID: <o1@acme.com>\r\n\r\nLast month.\r\n"),
        ("", f"From: Boss <boss@acme.com>\r\nTo: {USER}\r\nSubject: Re: Quarterly plan\r\nDate: {now}\r\nMessage-ID: <q2@acme.com>\r\n"
             "In-Reply-To: <q1@acme.com>\r\nReferences: <q1@acme.com>\r\nContent-Type: multipart/mixed; boundary=BB\r\n\r\n--BB\r\n"
             "Content-Type: text/plain\r\n\r\nAttached the numbers.\r\n--BB\r\nContent-Type: text/csv\r\n"
             "Content-Disposition: attachment; filename=\"numbers.csv\"\r\n\r\na,b\r\n1,2\r\n--BB--\r\n"),
    ]
    for flags, raw in msgs:
        when = time.time() - (40 * 86400 if "Old news" in raw else 0)  # SINCE / BEFORE use the arrival (internal) date
        m.append("INBOX", f"({flags})" if flags else None, imaplib.Time2Internaldate(when), raw.encode("latin-1"))
    m.append("Sent", "(\\Seen)", imaplib.Time2Internaldate(time.time()),
             f"From: {USER}\r\nTo: boss@acme.com\r\nSubject: Re: Quarterly plan\r\nDate: {now}\r\nMessage-ID: <q3@example.com>\r\n"
             "In-Reply-To: <q1@acme.com>\r\nReferences: <q1@acme.com>\r\n\r\nWill do.\r\n".encode())
    m.logout()


def main():
    seed()
    sink = Sink()
    ctl = Controller(sink, hostname="127.0.0.1", port=1025, authenticator=auth, auth_require_tls=False)
    ctl.start()
    try:
        acc = connect()
        run(sink, acc)
        outlook(sink)
    finally:
        ctl.stop()
    print("\n%d failures" % len(fails), fails)
    sys.exit(1 if fails else 0)


BODY = {"provider": "custom", "email": USER, "app_password": PW, "display_name": "Tester",
        "imap_host": "127.0.0.1", "imap_port": 1143, "imap_security": "none",
        "smtp_host": "127.0.0.1", "smtp_port": 1025, "smtp_security": "none"}


def act(tool, args, decision="approve"):
    """The agent's tool call; an approval card is resolved with `decision` and the action's own result returned."""
    r = c.post(B + "/internal/act", headers=RT,
               json={"task_id": "mail-t1", "call_id": "c" + str(time.time()), "tool": tool, "args": args}).json()
    if r.get("status") == "approval_required" and decision:
        res = c.post(B + f"/sentinel/api/approvals/{r['approval_id']}/resolve", headers=H, json={"decision": decision}).json()
        return {"approval": r, **(res.get("result") or res)}
    return r


def result(r):
    return r.get("result") if isinstance(r.get("result"), dict) else {}


def search(q, n=10):
    r = act("gmail_search", {"query": q, "max_results": n})
    return result(r).get("messages", []) if r.get("status") == "ok" else r


def subjects(msgs):
    return [m.get("subject") for m in msgs] if isinstance(msgs, list) else msgs


def connect():
    """Connect the mailbox through Sentinel's API, as Settings > Connections does."""
    r = c.post(B + "/sentinel/api/connections/gmail/credential", json={**BODY, "app_password": "wrong"}, headers=H)
    check("API: wrong password rejected, nothing saved", r.status_code == 400, r.text)
    r = c.post(B + "/sentinel/api/connections/gmail/credential", json={**BODY, "imap_host": "10.0.0.5"}, headers=H)
    check("API: unencrypted non-loopback refused", r.status_code == 400 and "localhost" in r.text, r.text)
    r = c.post(B + "/sentinel/api/connections/gmail/credential",
               json={**BODY, "provider": "qq", "email": "tester@qq.com", "app_password": "wrong"}, headers=H)
    check("bad password: provider-specific hint", r.status_code == 400 and "授权码" in r.text, r.text)
    r = c.post(B + "/sentinel/api/connections/gmail/credential", json=BODY, headers=H)
    check("API: custom IMAP connected", r.status_code == 200 and r.json()["ok"], r.text)
    conn = c.get(B + "/sentinel/api/connections", headers=H).json()
    gm = next(x for x in conn["connections"] if x["name"] == "gmail")
    acc = next((a for a in gm["accounts"] if a["email"] == USER), None)
    check("API: account listed with provider", acc and acc["provider"] == "custom" and acc["ready"], gm["accounts"])
    check("API: presets exposed", any(p["key"] == "outlook" and p["auth"] == "oauth" for p in gm["providers"]), "")
    t = c.post(B + "/sentinel/api/connections/gmail/test", json={"account": acc["id"]}, headers=H).json()
    tt = t.get("test") or {}
    check("test: generic mode, folders found", t.get("ok") and tt.get("provider") == "custom" and tt.get("sent") == "Sent"
          and tt.get("drafts") == "Drafts", t)
    check("test: unread count", tt.get("unread_inbox") == 3, t)
    return acc


def run(sink, acc):
    res = search("in:inbox")
    check("search in:inbox -> 5 messages, folder ids", isinstance(res, list) and len(res) == 5
          and all(r["id"].startswith("i-") for r in res), res)
    res = res if isinstance(res, list) else []
    check("decoded subjects / from", any(r["subject"] == "发票报销" and "财务" in r["from"] for r in res), subjects(res))
    check("search from:boss", len(search("from:boss@acme.com")) == 2)
    check("search is:unread", len(search("is:unread")) == 3)
    r = search("发票")
    check("search UTF-8 text", subjects(r) == ["发票报销"], r)
    check("search newer_than:7d excludes old", len(search("newer_than:7d")) == 4)
    check("search older_than:30d", subjects(search("older_than:30d")) == ["Old news"])
    check("search OR", len(search("subject:sale OR subject:news")) == 2)
    check("search {a b}", len(search("{from:news@shop.com from:old@acme.com}")) == 2)
    check("search -category:promotions", all("sale" not in x["subject"] for x in search("-category:promotions")))
    check("search category:promotions", subjects(search("category:promotions")) == ["Big sale today"])
    check("search has:attachment", subjects(search("has:attachment")) == ["Re: Quarterly plan"])
    check("search in:sent", [x["id"][:2] for x in search("in:sent")] == ["s-"])
    check("search in:anywhere covers sent", len(search("in:anywhere quarterly")) == 3)
    r = act("gmail_search", {"query": "label:Nope", "max_results": 5})
    check("unknown folder errors with folder list", r.get("status") == "error" and "Sent" in r.get("error", ""), r)
    sale = next((x for x in res if x["subject"] == "Big sale today"), {})
    check("unsubscribe method visible, URL hidden", sale.get("unsubscribe") == "one-click" and "XYZ" not in str(res), sale)

    reply = next((x for x in res if x["subject"] == "Re: Quarterly plan"), {})
    full = act("gmail_get_message", {"message_id": reply.get("id")})
    check("get_message body + attachment list", full.get("status") == "ok" and "numbers" in str(full)
          and "numbers.csv" in str(result(full).get("attachments")), full)
    r = act("gmail_save_attachment", {"message_id": reply.get("id"), "filename": "numbers.csv"})
    data = base64.b64decode(result(r).get("data_b64") or "")
    check("get_attachment", r.get("status") == "ok" and result(r).get("filename") == "numbers.csv" and b"1,2" in data, r)
    th = act("gmail_get_thread", {"thread_id": result(full).get("thread_id")})  # the agent reads it off the message
    tm = result(th).get("messages", [])
    check("thread via References (inbox + sent)", len(tm) == 3 and tm[0]["subject"] == "Quarterly plan", (full, th))

    d = act("gmail_create_draft", {"to": "boss@acme.com", "subject": "", "body": "Draft reply", "reply_to_message_id": reply.get("id")})
    dr = search("in:drafts", 5)
    check("draft saved in Drafts", d.get("status") == "ok" and result(d).get("folder") == "Drafts" and isinstance(dr, list) and dr
          and dr[0]["subject"] in ("Re: Quarterly plan", "Re: Re: Quarterly plan"), (d, dr))

    s = act("gmail_send", {"to": "boss@acme.com", "subject": "Hello", "body": "Body text", "reply_to_message_id": reply.get("id")})
    check("send needs approval", "approval" in s, s)
    check("send via SMTP (login)", s.get("status") == "ok" and len(sink.msgs) == 1 and sink.msgs[0][1] == ["boss@acme.com"],
          (s, sink.msgs))
    check("sent copy appended to Sent", result(s).get("saved_to_sent") is True
          and any(x["subject"] in ("Re: Quarterly plan", "Hello") for x in search("in:sent subject:Hello", 5)), s)
    check("reply headers", sink.msgs and b"In-Reply-To: <q2@acme.com>" in sink.msgs[0][2], sink.msgs[:1])

    old = next(x for x in search("in:inbox") if x["subject"] == "Old news")
    a = act("gmail_archive", {"message_ids": [old["id"]]})
    labels = result(act("gmail_list_labels", {})).get("labels", [])
    check("archive moves to Archive (created)", a.get("status") == "ok" and result(a).get("archived") == 1 and "Archive" in labels,
          (a, labels))
    arch = search("in:archive", 5)
    check("archived message found in Archive", subjects(arch) == ["Old news"] and arch[0]["id"].startswith("a-"), arch)
    check("default search covers Archive", "Old news" in subjects(search("older_than:30d")))

    boss = next(x for x in search("in:inbox") if x["subject"] == "Quarterly plan")
    lab = act("gmail_label", {"message_ids": [boss["id"]], "add_labels": ["STARRED", "工作"]})
    labels = result(act("gmail_list_labels", {})).get("labels", [])
    check("label: star + UTF-7 folder copy", result(lab).get("added") == 2 and "工作" in labels, (lab, labels))
    check("search label:<chinese folder>", subjects(search("label:工作", 5)) == ["Quarterly plan"])
    check("is:starred", subjects(search("is:starred", 5)) == ["Quarterly plan"])
    lab = act("gmail_label", {"message_ids": [boss["id"]], "remove_labels": ["STARRED"]})
    check("unstar", result(lab).get("removed") == 1 and not search("is:starred", 5), lab)
    mr = act("gmail_label", {"message_ids": [sale.get("id")], "mark_read": True})
    check("mark read", result(mr).get("updated") == 1 and len(search("is:unread")) == 2, mr)
    u = act("gmail_unsubscribe", {"message_ids": [sale.get("id")]}, decision="deny")
    check("unsubscribe targets shown on the approval card", u.get("status") == "denied"
          and "one-click" in str(u.get("approval", {}).get("summary")), u)
    c.delete(B + f"/sentinel/api/connections/gmail/accounts/{acc['id']}", headers=H)


def outlook(sink):
    """Outlook end to end through the API: device-code sign-in (fake Microsoft endpoint), XOAUTH2 to IMAP (Dovecot
    verifies the token through the same fake's introspection endpoint) and SMTP, access-token caching."""
    srv = fake_ms.serve()
    threading.Thread(target=srv.serve_forever, daemon=True).start()
    cid = "11111111-2222-3333-4444-555555555555"
    form = {"email": "me@outlook.com", "imap_host": "127.0.0.1", "imap_port": 1143, "imap_security": "none",
            "smtp_host": "127.0.0.1", "smtp_port": 1025, "smtp_security": "none"}
    try:
        r = c.post(B + "/sentinel/api/connections/gmail/oauth/start", json={**form, "client_id": "not-a-guid"}, headers=H)
        check("MS: bad client id rejected locally", r.status_code == 400 and "GUID" in r.text, r.text)
        r = c.post(B + "/sentinel/api/connections/gmail/oauth/start",
                   json={**form, "client_id": "99999999-2222-3333-4444-555555555555"}, headers=H)
        check("MS: unknown app -> readable error", r.status_code == 400 and "AADSTS700016" in r.text, r.text)
        r = c.post(B + "/sentinel/api/connections/gmail/credential", json={"email": "me@outlook.com", "app_password": "x" * 16},
                   headers=H)
        check("API: Outlook requires Microsoft sign-in", r.status_code == 400 and "Microsoft" in r.text, r.text)
        d = c.post(B + "/sentinel/api/connections/gmail/oauth/start", json={**form, "client_id": cid}, headers=H).json()
        check("MS: device code", d.get("user_code") == "ABCD-1234" and d.get("interval") == 1, d)
        polls = []
        for _ in range(20):
            p = c.get(B + f"/sentinel/api/connections/gmail/oauth/{d.get('flow')}", headers=H).json()
            polls.append(p.get("status"))
            if p.get("status") != "pending":
                break
            time.sleep(1.1)
        check("MS: pending then token, account connected", polls[0] == "pending" and polls[-1] == "done", (polls, p))
        conn = c.get(B + "/sentinel/api/connections", headers=H).json()
        gm = next(x for x in conn["connections"] if x["name"] == "gmail")
        acc = next((a for a in gm["accounts"] if a["email"] == "me@outlook.com"), {})
        check("Outlook: account listed", acc.get("provider") == "outlook" and acc.get("ready"), gm["accounts"])
        t = c.post(B + "/sentinel/api/connections/gmail/test", json={"account": acc.get("id")}, headers=H).json()
        check("Outlook: IMAP XOAUTH2 login + generic mode", t.get("ok") and t["test"].get("provider") == "outlook", t)
        n_before = fake_ms.STATE["n"]
        c.post(B + "/sentinel/api/connections/gmail/test", json={"account": acc.get("id")}, headers=H)
        check("Outlook: access token cached", fake_ms.STATE["n"] == n_before, fake_ms.STATE)
        r = act("gmail_send", {"to": "friend@example.com", "subject": "Hi from Outlook", "body": "Hello",
                               "from_account": "me@outlook.com"})
        check("Outlook: SMTP XOAUTH2 send", r.get("status") == "ok" and sink.xoauth and sink.msgs[-1][1] == ["friend@example.com"],
              (r, sink.xoauth))
        check("Outlook: sent copy kept", result(r).get("saved_to_sent") is True, r)
        c.delete(B + f"/sentinel/api/connections/gmail/accounts/{acc.get('id')}", headers=H)
    finally:
        srv.shutdown()


if __name__ == "__main__":
    main()
