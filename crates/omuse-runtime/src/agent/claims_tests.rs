//! The claims machine's tests: `check` on good, bad and missing fields; runs from the transcript the run loop leaves
//! (the user's message, the tool call, its `running` result), each code call answered by name, to the Done JSON.

use super::*;

const BACK: &str = "\"Takeover finished: the user handed browser control back.\"";

fn today() -> NaiveDate {
    NaiveDate::from_ymd_opt(2026, 10, 31).unwrap()
}

fn step(id: &str, name: &str, args: Value, result: &str) -> Vec<Value> {
    vec![
        json!({"role": "assistant", "content": "", "tool_calls": [{"id": id, "type": "function",
               "function": {"name": name, "arguments": args.to_string()}}]}),
        json!({"role": "tool", "tool_call_id": id, "content": result}),
    ]
}

/// A `browser_snapshot` result as the sentinel returns it: the snapshot inside the untrusted JSON envelope.
fn page(url: &str, snapshot: &str) -> String {
    format!(
        "<untrusted_content source=\"web page {url}\">\n{}\n</untrusted_content>",
        json!({"trust": "untrusted", "source": format!("web page {url}"), "url": url,
               "title": "page", "tabs": [], "snapshot": snapshot})
    )
}

/// A `browser_read_elements` result as the sentinel returns it: the elements read by their selectors, untrusted.
fn elements(url: &str, read: Value) -> String {
    format!(
        "<untrusted_content source=\"web page {url}\">\n{}\n</untrusted_content>",
        json!({"trust": "untrusted", "source": format!("web page {url}"), "url": url, "title": "page",
               "elements": read})
    )
}

fn cx<'a>(profile: &'a Value, mail: bool, t: &'a [Value]) -> Ctx<'a> {
    Ctx {
        profile,
        today: today(),
        mail_ready: mail,
        transcript: t,
    }
}

/// The validated arguments (panics when the check does not pass).
fn valid(tool: &str, args: Value, mail: bool, t: &[Value]) -> Value {
    let profile = json!({});
    match check(tool, &args, &cx(&profile, mail, t)) {
        Checked::Valid(v) => v,
        other => panic!("{tool} {args}: {other:?}"),
    }
}

/// The transcript the run loop leaves after a valid call: `before`, the user's message, the call and its `running`
/// result. The index of the call.
fn started(before: Vec<Value>, words: &str, tool: &str, args: &Value) -> (Vec<Value>, usize) {
    let mut t = before;
    t.push(json!({"role": "user", "content": words}));
    let from = t.len();
    t.extend(step(
        "c1",
        tool,
        args.clone(),
        &Checked::Valid(args.clone()).to_result().to_string(),
    ));
    (t, from)
}

/// The run to its Done JSON, each code call answered by `answer(name, args)`; the calls made (one turn each, ids with
/// the machine's prefix) and the result.
fn run(
    tool: &str,
    args: &Value,
    t: &mut Vec<Value>,
    from: usize,
    profile: &Value,
    answer: impl Fn(&str, &Value) -> String,
) -> (Vec<ToolCall>, Value) {
    let m = Machine {
        profile,
        today: today(),
    };
    let mut calls = Vec::new();
    for _ in 0..40 {
        match next(tool, args, t, from, &m) {
            Step::Calls(c) => {
                for call in c {
                    assert!(call.id.starts_with(PREFIX), "{}", call.id);
                    assert!(
                        !matches!(call.name.as_str(), "ask_user"),
                        "a question mid-run"
                    );
                    let out = answer(&call.name, &call.args);
                    t.extend(step(&call.id, &call.name, call.args.clone(), &out));
                    calls.push(call);
                }
            }
            Step::Done(v) => return (calls, v),
        }
    }
    panic!("the run did not end")
}

fn names(calls: &[ToolCall]) -> Vec<&str> {
    calls.iter().map(|c| c.name.as_str()).collect()
}

/// The lines code appends to the reply for this result (agent/show.rs).
fn must(v: &Value) -> String {
    super::super::show::lines(v, false).join("\n")
}

// ---------------------------------------------------------------- validation

#[test]
fn a_claims_facts_are_checked_and_normalised() {
    let t: Vec<Value> = Vec::new();
    let v = valid(
        CLAIM_FILE,
        json!({"insurer": "chase_sapphire", "benefit": "trip_cancellation", "incident_date": "2026-10-28",
               "paid": "1,840.00", "refunded": 86.4, "non_refundable": 1753.6, "card_last4": "4321",
               "bookings": [{"supplier": "United Airlines", "reference": "ABC123"}, {"supplier": "", "reference": "x"}]}),
        false,
        &t,
    );
    assert_eq!(
        (
            v["paid"].as_f64(),
            v["refunded"].as_f64(),
            v["card_last4"].as_str()
        ),
        (Some(1840.0), Some(86.4), Some("4321"))
    );
    assert_eq!(v["bookings"].as_array().unwrap().len(), 1);
    assert_eq!(
        (
            v["reason"].as_str(),
            v["who"].as_str(),
            v["card_product"].as_str()
        ),
        (
            Some("sickness"),
            Some("cardholder"),
            Some("Chase Sapphire Reserve")
        )
    );
    // a full card number is never kept
    let v = valid(
        CLAIM_FILE,
        json!({"insurer": "chase_sapphire", "benefit": "trip_cancellation", "incident_date": "2026-10-28",
               "paid": 1200, "card_last4": "4111111111111111"}),
        false,
        &t,
    );
    assert!(v.get("card_last4").is_none());
    // the FWD delay: the flight number and the times normalised, the market from the profile
    let v = valid(
        CLAIM_FILE,
        json!({"insurer": "fwd", "policy_no": "TI-SG-2026-55123", "incident_date": "2026-10-29", "flight_no": "sq636",
               "scheduled_departure": "10:30", "actual_departure": "17.45"}),
        false,
        &t,
    );
    assert_eq!(
        (
            v["flight_no"].as_str(),
            v["actual_departure"].as_str(),
            v["market"].as_str(),
            v["benefit"].as_str()
        ),
        (
            Some("SQ 636"),
            Some("17:45"),
            Some("sg"),
            Some("flight_delay")
        )
    );
}

