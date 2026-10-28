//! Cities with several airports: the IATA metropolitan ("city") code, the city's name and the airports Google Flights
//! takes for it. `flights_search` accepts any of the three forms — `TYO`, `Tokyo`, `HND,NRT` — so one search covers a
//! whole city; the flight skills carry the same list. One table for both flight sources: Google Flights wants the
//! airports (comma-separated), Kiwi.com's MCP wants the city's name.

/// (metro code, city name, airports, other names people use)
pub const CITIES: &[(&str, &str, &str, &[&str])] = &[
    ("TYO", "Tokyo", "HND,NRT", &[]),
    ("OSA", "Osaka", "KIX,ITM", &["kansai"]),
    ("SEL", "Seoul", "ICN,GMP", &[]),
    ("BJS", "Beijing", "PEK,PKX", &["peking"]),
    ("SHA", "Shanghai", "PVG,SHA", &[]),
    ("CTU", "Chengdu", "TFU,CTU", &[]),
    ("TPE", "Taipei", "TPE,TSA", &[]),
    ("BKK", "Bangkok", "BKK,DMK", &[]),
    ("KUL", "Kuala Lumpur", "KUL,SZB", &["kl"]),
    ("JKT", "Jakarta", "CGK,HLP", &[]),
    ("SGN", "Ho Chi Minh City", "SGN", &["saigon", "hcmc"]),
    ("HAN", "Hanoi", "HAN", &[]),
    ("SIN", "Singapore", "SIN", &[]),
    ("HKG", "Hong Kong", "HKG", &["hongkong"]),
    ("SZX", "Shenzhen", "SZX", &[]),
    ("CAN", "Guangzhou", "CAN", &["canton"]),
    ("MNL", "Manila", "MNL", &[]),
    ("DPS", "Bali", "DPS", &["denpasar"]),
    ("HKT", "Phuket", "HKT", &[]),
    ("DEL", "Delhi", "DEL", &["new delhi"]),
    ("BOM", "Mumbai", "BOM", &["bombay"]),
    ("DXB", "Dubai", "DXB,DWC", &[]),
    ("DOH", "Doha", "DOH", &[]),
    ("IST", "Istanbul", "IST,SAW", &[]),
    ("SYD", "Sydney", "SYD", &[]),
    ("MEL", "Melbourne", "MEL,AVV", &[]),
    ("AKL", "Auckland", "AKL", &[]),
    ("LON", "London", "LHR,LGW,STN,LTN,LCY", &[]),
    ("PAR", "Paris", "CDG,ORY", &[]),
    ("AMS", "Amsterdam", "AMS", &[]),
    ("FRA", "Frankfurt", "FRA", &[]),
    ("MUC", "Munich", "MUC", &["münchen", "munchen"]),
    ("ZRH", "Zurich", "ZRH", &["zürich"]),
    ("ROM", "Rome", "FCO,CIA", &["roma"]),
    ("MIL", "Milan", "MXP,LIN,BGY", &["milano"]),
    ("MAD", "Madrid", "MAD", &[]),
    ("BCN", "Barcelona", "BCN", &[]),
    ("BER", "Berlin", "BER", &[]),
    (
        "NYC",
        "New York",
        "JFK,EWR,LGA",
        &["new york city", "manhattan"],
    ),
    (
        "WAS",
        "Washington",
        "IAD,DCA,BWI",
        &["washington dc", "washington d c", "dc"],
    ),
    ("CHI", "Chicago", "ORD,MDW", &[]),
    (
        "QSF",
        "San Francisco",
        "SFO,OAK,SJC",
        &["sf", "bay area", "san francisco bay area"],
    ),
    ("QLA", "Los Angeles", "LAX,BUR,LGB,SNA,ONT", &["la"]),
    ("SEA", "Seattle", "SEA", &[]),
    ("BOS", "Boston", "BOS", &[]),
    ("MIA", "Miami", "MIA,FLL", &[]),
    ("HOU", "Houston", "IAH,HOU", &[]),
    ("DFW", "Dallas", "DFW,DAL", &["dallas fort worth"]),
    ("YTO", "Toronto", "YYZ,YTZ", &[]),
    ("YMQ", "Montreal", "YUL", &["montréal"]),
    ("YVR", "Vancouver", "YVR", &[]),
    ("MEX", "Mexico City", "MEX,NLU", &["cdmx"]),
    ("SAO", "São Paulo", "GRU,CGH,VCP", &["sao paulo"]),
    ("RIO", "Rio de Janeiro", "GIG,SDU", &["rio"]),
    ("BUE", "Buenos Aires", "EZE,AEP", &[]),
];

