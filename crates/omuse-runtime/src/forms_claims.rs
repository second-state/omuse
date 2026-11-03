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

/// FWD Singapore's online travel claim (https://www.fwd.com.sg/claim-online/travel.html → `/claim-online/travel-insurance/…`),
/// walked to its final button with fake data on 2026-11-03 (docs/sites/fwd-claim.md): no request reaches FWD before
/// that button. Stage `claim` (the default) fills every step: the claimant (step 1; the policy box has a fixed
/// `PNTR20` prefix, so only the rest is typed), the claim type (Travel Disruption → Flight Delay), the trip's dates and
/// country, the delayed flight's date and number, the documents the chat has (plain file inputs), PayNow by the
/// Vault's NRIC/FIN, and stops at the declaration dialog. Stage `submit` presses "agree and submit" (after the user
/// approved it in the app: `submit_confirm`); stage `cancel` closes the dialog unsubmitted.
pub(super) fn fwd_sg_claim(i: &Inputs) -> Vec<Op> {
    match i.arg("stage").as_str() {
        "submit" => {
            return vec![
                click_sel("agree_and_submit", "#TI_BuyEnhacements_Continue_Yes"),
                wait(6000),
            ]
        }
        "cancel" => {
            return vec![
                click_sel("cancel", "#TI_Optfordiscount_Discount_Skip"),
                wait(800),
            ]
        }
        _ => {}
    }
    let (code, number) = phone_parts(&i.phone());
    let mobile = if code == "65" { number } else { String::new() };
    let policy = i.arg("policy_no");
    let policy_rest = policy
        .strip_prefix("PNTR20")
        .or_else(|| policy.strip_prefix("pntr20"))
        .unwrap_or(&policy)
        .to_string();
    let date = |k: &str| NaiveDate::parse_from_str(&i.arg(k), "%Y-%m-%d").ok();
    let tap_input = |field: &'static str, name: &str| {
        Op::new(
            field,
            json!({"op": "tap", "sel": format!("input[name={name}]")}),
            "x",
        )
    };
    let mut ops = vec![
        wait(1500),
        text(
            "claimant_name",
            "input[name='claimant name']",
            &i.full_name(),
        ),
        vault_as("nric_fin", "text", "input[name='NRIC/FIN']", "number", ""),
        text("mobile", "input[name='mobile']", &mobile),
        text("email", "input[name='email']", &i.email()),
        text("policy_no", "input[name='policy number']", &policy_rest),
        click_sel("details_next", "#TI_GetQuotepage_Next_1"),
        Op::new(
            "claim_type",
            json!({"op": "require", "sel": "[role=radio]", "text": "Travel Disruption",
                   "why": "FWD did not accept the claimant's details (the policy number's format: PNTR20NN-NNNNNNNN)", "wait_ms": 10_000}),
            "x",
        ),
        // FWD's radio tiles answer a real mouse click only (insurance_real 2026-11-03: a script's click left the
        // sub-types closed)
        tap_text("travel_disruption", "[role=radio]", "Travel Disruption"),
        wait(800),
        tap_text("flight_delay", "[role=radio]", "Flight Delay"),
        require_within(
            "cover_dates",
            "input[name=dateOfLeaveSG]",
            "FWD's flight delay questions did not open",
            10_000,
        ),
    ];
    for (field, name, k) in [
        ("left_singapore", "dateOfLeaveSG", "leave_sg"),
        ("back_in_singapore", "dateOfBackSG", "return_sg"),
    ] {
        if let Some(d) = date(k) {
            ops.push(tap_input(field, name));
            ops.extend(picker_past(field, d));
        }
    }
    let country = i.arg("destination");
    ops.extend([
        Op::new(
            "country",
            json!({"op": "tap", "sel": "input[data-vv-name=placeOfIncident]"}),
            &country,
        ),
        // typed after the real click (no script focus: it reset FWD's autocomplete), slowly enough for its filter
        // (25 ms kept "na" of "Japan"), then picked once the list has filtered (insurance_real 2026-11-03)
        Op::new(
            "country",
            json!({"op": "pick", "sel": "input[data-vv-name=placeOfIncident]", "type": country,
                   "value": country, "option_sel": FWD_OPTION, "delay_ms": 90, "no_focus": true}),
            &country,
        ),
        click_text("cover_next", "button", "next"),
        require_within(
            "delayed_flight",
            "input[name=dateOfOriginalFlight]",
            "FWD did not take the trip's dates and country",
            10_000,
        ),
    ]);
    if let Some(d) = date("incident_date") {
        ops.push(tap_input("flight_date", "dateOfOriginalFlight"));
        ops.extend(picker_past("flight_date", d));
    }
    ops.extend([
        text(
            "flight_no",
            "input[name=originalFlightNumber]",
            &i.arg("flight_no").replace(' ', ""),
        ),
        click_sel("delay_next", "#TI_GetQuotepage_Next_1"),
        require_within(
            "documents",
            "input#itinerary",
            "FWD did not take the delayed flight's details",
            10_000,
        ),
    ]);
    for (field, sel, k) in [
        ("itinerary", "input#itinerary", "itinerary_file"),
        ("boarding_pass", "input#newItinerary", "boarding_pass_file"),
        (
            "airline_letter",
            "input#operatorLetter",
            "airline_letter_file",
        ),
        ("supporting", "input#supporting", "other_file"),
    ] {
        let path = i.arg(k);
        ops.push(Op::new(
            field,
            json!({"op": "set_file", "sel": sel, "path": path}),
            &path,
        ));
    }
    ops.extend([
        click_sel("documents_next", "#TI_Application_Next"),
        Op::new(
            "payment",
            json!({"op": "require", "sel": "[role=radio]", "text": "Deposit via PayNow",
                   "why": "FWD's payment step did not open", "wait_ms": 10_000}),
            "x",
        ),
        tap_text("paynow", "[role=radio]", "Deposit via PayNow"),
        wait(800),
        vault_as("paynow_id", "text", "input[name=payNowId]", "number", ""),
        vault_as(
            "paynow_id_again",
            "text",
            "input[name=confirmPayNowId]",
            "number",
            "",
        ),
        Op::new(
            "payment_confirmed",
            json!({"op": "tap", "sel": "[role=checkbox]"}),
            "x",
        ),
        click_sel("payment_next", "#TI_GetQuotepage_Next_1"),
        require_within(
            "declaration",
            "#TI_BuyEnhacements_Continue_Yes",
            "FWD's declaration (the submit step) did not open",
            10_000,
        ),
    ]);
    ops
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
/// A past date on the open picker: the month reached going back first (`seek` with `first: prev`), then the day.
fn picker_past(field: &'static str, day: NaiveDate) -> Vec<Op> {
    // the open calendar's own arrows, heading and days: FWD's claim keeps each date's calendar in the page
    let open = |sel: &str| format!(".menuable__content__active {sel}");
    vec![
        Op::new(
            field,
            json!({"op": "seek", "sel": open(FWD_PICKER_ARROWS), "next": 1, "prev": 0, "max": 24, "first": "prev",
                   "until": {"sel": open(FWD_PICKER_MONTH), "text": day.format("%B %Y").to_string()}}),
            "x",
        ),
        tap_text(field, &open(FWD_PICKER_DAYS), &day.day().to_string()),
        wait(600),
    ]
}

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