#[test]
fn wrong_facts_are_the_models_to_fix() {
    let t: Vec<Value> = Vec::new();
    let p = json!({});
    for args in [
        json!({"insurer": "aia"}),
        json!({"insurer": "chase_sapphire", "benefit": "trip_cancellation", "incident_date": "2026-11-20", "paid": 100}),
        json!({"insurer": "chase_sapphire", "benefit": "trip_cancellation", "incident_date": "2026-10-28", "paid": 100, "refunded": 300}),
        json!({"insurer": "chase_sapphire", "benefit": "flight_delay", "incident_date": "2026-10-28", "paid": 100}),
        json!({"insurer": "fwd", "policy_no": "P1", "incident_date": "2026-10-29", "flight_no": "SQ 636",
               "scheduled_departure": "half past ten", "actual_departure": "17:45"}),
        json!({"insurer": "fwd", "policy_no": "P1", "incident_date": "2026-10-29", "flight_no": "delayed",
               "scheduled_departure": "10:30", "actual_departure": "17:45"}),
    ] {
        assert!(
            matches!(
                check(CLAIM_FILE, &args, &cx(&p, true, &t)),
                Checked::Invalid(_)
            ),
            "{args}"
        );
    }
    assert!(matches!(
        check(
            INSURANCE_BUY,
            &json!({"destination": "Tokyo", "depart": "2026-12-01", "return": "2026-11-24"}),
            &cx(&p, false, &t)
        ),
        Checked::Invalid(_)
    ));
    assert!(matches!(
        check(
            BAG_TRACE,
            &json!({"file_ref": "7HK2LM"}),
            &cx(&p, false, &t)
        ),
        Checked::Invalid(_)
    ));
}

#[test]
fn missing_facts_read_the_mail_once_then_ask_the_user() {
    let p = json!({});
    let bare = json!({"insurer": "chase_sapphire"});
    // the mail is connected and not read yet: the run reads it first
    let v = valid(CLAIM_FILE, bare.clone(), true, &[]);
    assert_eq!(v["_read_mail"], true);
    assert!(v["_missing"].to_string().contains("incident_date"));
    // no mail: needs_input at once, before any page opens
    let Checked::NeedsInput(missing) = check(CLAIM_FILE, &bare, &cx(&p, false, &[])) else {
        panic!()
    };
    assert!(missing.iter().any(|m| m.starts_with("benefit")));
    assert!(missing.iter().any(|m| m.starts_with("paid")));
    // the mail was read for this tool (a mail_read result): what is still missing is the user's
    let mut t = vec![json!({"role": "user", "content": "claim my trip"})];
    t.extend(workflow::result_exchange(
        &t,
        CLAIM_FILE,
        &json!({"status": MAIL_READ, "emails": []}),
    ));
    assert!(matches!(
        check(CLAIM_FILE, &bare, &cx(&p, true, &t)),
        Checked::NeedsInput(_)
    ));
    // another tool's mail read does not count
    assert!(matches!(
        check(BAG_TRACE, &json!({}), &cx(&p, true, &t)),
        Checked::Valid(_)
    ));
    // the purchase without its trip; the bag without its reference
    assert!(matches!(
        check(
            INSURANCE_BUY,
            &json!({"destination": "Tokyo"}),
            &cx(&p, false, &[])
        ),
        Checked::NeedsInput(_)
    ));
    assert!(matches!(
        check(BAG_TRACE, &json!({}), &cx(&p, false, &[])),
        Checked::NeedsInput(_)
    ));
}

#[test]
fn the_fwd_market_comes_from_the_argument_the_policy_sender_or_the_profile() {
    let fwd = json!({"insurer": "fwd", "policy_no": "P1", "incident_date": "2026-10-29", "flight_no": "CX 543",
                     "scheduled_departure": "09:05", "actual_departure": "15:20"});
    let sg = json!({});
    let hk = json!({"address_home": "Flat 12B, 88 Robinson Road, Mid-Levels, Hong Kong", "home_country": "HK"});
    let market = |a: &Value, p: &Value, t: &[Value]| {
        let Checked::Valid(v) = check(CLAIM_FILE, a, &cx(p, false, t)) else {
            panic!()
        };
        get_str(&v, "market")
    };
    assert_eq!(market(&fwd, &sg, &[]), "sg");
    assert_eq!(market(&fwd, &hk, &[]), "hk");
    let mut a = fwd.clone();
    a["market"] = json!("hk");
    assert_eq!(market(&a, &sg, &[]), "hk");
    // the policy email's structured sender
    let t = step("x1", "gmail_get_message", json!({"message_id": "p1"}),
        "<untrusted_content source=\"email\">{\"from\": \"FWD Hong Kong <noreply@fwd.com.hk>\", \"subject\": \"Your MyTravel policy\", \"body\": \"…\"}</untrusted_content>");
    assert_eq!(market(&fwd, &sg, &t), "hk");
}

// ---------------------------------------------------------------- the mail read for missing facts

