//! Known forms, filled by code: one function per form URL (the user's rule, 2026-10-31: "Create a special function
//! to do this in the code for each URL"). The model only extracts the trip facts that need understanding — the flight,
//! the dates, where the user stays and why they travel ([`Facts`], the arguments of `form_fill`); the form's function
//! decides every field, value, format, option and default from those facts, the user's profile and the Vault, and
//! returns a plan of operations ([`Op`]) that Sentinel runs through the browser in one call (`browser_fill_form`).
//! The functions are written against the real sites' forms (mapped from the live pages); a URL with no function, or a
//! plan step that does not find its field, leaves the rest to the generic path (the model, with the essential guards).

use chrono::{Duration, NaiveDate};
use serde_json::{json, Value};

use omuse_common::util::{get_str, py_str};

/// The trip facts the model reads from the request, the mail or the e-ticket (`form_fill`'s arguments). Everything
/// else a form needs comes from the profile, the Vault or a default rule.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Facts {
    /// "SQ 231"
    pub flight_no: String,
    /// YYYY-MM-DD
    pub arrival_date: String,
    /// the city the flight comes from ("Sydney")
    pub origin_city: String,
    /// the country it comes from ("Australia")
    pub origin_country: String,
    /// YYYY-MM-DD, when a return / onward flight, a check-out or the user gives one
    pub departure_date: String,
    /// the next city after this country, when an onward flight or the user gives one
    pub next_city: String,
    /// the stay's name ("Riverside Hotel") — empty for the user's own home
    pub stay_name: String,
    /// the stay's address
    pub stay_address: String,
    /// hotel | own | friends | other
    pub stay_kind: String,
    /// holiday | business | family | study | transit
    pub purpose: String,
    /// the arrival airport's IATA code ("DPS"), from the e-ticket
    pub arrival_airport: String,
    /// the airline's name ("Garuda Indonesia"), from the e-ticket
    pub airline: String,
    /// every argument of the call as given: the claim forms' facts (`agent::claims`: the policy number, the loss
    /// date, the amount, the benefit, …) are read from here by key ([`Inputs::arg`])
    pub extra: Value,
}

impl Facts {
    pub fn from_args(a: &Value) -> Self {
        let g = |k: &str| get_str(a, k).trim().to_string();
        Facts {
            extra: if a.is_object() { a.clone() } else { json!({}) },
            flight_no: g("flight_no"),
            arrival_date: g("arrival_date"),
            origin_city: g("origin_city"),
            origin_country: g("origin_country"),
            departure_date: g("departure_date"),
            next_city: g("next_city"),
            stay_name: g("stay_name"),
            stay_address: g("stay_address"),
            stay_kind: g("stay_kind").to_lowercase(),
            purpose: g("purpose").to_lowercase(),
            arrival_airport: g("arrival_airport").to_uppercase(),
            airline: g("airline"),
        }
    }
}

/// What a form function reads: the facts, the profile and the defaults computed from them.
pub struct Inputs<'a> {
    pub facts: &'a Facts,
    pub profile: &'a Value,
}

impl Inputs<'_> {
    fn p(&self, k: &str) -> String {
        self.profile
            .get(k)
            .map(py_str)
            .unwrap_or_default()
            .trim()
            .to_string()
    }
    /// The name as on the passport, upper case.
    pub fn full_name(&self) -> String {
        self.p("name_en").to_uppercase()
    }
    /// (given names, family name) of the passport name: the profile writes it given names first.
    pub fn name_parts(&self) -> (String, String) {
        let full = self.full_name();
        match full.rsplit_once(' ') {
            Some((given, family)) => (given.trim().to_string(), family.trim().to_string()),
            None => (String::new(), full),
        }
    }
    /// F / M / X from the profile's gender.
    pub fn sex_code(&self) -> String {
        match self.p("gender").to_lowercase().as_str() {
            "female" | "f" | "女" => "F",
            "male" | "m" | "男" => "M",
            "" => "",
            _ => "X",
        }
        .to_string()
    }
    pub fn birthday(&self) -> Option<NaiveDate> {
        NaiveDate::parse_from_str(&self.p("birthday"), "%Y-%m-%d").ok()
    }
    pub fn nationality(&self) -> String {
        self.p("nationality")
    }
    pub fn email(&self) -> String {
        let e = self.p("email_personal");
        if e.is_empty() {
            self.p("email")
        } else {
            e
        }
    }
    pub fn phone(&self) -> String {
        self.p("phone")
    }
    pub fn home_address(&self) -> String {
        self.p("address_home")
    }
    /// The occupation: the one the user gave (the call's `occupation`), else the profile's job title.
    pub fn job_title(&self) -> String {
        let given = self.arg("occupation");
        if given.is_empty() {
            self.p("job_title")
        } else {
            given
        }
    }
    /// The country of birth as the profile gives it (`birthplace`, a country name); empty when it does not — never
    /// guessed from the nationality.
    pub fn birthplace(&self) -> String {
        self.p("birthplace")
    }
    /// The user's home city and its country, by name ("Sydney", "Australia"), from the profile's structured
    /// `home_city` / `home_country` codes (`crate::places`) — never read out of the address text; `None` when unknown.
    pub fn home_city(&self) -> Option<(String, String)> {
        let city = crate::places::home_city(self.profile)?;
        let country = crate::places::country(&crate::places::home_country(self.profile))
            .or_else(|| crate::places::country(city.country))?;
        Some((city.name.to_string(), country.name.to_string()))
    }
    /// The country the user lives in: the one the home address ends with ("…, Australia") when the table knows it,
    /// else the nationality.
    pub fn residence_country(&self) -> String {
        let last = self
            .home_address()
            .rsplit(',')
            .next()
            .unwrap_or("")
            .trim()
            .to_string();
        if iso3(&last).is_empty() {
            self.nationality()
        } else {
            last
        }
    }
    /// ISO alpha-3 of the country the home address ends with ("…, Australia" → AUS), else of the nationality.
    pub fn residence_iso3(&self) -> String {
        let from_home = iso3(self.home_address().rsplit(',').next().unwrap_or(""));
        if from_home.is_empty() {
            iso3(&self.nationality())
        } else {
            from_home
        }
    }
    pub fn arrival(&self) -> Option<NaiveDate> {
        NaiveDate::parse_from_str(&self.facts.arrival_date, "%Y-%m-%d").ok()
    }
    /// The user's default: the date given, else arrival + 7 days.
    pub fn departure(&self) -> Option<NaiveDate> {
        NaiveDate::parse_from_str(&self.facts.departure_date, "%Y-%m-%d")
            .ok()
            .or_else(|| self.arrival().map(|a| a + Duration::days(7)))
    }
    /// The user's default: the city given, else the city the flight came from.
    pub fn next_city(&self) -> String {
        if self.facts.next_city.is_empty() {
            self.facts.origin_city.clone()
        } else {
            self.facts.next_city.clone()
        }
    }
    /// A fact given by key in the call's arguments (the claim forms' facts), trimmed; empty when missing.
    pub fn arg(&self, k: &str) -> String {
        get_str(&self.facts.extra, k).trim().to_string()
    }
    /// The stay as one line: "<name>, <address>", or the address alone.
    pub fn stay(&self) -> String {
        [
            self.facts.stay_name.as_str(),
            self.facts.stay_address.as_str(),
        ]
        .into_iter()
        .filter(|x| !x.is_empty())
        .collect::<Vec<_>>()
        .join(", ")
    }
}

/// One operation of a fill plan, run in order by the broker (`fill_form`; the ops are described at `FILL_ONE_JS`
/// in omuse-browser): built with the constructors below, sent as JSON. `skip` marks an operation with nothing to put in
/// (the value is unknown): it is left out of the plan and the field stays for the user.
#[derive(Debug, Clone, PartialEq)]
pub struct Op {
    pub field: &'static str,
    pub json: Value,
    pub skip: bool,
}

impl Op {
    fn new(field: &'static str, json: Value, value: &str) -> Self {
        Op {
            field,
            json,
            skip: value.trim().is_empty(),
        }
    }
    /// The operation as the broker takes it; `None` when skipped.
    pub fn to_json(&self) -> Option<Value> {
        if self.skip {
            return None;
        }
        let mut j = self.json.clone();
        j["field"] = json!(self.field);
        Some(j)
    }
}

/// Type `value` into the input at `sel` (set through the native setter).
pub fn text(field: &'static str, sel: &str, value: &str) -> Op {
    Op::new(
        field,
        json!({"op": "text", "sel": sel, "value": value}),
        value,
    )
}
/// Type `value` key by key into `sel` inside the section headed `scope` (split date boxes, masked inputs).
pub fn keys(field: &'static str, scope: &str, sel: &str, value: &str) -> Op {
    Op::new(
        field,
        json!({"op": "keys", "scope": scope, "sel": sel, "value": value}),
        value,
    )
}
/// A lookup that only takes a listed value: type `typed` into `sel`, then click the option whose text is (starts with,
/// contains) `pick`.
pub fn pick(field: &'static str, sel: &str, typed: &str, pick: &str) -> Op {
    Op::new(
        field,
        json!({"op": "pick", "sel": sel, "type": typed, "value": pick}),
        pick,
    )
}
/// Pick the option of a native select by value or visible text.
pub fn select(field: &'static str, sel: &str, value: &str) -> Op {
    Op::new(
        field,
        json!({"op": "select", "sel": sel, "value": value}),
        value,
    )
}
/// Check the radio of group `name` (an input name) whose value or label is `value`.
pub fn radio(field: &'static str, name: &str, value: &str) -> Op {
    Op::new(
        field,
        json!({"op": "radio", "sel": name, "value": value}),
        value,
    )
}
/// Click the choice whose own text is `text` inside the section headed `scope` (radios with generated ids).
pub fn choose(field: &'static str, scope: &str, text: &str) -> Op {
    Op::new(
        field,
        json!({"op": "click", "scope": scope, "text": text}),
        text,
    )
}
/// Tick the checkbox whose text is `text` (inside `scope` when given).
pub fn tick(field: &'static str, scope: &str, text: &str) -> Op {
    Op::new(
        field,
        json!({"op": "check", "scope": scope, "text": text}),
        text,
    )
}
/// Press the button whose own text is `text` (a step's Next).
pub fn button(field: &'static str, text: &str) -> Op {
    Op::new(
        field,
        json!({"op": "button", "sel": "button", "text": text}),
        text,
    )
}
/// Wait `ms` milliseconds (a step's fields appear).
pub fn wait(ms: u64) -> Op {
    Op::new("wait", json!({"op": "wait", "ms": ms}), "x")
}
/// Set a read-only date-picker box at `sel` to `value` (the page's own datepicker when it has one).
pub fn date_box(field: &'static str, sel: &str, value: &str) -> Op {
    Op::new(
        field,
        json!({"op": "date", "sel": sel, "value": value}),
        value,
    )
}
/// The Vault ID document's `vault_field` put in with operation `op` (text / date) at `sel`, dates as `date_fmt`.
pub fn vault_as(
    field: &'static str,
    op: &str,
    sel: &str,
    vault_field: &'static str,
    date_fmt: &'static str,
) -> Op {
    Op::new(
        field,
        json!({"op": op, "sel": sel, "vault": {"field": vault_field, "date_fmt": date_fmt}}),
        "x",
    )
}
/// The Vault ID document's `vault_field` (number / expiry), typed key by key into `sel` inside `scope`, dates as
/// `date_fmt` (Sentinel puts the value in; the plan never carries it).
pub fn vault(
    field: &'static str,
    scope: &str,
    sel: &str,
    vault_field: &'static str,
    date_fmt: &'static str,
) -> Op {
    Op::new(
        field,
        json!({"op": "keys", "scope": scope, "sel": sel,
                          "vault": {"field": vault_field, "date_fmt": date_fmt}}),
        "x",
    )
}

#[path = "forms_claims.rs"]
mod claims_pages;
pub use claims_pages::fwd_region;

/// A named script a form function runs in the page (`op: js`): a function `async (sel, nth, args) => {ok, value?,
/// why?}` that works a framework's control directly — open an Angular Material select and click its option, then
/// report the value it now shows — instead of a chain of simulated clicks. Code-only: the scripts are the constants of
/// this file (a table per site), the plan carries the script with its name, and the runtime refuses a `js` op in any
/// call the model wrote (docs/json-tools.md, "Form functions").
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Script {
    pub name: &'static str,
    pub src: &'static str,
}

/// Run `script` on the `nth` visible match of `sel` with `value` (and `args`); skipped when the value is empty.
pub fn js(field: &'static str, script: &Script, sel: &str, nth: usize, value: &str) -> Op {
    Op::new(
        field,
        json!({"op": "js", "script": script.name, "src": script.src, "sel": sel, "nth": nth, "value": value}),
        value,
    )
}

/// A named script with arguments besides the value (`args`: what to type into a list's search box, …).
pub fn js_with(
    field: &'static str,
    script: &Script,
    sel: &str,
    nth: usize,
    value: &str,
    args: Value,
) -> Op {
    Op::new(
        field,
        json!({"op": "js", "script": script.name, "src": script.src, "sel": sel, "nth": nth, "value": value,
               "args": args}),
        value,
    )
}

/// The same with the Vault ID document's `vault_field` (a date part as `date_fmt`) as the value: Sentinel puts it in.
pub fn js_vault(
    field: &'static str,
    script: &Script,
    sel: &str,
    nth: usize,
    vault_field: &'static str,
    date_fmt: &'static str,
) -> Op {
    Op::new(
        field,
        json!({"op": "js", "script": script.name, "src": script.src, "sel": sel, "nth": nth,
               "vault": {"field": vault_field, "date_fmt": date_fmt}}),
        "x",
    )
}

/// The scripts of All Indonesia (allindonesia.imigrasi.go.id, React with custom choice rows).
pub const ID_SCRIPTS: &[Script] = &[CHOICE_ROW, SCROLL_LIST, SEARCH_LIST];

/// A read-only box that opens a list with its own search box (All Indonesia's countries, ports, airlines, purposes):
/// open it, type `args.typed` into the search box that belongs to this list (several lists keep theirs on the page),
/// click the row whose text is the value (exactly, else starting with it), and check the box now holds it.
pub const SEARCH_LIST: Script = Script {
    name: "search_list",
    src: r#"async (sel, nth, args) => {
  const sleep = ms => new Promise(r => setTimeout(r, ms));
  const norm = s => (s || '').replace(/\s+/g, ' ').trim().toLowerCase();
  const vis = e => { const r = e.getBoundingClientRect(); return r.width > 0 && r.height > 0; };
  const all = [...document.querySelectorAll(sel)];
  const box = all.filter(vis)[nth || 0] || all[nth || 0];
  if (!box) return {ok: false, why: 'not on the page'};
  const want = norm(args.value);
  const shown = () => norm(box.value || box.innerText);
  if (shown() === want) return {ok: true, value: shown()};
  box.scrollIntoView({block: 'center'});
  box.click();
  await sleep(500);
  // this list's search box: the visible one nearest the box (up its ancestors)
  let search = null;
  for (let a = box.parentElement, k = 0; a && k < 8 && !search; a = a.parentElement, k++) {
    search = [...a.querySelectorAll('input[placeholder="Search"]')].filter(vis)[0] || null;
  }
  if (search && args.typed) {
    const set = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set;
    search.focus(); set.call(search, args.typed);
    search.dispatchEvent(new Event('input', {bubbles: true}));
    await sleep(700);
  }
  // the rows near the search box first (its list), else anywhere on the page
  const near = search ? search.parentElement.parentElement.parentElement : document;
  const rows = () => [...near.querySelectorAll('li, div, span, p')]
    .filter(e => vis(e) && e.children.length === 0 && e !== box && e.innerText);
  let r = rows().find(e => norm(e.innerText) === want) || rows().find(e => norm(e.innerText).startsWith(want));
  if (!r) r = [...document.querySelectorAll('li, div, span, p')].filter(e => vis(e) && e.children.length === 0)
    .find(e => norm(e.innerText) === want) || null;
  if (!r) { box.click(); return {ok: false, why: 'no such row'}; }
  r.scrollIntoView({block: 'center'});
  r.click();
  await sleep(400);
  const now = shown();
  return {ok: now.startsWith(want) || now === want, value: now, why: now.startsWith(want) ? null : 'shows ' + now};
}"#,
};

