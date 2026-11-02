//! `cars_search`: car rental at an airport. No rental site has a public search API (Kayak's is for approved affiliates
//! only, Hertz has none), and Google has no car-rental data, so the tool makes ONE web search for the companies and
//! "from" prices, and builds the booking link itself from the pickup airport, the dates and the car class: Kayak's
//! results page for that rental (`kayak.com/cars/LAX/SFO/2026-12-03-10h/2026-12-06-14h?fs=carclass=suv`), else
//! Rentalcars.com's (`/search-results?locationIata=…`). Both formats were checked in a browser on 2026-10-29: the page
//! opens on the live offers for those dates (Kayak's car classes: small, medium, large, suv, van, luxury, convertible;
//! an unknown one is ignored there).

use chrono::{Datelike, NaiveDate};
use serde_json::Value;

use omuse_common::util::get_str;

/// The car classes `cars_search` takes, as Kayak's `carclass` filter names them.
pub const CAR_CLASSES: [&str; 7] = [
    "small",
    "medium",
    "large",
    "suv",
    "van",
    "luxury",
    "convertible",
];

/// Rental companies as web results name them (capitals as written, so "budget hotels" or "economy cars" are no
/// company). Global brands, the ones aggregators list at US and European airports, and Japan's and Korea's own.
const COMPANIES: [&str; 46] = [
    "Hertz",
    "Avis",
    "Budget",
    "Enterprise",
    "National",
    "Alamo",
    "Sixt",
    "SIXT",
    "Europcar",
    "Dollar",
    "Thrifty",
    "Fox Rent A Car",
    "Payless",
    "Ace Rent A Car",
    "Economy Rent a Car",
    "NU Car",
    "Yesaway",
    "Movly",
    "NextCar",
    "Priceless",
    "U-Save",
    "Green Motion",
    "GREEN MOTION",
    "Drive Rental Cars",
    "Advantage",
    "Goldcar",
    "Firefly",
    "Keddy",
    "Surprice",
    "Zipcar",
    "Turo",
    "ORIX Rent a Car",
    "Toyota Rent a Car",
    "Nippon Rent-A-Car",
    "Times Car Rental",
    "Nissan Rent a Car",
    "Ota Rent a Car",
    "OTS Rent a Car",
    "Relax Rent A Car",
    "Easy Car",
    "Asuka Rental Car",
    "GALILEO Rent a Car",
    "Lotte Rent-a-car",
    "SK Rent a Car",
    "Bidvest",
    "Localiza",
];

/// The rental companies `text` (web results' titles and snippets) names, each once, in the order they first appear, so
/// the answer can name them even when the snippets bury them among aggregator blurbs.
pub fn companies_in(text: &str) -> Vec<String> {
    let mut found: Vec<(usize, String)> = COMPANIES
        .iter()
        .filter_map(|c| {
            let re = regex::Regex::new(&format!(r"\b{}\b", regex::escape(c))).ok()?;
            re.find(text).map(|m| (m.start(), (*c).to_string()))
        })
        .collect();
    found.sort();
    let mut out: Vec<String> = Vec::new();
    for (_, c) in found {
        let norm = |x: &str| x.to_lowercase();
        if !out.iter().any(|o| norm(o) == norm(&c)) {
            out.push(c);
        }
    }
    out
}

/// One rental, read from validated `cars_search` arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rental {
    /// IATA code of the pickup airport.
    pub pickup: String,
    /// IATA code of the return airport (the pickup one unless the user returns elsewhere).
    pub dropoff: String,
    pub pickup_date: NaiveDate,
    /// Local hour, 0–23.
    pub pickup_hour: u32,
    pub dropoff_date: NaiveDate,
    pub dropoff_hour: u32,
    /// One of [`CAR_CLASSES`].
    pub car_class: Option<String>,
    pub driver_age: u32,
}

/// The first airport of a place: an airport code as given, a city code or name → its first airport (`TYO` → `HND`).
fn airport(place: &str) -> Option<String> {
    let codes = crate::airports::airport_codes(place)?;
    codes.split(',').next().map(str::to_string)
}

