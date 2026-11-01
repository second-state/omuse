use super::*;
use crate::agent::driver::questions_after_navigate;

const TT: &str = "http://shop.test:8097";
const SHOP: &str = "http://shop.test:8099";

fn today() -> NaiveDate {
    NaiveDate::from_ymd_opt(2026, 11, 1).unwrap() // a Sunday
}

fn profile() -> Value {
    json!({"name_en": "Jordan Lu", "email_personal": "jordan@example.com", "phone": "+65 9123 4567",
           "address_home": "10 Anson Road, #12-01, Singapore 079903"})
}

fn loaded(skill: &str, ask: &str) -> Vec<Value> {
    vec![
        json!({"role": "user", "content": ask}),
        json!({"role": "assistant", "content": "", "tool_calls": [{"id": "k1", "type": "function",
               "function": {"name": "load_skill", "arguments": json!({"name": skill}).to_string()}}]}),
        json!({"role": "tool", "tool_call_id": "k1", "content": "---"}),
    ]
}

fn spec(skill: &str) -> &'static BookingSpec {
    SPECS.iter().find(|s| s.skill == skill).unwrap()
}

fn names(t: &[Value]) -> Vec<String> {
    t.iter()
        .flat_map(tool_calls)
        .map(|tc| stored_call(tc).0)
        .collect()
}

fn args_of(t: &[Value], name: &str) -> Vec<Value> {
    t.iter()
        .flat_map(tool_calls)
        .map(stored_call)
        .filter(|(n, _)| n == name)
        .map(|(_, a)| a)
        .collect()
}

fn snap(url: &str, body: &str) -> String {
    json!({"trust": "untrusted", "source": format!("web page {url}"), "url": url, "title": "", "snapshot": body}).to_string()
}

const BACK: &str = "\"Takeover finished: the user handed browser control back.\"";
const VAULT: &str = "Vault items:\n- id=card1 · Visa Jordan · card · number ****1111\n- id=pp1 · Passport · id_document · number ****321";

/// Runs the machine from `t`, answering each call with `answer(name, args, transcript)`, until a final text; panics on
/// an extraction, on a question to the user after a page opened, and on a run that does not end.
fn run(
    mut t: Vec<Value>,
    next: impl Fn(&[Value]) -> Step,
    answer: impl Fn(&str, &Value, &[Value]) -> String,
) -> (Vec<Value>, String) {
    for _ in 0..60 {
        match next(&t) {
            Step::Calls(c) => {
                assert_eq!(c.len(), 1, "one call per turn");
                for (j, call) in c.iter().enumerate() {
                    let id = format!("bkg_r{}_{j}", t.len());
                    let content = answer(&call.name, &call.args, &t);
                    t.push(json!({"role": "assistant", "content": "", "tool_calls": [{"id": id, "type": "function",
                        "function": {"name": call.name, "arguments": call.args.to_string()}}]}));
                    t.push(json!({"role": "tool", "tool_call_id": id, "content": content}));
                }
                let q = questions_after_navigate(&t);
                assert!(q.is_empty(), "a question after a page opened: {q:?}");
                if c[0].name == "load_skill" {
                    return (t, String::new());
                }
            }
            Step::Final(a) => return (t, a),
            Step::Extract(x) => panic!("an extraction: {x}"),
        }
    }
    panic!("the run did not end")
}

fn handed_back_in(t: &[Value]) -> bool {
    t.iter()
        .any(|m| get_str(m, "role") == "tool" && content_of(m).contains("Takeover finished"))
}

// ---------------------------------------------------------------- the request

#[test]
fn the_request_is_read_by_rule() {
    let p = profile();
    let r = parse_request(
        Kind::Flight,
        &["Book the cheapest direct flight from Singapore to Tokyo next Friday in business class for 2 adults, under S$1,500.".into()],
        &p,
        today(),
        None,
    );
    assert_eq!((r.origin.as_str(), r.destination.as_str()), ("SIN", "TYO"));
    assert_eq!(r.date, NaiveDate::from_ymd_opt(2026, 11, 6));
    assert_eq!((r.cabin, r.nonstop, r.party), ("business", true, Some(2)));
    assert_eq!(r.max_price, Some(1500.0));
    // one city: the destination, the origin from the home address
    let r = parse_request(
        Kind::Flight,
        &["Find me a flight to Bangkok on 3 December".into()],
        &p,
        today(),
        None,
    );
    assert_eq!((r.origin.as_str(), r.destination.as_str()), ("SIN", "BKK"));
    assert_eq!(r.date, NaiveDate::from_ymd_opt(2026, 12, 3));
    // a round trip only with return words
    let r = parse_request(
        Kind::Flight,
        &["Fly SIN to HKG on 2026-12-03, returning 2026-12-08".into()],
        &p,
        today(),
        None,
    );
    assert_eq!(r.return_date, NaiveDate::from_ymd_opt(2026, 12, 8));
    // a hotel: check-in and nights; the stars, free cancellation
    let r = parse_request(
        Kind::Hotel,
        &[
            "A 4-star hotel in Tokyo for 3 nights from 10 December, free cancellation please"
                .into(),
        ],
        &p,
        today(),
        None,
    );
    assert_eq!(r.city, "Tokyo");
    assert_eq!(
        (r.date, r.check_out),
        (
            NaiveDate::from_ymd_opt(2026, 12, 10),
            NaiveDate::from_ymd_opt(2026, 12, 13)
        )
    );
    assert_eq!((r.min_stars, r.free_cancel), (Some(4), true));
    // "2 nights next week" through the relative-date rule
    let r = parse_request(
        Kind::Hotel,
        &["Book me 2 nights in Singapore next week".into()],
        &p,
        today(),
        None,
    );
    assert_eq!(
        (r.date, r.check_out),
        (
            NaiveDate::from_ymd_opt(2026, 11, 2),
            NaiveDate::from_ymd_opt(2026, 11, 4)
        )
    );
    // a restaurant: the time, the distinction, the occasion, the default dinner time
    let r = parse_request(
        Kind::Michelin,
        &["A 2-star kaiseki dinner in Tokyo on 5 December around 7pm for my anniversary".into()],
        &p,
        today(),
        None,
    );
    assert_eq!(
        (
            r.distinction.as_str(),
            r.cuisine.as_str(),
            r.time.as_str(),
            r.occasion.as_str()
        ),
        ("2 stars", "kaiseki", "19:00", "anniversary")
    );
    let r = parse_request(
        Kind::Restaurant,
        &["Find a ramen place in Osaka tomorrow, table for 4".into()],
        &p,
        today(),
        None,
    );
    assert_eq!(
        (r.city.as_str(), r.time.as_str(), r.party),
        ("Osaka", "19:00", Some(4))
    );
    assert_eq!(r.date, NaiveDate::from_ymd_opt(2026, 11, 2));
    // the extraction's facts fill only what the regexes left empty
    let f = json!({"origin": "Sydney", "destination": "Tokyo", "date": "2026-12-01"});
    let r = parse_request(
        Kind::Flight,
        &["get me there cheaply".into()],
        &json!({}),
        today(),
        Some(&f),
    );
    assert_eq!((r.origin.as_str(), r.destination.as_str()), ("SYD", "TYO"));
}

