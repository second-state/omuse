//! The places code knows: the city table (code, name, the names a tool argument may carry, the ISO country) and the
//! countries (ISO 3166-1 alpha-2, English name, currency). The profile's structured place fields — `home_city` (a city
//! code of the table), `home_country` and the ISO country of each further address (`address_country:<label>`) — are
//! validated against these tables; code reads the profile's places only from those fields, never from address prose.

use serde_json::{json, Value};

use omuse_common::util::get_str;

/// A city of the table.
pub struct City {
    pub code: &'static str,
    pub name: &'static str,
    /// the names a structured argument may hold (compared whole, never searched in prose)
    pub words: &'static [&'static str],
    /// the ISO 3166-1 alpha-2 country
    pub country: &'static str,
}

#[rustfmt::skip]
pub const CITIES: &[City] = &[
    City { code: "TYO", name: "Tokyo", words: &["tokyo", "东京", "東京"], country: "JP" },
    City { code: "OSA", name: "Osaka", words: &["osaka", "大阪"], country: "JP" },
    City { code: "SEL", name: "Seoul", words: &["seoul", "首尔", "首爾"], country: "KR" },
    City { code: "SIN", name: "Singapore", words: &["singapore", "新加坡"], country: "SG" },
    City { code: "HKG", name: "Hong Kong", words: &["hong kong", "hongkong", "香港"], country: "HK" },
    City { code: "BKK", name: "Bangkok", words: &["bangkok", "曼谷"], country: "TH" },
    City { code: "TPE", name: "Taipei", words: &["taipei", "台北", "臺北"], country: "TW" },
    City { code: "SHA", name: "Shanghai", words: &["shanghai", "上海"], country: "CN" },
    City { code: "BJS", name: "Beijing", words: &["beijing", "北京"], country: "CN" },
    City { code: "CTU", name: "Chengdu", words: &["chengdu", "成都"], country: "CN" },
    City { code: "SZX", name: "Shenzhen", words: &["shenzhen", "深圳"], country: "CN" },
    City { code: "CAN", name: "Guangzhou", words: &["guangzhou", "广州"], country: "CN" },
    City { code: "KUL", name: "Kuala Lumpur", words: &["kuala lumpur", "吉隆坡"], country: "MY" },
    City { code: "DPS", name: "Bali", words: &["bali", "denpasar", "巴厘岛"], country: "ID" },
    City { code: "JKT", name: "Jakarta", words: &["jakarta", "雅加达"], country: "ID" },
    City { code: "MNL", name: "Manila", words: &["manila", "马尼拉"], country: "PH" },
    City { code: "HAN", name: "Hanoi", words: &["hanoi", "河内"], country: "VN" },
    City { code: "SGN", name: "Ho Chi Minh City", words: &["ho chi minh city", "ho chi minh", "saigon", "胡志明"], country: "VN" },
    City { code: "FUK", name: "Fukuoka", words: &["fukuoka", "福冈"], country: "JP" },
    City { code: "CTS", name: "Sapporo", words: &["sapporo", "札幌"], country: "JP" },
    City { code: "SYD", name: "Sydney", words: &["sydney", "悉尼"], country: "AU" },
    City { code: "MEL", name: "Melbourne", words: &["melbourne", "墨尔本"], country: "AU" },
    City { code: "LON", name: "London", words: &["london", "伦敦"], country: "GB" },
    City { code: "PAR", name: "Paris", words: &["paris", "巴黎"], country: "FR" },
    City { code: "NYC", name: "New York", words: &["new york", "纽约"], country: "US" },
    City { code: "QSF", name: "San Francisco", words: &["san francisco", "旧金山"], country: "US" },
    City { code: "QLA", name: "Los Angeles", words: &["los angeles", "洛杉矶"], country: "US" },
    City { code: "DXB", name: "Dubai", words: &["dubai", "迪拜"], country: "AE" },
];

/// A country: ISO 3166-1 alpha-2, its English name, its currency.
pub struct Country {
    pub iso: &'static str,
    pub name: &'static str,
    pub currency: &'static str,
}

#[rustfmt::skip]
pub const COUNTRIES: &[Country] = &[
    Country { iso: "AE", name: "United Arab Emirates", currency: "AED" },
    Country { iso: "AU", name: "Australia", currency: "AUD" },
    Country { iso: "BR", name: "Brazil", currency: "BRL" },
    Country { iso: "CA", name: "Canada", currency: "CAD" },
    Country { iso: "CH", name: "Switzerland", currency: "CHF" },
    Country { iso: "CN", name: "China", currency: "CNY" },
    Country { iso: "DE", name: "Germany", currency: "EUR" },
    Country { iso: "ES", name: "Spain", currency: "EUR" },
    Country { iso: "FR", name: "France", currency: "EUR" },
    Country { iso: "GB", name: "United Kingdom", currency: "GBP" },
    Country { iso: "HK", name: "Hong Kong", currency: "HKD" },
    Country { iso: "ID", name: "Indonesia", currency: "IDR" },
    Country { iso: "IE", name: "Ireland", currency: "EUR" },
    Country { iso: "IN", name: "India", currency: "INR" },
    Country { iso: "IT", name: "Italy", currency: "EUR" },
    Country { iso: "JP", name: "Japan", currency: "JPY" },
    Country { iso: "KH", name: "Cambodia", currency: "KHR" },
    Country { iso: "KR", name: "Korea", currency: "KRW" },
    Country { iso: "MO", name: "Macau", currency: "MOP" },
    Country { iso: "MX", name: "Mexico", currency: "MXN" },
    Country { iso: "MY", name: "Malaysia", currency: "MYR" },
    Country { iso: "NL", name: "Netherlands", currency: "EUR" },
    Country { iso: "NZ", name: "New Zealand", currency: "NZD" },
    Country { iso: "PH", name: "Philippines", currency: "PHP" },
    Country { iso: "SG", name: "Singapore", currency: "SGD" },
    Country { iso: "TH", name: "Thailand", currency: "THB" },
    Country { iso: "TW", name: "Taiwan", currency: "TWD" },
    Country { iso: "US", name: "United States", currency: "USD" },
    Country { iso: "VN", name: "Vietnam", currency: "VND" },
];

