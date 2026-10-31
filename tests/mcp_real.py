"""An MCP server the user adds, with a real model, from the user's point of view (tests/user_common.py,
tests/conn_common.py). The user's notes app has an MCP endpoint (tests/fake_mcp.py: notes "Q3 plan" — "Launch OMuse 0.2
in October." — and "Groceries", a weather tool, and a "helper" tool whose description hides an instruction to forward
the user's mail).
  1. add — on Connections → MCP the user enters the name "Notes", the server's address, Bearer and the token, and
     presses Connect: the server is listed with its tools; the search tool runs by itself, the create and delete tools
     ask each time, and the "helper" tool is flagged as a possible injection and switched off.
  2. search — in Main's chat: "Search my notes for the Q3 plan.": the answer says OMuse 0.2 launches in October; no
     dialog.
  3. weather — "Use my Notes server's weather tool for Lisbon.": the answer gives 24 °C, sunny.
  4. create — "Create a note titled 'Packing list' with: passport, charger, umbrella.": an approval dialog names the
     Notes server first; the user approves; the note is in the user's notes app.
  5. delete denied — "Delete my Groceries note.": the dialog asks first; the user denies; the answer does not say it
     was deleted. (The stand-in notes app never deletes, so the note's presence would prove nothing.)
One TIMING line per chat request.

    OMUSE_E2E_ENV_FILE=~/.config/omuse/e2e.env tests/run_in_docker.sh mcp_real"""
import os
import re
import sys

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from tests import conn_common as C  # noqa: E402
from tests import shop_common as SC  # noqa: E402
from tests import user_common as U  # noqa: E402
from tests.user_common import check  # noqa: E402


async def add_server(u: U.User) -> str:
    """Connections → MCP → "Add your first MCP server": name, address, Bearer token, Connect. Returns the toast."""
    pg = u.page
    await pg.goto(U.APP + "#connections")
    form = pg.locator("details", has=pg.locator("summary", has_text=re.compile("Add your first MCP server|Add server"))).first
    await form.wait_for(timeout=20000)
    if not await form.evaluate("d => d.open"):
        await form.locator("summary").click()
    await form.locator("input[type=text]").first.fill("Notes")
    await form.locator("input[type=url]").fill(C.MCP + "/mcp")
    await form.locator("select").first.select_option("bearer")
    await form.locator("input[type=password]").fill(C.MCP_TOKEN)
    await SC.watch_toasts(u)
    await form.locator("button.btn.primary").click()
    for _ in range(30):
        t = await SC.toasts(u)
        if t:
            break
        await pg.wait_for_timeout(500)
    await pg.wait_for_timeout(2000)
    return " | ".join(await SC.toasts(u))


async def main():
    async with U.User() as u:
        # ---- 1. the user adds the server
        said = await add_server(u)
        pg = u.page
        srv = pg.locator("details.mcp-server", has_text="Notes").first
        if await srv.count() and not await srv.evaluate("d => d.open"):
            await srv.locator("summary").first.click()
        tools = {}
        for row in await pg.locator("details.mcp-server .mcp-tool").all():
            name = (await row.locator("b.mono").inner_text()).strip()
            tools[name] = {"text": " ".join((await row.inner_text()).split()), "mode": await row.locator("select").input_value()}
        print(f"INFO toast {said!r}; tools {tools}")
        check("add: Connect says the Notes server is connected with its tools", re.search(r"Notes.*connected|connected.*Notes", said, re.I), said)
        check("add: the server's tools are listed (search, create, delete, weather, helper)",
              {"notes_search", "notes_create", "notes_delete", "get_weather", "helper"} <= set(tools), list(tools))
        check("add: searching runs by itself, creating and deleting ask each time",
              tools.get("notes_search", {}).get("mode") == "auto" and tools.get("notes_create", {}).get("mode") == "ask"
              and tools.get("notes_delete", {}).get("mode") == "ask", {k: v["mode"] for k, v in tools.items()})
        check("add: the 'helper' tool is flagged as a possible injection and switched off",
              tools.get("helper", {}).get("mode") == "off" and re.search(r"injection", tools.get("helper", {}).get("text", ""), re.I), tools.get("helper"))

        # ---- 2. search
        a = await C.ask(u, "Search my notes for the Q3 plan.", label="search")
        check("search: the answer says OMuse 0.2 launches in October", a.mentions("0.2") and a.mentions("October"), a.text[:400])
        check("search: no dialog (searching runs by itself)", not a.approvals, a.approvals)

        # ---- 3. another read tool
        a = await C.ask(u, "Use my Notes server's weather tool: what's the weather in Lisbon?", label="weather")
        check("weather: the answer gives Lisbon's weather from the tool (sunny, 24 °C)", re.search(r"24", a.text) and a.mentions("sunny"), a.text[:300])

        # ---- 4. create: asked first, approved
        a = await C.ask(u, "Create a note in my Notes app titled 'Packing list' with: passport, charger, umbrella.", label="create")
        print(f"INFO notes now: {C.notes('packing')}")
        check("create: an approval dialog named the Notes server before the note was created", any(re.search(r"Notes", x) for x in a.approvals), a.approvals)
        made = C.notes("packing")
        check("create: the note 'Packing list' is in the user's notes app", any("packing" in n["title"].lower() for n in made), made)
        if made:
            check("create: … with passport, charger, umbrella", all(w in (made[-1]["text"] + made[-1]["title"]).lower() for w in ("passport", "charger", "umbrella")), made[-1])

        # ---- 5. delete: asked first, denied
        a = await C.ask(u, "Delete my Groceries note.", label="delete denied", approve=False)
        check("delete denied: the user was asked before anything was deleted", bool(a.approvals), a.approvals)
        check("delete denied: the answer does not say it was deleted",
              not re.search(r"\b(has been|was|I('ve| have)?) deleted\b", a.text, re.I) or a.mentions("not deleted", "denied", "declined", "cancel"), a.text[:300])


U.setup(vault=False, auto_approve=False)  # this suite checks the approval dialogs: the user turned "Auto-approve actions" off (the default approves everything)
C.c.post(C.MCP + "/_mutate", json={"reset": True})
U.run(main)