#[test]
fn the_mail_is_read_by_fixed_searches_and_listed_for_the_model() {
    let profile = json!({});
    let args = valid(CLAIM_FILE, json!({"insurer": "chase_sapphire"}), true, &[]);
    let (mut t, from) = started(
        Vec::new(),
        "Claim my trip on my Chase card.",
        CLAIM_FILE,
        &args,
    );
    let (calls, res) = run(CLAIM_FILE, &args, &mut t, from, &profile, |name, a| {
        match name {
        "gmail_search" if sget(a, "query").starts_with("(e-ticket") => {
            json!({"count": 2, "messages": [{"id": "m1", "subject": "eTicket"}, {"id": "m3", "subject": "Refund"}]}).to_string()
        }
        "gmail_search" => json!({"count": 0, "messages": []}).to_string(),
        "gmail_get_message" => json!({"from": "United <noreply@united.example>", "subject": format!("mail {}", sget(a, "message_id")),
                                      "body": "Fare USD 1,840.00", "attachments": [{"filename": "receipt.pdf"}]}).to_string(),
        _ => panic!("{name}"),
    }
    });
    assert_eq!(
        names(&calls),
        [
            "gmail_search",
            "gmail_search",
            "gmail_search",
            "gmail_search",
            "gmail_get_message",
            "gmail_get_message"
        ]
    );
    assert_eq!(res["status"], MAIL_READ);
    assert_eq!(res["call_again"], CLAIM_FILE);
    assert_eq!(res["emails"][0]["message_id"], "m1");
    assert_eq!(res["emails"][0]["attachments"][0], "receipt.pdf");
    // the reply turn after it keeps the workflow tools: the model calls claim_file again with the facts
    t.extend(workflow::result_exchange(&t, CLAIM_FILE, &res));
    assert!(!workflow::reply_turn(&t));
    assert!(matches!(
        check(
            CLAIM_FILE,
            &json!({"insurer": "chase_sapphire"}),
            &cx(&profile, true, &t)
        ),
        Checked::NeedsInput(_)
    ));
    // the FWD delay's searches name the flight
    let args = valid(
        CLAIM_FILE,
        json!({"insurer": "fwd", "flight_no": "SQ636"}),
        true,
        &[],
    );
    let (t, from) = started(Vec::new(), "claim the delay", CLAIM_FILE, &args);
    let m = Machine {
        profile: &profile,
        today: today(),
    };
    let Step::Calls(c) = next(CLAIM_FILE, &args, &t, from, &m) else {
        panic!()
    };
    assert_eq!(c.len(), 2);
    assert!(sget(&c[1].args, "query").starts_with("(\"SQ 636\" OR SQ636) (delay"));
}

// ---------------------------------------------------------------- the CSR trip claim

fn csr_cancellation() -> Value {
    valid(
        CLAIM_FILE,
        json!({"insurer": "chase_sapphire", "benefit": "trip_cancellation", "incident_date": "2026-10-28",
               "reason": "sickness", "who": "cardholder", "paid": 1840, "refunded": 86.40, "non_refundable": 1753.60,
               "bookings": [{"supplier": "United Airlines", "reference": "ABC123"}], "trip": ["UA 837 SFO–NRT"],
               "card_product": "Chase Sapphire Reserve", "card_last4": "4321", "doctor_letter": true}),
        false,
        &[],
    )
}

const CCC: &str = "https://cardclaimcenter.com/internal/log-in";

/// Card Claim Center after the hand-back: the sign-in page (its password box) still there, or not.
fn csr_answers(signed_in: bool) -> impl Fn(&str, &Value) -> String {
    move |name, _| {
        match name {
        "browser_navigate" => page(CCC, "[e1] textbox \"Email Address\"\n[e2] textbox \"Password\""),
        "form_fill" => format!("{{\"url\": \"{CCC}\", \"filled\": [{{\"op\": \"text\", \"field\": \"email\", \"ok\": true}}]}}"),
        "browser_request_takeover" => BACK.into(),
        "browser_read_elements" if signed_in => elements("https://cardclaimcenter.com/internal/claims/new", json!({"password": []})),
        "browser_read_elements" => elements(CCC, json!({"password": ["password"]})),
        other => panic!("{other}"),
    }
    }
}

#[test]
fn the_csr_claim_reaches_the_sign_in_and_hands_it_over_with_the_facts() {
    // www.eclaimsline.com redirects to Card Claim Center's sign-in (2026-11-02): the account is the user's
    let profile = json!({"name_en": "LUCAS LU", "email_personal": "lucas@example.com"});
    let args = csr_cancellation();
    let (mut t, from) = started(Vec::new(), "I couldn't take my United flight — claim the trip cancellation insurance on my Sapphire Reserve. I'll press Submit myself.", CLAIM_FILE, &args);
    let (calls, res) = run(
        CLAIM_FILE,
        &args,
        &mut t,
        from,
        &profile,
        csr_answers(false),
    );
    assert_eq!(
        names(&calls),
        [
            "browser_navigate",
            "form_fill",
            "browser_request_takeover",
            "browser_read_elements"
        ]
    );
    assert_eq!(sget(&calls[0].args, "url"), CCC);
    let reason = sget(&calls[2].args, "reason");
    assert!(
        reason.starts_with(OWN_STEP)
            && reason.contains("sign in to Card Claim Center")
            && reason.contains("paid 1,840.00 − refunded 86.40"),
        "{reason}"
    );
    assert_eq!(calls[3].args["read"][0]["sel"], CCC_PASSWORD);
    assert_eq!(res["status"], "waiting_on_user");
    let m = must(&res);
    assert!(
        m.contains("- Trip Cancellation claim on the Chase Sapphire Reserve: USD 1,753.60\n- How it adds up: paid 1,840.00 − refunded 86.40")
            && m.contains("- Notify the administrator by: 17 November 2026 (loss date 28 October 2026 + 20 days)")
            && m.contains("- Documents by: 26 January 2027")
            && m.contains("sign in or create your account")
            && m.contains("nothing has been filed")
            && m.contains("- Decides the claim: Card Benefit Services (Chase's benefit administrator)"),
        "{m}"
    );
    assert!(!res.to_string().contains("4321"), "{res}");
    // signed in: the claim pages have no form function — the browser-use skill fills them with the facts
    let (mut t, from) = started(Vec::new(), "claim", CLAIM_FILE, &args);
    let (_, res) = run(CLAIM_FILE, &args, &mut t, from, &profile, csr_answers(true));
    assert_eq!(res["status"], "browser_use");
    assert_eq!(
        res["url"],
        "https://cardclaimcenter.com/internal/claims/new"
    );
    assert!(must(&res).contains("USD 1,753.60"));
}