/// The city of the table with this code.
pub fn city(code: &str) -> Option<&'static City> {
    let c = code.trim().to_uppercase();
    CITIES.iter().find(|x| x.code == c)
}

/// The country with this ISO code.
pub fn country(iso: &str) -> Option<&'static Country> {
    let c = iso.trim().to_uppercase();
    COUNTRIES.iter().find(|x| x.iso == c)
}

/// The ISO code of a country of the table by its English name (code's own constants, e.g. an arrival card's country).
pub fn iso_of(name: &str) -> &'static str {
    COUNTRIES
        .iter()
        .find(|c| c.name.eq_ignore_ascii_case(name.trim()))
        .map(|c| c.iso)
        .unwrap_or("")
}

/// The profile key that holds the ISO country of an address key: `address_home` → `home_country`, `address_work` →
/// `address_country:work`, `address:<label>` → `address_country:<label>`; "" for anything else.
pub fn country_key(address_key: &str) -> String {
    match address_key {
        "address_home" => "home_country".into(),
        "address_work" => "address_country:work".into(),
        k => k
            .strip_prefix("address:")
            .map(|l| format!("address_country:{l}"))
            .unwrap_or_default(),
    }
}

/// A place field's value as stored: a city code of the table for `home_city`, an ISO code of the table for a country
/// field; `None` when the value is not one (the field is not a place field: `Some` of the value unchanged).
pub fn clean_value(key: &str, value: &str) -> Option<String> {
    let v = value.trim();
    if v.is_empty() {
        return Some(String::new());
    }
    if key == "home_city" {
        return city(v).map(|c| c.code.to_string());
    }
    if key == "home_country" || key.starts_with("address_country:") {
        return country(v).map(|c| c.iso.to_string());
    }
    Some(v.to_string())
}

/// The user's home city (the profile's `home_city`).
pub fn home_city(profile: &Value) -> Option<&'static City> {
    city(&get_str(profile, "home_city"))
}

/// The user's home country (the profile's `home_country`, else the home city's country), "" when unknown.
pub fn home_country(profile: &Value) -> String {
    let c = get_str(profile, "home_country");
    if country(&c).is_some() {
        return c.to_uppercase();
    }
    home_city(profile)
        .map(|c| c.country.to_string())
        .unwrap_or_default()
}

/// The profile's addresses that lack their country field: `{address key: address}`.
pub fn addresses_without_country(profile: &Value) -> Value {
    let mut out = serde_json::Map::new();
    if let Some(o) = profile.as_object() {
        for (k, v) in o {
            let ck = country_key(k);
            if !ck.is_empty() && get_str(profile, &ck).is_empty() {
                out.insert(k.clone(), v.clone());
            }
        }
    }
    Value::Object(out)
}

/// The city and country choices the UI and the model are offered: `{"cities": [{code, name}], "countries": [{iso,
/// name}]}`.
pub fn choices() -> Value {
    json!({
        "cities": CITIES.iter().map(|c| json!({"code": c.code, "name": c.name})).collect::<Vec<_>>(),
        "countries": COUNTRIES.iter().map(|c| json!({"iso": c.iso, "name": c.name})).collect::<Vec<_>>(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn place_fields_are_codes_of_the_tables() {
        assert_eq!(clean_value("home_city", "sin").as_deref(), Some("SIN"));
        assert_eq!(clean_value("home_city", "Singapore"), None);
        assert_eq!(clean_value("home_country", "hk").as_deref(), Some("HK"));
        assert_eq!(clean_value("address_country:flat", "Hong Kong"), None);
        assert_eq!(clean_value("home_country", "").as_deref(), Some(""));
        assert_eq!(clean_value("phone", " 123 ").as_deref(), Some("123"));
    }

    #[test]
    fn the_country_of_an_address_has_its_own_key() {
        assert_eq!(country_key("address_home"), "home_country");
        assert_eq!(
            country_key("address:Singapore flat"),
            "address_country:Singapore flat"
        );
        assert_eq!(country_key("phone"), "");
        let p = json!({"address_home": "1 A St", "home_country": "AU", "address:flat": "2 B Rd"});
        assert_eq!(
            addresses_without_country(&p),
            json!({"address:flat": "2 B Rd"})
        );
    }

    #[test]
    fn home_country_falls_back_to_the_home_city() {
        assert_eq!(home_country(&json!({"home_city": "HKG"})), "HK");
        assert_eq!(
            home_country(&json!({"home_country": "sg", "home_city": "HKG"})),
            "SG"
        );
        assert_eq!(
            home_country(&json!({"address_home": "Flat 1, Hong Kong"})),
            ""
        );
        assert_eq!(iso_of("United Kingdom"), "GB");
    }
}