#[test]
fn the_machine_runs_only_when_the_user_asks_for_the_work() {
    assert!(active(&loaded(
        "hotel-booking",
        "Is it usual to tip at hotels in Japan?"
    ))
    .is_none());
    assert_eq!(
        active(&loaded(
            "hotel-booking",
            "Find me a hotel in Tokyo for 2 nights next week"
        ))
        .map(|s| s.kind),
        Some(Kind::Hotel)
    );
    assert_eq!(
        active(&loaded(
            "flight-booking",
            "Book the cheapest flight to Seoul tomorrow"
        ))
        .map(|s| s.kind),
        Some(Kind::Flight)
    );
    // shopping needs a shop link in the user's words; otherwise the model path keeps it
    assert!(active(&loaded("online-shopping", "Buy me a phone case")).is_none());
    assert!(active(&loaded(
        "online-shopping",
        "Buy this: https://www.example-shop.com/p/123"
    ))
    .is_some());
    // once the page went to browser-use, the model path has it
    let mut t = loaded(
        "hotel-booking",
        "Book a hotel in Tokyo from 10 December to 12 December 2026",
    );
    t.push(json!({"role": "assistant", "content": "", "tool_calls": [{"id": "bkg_9", "type": "function",
           "function": {"name": "load_skill", "arguments": "{\"name\": \"browser-use\"}"}}]}));
    t.push(json!({"role": "tool", "tool_call_id": "bkg_9", "content": "---"}));
    assert!(active(&t).is_none());
}

#[test]
fn a_missing_parameter_is_extracted_then_asked_once_before_the_search() {
    let s = spec("flight-booking");
    let p = json!({});
    let user = vec!["Book me a flight please".to_string()];
    let w = World {
        user_texts: &user,
        profile: &p,
        today: today(),
        mail_ready: false,
        zh: false,
    };
    let mut t = loaded(s.skill, &user[0]);
    let Step::Extract(x) = next(s, &t, &w) else {
        panic!("expected the extraction")
    };
    assert!(
        x.starts_with("Today: 2026-11-01") && x.contains("Book me a flight"),
        "{x}"
    );
    t.extend(extract_exchange(1, &x, &json!({})));
    let Step::Final(q) = next(s, &t, &w) else {
        panic!("expected the question")
    };
    assert!(
        q.starts_with(ASK_MARK)
            && q.contains("where you fly from")
            && q.contains("the departure date"),
        "{q}"
    );
    t.push(json!({"role": "assistant", "content": q}));
    // the user's answer: one more extraction, then the search
    let user2 = vec![
        user[0].clone(),
        "from Sydney to Tokyo, first week of December".to_string(),
    ];
    t.push(json!({"role": "user", "content": user2[1]}));
    let w2 = World {
        user_texts: &user2,
        profile: &p,
        today: today(),
        mail_ready: false,
        zh: false,
    };
    let Step::Extract(x) = next(s, &t, &w2) else {
        panic!("expected the second extraction")
    };
    t.extend(extract_exchange(2, &x, &json!({"date": "2026-12-01"})));
    let Step::Calls(c) = next(s, &t, &w2) else {
        panic!("expected the search")
    };
    assert_eq!(c[0].name, "flights_search");
    assert_eq!(c[0].args["origin"], "SYD");
    assert_eq!(c[0].args["date"], "2026-12-01");
}

// ---------------------------------------------------------------- flights

fn flight_result() -> String {
    let o = |i: u32, title: &str, airline: &str, stops: u32, fare: u32| {
        json!({"offer_id": format!("F-SIN-HND-20261203-{i}"), "stops": stops, "fare_amount": fare, "title": title, "airline": airline,
               "schedule": "SIN 08:00 - HND 16:00", "duration": "7h 00m", "stops_text": if stops == 0 { "Direct" } else { "1 stop via Hong Kong (HKG)" },
               "fare": format!("SGD {fare}.00"), "baggage": "30 kg checked + 7 kg cabin", "refund_rules": "Refundable with fee SGD 150",
               "currency": "SGD", "booking_url": format!("{TT}/book/flight/F-SIN-HND-20261203-{i}")})
    };
    json!({"route": "Singapore -> Tokyo", "count": 3, "offers": [
        o(1, "Cathay Pacific CX 712", "Cathay Pacific", 1, 380),
        o(2, "Singapore Airlines SQ 638", "Singapore Airlines", 0, 520),
        o(3, "Japan Airlines JL 36", "Japan Airlines", 0, 455)]})
    .to_string()
}