#[test]
fn the_interruption_claims_the_unrefunded_nights_and_the_change_fee() {
    // cards_interruption_real: 750.00 not refunded by the hotel + the USD 300.00 change fee to fly home early
    let args = valid(
        CLAIM_FILE,
        json!({"insurer": "chase_sapphire", "benefit": "trip_interruption", "incident_date": "2026-10-24",
               "reason": "injury", "paid": 2870, "refunded": 250, "non_refundable": 750, "change_fee": 300,
               "bookings": [{"supplier": "United Airlines", "reference": "QX7P2L"}, {"supplier": "Park Hyatt Tokyo", "reference": "HY-55821"}]}),
        false,
        &[],
    );
    let t = TripClaim::from_args(&args);
    assert_eq!(
        (t.benefit.as_str(), t.amount, t.reason.as_str()),
        ("Trip Interruption", 1050.0, "injury of the cardholder")
    );
    assert_eq!(
        t.arithmetic(),
        "750.00 stated as non-refundable by the supplier + change fee 300.00 to fly home early"
    );
    // the caps are data
    let big = TripClaim {
        benefit: "Trip Cancellation".into(),
        paid: 25_000.0,
        travellers: 1,
        ..Default::default()
    };
    assert_eq!(claim_amount(&big, spec(Flow::CsrTrip)), 10_000.0);
    let two = TripClaim {
        travellers: 2,
        ..big.clone()
    };
    assert_eq!(claim_amount(&two, spec(Flow::CsrTrip)), 20_000.0);
    // a late notice: the result says the window closed
    let late = TripClaim {
        loss: NaiveDate::from_ymd_opt(2026, 10, 1),
        ..t
    };
    let v = csr_result(spec(Flow::CsrTrip), &late, "waiting_on_user", today());
    assert!(must(&v).contains("that window has closed"), "{v}");
    assert_eq!(v["notice_window_closed"], true);
}

#[test]
fn a_sign_in_the_user_did_not_hand_back_ends_with_the_facts_and_nothing_filed() {
    let profile = json!({});
    let args = csr_cancellation();
    let (mut t, from) = started(Vec::new(), "claim", CLAIM_FILE, &args);
    let (calls, res) = run(
        CLAIM_FILE,
        &args,
        &mut t,
        from,
        &profile,
        |name, _| match name {
            "browser_request_takeover" => {
                "\"Takeover ended without the hand-back (timed out).\"".into()
            }
            "form_fill" => "{\"url\": \"x\", \"filled\": []}".into(),
            _ => "ok".into(),
        },
    );
    assert_eq!(names(&calls).last(), Some(&"browser_request_takeover"));
    assert_eq!(res["status"], "waiting_on_user");
    let m = must(&res);
    assert!(
        m.contains("USD 1,753.60") && m.contains("nothing has been filed"),
        "{m}"
    );
}

// ---------------------------------------------------------------- the FWD delay claims

/// The mail code read before the call (a `mail_read` run): the policy and the airline's letter with its PDF.
fn mail_before(cert_from: &str, cert_id: &str, file: &str) -> Vec<Value> {
    let mut t = vec![json!({"role": "user", "content": "claim the delay"})];
    t.extend(step("clm_1", "gmail_get_message", json!({"message_id": "p1"}),
        &json!({"from": "FWD <noreply@fwd.com.sg>", "subject": "Your policy", "body": "…", "attachments": [{"filename": "policy_wording.pdf"}]}).to_string()));
    t.extend(step("clm_2", "gmail_get_message", json!({"message_id": cert_id}),
        &json!({"from": cert_from, "subject": "Delay certificate", "body": "…", "attachments": [{"filename": file}]}).to_string()));
    t
}

