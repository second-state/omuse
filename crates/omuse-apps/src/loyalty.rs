//! Airline alliances and hotel groups: a preference such as "Star Alliance" or "IHG" stands for every member airline
//! or brand, so `flights_search airlines=["Star Alliance"]` flags a United or ANA offer and
//! `hotels_search brands=["IHG"]` flags a Holiday Inn. The Profile card's dropdowns (`web/app.js`) offer the same
//! names.

/// (alliance, Google Flights' `include_airlines` value, member airlines as Google / Kiwi name them, the members' IATA
/// codes — what Kiwi.com's `select_airlines` takes: it knows no alliances, so an alliance is searched as its members)
pub const ALLIANCES: &[(&str, &str, &[&str], &[&str])] = &[
    (
        "Star Alliance",
        "STAR_ALLIANCE",
        &[
            "Singapore Airlines",
            "ANA",
            "All Nippon Airways",
            "United",
            "Lufthansa",
            "SWISS",
            "Austrian",
            "Air Canada",
            "Thai Airways",
            "EVA Air",
            "Asiana",
            "Air China",
            "Shenzhen Airlines",
            "Air India",
            "Turkish Airlines",
            "Air New Zealand",
            "Avianca",
            "Copa",
            "Ethiopian",
            "EgyptAir",
            "South African Airways",
            "LOT",
            "TAP",
            "Brussels Airlines",
            "Aegean",
            "Croatia Airlines",
        ],
        &[
            "A3", "AC", "CA", "AI", "NZ", "NH", "OZ", "OS", "AV", "SN", "CM", "OU", "MS", "ET",
            "BR", "LO", "LH", "ZH", "SQ", "SA", "LX", "TP", "TG", "TK", "UA",
        ],
    ),
    (
        "oneworld",
        "ONEWORLD",
        &[
            "Cathay Pacific",
            "Japan Airlines",
            "JAL",
            "Qantas",
            "British Airways",
            "American Airlines",
            "American",
            "Qatar Airways",
            "Finnair",
            "Iberia",
            "Malaysia Airlines",
            "Royal Jordanian",
            "SriLankan",
            "Alaska Airlines",
            "Fiji Airways",
            "Oman Air",
            "Royal Air Maroc",
        ],
        &[
            "AS", "AA", "BA", "CX", "AY", "IB", "JL", "MH", "QF", "QR", "AT", "RJ", "UL", "FJ",
            "WY",
        ],
    ),
    (
        "SkyTeam",
        "SKYTEAM",
        &[
            "Korean Air",
            "China Eastern",
            "China Airlines",
            "Vietnam Airlines",
            "Garuda",
            "Delta",
            "Air France",
            "KLM",
            "Virgin Atlantic",
            "Aeromexico",
            "Saudia",
            "Kenya Airways",
            "Middle East Airlines",
            "Xiamen",
            "SAS",
            "Aerolineas Argentinas",
            "TAROM",
        ],
        &[
            "AR", "AM", "AF", "CI", "MU", "DL", "GA", "KQ", "KL", "KE", "ME", "SV", "RO", "VN",
            "VS", "MF", "SK",
        ],
    ),
];