/// An hour from "10:00", "9", "14h", "2:30 PM" …; `None` when it is not a time of day.
pub fn hour(s: &str) -> Option<u32> {
    let t = s.trim().to_ascii_lowercase();
    let (t, pm) = match (t.strip_suffix("pm"), t.strip_suffix("am")) {
        (Some(x), _) => (x.trim().to_string(), Some(true)),
        (_, Some(x)) => (x.trim().to_string(), Some(false)),
        _ => (t.trim_end_matches('h').to_string(), None),
    };
    let h: u32 = t.split(':').next()?.trim().parse().ok()?;
    let h = match pm {
        Some(true) if h < 12 => h + 12,
        Some(false) if h == 12 => 0,
        _ => h,
    };
    (h < 24).then_some(h)
}

impl Rental {
    /// From `cars_search` arguments that `travel_args::validate` has already checked (dates real and in order, times
    /// and class normalised); `Err` says what is missing.
    pub fn from_args(a: &Value) -> Result<Self, String> {
        let pickup_place = get_str(a, "pickup");
        let pickup = airport(&pickup_place).ok_or_else(|| {
            format!("cars_search: unknown pickup place {pickup_place:?} — give the pickup airport's code (NRT, LAX, LHR …)")
        })?;
        let drop_place = get_str(a, "dropoff");
        let dropoff = if drop_place.trim().is_empty() {
            pickup.clone()
        } else {
            airport(&drop_place).ok_or_else(|| {
                format!("cars_search: unknown return place {drop_place:?} — give the return airport's code")
            })?
        };
        let date = |k: &str| {
            NaiveDate::parse_from_str(&get_str(a, k), "%Y-%m-%d")
                .map_err(|_| format!("cars_search: {k} must be YYYY-MM-DD"))
        };
        let class = get_str(a, "car_class").to_ascii_lowercase();
        Ok(Self {
            pickup,
            dropoff,
            pickup_date: date("pickup_date")?,
            pickup_hour: hour(&get_str(a, "pickup_time")).unwrap_or(10),
            dropoff_date: date("dropoff_date")?,
            dropoff_hour: hour(&get_str(a, "dropoff_time")).unwrap_or(10),
            car_class: CAR_CLASSES.contains(&class.as_str()).then_some(class),
            driver_age: a
                .get("driver_age")
                .and_then(Value::as_u64)
                .and_then(|n| u32::try_from(n).ok())
                .filter(|n| (18..=99).contains(n))
                .unwrap_or(30),
        })
    }

    /// Days the car is kept (a started day counts).
    pub fn days(&self) -> i64 {
        let d = (self.dropoff_date - self.pickup_date).num_days();
        if self.dropoff_hour > self.pickup_hour {
            d + 1
        } else {
            d.max(1)
        }
    }

    /// Kayak's results page for this rental; `base` is `https://www.kayak.com` (tests: the fake).
    pub fn kayak_url(&self, base: &str) -> String {
        let route = if self.dropoff == self.pickup {
            self.pickup.clone()
        } else {
            format!("{}/{}", self.pickup, self.dropoff)
        };
        let mut url = format!(
            "{}/cars/{route}/{}-{}h/{}-{}h",
            base.trim_end_matches('/'),
            self.pickup_date,
            self.pickup_hour,
            self.dropoff_date,
            self.dropoff_hour
        );
        if let Some(c) = &self.car_class {
            url.push_str(&format!("?fs=carclass={c}"));
        }
        url
    }

    /// Rentalcars.com's results page for this rental; `base` is `https://www.rentalcars.com`.
    pub fn rentalcars_url(&self, base: &str) -> String {
        let (p, d) = (self.pickup_date, self.dropoff_date);
        format!(
            "{}/search-results?locationIata={}&dropLocationIata={}&puDay={}&puMonth={}&puYear={}&puHour={}&puMinute=0\
&doDay={}&doMonth={}&doYear={}&doHour={}&doMinute=0&driversAge={}",
            base.trim_end_matches('/'),
            self.pickup,
            self.dropoff,
            p.day(),
            p.month(),
            p.year(),
            self.pickup_hour,
            d.day(),
            d.month(),
            d.year(),
            self.dropoff_hour,
            self.driver_age
        )
    }

