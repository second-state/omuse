# OMuse knowledge base

How OMuse works inside, and the knowledge files it ships. Building, running and the travel use cases are in
[README.md](README.md); repository rules, the test commands and the release steps are in [AGENTS.md](AGENTS.md);
`docs/rust-port.md` records how the earlier Python version (v0.3.6, commit 562a36f) maps to the Rust crates.

## Architecture

```
 browser / Telegram ──► Sentinel :8080  (UI, login gate, policy, vault, approvals, audit, connectors)
                            │
                            ├──► Runtime :8081  (planner / executor, sub-agents, scheduler, goals, memory, skills)
                            │        └──► LLM (OpenAI-compatible; default on Olares: the Olares router)
                            └──► Browser broker :8082  (Chromium over CDP, Xvfb or headless)
```

One binary (`omuse`, Axum + Tokio) runs the three services in one process; `omuse --role
sentinel|runtime|browser|all` runs one of them (the Olares chart runs three containers). They talk over loopback HTTP
with shared tokens (`X-Persona-Runtime` runtime ⇄ Sentinel, `X-Browser-Token` Sentinel → broker, `X-Persona-UI: 1`
from the web UI); `[internal]` tokens are generated per start when blank. Sentinel also serves the optional voice app
on 8083.

### Crates (`crates/`)

| Crate | Role |
|---|---|
| `omuse` | the binary: config, roles, process lifecycle |
| `omuse-common` | config (`config.toml` → `Config`), SQLite helper, URL / injection guard, Python-compatible JSON, utilities |
| `omuse-docs` | Markdown → HTML, xlsx / docx writers, data queries, charts |
| `omuse-web` | the embedded web UI (`web/`) and the login gate |
| `omuse-sentinel-core` | store, vault, password, catalog, **policy**, ledger (no HTTP) |
| `omuse-mail`, `omuse-apps`, `omuse-mcp`, `omuse-phone` | connectors: mailboxes; Notion / Slack / calendars / market data / billing; MCP client and OAuth; phone |
| `omuse-sentinel` | Sentinel routes, actions, Telegram bot, watchers |
| `omuse-runtime` | agent loop, prompts, LLM client, scheduler, goals, memory, library, skills, image and video generation, attachments |
| `omuse-browser` | the browser broker and its page scripts (`js/`) |

The web UI, the skills (`skills/`) and the broker scripts are compiled into the binary (`include_dir`); built-in skills
are extracted to `<data>/runtime/skills-builtin/` at start unless `[skills] dir` points elsewhere.

### Configuration

Every setting comes from one `config.toml` (`--config`, or `OMUSE_CONFIG`); `config.example.toml` documents each key
and the environment variable that sets it in a container without a config file. The binary itself never reads
settings from the environment: the container entrypoint writes a config file from `OMUSE_*` variables when none is
mounted, and a mounted file always wins. `OMUSE_VERSION` is a build-time value (the release tag).

### Data layout (`[data] dir`, default `/omuse`)

```
<dir>/sentinel/        sentinel.db  audit.jsonl  vault.key  auth.json  session.key  shots/
<dir>/runtime/         runtime.db  skills-builtin/
<dir>/workspace/       the agent's files (uploads/, reports/, charts/, media/, library/, …)
<dir>/browser-profile/ Chromium user data
```

## How a request runs

1. **Look first**: the executor model sees every request first, with no planner call before it. Conversation, advice,
   writing and lookups of a few tool calls are answered directly. When the request needs several dependent steps,
   several sources or candidates, sub-agents or approvals in sequence, the executor calls `plan_task` (its notes go
   to the planner; `Runtime::plan_on_request`, once per task; the task shows `PLANNING` meanwhile). A skill is a
   procedure, not a reason to plan: `load_skill` returns the sheet and the executor follows it step by step with no
   planner call. Only a skill whose front matter says `plan: on` (the long, branching or parallel ones, see
   [Skill front matter](#skill-front-matter)) is planned on loading, with the sheet as the planner's context, and
   the plan is appended to the tool result. The planner model turns the request into steps; steps that don't depend
   on each other carry the same `"parallel": "p1"` group. After repeated failures the loop re-plans on its own. A
   follow-up that acts on a result already in the conversation ("book it", "buy the second one", "send it") is never
   planned or searched again: the prompt and the booking skills send the executor straight to the action (the airline's
   booking page with the traveller, passport and card filled, then the approval or the hand-over;
   `tests/kiwi_flights_e2e.py`, and `travel_common.verify` for a real model).
2. **Execute**: the executor model calls tools step by step. Read-only tools in one turn run concurrently; a parallel
   group runs as sub-agents (`delegate`, up to 8), each with its own browser tab ("lane", broker page
   `<task>~sub<n>`), and the executor continues when all have reported. `llm_concurrency` (Settings, default 4) caps
   simultaneous model calls.
   Every chat request is OpenAI `/chat/completions`. `service_tier: "fast"` (fast mode) and `reasoning_effort` are
   added from the settings in force: `[model] fast` / `[model] reasoning_effort` for the system's models, the user's
   own choice (Settings → Models) only while "use my own model" is on — the API ignores changes otherwise
   (`store::MODEL_OPTIONS`). An endpoint that rejects either field by name gets the request again without it;
   `reasoning_effort = "none"` also sends `chat_template_kwargs: {enable_thinking: false}` for chat-template models.
   Both are chat options only: image generation, edits and variations (`imagegen.rs`, the Images API), video
   generation (`videogen.rs`) and speech to text build their own requests and never carry them (`imagegen_e2e.py`,
   `videogen_e2e.py` and the unit tests check it).
3. **Skills**: the executor loads a skill (`load_skill(name)`) when the request matches its description; a skill can
   ship reference files read with `load_skill(name, file=…)` (relative path, 400 KB cap).
4. **Act**: every action goes through Sentinel (`/internal/act`), which decides ALLOW / DENY / ASK, runs it, audits
   it and returns a redacted result. The task waits in `WAITING_APPROVAL` for an approval card, or in
   `WAITING_EXTERNAL` for a takeover (the user drives the live browser, then hands it back).
5. **Answer**: an outcome check makes sure the answer doesn't claim actions that didn't happen (a how-to question,
   "how do I book", asks for advice, not the act, and "can be booked" is advice, not a claim: `outcome::intents`,
   `outcome::claims`); memory extraction proposes facts and profile fields for the user to confirm.

## Flights & hotels search

`flights_search` is always on and makes **ONE search per call** (`actions::travel_sync`). With a SerpApi key and one
departure date it asks Google Flights (below; one credit): measured on 2026-10-29 against Kiwi.com over nine routes, Google
had more airlines, the nonstops Kiwi missed (JFK–LAX), much cheaper premium cabins (SIN–LHR business $3,639 vs $6,754)
and the airline-issued ticket with a partner leg (United UA 888 + Air China CA 4110, `ticket_also_sold_by` →
`ticket: "one ticket, sold by United"`). Kiwi.com answers a range of dates (one query, `departureDateTo`), self-transfer
(`self_transfer=true`, only when the user asks; otherwise `allow_self_transfer=false`, connections on one booking), the
no-key case, and Google failing outright (the result's `fallback` line says why). An empty result is the answer (a `note`
says to ask the user what to change); nothing else is searched — no web search for flights. Sentinel asks Kiwi.com's
public MCP server itself (`omuse_apps::kiwi`, `https://mcp.kiwi.com/`, one JSON-RPC request per search — its server is stateless, so no `initialize` — no key, no connector
registration) and compacts the itineraries — airlines, flight numbers, local times, stops, price, baggage.

Every flight and hotel result carries a `booking_url`, at no extra cost: a Kiwi.com offer its Kiwi booking page
(`kiwi.com/u/…`, kept only for `kiwi.com` hosts; `book_at` is the airline's own site), a Google Flights offer Google
Flights' own page for that itinerary (`serpapi::itinerary_url`: the search's `google_flights_url` with the itinerary's
flights added to its `tfs` protobuf — field 3 is a slice of the trip, a slice's field 4 a chosen flight with from 1,
date 2, to 3, airline 5, number 6; one way opens `/travel/flights/booking` on "Book with United", a round trip opens with
the outbound chosen for picking the return; checked in a browser on 2026-10-29), a hotel Google's page for it and the
stay (`serpapi::hotel_page_url`: `/travel/hotels/entity/<property_token>?ts=…`, the dates in the `ts` protobuf — every
seller and price, the hotel's own site among them; its own site is `hotel_site`, which may be down, as Hotel Keihan's
was in a run). Each flight and hotel result also has a `compare_url`, Kayak's page for the same search (`omuse_apps::kayak`, built from the
arguments, no call: `/flights/HND,NRT-SIN/2026-12-10/2026-12-20/business/2adults?sort=price_a&fs=stops=0;airlines=SQ,NH` — a
range links its cheapest day — and `/hotels/Ginza%2CTokyo/…/2adults`). `flight_offer` (the booking options with prices per seller) costs another credit for Google data and is only
for when the user wants those details.

ONE travel search **per request and per turn** — flights, hotels, cars and places together (web and news search are not
counted), in code: `guards::one_search_guard` refuses a travel search when the task already ran one (a failed call, e.g.
on a bad date, may be corrected), and within a model turn the runtime skips every travel search after the first
(`Skip.one_search`), both naming the search that ran. A request for a flight, a hotel and a car searches the first and
offers the next ("Shall I search the hotel now?"); the user's next message runs it (`tests/kiwi_flights_e2e.py`
TWOSEARCH, `tests/brave_travel_e2e.py`, `agent/tests.rs`).
Google first, also beside a connector: with a SerpApi key (`/internal/catalog` → `connections.travel.google`) the
built-in travel searches stay. Without a key they fall back (flights to Kiwi.com, hotels to a web search), so a travel
search the user's own connector also offers (`mcp_<id>__hotels_search` beside `hotels_search`) is then the
connector's alone: `Runtime::tools_for` leaves the built-in one out of that run, so the one search is not spent on the
weaker fallback.

**The preferred airline is always covered.** When a run searched flights and the user has a preferred airline in the
profile, an answer that names neither it (an alliance: nor a member; any name it goes by in `loyalty::AIRLINE_CODES`)
nor one of its flight numbers gets a line from the results — the cheapest preferred offer with its price, or that
none was found and an offer to look (now the booking machine's `must_say`, "… is on your preferred airline"; the
Travel bot's flight searches are the machine's, `guards::MACHINE_ONLY`).

No mail before a travel search: the booking machine never reads mail for a search (its own mail is the results the
user asked to be mailed). The old request-words guard (`mail_before_travel_guard`) is gone with phase 2d.