fn norm(s: &str) -> String {
    s.trim()
        .to_lowercase()
        .replace(['-', '_', '.', ','], " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn is_code(s: &str) -> bool {
    s.len() == 3 && s.chars().all(|c| c.is_ascii_alphabetic())
}

/// The city behind a metro code (`TYO`) or a city name (`Tokyo`, `saigon`); `None` for an airport code or an
/// unknown name.
pub fn city(
    place: &str,
) -> Option<&'static (
    &'static str,
    &'static str,
    &'static str,
    &'static [&'static str],
)> {
    let p = place.trim();
    if is_code(p) {
        let up = p.to_ascii_uppercase();
        return CITIES.iter().find(|c| c.0 == up);
    }
    let n = norm(p);
    CITIES
        .iter()
        .find(|c| norm(c.1) == n || c.3.iter().any(|a| norm(a) == n))
}

/// The airports Google Flights should take for a place: an airport code as given, a metro code or a city name as
/// all the city's airports; a comma list mixes them (`TYO,SIN` → `HND,NRT,SIN`). `None` for an unknown name.
pub fn airport_codes(place: &str) -> Option<String> {
    let mut out: Vec<String> = Vec::new();
    for part in place.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        let codes = match city(part) {
            Some(c) => c.2.to_string(),
            None if is_code(part) => part.to_ascii_uppercase(),
            None => return None,
        };
        for c in codes.split(',') {
            if !out.iter().any(|x| x == c) {
                out.push(c.to_string());
            }
        }
    }
    if out.is_empty() {
        None
    } else {
        Some(out.join(","))
    }
}

/// The place as Kiwi.com's search wants it: a metro code becomes the city's name (`TYO` → `Tokyo`, one search over
/// all its airports); an airport code or a name is passed as given.
pub fn kiwi_place(place: &str) -> String {
    let p = place.trim();
    match city(p) {
        Some(c) if is_code(p) => c.1.to_string(),
        _ => p.to_string(),
    }
}

/// The list the flight skills carry: "TYO = Tokyo (HND, NRT)" for every city with more than one airport.
pub fn skill_lines() -> Vec<String> {
    CITIES
        .iter()
        .filter(|c| c.2.contains(','))
        .map(|c| format!("{} = {} ({})", c.0, c.1, c.2.replace(',', ", ")))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metro_codes_city_names_and_airport_codes() {
        assert_eq!(airport_codes("TYO").as_deref(), Some("HND,NRT"));
        assert_eq!(airport_codes("tyo, sin").as_deref(), Some("HND,NRT,SIN"));
        assert_eq!(airport_codes("Tokyo").as_deref(), Some("HND,NRT"));
        assert_eq!(airport_codes("HND, NRT").as_deref(), Some("HND,NRT"));
        assert_eq!(airport_codes("LON").as_deref(), Some("LHR,LGW,STN,LTN,LCY"));
        assert_eq!(airport_codes("Ho-Chi-Minh City").as_deref(), Some("SGN"));
        assert_eq!(airport_codes("saigon").as_deref(), Some("SGN"));
        assert_eq!(airport_codes("sin").as_deref(), Some("SIN"));
        // an airport code that is also a city code stays one search over the city's airports
        assert_eq!(airport_codes("BKK").as_deref(), Some("BKK,DMK"));
        assert_eq!(airport_codes("XYZ").as_deref(), Some("XYZ"));
        assert_eq!(airport_codes("Atlantis"), None);
        assert_eq!(airport_codes(""), None);
        assert_eq!(kiwi_place("TYO"), "Tokyo");
        assert_eq!(kiwi_place("nyc"), "New York");
        assert_eq!(kiwi_place("NRT"), "NRT");
        assert_eq!(kiwi_place("Tokyo"), "Tokyo");
        assert!(skill_lines().contains(&"TYO = Tokyo (HND, NRT)".to_string()));
        assert!(!skill_lines().iter().any(|l| l.starts_with("SIN ")));
    }

    #[test]
    fn the_flight_skills_carry_the_same_city_list() {
        // skills/flight-booking and skills/travel-planning list every multi-airport city of this table
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../skills");
        for name in ["flight-booking", "travel-planning"] {
            let text = std::fs::read_to_string(root.join(name).join("SKILL.md")).unwrap();
            for line in skill_lines() {
                assert!(
                    text.contains(&line),
                    "{name}/SKILL.md lacks the line: {line}"
                );
            }
        }
    }
}