/// A read-only box that opens a scrolling list rendering only the rows in view (All Indonesia's arrival day: "04
/// NOVEMBER 2026"): open it, scroll the list until a row whose own text is the value shows, click it, and check the
/// box now holds it.
pub const SCROLL_LIST: Script = Script {
    name: "scroll_list",
    src: r#"async (sel, nth, args) => {
  const sleep = ms => new Promise(r => setTimeout(r, ms));
  const norm = s => (s || '').replace(/\s+/g, ' ').trim().toLowerCase();
  const vis = e => { const r = e.getBoundingClientRect(); return r.width > 0 && r.height > 0; };
  const all = [...document.querySelectorAll(sel)];
  const box = all.filter(vis)[nth || 0] || all[nth || 0];
  if (!box) return {ok: false, why: 'not on the page'};
  const want = norm(args.value);
  const shown = () => norm(box.value || box.innerText);
  if (shown() === want) return {ok: true, value: shown()};
  box.scrollIntoView({block: 'center'});
  box.click();
  await sleep(400);
  const row = () => [...document.querySelectorAll('li, div, span, p')]
    .filter(e => vis(e) && e.children.length === 0 && norm(e.innerText) === want)[0];
  const scroller = () => [...document.querySelectorAll('div, ul')].filter(e => vis(e) && e.scrollHeight > e.clientHeight + 4
    && /(auto|scroll)/.test(getComputedStyle(e).overflowY) && e !== document.scrollingElement && e.innerText.length < 4000)[0];
  for (let i = 0; i < 60; i++) {
    const r = row();
    if (r) { r.scrollIntoView({block: 'center'}); r.click(); await sleep(400); break; }
    const sc = scroller();
    if (!sc) break;
    sc.scrollTop += 60;
    await sleep(120);
  }
  const now = shown();
  return {ok: now === want, value: now, why: now === want ? null : 'shows ' + now};
}"#,
};

/// A row of choices built as a read-only `<input value="FEMALE">` inside a clickable row (All Indonesia's gender and
/// Yes / No questions): click the row whose input holds the value — the click handler is the row's, not the input's.
pub const CHOICE_ROW: Script = Script {
    name: "choice_row",
    src: r#"async (sel, nth, args) => {
  const norm = s => (s || '').replace(/\s+/g, ' ').trim().toLowerCase();
  const vis = e => { const r = e.getBoundingClientRect(); return r.width > 0 && r.height > 0; };
  const all = [...document.querySelectorAll(sel)];
  const box = all.filter(vis)[nth || 0] || all[nth || 0];
  if (!box) return {ok: false, why: 'not on the page'};
  const want = norm(args.value);
  const inp = [...box.querySelectorAll('input')].find(i => norm(i.value) === want);
  if (!inp) return {ok: false, why: 'no such choice'};
  const row = inp.parentElement || inp;
  row.scrollIntoView({block: 'center'});
  row.click();
  await new Promise(r => setTimeout(r, 300));
  return {ok: true, value: want};
}"#,
};

/// The scripts of the Cambodia e-Arrival Card (arrival.gov.kh, Angular Material).
pub const KH_SCRIPTS: &[Script] = &[MAT_SELECT];

/// An Angular Material `mat-select`: open it (its own click), click the overlay's `mat-option` whose text is the value
/// (exactly, else starting with it), wait for the panel to close (Escape when it does not), and check the value the
/// select now shows. Nothing listed: the panel is closed again and the select left as it was.
pub const MAT_SELECT: Script = Script {
    name: "mat_select",
    src: r#"async (sel, nth, args) => {
  const sleep = ms => new Promise(r => setTimeout(r, ms));
  const norm = s => (s || '').replace(/\s+/g, ' ').trim().toLowerCase();
  const vis = e => { const r = e.getBoundingClientRect(); return r.width > 0 && r.height > 0; };
  const all = [...document.querySelectorAll(sel)];
  const el = all.filter(vis)[nth || 0] || all[nth || 0];
  if (!el) return {ok: false, why: 'not on the page'};
  const want = norm(args.value);
  const shown = () => norm((el.querySelector('.mat-mdc-select-value, .mat-select-value') || el).innerText);
  const listed = () => [...document.querySelectorAll('.cdk-overlay-container mat-option')];
  const close = () => el.dispatchEvent(new KeyboardEvent('keydown', {key: 'Escape', code: 'Escape', keyCode: 27, bubbles: true}));
  if (shown() === want) return {ok: true, value: shown()};
  el.scrollIntoView({block: 'center'});
  el.click();
  let opts = [];
  for (let i = 0; i < 30 && !opts.length; i++) { await sleep(100); opts = listed(); }
  const o = opts.find(x => norm(x.innerText) === want) || opts.find(x => norm(x.innerText).startsWith(want));
  if (!o) { close(); await sleep(200); return {ok: false, why: 'no such option (' + opts.length + ' listed)'}; }
  o.scrollIntoView({block: 'center'});
  o.click();
  for (let i = 0; i < 15 && listed().length; i++) await sleep(100);
  if (listed().length) { close(); await sleep(200); }
  const now = shown();
  return {ok: now === want, value: now, why: now === want ? null : 'shows ' + now};
}"#,
};

/// A form's fill function: the whole plan, page by page, up to the user's final step.
pub type FormFn = fn(&Inputs) -> Vec<Op>;

/// The form functions by the page they fill: (name, host, path prefix, function). Matched on the page's host and
/// the start of its path.
#[rustfmt::skip]
const FORMS: &[(&str, &str, &str, FormFn)] = &[
    ("sg-arrival-card", "eservices.ica.gov.sg", "/arrivalcard/sgac", sg_arrival_card),
    ("my-arrival-card", "imigresen-online.imi.gov.my", "/mdac/main", my_arrival_card),
    ("kr-arrival-card", "www.e-arrivalcard.go.kr", "/portal", kr_arrival_card),
    ("id-arrival-card", "allindonesia.imigrasi.go.id", "/", id_arrival_card),
    ("tw-arrival-card", "twac.immigration.gov.tw", "/", tw_arrival_card),
    ("cn-arrival-card", "s.nia.gov.cn", "/arrivalcardfillingpc", cn_arrival_card),
    ("th-arrival-card", "tdac.immigration.go.th", "/", th_arrival_card),
    ("jp-visit-japan-web", "www.vjw.digital.go.jp", "/main", jp_visit_japan_web),
    ("kh-arrival-card", "arrival.gov.kh", "/", kh_arrival_card),
    ("ph-etravel", "etravel.gov.ph", "/", ph_etravel),
    ("uk-eta", "apply-for-an-eta.homeoffice.gov.uk", "/", uk_eta),
    // the claims, the purchase, the delayed-bag lookup and the Hong Kong departure tax refund, driven by
    // `agent::claims` / `agent::bagtrace` / `agent::apdt` (forms_claims.rs: the real sites, mapped 2026-11-02)
    ("csr-trip-claim", "cardclaimcenter.com", "/internal/log-in", claims_pages::cardclaim_login),
    ("fwd-sg-claim", "www.fwd.com.sg", "/claim-online/travel", claims_pages::fwd_sg_claim),
    ("fwd-sg-quote", "www.fwd.com.sg", "/travel-insurance/online-quote", claims_pages::fwd_sg_quote),
    ("worldtracer-manage", "mybag.aero", "/baggage/", claims_pages::mybag_manage),
    ("hk-apdt-start", "app.apdtrefund.hk", "/hktaxmobile/", claims_pages::apdt_start),
];

/// The function for the page at `url`, with its name: `None` for a page no function knows (the generic path).
pub fn form_for(url: &str) -> Option<(&'static str, FormFn)> {
    let u = url::Url::parse(url).ok()?;
    let host = u.host_str()?.to_lowercase();
    let path = u.path().to_lowercase();
    FORMS
        .iter()
        .find(|(_, h, p, _)| host == *h && path.starts_with(p))
        .map(|(n, _, _, f)| (*n, *f))
}

/// The nationality adjective lists like ICA's use ("AUSTRALIAN" for Australia); empty when the country is not one
/// this table knows (the field is then left for the user — never a guess).
pub fn demonym(country: &str) -> String {
    const T: &[(&str, &str)] = &[
        ("australia", "AUSTRALIAN"),
        ("united kingdom", "BRITISH"),
        ("uk", "BRITISH"),
        ("united states", "AMERICAN"),
        ("usa", "AMERICAN"),
        ("canada", "CANADIAN"),
        ("new zealand", "NEW ZEALANDER"),
        ("china", "CHINESE"),
        ("hong kong", "HONG KONGER"),
        ("taiwan", "TAIWANESE"),
        ("japan", "JAPANESE"),
        ("south korea", "KOREAN, SOUTH"),
        ("korea", "KOREAN, SOUTH"),
        ("india", "INDIAN"),
        ("malaysia", "MALAYSIAN"),
        ("indonesia", "INDONESIAN"),
        ("thailand", "THAI"),
        ("philippines", "FILIPINO"),
        ("vietnam", "VIETNAMESE"),
        ("germany", "GERMAN"),
        ("france", "FRENCH"),
        ("italy", "ITALIAN"),
        ("spain", "SPANISH"),
        ("netherlands", "DUTCH"),
        ("ireland", "IRISH"),
        ("switzerland", "SWISS"),
        ("brazil", "BRAZILIAN"),
        ("singapore", "SINGAPORE CITIZEN"),
    ];
    let c = country.trim().to_lowercase();
    T.iter()
        .find(|(k, _)| *k == c)
        .map(|(_, v)| v.to_string())
        .unwrap_or_default()
}

/// (country calling code, the rest as digits) of a phone number written with its "+code"; ("", "") otherwise.
pub fn phone_parts(phone: &str) -> (String, String) {
    const CODES: &[&str] = &[
        "1", "44", "61", "64", "65", "60", "62", "63", "66", "81", "82", "84", "86", "852", "853",
        "886", "91",
    ];
    let p = phone.trim();
    let Some(rest) = p.strip_prefix('+') else {
        return (String::new(), String::new());
    };
    let digits: String = rest.chars().filter(char::is_ascii_digit).collect();
    let mut codes: Vec<&&str> = CODES.iter().collect();
    codes.sort_by_key(|c| std::cmp::Reverse(c.len()));
    codes
        .into_iter()
        .find(|c| digits.starts_with(**c))
        .map(|c| (c.to_string(), digits[c.len()..].to_string()))
        .unwrap_or_default()
}

/// (airline code, flight number digits) of "SQ 231" / "SQ0231" / "sq231"; ("", "") when it is not one.
pub fn flight_parts(flight: &str) -> (String, String) {
    let f: String = flight
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect::<String>()
        .to_uppercase();
    let code: String = f.chars().take(2).collect();
    let num: String = f.chars().skip(2).collect();
    if code.chars().all(|c| c.is_ascii_alphanumeric())
        && code.chars().any(|c| c.is_ascii_alphabetic())
        && !num.is_empty()
        && num.chars().all(|c| c.is_ascii_digit())
    {
        (code, num.trim_start_matches('0').to_string())
    } else {
        (String::new(), String::new())
    }
}

/// ISO 3166 alpha-3 of a country name ("Australia" → AUS); empty for one this table does not know (left for the user).
pub fn iso3(country: &str) -> String {
    const T: &[(&str, &str)] = &[
        ("australia", "AUS"),
        ("united kingdom", "GBR"),
        ("uk", "GBR"),
        ("united states", "USA"),
        ("usa", "USA"),
        ("canada", "CAN"),
        ("new zealand", "NZL"),
        ("china", "CHN"),
        ("hong kong", "HKG"),
        ("taiwan", "TWN"),
        ("japan", "JPN"),
        ("south korea", "KOR"),
        ("korea", "KOR"),
        ("india", "IND"),
        ("malaysia", "MYS"),
        ("indonesia", "IDN"),
        ("thailand", "THA"),
        ("philippines", "PHL"),
        ("vietnam", "VNM"),
        ("singapore", "SGP"),
        ("germany", "DEU"),
        ("france", "FRA"),
        ("italy", "ITA"),
        ("spain", "ESP"),
        ("netherlands", "NLD"),
        ("ireland", "IRL"),
        ("switzerland", "CHE"),
        ("brazil", "BRA"),
    ];
    let c = country.trim().to_lowercase();
    T.iter()
        .find(|(k, _)| *k == c)
        .map(|(_, v)| v.to_string())
        .unwrap_or_default()
}

/// (state, city, postcode) of a Malaysian address: the state and city names it contains ("…, 55100 Kuala Lumpur" →
/// KUALA LUMPUR, KUALA LUMPUR, 55100), the 5-digit postcode. A part it cannot find is empty.
pub fn my_address(addr: &str) -> (String, String, String) {
    const STATES: &[&str] = &[
        "KUALA LUMPUR",
        "PUTRAJAYA",
        "LABUAN",
        "JOHOR",
        "KEDAH",
        "KELANTAN",
        "MELAKA",
        "NEGERI SEMBILAN",
        "PAHANG",
        "PULAU PINANG",
        "PENANG",
        "PERAK",
        "PERLIS",
        "SELANGOR",
        "TERENGGANU",
        "SABAH",
        "SARAWAK",
    ];
    let up = addr.to_uppercase();
    let state = STATES
        .iter()
        .find(|s| up.contains(**s))
        .map(|s| if *s == "PENANG" { "PULAU PINANG" } else { s })
        .unwrap_or("");
    let postcode = addr
        .split(|c: char| !c.is_ascii_digit())
        .find(|t| t.len() == 5)
        .unwrap_or("")
        .to_string();
    // the city: the words after the postcode ("55100 Kuala Lumpur"), else the state's name for a federal territory
    let city = up
        .split_once(postcode.as_str())
        .filter(|_| !postcode.is_empty())
        .map(|(_, rest)| {
            rest.trim_matches(|c: char| c == ',' || c.is_whitespace())
                .split(',')
                .next()
                .unwrap_or("")
                .trim()
                .to_string()
        })
        .filter(|c| !c.is_empty())
        .unwrap_or_else(|| {
            if matches!(state, "KUALA LUMPUR" | "PUTRAJAYA" | "LABUAN") {
                state.to_string()
            } else {
                String::new()
            }
        });
    (state.to_string(), city, postcode)
}

/// The Thailand Digital Arrival Card (TDAC, https://tdac.immigration.go.th → `/arrival-card/#/home`), mapped from the
/// live site on 2026-11-02: the home page holds a Cloudflare Turnstile check, and its "Arrival Card" tile opens the form
/// only once the check has passed — no field is reachable before it. The check is the user's (code never solves or
/// evades it): the plan presses the tile and stops until a form field shows, so the hand-over is at the check. The form
/// behind it could not be mapped (it never opens for automation), so nothing more is filled in code: after the check
/// the user fills it in themselves (the CAPTCHA and Submit are theirs anyway).
fn th_arrival_card(_: &Inputs) -> Vec<Op> {
    vec![
        // the home page's tile (inert while the check is pending)
        choose("arrival_card", "", "Arrival Card"),
        wait(3000),
        require(
            "tdac_form",
            "input[type=text]",
            "the Cloudflare check (\"Verify you are human\") on the TDAC home page is the user's: hand the page over for it, then call form_fill again",
        ),
    ]
}