#[test]
fn a_flight_comparison_picks_by_rule_and_books_nothing() {
    let s = spec("flight-booking");
    let p = profile();
    let user = vec![
        "Find me the cheapest direct flight from Singapore to Tokyo on 3 December 2026."
            .to_string(),
    ];
    let w = World {
        user_texts: &user,
        profile: &p,
        today: today(),
        mail_ready: false,
        zh: false,
    };
    let t = loaded(s.skill, &user[0]);
    assert!(active(&t).is_some());
    let (t, a) = run(
        t,
        |t| next(s, t, &w),
        |name, _, _| match name {
            "flights_search" => flight_result(),
            _ => "{\"shown\": 3, \"verified\": true}".into(),
        },
    );
    let search = &args_of(&t, "flights_search")[0];
    assert_eq!(search["origin"], "SIN");
    assert_eq!(search["destination"], "TYO");
    assert_eq!(
        (
            search["date"].as_str(),
            search["stops"].as_str(),
            search["cabin"].as_str()
        ),
        (Some("2026-12-03"), Some("nonstop"), Some("economy"))
    );
    let pc = &args_of(&t, "present_choices")[0];
    assert_eq!(pc["picked"], "Japan Airlines JL 36");
    assert!(pc["options"]
        .as_array()
        .unwrap()
        .iter()
        .all(|o| sget(o, "label") != "Cathay Pacific CX 712"));
    assert_eq!(
        names(&t)
            .iter()
            .filter(|n| n.starts_with("browser_") || *n == "purchase_confirm")
            .count(),
        0
    );
    assert!(
        a.contains("Japan Airlines JL 36")
            && a.contains("direct, the cheapest")
            && a.contains("/book/flight/F-SIN-HND-20261203-3"),
        "{a}"
    );
}

#[test]
fn a_flight_is_booked_on_a_known_site_with_fixed_ids_and_handed_over() {
    let s = spec("flight-booking");
    let p = profile();
    let user = vec![
        "Book the cheapest flight from Singapore to Tokyo next Friday, business class.".to_string(),
    ];
    let w = World {
        user_texts: &user,
        profile: &p,
        today: today(),
        mail_ready: false,
        zh: false,
    };
    let form = "[e10] textbox \"Full name (as in passport)\"\n[e11] textbox \"Email\"\n[e12] textbox \"Phone\"\n[e13] textbox \"Passport number\"\n\
[e20] textbox \"Card number\"\n[e21] textbox \"Expiry (MM/YY)\"\n[e22] textbox \"CVC\"\n[e23] textbox \"Name on card\"\n[e30] button \"Pay now\"";
    let (t, a) = run(
        loaded(s.skill, &user[0]),
        |t| next(s, t, &w),
        |name, args, t| match name {
            "flights_search" => flight_result(),
            "vault_list" => VAULT.into(),
            "purchase_confirm" => "{\"status\": \"approved\", \"purchase_id\": 7}".into(),
            "browser_navigate" => snap(&sget(args, "url"), form),
            "browser_fill_form" => "{\"url\": \"x\", \"filled\": [], \"skipped\": []}".into(),
            "browser_fill_secret" => "{\"filled\": \"filled: Visa\"}".into(),
            "browser_request_takeover" => BACK.into(),
            "browser_snapshot" if handed_back_in(t) => snap(
                &format!("{TT}/book/flight/F-SIN-HND-20261203-1"),
                "Booking confirmed\nBooking reference: TRV-F1A2B3C\nTotal paid: SGD 380.00",
            ),
            "browser_snapshot" => snap(&format!("{TT}/book/flight/F-SIN-HND-20261203-1"), form),
            _ => "{}".into(),
        },
    );
    assert_eq!(args_of(&t, "flights_search")[0]["date"], "2026-11-06");
    assert_eq!(args_of(&t, "flights_search")[0]["cabin"], "business");
    let seq: Vec<String> = names(&t).into_iter().skip(1).collect();
    assert_eq!(
        seq,
        [
            "flights_search",
            "present_choices",
            "vault_list",
            "purchase_confirm",
            "browser_navigate",
            "browser_fill_form",
            "browser_snapshot",
            "browser_fill_secret",
            "browser_fill_secret",
            "browser_fill_secret",
            "browser_fill_secret",
            "browser_request_takeover",
            "browser_snapshot"
        ]
    );
    let pc = &args_of(&t, "purchase_confirm")[0];
    assert_eq!(
        (
            pc["site"].as_str(),
            pc["total"].as_f64(),
            pc["card_item_id"].as_str()
        ),
        (Some("shop.test"), Some(380.0), Some("card1"))
    );
    assert_eq!(
        args_of(&t, "browser_navigate")[0]["url"],
        format!("{TT}/book/flight/F-SIN-HND-20261203-1")
    );
    // the plan: fixed ids, the passport from the Vault (never a value in the plan)
    let fill = &args_of(&t, "browser_fill_form")[0];
    assert_eq!(fill["form"], "traveltest-flight");
    let sels: Vec<String> = fill["fills"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| sget(f, "sel"))
        .collect();
    assert_eq!(sels, ["#full_name", "#email", "#phone", "#passport"]);
    assert_eq!(fill["fills"][0]["value"], "JORDAN LU");
    assert_eq!(fill["fills"][3]["vault"]["field"], "number");
    let secrets: Vec<(String, String)> = args_of(&t, "browser_fill_secret")
        .iter()
        .map(|a| (sget(a, "ref"), sget(a, "field")))
        .collect();
    assert_eq!(
        secrets,
        [
            ("e20".into(), "number".into()),
            ("e21".into(), "expiry".into()),
            ("e22".into(), "cvc".into()),
            ("e23".into(), "holder".into())
        ]
    );
    let reason = sget(&args_of(&t, "browser_request_takeover")[0], "reason");
    assert!(
        reason.contains("press Pay now yourself")
            && reason.contains("Visa Jordan")
            && reason.contains("SGD 380.00"),
        "{reason}"
    );
    assert!(
        a.contains("**TRV-F1A2B3C**")
            && a.contains("Cathay Pacific CX 712")
            && a.contains("SGD 380.00"),
        "{a}"
    );
}