/// (hotel group / loyalty programme, its brands as Google Hotels names them)
pub const HOTEL_GROUPS: &[(&str, &[&str])] = &[
    (
        "Marriott Bonvoy",
        &[
            "Marriott",
            "JW Marriott",
            "Westin",
            "Sheraton",
            "W Hotels",
            "St. Regis",
            "Ritz-Carlton",
            "Le Méridien",
            "Le Meridien",
            "Renaissance",
            "Courtyard",
            "Four Points",
            "Aloft",
            "Moxy",
            "Fairfield",
            "Residence Inn",
            "Autograph Collection",
            "Luxury Collection",
            "EDITION",
            "Delta Hotels",
            "AC Hotels",
            "Element",
            "Tribute Portfolio",
        ],
    ),
    (
        "Hilton Honors",
        &[
            "Hilton",
            "Conrad",
            "Waldorf Astoria",
            "DoubleTree",
            "Hampton",
            "Hilton Garden Inn",
            "Embassy Suites",
            "Canopy",
            "Curio",
            "Tapestry",
            "LXR",
            "Homewood Suites",
            "Home2",
            "Tru by Hilton",
            "Motto",
            "Signia",
        ],
    ),
    (
        "World of Hyatt",
        &[
            "Hyatt",
            "Park Hyatt",
            "Grand Hyatt",
            "Hyatt Regency",
            "Andaz",
            "Hyatt Place",
            "Hyatt House",
            "Hyatt Centric",
            "Thompson",
            "Alila",
            "Miraval",
            "Caption by Hyatt",
            "Destination by Hyatt",
        ],
    ),
    (
        "IHG One Rewards",
        &[
            "InterContinental",
            "Holiday Inn",
            "Holiday Inn Express",
            "Crowne Plaza",
            "Kimpton",
            "Hotel Indigo",
            "Regent",
            "Six Senses",
            "voco",
            "EVEN Hotels",
            "Staybridge Suites",
            "Candlewood Suites",
            "avid",
            "Vignette Collection",
            "Iberostar",
        ],
    ),
    (
        "Accor Live Limitless",
        &[
            "Sofitel",
            "Pullman",
            "Novotel",
            "Mercure",
            "ibis",
            "MGallery",
            "Fairmont",
            "Raffles",
            "Swissôtel",
            "Swissotel",
            "Mövenpick",
            "Movenpick",
            "Grand Mercure",
            "Rixos",
            "SLS",
            "Mondrian",
            "25hours",
            "Tribe",
            "Adagio",
            "Banyan Tree",
        ],
    ),
    (
        "Wyndham Rewards",
        &[
            "Wyndham",
            "Ramada",
            "Days Inn",
            "Super 8",
            "La Quinta",
            "Microtel",
            "Howard Johnson",
            "Travelodge",
            "Wingate",
            "TRYP",
        ],
    ),
    (
        "Choice Privileges",
        &[
            "Comfort Inn",
            "Comfort Suites",
            "Quality Inn",
            "Clarion",
            "Sleep Inn",
            "Cambria",
            "Ascend",
        ],
    ),
    (
        "Radisson Rewards",
        &[
            "Radisson",
            "Radisson Blu",
            "Radisson RED",
            "Park Inn",
            "Park Plaza",
            "Country Inn",
        ],
    ),
    (
        "Shangri-La Circle",
        &["Shangri-La", "Kerry Hotel", "JEN", "Traders"],
    ),
    (
        "Minor Hotels",
        &[
            "Anantara",
            "Avani",
            "Tivoli",
            "NH Hotels",
            "NH Collection",
            "Oaks",
        ],
    ),
    (
        "Okura Nikko",
        &[
            "Hotel Okura",
            "Okura",
            "Hotel Nikko",
            "Nikko",
            "Hotel JAL City",
        ],
    ),
    (
        "Jin Jiang",
        &[
            "Jin Jiang",
            "Jinjiang",
            "Vienna Hotel",
            "7 Days Inn",
            "Metropolo",
        ],
    ),
    (
        "Huazhu",
        &[
            "Hanting",
            "Ji Hotel",
            "Orange Hotel",
            "Crystal Orange",
            "Joya",
            "Intercity Hotel",
        ],
    ),
    ("Langham", &["Langham", "Cordis"]),
    ("Dusit", &["Dusit Thani", "dusitD2", "Dusit Princess"]),
];

/// Google Hotels' brand id of a hotel group (its `brands` filter: the group's id takes every member brand, e.g. 46 →
/// Sheraton, Westin, Aloft, AC Hotels, Moxy, EDITION …), as Google listed them on 2026-10-29. A group not here is looked
/// up by name in a search's own `brands` list (`serpapi::Client::hotels`).
pub const GOOGLE_HOTEL_GROUP_IDS: &[(&str, u32)] = &[
    ("Marriott Bonvoy", 46),
    ("Hilton Honors", 28),
    ("World of Hyatt", 37),
    ("IHG One Rewards", 17),
    ("Accor Live Limitless", 33),
    ("Wyndham Rewards", 53),
    ("Shangri-La Circle", 315),
];

