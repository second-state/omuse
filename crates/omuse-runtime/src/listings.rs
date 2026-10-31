//! Product listings read off a shop's results page, and the ranking of them.
//!
//! The model turns the user's words ("the three best-rated cases under S$25 with at least 200 ratings") into
//! [`Ranking`] criteria; picking and ordering the listings is done here, from the page text, so a comparison shows
//! exactly the listings that meet them.

use std::sync::LazyLock;

use regex::Regex;
use serde_json::Value;

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
}

static STARS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b([0-5](?:\.\d)?)\s*(?:out of 5(?:\s*stars?)?|/\s*5\b|★|stars?\b)").unwrap()
});
static COUNT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(\d[\d,.]*)\s*(k)?\+?\s*(?:ratings?|reviews?|评价|条评论)|\((\d[\d,.]*)(k)?\)")
        .unwrap()
});
static PRICE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?:S\$|US\$|A\$|HK\$|NT\$|RM|SGD|USD|EUR|GBP|MYR|\$|€|£|¥|₹|฿|₩)\s?(\d[\d,]*(?:\.\d{1,2})?)").unwrap()
});
// snapshot lines: `[e12] link "Name"` → `Name`
static SNAP: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"^\s*(?:\[e\d+\]\s*)?(?:[a-z]+\s+")?(.*?)"?\s*$"#).unwrap());
static NOISE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^(sponsored|ad|free delivery\b.*|delivery\b.*|add to cart|buy now|in stock|only \d+ left.*)$")
        .unwrap()
});

fn number(s: &str) -> Option<f64> {
    s.replace(',', "").parse().ok()
}

fn count_of(text: &str) -> u64 {
    COUNT
        .captures(text)
        .and_then(|c| {
            let (n, k) = match c.get(1) {
                Some(n) => (n.as_str(), c.get(2).is_some()),
                None => (c.get(3)?.as_str(), c.get(4).is_some()),
            };
            let n = number(n)?;
            Some((if k { n * 1000.0 } else { n }) as u64)
        })
        .unwrap_or(0)
}

fn clean(line: &str) -> String {
    SNAP.captures(line)
        .and_then(|c| c.get(1))
        .map_or(line, |m| m.as_str())
        .trim()
        .to_string()
}

fn is_title(line: &str) -> bool {
    line.split_whitespace().count() >= 3
        && !STARS.is_match(line)
        && !PRICE.is_match(line)
        && !NOISE.is_match(line)
}

/// The listings on a results page: every star rating with the name above it and the price next to it.
pub fn parse(text: &str) -> Vec<Listing> {
    let lines: Vec<String> = text.lines().map(clean).collect();
    let mut out: Vec<Listing> = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        let Some(m) = STARS.captures(line) else {
            continue;
        };
        let whole = m.get(0).unwrap();
        let Some(stars) = number(&m[1]) else { continue };
        // the name: before the rating on the same line, or the nearest name line above it
        let before = line[..whole.start()].trim();
        let title = if is_title(before) {
            before.to_string()
        } else {
            match (1..=3)
                .filter_map(|k| i.checked_sub(k))
                .map(|j| &lines[j])
                .find(|l| is_title(l))
            {
                Some(t) => t.clone(),
                None => continue,
            }
        };
        // the price: after the rating on the same line, or within the next lines (before the next listing's name)
        let after = &line[whole.end()..];
        let price = PRICE.captures(after).or_else(|| {
            lines
                .iter()
                .skip(i + 1)
                .take(3)
                .take_while(|l| !is_title(l))
                .find_map(|l| PRICE.captures(l))
        });
        let rating_text = match PRICE.find(after) {
            Some(p) => line[..whole.end() + p.start()].trim(),
            None => line.as_str(),
        };
        let rating_text = rating_text
            .strip_prefix(before)
            .unwrap_or(rating_text)
            .trim();
        if out.iter().any(|l| l.title == title) {
            continue;
        }
        out.push(Listing {
            title,
            stars,
            ratings: count_of(rating_text),
            rating_text: rating_text.to_string(),
            price: price.as_ref().and_then(|p| number(&p[1])),
            price_text: price
                .as_ref()
                .map(|p| p[0].trim().to_string())
                .unwrap_or_default(),
        });
    }
    out
}

/// Whether `text` shows a price.
pub fn has_price(text: &str) -> bool {
    PRICE.is_match(text)
}