Links (`crate::links`, 2026-10-31): the model sees every link as it is — with the opaque base64 stand-ins used before
it could not tell where a link went (it guessed a shop's address and dropped its port) and a stand-in mistyped by one
letter could not be recovered. Where the model's words leave the agent — a tool call's arguments as they are parsed,
and the final answer — each link is repaired against the links the run's tool results showed (`links::seen`,
`links::repair`): a link cut short (the same host and path, query parameters missing — a hotel page's `&ts=…`) or with
its path one or two characters off (a letter dropped) becomes the shown link; another page or other query values
(dates the model chose) stay as written. The rest of OMuse only ever sees whole, real links. Stand-ins left in older
conversations are still decoded (`links::decode`). The booking and Kayak compare links are the booking machine's `must_say` (the reply carries them through the review). A travel MCP
connector's flight or hotel results get `compare_url` too, inside their JSON. When a bot answers for Main, the files it
handed over (PDF, picture, video) are shown in Main's chat above its answer (`bots::finish_handoff`), and the router
gives the bot the user's local date and time (Settings → Timezone), never UTC's.

Sentinel's `travel` connector: a SerpApi key (`cred_travel_1`, `POST /sentinel/api/connections/travel/credential`,
checked against `/account.json`, which costs no search; `/test` reads it again) behind six read-only tools —
`flights_search`, `flight_offer`, `hotels_search`, `hotel_rooms`, `places_search`, `place_details` (`catalog.rs`,
connector `travel`, permission `search`). The places tools are Google Maps (`engine=google_maps`, `type=search` with
the query "<what> near <area>, <city>", `min_rating`, `open_state=now`, price levels; `place_id` for the details) —
rating, reviews, price, address, hours, phone, website, menu or reservation links when Google lists them.
`omuse_apps::serpapi` builds the Google Flights / Google Hotels requests and compacts the replies. Places come from
one table, `omuse_apps::airports::CITIES` (metro code, city name, airports, other names): `flights_search` takes an
airport code (`NRT`), a city code (`TYO`, `OSA`, `SEL`, `LON`, `NYC` …), a city name, or a comma list of them, and one
search covers the whole city — Google gets every airport (`TYO` → `departure_id=HND,NRT`, `airport_codes`), Kiwi gets
the city's name (`kiwi_place`: `TYO` → `Tokyo`). The flight skills carry the same list of multi-airport cities (a unit
test in `airports.rs` keeps `skills/flight-booking` and `skills/travel-planning` in step with the table). The request's
`cabin` (economy / premium_economy / business / first), `stops` (any / nonstop / one_stop_max / two_stops_max — an enum so a model that fills every field with a default lands on "any"; direct callers may still send `max_stops` 0–2 or `nonstop_only`; a price cap of 0 is ignored), adults, return date,
price cap, sort and currency go through as given (Google `travel_class` / `stops`, Kiwi `cabinClass` /
`max_sector_stopovers`), and every flights result carries a `searched` block (origin and destination as used, date,
trip, adults, cabin, stops, and a rule line) so the executor answers in the cabin and stop limit the user asked for and
searches again rather than presenting economy fares for a business-class request. A premium-cabin itinerary with a short
economy leg is kept and labelled (`tag_cabins`: `cabin_mix: "business + economy"`, `economy_legs`); it counts as the
cabin searched. A range of departure dates is one call (`date` + `date_to`, up to 14 days) and one Kiwi.com query (`departureDateTo`;
a round trip keeps its stay as `nights_in_dst_from` / `_to`); the result is split per day (`merge_days`): each offer's
`date` and the cheapest per day in `by_date` — the prompt and the skills say never to plan or call per day (`tests/kiwi_flights_e2e.py`,
`tests/travel_search_e2e.py`; the skills tell the executor to read date, cabin, stops and passengers from the request
first, and that a cheapest-fare question is one search with no plan). Preferences: `flights_search` takes `airlines`
(names, 2-letter codes or alliances) and `hotels_search` takes `brands` (brands or groups); both **narrow the search
itself**, in code, and matching offers or hotels come back with `preferred: true` (`mark_preferred`),
`searched.preferred_found` counting them:
- Flights (`omuse_apps::loyalty::airline_codes`): each preference becomes IATA codes — a code as given, an airline by
  name from `AIRLINE_CODES` (every airline in the Profile dropdown; a unit test checks), an alliance as all its members
  (`ALLIANCES`' fourth column: Star Alliance 25, oneworld 15, SkyTeam 17). Kiwi.com gets them as `select_airlines` (it
  knows no alliances — added 2026-10-29 after a Star Alliance user got no Star flights); Google Flights gets
  `include_airlines` with its own alliance values (`STAR_ALLIANCE`) and the codes. A preference with no known code
  leaves every airline in. `searched.airline_filter` says which happened. A narrowed Kiwi search that finds nothing is
  the answer (no preferred flight → ask before searching all airlines): Google Flights is tried with the same filter
  when there is a key, never the web search.
- Hotels: ONE Google Hotels search. Known groups narrow it with Google's own `brands` filter, where a group's id takes
  all its brands (46 Marriott Bonvoy → Sheraton, Westin, Aloft, AC, Moxy, EDITION …; `GOOGLE_HOTEL_GROUP_IDS`: Marriott
  46, Hilton 28, Hyatt 37, IHG 17, Accor 33, Wyndham 53, Shangri-La 315); any other brand goes into the query words
  ("Westin hotels near Ginza Tokyo") — no first lookup search for Google's brand list (removed 2026-10-29: one search
  per call).
  `searched.brand_filter` says which happened.
An alliance or a hotel group also stands for all its members when flagging (`HOTEL_GROUPS`, `expand`). The
preferences come from the request, else from the profile: two standard fields, `preferred_airline` (an airline or an
alliance) and `preferred_hotel_brand` (a brand or a group), dropdowns on Memory → Profile in two groups each
(`ALLIANCES` / `AIRLINES`, `HOTEL_GROUPS` / `HOTEL_BRANDS` in `web/app.js`; a unit test keeps the alliance and group
names equal to the Rust table; the card says anything not on the list goes in a custom field), plus any custom field
the user ticked "Always use" on (a `standing` flag per profile row, `Store::profile_standing`; off by default, also for
fields saved before 0.6.0 — no label is read for travel words). `Runtime::standing_prefs` collects them and the executor prompt always carries them as a "Standing
preferences" block (`prompts::prefs_text`) with the rule — pass them to the search tools, show the preferred option
next to the best value with the price difference, say so when none matched, a wish in the request wins — so they
count whether or not a skill was loaded and whichever flight tool (built-in or an MCP connector's) the executor picks;
no other profile field is ever in the prompt. Memory extraction can propose the two fields from chat ("I always fly
ANA"). The compaction gives
itineraries with airlines, legs, local times, duration, stops, price and an `offer_id`; hotels with stars, rating, price
per night and total, nearby places and a `hotel_id`. The long booking and property tokens stay in a per-process table
behind those short ids (`short_id` / `token_of`; a flight token is kept together with the route and dates of its search, which SerpApi wants
repeated on the booking-options request), so a second call (`flight_offer` → the booking options and the seller;
`hotel_rooms` → rates per seller and room types) needs no token in the chat. Without the key the policy denies with the
way to connect: the card (Settings → Connections → Flights & hotels) or Kiwi.com's free MCP server
(`https://mcp.kiwi.com/`, streamable HTTP, no auth, one `search-flight` tool — the MCP card has a preset button that fills
the add form). The travel skills name the built-in tools first, then an MCP connector's, then the browser with a one-line
note on how to connect. `[endpoints] serpapi` points the client at `tests/fake_apps.py`'s stand-in (`/serpapi/*`);
`tests/travel_search_e2e.py`, `tests/travel_card_ui.py`.

## Search input validation (`omuse_apps::travel_args`, `omuse_common::util::sanitize_search_args`)

The model's arguments are never trusted for numbers: a model filling in defaults sends `max_price: 0` ("nothing is
affordable") or `adults: 0`. Validation is code, deterministic, and runs before any data source is asked:

- **Sentinel, every action** (`/internal/act`): `sanitize_search_args` on the arguments of every tool, built-in and
  connector alike — a price cap (`max_price`, `max_price_per_night`, `price_to`, `budget` …) that is not a positive
  number is removed (no limit), a count of people or rooms (`adults`, `guests`, `party_size`, `passengers` …) below 1
  becomes 1, children / infants below 0 become 0, a rating outside 0–5, a star class outside 1–5 and a price level
  outside 1–4 are removed. The runtime applies the same function before a call is logged, so the timeline shows what
  ran.
- **The travel and web search tools** (`travel_args::validate`): per tool — `flights_search`, `flight_offer`,
  `hotels_search`, `hotel_rooms`, `places_search`, `place_details`, `web_search`, `news_search`. Values that have an
  obvious meaning are replaced (adults 1–9, guests 1–20, an unknown cabin → economy, unknown stops → any, an unknown
  sort → the source's order, a bad currency → the default, `max_stops` outside 0–2 → removed, yes/no flags made real
  booleans, a minimum price level above the maximum → swapped, `2026/11/5` → `2026-11-05`, `max_results` outside 1–20
  → 8) and reported to the model as `input_fixed`; what cannot be guessed is refused with how to fix it (no or an
  impossible date, a date in the past, a return before the departure, a range ending before it starts, check-out on
  or before check-in, more than 30 nights, origin = destination, a missing city or id).
- The flight and hotel clients keep their own defaults on top (`serpapi::people`, `price_cap`), and each result's
  `searched` block states the price limit (`no limit (every price)`) and the party size used.
- `tests/args_guard.py` runs after **every** e2e suite (`tests/run_in_docker.sh`): no recorded tool call and no request
  to the fake Kiwi.com, SerpApi or travel connector may carry a price cap ≤ 0 or fewer than 1 traveller; a violation
  fails the suite.

## Search services: who answers what

- `web_search` (every model call): the browser's own search engines first (free; as fast as the APIs — about 1.6 s a
  search in `tests/bench_search_timing.py`, 15 in a row without a challenge page on 2026-10-28), then the search APIs when
  the browser fails, meets a challenge page or finds nothing: Brave → Exa's free MCP → Exa (the keys on hand, user over
  server; among them the one that worked last goes first). Tavily is not in this chain. The SerpApi key is never used here.
- `news_search` (every model call): Tavily's news topic (`topic: news`, `days`; dated articles with snippets) when a
  Tavily key is set and the search connection is on, else `web_search` with "<query> news <period>"; `news_fallback`
  says why. The `news-search` skill tells the model to answer with dates and sources.
- `flights_search` / `flight_offer`: ONE search per call. Google Flights through the SerpApi key for one date; Kiwi.com's
  public MCP for a range of dates, self-transfer, without the key or when Google fails. Every offer has its `booking_url`.
- `hotels_search` / `hotel_rooms`: ONE search per call. Google Hotels data with the SerpApi key (an empty result is the
  answer); without it, or when Google fails, `hotels_search` falls back to the web search chain (the request spelled
  out, `hotels_fallback` says why).
  `hotel_rooms` reads Google ids, so it needs the key (the policy says so).