/// Visit Japan Web (https://www.vjw.digital.go.jp/main/#/vjwplo001 — the service moved there from vjw-lp), mapped
/// from the live site on 2026-11-02: it opens on the sign-in to the user's own account (`#textbox01` email,
/// `#textbox02` password). The sign-in is the user's: code puts the email in and stops while the password box shows.
/// The pages behind the sign-in (the planned entry) cannot be mapped without an account, so nothing more is filled in
/// code; the user registers the entry themselves after signing in.
fn jp_visit_japan_web(i: &Inputs) -> Vec<Op> {
    vec![
        text("email", "#textbox01", &i.email()),
        require(
            "signed_in",
            "body:not(:has(#textbox02))",
            "the sign-in to your Visit Japan Web account (password and any 2-factor code) is the user's: hand the page over for it, then call form_fill again",
        ),
    ]
}

/// The Cambodia e-Arrival Card (https://arrival.gov.kh/ → "Submit e-Arrival" → "Sign In as Guest" →
/// `/e-arrival/foreigner/individual-form/draft`), mapped from the live site on 2026-11-02: an Angular Material stepper
/// (Profile, Travel Information, Health, Customs, Confirm). Fields are `formcontrolname`s; dates are three
/// `mat-select`s (Day / Month / Year, two-digit options) — passport expiry, birth date and departure in that order on
/// the page; gender a `mat-select` (Female / Male / Other); countries a picker dialog (the visible box next to the
/// control opens a search box and a list "Australian . AUS"); the arrival date a radio per allowed day ("November 04,
/// 2026"). Best effort, the Profile step only: the passport, names, gender, birth date and contact. Its Next needs the
/// occupation, the phone's country code and the country of birth (pickers whose rows are not mapped), so the Travel,
/// Health and Customs steps after it (hidden until then) are the user's, as are the CAPTCHA and Submit. The `mat-select`s
/// are set by the named script `MAT_SELECT`.
fn kh_arrival_card(i: &Inputs) -> Vec<Op> {
    let (given, family) = i.name_parts();
    let nat3 = iso3(&i.nationality());
    let (_, mobile) = phone_parts(&i.phone());
    let dob = i.birthday();
    let sex = match i.sex_code().as_str() {
        "F" => "Female",
        "M" => "Male",
        "X" => "Other",
        _ => "",
    };
    // a mat-select (control `name`, the `nth` on the page): the named script opens it, clicks the option and checks it
    let mat = |field: &'static str, name: &str, nth: usize, value: &str| {
        [js(
            field,
            &MAT_SELECT,
            &format!("mat-select[formcontrolname={name}]"),
            nth,
            value,
        )]
    };
    // the same with the Vault ID document's date part (Sentinel puts the value in)
    let mat_vault = |field: &'static str, name: &str, nth: usize, fmt: &'static str| {
        [js_vault(
            field,
            &MAT_SELECT,
            &format!("mat-select[formcontrolname={name}]"),
            nth,
            "expiry",
            fmt,
        )]
    };
    // a country picker: open the dialog, type the code into its search box, click the row ending ". AUS"
    let picker = |field: &'static str, name: &str, code: &str| {
        let row = if code.is_empty() {
            String::new()
        } else {
            format!(". {code}")
        };
        [
            Op::new(
                field,
                json!({"op": "click", "sel": format!("input[formcontrolname={name}] + input")}),
                code,
            ),
            Op::new(
                field,
                json!({"op": "pick", "sel": "input[formcontrolname=searchControl]", "type": code,
                       "value": row, "option_sel": "a.avatar-vt-item"}),
                code,
            ),
        ]
    };
    let mut ops = vec![
        // the home page and the guest sign-in (already on the form: not there, skipped)
        choose("submit_e_arrival", "", "Submit e-Arrival"),
        wait(1500),
        choose("guest", "", "Sign In as Guest"),
        // the guest form renders in 6–8 s
        wait(9000),
        vault_as(
            "passport_number",
            "text",
            "input[formcontrolname=passportNumber]",
            "number",
            "",
        ),
    ];
    ops.extend(picker("passport_country", "nationality", &nat3));
    ops.push(wait(500));
    ops.extend(mat_vault("passport_expiry_day", "day", 0, "%d"));
    ops.extend(mat_vault("passport_expiry_month", "month", 0, "%m"));
    ops.extend(mat_vault("passport_expiry_year", "year", 0, "%Y"));
    ops.extend([
        text("surname", "input[formcontrolname=surname]", &family),
        text("given_name", "input[formcontrolname=givenName]", &given),
    ]);
    ops.extend(mat("sex", "gender", 0, sex));
    ops.extend(mat("dob_day", "day", 1, &date(dob, "%d")));
    ops.extend(mat("dob_month", "month", 1, &date(dob, "%m")));
    ops.extend(mat("dob_year", "year", 1, &date(dob, "%Y")));
    // (the country of birth's picker is left for the user: its rows are not mapped, and a picker left open blocks the
    // rest of the page)
    ops.extend([
        text("email", "input[formcontrolname=email]", &i.email()),
        text("phone", "input[formcontrolname=phoneNumber]", &mobile),
    ]);
    ops
}

/// The Philippines eTravel (https://etravel.gov.ph/ → "Click here to Sign In" → `/signin`), mapped from the live site
/// on 2026-11-02: every registration starts with the sign-in to the user's own eTravel account (a Cloudflare check,
/// the email `#email` and password `#password`, or eGovPH). The sign-in is the user's: code puts the email in and stops
/// while the password box shows. The registration behind it cannot be mapped without an account, so nothing more is
/// filled in code.
fn ph_etravel(i: &Inputs) -> Vec<Op> {
    vec![
        choose("sign_in", "", "Click here to Sign In"),
        wait(4000),
        text("email", "#email", &i.email()),
        require(
            "signed_in",
            "body:not(:has(#password))",
            "the sign-in to eTravel (the Cloudflare check, your email and password, or eGovPH) is the user's: hand the page over for it, then call form_fill again",
        ),
    ]
}

/// The UK ETA application on the web (https://apply-for-an-eta.homeoffice.gov.uk/apply/electronic-travel-authorisation),
/// mapped from the live site on 2026-11-02: one question per page. The start pages steer to the app — "The app is not
/// available for my phone" → "I cannot apply on the UK ETA app" → "Continue application online" → "How to apply"
/// Continue — then the email page (`#enter-email-address-value`, "Continue application" sends a security code by
/// email) and the code page. The email and its code are the machine's `email_code` step (agent/email_code.rs): the plan
/// stops while the email page shows. After it the passport and face photos are the user's (taken with their camera),
/// then the questions and the payment, so nothing more is filled in code. A Queue-it waiting room may come first: the
/// plan then finds none of its pages and the user waits in the queue.
fn uk_eta(_: &Inputs) -> Vec<Op> {
    vec![
        choose("app_not_available", "", "The app is not available for my phone"),
        wait(800),
        choose("cannot_use_app", "", "I cannot apply on the UK ETA app"),
        wait(2500),
        choose("continue_online", "", "Continue application online"),
        wait(2500),
        // only on "How to apply": a Continue elsewhere is the user's
        Op::new(
            "how_to_apply",
            json!({"op": "button", "scope": "How to apply", "sel": "button", "text": "Continue"}),
            "x",
        ),
        wait(2500),
        require(
            "past_email",
            "body:not(:has(#enter-email-address-value))",
            "the email address and the security code the Home Office emails are the user's step: hand the page over for it, then call form_fill again",
        ),
    ]
}

/// The Malaysia Digital Arrival Card (MDAC, https://imigresen-online.imi.gov.my/mdac/main → REGISTER =
/// `main?registerMain`), mapped from the live form on 2026-10-31: one page. Dates are read-only datepicker boxes
/// (dd/mm/yyyy), selects carry hidden mirrors updated on change, Nationality reloads the dialling codes and State the
/// cities, City re-renders the postcode — so those go in that order, with waits. The slider CAPTCHA and Submit are the
/// user's.
fn my_arrival_card(i: &Inputs) -> Vec<Op> {
    let f = i.facts;
    let (code, mobile) = phone_parts(&i.phone());
    let (state, city, postcode) = my_address(&f.stay_address);
    let stay_type = match f.stay_kind.as_str() {
        "hotel" => "01",
        "friends" => "02",
        "own" | "other" => "99",
        _ => "",
    };
    let address: String = [
        f.stay_name.as_str(),
        f.stay_address.split(',').next().unwrap_or(""),
    ]
    .into_iter()
    .map(str::trim)
    .filter(|x| !x.is_empty())
    .collect::<Vec<_>>()
    .join(", ")
    .to_uppercase();
    let sex = match i.sex_code().as_str() {
        "F" => "FEMALE",
        "M" => "MALE",
        _ => "",
    };
    vec![
        // the landing page: REGISTER (on the form already: not there, skipped)
        choose("register", "", "REGISTER"),
        wait(1500),
        text("name", "#name", &i.full_name()),
        vault_as("passport_number", "text", "#passNo", "number", ""),
        date_box("dob", "#dob", &date(i.birthday(), "%d/%m/%Y")),
        select("nationality", "#nationality", &iso3(&i.nationality())),
        wait(1200),
        // the country of birth only when the profile gives it (never the nationality's)
        select("place_of_birth", "#pob", &iso3(&i.birthplace())),
        select("sex", "#sex", sex),
        vault_as(
            "passport_expiry",
            "date",
            "#passExpDte",
            "expiry",
            "%d/%m/%Y",
        ),
        text("email", "#email", &i.email()),
        text("confirm_email", "#confirmEmail", &i.email()),
        // after Nationality: the dialling codes are reloaded
        select("country_code", "#region", &code),
        text("mobile", "#mobile", &mobile),
        // arrival first: it clears and limits the departure
        date_box("arrival_date", "#arrDt", &date(i.arrival(), "%d/%m/%Y")),
        date_box("departure_date", "#depDt", &date(i.departure(), "%d/%m/%Y")),
        text(
            "flight_no",
            "#vesselNm",
            &f.flight_no.replace(' ', "").to_uppercase(),
        ),
        select(
            "mode_of_travel",
            "#trvlMode",
            if f.flight_no.is_empty() { "" } else { "1" },
        ),
        select("last_port", "#embark", &iso3(&f.origin_country)),
        select("accommodation_type", "#accommodationStay", stay_type),
        text("address", "#accommodationAddress1", &address),
        select("state", "#accommodationState", &state),
        wait(1200),
        select("city", "#accommodationCity", &city),
        wait(1200),
        text("postcode", "#accommodationPostcode", &postcode),
    ]
}

/// Check the checkbox at `sel`.
pub fn tick_sel(field: &'static str, sel: &str) -> Op {
    Op::new(field, json!({"op": "check", "sel": sel}), "x")
}
/// Click the element at `sel`.
pub fn click_sel(field: &'static str, sel: &str) -> Op {
    Op::new(field, json!({"op": "click", "sel": sel}), "x")
}
/// A custom dropdown with a search box: open it (`open`), type `typed` into its search input (`search`), then click
/// the option (`option_sel`) whose text is (starts with, contains) `pick`.
pub fn dropdown(
    field: &'static str,
    open: &str,
    search: &str,
    option_sel: &str,
    typed: &str,
    pick: &str,
) -> [Op; 2] {
    [
        Op::new(field, json!({"op": "click", "sel": open}), pick),
        Op::new(
            field,
            json!({"op": "pick", "sel": search, "type": typed, "value": pick, "option_sel": option_sel}),
            pick,
        ),
    ]
}

/// The Korea e-Arrival Card (https://www.e-arrivalcard.go.kr/portal/main/index.do → Individual "Submit" → the terms
/// page `/portal/apply/agreementPolicy.do` → Confirm → `/portal/apply/individual.do`), mapped from the live form on
/// 2026-10-31: the terms page takes the consents and the representative email; the form opens after about 15 s under
/// a passport-photo popup (closed: manual entry works); inputs are classes inside the first declarant's block;
/// nationality and airline are type-to-filter dropdowns; birth date and passport expiry three selects; the arrival date
/// a jQuery UI datepicker (YYYY-MM-DD). Best effort: the address lookup popup, job and contact in Korea are left for
/// the user (not in the profile, or not mapped). Submit (and its email code) is the user's.
fn kr_arrival_card(i: &Inputs) -> Vec<Op> {
    const B: &str = ".info_wrap.applyNo01";
    let f = i.facts;
    let (given, family) = i.name_parts();
    let (airline, number) = flight_parts(&f.flight_no);
    let dob = i.birthday();
    let purpose = match f.purpose.as_str() {
        "holiday" | "" => "01",
        "business" => "03",
        "study" => "05",
        "family" => "06",
        _ => "",
    };
    let stay = [f.stay_name.as_str(), f.stay_address.as_str()]
        .into_iter()
        .filter(|x| !x.is_empty())
        .collect::<Vec<_>>()
        .join(", ");
    let sel = |c: &str| format!("{B} {c}");
    let mut ops = vec![
        // the landing page: the individual card's "Submit" (`#btnApplyP`; the group card's is #btnApplyG; elsewhere: not
        // there, skipped)
        wait(2000),
        click_sel("start_individual", "#btnApplyP"),
        wait(3000),
        // the terms page: every consent, the representative email, Confirm; the form takes a while to render
        tick_sel("agree_all", "#chkAgreementAll"),
        text("email", "#emlAddr1", &i.email()),
        text("email_confirm", "#emlAddr2", &i.email()),
        click_sel("confirm_terms", "#btnOk"),
        wait(10_000),
        wait(5_000),
        click_sel("close_photo_popup", ".popBox button.popClose"),
        wait(500),
        text("surname", &sel("input.ps_fmnm"), &family),
        text("given_names", &sel("input.ps_gvnm"), &given),
        select("dob_year", &sel("select.btd_year"), &date(dob, "%Y")),
        select("dob_month", &sel("select.btd_month"), &date(dob, "%m")),
        wait(300),
        select("dob_day", &sel("select.btd_day"), &date(dob, "%d")),
    ];
    let nat = i.nationality().to_uppercase();
    ops.extend(dropdown(
        "nationality",
        &sel("input.info_nat"),
        &sel("input.info-nat-content"),
        "div.nat-options-value",
        &nat,
        &nat,
    ));
    ops.extend([
        vault_as("passport_number", "text", &sel("input.ps_no"), "number", ""),
        select("sex", &sel("select.sd_cd"), &i.sex_code()),
        vault_as(
            "passport_expiry_year",
            "select",
            &sel("select.ps_expr_year"),
            "expiry",
            "%Y",
        ),
        vault_as(
            "passport_expiry_month",
            "select",
            &sel("select.ps_expr_month"),
            "expiry",
            "%m",
        ),
        wait(300),
        vault_as(
            "passport_expiry_day",
            "select",
            &sel("select.ps_expr_day"),
            "expiry",
            "%d",
        ),
        click_sel("arrival_by_air", &sel("button.btnEntAir")),
        date_box(
            "arrival_date",
            &sel("input.ent_prr_ymd"),
            &date(i.arrival(), "%Y-%m-%d"),
        ),
    ]);
    let code = format!("({airline})");
    ops.extend(dropdown(
        "airline",
        &sel("input.ent_cno_cd"),
        &sel("input.str-cno-content"),
        "div.cno-options-value",
        &code,
        &code,
    ));
    // the flight number field is pre-filled with the airline's code: the whole number goes in, 2 digits padded
    let flight = if number.is_empty() {
        String::new()
    } else {
        format!("{airline}{number:0>4}")
    };
    ops.extend([
        text("flight_number", &sel("input.ent_cno_nm"), &flight),
        click_sel("departure_by_air", &sel("button.btnDepAir")),
        date_box(
            "departure_date",
            &sel("input.dep_prr_ymd"),
            &date(i.departure(), "%Y-%m-%d"),
        ),
        select("purpose", &sel("select.ent_purp_cd"), purpose),
        text("stay_detail", &sel("input.soj_prrpl_rnm_det_addr"), &stay),
    ]);
    ops
}