    /// The one web search's words: the place, the class, the preferred companies (no dates: rental pages are not dated,
    /// and dates in the words only push them down; the booking link carries the dates). With no company asked for, the
    /// words ask for the rental *companies*: the snippets then name them with their "from" prices ("Compare offers from
    /// Europcar, NU Car and Yesaway, starting at $24/day"), where plain "car rental" brought aggregator pages that name
    /// none (2026-10-30, Brave and DuckDuckGo).
    pub fn web_query(&self, companies: &[String]) -> String {
        let place = crate::airports::CITIES
            .iter()
            .find(|c| c.2.split(',').any(|x| x == self.pickup))
            .map(|c| format!("{} {} airport", c.1, self.pickup))
            .unwrap_or_else(|| format!("{} airport", self.pickup));
        let to = if self.dropoff == self.pickup {
            String::new()
        } else {
            format!(" one way to {}", self.dropoff)
        };
        let class = self
            .car_class
            .as_deref()
            .map(|c| {
                if c == "suv" {
                    "SUV ".to_string()
                } else {
                    format!("{c} ")
                }
            })
            .unwrap_or_default();
        if companies.is_empty() {
            format!("{class}car rental companies {place}{to} prices")
        } else {
            format!(
                "{} {class}car rental {place}{to} prices",
                companies.join(" or ")
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn rental_companies_are_read_from_the_snippets() {
        let t = "SUV rentals at Los Angeles Airport. Compare offers from Europcar, NU Car and Yesaway, starting at $24/day | KAYAK\n\
                 Find budget deals and economy cars. Latest prices: Fox Rent A Car $25/day. SIXT $25/day. Sixt again. \
                 ORIX Rent a Car and Relax Rent A Car each operate 3 locations; Los Angeles International Airport";
        assert_eq!(
            companies_in(t),
            [
                "Europcar",
                "NU Car",
                "Yesaway",
                "Fox Rent A Car",
                "SIXT",
                "ORIX Rent a Car",
                "Relax Rent A Car"
            ]
        );
        assert!(companies_in("Cheap car hire at the airport from $15/day").is_empty());
    }

    #[test]
    fn hours_from_what_people_write() {
        for (s, h) in [
            ("10:00", Some(10)),
            ("9", Some(9)),
            ("14h", Some(14)),
            ("2:30 PM", Some(14)),
            ("12am", Some(0)),
            ("12 pm", Some(12)),
            ("25:00", None),
            ("noon", None),
            ("", None),
        ] {
            assert_eq!(hour(s), h, "{s}");
        }
    }

    #[test]
    fn a_rental_becomes_the_kayak_and_rentalcars_pages_for_it() {
        let r = Rental::from_args(&json!({"pickup": "LAX", "dropoff": "SFO", "pickup_date": "2026-12-03",
                                          "dropoff_date": "2026-12-06", "pickup_time": "10:00", "dropoff_time": "2 PM",
                                          "car_class": "suv"})).unwrap();
        assert_eq!(
            r.kayak_url("https://www.kayak.com"),
            "https://www.kayak.com/cars/LAX/SFO/2026-12-03-10h/2026-12-06-14h?fs=carclass=suv"
        );
        assert_eq!(r.rentalcars_url("https://www.rentalcars.com/"),
                   "https://www.rentalcars.com/search-results?locationIata=LAX&dropLocationIata=SFO&puDay=3&puMonth=12&puYear=2026\
&puHour=10&puMinute=0&doDay=6&doMonth=12&doYear=2026&doHour=14&doMinute=0&driversAge=30");
        assert_eq!(r.days(), 4);
        assert_eq!(
            r.web_query(&["Hertz".into()]),
            "Hertz SUV car rental Los Angeles LAX airport one way to SFO prices"
        );
        // a city code is its first airport, the return defaults to the pickup, no class means no filter
        let r = Rental::from_args(
            &json!({"pickup": "TYO", "pickup_date": "2026-12-03", "dropoff_date": "2026-12-06"}),
        )
        .unwrap();
        assert_eq!(
            (r.pickup.as_str(), r.dropoff.as_str(), r.days()),
            ("HND", "HND", 3)
        );
        assert_eq!(
            r.kayak_url("https://www.kayak.com"),
            "https://www.kayak.com/cars/HND/2026-12-03-10h/2026-12-06-10h"
        );
        // no company asked for: the search asks for the companies, so the results name them
        assert_eq!(
            r.web_query(&[]),
            "car rental companies Tokyo HND airport prices"
        );
        assert!(Rental::from_args(
            &json!({"pickup": "Narnia", "pickup_date": "2026-12-03", "dropoff_date": "2026-12-06"})
        )
        .unwrap_err()
        .contains("pickup airport's code"));
    }
}