/// The Google Hotels brand id of a preference that names a hotel group in `GOOGLE_HOTEL_GROUP_IDS`.
pub fn google_group_id(pref: &str) -> Option<u32> {
    let g = hotel_group(pref)?;
    GOOGLE_HOTEL_GROUP_IDS
        .iter()
        .find(|(n, _)| *n == g.0)
        .map(|(_, id)| *id)
}

/// The ids in a Google Hotels `brands` list (`[{id, name, children: [{id, name}]}]`) that a preference stands for: a
/// group by its programme's name or the word it goes by ("Hyatt" for World of Hyatt, "IHG" for "IHG Hotels &
/// Resorts"), a brand by a child whose name starts with it ("Westin" → "Westin Hotels & Resorts"). Empty: none.
pub fn google_brand_ids(pref: &str, list: &[serde_json::Value]) -> Vec<u64> {
    let p = pref.trim().to_lowercase();
    let word = hotel_group(pref).map(|g| group_word(g.0).to_lowercase());
    // exact names first ("Holiday Inn" is not "Holiday Inn Express"); else the names that start with it
    let exact = |n: &str, w: &str| n == w;
    let prefix = |n: &str, w: &str| n.starts_with(&format!("{w} "));
    let id = |v: &serde_json::Value| v.get("id").and_then(serde_json::Value::as_u64);
    let name = |v: &serde_json::Value| {
        v.get("name")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("")
            .trim()
            .to_lowercase()
    };
    for rule in [exact, prefix] {
        let named =
            |n: &str| !p.is_empty() && (rule(n, &p) || word.as_ref().is_some_and(|w| rule(n, w)));
        let mut out = Vec::new();
        for parent in list {
            if named(&name(parent)) {
                out.extend(id(parent));
                continue;
            }
            for child in parent
                .get("children")
                .and_then(serde_json::Value::as_array)
                .into_iter()
                .flatten()
            {
                if named(&name(child)) {
                    out.extend(id(child));
                }
            }
        }
        if !out.is_empty() {
            return out;
        }
    }
    Vec::new()
}

/// IATA airline code → the airline's own website, where a flight from Kiwi.com data is booked (Kiwi gives no link).
pub const AIRLINE_SITES: &[(&str, &str)] = &[
    ("SQ", "singaporeair.com"),
    ("TR", "flyscoot.com"),
    ("CX", "cathaypacific.com"),
    ("UO", "hkexpress.com"),
    ("HX", "hongkongairlines.com"),
    ("NH", "ana.co.jp"),
    ("JL", "jal.co.jp"),
    ("MM", "flypeach.com"),
    ("GK", "jetstar.com"),
    ("JQ", "jetstar.com"),
    ("3K", "jetstar.com"),
    ("KE", "koreanair.com"),
    ("OZ", "flyasiana.com"),
    ("7C", "jejuair.net"),
    ("TW", "twayair.com"),
    ("LJ", "jinair.com"),
    ("CI", "china-airlines.com"),
    ("BR", "evaair.com"),
    ("JX", "starlux-airlines.com"),
    ("IT", "tigerairtw.com"),
    ("TG", "thaiairways.com"),
    ("AK", "airasia.com"),
    ("FD", "airasia.com"),
    ("D7", "airasia.com"),
    ("MH", "malaysiaairlines.com"),
    ("GA", "garuda-indonesia.com"),
    ("VN", "vietnamairlines.com"),
    ("VJ", "vietjetair.com"),
    ("PR", "philippineairlines.com"),
    ("5J", "cebupacificair.com"),
    ("CA", "airchina.com.cn"),
    ("MU", "ceair.com"),
    ("CZ", "csair.com"),
    ("HU", "hainanairlines.com"),
    ("QF", "qantas.com"),
    ("NZ", "airnewzealand.com"),
    ("EK", "emirates.com"),
    ("QR", "qatarairways.com"),
    ("EY", "etihad.com"),
    ("TK", "turkishairlines.com"),
    ("LH", "lufthansa.com"),
    ("LX", "swiss.com"),
    ("OS", "austrian.com"),
    ("KL", "klm.com"),
    ("AF", "airfrance.com"),
    ("BA", "britishairways.com"),
    ("VS", "virginatlantic.com"),
    ("IB", "iberia.com"),
    ("AY", "finnair.com"),
    ("SK", "flysas.com"),
    ("UA", "united.com"),
    ("DL", "delta.com"),
    ("AA", "aa.com"),
    ("AC", "aircanada.com"),
    ("AS", "alaskaair.com"),
    ("B6", "jetblue.com"),
    ("WN", "southwest.com"),
    ("AI", "airindia.com"),
    ("6E", "goindigo.in"),
    ("UL", "srilankan.com"),
];