#[test]
fn the_sg_delay_claim_counts_the_hours_fills_step_one_and_hands_the_rest_over() {
    let profile = json!({"name_en": "TAN WEI MING"});
    let args = valid(
        CLAIM_FILE,
        json!({"insurer": "fwd", "market": "sg", "policy_no": "PNTR20123456", "plan": "Business",
               "incident_date": "2026-10-29", "flight_no": "SQ 636", "from": "Singapore", "to": "Tokyo Haneda",
               "airline": "Singapore Airlines", "scheduled_departure": "10:30", "actual_departure": "17:45",
               "delay_reason": "the late arrival of the inbound aircraft", "benefit_per_block": 100, "block_hours": 6,
               "benefit_cap": 1500, "claim_within_days": 30, "emergency_phone": "+65 6322 2072",
               "certificate_message_id": "c1"}),
        false,
        &[],
    );
    let before = mail_before(
        "Singapore Airlines <noreply@singaporeair.example>",
        "c1",
        "delay_certificate_SQ636.pdf",
    );
    let (mut t, from) = started(before, "Please go ahead.", CLAIM_FILE, &args);
    let (calls, res) = run(CLAIM_FILE, &args, &mut t, from, &profile, |name, _| {
        match name {
        "gmail_save_attachment" => "attachment saved: attachments/delay_certificate_SQ636.pdf (12 KB, application/pdf).".into(),
        "browser_navigate" => page("https://www.fwd.com.sg/claim-online/travel-insurance/quickQuestions", "{}"),
        "form_fill" => "{\"url\": \"https://www.fwd.com.sg/claim-online/travel-insurance/quickQuestions\", \"filled\": []}".into(),
        "browser_request_takeover" => BACK.into(),
        "schedule_create" => "created".into(),
        other => panic!("{other}"),
    }
    });
    // no upload (step 3 is not mapped) and no confirmation read (a real confirmation page is not known)
    assert_eq!(
        names(&calls),
        [
            "gmail_save_attachment",
            "browser_navigate",
            "form_fill",
            "browser_request_takeover",
            "schedule_create",
            "schedule_create"
        ]
    );
    let fill = &calls[2].args;
    assert_eq!(
        (
            sget(fill, "policy_no").as_str(),
            sget(fill, "stage").as_str()
        ),
        ("PNTR20123456", "claim")
    );
    let reason = sget(&calls[3].args, "reason");
    assert!(
        reason.contains("delayed 7 h 15 min")
            && reason.contains("attachments/delay_certificate_SQ636.pdf")
            && reason.contains("press next"),
        "{reason}"
    );
    assert_eq!(res["status"], "handed_over");
    let m = must(&res);
    assert!(
        m.contains("delayed 7 h 15 min")
            && m.contains("- Claim amount: SGD 100.00")
            && m.contains("- Rule: SGD 100 for every 6 hours of delay (1 × 6 h), up to SGD 1,500")
            && m.contains(
                "- Tell FWD by: 28 November 2026 (incident + 30 days, from your policy email)"
            )
            && m.contains("- 24-hour emergency assistance: +65 6322 2072")
            && m.contains("step 1 of FWD's online claim is filled in"),
        "{m}"
    );
    // 5 hours: under the first block, nothing filed
    let short = valid(
        CLAIM_FILE,
        json!({"insurer": "fwd", "market": "sg", "policy_no": "P1", "incident_date": "2026-10-29", "flight_no": "SQ 636",
               "scheduled_departure": "10:30", "actual_departure": "15:30"}),
        false,
        &[],
    );
    let (mut t, from) = started(Vec::new(), "claim", CLAIM_FILE, &short);
    let (calls, res) = run(CLAIM_FILE, &short, &mut t, from, &profile, |_, _| {
        "ok".into()
    });
    assert!(calls.is_empty());
    assert_eq!(res["status"], "no_benefit");
    assert!(must(&res).contains("delayed 5 h 00 min")); // a cap given as 0 (the user named none) is no cap: the market's rule stands, 7 h 15 min is one block
    let zero = valid(
        CLAIM_FILE,
        json!({"insurer": "fwd", "market": "sg", "policy_no": "P1", "incident_date": "2026-10-29", "flight_no": "SQ 636",
               "scheduled_departure": "10:30", "actual_departure": "17:45", "benefit_per_block": 100, "block_hours": 6,
               "benefit_cap": 0}),
        false,
        &[],
    );
    assert!(zero.get("benefit_cap").is_none());
    assert_eq!(
        DelayClaim::from_args(spec(Flow::FwdSgClaim), &zero).amount,
        100.0
    );
}

fn hk_args() -> Value {
    valid(
        CLAIM_FILE,
        json!({"insurer": "fwd", "market": "hk", "policy_no": "MT-HK-2026-00881", "plan": "Premium Plan", "incident_date": "2026-10-29",
               "flight_no": "CX 543", "from": "Hong Kong", "to": "Tokyo Narita", "airline": "Cathay Pacific",
               "airline_ref": "CX-DLY-55821", "scheduled_departure": "09:05", "actual_departure": "15:20",
               "benefit_per_block": 300, "block_hours": 6, "benefit_cap": 1500, "claim_within_days": 90,
               "emergency_phone": "+852 2000 0100"}),
        false,
        &[],
    )
}

fn hk_answers(name: &str, _: &Value) -> String {
    match name {
        "gmail_save_attachment" => "attachment saved: attachments/delay_letter_CX543.pdf (9 KB, application/pdf).".into(),
        "browser_navigate" => "{\"url\": \"https://www.fwd.com.hk/files/x.pdf\", \"pdf\": {\"path\": \"downloads/travel-insurance-clm-form-fillable.pdf\"}}".into(),
        "pdf_form_fill" => "{\"output\": \"attachments/bolttech-travel-claim-MT-HK-2026-00881.pdf\", \"filled\": 14, \"problems\": []}".into(),
        "send_file" | "gmail_create_draft" | "schedule_create" => "ok".into(),
        other => panic!("{other}"),
    }
}

#[test]
fn the_hk_delay_claim_opens_bolttechs_pdf_directly_fills_it_and_drafts_the_email() {
    let profile = json!({"name_en": "CHAN TAI MAN", "email_personal": "taiman@example.com",
                         "address_home": "Flat 12B, 88 Robinson Road, Mid-Levels, Hong Kong", "home_country": "HK",
                         "mailbox": "taiman@example.com"});
    let args = hk_args();
    assert_eq!(args["market"], "hk");
    let before = mail_before(
        "Cathay Pacific <noreply@cathaypacific.example>",
        "c1",
        "delay_letter_CX543.pdf",
    );
    let (mut t, from) = started(
        before,
        "Prepare the claim; don't send it.",
        CLAIM_FILE,
        &args,
    );
    let (calls, res) = run(CLAIM_FILE, &args, &mut t, from, &profile, hk_answers);
    // the PDF's own address, no home page, no link to find and click
    assert_eq!(
        names(&calls),
        [
            "gmail_save_attachment",
            "browser_navigate",
            "pdf_form_fill",
            "send_file",
            "gmail_create_draft",
            "schedule_create"
        ]
    );
    assert!(sget(&calls[1].args, "url").ends_with("travel-insurance-clm-form-fillable.pdf"));
    let v = &calls[2].args["values"];
    assert_eq!(
        (
            sget(v, "fill_17").as_str(),
            sget(v, "Full name").as_str(),
            sget(v, "fill_14_2").as_str(),
            sget(v, "fill_1").as_str(),
            sget(v, "fill_7").as_str(),
            sget(v, "fill_9_2").as_str()
        ),
        (
            "MT-HK-2026-00881",
            "CHAN TAI MAN",
            "CX 543",
            "6 h 15 min",
            "300.00",
            "CX-DLY-55821"
        )
    );
    assert!(v.get("HKID Card no").is_none() && v.get("Claimants signature").is_none());
    let draft = &calls[4].args;
    assert_eq!(sget(draft, "to"), "claims.hk@bolttechinsurance.com");
    assert_eq!(
        draft["attachments"],
        json!([
            "attachments/bolttech-travel-claim-MT-HK-2026-00881.pdf",
            "attachments/delay_letter_CX543.pdf"
        ])
    );
    assert_eq!(res["status"], "drafted");
    let m = must(&res);
    assert!(
        m.contains("- Handled by: bolttech")
            && m.contains("- Claim amount: HKD 300.00")
            && m.contains("- You still add: your HKID card number")
            && m.contains("- Claim form (filled in): [bolttech-travel-claim-MT-HK-2026-00881.pdf](/api/files/raw?path=attachments")
            && m.contains("- Email draft (not sent): ")
            && m.contains("- Claim by: 27 January 2027")
            && m.contains("+852 2000 0100"),
        "{m}"
    );
    // no mailbox: no draft and no mail check — the result says where to send the signed form
    let no_mail = json!({"name_en": "CHAN TAI MAN"});
    let (mut t, from) = started(Vec::new(), "Prepare the claim.", CLAIM_FILE, &args);
    let (calls, res) = run(CLAIM_FILE, &args, &mut t, from, &no_mail, hk_answers);
    assert_eq!(
        names(&calls),
        ["browser_navigate", "pdf_form_fill", "send_file"]
    );
    assert_eq!(res["status"], "form_filled");
    let m = must(&res);
    assert!(
        m.contains("- Send it to: claims.hk@bolttechinsurance.com — no mailbox is connected")
            && !m.contains("Email draft")
            && !m.contains("Reminder set"),
        "{m}"
    );
    // the navigation's result without the PDF: the browser's downloads, then an error that says what to do
    let (mut t, from) = started(Vec::new(), "Prepare the claim.", CLAIM_FILE, &args);
    let (calls, res) = run(
        CLAIM_FILE,
        &args,
        &mut t,
        from,
        &no_mail,
        |name, a| match name {
            "browser_navigate" => "{\"url\": \"https://www.fwd.com.hk/\"}".into(),
            "browser_downloads" => "no downloads".into(),
            _ => hk_answers(name, a),
        },
    );
    assert_eq!(names(&calls), ["browser_navigate", "browser_downloads"]);
    assert_eq!(res["status"], "error");
}

