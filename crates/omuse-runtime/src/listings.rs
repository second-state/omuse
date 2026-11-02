//! Product listings and the ranking of them (docs/json-tools.md, "Reading pages").
//!
//! The model turns the user's words ("the three best-rated cases under S$25 with at least 200 ratings") into
//! [`Ranking`] criteria; picking and ordering the listings is done here. The listings themselves are structured data,
//! never found by a pattern over a page's text: the model extracts them from the shop's page into JSON
//! (`present_choices` options with `price`, `stars` and `ratings`, [`Listing::from_option`]).

use serde_json::{json, Value};

/// One listing: the name, the star rating, the number of ratings and the price, each as read on the page.
#[derive(Debug, Clone, PartialEq)]
pub struct Listing {
    pub title: String,
    pub stars: f64,
    pub ratings: u64,
    /// The rating text exactly as on the page ("4.7 out of 5 stars · 1,203 ratings").
    pub rating_text: String,
    pub price: Option<f64>,
    /// The price text exactly as on the page ("S$21.90").
    pub price_text: String,
    /// The listing's own link, when the page gives one.
    pub url: String,
}

/// The numbers in a structured value, in order ("4.7 out of 5 stars · 1,203 ratings" → 4.7, 5, 1203; "S$21.90" →
/// 21.9): runs of digits with their thousands commas and decimal point.
pub fn numbers(s: &str) -> Vec<f64> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let flush = |cur: &mut String, out: &mut Vec<f64>| {
        let t = cur.trim_end_matches([',', '.']).replace(',', "");
        if let Ok(n) = t.parse::<f64>() {
            out.push(n);
        }
        cur.clear();
    };
    for c in s.chars() {
        if c.is_ascii_digit() || (!cur.is_empty() && (c == ',' || c == '.')) {
            cur.push(c);
        } else if !cur.is_empty() {
            flush(&mut cur, &mut out);
        }
    }
    if !cur.is_empty() {
        flush(&mut cur, &mut out);
    }
    out
}

/// The amount of a price element's value ("S$21.90" → 21.9): its first number.
pub fn amount(s: &str) -> Option<f64> {
    numbers(s).first().copied()
}

impl Listing {
    /// A listing the model extracted from a page into a `present_choices` option: `label`, `price` (a number),
    /// `price_text`, `stars`, `ratings`, `source_url`, and its `details` as the rating text.
    pub fn from_option(o: &Value) -> Option<Listing> {
        let title = o.get("label").and_then(Value::as_str)?.trim().to_string();
        if title.is_empty() {
            return None;
        }
        let num = |k: &str| {
            o.get(k).and_then(|x| {
                x.as_f64()
                    .or_else(|| x.as_str().and_then(amount))
                    .filter(|n| *n >= 0.0)
            })
        };
        let price_text = o
            .get("price_text")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim()
            .to_string();
        let details: Vec<String> = o
            .get("details")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .filter(|d| d != &price_text)
            .collect();
        Some(Listing {
            title,
            stars: num("stars").filter(|s| *s <= 5.0).unwrap_or(0.0),
            ratings: num("ratings").map_or(0, |r| r as u64),
            rating_text: details.join(" · "),
            price: num("price").or_else(|| amount(&price_text)),
            price_text,
            url: o
                .get("source_url")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
        })
    }
}

/// What the user asked for, as the model read it from their words.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Ranking {
    /// "rating" (best-rated first), "price" (cheapest first) or "ratings" (most ratings first).
    pub by: String,
    pub max_price: Option<f64>,
    pub min_price: Option<f64>,
    pub min_stars: Option<f64>,
    pub min_ratings: Option<u64>,
    /// Words every chosen name has ("case", "iPhone 17 Pro Max").
    pub title_has: Vec<String>,
    /// Words no chosen name has.
    pub title_not: Vec<String>,
    pub count: Option<usize>,
}

impl Ranking {
    /// The criteria as a tool call's `ranking` object (the inverse of [`Ranking::from_json`]).
    pub fn to_json(&self) -> Value {
        let mut o = serde_json::Map::new();
        o.insert("by".into(), json!(self.by));
        let mut put = |k: &str, v: Option<Value>| {
            if let Some(v) = v {
                o.insert(k.into(), v);
            }
        };
        put("max_price", self.max_price.map(|x| json!(x)));
        put("min_price", self.min_price.map(|x| json!(x)));
        put("min_stars", self.min_stars.map(|x| json!(x)));
        put("min_ratings", self.min_ratings.map(|x| json!(x)));
        put("count", self.count.map(|x| json!(x)));
        if !self.title_has.is_empty() {
            o.insert("title_has".into(), json!(self.title_has));
        }
        if !self.title_not.is_empty() {
            o.insert("title_not".into(), json!(self.title_not));
        }
        Value::Object(o)
    }