/// Stop the plan here unless `sel` is on the page: the user's step (`why`: a sign-in, an emailed code) comes first.
pub fn require(field: &'static str, sel: &str, why: &str) -> Op {
    Op::new(field, json!({"op": "require", "sel": sel, "why": why}), "x")
}
/// Click `open` (a list without a search box), then the visible option whose text is `value`.
pub fn list_pick(field: &'static str, open: &str, value: &str) -> [Op; 2] {
    [
        Op::new(field, json!({"op": "click", "sel": open}), value),
        Op::new(field, json!({"op": "option", "value": value}), value),
    ]
}
/// Click the choice inside `sel`'s matches whose own text or value is `text` (custom radios: a readonly input per option).
pub fn choose_in(field: &'static str, scope: &str, sel: &str, text: &str) -> Op {
    Op::new(
        field,
        json!({"op": "click", "scope": scope, "sel": sel, "text": text}),
        text,
    )
}

/// All Indonesia's arrival card (https://allindonesia.imigrasi.go.id/ → Languages → English → Foreign Visitor), mapped
/// from the live site on 2026-10-31: four steps — Personal Information, Travel Details, Mode of Transportation &
/// Address, Declaration — each saved on Next. Ids carry a session suffix (prefix selectors); dropdowns are readonly
/// inputs with a Search box and a list; Yes / No and gender are readonly inputs to click; dates DD/MM/YYYY typed; the
/// arrival date a list of the allowed days ("DD MONTH YYYY"). The visa question defaults to No (a visa on arrival is
/// bought at the airport; the user reviews it). The certification and Submit are the user's.
fn id_arrival_card(i: &Inputs) -> Vec<Op> {
    let f = i.facts;
    let (_, number) = flight_parts(&f.flight_no);
    let (code, mobile) = phone_parts(&i.phone());
    let nat = i.nationality().to_uppercase();
    let birthplace = i.birthplace().to_uppercase();
    let sex = match i.sex_code().as_str() {
        "F" => "FEMALE",
        "M" => "MALE",
        _ => "",
    };
    let purpose = match f.purpose.as_str() {
        "holiday" | "" => "HOLIDAY/SIGHTSEEING/LEISURE",
        "business" => "BUSINESS/MEETING",
        "study" => "EDUCATION/TRAINING",
        "family" => "VISITING FRIENDS/RELATIVES",
        "transit" => "1-DAY TRANSIT",
        _ => "",
    };
    let stay_type = match f.stay_kind.as_str() {
        "hotel" => "HOTEL",
        "own" | "friends" => "RESIDENTIAL",
        "other" => "OTHERS",
        _ => "",
    };
    let hotel_words: String = f
        .stay_name
        .split_whitespace()
        .take(2)
        .collect::<Vec<_>>()
        .join(" ")
        .to_uppercase();
    let search = "input[placeholder=\"Search\"]";
    // a list with its own search box: the named script types into that list's box and clicks the row
    let dd = |field: &'static str, open: &str, typed: &str, pick: &str| {
        js_with(field, &SEARCH_LIST, open, 0, pick, json!({"typed": typed}))
    };
    let mut ops = vec![
        // the home page: English, then the foreign visitor's card (already past it: not there, skipped)
        // the home page's scripts take a few seconds before its menu answers clicks
        wait(4000),
        choose("language", "", "Languages"),
        wait(800),
        // the menu's entry is a <p>, the tile's title an <h1> (mapped 2026-11-02); the page re-renders in English
        choose_in("language_english", "", "p", "English"),
        // the English tile shows once the page has re-rendered (up to 10 s), then the form after its click
        Op::new(
            "await_tile",
            json!({"op": "await", "sel": "h1", "text": "Foreign Visitor", "wait_ms": 10_000}),
            "x",
        ),
        choose_in("foreign_visitor", "", "h1", "Foreign Visitor"),
        Op::new(
            "await_form",
            json!({"op": "await", "sel": "[id^=\"spi_full_name_\"]", "wait_ms": 10_000}),
            "x",
        ),
        // the form's dropdowns answer a moment after its first field shows
        wait(2000),
        // --- 1. Personal Information
    ];
    ops.push(dd("nationality", "[id^=\"spi_nationality_\"]", &nat, &nat));
    ops.extend([
        text("full_name", "[id^=\"spi_full_name_\"]", &i.full_name()),
        keys(
            "dob",
            "",
            "[id^=\"spi_dob_\"]",
            &date(i.birthday(), "%d/%m/%Y"),
        ),
        js("sex", &CHOICE_ROW, "[id^=\"spi_gender_\"]", 0, sex),
        vault(
            "passport_number",
            "",
            "[id^=\"spi_passport_no_\"]",
            "number",
            "",
        ),
        vault(
            "passport_expiry",
            "",
            "[id^=\"spi_date_of_passport_expiry_\"]",
            "expiry",
            "%d/%m/%Y",
        ),
        js(
            "different_passport",
            &CHOICE_ROW,
            "[id^=\"spi_diff_passport_\"]",
            0,
            "No",
        ),
    ]);
    // the country of birth only when the profile gives it (the same dropdown as the nationality)
    ops.push(dd(
        "place_of_birth",
        "[id^=\"spi_country_or_place_of_birth_\"]",
        &birthplace,
        &birthplace,
    ));
    // the calling code: the readonly box beside the mobile number (no id), its list "+61 - AUSTRALIA"
    let dial = if code.is_empty() {
        String::new()
    } else {
        format!("+{code}")
    };
    ops.extend([
        Op::new(
            "mobile_code",
            json!({"op": "click", "scope": "Mobile Number", "sel": "input[readonly]"}),
            &dial,
        ),
        Op::new(
            "mobile_code",
            json!({"op": "pick", "sel": search, "type": dial, "value": format!("{dial} -")}),
            &dial,
        ),
        text("mobile", "[id^=\"spi_mobile_no_\"]", &mobile),
        text("email", "[id^=\"spi_email_\"]", &i.email()),
        button("next_to_travel", "Next"),
        wait(2500),
        // --- 2. Travel Details
    ]);
    // the list renders only the rows in view: the named script scrolls it to the day and checks the box holds it
    ops.push(js(
        "arrival_date",
        &SCROLL_LIST,
        "[id^=\"std_arrival_date_foreigner_individual\"]",
        0,
        &date(i.arrival(), "%d %B %Y").to_uppercase(),
    ));
    ops.extend([
        keys(
            "departure_date",
            "",
            "[id^=\"std_departure_date_foreigner_individual\"]",
            &date(i.departure(), "%d/%m/%Y"),
        ),
        choose_in(
            "visa",
            "",
            "[id^=\"std_do_have_visa_kitas_kitap_foreigner_individual\"] input",
            "No",
        ),
        button("next_to_transport", "Next"),
        wait(2500),
        // --- 3. Mode of Transportation & Address
    ]);
    let air = !f.flight_no.is_empty();
    ops.push(dd(
        "mode",
        "[id^=\"smta_mode_transport_foreigner\"]",
        if air { "AIR" } else { "" },
        if air { "AIR" } else { "" },
    ));
    let port = if f.arrival_airport.is_empty() {
        String::new()
    } else {
        format!("{} -", f.arrival_airport)
    };
    ops.push(dd(
        "port_of_entry",
        "[id^=\"smta_place_of_arrival_air_foreigner\"]",
        &f.arrival_airport,
        &port,
    ));
    ops.push(dd(
        "air_transport",
        "[id^=\"smta_air_transport_type_foreigner\"]",
        if air { "COMMERCIAL" } else { "" },
        if air { "COMMERCIAL FLIGHT" } else { "" },
    ));
    let airline = f.airline.to_uppercase();
    ops.push(dd(
        "airline",
        "[id^=\"smta_flight_name_foreigner\"]",
        &airline,
        &airline,
    ));
    ops.push(text(
        "flight_number",
        "[id^=\"smta_flight_no_foreigner\"]",
        &number,
    ));
    ops.push(dd(
        "purpose",
        "[id^=\"smta_purpose_travel_foreigner\"]",
        purpose,
        purpose,
    ));
    ops.push(dd(
        "accommodation_type",
        "[id^=\"smta_residence_type_foreigner\"]",
        stay_type,
        stay_type,
    ));
    if stay_type == "HOTEL" {
        ops.push(dd(
            "hotel",
            "[id^=\"smta_hotel_name_foreigner\"]",
            &hotel_words,
            &f.stay_name.to_uppercase(),
        ));
    } else if stay_type == "RESIDENTIAL" {
        ops.push(text(
            "address",
            "[id^=\"smta_residential_address_foreigner\"]",
            &f.stay_address,
        ));
    }
    ops.extend([
        button("next_to_declaration", "Next"),
        wait(2500),
        // --- 4. Declaration: No, the user's own default (they review every answer before they submit)
        choose_in("symptoms", "symptoms", "input", "No"),
        choose_in("animals_plants", "animals, fish, plants", "input", "No"),
        choose_in("customs_goods", "declared to Customs", "input", "No"),
        choose_in("imei", "Mobile Phones", "input", "No"),
    ]);
    ops
}

/// Click `sel` inside the section headed `scope` (Element UI fields have no ids: their label heads the section).
pub fn click_in(field: &'static str, scope: &str, sel: &str, value: &str) -> Op {
    Op::new(
        field,
        json!({"op": "click", "scope": scope, "sel": sel}),
        value,
    )
}
/// Click the visible option `option_sel` whose text is (starts with, contains) `value`.
pub fn option(field: &'static str, option_sel: &str, value: &str) -> Op {
    Op::new(
        field,
        json!({"op": "option", "option_sel": option_sel, "value": value}),
        value,
    )
}
/// An Element UI select inside the section headed `scope` (`nth` input of it): open it, click the listed option.
fn el_select(field: &'static str, scope: &str, nth: usize, value: &str) -> [Op; 2] {
    [
        Op::new(
            field,
            json!({"op": "click", "scope": scope, "sel": "input", "nth": nth}),
            value,
        ),
        option(field, "li.el-select-dropdown__item", value),
    ]
}

/// Nationalities China admits visa-free for a short stay (ISO alpha-3; the unilateral and mutual visa-free lists of
/// 2024–2026, the common ones): the arrival card's entry policy is then "Visa-free Entry". Others are left for the
/// user (a visa, a transit policy — not guessed).
const CN_VISA_FREE: &[&str] = &[
    "AUS", "NZL", "FRA", "DEU", "ITA", "ESP", "NLD", "CHE", "IRL", "AUT", "BEL", "LUX", "PRT",
    "GRC", "POL", "DNK", "FIN", "NOR", "SWE", "HUN", "JPN", "KOR", "SGP", "MYS", "THA", "BRN",
    "GBR",
];

/// China's ports by airport code: the word the port of entry's option carries.
fn cn_port(iata: &str) -> &'static str {
    match iata {
        "PVG" => "Pudong",
        "SHA" => "Hongqiao",
        "PEK" => "Capital",
        "PKX" => "Daxing",
        "CAN" => "Baiyun",
        "SZX" => "Bao",
        "CTU" | "TFU" => "Tianfu",
        "HGH" => "Xiaoshan",
        _ => "",
    }
}

/// China's online arrival card (https://s.nia.gov.cn/ArrivalCardFillingPC/ → Entry Declaration → I AGREE → the form),
/// mapped from the live site on 2026-10-31: an Element UI form in five steps on one URL. Step 1 is the user's (a photo
/// of the passport page, which the site reads to fill step 2); then Basic, Personal and Travel Information. Fields have
/// no ids — each is found by its label; selects open a list (`li.el-select-dropdown__item`), filterable ones take typed
/// text; cities come from the "Popular Cities" list; the entry date is typed yyyy-MM-dd with Enter. The plan stops
/// after Travel Information: the companion prompt, the accompanying persons and Submit are the user's.
fn cn_arrival_card(i: &Inputs) -> Vec<Op> {
    let f = i.facts;
    let (given, family) = i.name_parts();
    let dob = i.birthday();
    let nat3 = iso3(&i.nationality());
    let nat_name = i.nationality();
    let sex = match i.sex_code().as_str() {
        "F" => "Female",
        "M" => "Male",
        _ => "",
    };
    let visa_free = CN_VISA_FREE.contains(&nat3.as_str());
    let purpose = match f.purpose.as_str() {
        "holiday" | "" => "Sightseeing/Leisure",
        "business" => "Conference/Business",
        "family" => "Visiting",
        _ => "",
    };
    // the city: the stay address's last part ("…, Huangpu District, Shanghai" → Shanghai)
    let city = f
        .stay_address
        .rsplit(',')
        .next()
        .unwrap_or("")
        .trim()
        .to_string();
    let detail = [f.stay_name.as_str(), f.stay_address.as_str()]
        .into_iter()
        .filter(|x| !x.is_empty())
        .collect::<Vec<_>>()
        .join(", ");
    let flight = f.flight_no.replace(' ', "").to_uppercase();
    let mut ops = vec![
        // the home page and the notice (already past them: not there, skipped)
        choose("entry_declaration", "", "Entry Declaration"),
        wait(1500),
        choose("agree", "", "I AGREE"),
        wait(1500),
        // step 1 (the passport photo, read by the site) is the user's
        Op::new(
            "basic_information",
            json!({"op": "require", "sel": "div.el-form-item", "text": "Last Name",
            "why": "the passport photo upload (step 1) is the user's: hand the page over for it, then call form_fill again"}),
            "x",
        ),
        // --- Basic Information
        Op::new(
            "last_name",
            json!({"op": "text", "scope": "Last Name", "sel": "input", "value": family}),
            &family,
        ),
        Op::new(
            "first_name",
            json!({"op": "text", "scope": "First Name", "sel": "input", "value": given}),
            &given,
        ),
    ];
    ops.extend(el_select("sex", "Gender", 0, sex));
    ops.extend(el_select("dob_year", "Date of Birth", 0, &date(dob, "%Y")));
    ops.extend(el_select("dob_month", "Date of Birth", 1, &date(dob, "%m")));
    ops.push(wait(300));
    ops.extend(el_select("dob_day", "Date of Birth", 2, &date(dob, "%d")));
    ops.extend([
        Op::new("nationality", json!({"op": "pick", "scope": "Country/Region of Citizenship", "sel": "input", "type": nat_name,
                                      "value": format!("（{nat3}）"), "option_sel": "li.el-select-dropdown__item"}), &nat3),
        Op::new("passport_number", json!({"op": "text", "scope": "Passport Number", "sel": "input", "vault": {"field": "number", "date_fmt": ""}}), "x"),
    ]);
    ops.extend(el_select(
        "entry_mode",
        "Entry Transportation Mode",
        0,
        if flight.is_empty() { "" } else { "Flight" },
    ));
    ops.extend([
        Op::new(
            "flight_number",
            json!({"op": "text", "scope": "Arrival Flight", "sel": "input", "value": flight}),
            &flight,
        ),
        click_in("city_of_entry", "City of Entry", "input", &city),
        option("city_of_entry", "ul.hot-city li", &city),
        wait(800),
    ]);
    ops.extend(el_select(
        "port_of_entry",
        "Port of Entry",
        0,
        cn_port(&f.arrival_airport),
    ));
    ops.extend([button("next_to_personal", "Next"), wait(1200)]);
    // --- Personal Information
    ops.extend([
        Op::new(
            "contact_number",
            json!({"op": "text", "scope": "Contact Number", "sel": "input", "value": i.phone()}),
            &i.phone(),
        ),
        Op::new(
            "email",
            json!({"op": "text", "scope": "Email", "sel": "input", "value": i.email()}),
            &i.email(),
        ),
    ]);
    ops.extend(el_select(
        "holds_visa",
        "valid visa",
        0,
        if visa_free { "No" } else { "" },
    ));
    ops.push(wait(300));
    ops.extend(el_select(
        "entry_policy",
        "Entry Policy",
        0,
        if visa_free { "Visa-free Entry" } else { "" },
    ));
    ops.extend([button("next_to_travel", "Next"), wait(1200)]);
    // --- Travel Information
    ops.extend(el_select("purpose", "Purpose of Entry", 0, purpose));
    ops.extend([
        Op::new("entry_date", json!({"op": "keys", "scope": "Date of Entry", "sel": "input", "value": format!("{}\n", date(i.arrival(), "%Y-%m-%d"))}),
                &date(i.arrival(), "%Y-%m-%d")),
        click_in("destination_city", "Destination Cities", "input", &city),
        option("destination_city", "ul.hot-city li", &city),
        Op::new("address_detail", json!({"op": "text", "scope": "Address in China", "sel": "textarea", "value": detail}), &detail),
        choose_in("inviter", "inviting entities", "label.el-radio", "No"),
        choose_in("departure_itinerary", "Confirmed Departure Itinerary", "label.el-radio", "No"),
    ]);
    ops
}