- `cars_search`: car rental at an airport (`omuse_apps::cars`). No rental site has a public search API (Kayak's is for
  approved affiliates, Hertz has none) and Google has no car-rental data, so: ONE web search (the web search chain) for
  the companies and "from" prices (the words ask for "car rental companies": plain "car rental" brought aggregator
  snippets that named none, 2026-10-30; `companies_found` lists the companies the results name, read in code by
  `cars::companies_in`, so the answer names them), and a `booking_url` built from the rental — Kayak's results page
  (`/cars/LAX/SFO/2026-12-03-9h/2026-12-06-14h?fs=carclass=suv`, live offers for those dates; classes small, medium,
  large, suv, van, luxury, convertible), else Rentalcars.com's (`/search-results?locationIata=…&driversAge=…`) only when
  Kayak's page does not answer (`page_answers`: a 2xx within 8 s whose title is no bot check — Rentalcars' normal page
  mentions "captcha" in its scripts, so the body is not searched); neither answering → Kayak's link with a note. Compared
  on 2026-10-29 over five airports: Brave, Tavily and SerpApi's Google results had no offer for the dates and no booking
  link (SerpApi was the slowest and noisiest); Kayak's, Rentalcars' and Priceline's prefilled pages showed the live prices,
  Expedia and Skyscanner a bot check. Endpoints `[endpoints] kayak` / `rentalcars` (tests: the fakes'
  `/kayak`, `/rentalcars`, `tests/cars_search_e2e.py`). One car search per request and per turn, like flights and hotels.
- `places_search` / `place_details`: restaurants and other places, Google Maps data with the key, else the web search
  chain the same way (`places_fallback`); `place_details` needs the key. Booking: the restaurant's own site or the
  platform the user names.
- Keys: the user's (Settings → Connections) win per provider over the server's defaults (`[search]` in config.toml);
  the cards mark each key as "your key" or "server key"; removing an own key falls back to the server's.

## Web search providers and default keys

The one `web_search` tool (catalog connector `search`, always listed and always allowed by the policy, open to the
executor, sub-agents and skills that run with the browser off; the old `browser_search` is gone) goes through the
search services when the `search` connection is on and ends in the browser's own search engines (`/agent/search` on
the browser service) when none answers or none is configured. Keys are per provider: the user's
(`cred_search_1` = `{"keys": {id: token}}`, `POST /sentinel/api/connections/search/credential` with `provider`, checked
with one small search; `DELETE …/credential/{provider}` removes one) win over the server's defaults from `[search]`
(`tavily_key`, `brave_key`, `exa_key`, `serpapi_key`; `SearchConfig::default_keys`). `actions::search_clients` builds
the clients in try order — `websearch::order`: the provider that worked last (`last_ok` in the connection config) first,
then Exa's public MCP server (`https://mcp.exa.ai/mcp`, keyless, rate-limited, `[search] free_exa`, spoken in
`websearch::search_exa_mcp` as initialize + `tools/call web_search_exa`, its text reply parsed by
`parse_exa_mcp_text`), Tavily, Brave, Exa, Google (SerpApi) — and `actions::browser` tries them in turn: the first that answers wins and is
remembered (`remember_search_ok`), a failure moves on (the result's `note` says so), and when none answers the browser's
engines are used. `omuse_apps::websearch` speaks
Brave (`GET /web/search`, `X-Subscription-Token`; the free plan allows one request a second, so a 429 is retried once
after the wait asked), Tavily (`POST /search`, bearer) and Exa (`POST /search`, `x-api-key`, highlights as snippets) and returns the browser's own shape (`{query, engine, results: [{title, url, snippet, age?}]}`), so
nothing downstream changes. The same `[search] serpapi_key` is the default for the Flights & hotels connection
(`travel_client`). `state::seed_default_keys` runs at startup: the first time a default appears the connection is
switched on and its config records it (`default_key` / `default_providers` + `default_seeded`), so the policy
(`travel` without a key) and the page (chip "server key") know; the user's own key always wins, deleting it falls back
to the default, and switching the connection off is respected. `[endpoints] brave / tavily / serpapi` point the clients
at `tests/fake_apps.py` (`/brave/web/search`, `/tavily/search`, `/serpapi/search.json?engine=google`).
`tests/web_search_e2e.py`, `tests/search_card_ui.py` (the test stack ships a default Brave key in `tests/run_local.sh`, so
the override and the fall-back to the server key are exercised). The search providers are never tools of their own: the
model sees `web_search` (and `web_search`) and OMuse picks the provider (`tests/web_search_e2e.py` asserts no
provider-named tool). Kiwi.com's MCP server is used the same way inside `flights_search` (`omuse_apps::kiwi`), never as a connector
of its own, so no Kiwi tool reaches the model (its booking links do, as each offer's `booking_url`); Exa's hosted MCP is the first link of the chain, called by Sentinel itself (two JSON-RPC calls, no connector
registration), rather than an MCP connector of its own, which would add a provider-named tool.

## Streamed answers

The executor's model calls are `stream: true` (`stream_options.include_usage` for the token counts) when Settings
`stream_answers` is on (the default; API only). `llm::SseAssembler` puts the `chat.completion.chunk` lines back into
the plain reply shape, so the parsing after the call is the same; `LlmTransport::send_chunked` hands every body piece
to it as it arrives (test doubles feed the whole body at once). The visible text so far — never the reasoning: a
`reasoning_content` part is dropped and a `<think>…</think>` block at the start of the content is held back whole until
it closes — goes through `ChatOpts::stream_to`; `Runtime::executor_opts` forwards it as `answer_delta` events
(`task_id`, `conv_id`, `text`, `calls`), published and not stored, at most ten a second, and the loop awaits the
forwarder before acting on the reply so no delta trails the stored message. The page keeps one live bubble under the
running task's card (`onDelta`, `liveBubble` in `web/app.js`), hides it for a turn that carries tool calls (commentary,
not an answer), and drops it when the task finishes, when the stored message takes over — an answer the runtime then
rewrites (cut off, wrong language, an outcome notice) simply replaces the draft. An endpoint that rejects
`stream_options` gets the request without it; one that rejects `stream` gets the plain request. Planner, sub-agent,
memory and title calls never stream. Whether the text really arrives as it is written depends on the provider: the
e2e gateway streams its `openai-codex/…` and `xai/…` models chunk by chunk but hands an `openai/…` reply over in one
piece at the end (checked with a raw request, compression off), which then shows as a single delta — correct, just
not live. `tests/stream_e2e.py`, `tests/stream_real.py`.

## Google Drive

One Google sign-in covers Calendar and Drive. The Drive card (Settings → Connections) starts the same OAuth flow as
the calendar (`connections/drive/google/start` with the managed client and the relay page, or `connections/drive/start`
with the user's own client and this box's callback) with `services: ["calendar", "drive"]`; `gcal::scopes_for` builds
the consent's scope string and the pending flow (`calendar_oauth` kv) records the services. The shared callback
(`/sentinel/api/connections/calendar/callback`, so no new redirect URI has to be registered) narrows the services to
the scopes Google actually granted (`gcal::granted_services`; the user can untick one), verifies each (a calendar
listing, a Drive `about`), stores the same refresh token as `cred_calendar_1` and/or `cred_drive_1`, and enables the
connections (a Drive row made before this version gets `read` / `upload` / `write` seeded once). Disconnecting one
leaves the other. The Drive scope (`auth/drive`) is restricted at Google: the user's own OAuth client in Testing
status may use it; the managed client would need Google's verification first.

`omuse-apps/src/gdrive.rs` is the Drive v3 client (`about`, `list` with the Drive query language, `get`, `download`
with Google Docs / Sheets / Slides exported as PDF / XLSX / PDF, multipart `upload`, `create_folder`, `update` =
move / rename, `trash` — never a permanent delete). It borrows `GCal`'s token refresh (`access_token()`); access
tokens are cached in `cred_drive_access`. Sentinel's `actions::drive_call` runs the tools: `drive_search` (`drive_query`
turns kind / folder / dates / `unused_days` into `q`; `min_size_mb` is filtered after fetching pages ordered by
`quotaBytesUsed`, since the query language has no size term; files never opened count as unused), `drive_get`,
`drive_download` (≤ 200 MB, bytes handed to the runtime, which writes `workspace/drive/<name>` like a mail
attachment), `drive_upload` (the runtime serves the workspace file; approval), `drive_create_folder`, `drive_move`
(approval), `drive_trash` (≤ 50 ids per call; approval card lists the names; policy reason says the bin keeps them
30 days), `drive_storage`. Reads taint the task like mail; an upload from a tainted task is a possible egress (high
risk). File names and metadata are untrusted (`untrusted()` scans them).

`edit_video` (`agent/videos.rs`) is local ffmpeg: `info`, `concat` (every clip fitted to the first's frame; audio only
when every clip has it), `trim`, `resize` (presets 720p / 1080p / square / vertical), `speed` (`atempo` chained),
`mute`, `add_audio` (looped and cut to the video), `caption` (drawtext with the Noto CJK font, text via a temp file),
`gif`. Outputs go to `videos/`, a `video` event is emitted and the file is sent to the chat unless `send=false`.

Skills: `drive-cleanup` (quota, big and unused files in one turn, a numbered list the user confirms, one `drive_trash`
batch, the 30-day note), `drive-video-edit` (search → download → `edit_video` chain with `send=false` on intermediate
steps → upload back), `drive-photo-storybook` (photos → `file_look` → optional edits → text → one `make_pdf` with
`![…](drive/…)` pages → upload). All three are `browser: off`.

Tests: `tests/fake_apps.py` serves a Drive fake (`/drive`, `/drive-upload`: seeded folders, two real MP4 clips made with
ffmpeg at start, three JPEG photos, big and old files with metadata only, a shared file, a trashed one; `/drive/_state`
and `/drive/_reset` for the suites) behind the same fake Google OAuth as the calendar; `tests/drive_connect_e2e.py`
(one sign-in connects both, a sign-in where Drive was unticked, catalog tools and permissions, disconnect one),
and the real-model `drive_cleanup_real.py`, `drive_video_real.py`, `drive_storybook_real.py` (the user's Drive as they
see it; photos in `tests/pages/drive/`).

## PDF page counts

`make_pdf(pages=N)` fits the PDF to the most pages the user asked for ("a one-page packing list"): the browser prints it,
counts the pages (`lopdf`) and prints it again 10% smaller each time, down to 60% (`PDF_MIN_SCALE`, still readable), so a
list that runs a little over fits without the model guessing how much to cut. The result says the count and the scale;
when it still does not fit, it tells the model to shorten the content (`tests/pdf_e2e.py`).

## Image and video generation