// ---------------------------------------------------------------- the FWD Singapore purchase

/// FWD's quote summary as `browser_read_elements` reads it (the real page, 2026-11-02: a single trip to Asia for one,
/// 25 Nov – 2 Dec): the plans' buttons, the prices, the benefit lines.
fn summary() -> Value {
    let line = |l: &str, a: &str, b: &str, c: &str| json!({"label": l, "p1": a, "p2": b, "p3": c});
    json!({
        "buy": ["TI_QuoteSummarypage_Premium_Buy", "TI_QuoteSummarypage_Business_Buy", "TI_QuoteSummarypage_First_Buy"],
        "price": ["S$42.52", "S$66.08", "S$84.81"],
        "lines": [
            line("Overseas medical expenses (per adult below 70 years old)", "S$200,000", "S$500,000", "S$1,000,000"),
            line("Theft of or damage to your personal belongings Note: sub-limit of S$300 per single item for Premium plan", "S$3,000", "S$5,000", "S$7,500"),
            line("Baggage delay (S$150 for every six hours of delay overseas and S$150 after six hours of delay in Singapore)", "S$150", "S$600", "S$900"),
            line("Travel delay (S$100 for every six hours of delay overseas and S$100 after six hours of delay in Singapore)", "S$300", "S$800", "S$1,500"),
            line("Trip cancelled before you leave Singapore", "S$7,500", "S$10,000", "S$15,000"),
            line("Trip cancelled before you leave Singapore for Family Plans", "S$18,750", "S$25,000", "S$37,500"),
        ]
    })
}

#[test]
fn the_plans_are_read_from_the_summary_and_picked_by_rule() {
    let url = "https://www.fwd.com.sg/travel-insurance/online-quote/summary";
    let m = json!({"role": "tool", "content": elements(url, summary())});
    let plans = plans_of(&m);
    assert_eq!(
        plans.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(),
        ["Premium", "Business", "First"]
    );
    assert_eq!(plans[1].premium, 66.08);
    assert_eq!(plans[1].cover_of("medical"), Some(500_000.0));
    assert_eq!(plans[1].cover_of("delay"), Some(800.0));
    assert_eq!(plans[1].cover_of("baggage"), Some(600.0));
    // the single traveller's line, not the family plans'
    assert_eq!(plans[1].cover_of("cancellation"), Some(10_000.0));
    let (p, rule) = pick_plan(&plans, &Some(("cancellation".into(), 10_000.0)), "").unwrap();
    assert_eq!(
        (p.name.as_str(), rule.as_str()),
        (
            "Business",
            "the cheapest plan with cancellation cover of at least 10,000"
        )
    );
    let (p, _) = pick_plan(&plans, &Some(("medical".into(), 800_000.0)), "").unwrap();
    assert_eq!(p.name, "First");
    let (p, rule) = pick_plan(&plans, &None, "").unwrap();
    assert_eq!(
        (p.name.as_str(), rule.as_str()),
        ("Premium", "the cheapest plan")
    );
    let (p, rule) = pick_plan(&plans, &Some(("medical".into(), 9_000_000.0)), "").unwrap();
    assert!(p.name == "Premium" && rule.starts_with("no plan has medical cover"));
    // the plan the user chose on the card wins
    let (p, rule) = pick_plan(&plans, &None, "first").unwrap();
    assert_eq!(
        (p.name.as_str(), rule.as_str()),
        ("First", "the plan you chose")
    );
    // a phrase around the name is not read: the default rule picks
    assert_ne!(
        pick_plan(&plans, &None, "I choose: First").unwrap().1,
        "the plan you chose"
    );
    // a button that is not a plan's is not a plan
    let odd = json!({"role": "tool", "content": elements(url, json!({"buy": ["TI_QuoteSummarypage_x\"]_Buy", "other"], "price": ["S$1", "S$2"], "lines": []}))});
    assert!(plans_of(&odd).is_empty());
    // the Vault's cards, and the one named in the argument (the APDT and CSR flows' shared helpers)
    let listing = "- id=v1 · DBS Altitude Visa Signature · issued in Singapore · card · card_id=sg-dbs-altitude-visa-signature · •••• UNKNOWN · fields: number, expiry, cvc, holder · number, expiry, cvc UNKNOWN (name only)\n- id=v2 · Visa Wei Ming · card · •••• 1111 · fields: number, expiry, cvc, holder · only on: fwd.com.sg\n- id=v3 · Passport Wei Ming · id_document · •••• 567Z · fields: number, expiry";
    let cards = vault_cards(listing);
    assert_eq!(cards.len(), 2);
    for named in [
        "v1",
        "sg-dbs-altitude-visa-signature",
        "dbs altitude visa signature",
    ] {
        assert_eq!(
            pick_card(&cards, &[named.into()]).map(|c| c.id.as_str()),
            Some("v1"),
            "{named}"
        );
    }
    assert_eq!(
        pick_card(&cards, &["DBS Altitude".into()]).map(|c| c.id.as_str()),
        Some("v2")
    );
}