/// Nationalities Taiwan admits visa-free (ISO alpha-3; the National Immigration Agency's list, the common ones): the
/// TWAC's visa type is then "Visa-Exempt". Others are left for the user (a visa, a landing visa or a travel
/// authorisation — not guessed).
const TW_VISA_EXEMPT: &[&str] = &[
    "AUS", "NZL", "USA", "CAN", "GBR", "IRL", "JPN", "KOR", "SGP", "MYS", "DEU", "FRA", "ITA",
    "ESP", "NLD", "BEL", "AUT", "CHE", "SWE", "NOR", "DNK", "FIN", "POL", "CZE", "PRT", "GRC",
    "ISR",
];

/// The Taiwan Arrival Card's occupation (`#traveler1_occupation`, a native select with a fixed list, read from the live
/// site's bundle 2026-11-02): the general business person, option value "10" (" 商/執行長/總裁/BUSINESS/DIRECTOR/CEO").
/// The user's decision (2026-11-02): every card with a fixed occupation list gets that list's general business-person
/// option, by its fixed value — no model choice, no guess from the profile's job title.
pub const TW_OCCUPATION_BUSINESS: &str = "10";

/// Letters, digits, spaces and . , - only (Taiwan's address box), upper case.
fn tw_text(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, ' ' | '.' | ',' | '-'))
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_uppercase()
}

/// The Taiwan Arrival Card (https://twac.immigration.gov.tw/ → Submit Arrival Card → `/notice` START → `/submit`),
/// mapped from the live site on 2026-10-31 and 2026-11-02: step 1 is the email verification (`input[name=email]`,
/// "SEND VERIFICATION CODE", the code Taiwan emails — the machine's `email_code` step, else the user's); then,
/// on the same URL, Traveler Information, Itinerary and Review. Ids are `traveler1_<field>`; dates DD/MM/YYYY text
/// (react-datepicker); nationality, the calling code and the flight code react-select comboboxes that need the exact
/// option text ("AUS,AUSTRALIA", "61 AUS", "CI : CHINA AIRLINES"); radios are labels inside `.radioGrp`. The
/// declaration and SUBMIT are the user's.
fn tw_arrival_card(i: &Inputs) -> Vec<Op> {
    let f = i.facts;
    let (airline, number) = flight_parts(&f.flight_no);
    let (code, mobile) = phone_parts(&i.phone());
    let nat3 = iso3(&i.nationality());
    let nat = if nat3.is_empty() {
        String::new()
    } else {
        format!("{nat3},{}", i.nationality().to_uppercase())
    };
    // a country as the site's lists write it: "AUS,AUSTRALIA"
    let listed = |name: &str| {
        let c = iso3(name);
        if c.is_empty() {
            String::new()
        } else {
            format!("{c},{}", name.to_uppercase())
        }
    };
    let birth = listed(&i.birthplace());
    let living = crate::places::country(&crate::places::home_country(i.profile))
        .map(|c| listed(c.name))
        .unwrap_or_default();
    let calling = if code.is_empty() || nat3.is_empty() {
        String::new()
    } else {
        format!("{code} {nat3}")
    };
    let radio = |field: &'static str, group: &str, value: &str| {
        Op::new(
            field,
            json!({"op": "click", "sel": format!(".radioGrp:has(#traveler1_{group}) label.btnStyle:has(input[value=\"{value}\"])")}),
            value,
        )
    };
    let sex = match i.sex_code().as_str() {
        "F" => "Female",
        "M" => "Male",
        _ => "",
    };
    let visa = if TW_VISA_EXEMPT.contains(&nat3.as_str()) {
        "VisaExempt"
    } else {
        ""
    };
    let purpose = match f.purpose.as_str() {
        "holiday" | "" => "1",
        "business" => "16",
        "study" => "213",
        _ => "",
    };
    let (stay_flag, address) = match f.stay_kind.as_str() {
        "hotel" => ("Hotel Name", tw_text(&f.stay_name)),
        "own" | "friends" => ("Residential Address", tw_text(&f.stay_address)),
        _ => ("", String::new()),
    };
    let flight_code = if airline.is_empty() {
        String::new()
    } else {
        format!("{airline} :")
    };
    let flight_typed = if airline.is_empty() {
        String::new()
    } else {
        format!("{airline} : {}", f.airline.to_uppercase())
            .trim_end_matches(" : ")
            .to_string()
    };
    // the flight that leaves Taiwan (its "Expected Mode of Departure" by air requires its code and number): the same
    // react-select as the arrival's; with the arrival's airline the same typed text, else the code alone
    let (leave_airline, leave_number) = flight_parts(&i.arg("departure_flight_no"));
    let leave_code = if leave_airline.is_empty() {
        String::new()
    } else {
        format!("{leave_airline} :")
    };
    let leave_typed = if leave_airline == airline {
        flight_typed.clone()
    } else {
        leave_airline.clone()
    };
    vec![
        // the landing page: Submit Arrival Card → the notice's START → /submit (not there once on the form: skipped)
        choose("submit_card", "", "Submit Arrival Card"),
        Op::new(
            "await_notice",
            json!({"op": "await", "sel": "button", "text": "START", "wait_ms": 10_000}),
            "x",
        ),
        button("start", "START"),
        Op::new(
            "await_email_step",
            json!({"op": "await", "sel": "input[name=email]", "wait_ms": 10_000}),
            "x",
        ),
        // step 1, the email verification (input[name=email], "SEND VERIFICATION CODE", the emailed code), is the
        // machine's `email_code` step (agent/email_code.rs) or the user's: nothing to fill until the traveller's form shows
        require("traveler_form", "#traveler1_englishName",
                "the email verification (the code Taiwan emails) is the user's: hand the page over for it, then call form_fill again"),
        // --- Traveler Information
        text("entry_date", "#traveler1_expectEntryDate", &date(i.arrival(), "%d/%m/%Y")),
        text("departure_date", "#traveler1_intendedDepartureDate", &date(i.departure(), "%d/%m/%Y")),
        text("english_name", "#traveler1_englishName", &i.full_name()),
        vault_as("passport_number", "text", "#traveler1_passportNo", "number", ""),
        vault_as("passport_expiry", "text", "#traveler1_passportExpiryDate", "expiry", "%d/%m/%Y"),
        Op::new("sex", json!({"op": "click", "sel": format!("label.btnStyle:has(input[type=radio][value=\"{sex}\"])")}), sex),
        text("dob", "#traveler1_birthDate", &date(i.birthday(), "%d/%m/%Y")),
        pick("nationality", "#traveler1_nation", &nat, &nat),
        // the country of birth (only from the profile) and of residence (the home country's code): the same list as the
        // nationality, "AUS,AUSTRALIA" (a row not found leaves the box for the user)
        wait(500),
        pick("place_of_birth", "#traveler1_birthPlace1", &birth, &birth),
        wait(500),
        pick("place_of_residence", "#traveler1_livingCity", &living, &living),
        radio("visa_type", "visaTypeRadio", visa),
        pick("country_code", "#traveler1_phoneCountryCode", &calling, &calling),
        text("mobile", "#traveler1_phoneNumber", &mobile),
        // the occupation (required by NEXT): the list's general business person, by its fixed value
        select("occupation", "#traveler1_occupation", TW_OCCUPATION_BUSINESS),
        text("email", "#traveler1_email", &i.email()),
        button("next_to_itinerary", "NEXT"),
        wait(2000),
        // --- Itinerary
        radio("entry_mode", "arrivalTravelModeFlag", if airline.is_empty() { "" } else { "AIR" }),
        pick("flight_code", "#traveler1_arrivalFlightCode", &flight_typed, &flight_code),
        text("flight_number", "#traveler1_arrivalFlightNumber", &number),
        radio("exit_mode", "departureTravelModeFlag", if leave_airline.is_empty() { "" } else { "AIR" }),
        pick("exit_flight_code", "#traveler1_departureFlightCode", &leave_typed, &leave_code),
        text("exit_flight_number", "#traveler1_departureFlightNumber", &leave_number),
        select("purpose", "#traveler1_reason", purpose),
        radio("accommodation_type", "hotelFlag", stay_flag),
        text("address", "#traveler1_address", &address),
        button("next_to_review", "NEXT"),
        wait(1500),
    ]
}

/// (block / house number, street, postal code) of a Singapore address written "10 Bayfront Avenue, Singapore 018956":
/// the leading number, the words after it up to the first comma, the 6-digit code. A part it cannot find is empty.
pub fn sg_address(addr: &str) -> (String, String, String) {
    let first = addr.split(',').next().unwrap_or("").trim();
    let (block, street) = match first.split_once(' ') {
        Some((n, rest)) if n.chars().next().is_some_and(|c| c.is_ascii_digit()) => {
            (n.to_string(), rest.trim().to_uppercase())
        }
        _ => (String::new(), first.to_uppercase()),
    };
    let postal = addr
        .split(|c: char| !c.is_ascii_digit())
        .find(|t| t.len() == 6)
        .unwrap_or("")
        .to_string();
    (block, street, postal)
}

/// (floor, unit) of a Singapore address's unit, written "#12-05" (the form's Floor and Unit boxes); empty when the
/// address has none (a house: the user ticks "Not Applicable").
pub fn sg_unit(addr: &str) -> (String, String) {
    addr.split([',', ' '])
        .map(str::trim)
        .filter_map(|t| t.strip_prefix('#'))
        .filter_map(|t| t.split_once('-'))
        .find(|(f, u)| {
            !f.is_empty()
                && !u.is_empty()
                && f.chars().all(|c| c.is_ascii_digit())
                && u.chars().all(|c| c.is_ascii_alphanumeric())
        })
        .map(|(f, u)| (f.to_string(), u.to_uppercase()))
        .unwrap_or_default()
}

/// The SG Arrival Card (ICA, https://eservices.ica.gov.sg/sgarrivalcard/ → "Fill in Manually" as a visitor →
/// `/arrivalcard/sgac/fvipa/submit`), mapped from the live form on 2026-10-31: an Angular stepper — Traveller Details,
/// Visit Details, Review — on one URL. Radios are chosen by their text (their ids change every session), dates are
/// three key-typed boxes per field, lookups take only a listed value (type, then pick). The plan ends on Review: the
/// declaration and Submit are the user's.
fn sg_arrival_card(i: &Inputs) -> Vec<Op> {
    const DAY: &str = "input[id$=-input-day]";
    const MONTH: &str = "input[id$=-input-month]";
    const YEAR: &str = "input[id$=-input-year]";
    let f = i.facts;
    // a radio of the Angular form, chosen by its text inside the block headed `scope` — the block that holds the
    // `mat-radio-button`s (a scope alone would find the bare heading; arrival_real 2026-11-02)
    let radio = |field: &'static str, scope: &str, text: &str| {
        choose_in(field, scope, "mat-radio-button", text)
    };
    let date3 = |field: [&'static str; 3], scope: &str, d: Option<NaiveDate>| {
        vec![
            keys(field[0], scope, DAY, &date(d, "%d")),
            keys(field[1], scope, MONTH, &date(d, "%m")),
            keys(field[2], scope, YEAR, &date(d, "%Y")),
        ]
    };
    let (code, mobile) = phone_parts(&i.phone());
    let (airline, number) = flight_parts(&f.flight_no);
    let sex = match i.sex_code().as_str() {
        "F" => "FEMALE",
        "M" => "MALE",
        "X" => "OTHERS",
        _ => "",
    };
    let purpose = match f.purpose.as_str() {
        "business" => "BUSINESS",
        "study" => "EDUCATION",
        "transit" => "TRANSIT",
        "family" => "VISIT FAMILY",
        "holiday" | "" => "HOLIDAY",
        _ => "",
    };
    let origin = f.origin_city.to_uppercase();
    let next = i.next_city().to_uppercase();
    let mut ops = vec![
        // the landing page (/arrivalcard/sgac): the visitor's "Fill in Manually" (on the form already: not there, skipped)
        // (the visitor's button by where it goes — the resident's card has a "Fill in Manually" too; mapped 2026-11-02)
        click_sel(
            "start_as_visitor",
            "app-general-button[navigatetopage=\"/sgac/fvipa/submit\"] button",
        ),
        wait(1500),
        // --- Traveller Details
        radio(
            "arrival_date",
            "Date of Arrival",
            &date(i.arrival(), "%d %B %Y").to_uppercase(),
        ),
        text("full_name", "#traveller-0-input-fullName", &i.full_name()),
        vault(
            "passport_number",
            "",
            "#traveller-0-input-passportNumber",
            "number",
            "",
        ),
    ];
    ops.extend([
        vault(
            "passport_expiry_day",
            "Date of Passport Expiry",
            DAY,
            "expiry",
            "%d",
        ),
        vault(
            "passport_expiry_month",
            "Date of Passport Expiry",
            MONTH,
            "expiry",
            "%m",
        ),
        vault(
            "passport_expiry_year",
            "Date of Passport Expiry",
            YEAR,
            "expiry",
            "%Y",
        ),
        radio("sex", "Sex as indicated in Passport", sex),
    ]);
    ops.extend(date3(
        ["dob_day", "dob_month", "dob_year"],
        "Date of Birth",
        i.birthday(),
    ));
    let nationality = demonym(&i.nationality());
    let birthplace = i.birthplace().to_uppercase();
    // the residence list is "COUNTRY, STATE, CITY (COUNTRY)": the home city typed, its row picked by "CITY (COUNTRY)"
    // (a country alone would pick its first city — a wrong value); unknown home city: left for the user
    let (res_typed, res_pick) = match i.home_city() {
        Some((city, country)) => (
            city.to_uppercase(),
            format!("{} ({})", city.to_uppercase(), country.to_uppercase()),
        ),
        None => (String::new(), String::new()),
    };
    ops.extend([
        pick(
            "nationality",
            "#traveller-0-input-nationality",
            &nationality,
            &nationality,
        ),
        // the country of birth only when the profile gives it; the residence is the home address's country
        wait(600),
        pick(
            "place_of_birth",
            "#traveller-0-input-placeOfBirth",
            &birthplace,
            &birthplace,
        ),
        wait(600),
        pick(
            "place_of_residence",
            "#traveller-0-input-placeOfResidence",
            &res_typed,
            &res_pick,
        ),
        wait(600),
        text("email", "#traveller-0-input-emailAddress", &i.email()),
        pick(
            "country_code",
            "#traveller-0-input-countryCode",
            &code,
            &code,
        ),
        text("mobile", "#traveller-0-input-mobileNumber", &mobile),
        pick("last_city", "#traveller-0-input-lastCity", &origin, &origin),
        pick(
            "purpose",
            "#traveller-0-input-purposeOfTravel",
            purpose,
            purpose,
        ),
    ]);
    ops.extend(date3(
        ["departure_day", "departure_month", "departure_year"],
        "Date of Departure from Singapore",
        i.departure(),
    ));
    if next == origin {
        ops.push(tick("next_city", "", "Same as Last City"));
    } else {
        ops.push(pick(
            "next_city",
            "#traveller-0-input-nextCity",
            &next,
            &next,
        ));
    }
    // the declarations: No, the user's own default (they review every answer before they submit)
    ops.extend([
        radio(
            "different_name_passport",
            "passport under different name",
            "NO",
        ),
        radio("health_symptoms", "fever", "NO"),
        wait(500),
        radio("health_yellow_fever", "yellow fever", "NO"),
        button("next_to_visit_details", "Next"),
        wait(1200),
        // --- Visit Details
        radio(
            "mode_of_travel",
            "Mode of Travel",
            if airline.is_empty() { "" } else { "AIR" },
        ),
        radio(
            "mode_of_transport",
            "Mode of Transport",
            if airline.is_empty() {
                ""
            } else {
                "COMMERCIAL FLIGHT"
            },
        ),
        pick(
            "flight_code",
            "#traveller-0-input-flightCode",
            &airline,
            &format!("({airline})"),
        ),
        text("flight_number", "#traveller-0-input-flightNumber", &number),
    ]);
    match f.stay_kind.as_str() {
        "hotel" => {
            let words: String = f
                .stay_name
                .split_whitespace()
                .take(2)
                .collect::<Vec<_>>()
                .join(" ")
                .to_uppercase();
            ops.extend([
                radio("accommodation_type", "Type of Accommodation", "HOTEL"),
                pick(
                    "hotel",
                    "#traveller-0-input-nameOfHotel",
                    &words,
                    &f.stay_name.to_uppercase(),
                ),
            ]);
        }
        "own" | "friends" => {
            // the real form has one RESIDENTIAL type for a home and a host's; its address in parts
            let (block, street, postal) = sg_address(&f.stay_address);
            let (floor, unit) = sg_unit(&f.stay_address);
            ops.extend([
                radio("accommodation_type", "Type of Accommodation", "RESIDENTIAL"),
                text("postal_code", "#traveller-0-input-postalCode", &postal),
                text("block_number", "#traveller-0-input-blockNumber", &block),
                text("street_name", "#traveller-0-input-streetName", &street),
                text("floor_number", "#traveller-0-input-floorNumber", &floor),
                text("unit_number", "#traveller-0-input-unitNumber", &unit),
            ]);
        }
        "transit" => ops.push(radio(
            "accommodation_type",
            "Type of Accommodation",
            "TRANSIT",
        )),
        _ => {}
    }
    ops.extend([button("next_to_review", "Next"), wait(800)]);
    ops
}

