//! The known pages of the claims, the insurance purchase, the delayed-bag lookup and the Hong Kong departure tax
//! refund (`agent::claims`, `agent::bagtrace`, `agent::apdt` drive them), each mapped from the REAL site (2026-11-02):
//! one function per page, fixed selectors, every value from the facts, the profile or the Vault. What lies behind a
//! step that is the user's (an account's sign-in, an eligibility declaration, a document photo, a server-side policy
//! check) is not mapped and never guessed: the machine hands over, or stops, there.

use chrono::{Datelike, NaiveDate};
use serde_json::json;

use super::{click_sel, phone_parts, text, vault_as, wait, Inputs, Op};

/// Click the `nth` visible match of `sel`.
fn click_nth(field: &'static str, sel: &str, nth: usize) -> Op {
    Op::new(field, json!({"op": "click", "sel": sel, "nth": nth}), "x")
}

/// A real mouse click on the `nth` visible match of `sel` (`tap`: the broker clicks at its centre) — for components
/// that ignore a script's click.
fn tap_nth(field: &'static str, sel: &str, nth: usize) -> Op {
    Op::new(field, json!({"op": "tap", "sel": sel, "nth": nth}), "x")
}

/// A real mouse click on the visible match of `sel` whose own text is (starts with, contains) `text`.
fn tap_text(field: &'static str, sel: &str, text: &str) -> Op {
    Op::new(field, json!({"op": "tap", "sel": sel, "text": text}), text)
}

/// Stop the plan unless `sel` shows within `wait_ms` (a single-page app that draws its form late).
fn require_within(field: &'static str, sel: &str, why: &str, wait_ms: u64) -> Op {
    Op::new(
        field,
        json!({"op": "require", "sel": sel, "why": why, "wait_ms": wait_ms}),
        "x",
    )
}

/// Click the visible match of `sel` whose own text is (starts with, contains) `text`.
fn click_text(field: &'static str, sel: &str, text: &str) -> Op {
    Op::new(
        field,
        json!({"op": "click", "sel": sel, "text": text}),
        text,
    )
}

// ---------------------------------------------------------------- Card Claim Center (the Chase Sapphire claims)

/// Card Claim Center's sign-in page (https://cardclaimcenter.com/internal/log-in — where www.eclaimsline.com redirects
/// since 2026; Card Benefit Services, Chase's benefit administrator): the account's email from the profile. The
/// password and "LOG IN" (or creating the account) are the user's; the claim pages behind the sign-in are not mapped.
pub(super) fn cardclaim_login(i: &Inputs) -> Vec<Op> {
    vec![wait(1500), text("email", "input[type=email]", &i.email())]
}

// ---------------------------------------------------------------- FWD Singapore

/// FWD Singapore's online travel claim (https://www.fwd.com.sg/claim-online/travel.html → `/claim-online/
/// travel-insurance/quickQuestions`), step 1 of 3, "who is this claim for?": no sign-in; inputs by their fixed names —
/// the claimant as on the NRIC (the profile's passport name), the NRIC / FIN from the Vault (never typed from chat), the
/// Singapore mobile number (8 digits: a number of another country is left for the user), the email and the policy
/// number. "next" checks the policy on FWD's server; steps 2 (payment: the user's bank account) and 3 (the incident,
/// the documents, Submit) open only for a real policy and are not mapped: they are the user's, with the facts the
/// machine computed in the hand-over.
pub(super) fn fwd_sg_claim(i: &Inputs) -> Vec<Op> {
    let (code, number) = phone_parts(&i.phone());
    let mobile = if code == "65" { number } else { String::new() };
    vec![
        wait(1500),
        text(
            "claimant_name",
            "input[name='claimant name']",
            &i.full_name(),
        ),
        vault_as("nric_fin", "text", "input[name='NRIC/FIN']", "number", ""),
        text("mobile", "input[name='mobile']", &mobile),
        text("email", "input[name='email']", &i.email()),
        text(
            "policy_no",
            "input[name='policy number']",
            &i.arg("policy_no"),
        ),
    ]
}