// ---------------------------------------------------------------- hotels

#[test]
fn an_unknown_booking_site_is_opened_and_passed_to_browser_use_with_no_fill_by_code() {
    let s = spec("hotel-booking");
    let p = profile();
    let user = vec![
        "Book a hotel in Tokyo from 10 December to 12 December 2026 for 2 guests.".to_string(),
    ];
    let w = World {
        user_texts: &user,
        profile: &p,
        today: today(),
        mail_ready: false,
        zh: false,
    };
    let res = json!({"count": 2, "currency": "SGD", "hotels": [
        {"name": "Park Hyatt Tokyo", "stars": 5, "rating": 4.6, "price_per_night": 900, "booking_url": "https://www.hyatt.com/book/pht?d=1210", "free_cancellation": true},
        {"name": "Hotel Gracery Shinjuku", "stars": 3, "rating": 4.1, "price_per_night": 210, "booking_url": "https://www.google.com/travel/hotels/x?d=1210", "free_cancellation": false}]});
    let (t, _) = run(
        loaded(s.skill, &user[0]),
        |t| next(s, t, &w),
        |name, args, _| match name {
            "hotels_search" => res.to_string(),
            "browser_navigate" => snap(
                &sget(args, "url"),
                "[e5] textbox \"First name\"\n[e6] textbox \"Last name\"\n[e9] button \"Book now\"",
            ),
            _ => "{\"shown\": 2}".into(),
        },
    );
    let search = &args_of(&t, "hotels_search")[0];
    assert_eq!(
        (
            search["check_in"].as_str(),
            search["check_out"].as_str(),
            search["guests"].as_u64()
        ),
        (Some("2026-12-10"), Some("2026-12-12"), Some(2))
    );
    assert_eq!(
        args_of(&t, "browser_navigate")[0]["url"],
        "https://www.google.com/travel/hotels/x?d=1210"
    );
    let seq = names(&t);
    assert_eq!(seq.last().map(String::as_str), Some("load_skill"));
    assert_eq!(
        args_of(&t, "load_skill").last().unwrap()["name"],
        BROWSER_USE
    );
    for n in [
        "browser_fill_form",
        "browser_fill_secret",
        "browser_type",
        "browser_click",
        "form_fill",
    ] {
        assert!(
            !seq.iter().any(|x| x == n),
            "code filled the unknown page: {seq:?}"
        );
    }
    assert!(active(&t).is_none(), "the model path has the page now");
}

#[test]
fn a_hotel_room_is_picked_by_rule_and_a_pay_at_hotel_rate_needs_no_card() {
    let s = spec("hotel-booking");
    let p = profile();
    let user =
        vec!["Book a 4-star hotel in Tokyo, check in 2026-12-10, 2 nights, 2 guests.".to_string()];
    let w = World {
        user_texts: &user,
        profile: &p,
        today: today(),
        mail_ready: false,
        zh: false,
    };
    let hotels = json!({"city": "Tokyo", "count": 3, "hotels": [
        {"hotel_id": "H-TYO-1", "name": "Hotel Ginza Grand", "stars": 4, "rating": "8.6/10", "area": "Ginza", "from_price_per_night": "SGD 210.00", "booking_url": format!("{TT}/book/hotel/H-TYO-1")},
        {"hotel_id": "H-TYO-2", "name": "Shinjuku Budget Inn", "stars": 2, "rating": "7.1/10", "area": "Shinjuku", "from_price_per_night": "SGD 95.00", "booking_url": format!("{TT}/book/hotel/H-TYO-2")},
        {"hotel_id": "H-TYO-3", "name": "Asakusa View", "stars": 4, "rating": "8.9/10", "area": "Asakusa", "from_price_per_night": "SGD 260.00", "booking_url": format!("{TT}/book/hotel/H-TYO-3")}]});
    let room = |i: u32, ty: &str, nightly: u32, cancel: &str, pay: &str| {
        json!({"room_offer_id": format!("H-TYO-1-20261210-20261212-2-R{i}"), "room_type": ty, "price_per_night": format!("SGD {nightly}.00"),
               "nights": 2, "total_amount": nightly * 2, "total": format!("SGD {}.00 for 2 nights", nightly * 2), "cancellation": cancel, "payment": pay,
               "booking_url": format!("{TT}/book/hotel/H-TYO-1-20261210-20261212-2-R{i}")})
    };
    let rooms = json!({"hotel": {"name": "Hotel Ginza Grand"}, "rooms": [
        room(1, "Standard Double", 190, "Non-refundable: no changes or refund after booking", "Prepay now by card"),
        room(2, "Superior Twin", 210, "Free cancellation until 2026-12-07 23:59 hotel local time", "Pay at the hotel (no prepayment)"),
        room(3, "Deluxe King with breakfast", 280, "Free cancellation until 2026-12-09 23:59 hotel local time", "Pay at the hotel (no prepayment)")]});
    let form = "[e10] textbox \"Full name\"\n[e11] textbox \"Email\"\n[e12] textbox \"Phone\"\n[e13] textbox \"Estimated arrival time (optional)\"\n[e14] textbox \"Special requests (optional)\"\n[e20] textbox \"Card number (optional)\"";
    let (t, a) = run(
        loaded(s.skill, &user[0]),
        |t| next(s, t, &w),
        |name, args, t| match name {
            "hotels_search" => hotels.to_string(),
            "hotel_rooms" => rooms.to_string(),
            "browser_navigate" => snap(&sget(args, "url"), form),
            "browser_fill_form" => "{\"url\": \"x\", \"filled\": []}".into(),
            "browser_request_takeover" => BACK.into(),
            "browser_snapshot" if handed_back_in(t) => {
                snap("x", "Booking confirmed\nBooking reference: TRV-HAB12CD")
            }
            _ => "{\"shown\": 2}".into(),
        },
    );
    assert_eq!(args_of(&t, "hotels_search")[0]["min_stars"], 4);
    assert_eq!(
        args_of(&t, "present_choices")[0]["picked"],
        "Hotel Ginza Grand"
    );
    assert_eq!(args_of(&t, "hotel_rooms")[0]["hotel_id"], "H-TYO-1");
    // the cheapest room with free cancellation, not the cheaper non-refundable one; pay at the hotel: no card
    assert_eq!(
        args_of(&t, "browser_navigate")[0]["url"],
        format!("{TT}/book/hotel/H-TYO-1-20261210-20261212-2-R2")
    );
    let seq = names(&t);
    for n in ["purchase_confirm", "vault_list", "browser_fill_secret"] {
        assert!(
            !seq.iter().any(|x| x == n),
            "{n} for a pay-at-hotel rate: {seq:?}"
        );
    }
    let fill = &args_of(&t, "browser_fill_form")[0];
    assert_eq!(fill["form"], "traveltest-hotel");
    assert_eq!(fill["fills"][0]["value"], "Jordan Lu");
    let reason = sget(&args_of(&t, "browser_request_takeover")[0], "reason");
    assert!(
        reason.contains("press Confirm booking yourself") && reason.contains("Superior Twin"),
        "{reason}"
    );
    assert!(
        a.contains("**TRV-HAB12CD**")
            && a.contains("Free cancellation until 2026-12-07")
            && a.contains("nothing charged now"),
        "{a}"
    );
}

