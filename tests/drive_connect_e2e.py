"""Google Drive connection: ONE Google sign-in connects the calendar and Drive together (the Drive card's button asks for
both scopes; the shared callback stores the same refresh token for both), a sign-in where the user unticked Drive on
Google's page connects the calendar only, Drive alone can be asked for, the user's own client works through the relay,
the agent's catalog gets the drive_* tools with their permissions, reads work through Sentinel, and disconnecting Drive
leaves the calendar connected."""
import json
from urllib.parse import parse_qs, urlparse

from tests import drive_common as D
from tests.skills_common import check, finish

c, S, B, F, H = D.c, D.S, D.B, D.F, D.H

D.disconnect_all()
D.reset_drive()
D.managed_client(False)
x = D.conn("drive")
check("no OMuse Google client in this build -> the one-click button is hidden on the Drive card", x["managed_available"] is False and x["mode"] == "", x)
r = c.post(S + "/connections/drive/google/start", json={"origin": B}, headers=H)
check("one-click start refuses clearly without a client", r.status_code == 400, r.text)

# ---------------------------------------------------------------- one sign-in, both services
D.managed_client(True)
r = c.post(S + "/connections/drive/google/start", json={"origin": B}, headers=H).json()
q = parse_qs(urlparse(r["auth_url"]).query)
scope = q["scope"][0]
check("the consent asks for the calendar AND Drive scopes in one go (default services of the Drive card)",
      "auth/calendar.events" in scope and "auth/drive" in scope and r["services"] == ["calendar", "drive"]
      and q["redirect_uri"] == [F + "/relay/callback"] and q["code_challenge_method"] == ["S256"], (scope, r.get("services")))
page = c.get(r["auth_url"], follow_redirects=True)
check("Google -> relay -> the shared callback: both connected on the result page",
      page.status_code == 200 and "lucas@example.com" in page.text and "Google Calendar + Google Drive" in page.text, page.text[:400])
cal, drv = D.conn("calendar"), D.conn("drive")
check("calendar: connected by the same sign-in (mode google, time zone read)", cal["mode"] == "google" and cal["enabled"] and cal["config"]["time_zone"] == "Asia/Singapore", cal)
check("drive: connected, account and quota recorded, permissions seeded",
      drv["mode"] == "google" and drv["enabled"] and drv["has_credential"] and drv["config"]["email"] == "lucas@example.com"
      and drv["config"]["quota"]["used_gb"] > 7 and drv["permissions"] == {"read": True, "upload": True, "write": True}
      and drv["calendar_connected"] is True and "refresh_token" not in json.dumps(drv), drv)
t = D.tools()
check("the agent's catalog has the Drive tools (and the calendar's)",
      {"drive_search", "drive_get", "drive_download", "drive_upload", "drive_create_folder", "drive_move", "drive_trash", "drive_storage",
       "calendar_list_events", "calendar_create_event"} <= t, sorted(x for x in t if "drive" in x))
st = c.get(B + "/internal/catalog", headers=D.RT).json()["connections"]["drive"]
check("catalog status: drive ready with the account", st["ready"] and st["account"] == "lucas@example.com", st)
tst = c.post(S + "/connections/drive/test", json={}, headers=H).json()
check("the Test button reads the account and quota", tst.get("ok") is True and tst["email"] == "lucas@example.com" and tst["used_gb"] > 7, tst)

# ---------------------------------------------------------------- reads through Sentinel
r = D.act("drive_storage", {})
check("drive_storage: quota numbers", r["status"] == "ok" and r["result"]["limit_gb"] == 16.0 and r["result"]["trash_gb"] > 0.2, r)
r = D.act("drive_search", {"kind": "video", "order_by": "size"})
res = r.get("result") or {}
files = (res.get("content") or {}).get("files") or []
names = [f["name"] for f in files]
check("drive_search kind=video: the user's own, not-trashed videos, largest first, marked untrusted",
      r["status"] == "ok" and res.get("trust") == "untrusted" and names[:2] == ["wedding-raw.mov", "old-render.mov"]
      and set(names[2:]) == {"kyoto-temple.mp4", "kyoto-arrival.mp4"} and all(f["kind"] == "video" for f in files), (r.get("status"), names))
r = D.act("drive_search", {"query": "kyoto", "folder_id": "fld-trip"})
files = ((r.get("result") or {}).get("content") or {}).get("files") or []
check("drive_search by name inside a folder: duration and size per clip", {f["id"] for f in files} == {"vid-1", "vid-2"}
      and all(f["duration_s"] == 2.0 and f["size_mb"] > 0 for f in files), files)
r = D.act("drive_search", {"min_size_mb": 100})
files = ((r.get("result") or {}).get("content") or {}).get("files") or []
check("drive_search min_size_mb: only the big own files (the 40 MB shared sheet and the trashed one are out)",
      [f["id"] for f in files] == ["big-3", "big-1", "big-2"], [f["id"] for f in files])
r = D.act("drive_search", {"unused_days": 365, "min_size_mb": 20})
files = ((r.get("result") or {}).get("content") or {}).get("files") or []
check("drive_search unused_days: never-opened and long-unopened big files, recent ones excluded",
      [f["id"] for f in files] == ["big-1", "big-2"] and files[0]["last_viewed"] == "" and files[1]["last_viewed"].startswith("2023-02-01"), files)