/// A date in a form's format, or empty.
pub fn date(d: Option<NaiveDate>, fmt: &str) -> String {
    d.map(|d| d.format(fmt).to_string()).unwrap_or_default()
}

/// bolttech's Travel Insurance Claim Form (the fillable PDF FWD Hong Kong links, `travel-insurance-clm-form-fillable.pdf`),
/// its field names as the form defines them: the values `pdf_form_fill` gets for a flight-delay claim. The HKID number,
/// the bank account, the signature and the date of signing are the user's and never set.
pub fn bolttech_delay_claim(a: &Value, profile: &Value) -> Value {
    let g = |k: &str| get_str(a, k).trim().to_string();
    let p = |k: &str| {
        profile
            .get(k)
            .map(py_str)
            .unwrap_or_default()
            .trim()
            .to_string()
    };
    let email = if p("email_personal").is_empty() {
        p("email")
    } else {
        p("email_personal")
    };
    let when = |t: &str| {
        [g("incident_date").as_str(), t]
            .iter()
            .filter(|x| !x.is_empty())
            .cloned()
            .collect::<Vec<_>>()
            .join(" ")
    };
    let place = if g("from").is_empty() {
        when(&g("scheduled"))
    } else {
        format!("{}, {} airport", when(&g("scheduled")), g("from"))
    };
    let mut v = json!({
        "fill_17": g("policy_no"),
        "Full name": p("name_en").to_uppercase(),
        "Correspondence address": p("address_home"),
        "Email address": email,
        "fill_12": place,
        "fill_15": g("what"),
        "fill_11": when(&g("scheduled")),
        "fill_12_2": g("from"),
        "fill_13_2": g("to"),
        "fill_14_2": g("flight_no"),
        "fill_15_2": when(&g("actual")),
        "fill_16_2": g("from"),
        "fill_17_2": g("to"),
        "fill_18_2": g("flight_no"),
        "fill_1": g("hours"),
        "fill_7": g("amount"),
        "fill_8": g("airline"),
        "fill_9_2": g("airline_ref"),
    });
    if let Some(m) = v.as_object_mut() {
        m.retain(|_, x| !x.as_str().unwrap_or("").is_empty());
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile() -> Value {
        json!({"name_en": "Jordan Lu", "gender": "Female", "birthday": "1985-04-12", "nationality": "Australia",
               "email_personal": "jordan.lu@example.com", "phone": "+61 400 000 000",
               "address_home": "12 Harbour St, Sydney NSW 2000, Australia"})
    }

    #[test]
    fn inputs_and_defaults_come_from_the_profile_and_the_facts() {
        let facts = Facts::from_args(
            &json!({"flight_no": "SQ 231", "arrival_date": "2026-11-02", "origin_city": "Sydney",
                                             "stay_name": "Riverside Hotel", "stay_address": "1 River Rd", "stay_kind": "Hotel"}),
        );
        let p = profile();
        let i = Inputs {
            facts: &facts,
            profile: &p,
        };
        assert_eq!(i.name_parts(), ("JORDAN".to_string(), "LU".to_string()));
        assert_eq!(i.sex_code(), "F");
        assert_eq!(date(i.birthday(), "%d/%m/%Y"), "12/04/1985");
        // departure = arrival + 7 days, next city = where the flight came from, unless given
        assert_eq!(date(i.departure(), "%Y-%m-%d"), "2026-11-09");
        assert_eq!(i.next_city(), "Sydney");
        assert_eq!(i.stay(), "Riverside Hotel, 1 River Rd");
        assert_eq!(facts.stay_kind, "hotel");
        let given = Facts {
            departure_date: "2026-11-05".into(),
            next_city: "Bangkok".into(),
            ..facts.clone()
        };
        let i = Inputs {
            facts: &given,
            profile: &p,
        };
        assert_eq!(date(i.departure(), "%Y-%m-%d"), "2026-11-05");
        assert_eq!(i.next_city(), "Bangkok");
    }

    #[test]
    fn a_plan_leaves_out_what_it_has_no_value_for_and_never_carries_a_vault_value() {
        let ops = [
            text("name", "#name", "JORDAN LU"),
            text("middle", "#middle", ""),
            choose("sex", "Sex", "FEMALE"),
            vault(
                "passport_expiry",
                "Date of Passport Expiry",
                "input[id$=-input-day]",
                "expiry",
                "%d",
            ),
        ];
        let plan: Vec<Value> = ops.iter().filter_map(Op::to_json).collect();
        assert_eq!(plan.len(), 3);
        assert_eq!(
            plan[2],
            json!({"op": "keys", "scope": "Date of Passport Expiry", "sel": "input[id$=-input-day]",
                                   "vault": {"field": "expiry", "date_fmt": "%d"}, "field": "passport_expiry"})
        );
    }

    #[test]
    fn the_sg_arrival_card_plan_is_the_real_forms() {
        let facts = Facts::from_args(
            &json!({"flight_no": "SQ 231", "arrival_date": "2026-11-02", "origin_city": "Sydney",
                                             "stay_name": "Marina Bay Sands", "stay_address": "10 Bayfront Ave, Singapore 018956",
                                             "stay_kind": "hotel", "purpose": "holiday"}),
        );
        let p = profile();
        let plan: Vec<Value> = sg_arrival_card(&Inputs {
            facts: &facts,
            profile: &p,
        })
        .iter()
        .filter_map(Op::to_json)
        .collect();
        let by = |field: &str| {
            plan.iter()
                .find(|o| o["field"] == field)
                .cloned()
                .unwrap_or(Value::Null)
        };
        assert_eq!(by("arrival_date")["text"], "02 NOVEMBER 2026");
        // a scoped radio needs its block to hold the radios (a scope alone finds the bare heading)
        assert_eq!(by("arrival_date")["sel"], "mat-radio-button");
        assert_eq!(by("sex")["sel"], "mat-radio-button");
        assert_eq!(by("sex")["text"], "FEMALE");
        assert_eq!(by("dob_day")["value"], "12");
        assert_eq!(by("nationality")["value"], "AUSTRALIAN");
        // no home city code in the profile: the residence is left for the user (a country alone picks a wrong
        // city); no country of birth either
        assert!(by("place_of_residence").is_null());
        assert!(by("place_of_birth").is_null());
        let mut born = p.clone();
        born["birthplace"] = json!("Australia");
        born["home_city"] = json!("SYD");
        born["home_country"] = json!("AU");
        let plan2: Vec<Value> = sg_arrival_card(&Inputs {
            facts: &facts,
            profile: &born,
        })
        .iter()
        .filter_map(Op::to_json)
        .collect();
        assert!(plan2.iter().any(|o| o["field"] == "place_of_birth"
            && o["sel"] == "#traveller-0-input-placeOfBirth"
            && o["value"] == "AUSTRALIA"));
        assert_eq!(by("country_code")["value"], "61");
        assert_eq!(by("mobile")["value"], "400000000");
        assert_eq!(
            (
                by("departure_day")["value"].clone(),
                by("departure_month")["value"].clone()
            ),
            (json!("09"), json!("11"))
        );
        assert_eq!(by("next_city")["op"], "check"); // next city = last city: "Same as Last City"
        assert_eq!(by("flight_code")["value"], "(SQ)");
        assert_eq!(by("flight_number")["value"], "231");
        assert_eq!(by("hotel")["type"], "MARINA BAY");
        assert!(
            by("passport_expiry_day")["vault"]["date_fmt"] == "%d"
                && by("passport_expiry_day").get("value").is_none()
        );
        // the plan stops before the declaration and Submit
        assert!(!plan.iter().any(|o| o["text"] == "Submit"
            || o["text"]
                .as_str()
                .is_some_and(|t| t.contains("declaration"))));
        assert!(plan2.iter().any(|o| o["field"] == "place_of_residence"
            && o["type"] == "SYDNEY"
            && o["value"] == "SYDNEY (AUSTRALIA)"));
    }

    /// `OMUSE_FORM_PLAN_OUT=<file> cargo test -p omuse-runtime forms::tests::dump_sg_plan`: the SG plan as JSON, for
    /// replaying it against the official page in a browser.
    #[test]
    fn dump_sg_plan() {
        let Ok(out) = std::env::var("OMUSE_FORM_PLAN_OUT") else {
            return;
        };
        let arrive = (chrono::Local::now().date_naive() + Duration::days(2))
            .format("%Y-%m-%d")
            .to_string();
        let facts = Facts::from_args(
            &json!({"flight_no": "SQ 231", "arrival_date": arrive, "origin_city": "Sydney",
                                             "stay_name": "Marina Bayfront Hotel", "stay_address": "10 Bayfront Avenue, Singapore 018956",
                                             "stay_kind": "hotel", "purpose": "holiday"}),
        );
        let p = profile();
        let plan: Vec<Value> = sg_arrival_card(&Inputs {
            facts: &facts,
            profile: &p,
        })
        .iter()
        .filter_map(Op::to_json)
        .collect();
        std::fs::write(out, serde_json::to_string_pretty(&plan).unwrap()).unwrap();
    }

    #[test]
    fn the_malaysia_card_plan_follows_the_forms_order() {
        let facts = Facts::from_args(
            &json!({"flight_no": "MH 122", "arrival_date": "2026-11-02", "origin_city": "Sydney",
                                             "origin_country": "Australia", "stay_name": "Bukit Bintang Grand Hotel",
                                             "stay_address": "22 Jalan Bukit Bintang, 55100 Kuala Lumpur", "stay_kind": "hotel"}),
        );
        let p = profile();
        let plan: Vec<Value> = my_arrival_card(&Inputs {
            facts: &facts,
            profile: &p,
        })
        .iter()
        .filter_map(Op::to_json)
        .collect();
        let at = |field: &str| {
            plan.iter()
                .position(|o| o["field"] == field)
                .unwrap_or(usize::MAX)
        };
        let by = |field: &str| {
            plan.iter()
                .find(|o| o["field"] == field)
                .cloned()
                .unwrap_or(Value::Null)
        };
        // nationality before the dialling code, arrival before departure, state → city → postcode
        assert!(
            at("nationality") < at("country_code") && at("arrival_date") < at("departure_date")
        );
        assert!(at("state") < at("city") && at("city") < at("postcode"));
        assert_eq!(by("nationality")["value"], "AUS");
        assert_eq!(by("dob")["value"], "12/04/1985");
        assert_eq!(by("departure_date")["value"], "09/11/2026");
        assert_eq!(by("flight_no")["value"], "MH122");
        assert_eq!(by("accommodation_type")["value"], "01");
        assert_eq!(
            by("address")["value"],
            "BUKIT BINTANG GRAND HOTEL, 22 JALAN BUKIT BINTANG"
        );
        assert_eq!(
            (
                by("state")["value"].clone(),
                by("postcode")["value"].clone()
            ),
            (json!("KUALA LUMPUR"), json!("55100"))
        );
        assert_eq!(by("passport_expiry")["vault"]["date_fmt"], "%d/%m/%Y");
        // the place of birth (#pob) only from the profile's birthplace, never the nationality
        assert!(by("place_of_birth").is_null());
        let mut born = p.clone();
        born["birthplace"] = json!("Australia");
        let plan2: Vec<Value> = my_arrival_card(&Inputs {
            facts: &facts,
            profile: &born,
        })
        .iter()
        .filter_map(Op::to_json)
        .collect();
        assert!(plan2
            .iter()
            .any(|o| o["sel"] == "#pob" && o["value"] == "AUS"));
    }

    /// `OMUSE_FORM_PLAN_OUT=<file>`: the Malaysia plan as JSON, for replaying it against the official page in a browser.
    #[test]
    fn dump_my_plan() {
        let Ok(out) = std::env::var("OMUSE_FORM_PLAN_OUT_MY") else {
            return;
        };
        let arrive = (chrono::Local::now().date_naive() + Duration::days(2))
            .format("%Y-%m-%d")
            .to_string();
        let facts = Facts::from_args(
            &json!({"flight_no": "MH 122", "arrival_date": arrive, "origin_city": "Sydney",
                                             "origin_country": "Australia", "stay_name": "Bukit Bintang Grand Hotel",
                                             "stay_address": "22 Jalan Bukit Bintang, 55100 Kuala Lumpur", "stay_kind": "hotel"}),
        );
        let p = profile();
        let plan: Vec<Value> = my_arrival_card(&Inputs {
            facts: &facts,
            profile: &p,
        })
        .iter()
        .filter_map(Op::to_json)
        .collect();
        std::fs::write(out, serde_json::to_string_pretty(&plan).unwrap()).unwrap();
    }

    #[test]
    fn the_korea_card_plan_starts_on_the_terms_page() {
        let facts = Facts::from_args(
            &json!({"flight_no": "KE 122", "arrival_date": "2026-11-02", "origin_city": "Sydney",
                                             "stay_name": "Hotel Namsan Gardens", "stay_address": "15 Toegye-ro 20-gil, Jung-gu, Seoul 04637",
                                             "stay_kind": "hotel", "purpose": "holiday"}),
        );
        let p = profile();
        let plan: Vec<Value> = kr_arrival_card(&Inputs {
            facts: &facts,
            profile: &p,
        })
        .iter()
        .filter_map(Op::to_json)
        .collect();
        let at = |field: &str| {
            plan.iter()
                .position(|o| o["field"] == field)
                .unwrap_or(usize::MAX)
        };
        let by = |field: &str| {
            plan.iter()
                .find(|o| o["field"] == field && o["op"] != "click")
                .cloned()
                .unwrap_or(Value::Null)
        };
        assert!(at("agree_all") < at("confirm_terms") && at("confirm_terms") < at("surname"));
        assert_eq!(
            (
                by("surname")["value"].clone(),
                by("given_names")["value"].clone()
            ),
            (json!("LU"), json!("JORDAN"))
        );
        assert_eq!(by("nationality")["value"], "AUSTRALIA");
        assert_eq!(by("airline")["value"], "(KE)");
        assert_eq!(by("flight_number")["value"], "KE0122");
        assert_eq!(by("arrival_date")["value"], "2026-11-02");
        assert_eq!(by("purpose")["value"], "01");
        assert_eq!(by("passport_expiry_year")["vault"]["date_fmt"], "%Y");
    }

    /// `OMUSE_FORM_PLAN_OUT_KR=<file>`: the Korea plan as JSON, for replaying it against the official page in a browser.
    #[test]
    fn dump_kr_plan() {
        let Ok(out) = std::env::var("OMUSE_FORM_PLAN_OUT_KR") else {
            return;
        };
        let arrive = (chrono::Local::now().date_naive() + Duration::days(2))
            .format("%Y-%m-%d")
            .to_string();
        let facts = Facts::from_args(
            &json!({"flight_no": "KE 122", "arrival_date": arrive, "origin_city": "Sydney",
                                             "stay_name": "Hotel Namsan Gardens", "stay_address": "15 Toegye-ro 20-gil, Jung-gu, Seoul 04637",
                                             "stay_kind": "hotel", "purpose": "holiday"}),
        );
        let p = profile();
        let plan: Vec<Value> = kr_arrival_card(&Inputs {
            facts: &facts,
            profile: &p,
        })
        .iter()
        .filter_map(Op::to_json)
        .collect();
        std::fs::write(out, serde_json::to_string_pretty(&plan).unwrap()).unwrap();
    }

    #[test]
    fn the_indonesia_card_plan_walks_its_four_steps() {
        let facts = Facts::from_args(
            &json!({"flight_no": "GA 715", "arrival_date": "2026-11-02", "origin_city": "Sydney",
                                             "arrival_airport": "dps", "airline": "Garuda Indonesia",
                                             "stay_name": "Seminyak Lagoon Resort", "stay_address": "Jl. Kayu Aya No. 8, Seminyak, Bali 80361",
                                             "stay_kind": "hotel", "purpose": "holiday"}),
        );
        let p = profile();
        let plan: Vec<Value> = id_arrival_card(&Inputs {
            facts: &facts,
            profile: &p,
        })
        .iter()
        .filter_map(Op::to_json)
        .collect();
        let nexts: Vec<usize> = plan
            .iter()
            .enumerate()
            .filter(|(_, o)| o["text"] == "Next")
            .map(|(i, _)| i)
            .collect();
        assert_eq!(nexts.len(), 3);
        let by = |field: &str| {
            plan.iter()
                .rev()
                .find(|o| o["field"] == field)
                .cloned()
                .unwrap_or(Value::Null)
        };
        assert_eq!(by("nationality")["value"], "AUSTRALIA");
        // the calling code: the box beside the mobile number, its row "+61 - AUSTRALIA"
        // the gender and the "different passport" rows: the named script that clicks the row holding the value
        assert_eq!(
            (
                by("sex")["op"].as_str(),
                by("sex")["script"].as_str(),
                by("sex")["value"].as_str()
            ),
            (Some("js"), Some("choice_row"), Some("FEMALE"))
        );
        assert_eq!(by("different_passport")["src"], CHOICE_ROW.src);
        assert!(ID_SCRIPTS.contains(&CHOICE_ROW));
        assert_eq!(by("mobile_code")["type"], "+61");
        assert_eq!(by("mobile_code")["value"], "+61 -");
        assert!(plan
            .iter()
            .any(|o| o["field"] == "mobile_code" && o["scope"] == "Mobile Number"));
        assert!(plan.iter().all(|o| o["field"] != "place_of_birth"));
        assert_eq!(by("arrival_date")["value"], "02 NOVEMBER 2026");
        assert_eq!(by("arrival_date")["script"], "scroll_list");
        assert!(ID_SCRIPTS.contains(&SCROLL_LIST));
        assert_eq!(by("port_of_entry")["value"], "DPS -");
        assert_eq!(by("airline")["value"], "GARUDA INDONESIA");
        assert_eq!(by("flight_number")["value"], "715");
        assert_eq!(by("purpose")["value"], "HOLIDAY/SIGHTSEEING/LEISURE");
        assert_eq!(by("hotel")["args"]["typed"], "SEMINYAK LAGOON");
        assert_eq!(by("hotel")["script"], "search_list");
        // the certification and Submit are the user's
        assert!(!plan
            .iter()
            .any(|o| o["text"] == "Submit"
                || o["text"].as_str().is_some_and(|t| t.contains("certify"))));
    }

    /// `OMUSE_FORM_PLAN_OUT_ID=<file>`: the Indonesia plan as JSON, for replaying it against the official page.
    #[test]
    fn dump_id_plan() {
        let Ok(out) = std::env::var("OMUSE_FORM_PLAN_OUT_ID") else {
            return;
        };
        let arrive = (chrono::Local::now().date_naive() + Duration::days(2))
            .format("%Y-%m-%d")
            .to_string();
        let facts = Facts::from_args(
            &json!({"flight_no": "GA 715", "arrival_date": arrive, "origin_city": "Sydney",
                                             "arrival_airport": "DPS", "airline": "Garuda Indonesia",
                                             "stay_name": "Seminyak Lagoon Resort", "stay_address": "Jl. Kayu Aya No. 8, Seminyak, Bali 80361",
                                             "stay_kind": "hotel", "purpose": "holiday"}),
        );
        let p = profile();
        let plan: Vec<Value> = id_arrival_card(&Inputs {
            facts: &facts,
            profile: &p,
        })
        .iter()
        .filter_map(Op::to_json)
        .collect();
        std::fs::write(out, serde_json::to_string_pretty(&plan).unwrap()).unwrap();
    }

    #[test]
    fn the_taiwan_card_plan_waits_for_the_email_step_and_knows_the_visa_exempt() {
        let facts = Facts::from_args(
            &json!({"flight_no": "CI 52", "arrival_date": "2026-11-02", "airline": "China Airlines",
                                             "stay_name": "Xinyi Park Hotel", "stay_address": "6 Songshou Road, Xinyi District, Taipei 110",
                                             "stay_kind": "hotel", "purpose": "holiday", "departure_flight_no": "CI 51"}),
        );
        let p = profile();
        let plan: Vec<Value> = tw_arrival_card(&Inputs {
            facts: &facts,
            profile: &p,
        })
        .iter()
        .filter_map(Op::to_json)
        .collect();
        // the stop for the user's email step comes before any traveller field (after the landing clicks and the email)
        let at = |pred: &dyn Fn(&Value) -> bool| plan.iter().position(pred).unwrap_or(usize::MAX);
        assert!(
            at(&|o| o["op"] == "require") < at(&|o| o["field"] == "english_name"),
            "{plan:?}"
        );
        let by = |field: &str| {
            plan.iter()
                .find(|o| o["field"] == field)
                .cloned()
                .unwrap_or(Value::Null)
        };
        assert_eq!(by("nationality")["value"], "AUS,AUSTRALIA");
        assert_eq!(by("country_code")["value"], "61 AUS");
        assert!(by("visa_type")["sel"]
            .as_str()
            .unwrap_or("")
            .contains("VisaExempt"));
        assert_eq!(
            (
                by("flight_code")["type"].clone(),
                by("flight_code")["value"].clone()
            ),
            (json!("CI : CHINA AIRLINES"), json!("CI :"))
        );
        assert_eq!(by("flight_number")["value"], "52");
        // the flight leaving Taiwan (required by the itinerary's departure mode AIR): the same airline's typed text
        assert!(by("exit_mode")["sel"]
            .as_str()
            .unwrap_or("")
            .contains("departureTravelModeFlag"));
        assert_eq!(
            (
                by("exit_flight_code")["type"].clone(),
                by("exit_flight_code")["value"].clone()
            ),
            (json!("CI : CHINA AIRLINES"), json!("CI :"))
        );
        assert_eq!(by("exit_flight_number")["value"], "51");
        assert_eq!(by("address")["value"], "XINYI PARK HOTEL");
        // the occupation NEXT requires (arrival_cards2_real 2026-11-02: "Occupation: Please fill in the field above"):
        // the fixed list's general business person by its value, before the traveller step's NEXT — also with no job
        // title in the profile
        assert_eq!(
            (
                by("occupation")["op"].clone(),
                by("occupation")["sel"].clone(),
                by("occupation")["value"].clone()
            ),
            (json!("select"), json!("#traveler1_occupation"), json!("10"))
        );
        assert!(
            at(&|o| o["field"] == "occupation") < at(&|o| o["field"] == "next_to_itinerary"),
            "{plan:?}"
        );
        // a nationality not on the visa-exempt list: the visa type is left for the user
        let p2 = json!({"name_en": "A B", "nationality": "India"});
        let plan2: Vec<Value> = tw_arrival_card(&Inputs {
            facts: &facts,
            profile: &p2,
        })
        .iter()
        .filter_map(Op::to_json)
        .collect();
        assert!(!plan2.iter().any(|o| o["field"] == "visa_type"));
    }

    /// `OMUSE_FORM_PLAN_OUT_TW=<file>`: the Taiwan plan as JSON, for replaying it against the official page.
    #[test]
    fn dump_tw_plan() {
        let Ok(out) = std::env::var("OMUSE_FORM_PLAN_OUT_TW") else {
            return;
        };
        let arrive = (chrono::Local::now().date_naive() + Duration::days(2))
            .format("%Y-%m-%d")
            .to_string();
        let facts = Facts::from_args(
            &json!({"flight_no": "CI 52", "arrival_date": arrive, "airline": "China Airlines",
                                             "stay_name": "Xinyi Park Hotel", "stay_address": "6 Songshou Road, Xinyi District, Taipei 110",
                                             "stay_kind": "hotel", "purpose": "holiday"}),
        );
        let p = profile();
        let plan: Vec<Value> = tw_arrival_card(&Inputs {
            facts: &facts,
            profile: &p,
        })
        .iter()
        .filter_map(Op::to_json)
        .collect();
        std::fs::write(out, serde_json::to_string_pretty(&plan).unwrap()).unwrap();
    }

    #[test]
    fn the_china_card_plan_finds_fields_by_label_and_stops_before_the_companions() {
        let facts = Facts::from_args(
            &json!({"flight_no": "MU 562", "arrival_date": "2026-11-02", "arrival_airport": "PVG",
                                             "stay_name": "Bund Riverside Hotel",
                                             "stay_address": "99 Zhongshan East 1st Road, Huangpu District, Shanghai",
                                             "stay_kind": "hotel", "purpose": "holiday"}),
        );
        let p = profile();
        let plan: Vec<Value> = cn_arrival_card(&Inputs {
            facts: &facts,
            profile: &p,
        })
        .iter()
        .filter_map(Op::to_json)
        .collect();
        let by = |field: &str| {
            plan.iter()
                .rev()
                .find(|o| o["field"] == field)
                .cloned()
                .unwrap_or(Value::Null)
        };
        assert_eq!(by("basic_information")["op"], "require");
        assert_eq!(
            (
                by("last_name")["scope"].clone(),
                by("last_name")["value"].clone()
            ),
            (json!("Last Name"), json!("LU"))
        );
        assert_eq!(by("nationality")["value"], "（AUS）");
        assert_eq!(by("port_of_entry")["value"], "Pudong");
        assert_eq!(by("city_of_entry")["value"], "Shanghai");
        assert_eq!(by("entry_policy")["value"], "Visa-free Entry"); // Australia is on China's visa-free list
        assert_eq!(by("entry_date")["value"], "2026-11-02\n");
        // the companion prompt and Submit stay the user's: the last Next goes to Travel Information
        assert_eq!(plan.iter().filter(|o| o["text"] == "Next").count(), 2);
    }

    /// `OMUSE_FORM_PLAN_OUT_CN=<file>`: the China plan as JSON, for replaying it against the official page.
    #[test]
    fn dump_cn_plan() {
        let Ok(out) = std::env::var("OMUSE_FORM_PLAN_OUT_CN") else {
            return;
        };
        let arrive = (chrono::Local::now().date_naive() + Duration::days(2))
            .format("%Y-%m-%d")
            .to_string();
        let facts = Facts::from_args(
            &json!({"flight_no": "MU 562", "arrival_date": arrive, "arrival_airport": "PVG",
                                             "stay_name": "Bund Riverside Hotel",
                                             "stay_address": "99 Zhongshan East 1st Road, Huangpu District, Shanghai",
                                             "stay_kind": "hotel", "purpose": "holiday"}),
        );
        let p = profile();
        let plan: Vec<Value> = cn_arrival_card(&Inputs {
            facts: &facts,
            profile: &p,
        })
        .iter()
        .filter_map(Op::to_json)
        .collect();
        std::fs::write(out, serde_json::to_string_pretty(&plan).unwrap()).unwrap();
    }

    #[test]
    fn phone_flight_and_nationality_parts() {
        assert_eq!(
            phone_parts("+61 400 000 000"),
            ("61".into(), "400000000".into())
        );
        assert_eq!(
            phone_parts("+852 9123 4567"),
            ("852".into(), "91234567".into())
        );
        assert_eq!(phone_parts("0400 000 000"), (String::new(), String::new()));
        assert_eq!(flight_parts("SQ 231"), ("SQ".into(), "231".into()));
        assert_eq!(flight_parts("tr0012"), ("TR".into(), "12".into()));
        assert_eq!(
            flight_parts("Singapore Airlines"),
            (String::new(), String::new())
        );
        assert_eq!(demonym("Australia"), "AUSTRALIAN");
        assert_eq!(demonym("Narnia"), "");
        assert_eq!(
            sg_address("10 Bayfront Avenue, Singapore 018956"),
            ("10".into(), "BAYFRONT AVENUE".into(), "018956".into())
        );
        assert_eq!(sg_address("Blk 5 Jalan Kayu, Singapore 799487").2, "799487");
        assert_eq!(
            sg_unit("10 Bayfront Avenue, #12-05, Singapore 018956"),
            ("12".into(), "05".into())
        );
        assert_eq!(
            sg_unit("5 Jalan Kayu #03-01a Singapore 799487"),
            ("03".into(), "01A".into())
        );
        assert_eq!(
            sg_unit("10 Bayfront Avenue, Singapore 018956"),
            (String::new(), String::new())
        );
        assert_eq!(iso3("Australia"), "AUS");
        assert_eq!(
            my_address("22 Jalan Bukit Bintang, 55100 Kuala Lumpur"),
            ("KUALA LUMPUR".into(), "KUALA LUMPUR".into(), "55100".into())
        );
        assert_eq!(
            my_address("1 Jalan Ampang, 50450 Kuala Lumpur, Malaysia").1,
            "KUALA LUMPUR"
        );
    }

    #[test]
    fn each_known_form_is_found_by_its_real_address() {
        for (url, name) in [
            ("https://eservices.ica.gov.sg/arrivalcard/sgac/fvipa/submit", "sg-arrival-card"),
            ("https://eservices.ica.gov.sg/arrivalcard/sgac", "sg-arrival-card"),
            ("https://imigresen-online.imi.gov.my/mdac/main?registerMain", "my-arrival-card"),
            ("https://www.e-arrivalcard.go.kr/portal/apply/agreementPolicy.do?applyType=P&type=PC", "kr-arrival-card"),
            ("https://www.e-arrivalcard.go.kr/portal/main/index.do?locale=EN", "kr-arrival-card"),
            ("https://allindonesia.imigrasi.go.id/arrival-card-submission/personal-information", "id-arrival-card"),
            ("https://twac.immigration.gov.tw/submit", "tw-arrival-card"),
            ("https://s.nia.gov.cn/ArrivalCardFillingPC/entry-registation-form", "cn-arrival-card"),
            ("https://www.vjw.digital.go.jp/main/#/vjwplo001", "jp-visit-japan-web"),
            ("https://arrival.gov.kh/", "kh-arrival-card"),
            ("https://arrival.gov.kh/e-arrival/foreigner/individual-form/draft", "kh-arrival-card"),
            ("https://etravel.gov.ph/", "ph-etravel"),
            ("https://etravel.gov.ph/signin", "ph-etravel"),
            ("https://tdac.immigration.go.th/arrival-card/#/home", "th-arrival-card"),
            ("https://apply-for-an-eta.homeoffice.gov.uk/apply/electronic-travel-authorisation", "uk-eta"),
            ("https://apply-for-an-eta.homeoffice.gov.uk/electronic-travel-authorisation/2021-2611-0292-0230/electronic-travel-authorisation/enter-email-address", "uk-eta"),
        ] {
            assert_eq!(form_for(url).map(|(n, _)| n), Some(name), "{url}");
        }
    }

    #[test]
    fn an_unknown_page_has_no_function() {
        assert!(form_for("https://example.com/form").is_none());
        assert!(form_for("not a url").is_none());
    }

    #[test]
    fn the_tdac_plan_presses_the_tile_and_hands_over_at_the_cloudflare_check() {
        // the real TDAC home page holds a Turnstile check before any field: the plan presses the tile and stops until a
        // field shows, and fills nothing it could not map
        let facts = one_page_facts(
            "TG 476",
            "Siam Riverside Hotel",
            "88 Charoen Krung Road, Bangkok 10500",
        );
        let plan = plan_of(th_arrival_card, &facts, &profile());
        let names: Vec<&str> = plan.iter().filter_map(|o| o["field"].as_str()).collect();
        assert_eq!(names, ["arrival_card", "wait", "tdac_form"]);
        let stop = field(&plan, "tdac_form");
        assert_eq!(stop["op"], "require");
        assert!(stop["why"]
            .as_str()
            .unwrap()
            .starts_with("the Cloudflare check"));
        assert!(stop["why"]
            .as_str()
            .unwrap()
            .contains(" is the user's: hand the page over for it, then call form_fill again"));
        assert!(form_for("https://tdac.immigration.go.th/")
            .is_some_and(|(n, _)| n == "th-arrival-card"));
    }

    #[test]
    fn the_twac_plan_walks_to_the_email_step_and_leaves_the_email_to_the_code_step() {
        let facts = Facts {
            flight_no: "CI 52".into(),
            arrival_date: "2026-11-03".into(),
            origin_city: "Sydney".into(),
            stay_name: "Xinyi Park Hotel".into(),
            stay_kind: "hotel".into(),
            purpose: "holiday".into(),
            ..Facts::default()
        };
        let p = serde_json::json!({"name_en": "JORDAN LU", "gender": "Female", "birthday": "1985-04-12",
            "nationality": "Australia", "email_personal": "jordan@example.com", "phone": "+61 400 000 000"});
        let ops = plan_of(tw_arrival_card, &facts, &p);
        let at = |field: &str| index(&ops, field);
        // the real start: the tile, the notice's START (a button by its text), then the stop at the email step
        assert!(
            at("submit_card") < at("start") && at("start") < at("traveler_form"),
            "{ops:?}"
        );
        assert_eq!(ops[at("start")]["op"], "button");
        assert_eq!(ops[at("start")]["text"], "START");
        assert_eq!(ops[at("traveler_form")]["op"], "require");
        // the email and its code are the `email_code` step's (agent/email_code.rs), never the plan's
        assert!(ops[..at("traveler_form")]
            .iter()
            .all(|o| o["value"] != "jordan@example.com"));
        assert!(!ops
            .iter()
            .any(|o| o["field"] == "verify_email" || o["field"] == "send_code"));
        // the country of birth only from the profile; the residence from the home country's code
        assert!(ops
            .iter()
            .all(|o| o["field"] != "place_of_birth" && o["field"] != "place_of_residence"));
        let mut p2 = p.clone();
        p2["birthplace"] = json!("Australia");
        p2["home_country"] = json!("AU");
        let ops2 = plan_of(tw_arrival_card, &facts, &p2);
        assert_eq!(field(&ops2, "place_of_birth")["value"], "AUS,AUSTRALIA");
        assert_eq!(
            field(&ops2, "place_of_residence")["sel"],
            "#traveler1_livingCity"
        );
        assert_eq!(field(&ops2, "place_of_residence")["value"], "AUS,AUSTRALIA");
    }

    /// The plan of `f` for a one-page card: the ops as the broker takes them.
    fn plan_of(f: FormFn, facts: &Facts, profile: &Value) -> Vec<Value> {
        f(&Inputs { facts, profile })
            .iter()
            .filter_map(Op::to_json)
            .collect()
    }

    fn one_page_facts(flight: &str, stay: &str, address: &str) -> Facts {
        Facts {
            flight_no: flight.into(),
            arrival_date: "2026-11-03".into(),
            origin_city: "Sydney".into(),
            origin_country: "Australia".into(),
            stay_name: stay.into(),
            stay_address: address.into(),
            stay_kind: "hotel".into(),
            purpose: "holiday".into(),
            ..Facts::default()
        }
    }

    fn field<'a>(plan: &'a [Value], name: &str) -> &'a Value {
        plan.iter()
            .find(|o| o["field"] == name)
            .unwrap_or(&Value::Null)
    }

    fn index(plan: &[Value], name: &str) -> usize {
        plan.iter()
            .position(|o| o["field"] == name)
            .unwrap_or(usize::MAX)
    }

    #[test]
    fn the_visit_japan_web_plan_puts_the_email_in_and_stops_at_the_password() {
        let facts = one_page_facts(
            "NH 880",
            "Shinjuku Granbell Inn",
            "2-14-5 Kabukicho, Shinjuku-ku, Tokyo 160-0021",
        );
        let plan = plan_of(jp_visit_japan_web, &facts, &profile());
        assert_eq!(field(&plan, "email")["sel"], "#textbox01");
        assert_eq!(field(&plan, "email")["value"], "jordan.lu@example.com");
        let stop = field(&plan, "signed_in");
        assert_eq!(
            (stop["op"].as_str(), stop["sel"].as_str()),
            (Some("require"), Some("body:not(:has(#textbox02))"))
        );
        assert!(stop["why"]
            .as_str()
            .unwrap()
            .starts_with("the sign-in to your Visit Japan Web account"));
        assert!(index(&plan, "email") < index(&plan, "signed_in"));
        // the password is never typed
        assert!(!plan
            .iter()
            .any(|o| o["sel"] == "#textbox02" && o["op"] != "require"));
        assert!(form_for("https://www.vjw.digital.go.jp/main/#/vjwplo001")
            .is_some_and(|(n, _)| n == "jp-visit-japan-web"));
    }

    #[test]
    fn the_cambodia_plan_fills_the_first_step_by_its_control_names() {
        let facts = one_page_facts(
            "SQ 166",
            "Riverside Palace Hotel",
            "51 Sisowath Quay, Phnom Penh",
        );
        let p = profile();
        let plan = plan_of(kh_arrival_card, &facts, &p);
        assert!(!plan.iter().any(|o| o["op"] == "require"));
        // the guest start, then the passport: the number from the Vault, the issuing country from the picker
        assert!(index(&plan, "submit_e_arrival") < index(&plan, "guest"));
        assert_eq!(field(&plan, "passport_number")["vault"]["field"], "number");
        assert_eq!(
            field(&plan, "passport_number")["sel"],
            "input[formcontrolname=passportNumber]"
        );
        let pick = plan
            .iter()
            .find(|o| o["field"] == "passport_country" && o["op"] == "pick")
            .unwrap();
        assert_eq!(
            (pick["type"].as_str(), pick["value"].as_str()),
            (Some("AUS"), Some(". AUS"))
        );
        // the expiry's three selects from the Vault (day, month, year), the first of each
        let expiry: Vec<&Value> = plan
            .iter()
            .filter(|o| o["field"] == "passport_expiry_day")
            .collect();
        // one named script per select (op js, the mat-select script from this file's table), never a value in it
        assert_eq!(expiry.len(), 1);
        assert_eq!(
            (
                expiry[0]["op"].as_str(),
                expiry[0]["script"].as_str(),
                expiry[0]["nth"].as_i64()
            ),
            (Some("js"), Some("mat_select"), Some(0))
        );
        assert_eq!(expiry[0]["src"], MAT_SELECT.src);
        assert_eq!(expiry[0]["sel"], "mat-select[formcontrolname=day]");
        assert_eq!(expiry[0]["vault"]["date_fmt"], "%d");
        assert!(expiry[0].get("value").is_none());
        assert!(KH_SCRIPTS
            .iter()
            .all(|sc| sc.src.starts_with("async (sel, nth, args)")));
        assert_eq!(field(&plan, "surname")["value"], "LU");
        assert_eq!(field(&plan, "given_name")["value"], "JORDAN");
        let opt = |name: &str| {
            plan.iter()
                .find(|o| o["field"] == name && o["op"] == "js")
                .map(|o| o["value"].clone())
                .unwrap_or(Value::Null)
        };
        assert_eq!(opt("sex"), "Female");
        assert_eq!(
            (opt("dob_day"), opt("dob_month"), opt("dob_year")),
            (json!("12"), json!("04"), json!("1985"))
        );
        // the birth date is the second of the date selects; the Travel step (hidden until the Profile step is done)
        // is the user's: nothing of it in the plan
        assert_eq!(field(&plan, "dob_day")["nth"], 1);
        assert!(plan
            .iter()
            .all(|o| !["departure_day", "arrival_date", "flight_number"]
                .contains(&o["field"].as_str().unwrap_or(""))));
        assert_eq!(field(&plan, "email")["value"], "jordan.lu@example.com");
        // the country of birth's picker is left for the user (its rows are not mapped)
        let mut with_birth = p.clone();
        with_birth["birthplace"] = json!("Australia");
        assert!(plan_of(kh_arrival_card, &facts, &with_birth)
            .iter()
            .all(|o| o["field"] != "birth_country"));
        // the CAPTCHA and Submit stay the user's
        assert!(!plan
            .iter()
            .any(|o| o["field"] == "captcha" || o["text"] == "Submit"));
        assert!(form_for("https://arrival.gov.kh/").is_some_and(|(n, _)| n == "kh-arrival-card"));
    }

    #[test]
    fn the_etravel_plan_goes_to_the_sign_in_and_stops_at_the_password() {
        let facts = one_page_facts(
            "PR 210",
            "Makati Palm Suites",
            "1200 Ayala Avenue, Makati City, Metro Manila",
        );
        let plan = plan_of(ph_etravel, &facts, &profile());
        assert!(
            index(&plan, "sign_in") < index(&plan, "email")
                && index(&plan, "email") < index(&plan, "signed_in")
        );
        assert_eq!(field(&plan, "email")["value"], "jordan.lu@example.com");
        let stop = field(&plan, "signed_in");
        assert_eq!(stop["sel"], "body:not(:has(#password))");
        assert!(stop["why"]
            .as_str()
            .unwrap()
            .starts_with("the sign-in to eTravel"));
        assert!(!plan
            .iter()
            .any(|o| o["sel"] == "#password" && o["op"] != "require"));
        assert!(form_for("https://etravel.gov.ph/").is_some_and(|(n, _)| n == "ph-etravel"));
    }

    #[test]
    fn the_uk_eta_plan_walks_the_start_pages_and_stops_at_the_email_step() {
        let p = json!({"name_en": "Jordan Lu", "gender": "Female", "birthday": "1985-04-12", "nationality": "Australia",
               "email_personal": "jordan.lu@example.com", "address_home": "12 Harbour St, Sydney NSW 2000, Australia",
               "job_title": "Software engineer"});
        let plan = plan_of(uk_eta, &Facts::default(), &p);
        let names: Vec<&str> = plan
            .iter()
            .filter_map(|o| o["field"].as_str())
            .filter(|f| *f != "wait")
            .collect();
        assert_eq!(
            names,
            [
                "app_not_available",
                "cannot_use_app",
                "continue_online",
                "how_to_apply",
                "past_email"
            ]
        );
        // the Continue is pressed only on "How to apply": a Continue on any later page is the user's
        assert_eq!(field(&plan, "how_to_apply")["scope"], "How to apply");
        let stop = field(&plan, "past_email");
        assert_eq!(stop["sel"], "body:not(:has(#enter-email-address-value))");
        assert!(stop["why"].as_str().unwrap().contains("security code"));
        // nothing personal is typed by the plan (the email is the code step's), nothing is paid
        assert!(!plan
            .iter()
            .any(|o| o["op"] == "text" || o.get("vault").is_some()));
        // the trip facts change nothing
        let with_trip = plan_of(
            uk_eta,
            &one_page_facts("QF 1", "Covent Garden Court Hotel", "8 Bedford Street"),
            &p,
        );
        assert_eq!(with_trip, plan);
        assert!(form_for(
            "https://apply-for-an-eta.homeoffice.gov.uk/apply/electronic-travel-authorisation"
        )
        .is_some_and(|(n, _)| n == "uk-eta"));
    }

    #[test]
    fn bolttechs_pdf_values_leave_the_hkid_bank_and_signature_to_the_user() {
        let p = json!({"name_en": "Chan Tai Man", "email_personal": "taiman@example.com", "address_home": "Flat 12B, 88 Robinson Road, Hong Kong"});
        let v = bolttech_delay_claim(
            &json!({"policy_no": "MT-HK-2026-00881", "incident_date": "2026-10-29", "scheduled": "09:05", "actual": "15:20",
            "from": "Hong Kong", "to": "Tokyo Narita", "flight_no": "CX 543", "hours": "6 h 15 min", "amount": "300.00", "airline": "Cathay Pacific",
            "airline_ref": "CX-DLY-55821", "what": "CX 543 delayed 6 h 15 min (technical inspection)"}),
            &p,
        );
        assert_eq!(v["fill_17"], "MT-HK-2026-00881");
        assert_eq!(v["Full name"], "CHAN TAI MAN");
        assert_eq!(v["Email address"], "taiman@example.com");
        assert_eq!(v["fill_11"], "2026-10-29 09:05");
        assert_eq!(v["fill_15_2"], "2026-10-29 15:20");
        assert_eq!(v["fill_12"], "2026-10-29 09:05, Hong Kong airport");
        assert_eq!(
            (v["fill_14_2"].clone(), v["fill_18_2"].clone()),
            (json!("CX 543"), json!("CX 543"))
        );
        assert_eq!(
            (
                v["fill_1"].clone(),
                v["fill_7"].clone(),
                v["fill_8"].clone(),
                v["fill_9_2"].clone()
            ),
            (
                json!("6 h 15 min"),
                json!("300.00"),
                json!("Cathay Pacific"),
                json!("CX-DLY-55821")
            )
        );
        for user_only in [
            "HKID Card no",
            "fill_1_4",
            "fill_4_5",
            "Claimants signature",
            "Date",
        ] {
            assert!(v.get(user_only).is_none(), "{user_only}");
        }
        // nothing known: nothing written (no empty strings over the form's fields)
        assert_eq!(
            bolttech_delay_claim(&json!({}), &json!({}))
                .as_object()
                .map(|m| m.len()),
            Some(0)
        );
    }
}
