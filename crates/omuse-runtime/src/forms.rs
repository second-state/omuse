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
    /// The occupation (the profile's job title).
    pub fn job_title(&self) -> String {
        self.p("job_title")
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
    ("jp-visit-japan-web", "vjw-lp.digital.go.jp", "/", jp_visit_japan_web),
    ("kh-arrival-card", "arrival.gov.kh", "/", kh_arrival_card),
    ("ph-etravel", "etravel.gov.ph", "/", ph_etravel),
    ("uk-eta", "www.gov.uk", "/eta", uk_eta),
    // the claims and the purchase driven by `agent::claims`
    ("csr-trip-claim", "www.eclaimsline.com", "/", eclaims_trip_claim),
    ("fwd-sg-claim", "www.fwd.com.sg", "/claim-online/travel", fwd_sg_claim),
    ("fwd-sg-quote", "www.fwd.com.sg", "/travel-insurance/online-quote", fwd_sg_quote),
    ("fwd-sg-apply", "www.fwd.com.sg", "/fwd_sg_apply", fwd_sg_apply),
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

/// The Thailand Digital Arrival Card (TDAC, https://tdac.immigration.go.th), mapped from the form on 2026-11-01: one
/// page, dates DD/MM/YYYY, selects by ISO alpha-3, Yes / No radios by name. The CAPTCHA and Submit are the user's.
fn th_arrival_card(i: &Inputs) -> Vec<Op> {
    let f = i.facts;
    let (given, family) = i.name_parts();
    let nat = iso3(&i.nationality());
    let purpose = purpose_option(&f.purpose);
    let accom_type = accom_option(&f.stay_kind);
    let mut ops = vec![
        text("family_name", "#family_name", &family),
        text("first_name", "#first_name", &given),
        radio("sex", "sex", &i.sex_code()),
        text("dob", "#dob", &date(i.birthday(), "%d/%m/%Y")),
        select("nationality", "#nationality", &nat),
        select("country_residence", "#country_residence", &nat),
        vault_as("passport_no", "text", "#passport_no", "number", ""),
        vault_as(
            "passport_expiry",
            "text",
            "#passport_expiry",
            "expiry",
            "%d/%m/%Y",
        ),
        select("passport_country", "#passport_country", &nat),
        text("email", "#email", &i.email()),
        text("phone", "#phone", &i.phone()),
        select(
            "mode",
            "#mode",
            if f.flight_no.is_empty() { "" } else { "AIR" },
        ),
        text("flight_no", "#flight_no", &f.flight_no),
        text(
            "arrival_date",
            "#arrival_date",
            &date(i.arrival(), "%d/%m/%Y"),
        ),
        text("last_city", "#last_city", &f.origin_city),
        text("next_city", "#next_city", &i.next_city()),
        text(
            "departure_date",
            "#departure_date",
            &date(i.departure(), "%d/%m/%Y"),
        ),
        select("purpose", "#purpose", purpose),
        select("accom_type", "#accom_type", accom_type),
        text("accommodation", "#accommodation", &i.stay()),
    ];
    for q in [
        "h_symptoms",
        "h_travel",
        "d_names",
        "d_refused",
        "d_criminal",
        "d_goods",
    ] {
        ops.push(radio(q, q, "No"));
    }
    ops
}

/// The purpose option of the one-page cards (TDAC, Cambodia, eTravel, Visit Japan Web): HOLIDAY / BUSINESS / FAMILY /
/// STUDY / TRANSIT; empty for one not known (left for the user).
fn purpose_option(purpose: &str) -> &'static str {
    match purpose {
        "holiday" => "HOLIDAY",
        "business" => "BUSINESS",
        "family" => "FAMILY",
        "study" => "STUDY",
        "transit" => "TRANSIT",
        _ => "",
    }
}

/// The accommodation type option of the same cards: HOTEL / RESIDENCE (friends or relatives) / OWN / OTHERS.
fn accom_option(stay_kind: &str) -> &'static str {
    match stay_kind {
        "hotel" => "HOTEL",
        "friends" => "RESIDENCE",
        "own" => "OWN",
        "other" => "OTHERS",
        _ => "",
    }
}

/// Visit Japan Web (https://vjw-lp.digital.go.jp/ → Log in → "Register planned entry"), mapped from the pages rebuilt
/// from the site on 2026-11-01: the sign-in (email, password, any 2-factor code) is the user's — the plan stops until
/// the entry form's first field shows; then one page, dates DD/MM/YYYY, selects by ISO alpha-3, the customs questions
/// Yes / No radios by name (No, the user's default; they review before they register). There is no next-city field
/// and no health section. The security check and Register are the user's.
fn jp_visit_japan_web(i: &Inputs) -> Vec<Op> {
    let f = i.facts;
    let (given, family) = i.name_parts();
    let nat = iso3(&i.nationality());
    let mut ops = vec![
        // the sign-in page has no entry fields: the user's step comes first
        require("entry_form", "#family_name",
                "the sign-in to your Visit Japan Web account (email, password and any 2-factor code) is the user's: hand the page over for it, then call form_fill again"),
        text("family_name", "#family_name", &family),
        text("given_name", "#given_name", &given),
        radio("sex", "sex", &i.sex_code()),
        text("dob", "#dob", &date(i.birthday(), "%d/%m/%Y")),
        select("nationality", "#nationality", &nat),
        vault_as("passport_no", "text", "#passport_no", "number", ""),
        vault_as("passport_expiry", "text", "#passport_expiry", "expiry", "%d/%m/%Y"),
        select("passport_country", "#passport_country", &nat),
        text("email", "#email", &i.email()),
        text("phone", "#phone", &i.phone()),
        select("mode", "#mode", if f.flight_no.is_empty() { "" } else { "AIR" }),
        text("flight_no", "#flight_no", &f.flight_no),
        text("arrival_date", "#arrival_date", &date(i.arrival(), "%d/%m/%Y")),
        text("last_city", "#last_city", &f.origin_city),
        text("departure_date", "#departure_date", &date(i.departure(), "%d/%m/%Y")),
        select("purpose", "#purpose", purpose_option(&f.purpose)),
        select("accom_type", "#accom_type", accom_option(&f.stay_kind)),
        text("accommodation", "#accommodation", &i.stay()),
    ];
    for q in [
        "d_goods",
        "d_cash",
        "d_unaccompanied",
        "d_refused",
        "d_criminal",
    ] {
        ops.push(radio(q, q, "No"));
    }
    ops
}