/// The airline's own website for a flight number ("SQ638", "SQ 638") or an airline code; `None` when unknown.
pub fn airline_site(flight_or_code: &str) -> Option<&'static str> {
    let s = flight_or_code.trim().to_ascii_uppercase();
    let code: String = s.chars().take(2).collect();
    AIRLINE_SITES
        .iter()
        .find(|(c, _)| *c == code)
        .map(|(_, d)| *d)
}

/// Airlines by the names users and the Profile's dropdown give them (`web/app.js` AIRLINES), with their IATA codes; a
/// group sold under one name (AirAsia, Jetstar) has several.
pub const AIRLINE_CODES: &[(&[&str], &[&str])] = &[
    (&["Singapore Airlines", "Singapore Air"], &["SQ"]),
    (&["ANA", "All Nippon Airways"], &["NH"]),
    (&["Japan Airlines", "JAL"], &["JL"]),
    (&["Cathay Pacific", "Cathay"], &["CX"]),
    (&["Korean Air"], &["KE"]),
    (&["Asiana Airlines", "Asiana"], &["OZ"]),
    (&["EVA Air"], &["BR"]),
    (&["China Airlines"], &["CI"]),
    (&["STARLUX", "Starlux Airlines"], &["JX"]),
    (&["Thai Airways", "Thai"], &["TG"]),
    (&["Malaysia Airlines"], &["MH"]),
    (&["Garuda Indonesia", "Garuda"], &["GA"]),
    (&["Vietnam Airlines"], &["VN"]),
    (&["Philippine Airlines"], &["PR"]),
    (&["Air China"], &["CA"]),
    (&["China Eastern", "China Eastern Airlines"], &["MU"]),
    (&["China Southern", "China Southern Airlines"], &["CZ"]),
    (&["Hainan Airlines"], &["HU"]),
    (&["Shenzhen Airlines"], &["ZH"]),
    (&["Xiamen", "Xiamen Airlines"], &["MF"]),
    (&["Qantas"], &["QF"]),
    (&["Air New Zealand"], &["NZ"]),
    (&["Emirates"], &["EK"]),
    (&["Qatar Airways", "Qatar"], &["QR"]),
    (&["Etihad Airways", "Etihad"], &["EY"]),
    (&["Turkish Airlines"], &["TK"]),
    (&["Lufthansa"], &["LH"]),
    (&["SWISS", "Swiss International Air Lines"], &["LX"]),
    (&["Austrian", "Austrian Airlines"], &["OS"]),
    (&["KLM"], &["KL"]),
    (&["Air France"], &["AF"]),
    (&["British Airways"], &["BA"]),
    (&["Virgin Atlantic"], &["VS"]),
    (&["Iberia"], &["IB"]),
    (&["Finnair"], &["AY"]),
    (&["SAS", "Scandinavian Airlines"], &["SK"]),
    (&["United", "United Airlines"], &["UA"]),
    (&["Delta", "Delta Air Lines"], &["DL"]),
    (&["American Airlines", "American"], &["AA"]),
    (&["Air Canada"], &["AC"]),
    (&["Alaska Airlines"], &["AS"]),
    (&["JetBlue"], &["B6"]),
    (&["Southwest", "Southwest Airlines"], &["WN"]),
    (&["Scoot"], &["TR"]),
    (&["AirAsia"], &["AK", "D7", "FD", "QZ", "Z2"]),
    (&["Jetstar"], &["JQ", "GK", "3K"]),
    (&["Cebu Pacific"], &["5J"]),
    (&["Peach", "Peach Aviation"], &["MM"]),
    (&["Jeju Air"], &["7C"]),
    (&["IndiGo"], &["6E"]),
    (&["Air India"], &["AI"]),
    (&["Air India Express"], &["IX"]),
    (&["Hong Kong Airlines"], &["HX"]),
    (&["HK Express"], &["UO"]),
    (&["ZIPAIR", "Zipair Tokyo"], &["ZG"]),
    (&["VietJet", "VietJet Air"], &["VJ"]),
    (&["Batik Air"], &["ID", "OD"]),
    (&["Royal Brunei", "Royal Brunei Airlines"], &["BI"]),
    (&["Air Busan"], &["BX"]),
    (&["T'way", "T'way Air"], &["TW"]),
    (&["Bangkok Airways"], &["PG"]),
    (&["Lion Air"], &["JT"]),
    (&["Ethiopian", "Ethiopian Airlines"], &["ET"]),
    (&["EgyptAir"], &["MS"]),
    (&["South African Airways"], &["SA"]),
    (&["LOT", "LOT Polish Airlines"], &["LO"]),
    (&["TAP", "TAP Air Portugal"], &["TP"]),
    (&["Brussels Airlines"], &["SN"]),
    (&["Aegean", "Aegean Airlines"], &["A3"]),
    (&["Croatia Airlines"], &["OU"]),
    (&["Avianca"], &["AV"]),
    (&["Copa", "Copa Airlines"], &["CM"]),
    (&["Royal Jordanian"], &["RJ"]),
    (&["SriLankan", "SriLankan Airlines"], &["UL"]),
    (&["Fiji Airways"], &["FJ"]),
    (&["Oman Air"], &["WY"]),
    (&["Royal Air Maroc"], &["AT"]),
    (&["Aeromexico"], &["AM"]),
    (&["Saudia"], &["SV"]),
    (&["Kenya Airways"], &["KQ"]),
    (&["Middle East Airlines", "MEA"], &["ME"]),
    (&["Aerolineas Argentinas"], &["AR"]),
    (&["TAROM"], &["RO"]),
    (&["ITA Airways"], &["AZ"]),
];