/// FWD Hong Kong's MyTravel (https://www.fwd.com.hk/online-insurance/mytravel-insurance/en/; underwritten by bolttech;
/// walked 2026-11-03 with fake data). Stage `quote` (default): the region (Asia / Worldwide, from the destination:
/// [`fwd_hk_region`]) and the trip's dates on the page's flatpickr calendars (each day by its `aria-label`, its month
/// reached by the arrows), one traveller; the plan cards (`[data-testid=PlanOption]`) then show each premium. Stage
/// `pick`: the chosen plan's card — FWD creates the application, `…/application/?leadId=<id>`, which opens on any
/// device. Stage `details`: the insured's details (the name, title, date of birth, the HKID from the Vault and its date
/// of issue — a new Smart HKID —, the mobile, the email) and Next: FWD keeps them in the application; the add-ons, the
/// declarations and Pay are the user's.
pub(super) fn fwd_hk_mytravel(i: &Inputs) -> Vec<Op> {
    let day_label = |d: NaiveDate| d.format("%B %-d, %Y").to_string();
    match i.arg("stage").as_str() {
        "pick" => {
            let plan = i.arg("plan");
            if plan.is_empty() || !plan.chars().all(|c| c.is_ascii_alphabetic()) {
                return Vec::new();
            }
            return vec![
                tap_text("plan", "[data-testid=PlanOption] [data-part=name]", &plan),
                require_within(
                    "application",
                    "#lastName",
                    "FWD's application did not open",
                    15_000,
                ),
            ];
        }
        "details" => {
            let (code, number) = phone_parts(&i.phone());
            let mobile = if code == "852" { number } else { String::new() };
            // the name's parts from the call (the model split it: Hong Kong names come family name first)
            let (given, family) = match (i.arg("given_names"), i.arg("family_name")) {
                (g, f) if !g.is_empty() && !f.is_empty() => (g, f),
                _ => i.name_parts(),
            };
            let dmy = |k: &str| {
                NaiveDate::parse_from_str(&i.arg(k), "%Y-%m-%d")
                    .map(|d| d.format("%d/%m/%Y").to_string())
                    .unwrap_or_default()
            };
            let dob = i
                .birthday()
                .map(|d| d.format("%d/%m/%Y").to_string())
                .unwrap_or_default();
            let title = i.arg("title");
            return vec![
                super::keys("last_name", "", "#lastName", &family),
                super::keys("given_name", "", "#givenName", &given),
                Op::new("title", json!({"op": "tap", "sel": "#title"}), &title),
                Op::new(
                    "title",
                    json!({"op": "click", "sel": "[role=option]", "text": title}),
                    &title,
                ),
                super::keys("dob", "", "#dob", &dob),
                super::vault("hkid", "", "#hkid", "number", ""),
                super::keys("hkid_issued", "", "#idIssueDate", &dmy("hkid_issue_date")),
                super::keys("mobile", "", "#mobile", &mobile),
                super::keys("email", "", "#email", &i.email()),
                click_text("details_next", "button", "Next"),
                Op::new(
                    "saved",
                    json!({"op": "require", "sel": "button", "text": "Skip for now",
                           "why": "FWD did not take the details (a new Smart HKID's date of issue, the mobile)", "wait_ms": 15_000}),
                    "x",
                ),
            ];
        }
        _ => {}
    }
    let region = fwd_hk_region(&i.arg("destination"));
    let date = |k: &str| NaiveDate::parse_from_str(&i.arg(k), "%Y-%m-%d").ok();
    let mut ops = vec![
        require_within(
            "plans",
            "[data-testid=PlanOption]",
            "FWD's MyTravel page did not open",
            20_000,
        ),
        // the first quote done before the dates change (its late answer would overwrite the dates' prices)
        premiums_priced(),
        click_text("region", "button", region),
        wait(800),
    ];
    for (field, input, k) in [
        ("depart", "#field-departureDate-input", "depart"),
        ("return", "#field-returnDate-input", "return"),
    ] {
        let Some(d) = date(k) else { continue };
        let day = format!(
            "[role=dialog] .flatpickr-day[aria-label=\"{}\"]",
            day_label(d)
        );
        ops.push(Op::new(field, json!({"op": "tap", "sel": input}), "x"));
        ops.push(Op::new(
            field,
            json!({"op": "seek", "sel": "[role=dialog] .flatpickr-prev-month, [role=dialog] .flatpickr-next-month",
                   "next": 1, "prev": 0, "max": 14, "until": {"sel": day}}),
            "x",
        ));
        ops.push(Op::new(field, json!({"op": "tap", "sel": day}), "x"));
        ops.push(wait(800));
    }
    // FWD prices the plans from its pricing API (ecapi.fwd.com.hk, about 12 s after the page opens, again after each
    // date): each card shows a loader until its premium (`[data-part=premium]@data-value`) is in
    ops.push(wait(1500));
    ops.push(premiums_priced());
    ops
}