// ---------------------------------------------------------------- restaurants

#[test]
fn a_michelin_table_takes_the_next_candidate_when_full_and_the_deposit_is_confirmed() {
    let s = spec("michelin-dining");
    let p = profile();
    let user = vec!["Book a 2-star kaiseki dinner in Tokyo on 5 December 2026 around 7pm, it's our anniversary.".to_string()];
    let w = World {
        user_texts: &user,
        profile: &p,
        today: today(),
        mail_ready: false,
        zh: false,
    };
    let found = json!({"city": "Tokyo", "count": 2, "restaurants": [
        {"restaurant_id": "R-TYO-1", "name": "Kaiseki Hana", "distinction": "2 Stars", "cuisine": "Kaiseki", "price_range": "¥30,000 - ¥45,000 per person (JPY)", "closed": "Sunday", "deposit_policy": "x", "booking_url": format!("{TT}/book/restaurant/R-TYO-1")},
        {"restaurant_id": "R-TYO-2", "name": "Ryotei Matsu", "distinction": "2 Stars", "cuisine": "Kaiseki", "price_range": "¥45,000 - ¥60,000 per person (JPY)", "closed": "Monday", "deposit_policy": "¥18,000 per person charged by card at booking; refunded if cancelled 72 h before", "booking_url": format!("{TT}/book/restaurant/R-TYO-2")},
        {"restaurant_id": "R-TYO-3", "name": "Sushi Ume", "distinction": "1 Star", "cuisine": "Sushi", "price_range": "¥20,000 per person (JPY)", "closed": "open daily", "booking_url": format!("{TT}/book/restaurant/R-TYO-3")}]});
    let form = "[e10] textbox \"Full name\"\n[e11] textbox \"Email\"\n[e12] textbox \"Phone\"\n[e13] textbox \"Dietary needs / occasion (optional)\"\n\
[e20] textbox \"Card number\"\n[e21] textbox \"Expiry (MM/YY)\"\n[e22] textbox \"CVC\"\n[e23] textbox \"Name on card\"";
    let (t, a) = run(
        loaded(s.skill, &user[0]),
        |t| next(s, t, &w),
        |name, args, t| {
            match name {
        "mcp_travel__michelin_search" => found.to_string(),
        "mcp_travel__restaurant_availability" if sget(args, "restaurant_id") == "R-TYO-1" =>
            json!({"available": false, "reason": "fully booked", "next_available": ["2026-12-06 (Sunday): 18:00"]}).to_string(),
        "mcp_travel__restaurant_availability" => json!({"restaurant": found["restaurants"][1], "available": true, "date": "2026-12-05", "party_size": 2,
            "slots": [{"time": "18:00", "booking_url": format!("{TT}/book/restaurant/R-TYO-2-20261205-1800-2")},
                      {"time": "19:30", "booking_url": format!("{TT}/book/restaurant/R-TYO-2-20261205-1930-2")},
                      {"time": "21:00", "booking_url": format!("{TT}/book/restaurant/R-TYO-2-20261205-2100-2")}],
            "deposit_due_now": "¥36,000", "seating_limit": "2 hours"}).to_string(),
        "vault_list" => VAULT.into(),
        "purchase_confirm" => "{\"status\": \"approved\"}".into(),
        "browser_navigate" => snap(&sget(args, "url"), form),
        "browser_fill_form" => "{\"filled\": []}".into(),
        "browser_fill_secret" => "{\"filled\": \"ok\"}".into(),
        "browser_request_takeover" => BACK.into(),
        "browser_snapshot" if handed_back_in(t) => snap("x", "Booking confirmed\nBooking reference: TRV-RZZ9Y8X"),
        "browser_snapshot" => snap("x", form),
        _ => "{\"shown\": 2}".into(),
    }
        },
    );
    let search = &args_of(&t, "mcp_travel__michelin_search")[0];
    assert_eq!(
        (search["distinction"].as_str(), search["cuisine"].as_str()),
        (Some("2 stars"), Some("kaiseki"))
    );
    let tried: Vec<String> = args_of(&t, "mcp_travel__restaurant_availability")
        .iter()
        .map(|a| sget(a, "restaurant_id"))
        .collect();
    assert_eq!(tried, ["R-TYO-1", "R-TYO-2"]);
    assert_eq!(args_of(&t, "present_choices")[0]["picked"], "Ryotei Matsu");
    let pc = &args_of(&t, "purchase_confirm")[0];
    assert_eq!(
        (pc["total"].as_f64(), pc["currency"].as_str()),
        (Some(36000.0), Some("JPY"))
    );
    assert_eq!(
        args_of(&t, "browser_navigate")[0]["url"],
        format!("{TT}/book/restaurant/R-TYO-2-20261205-1930-2")
    );
    let fill = &args_of(&t, "browser_fill_form")[0];
    assert_eq!(fill["form"], "traveltest-restaurant");
    assert_eq!(fill["fills"][3]["value"], "anniversary");
    assert_eq!(args_of(&t, "browser_fill_secret").len(), 4);
    assert!(
        a.contains("**TRV-RZZ9Y8X**")
            && a.contains("JPY 36,000.00")
            && a.contains("Seating limit: 2 hours"),
        "{a}"
    );
}