/// The first price a page shows after `name` (within the next 300 characters), as written there ("US$365.57"): a card
/// without a price gets the one the page gives it.
pub fn price_after(text: &str, name: &str) -> Option<String> {
    let name = name.trim();
    if name.chars().count() < 4 {
        return None;
    }
    let low = text.to_lowercase();
    let at = low.find(&name.to_lowercase())? + name.len();
    let tail: String = text.get(at..)?.chars().take(300).collect();
    PRICE.find(&tail).map(|m| m.as_str().trim().to_string())
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
    /// The criteria from a tool call's `ranking` object; `None` when it is absent.
    pub fn from_json(v: &Value) -> Option<Ranking> {
        let o = v.as_object()?;
        let num = |k: &str| {
            o.get(k)
                .and_then(|x| {
                    x.as_f64().or_else(|| {
                        x.as_str()
                            .and_then(|s| PRICE.captures(s).map_or(number(s), |c| number(&c[1])))
                    })
                })
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

    const PAGE: &str = "Results for \"iphone 17 pro max case\"\n\
Sponsored\nTitan Pro Shockproof Case for iPhone 17 Pro (6.3-inch), Military Grade\n4.9 out of 5 stars · 860 ratings\nS$22.90\nFREE delivery by Kaya Mobile\n\
Camera Lens Protector for iPhone 17 Pro Max, 2-Pack\n4.8 out of 5 stars · 2,770 ratings\nS$9.90\nFREE delivery by Kaya Mobile\n\
Crystal Clear Case for iPhone 16 Pro Max\n4.8 out of 5 stars · 5,120 ratings\nS$11.90\n\
Kickstand Case for iPhone 17 Pro Max, MagSafe Ring Stand\n4.8 out of 5 stars · 379 ratings\nS$33.90\n\
MagSafe Silicone Case for iPhone 17 Pro Max\n4.4 out of 5 stars · 2,048 ratings\nS$19.90\n\
Crossbody Lanyard Case for iPhone 17 Pro Max\n4.5 out of 5 stars · 154 ratings\nS$23.90\n\
Carbon Fibre Case for iPhone 17 Pro Max, Ultra Thin 0.6 mm\n4.5 out of 5 stars · 268 ratings\nS$18.90\n\
Slim Leather Case for iPhone 17 Pro Max, MagSafe Compatible\n4.6 out of 5 stars · 512 ratings\nS$24.90\n\
Aurora Glitter Case for iPhone 17 Pro Max, MagSafe Compatible\n4.7 out of 5 stars · 1,203 ratings\nS$21.90\n";

    #[test]
    fn a_results_page_gives_each_listing_with_its_rating_count_and_price() {
        let l = parse(PAGE);
        assert_eq!(l.len(), 9);
        let aurora = l.iter().find(|x| x.title.starts_with("Aurora")).unwrap();
        assert_eq!(aurora.stars, 4.7);
        assert_eq!(aurora.ratings, 1203);
        assert_eq!(aurora.price, Some(21.9));
        assert_eq!(aurora.price_text, "S$21.90");
        assert_eq!(aurora.rating_text, "4.7 out of 5 stars · 1,203 ratings");
    }

    #[test]
    fn best_rated_cases_under_a_price_with_enough_ratings_are_the_top_three() {
        let r = Ranking::from_json(
            &json!({"by": "rating", "max_price": "S$25", "min_ratings": 200,
            "title_has": ["case", "iPhone 17 Pro Max"], "count": 3}),
        )
        .unwrap();
        let titles: Vec<String> = r.apply(&parse(PAGE)).into_iter().map(|l| l.title).collect();
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
        let titles: Vec<String> = r.apply(&parse(PAGE)).into_iter().map(|l| l.title).collect();
        assert_eq!(
            titles[0],
            "Carbon Fibre Case for iPhone 17 Pro Max, Ultra Thin 0.6 mm"
        );
        assert_eq!(titles.len(), 2);
    }

    #[test]
    fn a_price_is_read_after_a_name() {
        let page = "THE BLOSSOM HIBIYA\n9.0 Superb\nfrom US$368.22 per night\nkaraksa hotel premier Tokyo Ginza\n9.0\nUS$365.57";
        assert_eq!(
            price_after(page, "The Blossom Hibiya").as_deref(),
            Some("US$368.22")
        );
        assert_eq!(
            price_after(page, "karaksa hotel premier Tokyo Ginza").as_deref(),
            Some("US$365.57")
        );
        assert_eq!(price_after(page, "Nowhere Inn"), None);
    }

    #[test]
    fn zeros_a_model_fills_in_are_not_criteria() {
        let r = Ranking::from_json(
            &json!({"by": "rating", "max_price": 0, "min_price": 0, "min_ratings": 0,
            "count": 0, "title_has": [], "title_not": []}),
        )
        .unwrap();
        assert_eq!((r.max_price, r.min_ratings, r.count), (None, None, None));
        assert_eq!(r.apply(&parse(PAGE)).len(), 3);
    }

    #[test]
    fn one_line_listings_and_snapshot_lines_are_read_too() {
        let l = parse(
            "[e4] link \"Trail Runner Shoe Lightweight Mesh\"\n4.3/5 (1.2k)\nRM 129.00\n\
Waterproof Hiking Boot Men 4.6 stars 312 reviews $89.50\n",
        );
        assert_eq!(l.len(), 2);
        assert_eq!(l[0].title, "Trail Runner Shoe Lightweight Mesh");
        assert_eq!(
            (l[0].stars, l[0].ratings, l[0].price),
            (4.3, 1200, Some(129.0))
        );
        assert_eq!(l[1].title, "Waterproof Hiking Boot Men");
        assert_eq!((l[1].ratings, l[1].price_text.as_str()), (312, "$89.50"));
    }
}