/// The wait for FWD Hong Kong's premiums: every plan card's loader gone (at most 20 s).
fn premiums_priced() -> Op {
    Op::new(
        "premiums",
        json!({"op": "await", "sel": "[data-testid=PlanOption] .animation-bouncing-loader", "absent": true,
               "wait_ms": 20_000}),
        "x",
    )
}

/// FWD Hong Kong's insured areas (the page's "Check the insured area", 2026-11-03): Asia is these countries and
/// territories, everywhere else is Worldwide.
pub fn fwd_hk_region(country: &str) -> &'static str {
    match crate::places::iso_of(country) {
        "CN" | "MO" | "TW" | "JP" | "KR" | "BN" | "KH" | "ID" | "LA" | "MY" | "MM" | "PH"
        | "SG" | "TH" | "VN" => "Asia",
        _ => "Worldwide",
    }
}

/// The summary's "Save your application" (mapped 2026-11-02 on the live page with fake data): the header's
/// `button.save-btn` opens `.fwd-save-dialog` — name, email, mobile (a Singapore number, 8 digits), NRIC/FIN (the
/// Vault's ID number) — and its `button.primary` saves the quote (FWD's `updateQuote`; the user agreed to FWD
/// contacting them, 2026-11-02). The saved quote then opens on any device from
/// `https://www.fwd.com.sg/travel-insurance/emailRedirect.html?hash=<localStorage HashKey>` after the mobile number is
/// typed, at FWD's application (declaration, details, Pay — the user's). Without a Singapore mobile nothing is saved.
fn fwd_sg_save(i: &Inputs) -> Vec<Op> {
    let (code, number) = phone_parts(&i.phone());
    if code != "65" || number.len() != 8 {
        return Vec::new();
    }
    let dialog = ".fwd-save-dialog";
    vec![
        click_sel("save", "button.save-btn"),
        require_within(
            "save_dialog",
            &format!("{dialog} input[name=name]"),
            "FWD's save dialog did not open",
            10_000,
        ),
        text(
            "name",
            &format!("{dialog} input[name=name]"),
            &i.full_name(),
        ),
        text("email", &format!("{dialog} input[name=email]"), &i.email()),
        text("mobile", &format!("{dialog} input[name=mobile]"), &number),
        vault_as(
            "nric_fin",
            "text",
            &format!("{dialog} input[name=nric]"),
            "number",
            "",
        ),
        wait(800),
        click_sel("save_quote", &format!("{dialog} button.primary")),
        wait(5000),
    ]
}