fn buy_answers(name: &str, args: &Value) -> String {
    match name {
        "browser_navigate" => page(
            "https://www.fwd.com.sg/travel-insurance/online-quote/quickQuestions",
            "{}",
        ),
        "form_fill" => "{\"url\": \"https://www.fwd.com.sg/travel-insurance/online-quote/summary\", \"filled\": []}".into(),
        "browser_read_elements" if args["read"][0]["name"] == "buy" => elements(
            "https://www.fwd.com.sg/travel-insurance/online-quote/summary",
            summary(),
        ),
        "present_choices" => "Shown to the user as 3 cards".into(),
        "browser_request_takeover" => BACK.into(),
        other => panic!("{other}"),
    }
}

#[test]
fn the_purchase_quotes_picks_by_rule_and_hands_the_declaration_over() {
    let profile = json!({"name_en": "TAN WEI MING", "birthday": "1985-04-12"});
    let args = valid(
        INSURANCE_BUY,
        json!({"market": "sg", "action": "buy", "destination": "Tokyo", "depart": "2026-11-25", "return": "2026-12-02",
               "travellers": 1, "cover": "cancellation", "cover_at_least": 10000, "card": "Visa Wei Ming"}),
        false,
        &[],
    );
    assert_eq!(args["destination"], "Japan");
    let (mut t, from) = started(
        Vec::new(),
        "Get me the cheapest FWD plan with at least SGD 10,000 trip cancellation.",
        INSURANCE_BUY,
        &args,
    );
    let (calls, res) = run(INSURANCE_BUY, &args, &mut t, from, &profile, buy_answers);
    // no card fill, no payment approval: the payment page lies behind FWD's eligibility statements, the user's
    assert_eq!(
        names(&calls),
        [
            "browser_navigate",
            "form_fill",
            "browser_read_elements",
            "present_choices",
            "form_fill",
            "browser_request_takeover"
        ]
    );
    let quote = &calls[1].args;
    assert_eq!(
        (
            sget(quote, "destination").as_str(),
            sget(quote, "depart").as_str(),
            sget(quote, "return").as_str(),
            sget(quote, "travellers").as_str(),
            sget(quote, "today").as_str()
        ),
        ("Japan", "2026-11-25", "2026-12-02", "1", "2026-10-31")
    );
    assert_eq!(sget(&calls[3].args, "picked"), "Business");
    assert_eq!(
        (
            sget(&calls[4].args, "stage").as_str(),
            sget(&calls[4].args, "plan").as_str()
        ),
        ("pick", "Business")
    );
    let reason = sget(&calls[5].args, "reason");
    assert!(
        !reason.starts_with(OWN_STEP)
            && reason.contains("eligibility statements")
            && reason.contains("press Pay"),
        "{reason}"
    );
    assert_eq!(res["status"], "handed_over");
    let m = must(&res);
    assert!(
        m.contains("- Chosen: Business: SGD 66.08\n- Why: the cheapest plan with cancellation cover of at least 10,000")
            && m.contains("Premium — SGD 42.52")
            && m.contains("First — SGD 84.81")
            && m.contains("nothing has been paid"),
        "{m}"
    );
    // compare only: the plans shown, nothing picked on the page
    let args3 = valid(
        INSURANCE_BUY,
        json!({"action": "compare", "destination": "Japan", "depart": "2026-11-25", "return": "2026-12-02"}),
        false,
        &[],
    );
    let (mut t, from) = started(
        Vec::new(),
        "Which FWD plan is cheapest for my Japan trip?",
        INSURANCE_BUY,
        &args3,
    );
    let (calls, res) = run(INSURANCE_BUY, &args3, &mut t, from, &profile, buy_answers);
    assert_eq!(
        names(&calls),
        [
            "browser_navigate",
            "form_fill",
            "browser_read_elements",
            "present_choices"
        ]
    );
    assert_eq!(res["status"], "shown");
    // Hong Kong: no form function — the plan page goes to browser-use
    let hk = valid(INSURANCE_BUY, json!({"market": "hk"}), false, &[]);
    let (mut t, from) = started(Vec::new(), "buy FWD HK insurance", INSURANCE_BUY, &hk);
    let (_, res) = run(INSURANCE_BUY, &hk, &mut t, from, &profile, |_, _| {
        "ok".into()
    });
    assert_eq!(res["status"], "browser_use");
}

// ---------------------------------------------------------------- the WorldTracer lookup

const MANAGE: &str = "https://mybag.aero/baggage/#/pax/ana/en-gb/delayed/manage-bag";