#[test]
fn a_restaurant_is_picked_by_rating_and_its_own_site_goes_to_browser_use() {
    let s = spec("restaurant-booking");
    let p = profile();
    let places = json!({"count": 3, "places": [
        {"place_id": "p1", "name": "Ramen Nagi", "rating": 4.1, "reviews": 900, "price": "¥1,000–2,000", "address": "Shinjuku", "website": "https://ramen-nagi.example/reserve"},
        {"place_id": "p2", "name": "Kyushu Jangara Ginza", "rating": 4.4, "reviews": 2349, "price": "¥1,000–2,000", "address": "Ginza", "website": "https://jangara.example/"},
        {"place_id": "p3", "name": "Afuri Ebisu", "rating": 4.5, "reviews": 120, "price": "¥1,000–2,000", "address": "Ebisu", "website": "https://afuri.example/book"}]});
    // the comparison
    let user = vec!["Find a good ramen place in Tokyo".to_string()];
    let w = World {
        user_texts: &user,
        profile: &p,
        today: today(),
        mail_ready: false,
        zh: false,
    };
    let (t, a) = run(
        loaded(s.skill, &user[0]),
        |t| next(s, t, &w),
        |name, _, _| match name {
            "places_search" => places.to_string(),
            _ => "{\"shown\": 2}".into(),
        },
    );
    assert_eq!(args_of(&t, "places_search")[0]["query"], "ramen restaurant");
    assert_eq!(args_of(&t, "present_choices")[0]["picked"], "Afuri Ebisu");
    assert!(
        a.contains("Afuri Ebisu") && !names(&t).iter().any(|n| n == "browser_navigate"),
        "{a}"
    );
    // the booking: the restaurant's own site, filled by the model through browser-use
    let user =
        vec!["Book a table at Kyushu Jangara Ginza in Tokyo tomorrow at 7:30 pm".to_string()];
    let w = World {
        user_texts: &user,
        profile: &p,
        today: today(),
        mail_ready: false,
        zh: false,
    };
    let (t, _) = run(
        loaded(s.skill, &user[0]),
        |t| next(s, t, &w),
        |name, args, _| match name {
            "places_search" => places.to_string(),
            "browser_navigate" => snap(&sget(args, "url"), "[e3] button \"Reserve\""),
            _ => "{\"shown\": 2}".into(),
        },
    );
    assert_eq!(
        args_of(&t, "browser_navigate")[0]["url"],
        "https://jangara.example/"
    );
    assert_eq!(names(&t).last().map(String::as_str), Some("load_skill"));
    assert!(!names(&t)
        .iter()
        .any(|n| n == "browser_fill_form" || n == "browser_fill_secret"));
}

// ---------------------------------------------------------------- shopping

const RESULTS: &str = "Results for \"iPhone 17 Pro Max case\"\nSponsored\n\
Titan Pro Shockproof Case for iPhone 17 Pro (6.3-inch), Military Grade\n4.9 out of 5 stars · 860 ratings\nS$22.90\n\
Lumi Crystal Clear Case for iPhone 17 Pro Max, Anti-Scratch\n4.9 out of 5 stars · 38 ratings\nS$17.90\n\
Aurora Glitter Case for iPhone 17 Pro Max, MagSafe Compatible\n4.7 out of 5 stars · 1,203 ratings\nS$21.90\n\
Slim Leather Case for iPhone 17 Pro Max, MagSafe Compatible\n4.6 out of 5 stars · 512 ratings\nS$24.90\n\
Carbon Fibre Case for iPhone 17 Pro Max, Ultra Thin 0.6 mm\n4.5 out of 5 stars · 268 ratings\nS$18.90\n\
MagSafe Silicone Case for iPhone 17 Pro Max\n4.4 out of 5 stars · 2,048 ratings\nS$19.90\n\
Kickstand Case for iPhone 17 Pro Max, MagSafe Ring Stand\n4.8 out of 5 stars · 379 ratings\nS$33.90\n\
Camera Lens Protector for iPhone 17 Pro Max, 2-Pack\n4.8 out of 5 stars · 2,770 ratings\nS$9.90\n";

const SLIM: &str = "Slim Leather Case for iPhone 17 Pro Max, MagSafe Compatible";