/// The IATA codes a list of airline preferences stands for, in order and without repeats: a 2-character code as
/// given, an alliance as all its members, an airline by name from `AIRLINE_CODES`. `Err` names the preferences it
/// cannot place, so a search is never narrowed to part of what the user prefers.
pub fn airline_codes(prefs: &[String]) -> Result<Vec<String>, Vec<String>> {
    let (mut codes, mut unknown): (Vec<String>, Vec<String>) = (Vec::new(), Vec::new());
    let mut add = |c: &str| {
        let c = c.to_ascii_uppercase();
        if !codes.contains(&c) {
            codes.push(c);
        }
    };
    for p in prefs {
        let p = p.trim();
        if p.len() == 2 && p.chars().all(|c| c.is_ascii_alphanumeric()) {
            add(p);
        } else if let Some(a) = alliance(p) {
            a.3.iter().for_each(|c| add(c));
        } else if let Some((_, cs)) = AIRLINE_CODES
            .iter()
            .find(|(names, _)| names.iter().any(|n| same(n, p)))
        {
            cs.iter().for_each(|c| add(c));
        } else {
            unknown.push(p.to_string());
        }
    }
    if unknown.is_empty() {
        Ok(codes)
    } else {
        Err(unknown)
    }
}

fn same(a: &str, b: &str) -> bool {
    a.trim().eq_ignore_ascii_case(b.trim())
}

/// The alliance a preference names (any of "Star Alliance", "oneworld", "SkyTeam", case-insensitive).
pub fn alliance(
    name: &str,
) -> Option<&'static (
    &'static str,
    &'static str,
    &'static [&'static str],
    &'static [&'static str],
)> {
    ALLIANCES.iter().find(|a| same(a.0, name))
}

/// Programme words that do not name the group ("World of Hyatt" is the Hyatt group).
const FILLER: [&str; 10] = [
    "of",
    "world",
    "live",
    "limitless",
    "one",
    "rewards",
    "honors",
    "privileges",
    "circle",
    "bonvoy",
];

/// The word a group goes by: "Marriott" for Marriott Bonvoy, "Hyatt" for World of Hyatt, "IHG" for IHG One Rewards.
pub fn group_word(group: &str) -> &str {
    group
        .split_whitespace()
        .find(|w| !FILLER.contains(&w.to_lowercase().as_str()))
        .unwrap_or(group)
}