/// The visible list of an open Vuetify (0.x) select on FWD's quote page.
const FWD_OPTION: &str = ".menuable__content__active a.list__tile";
/// The open date picker's header arrows (← →) and its day buttons.
const FWD_PICKER_ARROWS: &str = ".picker--date__header-selector button";
const FWD_PICKER_DAYS: &str = ".picker--date__table button";

/// FWD's travel regions (the quote's "travelling to" list) for a destination country, by its ISO code: Asean, Asia
/// (inc Aust & NZ), Worldwide (the USA), else Worldwide Excluding USA; "" for a country the table cannot place (left
/// for the user — never a guess).
pub fn fwd_region(country: &str) -> &'static str {
    let iso = crate::places::iso_of(country);
    match iso {
        "" => "",
        "BN" | "KH" | "ID" | "LA" | "MY" | "MM" | "PH" | "TH" | "VN" => "Asean",
        "JP" | "KR" | "CN" | "HK" | "MO" | "TW" | "IN" | "LK" | "MV" | "NP" | "BD" | "PK"
        | "MN" | "BT" | "AU" | "NZ" => "Asia (inc Aust & NZ)",
        "US" | "PR" | "GU" | "VI" | "AS" | "MP" => "Worldwide",
        "SG" => "",
        _ => "Worldwide Excluding USA",
    }
}

/// The open date picker's month heading ("December 2026").
const FWD_PICKER_MONTH: &str = ".picker--date__header-selector-date strong";

/// A date picked on the open picker: its month reached by the arrows whatever month it opened on (`seek`: the quote
/// page remembers an earlier quote's dates), then the day — real mouse clicks: the picker's menu closes on a script's
/// click (mapped 2026-11-02).
fn picker_day(field: &'static str, day: NaiveDate) -> Vec<Op> {
    vec![
        Op::new(
            field,
            json!({"op": "seek", "sel": FWD_PICKER_ARROWS, "next": 1, "prev": 0, "max": 15,
                   "until": {"sel": FWD_PICKER_MONTH, "text": day.format("%B %Y").to_string()}}),
            "x",
        ),
        tap_text(field, FWD_PICKER_DAYS, &day.day().to_string()),
        wait(600),
    ]
}

/// FWD Singapore's travel insurance quote (https://www.fwd.com.sg/travel-insurance/online-quote/ → `quickQuestions`,
/// then `summary`), mapped from the live page on 2026-11-02: three Vuetify selects found by their labels ("plan type":
/// Single; "who": just myself, or a group; "region": by the destination, [`fwd_region`]), the trip's dates on two
/// read-only date pickers (each month reached by its heading, whatever month the picker opened on), "next" (`#TI_GetQuotepage_Next_1`), the promo code skipped and the discount questions declined
/// (a quote changes nothing). The summary lists the plans (read by the machine). Stage `pick`: the chosen plan's
/// button (`#TI_QuoteSummarypage_<Plan>_Buy`); FWD then asks the eligibility statements, which are the user's.
pub(super) fn fwd_sg_quote(i: &Inputs) -> Vec<Op> {
    if i.arg("stage") == "pick" {
        let plan = i.arg("plan");
        if plan.is_empty() || !plan.chars().all(|c| c.is_ascii_alphabetic()) {
            return Vec::new();
        }
        return vec![
            click_sel("plan", &format!("#TI_QuoteSummarypage_{plan}_Buy")),
            wait(3000),
        ];
    }
    let d = |k: &str| NaiveDate::parse_from_str(&i.arg(k), "%Y-%m-%d").ok();
    let who = match i.arg("travellers").parse::<u32>().unwrap_or(1) {
        0 | 1 => "just myself",
        _ => "group",
    };
    let mut ops = vec![
        // the Vue app draws its questions late
        require_within(
            "quote_form",
            "[role=combobox]",
            "FWD's quote questions did not open",
            15_000,
        ),
        Op::new(
            "plan_type",
            json!({"op": "click", "scope": "plan type", "sel": "[role=combobox]"}),
            "x",
        ),
        super::option("plan_type", FWD_OPTION, "Single"),
        Op::new(
            "who",
            json!({"op": "click", "scope": "who", "sel": "[role=combobox]"}),
            who,
        ),
        super::option("who", FWD_OPTION, who),
    ];
    let region = fwd_region(&i.arg("destination"));
    ops.extend([
        Op::new(
            "region",
            json!({"op": "click", "scope": "region", "sel": "[role=combobox]"}),
            region,
        ),
        super::option("region", FWD_OPTION, region),
    ]);
    if let (Some(dep), Some(ret)) = (d("depart"), d("return")) {
        ops.push(tap_nth("depart", "form input[readonly]", 0));
        ops.extend(picker_day("depart", dep));
        ops.push(tap_nth("return", "form input[readonly]", 1));
        ops.extend(picker_day("return", ret));
    }
    ops.extend([
        click_sel("next", "#TI_GetQuotepage_Next_1"),
        wait(2500),
        click_sel("skip_promo", "#TI_GetQuotepage_Promo_Skip"),
        wait(2000),
        click_sel("no_discount", "#TI_GetQuotepage_Discount_No"),
        // the summary's plan table, which the machine reads next (the page may be slow)
        Op::new(
            "plans",
            json!({"op": "await", "sel": "#plan-table .coverage-plan__subheader h5", "wait_ms": 20_000}),
            "x",
        ),
        wait(1500),
    ]);
    ops
}