`make_image` (`imagegen.rs`, the OpenAI Images API: `POST {base}/images/generations`, `/images/edits`) and
`make_video` (`videogen.rs`, the OpenAI Videos API: `POST {base}/videos` starts a job, `GET {base}/videos/{id}` polls
it, `GET {base}/videos/{id}/content` downloads the clip; a server that answers the POST with `data[0].b64_json` /
`url` is taken as it is) share one pattern:

- **Endpoint and key are optional.** Each kind resolves `<kind>_base_url` ("" = the model endpoint) and
  `<kind>_model` ("" = auto-pick from the endpoint's `/models` by mode or name) from the settings in force, and its
  bearer key with `imagegen::media_auth_headers`: `<kind>_api_key` when set, else the model endpoint's key. One
  provider for everything needs nothing but the model fields; a chat model at one provider and pictures or clips at
  another needs the extra URL (and key, if different).
- **Never a mix of the system's and the user's.** With the system's models the values are `[model] image_url` /
  `image` / `image_api_key` and `video_url` / `video` / `video_api_key` from `config.toml` (`store::system_model`,
  `effective_model`). With "use my own model" (Settings → Models) they are the user's own, optional, values; the
  system's media endpoints and keys are never used with the user's set, and the user's never with the system's.
  The defaults of `image_base_url` / `video_base_url` in the settings table are therefore "" — the system's URLs are
  overlaid by `system_model()` only while the system's models are in force.
- **Keys never leave the runtime.** `store::SECRET_KEYS` (`model_api_key`, `image_api_key`, `video_api_key`) are
  stripped by `redacted()` and reported as `<key>_set`; the audit log carries `(set)` / `(cleared)`. `PUT
  /api/settings` keeps a saved key on "" and clears it on `null`.
- **Vendor differences are absorbed, not configured.** Optional request fields (`quality`, `background`,
  `response_format`, … for images; `seconds`, `size`, `aspect_ratio` for video) are dropped one by one when a server
  rejects them by name; sizes walk a fallback list, and after the first rejected size of a non-square shape the shape
  is sent once as `aspect_ratio` without a size (xAI's grok-imagine models take ratios and only 1k/2k squares as
  sizes; OpenAI answers "unknown parameter" and the next size is tried). A video length the server does not offer is
  snapped to OpenAI's 4 / 8 / 12 once, then left to the server. Moderation refusals are relayed as such, without a
  retry.
- **Cost guards.** `[model] image_budget` (6) pictures and `[model] video_budget` (2) clips per task; the tools tell
  the model when the budget is spent, and the prompt says to generate once and offer changes.
- **Tests.** Unit tests with a fake transport in both modules; `tests/imagegen_e2e.py` and `tests/videogen_e2e.py`
  against the fake server (`/v1` and a second provider at `/vid/v1`, which records the bearer of every call);
  `tests/media_real.py` makes one picture and one clip with a real gateway (`xai/grok-imagine-image`,
  `xai/grok-imagine-video`) and is not in the default list.

## Profile

The profile (Memory → Profile) holds the fixed details forms and emails need: the named fields in
`store::PROFILE_FIELDS` (name as in the passport, phone, emails, home and work address, company, birthday,
nationality…), any number of further addresses as `address:<label>` ("Singapore flat", "Parents in Sydney"), and free
fields as `custom:<label>`. The agent reads it with `profile_get` (only the fields a form needs; `address:*` selects
every further address) and proposes changes with `profile_suggest`, which the user confirms on the Memory page.

## Long-term memory: structured fields, the model's reading, code's selection

No code reads a fact's prose or the request's words to sort or pick memory (CLAUDE.md, natural language is the
model's). Every fact carries structured fields the model sets when it writes it — `domain`, `topics`
(`store::TOPICS`), `scope` (always · topic · trigger with its `trigger` phrase), `kind` (`id_document`; `site_block` is
code's own), `entity` (a website as its host) and, on the entity, the user's `relation` — through `memory_remember`'s
JSON, the extraction prompt (`prompts::MEMORY_EXTRACT`) and, for a fact written without them (before 0.6.0, typed on
the Memory page), the sorting step `memory_tidy::classify` (prompt `MEMORY_SORT`, JSON; at once for a typed fact, in the
background for older ones, and in every tidy run). Code validates them against the enums (`FactOpts::with_model_fields`).
At a task's start the model reads the request against what memory holds and answers with choices from JSON lists
(`context::relevance`, prompt `MEMORY_RELEVANCE`: topics, entities with relation / aliases / one fact, trigger phrases,
unsorted fact ids); `context::select` scores the facts on their fields plus the task's data (`context::task_data`: the
bot, the skills loaded, the tools called). Memory → "Try it" runs the same path. `memory_search` takes `topics` and
`entities` from the model besides its query words; the words are only a full-text search (FTS / LIKE) over the stored
text. Identity-document and unsorted facts stay out unless `id_documents`. What a task used is recorded
(`task_memory`: topics, entities, fact ids) and its episode carries the fact ids, so forgetting a fact deletes the
episodes that used it by id (`Store::delete_fact`; `forget_fact` also drops pre-0.6.0 episodes, which carry no ids).
Memory tidy merges only facts whose text is identical after normalisation; other wordings are the LLM review's to merge.
The one text check left in code is the secrets backstop `store::looks_sensitive` (card / ID-shaped numbers, "password",
密码, CVV), a documented safety exception.

**Memory page layout**: the pending profile changes are the whole top row; the profile and the Vault (ID documents,
memberships, credit cards) sit side by side below it (`#memPending`, `#memProfileVault`).

**Credit cards live in the Vault, never in the profile.** A card the user holds can be saved by its name alone —
issuer and product, "Citi Prestige", with the issuing country ("USA") as a field of its own, optional (the
`vault_items.country` column: not a secret, the card picker reads the market from it) — as a Vault `card` item whose
number, expiry and CVC are `UNKNOWN` (`vault::UNKNOWN`) until the user adds them on the Vault card: "+ Add credit card" with only the name,
`vault_add_card(name, country)` (the card bot, or Main when the user names a card), or memory extraction (`kind: "card"` →
`POST /internal/vault/card`, audited as `vault.add` by the agent). `vault::add_card` keeps one item per card (the name,
case and spacing aside) (a country given later fills an empty one) and refuses a name with a run of five digits or more. Such an item lists its `missing` details;
`vault_list` says them, the Vault card shows "Name only: add the number…", the word UNKNOWN is never scrubbed from
pages, and in code a fill of an UNKNOWN detail and a `purchase_confirm` on that card are refused with the way to
complete it (`policy::card_details_missing`; `vault.rs`, `policy.rs` tests, `tests/memory_vault_e2e.py` CARDFACT,
`tests/profile_fields_ui.py`). The credit-card skills read and write cards in the Vault only.
Identity, membership and card numbers never go into the profile: they live in the Sentinel vault.

**First login.** While the profile lacks the name as in the passport, the birthday or the nationality
(`GET /api/profile/status`), the first chat view of a browser session goes straight to Memory → Profile and the chat
shows a banner until the profile is complete or the user chooses "Later" (`profile_reminder` setting). The profile
card takes a photo of the passport data page or an ID card: `PUT /api/profile/ocr` sends it to the vision model
(`You read an identity document…`, JSON answer) and returns the name, birthday and nationality to prefill the form
and the document number as a proposed vault item, which the user saves with one click. The picture is read in memory
and never written to the workspace; the number is not logged and not stored in the runtime.

The arrival-card skills use the addresses like this: a hotel booking for the dates in mail is the accommodation;
otherwise the one profile address in the destination country is used without asking (a resident going home, an
owner's flat); several addresses in that country → the agent asks in the chat which one, before opening the browser;
none → calendar and memory, then one question (or an uploaded booking). `tests/arrival_address_real.py` and
`tests/arrival_address2_real.py` cover the one-address and the two-address cases with a real model.

## Known forms: one tool, one function per URL (2026-10-31)

The model has one tool for every known form, `form_fill`, whose arguments are only the trip facts that need
understanding (flight, arrival date, origin city / country, a departure date or next city when one is given, the stay
and its kind, the purpose). The runtime reads the task's page URL from the browser and looks it up in
`crate::forms` (host + path → a Rust function written against the real site's form); the function turns the facts,
the profile and the defaults (departure = arrival + 7 days, next city = the flight's origin, Yes / No = No) into a fill
plan — selectors, values in the form's formats, options, steps — and the call goes to Sentinel as `browser_fill_form`
(never offered to the model), which adds the Vault ID document's values (masked in pictures, never into card fields)
and runs the plan in the browser in one call (`/agent/fill_form`). The result says per field what went in and what did
not, never a value. A page no function knows, or a field a function does not find, falls back to the generic path —
the model field by field, with the essential guards only. `form: on` in a skill's front matter now only marks a form
task (the 70-step budget, the hand-over rules); the separate "Form filling" model setting was removed on 2026-10-31:
with forms filled in code, the executor reads the facts and handles the fallback. An optional model (the planner) that
stays unavailable after its retries hands the call to the LLM (`chat_or_cancel`, a `model_fallback` event); each real
suite's model calls are saved as `models-<suite>.json` in `OMUSE_E2E_OUT`.

### Form functions so far (2026-10-31)

| Form | Function | Mapped from | Stand-in | Notes |
|---|---|---|---|---|
| SG Arrival Card | `sg_arrival_card` | the live ICA form | `tests/pages/sgac.html` (landing → 3-step stepper → Review) | radios by text, split date boxes typed by key, type-and-pick lookups; stops at Review (declaration + Submit are the user's) |
| Malaysia MDAC | `my_arrival_card` | the live form | `tests/pages/mdac.html` | read-only datepicker boxes (`date` op), order: nationality → dialling code, state → city → postcode; the slider puzzle is the user's |
| Korea e-Arrival card | `kr_arrival_card` | the live form | `kr_landing.html` → `kr_terms.html` → `kr_arrival.html` | terms + email first; type-to-filter nationality / airline dropdowns; the address lookup popup, job and contact in Korea are left (best effort); the emailed code and Submit are the user's |
| All Indonesia | `id_arrival_card` | the live site (every Next saves) | `id_arrival.html` (4 steps on their own paths) | Languages → English → Foreign Visitor; session-suffixed ids (prefix selectors); readonly dropdowns with a Search box; the visa question defaults to No (a visa on arrival); the certification and Submit are the user's |
| Taiwan TWAC | `tw_arrival_card` | the live site (steps 2–4 rendered client-side) | `tw_redirect.html` → `tw_home.html` → `twac.html` | step 1 (email, Cloudflare, emailed code) is the user's: the plan `require`s the traveller form and stops until it shows; react-select comboboxes take the exact option text; visa-exempt nationalities as data (`TW_VISA_EXEMPT`) |
| China arrival card | `cn_arrival_card` | the live site (steps 2–4) | `cn_arrival.html` | the passport-photo upload (step 1) is the user's (`require` on the Last Name field); Element UI fields found by their labels; visa-free nationalities as data (`CN_VISA_FREE`); the port by airport code; stops before the companion question |

Still on the generic path: Thailand TDAC (behind a Cloudflare check; mapped only from its public code), Cambodia
(a profile photo; its health questions not captured), Visit Japan Web, eTravel and the UK ETA (behind accounts and
emailed codes: their fields come from official guides, not live pages). The operations added for these forms: `require`
(stop for the user's step — the field must be *visible*, not merely in the page: Taiwan's traveller form is in the DOM, hidden, before the email step; 2026-11-01), `option` / text fallbacks that click the deepest visible element with the text, visible
matches first, and a guard against setting a value on a non-text element. A field-by-field fill on a page a function knows gets a note to call form_fill
(`guards::form_fill_note`); a hand-over on such a page before any form_fill is refused once, so code fills it first and names the user's step (`guards::takeover_before_fill_guard`). `browser_fill_form`'s ops (`FILL_ONE_JS` in omuse-browser): text, select (value, text,
starts-with, contains), radio, click / button / check by selector or by text inside a headed section, `date`
(datepicker), and in the broker `keys` (real key presses), `pick` (type, then click the listed option) and `wait`.

## Arrival cards and the HK tax refund: workflow tools (0.6.0, docs/json-tools.md)

Filling in an arrival card or travel authorisation with a form function (`agent/driver.rs` `CARDS`: SG, MY, KR, ID,
TW, TH, CN, JP, KH, PH, UK ETA) is ONE `arrival_card_fill` call; the Hong Kong departure tax refund is ONE
`apdt_refund` call (`agent/apdt.rs`). Both are Travel-bot tools (`bots/travel.md`). The model reads the user's words —
and the bookings the machine fetched — and passes the facts as JSON arguments; code validates them and runs every
step; the reply is written from the machine's JSON result (`must_say`). No regex or keyword reads the user's words or a
mail's or a document's text in these machines (the old `driver::extract`, `user_asks` / `asks_to_act`, `dates_in`,
`method_in`, the document classifier and `EXTRACT_SYSTEM` are gone).

- **`arrival_card_fill{country, flight_no, arrival_date, origin_city, stay_name, stay_address, own_address, purpose,
  departure_date, next_city, occupation, …}`**, `driver::check`: the country in the table, the flight number an
  airline code and digits (`norm_flight`), the dates ISO and the arrival not past, the own address one of the
  profile's labels; the fields of the chat's earlier calls for the same country carry over (`with_earlier`). The trip
  not known and a mailbox connected → `read_mail`: the machine runs the two fixed searches (the card's `mail_words`),
  fetches the first hits and returns `mail_read` — the workflow tools stay offered (`workflow::reply_turn`) and the
  model calls again with what the mails say. Then: one profile address in the country → the stay (`own`), several →
  `choose_address` (the machine shows the choice card, result `needs_choice`; the user's pick comes back as
  `own_address`), still missing → `needs_input` (flight, date, stay, and the purpose with them; else purpose holiday,
  `purpose_assumed`). The UK ETA needs no trip (`CardSpec::needs_trip`); its occupation goes to the form
  (`forms::Inputs::job_title`: the call's, else the profile's).
- **The machine** (`driver::next`): the filing window (too early → `schedule_create`, result `scheduled`), the official
  page, `form_fill` with the facts, a hand-over for a step that is the user's (a form function's `require` stop),
  `form_fill` again, the review hand-over (the reason names the assumptions and the card's `user_steps`), the card's
  reference element after the hand-back (`browser_read_elements` at `ref_sel`, its value checked against `ref_re`) → `submitted` / `not_confirmed` / `waiting_on_user` (the same
  user's step undone twice) / `handed_over`.
- **`apdt_refund{in_flight, in_date, out_flight, out_date, in_pass_file, out_pass_file, receipt_file, tax_hkd,
  passport_file, refund_method, wallet_account, card}`**, `apdt::check`: flight numbers, dates, the files are this
  chat's (the user's `uploads/…` or the saved `attachments/…`, a bare file name matched), the method the platform's;
  documents not known → `read_mail` (the fixed search, every PDF / picture attachment saved, every PDF read, the
  user's PDFs too) → `mail_read` with the files; the passport defaults to the user's only picture; what is still
  missing → `needs_input`. The machine: the rule (`apdt::ineligible`: same or next day, after the departure, within 28
  days, `tax_hkd` 0 → `not_eligible`), the Vault card (the one the user named, else the first complete one), the
  platform's pages by their form functions, the uploads by the page's box labels, `browser_fill_secret` (or the user's
  takeover for a card saved by name), stop before the face step → `confirmed` with the reference, the link and the
  face-recognition deadline in `must_say` (`stopped` with the site's own error lines, `not_eligible`, …).

Calls are `drv_<n>` / `apd_<n>`; the machines' tool calls and fetched mail are kept by the compression
(`compress_transcript`). Tests: `driver::tests`, `apdt::tests` (arguments in, steps, Done JSON),
`tests/arrival_*_real.py`, `tests/apdt_real.py`.

## Guarantees in code (2026-11-02)

The hard rule of `CLAUDE.md` holds in the runtime loop: **no natural language from the model anywhere in the decision
flow**. Nothing is pushed to the model as a "(System)" message during a task, no regex over the model's prose decides
a step, a refusal, a rewrite or a hint, and no tool result tells the model what to call next. What the model writes is
the reply the user reads; everything else is data or a code action:

- **Model calls that return JSON only**: the planner and the bots router (`agent/bots.rs`), `Runtime::until_condition`, the decisions (`Runtime::decide`, below), and
  `Runtime::review_answer` — the one reading of the final
  answer as the user would (ok / problem); when it does not hold together the answer is written again from the run's
  facts by `rescue_final` (code, once), never by a hint. `rescue_final` also writes the reply when the model returned
  no text or only tool markup (`strip_tool_markup` runs on every final answer). A reply cut by the token limit
  (`finish_reason: length`) is requested once more with a higher limit and the longer one is kept
  (`continue_cut_answer`). The English-UI fix for a Chinese answer stays (`rewrite_in_english`, triggered by the
  measured CJK share, `language_fixed` event).
- **Questions to the user are a tool**: `ask_user(fields, question)` (`agent/tools.rs`, `agent/local.rs`). The
  dispatcher refuses, in data, a field the profile or the form's defaults already answer (`guards::known_answer`:
  nationality, birthday, home address, sex; an arrival card's departure date, next city, declarations; the one profile
  address in the country) — `{"refused": "known", "known": [{field, value, source}]}` — and otherwise ends the run
  with the question as the reply (`asked` in the loop, a `needs_user` event, status COMPLETED) and an
  `{"type": "ask"}` card in the chat. `guards::asked_user` reads the call back for the appenders (`with_mailbox_hint`
  when no mailbox is connected, `with_own_addresses` for `stay` / `which_address`). `present_choices` stays for
  options; its result is `{"shown", "verified", "picked"}` plus the listing.
- **Budgets are tool-list limits** (`Runtime::budget`): the finishing tools only in the last `BUDGET_RESERVE` steps or
  past `TIME_NUDGE` of the minute budget; the page-reading tools withdrawn after `RESEARCH_NUDGE_PAGES + 3` page
  visits; the search tools withdrawn after `WEB_NUDGE` web searches (`guards::web_budget_guard` refuses one more in the
  same turn). When the step or minute budget is spent the model gets no tools and no message. A stuck run (every call
  of a turn refused) re-plans at the second turn and stops at the third. The live state reaches the model as ONE JSON
  message at the end of each request, never stored (`prompts::live_state`: `{"state": {time, minutes, plan: {objective,
  steps: [{id, step, status, parallel}], next_parallel}, dead_ends: [{host, fails, why_code}]}}`, only when there is a
  plan or a dead end); the system prompt is identical on every step of a task (prompt-cache reuse).
- **Refusals are results, not instructions**: every guard answers `ERROR: [reason_code] one neutral sentence` (bilingual,
  `system_text_en` keeps the English half): `repeat`, `retype`, `unknown_id`, `dead_host`, `one_search`,
  `search_order`, `no_mail_for_search`, `empty_mail`, `arrival_mail`, `user_submits` (the sentence `… that click is the
  user's own` is what `exec::hand_over_button` reads to hand the page over), `keep_default`, `apdt_face`,
  `form_first`, `user_edit`, `web_budget`, `other_bot`, `browser_off`, `remade`, `bad_arguments`,
  `arguments_cut`, `needs_approval_alone`, `[SITE BLOCKED site=…]`. The re-read pointer ("Already read: call c1") and
  Sentinel's `DENIED by Sentinel: <reason>.` carry no advice. Tool results carry facts as data: `input_fixed` on a
  `browser_type` whose text code replaced (departure = arrival + 7 days, next city = where the flight came from),
  `{"browser": "off" | "on"}` from `load_skill`, `{"profile": {field: value | null}}` for a skill's `profile:` fields,
  `{"sent": true}` / `{"already_sent": […]}` from the file tools, `{"made_times": n}`, a published report's JSON.
- **Code actions in the loop** (transcript facts only, never the answer's words): the automatic hand-over
  (`guards::needs_handover`: a typed-into arrival or claim form with no completed takeover after the last fill, or an
  approved purchase in a browser run; `auto_handover` event), the hand-over after a refused final click
  (`hand_over_button`), the snapshot after any hand-back (`needs_handback_snapshot`), the news report's delivery
  (`news_publish`'s schedule and mail, `news_mail`), files the request asked for sent at once (`auto_send`,
  `unsent_files`), the official site opened when the agent landed on a parked domain (`guards::parked_domain`,
  `exec::official_site`: one web search, the first result on another domain), the arrival defaults typed by code
  (`departure_fix`, `next_city_fix`, `departure_default` → `set_departure`, `default_no`), the APDT ticks
  (`apdt_fixed_ticks`).
- **The reply carries its facts through the review, never through appended text** (phase 2d, docs/json-tools.md
  "Facts the reply must carry"): `facts::run_facts` collects the trusted tools' `must_say` (a machine's result,
  `emergency_contacts`, `visa_rules`, `fx_convert`, `loan_payment`, an emergency `phone_call` refused in code) and the
  run's structured facts (`guards::*_fact(s)`: the model-read hand-back reference (`confirm_reference`), approval edits, ETIAS, the card
  catalog's date / unverified rows / FX fees / uncatalogued cards / the user's cards, the approved amount, an injection
  notice, notifications, a model-made comparison's options); `review_answer` gets them as `must_say` and
  `reply_with_facts` (the model) adds what the reply left out. Still code on the reply: `with_masked_cards` (secrets), the
  link repair (`links::repair_answer`), `merge_stranded_answer` / `with_handover_answer` (the model's own earlier text).
  Removed: the travel appenders (the Travel bot's flight and hotel searches are the booking machine's,
  `guards::MACHINE_ONLY`), the grounding of figures / codes / phone numbers in the reply (`number_check`), the loan,
  emergency-call, visa, FX, medical and consular appenders (now the tools above).
- **Outcome from events only** (`outcome::assess`; the request's "send" words are no longer read — `asks_send` /
  `with_request` removed): a purchase, send, calendar change or call is verified by its own
  evidence (the order number from structured JSON only — a known site's machine result, read at the page's reference
  element, or the model's `confirm_reference` on an unknown site — `confirm::confirmed`, its source `known_page` /
  `model_read` kept in the evidence; a message or event id); neither the answer's words nor a page's text is read for
  it. `outcome::notice` adds the "not confirmed" line when an action ran without its proof; the ledger gets that order
  number. A delayed flight's times are the `flight_status` machine's (the airline's status table read by element).
- **Hard stops stay two**: the final click is the user's (`policy::hand_over_click`, `arrival_submit_guard`,
  `apdt_face_guard`) and the Vault's secrets.

Gone with the rule (2026-11-02): `answer_traits` / `AnswerTraits` and every hint built on them (`arrival_default_hint`,
`self_press_hint`, `reconfirm_hint`, `page_option_hint`, `known_profile_hint`), `short_after_handback_hint`,
`proceed_with_pick_hint`, `visa_days_hint`, `missing_format_hint`, `card_comparison_hint`, `news_report_hint`,
`report_followup_note`, `form_fill_note`, `arrival_ask_first_guard`, `departure_date_guard`, `next_city_note`,
`requests_info`, `asks_user`, `cut_off_final`, `self_corrects`, `with_hours`, the time / web / stuck / step-limit /
status-note / cut-off / plain-text / number / outcome / replan "(System)" messages, `outcome::nudge` and its claim
regexes, the page hints (`deep_link_hint`, `product_page_hint`, `refusal_hint`, `parked_domain_hint`), and the
"(System note)" tool-result notes. Four of the judgements they covered are back as decisions (below).

### Decisions: `Runtime::decide` (`agent/decide.rs`, 2026-11-02)

When a step genuinely needs the model's judgement, it is a decision API call: the question, the context and the
choices as a JSON array; the model answers `{"choice": "<one of them>"}` (`decide`) or `{"choices": [...]}`
(`decide_many`), temperature 0, 60 tokens, no tools. The answer must be a listed choice (exact, case-insensitive,
returned in the list's spelling); anything else gets ONE retry (the bad answer and the list again), then the caller's
default; an unreachable model gives the default at once. Every call emits a `decision` event `{question, choices,
picked, retried, defaulted}`. Code acts on the choice; defaults are the no-op choice. Decisions are only on PAGES now (phase 2d):
the held pick, the file formats and the visa-free days went to JSON — `travel_book` takes the pick, the file calls carry
`deliver_as` (each format the run did not make is converted by `judgements::conversion_calls` and sent, `fmt_<n>`
calls, `format_conversion` event), `visa_rules` gives the official table's days (agent/facts.rs). What remains
(`agent/judgements.rs`, none in a driven run):

- **A page just opened** (`exec_call`, main run): a deep link (a path or query) redirected elsewhere → decide on the
  title + first lines `error_page | normal_page | login_page`; `error_page` → code opens `scheme://host/`
  (`deep_link_error`, once per site). In an `online-shopping` run, back on a page it opened and left without clicking
  that page's Add-to-Cart button (found by code: `[eN] button "Add to cart|bag|basket"`) → decide `product_page |
  listing_page | cart_page | other`; `product_page` → code clicks that ref (`add_to_cart`, once per page).

### Driven forms settle every value before the form opens

In the arrival-card, refund and claims machines the facts, the defaults, the choice of address and what the user must
give are settled by the tool's `check` and the steps before the `browser_navigate` to the form (`needs_input`,
`needs_choice` before any page opens). Once the form is open no question is asked again: a fact lost since
(compression) is a blank field the user fills in at the hand-over. The only pauses after the navigate are the
hand-overs for the user's own steps. Tests: `driver::tests::every_card_fills_from_its_arguments_and_asks_nothing_after_the_page_opens`,
`apdt::tests` and the claims tests (`driver::questions_after_navigate` lists an `ask_user` / pick-less
`present_choices` after the navigate).

## News research reports (`skills/news-research/`, 2026-11-01)

Main's `news-research` skill turns "research the news on X" into a newspaper-style edition:

- **Search in the browser only.** `browser_news_search(query, days)` (Sentinel, connector `search`) opens the engines'
  own news results in the browser: DuckDuckGo html with its date filter (`df` = d / w / m / y), then Bing News with its
  interval (`Broker::news_engines`); the test engines get `&df=…`. It never falls back to a search API. In a run that
  loaded the skill, a `web_search` call is marked `browser_only` (Sentinel skips the APIs) and a `news_search` call is
  run as `browser_news_search` (`exec_call`, `guards::news_research_run`).
- **Two workflow tools (docs/json-tools.md, `agent/news.rs`).** `news_gather{subject, window}`: code runs ONE
  `browser_news_search` of the subject (a second, wider one only when the first finds no article), reads the first six
  distinct links in one turn and returns the articles as JSON (number, title, publication, date, url, text). The model
  writes the edition as JSON and calls `news_publish{title, stories:[{article, headline, summary}], insights, schedule,
  weekday, hour, email}`: `news::check` keeps only stories whose article number exists, takes every link, publication
  and date from the gathered articles, and adds an article the model left out with its own headline and opening. The
  machine never calls the model and reads no user wording (the subject regexes and the in-code writing call are gone).
- **Layout in code.** The machine calls `news_report` (code's own call, never offered to the model): `crate::newsreport`
  drops stories without an http(s) link, keeps each article once, escapes every value and fills the skill's
  `template.html` (masthead, lead = the first story, insights, stories in columns, sources). The HTML goes to
  `<workspace>/artifacts/news/<subject slug>/<YYYY-MM-DD-HHMM>.html` (a second edition in the same minute gets `-2`),
  the PDF beside it through Chromium's print (`/internal/render_pdf`); both are file cards in the chat. A scheduled run
  also notifies the user ("📰 <title>").
- **`/artifacts/`** (Sentinel `routes/artifacts.rs`, behind the login gate): `/artifacts/` lists the files newest first,
  `/artifacts/<path>` shows one; paths stay inside the folder; HTML is served with a policy that runs no script and
  loads nothing from elsewhere (`HTML_CSP`, sandboxed). A link `/artifacts/…` given as a mail attachment is the
  workspace file `artifacts/…` (`actions::artifact_path`).
- **Delivery from the JSON.** `schedule` (daily / weekly / monthly) → `schedule_create` by code (cron from
  `newsreport::Ask`; the goal, `news::schedule_goal`, makes the scheduled run's model call `news_gather` →
  `news_publish`, with `email: true` when the editions are mailed); no new schedule in a schedule's own run. `email:
  true` → `gmail_send` to the user's own connected address with the PDF and HTML. "Email it" later in the chat →
  `news_mail{files}` (the edition's links the model copies from the earlier reply; code accepts only
  `artifacts/news/….pdf|.html`). The result's `must_say` carries the lead, the two links, the schedule and the mail;
  `next_question` offers the mail. The old regex guards (`news_report_action`, `news_schedule_goal`,
  `report_followup_call`, `with_report_links`) are removed.
- Tests: `tests/news_report_real.py` (a made-up subject, Lumora, in `tests/fake_apps.py NEWS`, so every fact comes from
  the articles read: the report, its links, "email it" landing in the SMTP sink with the PDF, a scheduled edition);
  `tests/news_demo_real.py` is a one-off demo on the real web (not in the gate).

## Sentinel policy

**The purchase's site comes from the open page.** When earlier versions showed the model links as stand-ins
(`crate::links`), a `purchase_confirm(site=…)` written from a link named the stand-in's host, `link.omuse.local`. Sentinel's act route
(`purchase_site`) replaces such a site with the host of the page the task has open — where the total shows and the
card is filled — and with no page open the policy refuses it ("open the booking page first"). Before this, the
purchase was approved for "omuse.local" and the card fill on the booking site was refused (travel_michelin_e2e,
2026-10-30). The fake booking flow writes the stand-in's host on purpose (`tests/fake_llm.py` `travel_book`).

**The approved amount is always in the answer.** When a run had a purchase approved (`purchase_confirm` →
`"status": "approved"`) and the final answer does not give its total (in any format: `JPY 28,000`, `¥28000`,
`28,000.00`), `guards::purchase_total_facts` gives "Amount you approved: JPY 28,000 (<the items>)" — also when the answer gives the
amount but not what it was for (the first word of the item's name).

- **Data classes and taint**: reading mail, the vault or private connectors taints the task; a tainted task that
  sends data somewhere new (a URL with parameters on a site it hasn't used, a message to a new recipient) is asked
  for (the data-egress check). A domain written in the user's own request is trusted.
- **Browser auto-approval** (`[browser] auto_approve`, default on; Settings → Agent): with it on, nothing in the
  browser asks — page loads, clicks, typing, submits, uploads, vault fills (the passport into an arrival card), the
  purchase confirmation and the pay / order-change clicks, also after a suspected prompt injection
  (`Decision::browser_auto_ok`; the user's decision, 2026-10-30). Each runs at once with an approval record resolved
  by "auto", audited, and shown in chat and the timeline like a manual approval. Off: every risky browser action asks
  again. Logins, CAPTCHAs and a form the user submits themselves are takeovers (a hand-over, not an approval).
- **Vault**: secrets are encrypted with `vault.key`; the agent refers to an item by id and Sentinel fills the value
  into the page, restricted to the item's domains, so the value never reaches the model, the chat or the trace.
- **Prompt injection**: mail, pages and tool output are untrusted; suspicious content escalates writes to approval.
- **Audit**: `audit.jsonl` is append-only and hash-chained (`/sentinel/api/audit/verify`).

## Web UI language

The UI shows one language at a time: English by default, Chinese only when the language setting says so. Strings go
through `T()` / `Tf()` with the tables in `web/i18n.js` and `web/i18n_tw.js`; `python3 tests/i18n_check.py` fails on
missing or stale translations and on mixed-language strings.

## Tests

- Rust unit tests live next to the code (`cargo test --workspace`).
- The end-to-end suites (`tests/*_e2e.py`, `*_ui.py`, `e2e_0*.py`) run inside the `Dockerfile.test` image: each gets a
  fresh stack started by `tests/run_local.sh` (the binary, the scripted model `tests/fake_llm.py`, fake third-party
  services in `tests/fake_apps.py`, an IMAP server, test pages under `tests/pages` served as `http://shop.test:8099`).
  The scripted model plays the agent: it matches a marker in the request (`SGAC`, `ARRCARD`, `ARRNOMAIL`, …) and
  returns the tool calls a correct agent would make, so a suite checks the tools, approvals, takeovers and the
  rules a skill must keep.
- The travel-skill suites share `tests/skills_common.py` (mail seeded by IMAP APPEND, calendar, travel connector,
  profile, vault, uploads, approvals and takeover hand-backs).
- Real-model suites (`*_real.py`, and the travel suites with `OMUSE_E2E_ENV_FILE`) go through `tests/model_proxy.py`,
  which times model calls so the `TIMING` line reports the agent's own time.

## Publishing images for Products Provisioner

`.github/workflows/release-image.yml` builds `Dockerfile.fly` for linux/amd64, tests the image
(`tests/docker_image_smoke.sh`) and pushes it to `ghcr.io/<repository-owner>/<repo>` on an `omuse-v*` tag (older
`locius-v*` tags are still accepted). It uses the workflow's `GITHUB_TOKEN` (`packages: write`); make the GHCR package
public after the first push. No password or key is baked in: each instance gets them at run time.

In the provisioner's `product.toml`, use a registry image source and **Sync image** with the new tag:

```toml
[image_source]
type = "registry"
repository = "ghcr.io/second-state/omuse"
```

Use a new version for changed source; never move a release tag (the provisioner does not let a version change its
image). For an optional callback import (`import_release` on a manual run, callback image sources only), set the
Actions variables `RELEASE_PRODUCT_KEY` and `RELEASE_IMPORT_URL` (HTTPS origin only) and the secret
`RELEASE_IMPORT_TOKEN` (that product's `callback_token`, not an admin key). The import is `POST /api/images` with
`product_key`, `version`, `image` (digest) and `parent_version: null`; the `image-manifest-<version>` artifact keeps the
JSON. Provisioner settings: port **8080**, persistent storage at **`/omuse`**, health check **`/sentinel/api/health`**,
a generated `OMUSE_PASSWORD`; keep the app always-on for schedules, triggers and Telegram.

Check an image locally without credentials:

```bash
docker build --platform linux/amd64 --build-arg OMUSE_VERSION=omuse-v0.4.1 -f Dockerfile.fly -t omuse:local .
OMUSE_VERSION=omuse-v0.4.1 bash tests/docker_image_smoke.sh omuse:local
```

The smoke test checks the three services, login, unprivileged start with a root-owned volume, Chromium PDF rendering
and persistence across container replacement, and shows the version from the tag (`omuse-v0.4.1` → `v0.4.1`).

# Knowledge files shipped with OMuse

Some skills carry data beside their `SKILL.md`; the agent reads it with `load_skill(name, file=…)`. This file says
where each knowledge base lives, what shape it has and how to update it. Rules shared by all of them:

- The source of truth is the file named below; generated files are rebuilt by the skill's script and never edited by
  hand (a header says so).
- Every fact carries a verification date and, where possible, a source URL; the agent is told to say the date and to
  treat rows marked unverified as leads to check on the official page, not as facts.
- Reference files are plain text or Markdown under the skill's folder (they are compiled into the binary with the
  other skills, extracted at start under `skills-builtin/`, and read with a 400 KB cap per file).

## Credit card catalog (`skills/credit-card-picker/`)

What the `credit-card-picker` skill reads to answer "which card should I use for this purchase".

| File | Role |
|---|---|
| `cards/catalog.json` | **source of truth**: one record per card |
| `cards/us.md`, `cards/hk.md`, `cards/sg.md` | generated per-market tables the agent loads (`load_skill(file="cards/sg.md")`) |
| `build_cards.py` | rebuilds the tables from the JSON, checks it, and can import the personal_finance catalog |

### Record shape

```json
{"issuer": "Chase", "product": "Sapphire Reserve", "aliases": ["CSR"], "region": "US", "currency": "USD",
 "network": "Visa", "unit": "points", "point_value_cents": 1.0, "annual_fee_cents": 79500, "fx_fee_pct": 0,
 "rules": {"dining": 3.0, "travel": 4.0, "transit": 5.0, "other": 1.0},
 "caps": {"travel": "direct airline / hotel bookings only"},
 "merchant_rules": [{"keywords": ["lyft"], "earn_rate": 10.0, "end_date": "2027-09-30", "notes": "…"}],
 "rule_notes": {"travel": "4x only on flights and hotels booked directly …"},
 "benefits": ["8x through Chase Travel …"], "verified_at": "2026-08-25", "verified": true,
 "source": "https://creditcards.chase.com/…"}
```

- `region` is the issuing market (`US`, `HK`, `SG`, …) and decides which table the card lands in; `currency` is the
  card's billing currency.
- `unit` is `points`, `miles` or `cashback`; `point_value_cents` is the value of one point / mile at a plain
  redemption (cash, statement credit, the programme's standard rate) — for `cashback` the rate itself is the value.
- `rules` maps the canonical categories (`dining, travel, transit, groceries, gas, online_retail, streaming,
  entertainment, drugstore, utilities, insurance, foreign, other`) to the multiplier (points per unit of currency) or
  the cashback percentage; `other` is the base rate and must be present.
- `caps` and `rule_notes` carry the conditions in words (monthly caps, direct bookings only, minimum spend); the agent
  quotes them. `merchant_rules` are merchant-limited multipliers with an end date.
- `verified_at` is the day someone checked the issuer's page; `verified: false` marks a seed from general knowledge
  (the Singapore rows shipped today), which the table shows as **UNVERIFIED (seed)**.

### Updating

1. **Change or add a card**: edit `cards/catalog.json` (a new record, or new numbers in an existing one), set
   `verified_at` to today and `verified` to true, and put the issuer's page in `source`. Remove a discontinued card
   or set its `rules` to what still applies and say so in `rule_notes.other`.
2. **Rebuild and check**: `python3 skills/credit-card-picker/build_cards.py` — rewrites the `.md` tables and prints
   `PROBLEM` lines for duplicates (issuer + product + region), missing fields, unknown categories and verification
   dates older than 400 days (those are warnings; the others fail the build).
3. **Import from personal_finance** (the same catalog, maintained there with a research agent that re-verifies
   entries): `python3 skills/credit-card-picker/build_cards.py --import ../personal_finance/crates/card_rewards/card_catalog.json`
   merges by issuer + product + region — imported records replace ours, our own additions (e.g. the SG seed) stay.
   Then review `git diff skills/credit-card-picker/cards/`.
4. **Verify a seed row**: open the issuer's page, fill `rules` / `caps` / `rule_notes` from it, set `source`,
   `verified_at` and `verified: true`, rebuild.
5. **Test**: `cargo test -p omuse-runtime load_skill` (the reference-file reader) and the Docker suite
   `OMUSE_E2E_ENV_FILE=~/.config/omuse/e2e.env tests/run_in_docker.sh cards_real cards_markets_real` (the cards from
   the Vault, the table's rates and date, no browser).
6. The binary embeds `skills/`, so a catalog change ships with the next build; users who imported a copy of the skill
   from GitHub get it on re-import.

### What the agent does with it

It reads the user's cards from the Vault (`vault_list`, the `card` items by name — never numbers; a card the user
names that is not there is added with `vault_add_card`), loads the market's table, matches by issuer + product (aliases, tiers kept apart),
computes the value per card with `calculate`, and answers with the maths, the caveats and the verification date. The
date is guaranteed in code: when a run read a catalog table and the answer gives neither its month nor a date in it,
`guards::catalog_date_fact` gives "Card terms from the catalog as of August 2026; terms change." (a fact for the review). A card missing from the catalog is looked up on the
issuer's page and reported as "can be added to the catalog".

## Web UI navigation

`web/app.js` `VIEWS` lists every view the router knows; `NAV` is what the sidebar shows, in four named sections:
**Bots** (Chat, shown as the bots, Main first — see "Bots") · **Tools** (Browser, Connections, Memory) · **Trust**
(Status — the page was called Trust before 0.5.3 —, Audit) · **Config** (Skills, Settings). A bot's menu icon is a
robot head with a badge for what it does (`botIcon`, the bot file's `badge`), so the bots never look like the tools,
which keep their emoji.
Two views are routable but unlisted:

- **Tasks** (`#tasks`, `#tasks/<id>`): the task list and detail. Audit's task links, the chat's task cards and the
  Status page still open it; the waiting-approval badge that used to sit on the tab now sits on Chat, where approvals
  and takeovers are answered.
- **Automations** (`#schedules`): the full page of schedules, goals and event triggers (Run now, Check now, Results,
  Pause). It is out of the navigation; `schedule_create`, `goal_create` and the event triggers work from the chat and
  the skills (order tracking, the morning brief, the arrival-card reminders), and a finished schedule shows its toast.

**Tasks** (`#timers`, in Tools) is the simple page of the user's scheduled tasks (`viewTimers`): for each, when it
runs next — **Pending** when that time has passed and it has not run (the machine slept or was suspended) — what it
does, a link to its chat and Delete (the chat is kept). A schedule's chat is its own (`conv_<schedule id>`, kind
`schedule`, listed under "Tasks" in the chat list), not the chat it was asked in; a run whose chat the user deleted
makes it again under the same id (`Scheduler::ensure_conv`; a goal's final check does the same), so the link keeps
working.

**`/run_pending_tasks`** (Sentinel, behind the login gate: `curl -N -u user:password https://…/run_pending_tasks`;
the runtime's `GET /api/run_pending_tasks`) runs every pending task now and streams NDJSON: `pending` (what is due),
`started` / `skipped` per task, `finished` per run (status, answer), a `waiting` heartbeat every 15 s, and `done`, then
closes. A run that stops for the user (an approval, a takeover) counts as finished. The background loop (every 20 s)
and this URL claim a due task the same way (`Scheduler::fire`: `next_run` moved on by compare-and-set), so a task never
runs twice; when the loop got there first, the stream follows that run. `[server] scheduler_loop` (default true) stays
on in production, also where the server is often suspended: the loop runs what is due within 20 s of waking, so a
missed wake-up hook loses nothing, and the URL is safe alongside it. Off, the loop keeps goal deadlines and the memory
tidy but starts no task (tests/run_pending_ui.py: a task put in the database an hour overdue shows Pending, stays so
past the loop's 20 s, and /run_pending_tasks runs it). Firing a task publishes `schedule_update`, so an open Tasks page
drops Pending at once.

The Skills page lists the skills under each bot, in the menu's order (`GET /api/skills` gives each skill's `bots`:
the bots that may use it, so a newly imported skill shows under Main); a shared skill shows under each of its bots,
and its checkboxes move together.

`tests/nav_ui.py` checks the order, the sections, the bot icons, the Skills page's groups and that `#tasks/<id>` still
renders.

## Bots (`bots/*.md`, `crates/omuse-runtime/src/bots.rs`)

The chat is split into bots. A bot is a set of skills and a short prompt of its own; every bot shares the memory, the
profile, the Vault, the connectors and the approval rules. The bots are built in (`bots/<id>.md`, compiled into the
binary like the skills): front matter with `name`, `name_zh`, `icon` (an emoji, for text), `badge` (the menu icon's
badge: `chat`, `plane`, `dollar`, `card`, `camera`), `order` (the menu), `aliases` (ids of earlier bots merged into
this one: `get` resolves them, and the store moves their conversations to this bot when it opens), `description` /
`description_zh` (what the main bot routes by), `skills` (comma list) and `tools` (comma list: tools only the bots
that name them may use — the travel searches `flights_search`, `flight_offer`, `hotels_search`, `hotel_rooms`; a
connector's `mcp_<server>__<name>` counts as `<name>`) and `examples` / `examples_zh` (3–5 requests it handles, `|`
between them: shown as buttons above the chat input of the bot's empty conversation; a click fills the input); the body
is the bot's prompt, added to the executor prompt.

- **Main** (`main`) is the entrance: it has every skill no specialised bot lists (imported skills included) plus the
  ones its own file names (`pdf-forms`). The specialised bots: Travel (`travel`: bookings, city guides, arrival cards
  and entry rules — 0.5.3 merged City guides `cities` and Arrival & entry `arrival` into it), Insurance & claims
  (`insurance`), Credit cards (`cards`), Photos & videos (`photos`). Bots may share skills (`pdf-forms`,
  `credit-card-picker`, `csr-trip-claim`).
- **A conversation belongs to one bot**: `conversations.bot_id` (`main` for older ones) and `parent_conv` (the main
  conversation that handed it over). `GET /api/bots`, `GET /api/conversations?bot=<id>`, `POST /api/chat` with `bot`
  for a new conversation. The executor's skill list and `load_skill` are the bot's; another bot's skill is refused
  with the bot that has it. The same for tools: another bot's tool is not offered, and a call to it is refused in
  code (2026-10-29: with the real model, Main answered a flight question with `flights_search` itself).
- **Routing** (`Runtime::route`, agent/bots.rs): every new request in Main first goes to the **planner model**
  (Settings → planner model; the default model when none is set) with the bots' descriptions and the recent
  conversation. It answers `{"bot", "question", "conversation_id"}`: the bot, the request rewritten to stand alone
  (the user's words plus what the bot needs from the conversation, no condition the user did not give) and, for a
  follow-up, the bot conversation to continue. The code checks it (`checked_route`: a known bot; the conversation
  only when it is that bot's and was started from this one; an empty question becomes the request) and hands over at
  once — a `route` event, then an `ask_bot` tool call marked `routed`. `main`: the executor answers without
  `ask_bot`. No usable answer (`route` with `bot: null`, or `route_error`): the executor gets `ask_bot` and decides.
  Not for scheduled runs, goal checks or a task resumed after an approval.
- **Hand-over** (`ask_bot(bot, question, conversation_id?)`, main bot only, alone in its turn): a new conversation in
  that bot (or the one given, for a follow-up) gets the question; the main task waits in `WAITING_BOT` (the bot's task
  starts only once the main task is suspended); when the bot's task ends, the main task ends at once with the bot's
  answer word for word, stored with `meta.handoff` (the UI's "Continue in …" link). The history shows the main model
  `[answered by the <bot> bot in conversation <id>]`, so a follow-up goes back to the same conversation. An approval
  or a takeover the bot asks for is also posted in the main conversation; cancelling the main task cancels the bot's.
- **UI**: the menu lists the bots in place of Chat, Main first; Main is `#chat/<conv>`, another bot
  `#bot/<id>/<conv>`. While Main waits for a bot (`WAITING_BOT`), Main's reply bubble stays: the dancing dots and a
  verb that moves with the bot's model and tool calls, then the bot's answer as it streams (`relaying`, `relayOf`:
  the bot task's `answer_delta` events are shown in Main's bubble too). It goes while the bot waits for the user.
- Tests: `tests/bots_e2e.py`, `tests/bots_ui.py`; suites of one bot's skills set `K.BOT` (`tests/skills_common.py`) or
  use `travel_common.BOT`.

## Skill front matter

Every `skills/<name>/SKILL.md` starts with a YAML block. `front_matter` (`skills.rs`) reads these keys and ignores
the rest:

| Key | Values | Effect |
|---|---|---|
| `name` | the folder name | the skill's id for `load_skill` |
| `description` | bilingual one-liner | shown to the executor to decide when to load the skill |
| `browser` | `off` | no browser and no sub-agents for the rest of the task (the city guides; below) |
| `plan` | `on` | the planner runs once when the skill is loaded, with the sheet as its context, and the plan is appended to the tool result |
| `form` | `on` | a form-filling skill: its step budget is at least 70 (`guards::FORM_STEPS`) |
| `form_kind` | `arrival` / `claim` | which form guards apply (defaults, hand-over, the submit refusal) |
| `steps` | a number | the skill's own step budget for the task, when above the setting's |
| `profile` | field names (`nationality, birthday`) | those profile values are appended to the load_skill result in code, or "not in the profile — ask once" (`local::skill_profile_note`; entry-requirements, eu-etias, uk-eta) |

**Default: no plan.** Most skills are a procedure of a few tool calls (search flights, fill one form, draft one
letter): the sheet already says what to call and in what order, so the executor follows it directly and the task
never shows `PLANNING`; a planner call would only add a model round trip and a second document to follow. The
executor can still call `plan_task` itself when a request turns out to need one.

`plan: on` is for the skills where the plan buys something — parallel groups run as sub-agents, visible progress over
a long run with approvals in between, or a report compiled from many sources. After the review of October 2026 these
seven carry it: `csr-trip-claim`, `travel-expense-report`, `travel-planning`, `deep-research`,
`web-research-compare`, `drive-cleanup`, `drive-photo-storybook`. The other 53 (the bookings, the arrival cards, the
disruption / baggage / emergency helpers, the city guides, mail, shopping, phone and PDF skills) run from their
sheets. A unit test (`built_in_skills_spell_the_plan_flag_as_on_and_only_the_long_ones_carry_it`) keeps the list in
step with the files; `tests/kiwi_flights_e2e.py` checks that a flight search makes no plan and
the real `cards_claim_real.py` covers the claim. Imported skills use the same keys.

## City guides (`skills/<city>/SKILL.md`)

Fifteen briefings for Westerners: `beijing`, `shanghai`, `shenzhen`, `guangzhou`, `chengdu`, `tokyo`, `osaka`, `seoul`,
`singapore`, `hong-kong`, `taipei`, `bangkok`, `kuala-lumpur`, `ho-chi-minh-city`, `hanoi`. Each guide is itself the
curated data: facts researched from official sources and reputable press, each volatile one dated ("checked October
2026"). The agent answers from the sheet and from data tools (`market_data` for exchange rates, the travel connector for
flights, hotels and prices, mail and calendar for the user's bookings), **never the browser**; for a rule that decides
whether the user can enter, it hands off to `entry-requirements`, and for tasks to the other travel skills.

The rule is enforced, not only written: the guides carry `browser: off` in their front matter. When `load_skill`
loads such a skill, the runtime records it for the task (`Inner::no_browser`): the browser tools and `delegate` are
no longer offered to the executor or its sub-agents, a call made anyway is refused and audited (`skill_policy`,
`browser_off`), and loading a skill without the flag (a booking skill, say) lifts it for the rest of the task. Any
skill can use the flag.

### Shape
Frontmatter (`name` = folder, a bilingual `description` ending "use for any question about visiting, doing business in
or moving to <City>"), then: How to use this skill (the **No browser** rule and the hand-off line naming the country's
arrival-card skill) · Orientation · Entering … and the airports · Money · Getting online and the apps · Getting around ·
Business scene and customs · Living here · Food · What to see · Timeline · Sources to verify against · Remember what you
learn. 14–20 KB each (CJK, Thai and Vietnamese text counts 2–3 bytes per character); `tests/city_guides_e2e.py` checks
the shape, the hand-offs, the absence of browser tools, the dates and the size, and that a scripted answer uses only
the guide and `market_data`.

### Refreshing a guide
1. Work through the facts most likely to change first: visa-free lists and durations, arrival cards and entry fees,
   airport rail fares, new metro lines and airports, payment rules for foreign cards, tax refund rules, rents and
   school fees, population and GDP (official statistics bulletins).
2. Check each against the official sources listed at the end of the guide; replace the fact and its date ("checked
   <month year>"). A fact that cannot be confirmed is removed or marked "(verify)" — never estimated.
3. Keep China-wide (or country-wide) facts identical across the guides of the same country (e.g. the 30-day visa-free
   list and the 240-hour transit in all five Chinese guides; the RTS Link date in `singapore` and `kuala-lumpur`).
4. Run `bash tests/run_in_docker.sh city_guides_e2e` (rebuild the test image first: skills are compiled in).

## FWD travel insurance (`skills/fwd-travel-insurance/`)

FWD has no public API, developer portal or MCP server (checked October 2026), so the skill works through FWD's own
websites: the user signs in, pays and signs; OMuse prepares everything else. Partner sales of FWD (and Klook's FWD
cover) run through bolttech's single partner API, which needs a commercial agreement and, in Singapore, an insurance
intermediary licence: that is the route if buying should ever happen inside OMuse.

- **Singapore**: FWD Singapore Pte. Ltd. is the insurer. Buying: the public quote page
  (`fwd.com.sg/travel-insurance/online-quote/`), then the application with the passport and card from the Vault after
  `purchase_confirm`, handed over for Pay. Claiming: the travel claim form (`fwd.com.sg/claim-online/travel.html`) or the
  customer portal, filled and handed over for Submit; FWD asks to be told within 30 days. FWD's robots.txt keeps
  crawlers off the quote pages; a browser acting for a signed-in user is not a crawler, but the pages may change.
- **Hong Kong**: the three travel plans on fwd.com.hk (MyTravel, TravelCare Plus, GBA Easy Travel) are underwritten by
  **bolttech Insurance (Hong Kong)**; FWD Financial sells them as bolttech's appointed agency. Only HKID holders in Hong
  Kong whose trip starts there can buy. Claims go to bolttech: its fillable PDF form (every field carries a tooltip
  label, so `pdf_form_fields` can map it) filled and attached, with the documents, to an email draft to
  `claims.hk@bolttechinsurance.com`; the HKID, bank account and signature stay for the user.

**PDF links in the browser.** Chromium shows a PDF in its viewer, which neither the snapshot nor the downloads list can
use. Since this skill, `browser_navigate` notices a main document of type `application/pdf`, fetches it again with the
page's cookies and keeps it as `downloads/<name>.pdf`; the result carries `pdf: {path, size}` and the file appears in
`browser_downloads`. `files_read` and `pdf_form_fields` / `pdf_form_fill` then work on it (any PDF form on the web, not
only this one).

`tests/insurance_real.py` and `tests/insurance_hk_real.py` run the three flows with a real model on stand-ins at the
official addresses (`tests/pages/fwd_sg_*.html`, `fwd_hk.html`, a field-for-field copy of bolttech's form) and mail with
PDF attachments (`skills_common.seed_mail` takes attachments).