/// FWD Singapore's travel insurance quote (https://www.fwd.com.sg/travel-insurance/online-quote/ → `quickQuestions`,
/// then `summary`), mapped from the live page on 2026-11-02: three Vuetify selects found by their labels ("plan type":
/// Single; "who": just myself, or a group; "region": by the destination, [`fwd_region`]), the trip's dates on two
/// read-only date pickers (each month reached by its heading, whatever month the picker opened on), "next" (`#TI_GetQuotepage_Next_1`), the promo code skipped and the discount questions declined
/// (a quote changes nothing). The summary lists the plans (read by the machine). Stage `pick`: the chosen plan's
/// button (`#TI_QuoteSummarypage_<Plan>_Buy`); FWD then asks the eligibility statements, which are the user's.
pub(super) fn fwd_sg_quote(i: &Inputs) -> Vec<Op> {
    if i.arg("stage") == "save" {
        return fwd_sg_save(i);
    }
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
/// paths `/hkTaxMobile/home`, `/hkTaxMobile/process/upload`), mapped 2026-11-02 with the user's own documents (nothing
/// submitted): the notes (a notice may cover them: closed), both statement boxes (`span.check-img`), "Start", the home
/// page's notice ("Close"), "New Application", the entry mode (Air: two one-way tickets, and its "Confirm" on a
/// desktop-sized window), then the documents the chat has, each in its own slot (`.card` titled by the platform): "Click here to
/// upload" (`.text-blue2`) opens a Sample dialog whose "Upload" opens the file chooser (op `file`: setting the input
/// does nothing). The recognised travel document creates the application and shows its reference number ("Your
/// application has been saved automatically", "Continue"); a multi-page receipt asks to confirm the slot's documents
/// for manual processing ("Submit" there confirms that slot, not the application). The refund method, the
/// application's Submit and the face recognition are the user's: the machine stops after the uploads, and the
/// reference number (`.bottom span.card-number.code`) opens the application on any device.
pub(super) fn apdt_start(i: &Inputs) -> Vec<Op> {
    // the entry mode the conversation settled (the call's JSON): the platform's tile for it (walked 2026-11-03: Land /
    // Sea and Air each confirm in a dialog; Land / Sea has no arrival boarding pass slot)
    let tile = match i.arg("entry_mode").as_str() {
        "land_sea" => "Land / Sea",
        "skypier" => "SkyPier Terminal",
        _ => "Air",
    };
    // each step waits for the next page's element (the app's routes draw late; a Queue-it waiting room may come first)
    let until = |field: &'static str, sel: &str, text: &str, why: &str, ms: u64| {
        Op::new(
            field,
            json!({"op": "require", "sel": sel, "text": text, "why": why, "wait_ms": ms}),
            "x",
        )
    };
    let close_notice = |field: &'static str| click_text(field, ".mask div.button", "Close");
    let mut ops = vec![
        require_within(
            "notes",
            "span.check-img",
            "the platform's notes did not open (its waiting room may be holding the page)",
            20_000,
        ),
        wait(1500),
        close_notice("close_notice"),
        wait(800),
        click_nth("pics_agree", "span.check-img", 0),
        close_notice("close_notice_2"),
        click_nth("pics_declare", "span.check-img", 1),
        close_notice("close_notice_3"),
        click_text("start", "div.button", "Start"),
        until(
            "home",
            "div.btn",
            "New Application",
            "the platform's home did not open",
            15_000,
        ),
        wait(1500),
        close_notice("close_home_notice"),
        click_text("new_application", "div.btn", "New Application"),
        until(
            "entry_mode",
            "div.border",
            "Air",
            "the application's entry mode did not open",
            15_000,
        ),
        click_text("entry_mode_tile", "div.border", tile),
        // a desktop-sized window gets a Confirm for the entry mode (the server's browser, 2026-11-03); a phone's
        // does not — a click that may find nothing
        click_text("confirm_entry", ".mask div.button", "Confirm"),
        until(
            "travel_document",
            "",
            "Click here to upload",
            "the platform did not reach its document upload",
            15_000,
        ),
    ];
    // the documents of this chat, slot by slot (a document the chat does not have is skipped)
    let passport = i.arg("passport_file");
    ops.extend(upload_slot(&PASSPORT_SLOT, &passport));
    if !passport.is_empty() {
        // the travel document recognised: the application is saved and its reference number shown
        ops.push(Op::new(
            "reference",
            json!({"op": "await", "sel": ".mask", "text": "reference number", "wait_ms": 10_000}),
            &passport,
        ));
        ops.push(Op::new(
            "reference_seen",
            json!({"op": "click", "sel": ".mask div.button", "text": "Continue"}),
            &passport,
        ));
        ops.push(wait(800));
    }
    for (slot, file) in [
        (&OUT_PASS_SLOT, i.arg("out_pass_file")),
        (&IN_PASS_SLOT, i.arg("in_pass_file")),
        (&RECEIPT_SLOT, i.arg("receipt_file")),
    ] {
        ops.extend(upload_slot(slot, &file));
    }
    ops
}