/// The Cambodia e-Arrival Card (https://arrival.gov.kh/), mapped from the page rebuilt from the site on 2026-11-01:
/// one page like the TDAC's — dates DD/MM/YYYY, selects by ISO alpha-3, a next-city box, health and declaration
/// Yes / No radios by name (No, the user's default). The CAPTCHA and Submit are the user's.
fn kh_arrival_card(i: &Inputs) -> Vec<Op> {
    let f = i.facts;
    let (given, family) = i.name_parts();
    let nat = iso3(&i.nationality());
    let mut ops = vec![
        text("family_name", "#family_name", &family),
        text("given_name", "#given_name", &given),
        radio("sex", "sex", &i.sex_code()),
        text("dob", "#dob", &date(i.birthday(), "%d/%m/%Y")),
        select("nationality", "#nationality", &nat),
        vault_as("passport_no", "text", "#passport_no", "number", ""),
        vault_as(
            "passport_expiry",
            "text",
            "#passport_expiry",
            "expiry",
            "%d/%m/%Y",
        ),
        select("passport_country", "#passport_country", &nat),
        text("email", "#email", &i.email()),
        text("phone", "#phone", &i.phone()),
        select(
            "mode",
            "#mode",
            if f.flight_no.is_empty() { "" } else { "AIR" },
        ),
        text("flight_no", "#flight_no", &f.flight_no),
        text(
            "arrival_date",
            "#arrival_date",
            &date(i.arrival(), "%d/%m/%Y"),
        ),
        text("last_city", "#last_city", &f.origin_city),
        text("next_city", "#next_city", &i.next_city()),
        text(
            "departure_date",
            "#departure_date",
            &date(i.departure(), "%d/%m/%Y"),
        ),
        select("purpose", "#purpose", purpose_option(&f.purpose)),
        select("accom_type", "#accom_type", accom_option(&f.stay_kind)),
        text("accommodation", "#accommodation", &i.stay()),
    ];
    for q in [
        "h_symptoms",
        "h_travel",
        "d_names",
        "d_refused",
        "d_criminal",
        "d_goods",
    ] {
        ops.push(radio(q, q, "No"));
    }
    ops
}

/// The Philippines eTravel registration (https://etravel.gov.ph/), mapped from the page rebuilt from the site on
/// 2026-11-01: one page — last / first / middle name (the middle name is not in the profile: left for the user),
/// the country of residence (the home address's country, else the nationality), dates DD/MM/YYYY, selects by ISO
/// alpha-3, a next-city box, health and declaration Yes / No radios by name (No, the user's default). The CAPTCHA and
/// Submit are the user's.
fn ph_etravel(i: &Inputs) -> Vec<Op> {
    let f = i.facts;
    let (given, family) = i.name_parts();
    let nat = iso3(&i.nationality());
    let mut ops = vec![
        text("last_name", "#last_name", &family),
        text("first_name", "#first_name", &given),
        radio("sex", "sex", &i.sex_code()),
        text("dob", "#dob", &date(i.birthday(), "%d/%m/%Y")),
        select("nationality", "#nationality", &nat),
        select(
            "country_residence",
            "#country_residence",
            &i.residence_iso3(),
        ),
        vault_as("passport_no", "text", "#passport_no", "number", ""),
        vault_as(
            "passport_expiry",
            "text",
            "#passport_expiry",
            "expiry",
            "%d/%m/%Y",
        ),
        select("passport_country", "#passport_country", &nat),
        text("email", "#email", &i.email()),
        text("phone", "#phone", &i.phone()),
        select(
            "mode",
            "#mode",
            if f.flight_no.is_empty() { "" } else { "AIR" },
        ),
        text("flight_no", "#flight_no", &f.flight_no),
        text(
            "arrival_date",
            "#arrival_date",
            &date(i.arrival(), "%d/%m/%Y"),
        ),
        text("last_city", "#last_city", &f.origin_city),
        text("next_city", "#next_city", &i.next_city()),
        text(
            "departure_date",
            "#departure_date",
            &date(i.departure(), "%d/%m/%Y"),
        ),
        select("purpose", "#purpose", purpose_option(&f.purpose)),
        select("accom_type", "#accom_type", accom_option(&f.stay_kind)),
        text("accommodation", "#accommodation", &i.stay()),
    ];
    for q in [
        "h_symptoms",
        "h_travel",
        "d_names",
        "d_refused",
        "d_criminal",
        "d_goods",
    ] {
        ops.push(radio(q, q, "No"));
    }
    ops
}