// ---------------------------------------------------------------- WorldTracer (mybag.aero)

/// WorldTracer's passenger site, "Manage your delayed baggage report" (https://mybag.aero/baggage/#/pax/<airline>/
/// en-gb/delayed/manage-bag; the airline's slug from `agent::bagtrace::airline_slug`), mapped 2026-11-02: the PIR's
/// reference (`#mngRptRefNoLTxt`) and the family name (`#mngRptLastNameTxt`), then "LOG IN" (`#mngRptLoginBtn`) — a
/// lookup changes nothing. The answer (an error box, or the report's page) is read by `agent::bagtrace`. The family
/// name is the one the PIR or the user gave (`family_name`), else the profile's.
pub(super) fn mybag_manage(i: &Inputs) -> Vec<Op> {
    let file_ref = i.arg("file_ref");
    let family = match i.arg("family_name") {
        f if f.is_empty() => i.name_parts().1,
        f => f.to_uppercase(),
    };
    let mut ops = vec![
        // the single-page app draws the form several seconds after the page loads (mapped 2026-11-02: ~7 s)
        require_within(
            "trace_form",
            "#mngRptRefNoLTxt",
            "the airline's baggage report page did not open",
            15_000,
        ),
        text("file_ref", "#mngRptRefNoLTxt", &file_ref),
        text("family_name", "#mngRptLastNameTxt", &family),
    ];
    if !file_ref.is_empty() && !family.is_empty() {
        ops.extend([click_sel("log_in", "#mngRptLoginBtn"), wait(5000)]);
    }
    ops
}

// ---------------------------------------------------------------- the Hong Kong departure tax refund

