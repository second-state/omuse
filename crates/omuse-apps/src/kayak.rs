//! Kayak "compare" links for a flight or hotel search: Kayak's results page for the same search, built in code from the
//! validated arguments — no extra call, no request to Kayak. Formats checked on 2026-10-29 (the page's title names the
//! search): `/flights/SFO-CTU/2026-12-10[/2026-12-20][/business|premium|first][/2adults]?fs=stops=0;airlines=SQ,NH`
//! (an airport list as `HND,NRT`), `/hotels/Ginza,Tokyo/2026-12-10/2026-12-12/2adults`.

use serde_json::Value;

use omuse_common::util::get_str;

fn enc(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes()).collect()
}

/// Kayak's flight results for a `flights_search`: `date` is the departure day to show (the first, or the cheapest day of a
/// range). `None` when a place is not an airport or a city OMuse knows.
pub fn flights_url(base: &str, a: &Value, date: &str) -> Option<String> {
    let from = crate::airports::airport_codes(&get_str(a, "origin"))?;
    let to = crate::airports::airport_codes(&get_str(a, "destination"))?;
    let mut url = format!("{}/flights/{from}-{to}/{date}", base.trim_end_matches('/'));
    let ret = get_str(a, "return_date");
    if ret.len() == 10 {
        // over a range the return moves with the departure: keep the stay's length
        let back = chrono::NaiveDate::parse_from_str(&ret, "%Y-%m-%d")
            .ok()
            .zip(chrono::NaiveDate::parse_from_str(&get_str(a, "date"), "%Y-%m-%d").ok())
            .zip(chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d").ok())
            .map(|((r, first), d)| (d + (r - first)).format("%Y-%m-%d").to_string())
            .unwrap_or(ret);
        url.push('/');
        url.push_str(&back);
    }
    match get_str(a, "cabin").as_str() {
        "business" => url.push_str("/business"),
        "first" => url.push_str("/first"),
        "premium_economy" => url.push_str("/premium"),
        _ => {}
    }
    let adults = crate::serpapi::people(a, "adults", 1, 9);
    if adults > 1 {
        url.push_str(&format!("/{adults}adults"));
    }
    let mut fs: Vec<String> = Vec::new();
    if crate::serpapi::max_stops(a) == Some(0) {
        fs.push("stops=0".into());
    }
    // preferred airlines given as airlines (an alliance stays out: Kayak's page would list only the codes we know)
    let prefs = crate::serpapi::pref_list(a, "airlines");
    if !prefs.is_empty() && !prefs.iter().any(|p| crate::loyalty::alliance(p).is_some()) {
        if let Ok(codes) = crate::loyalty::airline_codes(&prefs) {
            if !codes.is_empty() && codes.len() <= 8 {
                fs.push(format!("airlines={}", codes.join(",")));
            }
        }
    }
    url.push_str("?sort=price_a");
    if !fs.is_empty() {
        url.push_str(&format!("&fs={}", fs.join(";")));
    }
    Some(url)
}

/// Kayak's hotel results for a `hotels_search`: the area and city, the dates, the guests.
pub fn hotels_url(base: &str, a: &Value) -> Option<String> {
    let city = get_str(a, "city");
    let (ci, co) = (get_str(a, "check_in"), get_str(a, "check_out"));
    if city.trim().is_empty() || ci.len() != 10 || co.len() != 10 {
        return None;
    }
    let area = get_str(a, "area");
    let place = if area.trim().is_empty() {
        city.trim().to_string()
    } else {
        format!("{},{}", area.trim(), city.trim())
    };
    Some(format!(
        "{}/hotels/{}/{ci}/{co}/{}adults?sort=price_a",
        base.trim_end_matches('/'),
        enc(&place),
        crate::serpapi::people(a, "guests", 2, 20)
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const K: &str = "https://www.kayak.com";

    #[test]
    fn flight_searches_become_kayaks_page_for_the_same_search() {
        // a city code is every airport of the city, as the search itself uses (CTU = Tianfu + Shuangliu)
        let a = json!({"origin": "SFO", "destination": "CTU", "date": "2026-12-10"});
        assert_eq!(
            flights_url(K, &a, "2026-12-10").unwrap(),
            "https://www.kayak.com/flights/SFO-TFU,CTU/2026-12-10?sort=price_a"
        );
        let a = json!({"origin": "TYO", "destination": "SIN", "date": "2026-12-10", "return_date": "2026-12-20",
                       "cabin": "business", "adults": 2, "stops": "nonstop", "airlines": ["SQ", "ANA"]});
        assert_eq!(flights_url(K, &a, "2026-12-10").unwrap(),
                   "https://www.kayak.com/flights/HND,NRT-SIN/2026-12-10/2026-12-20/business/2adults?sort=price_a&fs=stops=0;airlines=SQ,NH");
        // the cheapest day of a range: the return keeps the stay's length
        assert!(flights_url(K, &a, "2026-12-12")
            .unwrap()
            .contains("/2026-12-12/2026-12-22/"));
        // an alliance is not a Kayak airline filter
        let a = json!({"origin": "SIN", "destination": "NRT", "date": "2026-12-10", "airlines": ["Star Alliance"]});
        assert!(!flights_url(K, &a, "2026-12-10")
            .unwrap()
            .contains("airlines="));
        assert!(flights_url(
            K,
            &json!({"origin": "Narnia", "destination": "NRT"}),
            "2026-12-10"
        )
        .is_none());
    }

    #[test]
    fn hotel_searches_become_kayaks_page_for_the_same_stay() {
        let a = json!({"city": "Tokyo", "area": "Ginza", "check_in": "2026-12-10", "check_out": "2026-12-12", "guests": 1});
        assert_eq!(
            hotels_url(K, &a).unwrap(),
            "https://www.kayak.com/hotels/Ginza%2CTokyo/2026-12-10/2026-12-12/1adults?sort=price_a"
        );
        let a = json!({"city": "Tokyo", "check_in": "2026-12-10", "check_out": "2026-12-12"});
        assert_eq!(
            hotels_url(K, &a).unwrap(),
            "https://www.kayak.com/hotels/Tokyo/2026-12-10/2026-12-12/2adults?sort=price_a"
        );
        assert!(hotels_url(K, &json!({"city": "Tokyo"})).is_none());
    }
}