r = D.act("drive_get", {"file_id": "doc-1"})
check("drive_get: a Google Doc", r["status"] == "ok" and r["result"]["content"]["kind"] == "doc" and r["result"]["content"]["name"] == "Tax 2019", r)
r = D.act("drive_get", {"file_id": "nope"})
check("drive_get of a missing file is a plain error", r["status"] != "ok" and "not found" in json.dumps(r), r)

# ---------------------------------------------------------------- permissions and policy
c.put(S + "/connections/drive", json={"permissions": {"write": False}}, headers=H)
t = D.tools()
check("write permission off: drive_trash and drive_move leave the catalog, reads and uploads stay",
      not t & {"drive_trash", "drive_move"} and {"drive_search", "drive_upload"} <= t, sorted(x for x in t if "drive" in x))
r = D.act("drive_trash", {"file_ids": ["big-1"]})
check("...and a trash call is denied by policy", r["status"] == "denied" and "permission" in json.dumps(r).lower(), r)
c.put(S + "/connections/drive", json={"permissions": {"write": True}}, headers=H)

# ---------------------------------------------------------------- disconnect Drive only
c.delete(S + "/connections/drive/credential", headers=H)
drv, cal = D.conn("drive"), D.conn("calendar")
check("disconnecting Drive removes its token and tools; the calendar stays connected",
      not drv["has_credential"] and not drv["enabled"] and cal["has_credential"] and cal["enabled"]
      and not D.tools() & {"drive_search"} and "calendar_list_events" in D.tools(), (drv, cal["enabled"]))
r = D.act("drive_search", {})
check("drive_search is now denied (connection disabled)", r["status"] == "denied" and ("not connected" in json.dumps(r) or "disabled" in json.dumps(r)), r)

# ---------------------------------------------------------------- the user unticked Drive on Google's page
c.post(F + "/g/_deny_drive")
r = c.post(S + "/connections/drive/google/start", json={"origin": B}, headers=H).json()
page = c.get(r["auth_url"], follow_redirects=True)
check("a sign-in where Drive was not granted connects the calendar only and says so",
      "Google Calendar" in page.text and "Google Drive" not in page.text and not D.conn("drive")["has_credential"] and D.conn("calendar")["has_credential"],
      page.text[:300])

# ---------------------------------------------------------------- Drive alone, and the user's own client through the relay
r = c.post(S + "/connections/drive/google/start", json={"origin": B, "services": ["drive"]}, headers=H).json()
q = parse_qs(urlparse(r["auth_url"]).query)
check("services=[drive]: the consent asks for Drive without the calendar scopes", "auth/drive" in q["scope"][0] and "calendar" not in q["scope"][0], q["scope"])
page = c.get(r["auth_url"], follow_redirects=True)
check("...connects Drive; the result page names Drive only", "Google Drive" in page.text and "Google Calendar" not in page.text and D.conn("drive")["has_credential"], page.text[:300])
D.disconnect_all()
D.managed_client(False)
r = c.post(S + "/connections/calendar/google/client", json={"client_id": "1234567890-abc.apps.googleusercontent.com", "client_secret": "GOCSPX-test-secret-123"}, headers=H)
check("the user's own Google client saved on this box (shared by the calendar and Drive cards)", r.status_code == 200 and r.json()["ok"], r.text[:200])
r = c.post(S + "/connections/drive/google/start", json={"origin": B}, headers=H).json()
q = parse_qs(urlparse(r["auth_url"]).query)
page = c.get(r["auth_url"], follow_redirects=True)
check("own client through the relay: both connected (the box client signs in like the managed one)",
      q["client_id"] == ["1234567890-abc.apps.googleusercontent.com"] and "Google Calendar + Google Drive" in page.text
      and D.conn("drive")["has_credential"] and D.conn("calendar")["has_credential"] and D.conn("drive")["managed_source"] == "box",
      (q.get("client_id"), page.text[:300], D.conn("drive").get("mode")))
r = D.act("drive_storage", {})
check("...and Drive calls work with it", r["status"] == "ok", r)
# the own client with this box's callback (no relay)
D.disconnect_all()
r = c.post(S + "/connections/drive/start", json={"origin": B, "client_id": "1234567890-abc.apps.googleusercontent.com", "client_secret": "GOCSPX-test-secret-123"}, headers=H).json()
check("drive/start (box callback): redirect is this box, scopes for both", r["redirect_uri"] == B + "/sentinel/api/connections/calendar/callback"
      and "auth/drive" in parse_qs(urlparse(r["auth_url"]).query)["scope"][0], r)
page = c.get(r["auth_url"], follow_redirects=True)
check("...connects both, mode google_own", "Google Calendar + Google Drive" in page.text and D.conn("drive")["has_credential"]
      and D.conn("drive")["mode"] == "google_own" and D.conn("calendar")["mode"] == "google_own", (page.text[:200], D.conn("drive").get("mode")))
# audit never carries a token
ev = c.get(S + "/audit?limit=200", headers=H).json()
ev = ev.get("events") or ev.get("entries") or ev
check("the audit log never carries a token", "grt-1" not in json.dumps(ev) and "gat-1" not in json.dumps(ev))
D.disconnect_all()
finish()