/// The hotel group a preference names: the programme's full name ("IHG One Rewards") or the word it goes by ("IHG",
/// "Marriott", "Hilton", "Hyatt", "Accor"…); a brand's own name ("Holiday Inn") is not a group (`None`).
pub fn hotel_group(name: &str) -> Option<&'static (&'static str, &'static [&'static str])> {
    let n = name.trim();
    HOTEL_GROUPS
        .iter()
        .find(|g| same(g.0, n) || same(group_word(g.0), n))
}

/// Every name a list of preferences stands for, lower-case: the preferences themselves plus the members of any
/// alliance (`kind` "airline") or hotel group (`kind` "hotel") among them.
pub fn expand(prefs: &[String], kind: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut push = |s: &str| {
        let l = s.trim().to_lowercase();
        if !l.is_empty() && !out.contains(&l) {
            out.push(l);
        }
    };
    for p in prefs {
        push(p);
        if kind == "airline" {
            if let Some(a) = alliance(p) {
                a.2.iter().for_each(|m| push(m));
            }
        } else if let Some(g) = hotel_group(p) {
            g.1.iter().for_each(|m| push(m));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn airline_sites_from_flight_numbers() {
        assert_eq!(airline_site("SQ638"), Some("singaporeair.com"));
        assert_eq!(airline_site("cx 710"), Some("cathaypacific.com"));
        assert_eq!(airline_site("3K 521"), Some("jetstar.com"));
        assert_eq!(airline_site("ZZ1"), None);
        assert_eq!(airline_site(""), None);
    }

    #[test]
    fn hotel_groups_and_brands_find_their_google_ids() {
        assert_eq!(google_group_id("Marriott"), Some(46));
        assert_eq!(google_group_id("IHG One Rewards"), Some(17));
        assert_eq!(google_group_id("Hyatt"), Some(37));
        assert_eq!(google_group_id("Westin"), None);
        assert_eq!(google_group_id("Okura Nikko"), None);
        // as Google listed them for Singapore on 2026-10-29
        let list: Vec<serde_json::Value> = serde_json::from_str(r#"[
            {"id": 17, "name": "IHG Hotels & Resorts", "children": [{"id": 1, "name": "Crowne Plaza"}, {"id": 2, "name": "Holiday Inn"},
                                                                   {"id": 3, "name": "Holiday Inn Express"}]},
            {"id": 46, "name": "Marriott Bonvoy", "children": [{"id": 70, "name": "Westin Hotels & Resorts"}, {"id": 71, "name": "Sheraton Hotels and Resorts"}]},
            {"id": 400, "name": "Okura Nikko Hotels", "children": [{"id": 401, "name": "Hotel Okura"}]},
            {"id": 289, "name": "Four Seasons", "children": []}]"#).unwrap();
        assert_eq!(google_brand_ids("IHG", &list), [17]);
        assert_eq!(google_brand_ids("Marriott Bonvoy", &list), [46]);
        assert_eq!(google_brand_ids("Westin", &list), [70]);
        assert_eq!(google_brand_ids("Holiday Inn", &list), [2]); // not Holiday Inn Express
        assert_eq!(google_brand_ids("Okura Nikko", &list), [400]);
        assert_eq!(google_brand_ids("Four Seasons", &list), [289]);
        assert!(google_brand_ids("Aman", &list).is_empty());
        assert!(google_brand_ids("", &list).is_empty());
    }

    #[test]
    fn airline_preferences_become_iata_codes() {
        let c = |v: &[&str]| airline_codes(&v.iter().map(|s| s.to_string()).collect::<Vec<_>>());
        let star = c(&["Star Alliance"]).unwrap();
        assert_eq!(star.len(), 25);
        assert!(["SQ", "NH", "UA", "LH", "TK", "TG"]
            .iter()
            .all(|x| star.contains(&x.to_string())));
        assert!(!star.contains(&"CX".to_string()) && !star.contains(&"JL".to_string()));
        assert_eq!(c(&["oneworld"]).unwrap().len(), 15);
        assert!(c(&["skyteam"]).unwrap().contains(&"KE".to_string()));
        // names, codes and alliances mix; repeats go
        assert_eq!(
            c(&["Cathay Pacific", "sq", "ANA", "Star Alliance"]).unwrap()[..3],
            ["CX", "SQ", "NH"]
        );
        assert_eq!(c(&["AirAsia"]).unwrap(), ["AK", "D7", "FD", "QZ", "Z2"]);
        assert_eq!(
            c(&["Air Nowhere", "SQ"]),
            Err(vec!["Air Nowhere".to_string()])
        );
        assert_eq!(c(&[]).unwrap(), Vec::<String>::new());
        // every member of every alliance has a code, and all three alliances fit Kiwi's 200-character select_airlines
        for a in ALLIANCES {
            assert!(a.3.iter().all(|x| x.len() == 2), "{}", a.0);
        }
        let all = c(&["Star Alliance", "oneworld", "SkyTeam"]).unwrap();
        assert!(all.join(",").len() <= 200, "{}", all.join(",").len());
    }

    #[test]
    fn every_airline_in_the_profile_dropdown_has_a_code() {
        let js = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../web/app.js"))
            .unwrap();
        let start = js.find("const AIRLINES = [").unwrap();
        let list = &js[start..start + js[start..].find("];").unwrap()];
        let names: Vec<String> = list
            .split('\'')
            .skip(1)
            .step_by(2)
            .map(str::to_string)
            .collect();
        assert!(names.len() > 40, "{}", names.len());
        let missing: Vec<&String> = names
            .iter()
            .filter(|n| airline_codes(&[(*n).clone()]).is_err())
            .collect();
        assert!(missing.is_empty(), "no IATA code for {missing:?}");
    }

    #[test]
    fn alliances_and_groups_expand_to_their_members() {
        let a = expand(
            &["star alliance".into(), "Cathay Pacific".into()],
            "airline",
        );
        assert!(
            a.contains(&"united".to_string())
                && a.contains(&"ana".to_string())
                && a.contains(&"cathay pacific".to_string())
        );
        assert!(!a.contains(&"delta".to_string()));
        assert_eq!(alliance("ONEWORLD").unwrap().1, "ONEWORLD");
        assert!(alliance("Singapore Airlines").is_none());
        let h = expand(&["IHG".into()], "hotel");
        assert!(
            h.contains(&"holiday inn".to_string()) && h.contains(&"intercontinental".to_string())
        );
        assert!(
            hotel_group("Marriott").is_some()
                && hotel_group("Hilton Honors").is_some()
                && hotel_group("World of Hyatt").is_some()
        );
        assert!(hotel_group("Holiday Inn").is_none() && hotel_group("Hyatt").is_some());
        assert_eq!(group_word("World of Hyatt"), "Hyatt");
        assert_eq!(group_word("IHG One Rewards"), "IHG");
        assert!(
            hotel_group("ihg").is_some()
                && hotel_group("Accor").is_some()
                && hotel_group("Minor").is_some()
        );
        assert_eq!(expand(&[], "hotel"), Vec::<String>::new());
    }

    #[test]
    fn the_profile_dropdowns_offer_the_same_alliances_and_groups() {
        // web/app.js: ALLIANCES and HOTEL_GROUPS must name exactly what this table knows, so a dropdown choice always expands
        let js = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../web/app.js"),
        )
        .unwrap();
        let list = |name: &str| -> Vec<String> {
            let start = js
                .find(&format!("const {name} = ["))
                .unwrap_or_else(|| panic!("{name} missing in web/app.js"));
            let end = js[start..].find("];").unwrap() + start;
            js[start..end]
                .split('\'')
                .skip(1)
                .step_by(2)
                .map(str::to_string)
                .collect()
        };
        let mut js_all = list("ALLIANCES");
        js_all.sort();
        let mut rs_all: Vec<String> = ALLIANCES.iter().map(|a| a.0.to_string()).collect();
        rs_all.sort();
        assert_eq!(js_all, rs_all);
        let mut js_groups = list("HOTEL_GROUPS");
        js_groups.sort();
        let mut rs_groups: Vec<String> = HOTEL_GROUPS.iter().map(|g| g.0.to_string()).collect();
        rs_groups.sort();
        assert_eq!(js_groups, rs_groups);
    }
}