/// The UK ETA application (https://www.gov.uk/eta), mapped from the page rebuilt from the site on 2026-11-01. No
/// trip: the application asks for the passport, the home address and the occupation (the profile's job title) — the
/// driver's trip facts are optional and the plan is the same with empty [`Facts`]. The face photo is the user's: the
/// plan fills what comes before it, then stops until the questions after the photo step show (on a one-page form they
/// are there already and the stop passes); the criminality / immigration-history questions are No, the user's own
/// default, reviewed by them before they pay. "Submit and pay" is the user's.
fn uk_eta(i: &Inputs) -> Vec<Op> {
    let (given, family) = i.name_parts();
    let nat = iso3(&i.nationality());
    let mut ops = vec![
        text("family_name", "#family_name", &family),
        text("given_name", "#given_name", &given),
        radio("sex", "sex", &i.sex_code()),
        text("dob", "#dob", &date(i.birthday(), "%d/%m/%Y")),
        select("nationality", "#nationality", &nat),
        vault_as("passport_no", "text", "#passport_no", "number", ""),
        vault_as("passport_expiry", "text", "#passport_expiry", "expiry", "%d/%m/%Y"),
        select("passport_country", "#passport_country", &nat),
        text("email", "#email", &i.email()),
        text("phone", "#phone", &i.phone()),
        text("home_address", "#home_address", &i.home_address()),
        text("occupation", "#occupation", &i.job_title()),
        // the photo step is the user's: the questions after it gate the rest of the plan
        require("history_questions", "input[name=\"d_criminal\"]",
                "the face photo upload is the user's: hand the page over for it, then call form_fill again"),
    ];
    for q in ["d_criminal", "d_refused", "d_terror"] {
        ops.push(radio(q, q, "No"));
    }
    ops
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
        // the landing page: the individual card's "Submit" (elsewhere: not there, skipped)
        choose("start_individual", "Individual", "Submit"),
        wait(2000),
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
    let (_, mobile) = phone_parts(&i.phone());
    let nat = i.nationality().to_uppercase();
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
    let mut ops = vec![
        // the home page: English, then the foreign visitor's card (already past it: not there, skipped)
        choose("language", "", "Languages"),
        choose("language_english", "", "English"),
        wait(800),
        choose("foreign_visitor", "", "Foreign Visitor"),
        wait(2500),
        // --- 1. Personal Information
    ];
    ops.extend(dropdown(
        "nationality",
        "[id^=\"spi_nationality_\"]",
        search,
        "",
        &nat,
        &nat,
    ));
    ops.extend([
        text("full_name", "[id^=\"spi_full_name_\"]", &i.full_name()),
        keys(
            "dob",
            "",
            "[id^=\"spi_dob_\"]",
            &date(i.birthday(), "%d/%m/%Y"),
        ),
        choose_in("sex", "", "[id^=\"spi_gender_\"] input", sex),
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
        choose_in(
            "different_passport",
            "",
            "[id^=\"spi_diff_passport_\"] input",
            "No",
        ),
        text("mobile", "[id^=\"spi_mobile_no_\"]", &mobile),
        text("email", "[id^=\"spi_email_\"]", &i.email()),
        button("next_to_travel", "Next"),
        wait(2500),
        // --- 2. Travel Details
    ]);
    ops.extend(list_pick(
        "arrival_date",
        "[id^=\"std_arrival_date_foreigner_individual\"]",
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
    ops.extend(dropdown(
        "mode",
        "[id^=\"smta_mode_transport_foreigner\"]",
        search,
        "",
        if air { "AIR" } else { "" },
        if air { "AIR" } else { "" },
    ));
    let port = if f.arrival_airport.is_empty() {
        String::new()
    } else {
        format!("{} -", f.arrival_airport)
    };
    ops.extend(dropdown(
        "port_of_entry",
        "[id^=\"smta_place_of_arrival_air_foreigner\"]",
        search,
        "",
        &f.arrival_airport,
        &port,
    ));
    ops.extend(dropdown(
        "air_transport",
        "[id^=\"smta_air_transport_type_foreigner\"]",
        search,
        "",
        if air { "COMMERCIAL" } else { "" },
        if air { "COMMERCIAL FLIGHT" } else { "" },
    ));
    let airline = f.airline.to_uppercase();
    ops.extend(dropdown(
        "airline",
        "[id^=\"smta_flight_name_foreigner\"]",
        search,
        "",
        &airline,
        &airline,
    ));
    ops.push(text(
        "flight_number",
        "[id^=\"smta_flight_no_foreigner\"]",
        &number,
    ));
    ops.extend(dropdown(
        "purpose",
        "[id^=\"smta_purpose_travel_foreigner\"]",
        search,
        "",
        purpose,
        purpose,
    ));
    ops.extend(dropdown(
        "accommodation_type",
        "[id^=\"smta_residence_type_foreigner\"]",
        search,
        "",
        stay_type,
        stay_type,
    ));
    if stay_type == "HOTEL" {
        ops.extend(dropdown(
            "hotel",
            "[id^=\"smta_hotel_name_foreigner\"]",
            search,
            "",
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

/// The Taiwan Arrival Card (https://twac.immigration.gov.tw/ → Submit Arrival Card → START → `/submit`), mapped from
/// the live site on 2026-10-31: step 1 is the user's (an email address, a Cloudflare check and an emailed code); then,
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
    vec![
        // step 1: the user's email goes in and the code is requested (code's part; not there once past this step),
        // then the Cloudflare check and the emailed code are the user's: nothing to fill until the traveller's form shows
        // the landing page: Submit Arrival Card → START → /submit (not there once on the form: skipped)
        choose("submit_card", "", "Submit Arrival Card"),
        wait(800),
        click_sel("start", "#start"),
        wait(1200),
        text("verify_email", "#emailVerify", &i.email()),
        click_sel("send_code", "#send"),
        wait(800),
        require("traveler_form", "#traveler1_englishName",
                "the email verification (Cloudflare check and the code Taiwan emails) is the user's: hand the page over for it, then call form_fill again"),
        // --- Traveler Information
        text("entry_date", "#traveler1_expectEntryDate", &date(i.arrival(), "%d/%m/%Y")),
        text("departure_date", "#traveler1_intendedDepartureDate", &date(i.departure(), "%d/%m/%Y")),
        text("english_name", "#traveler1_englishName", &i.full_name()),
        vault_as("passport_number", "text", "#traveler1_passportNo", "number", ""),
        vault_as("passport_expiry", "text", "#traveler1_passportExpiryDate", "expiry", "%d/%m/%Y"),
        Op::new("sex", json!({"op": "click", "sel": format!("label.btnStyle:has(input[type=radio][value=\"{sex}\"])")}), sex),
        text("dob", "#traveler1_birthDate", &date(i.birthday(), "%d/%m/%Y")),
        pick("nationality", "#traveler1_nation", &nat, &nat),
        radio("visa_type", "visaTypeRadio", visa),
        pick("country_code", "#traveler1_phoneCountryCode", &calling, &calling),
        text("mobile", "#traveler1_phoneNumber", &mobile),
        text("email", "#traveler1_email", &i.email()),
        button("next_to_itinerary", "NEXT"),
        wait(2000),
        // --- Itinerary
        radio("entry_mode", "arrivalTravelModeFlag", if airline.is_empty() { "" } else { "AIR" }),
        pick("flight_code", "#traveler1_arrivalFlightCode", &flight_typed, &flight_code),
        text("flight_number", "#traveler1_arrivalFlightNumber", &number),
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
        choose("start_as_visitor", "I am a Visitor", "Fill in Manually"),
        wait(1500),
        // --- Traveller Details
        choose(
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
        choose("sex", "Sex as indicated in Passport", sex),
    ]);
    ops.extend(date3(
        ["dob_day", "dob_month", "dob_year"],
        "Date of Birth",
        i.birthday(),
    ));
    let nationality = demonym(&i.nationality());
    ops.extend([
        pick(
            "nationality",
            "#traveller-0-input-nationality",
            &nationality,
            &nationality,
        ),
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
        choose(
            "different_name_passport",
            "passport under different name",
            "NO",
        ),
        choose("health_symptoms", "fever", "NO"),
        wait(500),
        choose("health_yellow_fever", "yellow fever", "NO"),
        button("next_to_visit_details", "Next"),
        wait(1200),
        // --- Visit Details
        choose(
            "mode_of_travel",
            "Mode of Travel",
            if airline.is_empty() { "" } else { "AIR" },
        ),
        choose(
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
                choose("accommodation_type", "Type of Accommodation", "HOTEL"),
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
            ops.extend([
                choose("accommodation_type", "Type of Accommodation", "RESIDENTIAL"),
                text("postal_code", "#traveller-0-input-postalCode", &postal),
                text("block_number", "#traveller-0-input-blockNumber", &block),
                text("street_name", "#traveller-0-input-streetName", &street),
            ]);
        }
        "transit" => ops.push(choose(
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

// ---------------------------------------------------------------- the claim forms (`agent::claims` drives them)

/// eClaimsLine's "File a claim" form (Card Benefit Services, https://www.eclaimsline.com — where
/// chasecardbenefits.com sends Chase cardholders), mapped from the portal's claim page on 2026-11-01: one page, native
/// inputs and selects with fixed ids. The facts come from `agent::claims` (the benefit by rule, the date of loss, the
/// amount by arithmetic, the reason in general terms, the supplier and booking reference, the card's last 4 from the
/// charge alert); the cardholder from the profile. A sign-in the portal may put first is the user's (`require`), and
/// "Submit claim" is the user's.
fn eclaims_trip_claim(i: &Inputs) -> Vec<Op> {
    vec![
        require(
            "claim_form",
            "#benefit",
            "the sign-in is the user's: hand the page over for it, then call form_fill again",
        ),
        text("name", "#name", &i.full_name()),
        text("email", "#email", &i.email()),
        text("phone", "#phone", &i.phone()),
        select("card", "#card", &i.arg("card_product")),
        text("last4", "#last4", &i.arg("last4")),
        select("benefit", "#benefit", &i.arg("benefit")),
        text("loss", "#loss", &i.arg("loss_date")),
        text("amount", "#amount", &i.arg("amount")),
        text("reason", "#reason", &i.arg("reason")),
        text("booking", "#booking", &i.arg("booking")),
    ]
}

/// FWD Singapore's online travel claim (https://www.fwd.com.sg/claim-online/travel.html), mapped 2026-11-01: one page.
/// The policy number, the incident and the amount come from `agent::claims` (the delay from the airline's
/// certificate, the benefit by the policy's 6-hour rule); the claimant from the profile; the NRIC / passport from the
/// Vault (never typed from chat). The supporting documents are uploaded by the driver (`browser_upload` needs the
/// file input's ref), and "Submit claim" is the user's.
fn fwd_sg_claim(i: &Inputs) -> Vec<Op> {
    vec![
        require(
            "claim_form",
            "#policy",
            "the sign-in (Singpass or a one-time code) is the user's: hand the page over for it, then call form_fill again",
        ),
        text("policy", "#policy", &i.arg("policy_no")),
        text("name", "#name", &i.full_name()),
        text("email", "#email", &i.email()),
        text("mobile", "#mobile", &i.phone()),
        vault_as("idno", "text", "#idno", "number", ""),
        select("type", "#type", &i.arg("claim_type")),
        text("date", "#date", &i.arg("incident_date")),
        text("what", "#what", &i.arg("what")),
        text("amount", "#amount", &i.arg("amount")),
    ]
}

/// FWD Singapore's travel insurance quote (https://www.fwd.com.sg/travel-insurance/online-quote/), mapped 2026-11-01:
/// the trip (destination, dates, travellers) and "Get quote"; the plans the page then lists are read by the driver,
/// which picks one by the user's rule and clicks its "Select" link.
fn fwd_sg_quote(i: &Inputs) -> Vec<Op> {
    vec![
        text("destination", "#dest", &i.arg("destination")),
        text("departure_date", "#from", &i.arg("depart")),
        text("return_date", "#to", &i.arg("return")),
        text("travellers", "#pax", &i.arg("travellers")),
        button("get_quote", "Get quote"),
        wait(800),
    ]
}

/// FWD Singapore's travel insurance application (the page a plan's "Select" link opens from the quote; path
/// `/fwd_sg_apply…` as the quote links it, 2026-11-01): the insured from the profile, the passport from the Vault. The
/// card is filled by the driver with `browser_fill_secret` after `purchase_confirm` (a Vault card never travels in a
/// plan); the policy-wording declaration and "Pay now" are the user's.
fn fwd_sg_apply(i: &Inputs) -> Vec<Op> {
    vec![
        text("name", "#name", &i.full_name()),
        text("dob", "#dob", &date(i.birthday(), "%Y-%m-%d")),
        text("email", "#email", &i.email()),
        text("mobile", "#mobile", &i.phone()),
        vault_as("passport", "text", "#passport", "number", ""),
    ]
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
        assert_eq!(by("sex")["text"], "FEMALE");
        assert_eq!(by("dob_day")["value"], "12");
        assert_eq!(by("nationality")["value"], "AUSTRALIAN");
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
    }

    /// `OMUSE_FORM_PLAN_OUT=<file> cargo test -p omuse-runtime forms::tests::dump_sg_plan`: the SG plan as JSON, for
    /// running it against the stand-in page in a browser.
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
    }

    /// `OMUSE_FORM_PLAN_OUT=<file>`: the Malaysia plan as JSON, for replaying it against the stand-in in a browser.
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

    /// `OMUSE_FORM_PLAN_OUT_KR=<file>`: the Korea plan as JSON, for replaying it against the stand-ins in a browser.
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
        assert_eq!(by("arrival_date")["value"], "02 NOVEMBER 2026");
        assert_eq!(by("port_of_entry")["value"], "DPS -");
        assert_eq!(by("airline")["value"], "GARUDA INDONESIA");
        assert_eq!(by("flight_number")["value"], "715");
        assert_eq!(by("purpose")["value"], "HOLIDAY/SIGHTSEEING/LEISURE");
        assert_eq!(by("hotel")["type"], "SEMINYAK LAGOON");
        // the certification and Submit are the user's
        assert!(!plan
            .iter()
            .any(|o| o["text"] == "Submit"
                || o["text"].as_str().is_some_and(|t| t.contains("certify"))));
    }

    /// `OMUSE_FORM_PLAN_OUT_ID=<file>`: the Indonesia plan as JSON, for replaying it against the stand-in.
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
        assert_eq!(by("address")["value"], "XINYI PARK HOTEL");
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

    /// `OMUSE_FORM_PLAN_OUT_TW=<file>`: the Taiwan plan as JSON, for replaying it against the stand-in.
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

    /// `OMUSE_FORM_PLAN_OUT_CN=<file>`: the China plan as JSON, for replaying it against the stand-in.
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
            ("https://vjw-lp.digital.go.jp/", "jp-visit-japan-web"),
            ("https://vjw-lp.digital.go.jp/vjw_entry.html", "jp-visit-japan-web"),
            ("https://arrival.gov.kh/", "kh-arrival-card"),
            ("https://etravel.gov.ph/", "ph-etravel"),
            ("https://www.gov.uk/eta", "uk-eta"),
            ("https://www.gov.uk/eta/apply", "uk-eta"),
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
    fn the_tdac_plan_fills_every_known_field_and_answers_no() {
        let facts = Facts {
            flight_no: "TG 476".into(),
            arrival_date: "2026-11-03".into(),
            origin_city: "Sydney".into(),
            origin_country: "Australia".into(),
            stay_name: "Siam Riverside Hotel".into(),
            stay_address: "88 Charoen Krung Road, Bangkok 10500".into(),
            stay_kind: "hotel".into(),
            purpose: "holiday".into(),
            ..Facts::default()
        };
        let p = serde_json::json!({"name_en": "JORDAN LU", "gender": "Female", "birthday": "1985-04-12",
            "nationality": "Australia", "email_personal": "jordan@example.com", "phone": "+61 400 000 000"});
        let i = Inputs {
            facts: &facts,
            profile: &p,
        };
        let ops: Vec<Value> = th_arrival_card(&i).iter().filter_map(Op::to_json).collect();
        let by = |field: &str| {
            ops.iter()
                .find(|o| o["field"] == field)
                .cloned()
                .unwrap_or(Value::Null)
        };
        assert_eq!(by("family_name")["value"], "LU");
        assert_eq!(by("first_name")["value"], "JORDAN");
        assert_eq!(by("sex")["value"], "F");
        assert_eq!(by("dob")["value"], "12/04/1985");
        assert_eq!(by("nationality")["value"], "AUS");
        assert_eq!(by("arrival_date")["value"], "03/11/2026");
        assert_eq!(by("departure_date")["value"], "10/11/2026");
        assert_eq!(by("next_city")["value"], "Sydney");
        assert_eq!(by("purpose")["value"], "HOLIDAY");
        assert_eq!(by("accom_type")["value"], "HOTEL");
        assert!(by("accommodation")["value"]
            .as_str()
            .unwrap()
            .starts_with("Siam Riverside Hotel"));
        assert_eq!(by("passport_no")["vault"]["field"], "number");
        for q in [
            "h_symptoms",
            "h_travel",
            "d_names",
            "d_refused",
            "d_criminal",
            "d_goods",
        ] {
            assert_eq!(by(q)["value"], "No", "{q}");
        }
        assert!(form_for("https://tdac.immigration.go.th/")
            .is_some_and(|(n, _)| n == "th-arrival-card"));
    }

    #[test]
    fn the_twac_plan_reaches_the_users_step_before_stopping() {
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
        let i = Inputs {
            facts: &facts,
            profile: &p,
        };
        let ops: Vec<Value> = tw_arrival_card(&i).iter().filter_map(Op::to_json).collect();
        let at = |field: &str| {
            ops.iter()
                .position(|o| o["field"] == field)
                .unwrap_or(usize::MAX)
        };
        // the email typed and the code requested (code's part of step 1) before the stop for the user's part
        assert!(
            at("submit_card") < at("start") && at("start") < at("verify_email"),
            "{ops:?}"
        );
        assert!(
            at("verify_email") < at("send_code") && at("send_code") < at("traveler_form"),
            "{ops:?}"
        );
        assert_eq!(ops[at("verify_email")]["value"], "jordan@example.com");
        assert_eq!(ops[at("traveler_form")]["op"], "require");
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
    fn the_visit_japan_web_plan_waits_for_the_sign_in_then_fills_the_entry() {
        let facts = one_page_facts(
            "NH 880",
            "Shinjuku Granbell Inn",
            "2-14-5 Kabukicho, Shinjuku-ku, Tokyo 160-0021",
        );
        let p = profile();
        let plan = plan_of(jp_visit_japan_web, &facts, &p);
        // the stop for the user's sign-in comes before every field; its reason reads as the driver expects
        assert_eq!(plan[0]["op"], "require");
        assert_eq!(plan[0]["sel"], "#family_name");
        assert!(plan[0]["why"]
            .as_str()
            .unwrap()
            .starts_with("the sign-in to your Visit Japan Web account"));
        assert!(plan[0]["why"]
            .as_str()
            .unwrap()
            .contains(" is the user's: hand the page over for it, then call form_fill again"));
        assert!(index(&plan, "entry_form") < index(&plan, "family_name"));
        assert_eq!(field(&plan, "family_name")["value"], "LU");
        assert_eq!(field(&plan, "given_name")["value"], "JORDAN");
        assert_eq!(field(&plan, "sex")["value"], "F");
        assert_eq!(field(&plan, "dob")["value"], "12/04/1985");
        assert_eq!(field(&plan, "nationality")["value"], "AUS");
        assert_eq!(field(&plan, "passport_country")["value"], "AUS");
        assert_eq!(field(&plan, "passport_no")["vault"]["field"], "number");
        assert_eq!(
            field(&plan, "passport_expiry")["vault"]["date_fmt"],
            "%d/%m/%Y"
        );
        assert_eq!(field(&plan, "mode")["value"], "AIR");
        assert_eq!(field(&plan, "flight_no")["value"], "NH 880");
        assert_eq!(field(&plan, "arrival_date")["value"], "03/11/2026");
        assert_eq!(field(&plan, "departure_date")["value"], "10/11/2026");
        assert_eq!(field(&plan, "last_city")["value"], "Sydney");
        assert_eq!(field(&plan, "purpose")["value"], "HOLIDAY");
        assert_eq!(field(&plan, "accom_type")["value"], "HOTEL");
        assert_eq!(
            field(&plan, "accommodation")["value"],
            "Shinjuku Granbell Inn, 2-14-5 Kabukicho, Shinjuku-ku, Tokyo 160-0021"
        );
        // the customs questions: No; the page has no next-city or health fields
        for q in [
            "d_goods",
            "d_cash",
            "d_unaccompanied",
            "d_refused",
            "d_criminal",
        ] {
            assert_eq!(field(&plan, q)["value"], "No", "{q}");
        }
        assert!(plan
            .iter()
            .all(|o| o["field"] != "next_city" && o["field"] != "h_symptoms"));
        // the security check and Register stay the user's
        assert!(!plan
            .iter()
            .any(|o| o["field"] == "captcha" || o["text"] == "Register"));
        assert!(form_for("https://vjw-lp.digital.go.jp/")
            .is_some_and(|(n, _)| n == "jp-visit-japan-web"));
    }

    #[test]
    fn the_cambodia_plan_fills_every_field_and_answers_no() {
        let facts = one_page_facts(
            "SQ 166",
            "Riverside Palace Hotel",
            "51 Sisowath Quay, Phnom Penh",
        );
        let p = profile();
        let plan = plan_of(kh_arrival_card, &facts, &p);
        assert!(!plan.iter().any(|o| o["op"] == "require"));
        assert_eq!(field(&plan, "family_name")["value"], "LU");
        assert_eq!(field(&plan, "given_name")["value"], "JORDAN");
        assert_eq!(field(&plan, "sex")["value"], "F");
        assert_eq!(field(&plan, "dob")["value"], "12/04/1985");
        assert_eq!(field(&plan, "nationality")["value"], "AUS");
        assert_eq!(field(&plan, "passport_no")["vault"]["field"], "number");
        assert_eq!(field(&plan, "email")["value"], "jordan.lu@example.com");
        assert_eq!(field(&plan, "mode")["value"], "AIR");
        assert_eq!(field(&plan, "flight_no")["value"], "SQ 166");
        assert_eq!(field(&plan, "arrival_date")["value"], "03/11/2026");
        assert_eq!(field(&plan, "last_city")["value"], "Sydney");
        assert_eq!(field(&plan, "next_city")["value"], "Sydney"); // the default: where the flight came from
        assert_eq!(field(&plan, "departure_date")["value"], "10/11/2026"); // arrival + 7
        assert_eq!(field(&plan, "purpose")["value"], "HOLIDAY");
        assert_eq!(field(&plan, "accom_type")["value"], "HOTEL");
        assert_eq!(
            field(&plan, "accommodation")["value"],
            "Riverside Palace Hotel, 51 Sisowath Quay, Phnom Penh"
        );
        for q in [
            "h_symptoms",
            "h_travel",
            "d_names",
            "d_refused",
            "d_criminal",
            "d_goods",
        ] {
            assert_eq!(field(&plan, q)["value"], "No", "{q}");
        }
        assert!(!plan
            .iter()
            .any(|o| o["field"] == "captcha" || o["text"] == "Submit"));
        assert!(form_for("https://arrival.gov.kh/").is_some_and(|(n, _)| n == "kh-arrival-card"));
    }

    #[test]
    fn the_etravel_plan_splits_the_name_and_knows_the_country_of_residence() {
        let facts = one_page_facts(
            "PR 210",
            "Makati Palm Suites",
            "1200 Ayala Avenue, Makati City, Metro Manila",
        );
        let p = profile();
        let plan = plan_of(ph_etravel, &facts, &p);
        assert!(!plan.iter().any(|o| o["op"] == "require"));
        assert_eq!(field(&plan, "last_name")["value"], "LU");
        assert_eq!(field(&plan, "first_name")["value"], "JORDAN");
        // no middle name in the profile: the box is left for the user, never guessed
        assert!(plan.iter().all(|o| o["field"] != "middle_name"));
        assert_eq!(field(&plan, "sex")["value"], "F");
        assert_eq!(field(&plan, "nationality")["value"], "AUS");
        assert_eq!(field(&plan, "country_residence")["value"], "AUS"); // "…, Australia" in the home address
        assert_eq!(field(&plan, "passport_country")["value"], "AUS");
        assert_eq!(
            field(&plan, "passport_expiry")["vault"]["date_fmt"],
            "%d/%m/%Y"
        );
        assert_eq!(field(&plan, "mode")["value"], "AIR");
        assert_eq!(field(&plan, "flight_no")["value"], "PR 210");
        assert_eq!(field(&plan, "arrival_date")["value"], "03/11/2026");
        assert_eq!(field(&plan, "next_city")["value"], "Sydney");
        assert_eq!(field(&plan, "departure_date")["value"], "10/11/2026");
        assert_eq!(field(&plan, "purpose")["value"], "HOLIDAY");
        assert_eq!(field(&plan, "accom_type")["value"], "HOTEL");
        assert!(field(&plan, "accommodation")["value"]
            .as_str()
            .unwrap()
            .starts_with("Makati Palm Suites"));
        for q in [
            "h_symptoms",
            "h_travel",
            "d_names",
            "d_refused",
            "d_criminal",
            "d_goods",
        ] {
            assert_eq!(field(&plan, q)["value"], "No", "{q}");
        }
        assert!(!plan
            .iter()
            .any(|o| o["field"] == "captcha" || o["text"] == "Submit"));
        // a home address without a known country: the residence falls back to the nationality
        let p2 = json!({"name_en": "Jordan Lu", "nationality": "Japan", "address_home": "1-1 Chiyoda, Chiyoda-ku"});
        let plan2 = plan_of(ph_etravel, &facts, &p2);
        assert_eq!(field(&plan2, "country_residence")["value"], "JPN");
        assert!(form_for("https://etravel.gov.ph/").is_some_and(|(n, _)| n == "ph-etravel"));
    }

    #[test]
    fn the_uk_eta_plan_needs_no_trip_and_stops_for_the_photo_before_the_questions() {
        let p = json!({"name_en": "Jordan Lu", "gender": "Female", "birthday": "1985-04-12", "nationality": "Australia",
               "email_personal": "jordan.lu@example.com", "phone": "+61 400 000 000",
               "address_home": "12 Harbour St, Sydney NSW 2000, Australia", "job_title": "Software engineer"});
        // empty facts: the ETA has no trip section, and the plan is complete without one
        let plan = plan_of(uk_eta, &Facts::default(), &p);
        assert_eq!(field(&plan, "family_name")["value"], "LU");
        assert_eq!(field(&plan, "given_name")["value"], "JORDAN");
        assert_eq!(field(&plan, "sex")["value"], "F");
        assert_eq!(field(&plan, "dob")["value"], "12/04/1985");
        assert_eq!(field(&plan, "nationality")["value"], "AUS");
        assert_eq!(field(&plan, "passport_country")["value"], "AUS");
        assert_eq!(field(&plan, "passport_no")["vault"]["field"], "number");
        assert_eq!(
            field(&plan, "passport_expiry")["vault"]["date_fmt"],
            "%d/%m/%Y"
        );
        assert_eq!(field(&plan, "email")["value"], "jordan.lu@example.com");
        assert_eq!(field(&plan, "phone")["value"], "+61 400 000 000");
        assert_eq!(
            field(&plan, "home_address")["value"],
            "12 Harbour St, Sydney NSW 2000, Australia"
        );
        assert_eq!(field(&plan, "occupation")["value"], "Software engineer");
        // no trip field is in the plan at all
        assert!(plan.iter().all(|o| {
            ![
                "flight_no",
                "arrival_date",
                "departure_date",
                "accommodation",
                "mode",
            ]
            .contains(&o["field"].as_str().unwrap_or(""))
        }));
        // the stop for the user's photo step gates the questions after it, and the photo itself is never touched
        let stop = field(&plan, "history_questions");
        assert_eq!(stop["op"], "require");
        assert_eq!(stop["sel"], "input[name=\"d_criminal\"]");
        assert!(stop["why"]
            .as_str()
            .unwrap()
            .starts_with("the face photo upload is the user's: hand the page over for it, then call form_fill again"));
        assert!(index(&plan, "occupation") < index(&plan, "history_questions"));
        for q in ["d_criminal", "d_refused", "d_terror"] {
            assert!(index(&plan, "history_questions") < index(&plan, q), "{q}");
            assert_eq!(field(&plan, q)["value"], "No", "{q}");
        }
        assert!(!plan.iter().any(
            |o| o["field"] == "photo" || o["text"].as_str().is_some_and(|t| t.contains("pay"))
        ));
        // the same plan with the trip facts given: the ETA ignores them
        let with_trip = plan_of(
            uk_eta,
            &one_page_facts(
                "QF 1",
                "Covent Garden Court Hotel",
                "8 Bedford Street, London WC2E 9ED",
            ),
            &p,
        );
        assert_eq!(with_trip, plan);
        // no job title in the profile: the box stays for the user
        let p2 = json!({"name_en": "Jordan Lu", "nationality": "Australia"});
        assert!(plan_of(uk_eta, &Facts::default(), &p2)
            .iter()
            .all(|o| o["field"] != "occupation"));
        assert!(form_for("https://www.gov.uk/eta").is_some_and(|(n, _)| n == "uk-eta"));
    }

    #[test]
    fn the_claim_forms_fill_their_fixed_ids_from_the_claim_facts() {
        let p = profile();
        let facts = Facts::from_args(
            &json!({"benefit": "Trip Interruption", "loss_date": "2026-10-24", "amount": "1050.00",
            "reason": "injury of the cardholder", "booking": "United Airlines QX7P2L; Park Hyatt Tokyo HY-55821",
            "card_product": "Chase Sapphire Reserve", "last4": "4321"}),
        );
        let i = Inputs {
            facts: &facts,
            profile: &p,
        };
        assert_eq!(i.arg("benefit"), "Trip Interruption");
        let plan: Vec<Value> = eclaims_trip_claim(&i)
            .iter()
            .filter_map(Op::to_json)
            .collect();
        let by = |field: &str| {
            plan.iter()
                .find(|o| o["field"] == field)
                .cloned()
                .unwrap_or(Value::Null)
        };
        assert_eq!(by("claim_form")["op"], "require"); // a sign-in first is the user's
        assert_eq!(
            (by("name")["sel"].clone(), by("name")["value"].clone()),
            (json!("#name"), json!("JORDAN LU"))
        );
        assert_eq!(
            (
                by("benefit")["op"].clone(),
                by("benefit")["sel"].clone(),
                by("benefit")["value"].clone()
            ),
            (
                json!("select"),
                json!("#benefit"),
                json!("Trip Interruption")
            )
        );
        assert_eq!(by("loss")["value"], "2026-10-24");
        assert_eq!(by("amount")["value"], "1050.00");
        assert_eq!(by("last4")["value"], "4321");
        assert_eq!(by("card")["value"], "Chase Sapphire Reserve");
        assert!(!plan.iter().any(|o| o["text"] == "Submit claim"));
        // an unknown fact leaves its field for the user
        let none = Facts::from_args(&json!({"benefit": "Trip Cancellation"}));
        let plan: Vec<Value> = eclaims_trip_claim(&Inputs {
            facts: &none,
            profile: &p,
        })
        .iter()
        .filter_map(Op::to_json)
        .collect();
        assert!(!plan
            .iter()
            .any(|o| o["field"] == "amount" || o["field"] == "loss"));
        // FWD Singapore's claim: the passport from the Vault, never a value in the plan
        let facts = Facts::from_args(
            &json!({"policy_no": "TI-SG-2026-55123", "claim_type": "Travel delay", "incident_date": "2026-10-29",
            "what": "SQ 636 delayed 7 h 15 min", "amount": "100.00"}),
        );
        let plan: Vec<Value> = fwd_sg_claim(&Inputs {
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
        assert_eq!(by("policy")["value"], "TI-SG-2026-55123");
        assert_eq!(
            (
                by("idno")["sel"].clone(),
                by("idno")["vault"]["field"].clone()
            ),
            (json!("#idno"), json!("number"))
        );
        assert!(by("idno").get("value").is_none());
        assert_eq!(
            (
                by("type")["value"].clone(),
                by("date")["value"].clone(),
                by("amount")["value"].clone()
            ),
            (json!("Travel delay"), json!("2026-10-29"), json!("100.00"))
        );
        // the quote and the application
        let facts = Facts::from_args(
            &json!({"destination": "Japan", "depart": "2026-11-24", "return": "2026-12-01", "travellers": "1"}),
        );
        let plan: Vec<Value> = fwd_sg_quote(&Inputs {
            facts: &facts,
            profile: &p,
        })
        .iter()
        .filter_map(Op::to_json)
        .collect();
        assert_eq!(
            plan[0],
            json!({"op": "text", "sel": "#dest", "value": "Japan", "field": "destination"})
        );
        assert!(plan
            .iter()
            .any(|o| o["op"] == "button" && o["text"] == "Get quote"));
        let plan: Vec<Value> = fwd_sg_apply(&Inputs {
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
        assert_eq!(by("dob")["value"], "1985-04-12");
        assert_eq!(by("passport")["vault"]["field"], "number");
        // no card field and no declaration in the plan: the card goes through browser_fill_secret, the box is the user's
        assert!(!plan
            .iter()
            .any(|o| o["sel"] == "#card" || o["sel"] == "#agree" || o["sel"] == "#cvc"));
        assert_eq!(
            form_for("https://www.eclaimsline.com/").map(|(n, _)| n),
            Some("csr-trip-claim")
        );
        assert_eq!(
            form_for("https://www.fwd.com.sg/claim-online/travel.html").map(|(n, _)| n),
            Some("fwd-sg-claim")
        );
        assert_eq!(
            form_for("https://www.fwd.com.sg/travel-insurance/online-quote/").map(|(n, _)| n),
            Some("fwd-sg-quote")
        );
        assert_eq!(
            form_for("https://www.fwd.com.sg/fwd_sg_apply.html?plan=Business").map(|(n, _)| n),
            Some("fwd-sg-apply")
        );
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