    /// The criteria from a tool call's `ranking` object; `None` when it is absent.
    pub fn from_json(v: &Value) -> Option<Ranking> {
        let o = v.as_object()?;
        let num = |k: &str| {
            o.get(k)
                .and_then(|x| x.as_f64().or_else(|| x.as_str().and_then(amount)))
                // a model fills every key: 0 is "not given" (no cap, no minimum, the options' own count)
                .filter(|n| *n > 0.0)
        };
        let words = |k: &str| -> Vec<String> {
            match o.get(k) {
                Some(Value::Array(a)) => a
                    .iter()
                    .filter_map(|w| w.as_str())
                    .map(str::to_string)
                    .collect(),
                Some(Value::String(s)) if !s.trim().is_empty() => vec![s.clone()],
                _ => vec![],
            }
        };
        Some(Ranking {
            by: o
                .get("by")
                .and_then(|x| x.as_str())
                .unwrap_or("rating")
                .to_lowercase(),
            max_price: num("max_price"),
            min_price: num("min_price"),
            min_stars: num("min_stars"),
            min_ratings: num("min_ratings").map(|n| n as u64),
            title_has: words("title_has"),
            title_not: words("title_not"),
            count: num("count").map(|n| n as usize),
        })
    }

    /// The listings that meet the criteria, best first.
    pub fn apply(&self, listings: &[Listing]) -> Vec<Listing> {
        let has = |title: &str, w: &str| title.to_lowercase().contains(&w.trim().to_lowercase());
        let mut kept: Vec<Listing> = listings
            .iter()
            .filter(|l| self.title_has.iter().all(|w| has(&l.title, w)))
            .filter(|l| {
                !self
                    .title_not
                    .iter()
                    .any(|w| !w.trim().is_empty() && has(&l.title, w))
            })
            .filter(|l| self.min_stars.is_none_or(|m| l.stars >= m))
            .filter(|l| self.min_ratings.is_none_or(|m| l.ratings >= m))
            .filter(|l| {
                self.max_price
                    .is_none_or(|m| l.price.is_some_and(|p| p <= m))
            })
            .filter(|l| {
                self.min_price
                    .is_none_or(|m| l.price.is_some_and(|p| p >= m))
            })
            .cloned()
            .collect();
        let price = |l: &Listing| l.price.unwrap_or(f64::MAX);
        kept.sort_by(|a, b| match self.by.as_str() {
            "price" => price(a)
                .total_cmp(&price(b))
                .then(b.stars.total_cmp(&a.stars)),
            "ratings" => b.ratings.cmp(&a.ratings).then(b.stars.total_cmp(&a.stars)),
            _ => b.stars.total_cmp(&a.stars).then(b.ratings.cmp(&a.ratings)),
        });
        kept.truncate(self.count.unwrap_or(3).clamp(1, 8));
        kept
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// A shop's results page as the model extracts its listings into `present_choices` options.
    fn page() -> Vec<Listing> {
        [
            (
                "Titan Pro Shockproof Case for iPhone 17 Pro (6.3-inch), Military Grade",
                "4.9 out of 5 stars · 860 ratings",
                "S$22.90",
            ),
            (
                "Camera Lens Protector for iPhone 17 Pro Max, 2-Pack",
                "4.8 out of 5 stars · 2,770 ratings",
                "S$9.90",
            ),
            (
                "Crystal Clear Case for iPhone 16 Pro Max",
                "4.8 out of 5 stars · 5,120 ratings",
                "S$11.90",
            ),
            (
                "Kickstand Case for iPhone 17 Pro Max, MagSafe Ring Stand",
                "4.8 out of 5 stars · 379 ratings",
                "S$33.90",
            ),
            (
                "MagSafe Silicone Case for iPhone 17 Pro Max",
                "4.4 out of 5 stars · 2,048 ratings",
                "S$19.90",
            ),
            (
                "Crossbody Lanyard Case for iPhone 17 Pro Max",
                "4.5 out of 5 stars · 154 ratings",
                "S$23.90",
            ),
            (
                "Carbon Fibre Case for iPhone 17 Pro Max, Ultra Thin 0.6 mm",
                "4.5 out of 5 stars · 268 ratings",
                "S$18.90",
            ),
            (
                "Slim Leather Case for iPhone 17 Pro Max, MagSafe Compatible",
                "4.6 out of 5 stars · 512 ratings",
                "S$24.90",
            ),
            (
                "Aurora Glitter Case for iPhone 17 Pro Max, MagSafe Compatible",
                "4.7 out of 5 stars · 1,203 ratings",
                "S$21.90",
            ),
        ]
        .iter()
        .filter_map(|(t, r, p)| {
            let n = numbers(r);
            Listing::from_option(&json!({"label": t, "details": [r], "price_text": p,
                "stars": n[0], "ratings": n[2], "source_url": "https://shop.example/item"}))
        })
        .collect()
    }

    #[test]
    fn a_listings_rating_count_and_price_are_numbers() {
        let l = page();
        assert_eq!(l.len(), 9);
        let aurora = l.iter().find(|x| x.title.starts_with("Aurora")).unwrap();
        assert_eq!(
            (aurora.stars, aurora.ratings, aurora.price),
            (4.7, 1203, Some(21.9))
        );
        assert_eq!(aurora.price_text, "S$21.90");
        assert_eq!(numbers("RM 1,299.50 (12)"), [1299.5, 12.0]);
    }

    #[test]
    fn best_rated_cases_under_a_price_with_enough_ratings_are_the_top_three() {
        let r = Ranking::from_json(
            &json!({"by": "rating", "max_price": "S$25", "min_ratings": 200,
            "title_has": ["case", "iPhone 17 Pro Max"], "count": 3}),
        )
        .unwrap();
        let titles: Vec<String> = r.apply(&page()).into_iter().map(|l| l.title).collect();
        assert_eq!(
            titles,
            [
                "Aurora Glitter Case for iPhone 17 Pro Max, MagSafe Compatible",
                "Slim Leather Case for iPhone 17 Pro Max, MagSafe Compatible",
                "Carbon Fibre Case for iPhone 17 Pro Max, Ultra Thin 0.6 mm"
            ]
        );
    }

    #[test]
    fn cheapest_first_and_excluded_words() {
        let r = Ranking::from_json(
            &json!({"by": "price", "title_has": "case", "title_not": ["16 Pro"], "count": 2}),
        )
        .unwrap();
        let titles: Vec<String> = r.apply(&page()).into_iter().map(|l| l.title).collect();
        assert_eq!(
            titles[0],
            "Carbon Fibre Case for iPhone 17 Pro Max, Ultra Thin 0.6 mm"
        );
        assert_eq!(titles.len(), 2);
    }

    #[test]
    fn zeros_a_model_fills_in_are_not_criteria() {
        let r = Ranking::from_json(
            &json!({"by": "rating", "max_price": 0, "min_price": 0, "min_ratings": 0,
            "count": 0, "title_has": [], "title_not": []}),
        )
        .unwrap();
        assert_eq!((r.max_price, r.min_ratings, r.count), (None, None, None));
        assert_eq!(r.apply(&page()).len(), 3);
    }

    #[test]
    fn listings_the_model_extracted_into_json_are_ranked_too() {
        let opts = [
            json!({"label": "Trail Runner Shoe Lightweight Mesh", "details": ["4.3/5 (1.2k)"], "price": 129,
                   "price_text": "RM 129.00", "stars": 4.3, "ratings": 1200, "source_url": "https://shop.example/s"}),
            json!({"label": "Waterproof Hiking Boot Men", "details": ["4.6 stars 312 reviews", "$89.50"],
                   "price_text": "$89.50", "stars": "4.6", "ratings": 312}),
            json!({"details": ["no label"]}),
        ];
        let l: Vec<Listing> = opts.iter().filter_map(Listing::from_option).collect();
        assert_eq!(l.len(), 2);
        assert_eq!(
            (l[0].stars, l[0].ratings, l[0].price),
            (4.3, 1200, Some(129.0))
        );
        assert_eq!(
            (l[1].price, l[1].rating_text.as_str()),
            (Some(89.5), "4.6 stars 312 reviews")
        );
        let r = Ranking::from_json(&json!({"by": "price"})).unwrap();
        assert_eq!(r.apply(&l)[0].title, "Waterproof Hiking Boot Men");
    }
}