fn shop_answer(name: &str, args: &Value, t: &[Value]) -> String {
    let added = names(t).iter().any(|n| n == "browser_fill_form")
        && args_of(t, "browser_fill_form")
            .iter()
            .any(|a| a["form"] == "kaya-item");
    match name {
        "browser_read" => json!({"pages": [{"url": format!("{SHOP}/kaya_search.html")}]}).to_string() + "\n" + RESULTS,
        "browser_navigate" if sget(args, "url").ends_with("kaya_cart.html") => {
            snap(&sget(args, "url"), if added { SLIM } else { "Your Kaya Mobile cart is empty." })
        }
        "browser_navigate" if sget(args, "url").ends_with("kaya_checkout.html") => snap(
            &sget(args, "url"),
            "[e10] textbox \"Full name\"\n[e11] textbox \"Mobile number\"\n[e12] textbox \"Address\"\n[e13] textbox \"Card number\"\n[e14] textbox \"Expiry (MM/YY)\"\n[e15] textbox \"Security code (CVC)\"\n[e16] button \"Place your order\"",
        ),
        "browser_navigate" => snap(&sget(args, "url"), "[e2] link \"Cart (0)\"\n[e40] link \"Slim Leather Case for iPhone 17 Pro Max, MagSafe Compatible\""),
        "browser_fill_form" => "{\"url\": \"x\", \"filled\": [{\"ok\": true}]}".into(),
        "browser_snapshot" if handed_back_in(t) => snap(&format!("{SHOP}/kaya_done.html"), "Thank you, your order has been placed.\nOrder number KM-50731"),
        "browser_snapshot" if names(t).iter().any(|n| n == "purchase_confirm") => snap(
            &format!("{SHOP}/kaya_checkout.html"),
            "[e10] textbox \"Full name\"\n[e13] textbox \"Card number\"\n[e14] textbox \"Expiry (MM/YY)\"\n[e15] textbox \"Security code (CVC)\"",
        ),
        "browser_snapshot" => snap(
            &format!("{SHOP}/kaya_item.html?id=slim-leather"),
            &format!("[e2] link \"Cart (1)\"\n[e9] status \"Added to Cart: {SLIM} (Saddle Brown)\""),
        ),
        "vault_list" => VAULT.into(),
        "purchase_confirm" => "{\"status\": \"approved\"}".into(),
        "browser_fill_secret" => "{\"filled\": \"ok\"}".into(),
        "browser_request_takeover" => BACK.into(),
        _ => "{\"shown\": 3, \"verified\": true}".into(),
    }
}

#[test]
fn the_shop_ranking_is_read_from_the_users_words() {
    let r = shop_ranking("show me the three best-rated cases under S$25 that have at least 200 ratings, for my iPhone 17 Pro Max case");
    assert_eq!(
        (r.by.as_str(), r.max_price, r.min_ratings, r.count),
        ("rating", Some(25.0), Some(200), Some(3))
    );
    assert_eq!(r.title_has, ["case", "iPhone 17 Pro Max"]);
    let got: Vec<String> = r
        .apply(&crate::listings::parse(RESULTS))
        .into_iter()
        .map(|l| l.title)
        .collect();
    assert_eq!(got.len(), 3);
    assert!(
        got[0].starts_with("Aurora Glitter")
            && got[1].starts_with("Slim Leather")
            && got[2].starts_with("Carbon Fibre"),
        "{got:?}"
    );
    assert_eq!(shop_ranking("the cheapest charger").by, "price");
}

#[test]
fn shopping_on_a_known_shop_choices_then_cart_then_checkout_by_fixed_ids() {
    let s = spec("online-shopping");
    let p = profile();
    let ask = format!("I need a case for my iPhone 17 Pro Max. Here are Kaya Mobile's search results: {SHOP}/kaya_search.html — show me the three \
best-rated cases under S$25 that have at least 200 ratings, as options I can choose from. I'll pick one for my cart.");
    let mut user = vec![ask.clone()];
    let t = loaded(s.skill, &ask);
    assert!(active(&t).is_some());
    // 1. the options: a choice card the user picks from (no pick by code), and the turn ends
    let w = World {
        user_texts: &user,
        profile: &p,
        today: today(),
        mail_ready: false,
        zh: false,
    };
    let (mut t, a) = run(t, |t| next(s, t, &w), shop_answer);
    let pc = &args_of(&t, "present_choices")[0];
    assert!(pc.get("picked").is_none());
    let labels: Vec<String> = pc["options"]
        .as_array()
        .unwrap()
        .iter()
        .map(|o| sget(o, "label"))
        .collect();
    assert_eq!(labels.len(), 3);
    assert!(labels.contains(&SLIM.to_string()));
    assert!(a.contains("press Choose"), "{a}");
    assert!(!names(&t).iter().any(|n| n == "browser_navigate"));
    // 2. the pick: into the cart (cart checked first, the product by its title, Add to Cart by #add), verified
    user.push(format!("I choose: {SLIM}"));
    t.push(json!({"role": "user", "content": user[1]}));
    let w = World {
        user_texts: &user,
        profile: &p,
        today: today(),
        mail_ready: false,
        zh: false,
    };
    let (mut t, a) = run(t, |t| next(s, t, &w), shop_answer);
    let fills = args_of(&t, "browser_fill_form");
    assert_eq!(fills.len(), 2, "{a} {:?}", names(&t));
    assert_eq!(fills[0]["form"], "kaya-results");
    assert_eq!(fills[0]["fills"][0]["text"], SLIM);
    assert_eq!(fills[1]["form"], "kaya-item");
    assert_eq!(
        fills[1]["fills"].as_array().unwrap().last().unwrap()["sel"],
        "#add"
    );
    assert!(
        a.contains("in your Kaya Mobile cart ✓")
            && a.contains("Saddle Brown")
            && a.contains("S$24.90"),
        "{a}"
    );
    assert!(!names(&t)
        .iter()
        .any(|n| n == "purchase_confirm" || n == "browser_request_takeover"));
    // 3. buy: no second Add to Cart; purchase_confirm, the checkout by fixed ids, the card, the hand-over, the order number
    user.push("Please buy it now — home delivery, and pay with my Visa from the Vault. I'll press Place order myself.".into());
    t.push(json!({"role": "user", "content": user[2]}));
    let w = World {
        user_texts: &user,
        profile: &p,
        today: today(),
        mail_ready: false,
        zh: false,
    };
    let (t, a) = run(t, |t| next(s, t, &w), shop_answer);
    assert_eq!(
        args_of(&t, "browser_fill_form")
            .iter()
            .filter(|a| a["form"] == "kaya-item")
            .count(),
        1
    );
    let pc = &args_of(&t, "purchase_confirm")[0];
    assert_eq!(
        (
            pc["total"].as_f64(),
            pc["currency"].as_str(),
            pc["card_item_id"].as_str()
        ),
        (Some(24.9), Some("SGD"), Some("card1"))
    );
    assert!(sget(pc, "delivery").contains("10 Anson Road"));
    let checkout = args_of(&t, "browser_fill_form")
        .into_iter()
        .find(|a| a["form"] == "kaya-checkout")
        .unwrap();
    let sels: Vec<String> = checkout["fills"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| sget(f, "sel"))
        .collect();
    assert_eq!(sels, ["#name", "#phone", "#address"]);
    let refs: Vec<String> = args_of(&t, "browser_fill_secret")
        .iter()
        .map(|a| sget(a, "ref"))
        .collect();
    assert_eq!(refs, ["e13", "e14", "e15"], "{:?}", names(&t));
    assert!(sget(&args_of(&t, "browser_request_takeover")[0], "reason")
        .contains("press Place order yourself"));
    assert!(
        a.contains("**KM-50731**") && a.contains(SLIM) && a.contains("SGD 24.90"),
        "{a}"
    );
}

