//! Airline alliances and hotel groups: a preference such as "Star Alliance" or "IHG" stands for every member airline
//! or brand, so `flights_search airlines=["Star Alliance"]` flags a United or ANA offer and
//! `hotels_search brands=["IHG"]` flags a Holiday Inn. The Profile card's dropdowns (`web/app.js`) offer the same
//! names.

/// (alliance, Google Flights' `include_airlines` value, member airlines as Google / Kiwi name them)
pub const ALLIANCES: &[(&str, &str, &[&str])] = &[
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
            "ITA Airways",
            "TAROM",
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

fn same(a: &str, b: &str) -> bool {
    a.trim().eq_ignore_ascii_case(b.trim())
}

/// The alliance a preference names (any of "Star Alliance", "oneworld", "SkyTeam", case-insensitive).
pub fn alliance(
    name: &str,
) -> Option<&'static (&'static str, &'static str, &'static [&'static str])> {
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