/// The Airport Authority's Refund Application Platform (https://app.apdtrefund.hk/hkTaxMobile/?lang=en; a Vue app,
/// paths `/hkTaxMobile/home`, `/hkTaxMobile/process/upload`), mapped 2026-11-02, up to the first document: the
/// anti-scam notice closed, both statement boxes (`span.check-img`, the user asked OMuse to apply for them), "Start",
/// "New Application", the entry mode (Air: two one-way tickets) and its "Confirm" — the page then asks for the travel
/// document's photo, which is the user's (never a made-up file); the machine stops there.
pub(super) fn apdt_start(_: &Inputs) -> Vec<Op> {
    // each step waits for the next page's element (the app's routes draw late; a Queue-it waiting room may come first)
    let until = |field: &'static str, sel: &str, text: &str, why: &str, ms: u64| {
        Op::new(
            field,
            json!({"op": "require", "sel": sel, "text": text, "why": why, "wait_ms": ms}),
            "x",
        )
    };
    vec![
        require_within(
            "notes",
            "span.check-img",
            "the platform's notes did not open (its waiting room may be holding the page)",
            20_000,
        ),
        wait(1500),
        click_text("close_notice", "div.button, button", "Close"),
        wait(800),
        click_nth("pics_agree", "span.check-img", 0),
        click_nth("pics_declare", "span.check-img", 1),
        click_text("start", "div.button", "Start"),
        until(
            "home",
            "div.btn",
            "New Application",
            "the platform's home did not open",
            15_000,
        ),
        click_text("new_application", "div.btn", "New Application"),
        until(
            "entry_mode",
            "div.border",
            "Air",
            "the application's entry mode did not open",
            15_000,
        ),
        click_text("entry_air", "div.border", "Air"),
        wait(1200),
        click_text("confirm_entry", "div.button", "Confirm"),
        until(
            "travel_document",
            "",
            "Click here to upload",
            "the platform did not reach its document upload",
            15_000,
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::super::{form_for, Facts};
    use super::*;
    use serde_json::Value;

    fn plan(f: fn(&Inputs) -> Vec<Op>, args: Value, profile: Value) -> Vec<Value> {
        let facts = Facts::from_args(&args);
        f(&Inputs {
            facts: &facts,
            profile: &profile,
        })
        .iter()
        .filter_map(Op::to_json)
        .collect()
    }

    fn by<'a>(p: &'a [Value], field: &str) -> Vec<&'a Value> {
        p.iter().filter(|o| o["field"] == field).collect()
    }

    fn sg() -> Value {
        json!({"name_en": "Tan Wei Ming", "email_personal": "weiming@example.com", "phone": "+65 9123 4567"})
    }

    #[test]
    fn the_real_pages_have_their_functions() {
        let name = |u: &str| form_for(u).map(|f| f.0);
        assert_eq!(
            name("https://cardclaimcenter.com/internal/log-in?utm_source=eclaimsline"),
            Some("csr-trip-claim")
        );
        assert_eq!(
            name("https://www.fwd.com.sg/claim-online/travel-insurance/quickQuestions"),
            Some("fwd-sg-claim")
        );
        assert_eq!(
            name("https://www.fwd.com.sg/travel-insurance/online-quote/summary"),
            Some("fwd-sg-quote")
        );
        assert_eq!(
            name("https://mybag.aero/baggage/#/pax/ana/en-gb/delayed/manage-bag"),
            Some("worldtracer-manage")
        );
        assert_eq!(
            name("https://app.apdtrefund.hk/hkTaxMobile/?lang=en"),
            Some("hk-apdt-start")
        );
        // the old addresses: eClaimsLine's own claim page and WorldTracer's tracing page are gone (404 / redirect)
        assert_eq!(name("https://www.eclaimsline.com/"), None);
        assert_eq!(
            name("https://wtrweb.worldtracer.aero/WorldTracerWeb/bagtrace.html"),
            None
        );
    }

    #[test]
    fn the_sign_in_gets_the_email_only() {
        let p = plan(cardclaim_login, json!({}), sg());
        let typed: Vec<&Value> = p.iter().filter(|o| o["op"] == "text").collect();
        assert_eq!(typed.len(), 1);
        assert_eq!(typed[0]["sel"], "input[type=email]");
        assert_eq!(typed[0]["value"], "weiming@example.com");
        assert!(!p.iter().any(|o| o.to_string().contains("password")));
    }

    #[test]
    fn fwds_claim_step_one_is_filled_by_its_input_names() {
        let p = plan(fwd_sg_claim, json!({"policy_no": "PNTR20123456"}), sg());
        let v = |f: &str| by(&p, f)[0].clone();
        assert_eq!(v("claimant_name")["sel"], "input[name='claimant name']");
        assert_eq!(v("claimant_name")["value"], "TAN WEI MING");
        assert_eq!(v("mobile")["value"], "91234567");
        assert_eq!(v("email")["value"], "weiming@example.com");
        assert_eq!(v("policy_no")["value"], "PNTR20123456");
        // the NRIC / FIN from the Vault: never a value in the plan
        assert_eq!(v("nric_fin")["vault"]["field"], "number");
        assert!(v("nric_fin").get("value").is_none());
        // no sign-in on the real page, and nothing pressed
        assert!(!p
            .iter()
            .any(|o| o["op"] == "require" || o["op"] == "button"));
        // a mobile number of another country is left for the user, never cut to fit
        let hk = plan(
            fwd_sg_claim,
            json!({}),
            json!({"name_en": "A B", "phone": "+852 9123 4567"}),
        );
        assert!(by(&hk, "mobile").is_empty());
    }

    #[test]
    fn fwds_quote_picks_the_trip_on_its_selects_and_pickers() {
        let p = plan(
            fwd_sg_quote,
            json!({"destination": "Japan", "depart": "2026-11-25", "return": "2026-12-02",
                   "travellers": "1", "today": "2026-11-01"}),
            sg(),
        );
        let opts: Vec<&str> = p
            .iter()
            .filter(|o| o["op"] == "option")
            .map(|o| o["value"].as_str().unwrap())
            .collect();
        assert_eq!(opts, ["Single", "just myself", "Asia (inc Aust & NZ)"]);
        // each date: its month by the picker's heading (whatever month it opened on), then the day
        let months: Vec<&str> = p
            .iter()
            .filter(|o| o["op"] == "seek")
            .map(|o| o["until"]["text"].as_str().unwrap())
            .collect();
        assert_eq!(months, ["November 2026", "December 2026"]);
        let days: Vec<&str> = p
            .iter()
            .filter(|o| o["sel"] == FWD_PICKER_DAYS)
            .map(|o| o["text"].as_str().unwrap())
            .collect();
        assert_eq!(days, ["25", "2"]);
        assert!(p.iter().any(|o| o["sel"] == "#TI_GetQuotepage_Next_1"));
        // the plan's button by the plan's name, nothing else
        let pick = plan(
            fwd_sg_quote,
            json!({"stage": "pick", "plan": "Business"}),
            sg(),
        );
        assert_eq!(pick[0]["sel"], "#TI_QuoteSummarypage_Business_Buy");
        assert!(plan(fwd_sg_quote, json!({"stage": "pick", "plan": "x\"]"}), sg()).is_empty());
    }

    #[test]
    fn the_region_comes_from_the_destination_or_is_left_blank() {
        assert_eq!(fwd_region("Japan"), "Asia (inc Aust & NZ)");
        assert_eq!(fwd_region("Thailand"), "Asean");
        assert_eq!(fwd_region("United States"), "Worldwide");
        assert_eq!(fwd_region("France"), "Worldwide Excluding USA");
        assert_eq!(fwd_region("Atlantis"), "");
        let p = plan(fwd_sg_quote, json!({"destination": "Atlantis"}), sg());
        assert!(by(&p, "region").is_empty());
    }

    #[test]
    fn mybag_takes_the_reference_and_the_family_name_then_looks_up() {
        // the profile writes the name given names first: its last word is the family name
        let p = plan(
            mybag_manage,
            json!({"file_ref": "NRTNH12345"}),
            json!({"name_en": "Wei Ming Tan"}),
        );
        assert_eq!(by(&p, "file_ref")[0]["sel"], "#mngRptRefNoLTxt");
        assert_eq!(by(&p, "family_name")[0]["value"], "TAN");
        assert_eq!(by(&p, "log_in")[0]["sel"], "#mngRptLoginBtn");
        let given = plan(
            mybag_manage,
            json!({"file_ref": "NRTNH12345", "family_name": "Lu"}),
            sg(),
        );
        assert_eq!(by(&given, "family_name")[0]["value"], "LU");
        assert!(by(&plan(mybag_manage, json!({}), sg()), "log_in").is_empty());
    }

    #[test]
    fn the_apdt_start_reaches_the_document_upload_and_stops() {
        let p = plan(apdt_start, json!({}), sg());
        let steps: Vec<&str> = p.iter().map(|o| o["field"].as_str().unwrap()).collect();
        for f in [
            "close_notice",
            "pics_agree",
            "pics_declare",
            "start",
            "new_application",
            "entry_air",
            "confirm_entry",
        ] {
            assert!(steps.contains(&f), "{f}");
        }
        assert_eq!(p.last().unwrap()["op"], "require");
        assert!(!p
            .iter()
            .any(|o| o["op"] == "text" || o.get("vault").is_some()));
    }
}