/// A document slot on the platform's upload page: the title the platform gives its `.card`, and the plan's field names
/// for its steps (open, the file, the recognition, the confirmation of several pages, settle).
struct Slot {
    title: &'static str,
    fields: [&'static str; 6],
}

const PASSPORT_SLOT: Slot = Slot {
    title: "Travel document",
    fields: [
        "passport_open",
        "passport_wait",
        "passport",
        "passport_recognised",
        "passport_pages",
        "passport_settle",
    ],
};
const OUT_PASS_SLOT: Slot = Slot {
    title: "Boarding pass of the departure flight",
    fields: [
        "out_pass_open",
        "out_pass_wait",
        "out_pass",
        "out_pass_recognised",
        "out_pass_pages",
        "out_pass_settle",
    ],
};
const IN_PASS_SLOT: Slot = Slot {
    title: "Boarding pass of the relevant arrival flight",
    fields: [
        "in_pass_open",
        "in_pass_wait",
        "in_pass",
        "in_pass_recognised",
        "in_pass_pages",
        "in_pass_settle",
    ],
};
const RECEIPT_SLOT: Slot = Slot {
    title: "Proof of tax payment",
    fields: [
        "receipt_open",
        "receipt_wait",
        "receipt",
        "receipt_recognised",
        "receipt_pages",
        "receipt_settle",
    ],
};

/// One document into `slot`: its "Click here to upload", the Sample dialog's "Upload" (the file chooser, given
/// `file`), the recognition's time, and the slot's confirmation for a document of several pages ("Documents to be
/// submitted": its "Submit" confirms the slot, not the application). Nothing when the chat has no such document.
fn upload_slot(slot: &Slot, file: &str) -> Vec<Op> {
    let [open, pause, upload, recognised, pages, settle] = slot.fields;
    vec![
        // the previous document's overlay gone first (apdt_real 2026-11-03: the receipt's tap landed on the boarding
        // pass's "processing" overlay and its dialog never opened)
        Op::new(
            settle,
            json!({"op": "await", "sel": ".mask", "absent": true, "wait_ms": 15_000}),
            file,
        ),
        Op::new(
            open,
            // the link by its own text: the receipt's slot has a hint in the same style before it ("such as a ticket
            // or e-ticket …"), which a bare `.text-blue2` tapped (apdt_real 2026-11-03)
            json!({"op": "tap", "scope": slot.title, "scope_sel": ".card", "sel": ".text-blue2",
                   "text": "Click here to upload"}),
            file,
        ),
        // the Sample dialog's button, as soon as it shows
        Op::new(
            pause,
            json!({"op": "await", "sel": ".mask div.button", "text": "Upload", "wait_ms": 5_000}),
            file,
        ),
        Op::new(
            upload,
            json!({"op": "file", "sel": ".mask div.button", "text": "Upload", "path": file, "after": pause}),
            file,
        ),
        // the platform's recognition (a few seconds): the slot shows the document ("View"), or a dialog about it — the
        // pages to confirm, or the saved application's reference number; whichever comes first
        Op::new(
            recognised,
            json!({"op": "await", "wait_ms": 20_000, "after": upload, "any": [
                {"scope": slot.title, "scope_sel": ".card", "text": "View"},
                {"sel": ".mask", "text": "Documents to be submitted"},
                {"sel": ".mask", "text": "reference number"}
            ]}),
            file,
        ),
        Op::new(
            pages,
            json!({"op": "click", "sel": ".mask div.button", "text": "Submit", "after": recognised}),
            file,
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
    fn fwd_hong_kong_waits_for_its_premiums_before_and_after_the_dates() {
        // the run of 2026-11-03 read the cards while they still showed the loader (no premium): FWD prices the plans
        // about 12 s after the page opens and again after each date
        let p = plan(
            fwd_hk_mytravel,
            json!({"destination": "Japan", "depart": "2026-12-03", "return": "2026-12-09"}),
            json!({}),
        );
        let at = |f: &str| p.iter().position(|o| o["field"] == f).unwrap();
        let waits: Vec<usize> = p
            .iter()
            .enumerate()
            .filter(|(_, o)| o["field"] == "premiums")
            .map(|(n, _)| n)
            .collect();
        assert_eq!(waits.len(), 2, "{p:?}");
        assert!(waits[0] < at("region") && waits[1] == p.len() - 1, "{p:?}");
        let w = &p[waits[1]];
        assert_eq!(
            (&w["op"], &w["absent"], &w["wait_ms"]),
            (&json!("await"), &json!(true), &json!(20_000))
        );
        assert_eq!(
            by(&p, "depart")[2]["sel"],
            "[role=dialog] .flatpickr-day[aria-label=\"December 3, 2026\"]"
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
    fn fwds_claim_is_filled_step_by_step_up_to_its_declaration() {
        // walked with fake data on 2026-11-03 (docs/sites/fwd-claim.md)
        let args = json!({"policy_no": "PNTR2012-34567890", "leave_sg": "2026-10-20", "return_sg": "2026-10-25",
                          "destination": "Japan", "incident_date": "2026-10-20", "flight_no": "SQ 636",
                          "itinerary_file": "uploads/itinerary.pdf", "airline_letter_file": "attachments/delay.pdf"});
        let p = plan(fwd_sg_claim, args, sg());
        let v = |f: &str| by(&p, f)[0].clone();
        assert_eq!(v("claimant_name")["sel"], "input[name='claimant name']");
        assert_eq!(v("claimant_name")["value"], "TAN WEI MING");
        assert_eq!(v("mobile")["value"], "91234567");
        // the policy box has a fixed PNTR20 prefix: only the rest is typed (the whole number failed FWD's check)
        assert_eq!(v("policy_no")["value"], "12-34567890");
        // the NRIC / FIN from the Vault: never a value in the plan (step 1 and PayNow)
        for f in ["nric_fin", "paynow_id", "paynow_id_again"] {
            assert_eq!(v(f)["vault"]["field"], "number", "{f}");
            assert!(v(f).get("value").is_none(), "{f}");
        }
        assert_eq!(v("travel_disruption")["text"], "Travel Disruption");
        // FWD's radio tiles and its confirm box take real mouse clicks (a script's click left the sub-types closed)
        for f in [
            "travel_disruption",
            "flight_delay",
            "paynow",
            "payment_confirmed",
        ] {
            assert_eq!(v(f)["op"], "tap", "{f}");
        }
        assert_eq!(v("flight_delay")["text"], "Flight Delay");
        // past dates on FWD's pickers, reached going back
        let seeks: Vec<&Value> = p.iter().filter(|o| o["op"] == "seek").collect();
        assert_eq!(seeks.len(), 3);
        assert!(seeks
            .iter()
            .all(|o| o["first"] == "prev" && o["until"]["text"] == "October 2026"));
        assert_eq!(v("flight_no")["value"], "SQ636");
        // the country typed and picked once FWD's list filtered (one `pick`, not a pick right after typing)
        let country = by(&p, "country");
        assert_eq!(country.last().unwrap()["op"], "pick");
        assert_eq!(country.last().unwrap()["value"], "Japan");
        assert_eq!(country.last().unwrap()["no_focus"], true);
        assert_eq!(country[0]["op"], "tap");
        // the documents the chat has, into their inputs; the others are left out
        assert_eq!(
            (
                v("itinerary")["op"].as_str(),
                v("itinerary")["path"].as_str()
            ),
            (Some("set_file"), Some("uploads/itinerary.pdf"))
        );
        assert_eq!(v("airline_letter")["sel"], "input#operatorLetter");
        assert!(by(&p, "boarding_pass").is_empty() && by(&p, "supporting").is_empty());
        // the plan ends at the declaration: never presses "agree and submit" itself
        assert_eq!(p.last().unwrap()["field"], "declaration");
        assert!(!p
            .iter()
            .any(|o| o["sel"] == "#TI_BuyEnhacements_Continue_Yes" && o["op"] == "click"));
        // the submit and cancel stages are one click each
        let sub = plan(fwd_sg_claim, json!({"stage": "submit"}), sg());
        assert_eq!(sub[0]["sel"], "#TI_BuyEnhacements_Continue_Yes");
        let cancel = plan(fwd_sg_claim, json!({"stage": "cancel"}), sg());
        assert_eq!(cancel[0]["sel"], "#TI_Optfordiscount_Discount_Skip");
        // a mobile number of another country is left out, never cut to fit
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
            "entry_mode_tile",
            "confirm_entry",
            "close_home_notice",
        ] {
            assert!(steps.contains(&f), "{f}");
        }
        // the tile follows the call's entry mode (Air when none is given)
        assert_eq!(by(&p, "entry_mode_tile")[0]["text"], "Air");
        let land = plan(apdt_start, json!({"entry_mode": "land_sea"}), sg());
        assert_eq!(by(&land, "entry_mode_tile")[0]["text"], "Land / Sea");
        // no documents in the chat: nothing uploaded, the plan ends at the upload page
        assert_eq!(p.last().unwrap()["op"], "require");
        assert!(!p
            .iter()
            .any(|o| o["op"] == "text" || o["op"] == "file" || o.get("vault").is_some()));
        // the chat's documents, each into its own slot through the platform's file chooser (mapped 2026-11-02)
        let p = plan(
            apdt_start,
            json!({"passport_file": "uploads/passport.jpg", "out_pass_file": "uploads/bp.jpg", "receipt_file": "uploads/r.pdf"}),
            sg(),
        );
        assert_eq!(by(&p, "passport_open")[0]["scope"], "Travel document");
        assert_eq!(by(&p, "passport_open")[0]["scope_sel"], ".card");
        assert_eq!(by(&p, "passport")[0]["op"], "file");
        assert_eq!(by(&p, "passport")[0]["path"], "uploads/passport.jpg");
        assert_eq!(by(&p, "passport")[0]["text"], "Upload");
        assert_eq!(by(&p, "reference")[0]["text"], "reference number");
        assert_eq!(by(&p, "reference_seen")[0]["text"], "Continue");
        assert_eq!(by(&p, "out_pass")[0]["path"], "uploads/bp.jpg");
        assert_eq!(
            by(&p, "out_pass_open")[0]["scope"],
            "Boarding pass of the departure flight"
        );
        assert_eq!(by(&p, "receipt_open")[0]["scope"], "Proof of tax payment");
        assert_eq!(by(&p, "receipt_pages")[0]["text"], "Submit");
        // the recognition's wait ends at the first thing the page shows (no fixed pause): the slot's "View", or a
        // dialog about the document
        // each slot waits for the previous document's overlay to go, and a step after a failed one is skipped
        let at = |f: &str| p.iter().position(|o| o["field"] == f).unwrap();
        assert!(
            by(&p, "receipt_settle")[0]["absent"] == true
                && at("receipt_settle") < at("receipt_open")
        );
        assert_eq!(by(&p, "receipt")[0]["after"], "receipt_wait");
        assert_eq!(by(&p, "receipt_recognised")[0]["after"], "receipt");
        // the slot's link by its own text (the receipt's hint shares its style and came first)
        assert_eq!(by(&p, "receipt_open")[0]["text"], "Click here to upload");
        let any = by(&p, "out_pass_recognised")[0]["any"]
            .as_array()
            .unwrap()
            .clone();
        assert_eq!(any[0]["scope"], "Boarding pass of the departure flight");
        assert_eq!(any[0]["text"], "View");
        assert!(any.iter().any(|c| c["text"] == "Documents to be submitted"));
        // the arrival pass the chat does not have: its slot is left for the user
        assert!(by(&p, "in_pass_open").is_empty() && by(&p, "in_pass").is_empty());
        // the uploads come after the upload page is reached
        let at = |f: &str| p.iter().position(|o| o["field"] == f).unwrap();
        assert!(
            at("travel_document") < at("passport_open")
                && at("reference_seen") < at("out_pass_open")
        );
    }

    #[test]
    fn fwds_save_fills_its_dialog_by_input_names_and_needs_a_singapore_mobile() {
        // mapped on the live summary 2026-11-02: the header's save button, the dialog's four inputs, its save button
        let p = plan(fwd_sg_quote, json!({"stage": "save"}), sg());
        assert_eq!(by(&p, "save")[0]["sel"], "button.save-btn");
        assert_eq!(
            by(&p, "name")[0]["sel"],
            ".fwd-save-dialog input[name=name]"
        );
        assert_eq!(by(&p, "name")[0]["value"], "TAN WEI MING");
        assert_eq!(by(&p, "email")[0]["value"], "weiming@example.com");
        assert_eq!(by(&p, "mobile")[0]["value"], "91234567");
        // the NRIC/FIN comes from the Vault: the plan never carries it
        let nric = by(&p, "nric_fin")[0];
        assert_eq!(nric["sel"], ".fwd-save-dialog input[name=nric]");
        assert_eq!(nric["vault"]["field"], "number");
        assert!(nric
            .get("value")
            .is_none_or(|v| v.as_str() == Some("x") || v.is_null()));
        assert_eq!(
            by(&p, "save_quote")[0]["sel"],
            ".fwd-save-dialog button.primary"
        );
        // another country's number: FWD's save and its link need a Singapore mobile — nothing is filled
        let hk = json!({"name_en": "Chan Tai Man", "email_personal": "tm@example.com", "phone": "+852 9123 4567"});
        assert!(plan(fwd_sg_quote, json!({"stage": "save"}), hk).is_empty());
    }
}