#[test]
fn the_bag_is_looked_up_on_the_airlines_worldtracer_page_and_its_answer_shown() {
    let profile = json!({"name_en": "Wei Ming Tan"});
    let args = valid(
        BAG_TRACE,
        json!({"file_ref": "NRT NH 12345", "flight_date": "2026-10-30", "watch": true}),
        false,
        &[],
    );
    assert_eq!(args["file_ref"], "NRTNH12345");
    let (mut t, from) = started(
        Vec::new(),
        "Check where my bag is (file NRTNH12345) and keep an eye on it.",
        BAG_TRACE,
        &args,
    );
    let (calls, res) = run(BAG_TRACE, &args, &mut t, from, &profile, |name, _| {
        match name {
        "browser_navigate" => page(MANAGE, "Manage your delayed baggage report"),
        "form_fill" => "{\"url\": \"x\", \"filled\": [{\"op\": \"click\", \"field\": \"log_in\", \"ok\": true}]}".into(),
        // the real page's error box for a reference it does not know
        "browser_read_elements" => elements(MANAGE, json!({"error_head": ["There was a problem in finding your details"],
            "error_text": ["No record was found with that reference number and family name combination. Check the values entered and try again or"],
            "login": ["mngRptRefNoLTxt"]})),
        "schedule_create" => "created".into(),
        other => panic!("{other}"),
    }
    });
    // no record for that reference: no check is made on it (claims_real 2026-11-02)
    assert_eq!(
        names(&calls),
        ["browser_navigate", "form_fill", "browser_read_elements"]
    );
    assert_eq!(sget(&calls[0].args, "url"), MANAGE);
    assert_eq!(calls[1].args, json!({"file_ref": "NRTNH12345"}));
    assert!(calls.iter().all(super::super::guards::code_call));
    assert_eq!(res["status"], "no_file");
    let m = must(&res);
    assert!(
        m.contains("- WorldTracer file: **NRTNH12345**\n- Bag status: WorldTracer says: No record was found with that reference number and family name combination")
            && m.contains("- Treated as lost after: 20 November 2026 (21 days after 30 October 2026)")
            && m.contains("- Bag check: not set up — WorldTracer has no file"),
        "{m}"
    );
    // the report opened (the sign-in form gone)
    let args = valid(
        BAG_TRACE,
        json!({"file_ref": "NRTNH12345", "family_name": "tan", "watch": true}),
        false,
        &[],
    );
    let (mut t, from) = started(Vec::new(), "where is my bag", BAG_TRACE, &args);
    let (calls, res) = run(
        BAG_TRACE,
        &args,
        &mut t,
        from,
        &profile,
        |name, _| match name {
            "browser_read_elements" => elements(
                MANAGE,
                json!({"error_head": [], "error_text": [], "login": []}),
            ),
            "form_fill" => "{\"url\": \"x\", \"filled\": []}".into(),
            "schedule_create" => "created".into(),
            _ => page(MANAGE, "Manage your delayed baggage report"),
        },
    );
    assert_eq!(calls[1].args["family_name"], "TAN");
    // the file is found: the check twice a day is made
    assert_eq!(names(&calls)[3], "schedule_create");
    assert_eq!(sget(&calls[3].args, "spec"), "720");
    assert_eq!(res["status"], "traced");
    assert!(must(&res).contains("WorldTracer has the report"));
    assert!(must(&res).contains("- Bag check: twice a day"));
    // a failed page: the result says so, no retry
    let (mut t, from) = started(Vec::new(), "where is my bag", BAG_TRACE, &args);
    let (calls, res) = run(BAG_TRACE, &args, &mut t, from, &profile, |_, _| {
        "ERROR: navigation failed: net::ERR_NAME_NOT_RESOLVED".into()
    });
    assert_eq!(names(&calls), ["browser_navigate"]);
    assert_eq!(res["status"], "error");
    // an airline that does not use mybag.aero: said so, nothing opened
    let sq = valid(BAG_TRACE, json!({"file_ref": "NRTSQ12345"}), false, &[]);
    let (mut t, from) = started(Vec::new(), "where is my bag", BAG_TRACE, &sq);
    let (calls, res) = run(BAG_TRACE, &sq, &mut t, from, &profile, |_, _| "ok".into());
    assert!(calls.is_empty());
    assert_eq!(res["status"], "not_on_worldtracer");
    assert!(must(&res).contains("the airline SQ does not track bags on WorldTracer"));
}

// ---------------------------------------------------------------- the protocol around the machine

#[test]
fn ask_user_before_the_claim_tool_is_refused() {
    let ev = |t: &str, data: Value| json!({"type": t, "data": data});
    let loaded = vec![ev(
        "tool_call",
        json!({"name": "load_skill", "args": {"name": "csr-trip-claim"}}),
    )];
    let r = ask_before_tool(&loaded).expect("refused");
    assert_eq!(r["tool"], CLAIM_FILE);
    let mut called = loaded.clone();
    called.push(ev("workflow_call", json!({"tool": CLAIM_FILE})));
    assert!(ask_before_tool(&called).is_none());
    assert!(ask_before_tool(&[ev(
        "tool_call",
        json!({"name": "load_skill", "args": {"name": "baggage-delayed-lost"}})
    )])
    .is_none());
}

#[test]
fn a_claim_run_is_active_until_its_result() {
    let args = csr_cancellation();
    let (t, _) = started(Vec::new(), "claim", CLAIM_FILE, &args);
    let (i, name, a) = workflow::active(&t, &TOOLS).unwrap();
    assert_eq!((i, name.as_str()), (1, CLAIM_FILE));
    assert_eq!(a["benefit"], "trip_cancellation");
}

#[test]
fn amounts_and_money() {
    assert_eq!(
        amounts_in("Fare USD 1,840.00, taxes $86.40 and 250.00 SGD"),
        vec![1840.0, 86.40, 250.0]
    );
    assert_eq!(money(1753.6), "1,753.60");
    assert_eq!(money(1_000_000.0), "1,000,000.00");
    assert!(amounts_in("Visa ending 4321 on 2026-09-21").is_empty());
    assert_eq!(hours_text(435), "7 h 15 min");
}