#[test]
fn a_product_page_with_its_checkout_is_bought_by_fixed_ids() {
    let s = spec("online-shopping");
    let p = profile();
    let user = vec![format!("Please order the Kiprun Run 500 running socks for me from Stride Sports: {SHOP}/stride_socks.html — home delivery, pay with my Visa from the Vault.")];
    let w = World {
        user_texts: &user,
        profile: &p,
        today: today(),
        mail_ready: false,
        zh: false,
    };
    let page = "[e1] heading \"Kiprun Run 500 running socks (pack of 1, size 39–42)\"\nS$4.90\n[e10] textbox \"Full name\"\n[e11] textbox \"Mobile number\"\n[e12] textbox \"Delivery address\"\n[e13] textbox \"Card number\"\n[e14] textbox \"Expiry\"\n[e15] textbox \"CVC\"\n[e16] button \"Place order\"";
    let (t, a) = run(
        loaded(s.skill, &user[0]),
        |t| next(s, t, &w),
        |name, args, t| match name {
            "browser_navigate" => snap(&sget(args, "url"), page),
            "vault_list" => VAULT.into(),
            "purchase_confirm" => "{\"status\": \"approved\"}".into(),
            "browser_fill_form" => "{\"filled\": []}".into(),
            "browser_fill_secret" => "{\"filled\": \"ok\"}".into(),
            "browser_request_takeover" => BACK.into(),
            "browser_snapshot" if handed_back_in(t) => snap(
                "x",
                "Thank you for your order! Order SR-48213. Total paid: S$4.90",
            ),
            "browser_snapshot" => snap("x", page),
            _ => "{}".into(),
        },
    );
    assert_eq!(
        names(&t)[1..]
            .iter()
            .filter(|n| *n == "browser_navigate")
            .count(),
        1
    );
    let pc = &args_of(&t, "purchase_confirm")[0];
    assert_eq!(
        (pc["total"].as_f64(), pc["items"][0]["name"].as_str()),
        (
            Some(4.9),
            Some("Kiprun Run 500 running socks (pack of 1, size 39–42)")
        )
    );
    assert_eq!(
        args_of(&t, "browser_fill_form")[0]["form"],
        "stride-checkout"
    );
    assert_eq!(args_of(&t, "browser_fill_secret").len(), 3);
    assert!(a.contains("**SR-48213**") && a.contains("SGD 4.90"), "{a}");
}

#[test]
fn an_unknown_shop_is_opened_and_passed_to_browser_use() {
    let s = spec("online-shopping");
    let p = profile();
    let user = vec!["Buy this for me: https://www.lazada.sg/products/socks-i123.html".to_string()];
    let w = World {
        user_texts: &user,
        profile: &p,
        today: today(),
        mail_ready: false,
        zh: false,
    };
    let (t, _) = run(
        loaded(s.skill, &user[0]),
        |t| next(s, t, &w),
        |name, args, _| match name {
            "browser_navigate" => snap(&sget(args, "url"), "[e5] button \"Add to Cart\""),
            _ => "{}".into(),
        },
    );
    assert_eq!(names(&t)[1..], ["browser_navigate", "load_skill"]);
    assert!(active(&t).is_none());
}

#[test]
fn no_question_after_the_booking_page_opens_even_with_the_facts_gone() {
    // the user's words are gone after the page opened (compression): the run fills what it has and hands over
    let s = spec("hotel-booking");
    let p = json!({});
    let user = vec!["Book a hotel in Tokyo from 10 December to 12 December 2026.".to_string()];
    let w = World {
        user_texts: &user,
        profile: &p,
        today: today(),
        mail_ready: false,
        zh: false,
    };
    let res = json!({"hotels": [{"name": "Hotel Ginza Grand", "price_per_night": 210, "booking_url": format!("{TT}/book/hotel/H-TYO-1-20261210-20261212-1-R2")}]});
    let (t, a) = run(
        loaded(s.skill, &user[0]),
        |t| next(s, t, &w),
        |name, args, t| match name {
            "hotels_search" => res.to_string(),
            "browser_navigate" => snap(&sget(args, "url"), "[e10] textbox \"Full name\""),
            "browser_fill_form" => "{\"filled\": []}".into(),
            "browser_request_takeover" => BACK.into(),
            "browser_snapshot" if handed_back_in(t) => snap("x", "Please complete: Full name"),
            _ => "{\"shown\": 1}".into(),
        },
    );
    // an empty profile: the blanks are named in the hand-over, never asked
    let reason = sget(&args_of(&t, "browser_request_takeover")[0], "reason");
    assert!(
        reason.contains("fill in full name, email, phone"),
        "{reason}"
    );
    assert!(questions_after_navigate(&t).is_empty());
    assert!(a.contains("not confirmed"), "{a}");
}
