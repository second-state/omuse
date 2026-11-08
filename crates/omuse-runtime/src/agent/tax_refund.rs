//! `tax_refund_rules{countries}`: the shopping tax refund for visitors (VAT, GST, consumption tax) in Asian countries,
//! from OMuse's checked table — who can claim, the minimum spend, the time limits, how it works at the shop and on the
//! way out, how the refund is paid and the official pages. Code answers from the table (never the model's memory); a
//! country without a scheme says so (Hong Kong, Macau: no sales tax; Malaysia: the scheme ended in 2018); a scheme in
//! law but not running yet says that (the Philippines). Checked against the official pages on 2026-11-04.

use serde_json::{json, Value};

use super::show as S;
use super::util::sget;

/// The date the table was checked against the official pages.
pub const CHECKED: &str = "2026-11-04";

/// One country's refund scheme for visitors.
pub struct Scheme {
    /// ISO 3166 code
    pub iso: &'static str,
    pub country: &'static str,
    /// the scheme's name, or why there is none
    pub scheme: &'static str,
    /// a refund visitors can claim today
    pub running: bool,
    pub who: &'static str,
    pub minimum: &'static str,
    pub time_limit: &'static str,
    pub how: &'static str,
    pub paid: &'static str,
    /// what the official pages do not confirm (said in the answer)
    pub unverified: &'static str,
    /// (label, official link)
    pub links: &'static [(&'static str, &'static str)],
}

pub static SCHEMES: &[Scheme] = &[
    Scheme {
        iso: "JP",
        country: "Japan",
        scheme: "tax-free shopping — a refund at departure for purchases from 1 November 2026 (until 31 October 2026 the tax was taken off at the till)",
        running: true,
        who: "visitors with a temporary-visitor status (most foreign tourists), passport shown at the shop",
        minimum: "¥5,000 before tax per shop per day (no upper limit in money; what you can carry out yourself)",
        time_limit: "the customs check within 90 days of purchase, before you check in your bags; then the goods leave Japan",
        how: "pay the full price with tax in the shop (passport scanned); on departure scan the passport at the tax-free kiosk in the departure lobby before check-in — or use Visit Japan Web at Narita, Haneda, Kansai, Chubu, Fukuoka, New Chitose and Naha; green = done, red = show the goods at customs (one missing item voids that receipt); goods used in Japan get no refund",
        paid: "by the shop or its refund operator: bank transfer, card, app or cash at the port, as the shop offers",
        unverified: "the operators' fees",
        links: &[
            ("Japan Tourism Agency — tax-free shopping (for travellers)", "https://www.mlit.go.jp/kankocho/tax-free/page01_000001_00028.html"),
            ("Japan Customs — tax refund at departure", "https://www.customs.go.jp/kaigairyoko/taxrefund.htm"),
        ],
    },
    Scheme {
        iso: "KR",
        country: "South Korea",
        scheme: "tax refund — immediate at the shop, or downtown, at the airport, by mailbox or mobile",
        running: true,
        who: "non-residents (foreigners staying under 6 months; overseas Koreans abroad 2+ years, in Korea 3 months or less); not diplomats, US forces or people employed in Korea",
        minimum: "KRW 15,000 per receipt; immediate refund at the shop for each payment under KRW 1,000,000 and up to KRW 5,000,000 per trip",
        time_limit: "the goods leave Korea within 3 months of purchase",
        how: "at departure show the passport, the refund vouchers and the goods (unopened, unused) at a customs kiosk or desk, then collect the refund after immigration at the refund counter or kiosk",
        paid: "cash (KRW, USD, CNY, JPY at Incheon's counter), card, Alipay or WeChat Pay at the kiosks",
        unverified: "the operators' fees; medical / cosmetic procedures losing the refund from 2026",
        links: &[
            ("VisitKorea — tax refund guide", "https://english.visitkorea.or.kr/svc/contents/contentsView.do?vcontsId=248765"),
            ("Korea Customs Service — tax refund", "https://www.customs.go.kr/english/cm/cntnts/cntntsView.do?mi=10803&cntntsId=5504"),
        ],
    },
    Scheme {
        iso: "CN",
        country: "Mainland China",
        scheme: "departure tax refund for overseas travellers, with refund-upon-purchase at many shops",
        running: true,
        who: "holders of a passport, or of a Mainland Travel Permit for Hong Kong / Macau / Taiwan residents, leaving within 183 days of the latest entry",
        minimum: "¥200 at one refund shop on one day",
        time_limit: "leave within 90 days of purchase (within 28 days for an instant refund at the shop)",
        how: "ask the shop for the refund (or the instant refund); at the port show the goods and the refund forms to customs (spot checks under ¥10,000, every item from ¥10,000), then the refund counter in the departure area",
        paid: "in RMB, cash or bank transfer up to ¥20,000 (transfer above); the refund is the VAT less a 2% fee — generally 9% or 6% of the invoice",
        unverified: "the list of excluded goods",
        links: &[
            ("State Taxation Administration — departure tax refund measures", "https://fgk.chinatax.gov.cn/eng/c101269/c5250095/content.html"),
            ("Shanghai government — departure tax refund", "https://english.shanghai.gov.cn/en-Individuals-ShoppingandTaxRefund-Departuretaxrefund/20260813/469b30ac4ff348b8ac3a1259f0e12d23.html"),
        ],
    },
    Scheme {
        iso: "TW",
        country: "Taiwan",
        scheme: "VAT refund for foreign travellers (Taiwan Tax Refund), 5% VAT",
        running: true,
        who: "foreign travellers who have stayed 183 days or less; ask for the refund form on the day of purchase",
        minimum: "NT$2,000 including VAT at one authorised shop on one day",
        time_limit: "the goods leave Taiwan within 90 days; the claim is approved before departure",
        how: "at the airport or port refund counter, an E-VAT kiosk or the TAIWAN TAX REFUND app before check-in; small amounts in the shop (up to NT$48,000 a day); downtown counters within 20 days before departure",
        paid: "as the counter offers; the operator keeps a fee (20% of the refund)",
        unverified: "the per-stay caps of the in-shop refund; the payout methods",
        links: &[
            ("Ministry of Finance — VAT refund for foreign travellers", "https://www.mof.gov.tw/eng/singlehtml/f48d641f159a4866b1d31c0916fbcc71?cntId=32bc7af09c4840d1a781991c4f753851"),
            ("Taiwan Tax Refund — questions and answers", "https://www.taxrefund.net.tw/ttr/ttr001e/articles?lang=en_US"),
        ],
    },
    Scheme {
        iso: "SG",
        country: "Singapore",
        scheme: "Tourist Refund Scheme (eTRS), GST",
        running: true,
        who: "visitors aged 16 or over, not Singapore citizens or PRs, not holding a work, dependant's or student's pass; the original passport shown at the shop",
        minimum: "S$100 including GST (up to 3 same-day receipts from the same retailer combined)",
        time_limit: "leave with the goods within 2 months of purchase; claim at the central refund counter within 2 months of approval",
        how: "at Changi or Seletar use the eTRS kiosk — before check-in for checked goods, after immigration for hand-carried or valuable goods; show the goods, invoices and boarding pass if asked; not for goods used in Singapore, services or hotels",
        paid: "to a card (within 10 days), Alipay (at once) or cash at Changi, less a handling fee",
        unverified: "the handling fee's amount",
        links: &[
            ("IRAS — Tourist Refund Scheme", "https://www.iras.gov.sg/taxes/goods-services-tax-(gst)/consumers/tourist-refund-scheme"),
            ("eTRS e-services", "https://touristrefund.sg/"),
        ],
    },
    Scheme {
        iso: "TH",
        country: "Thailand",
        scheme: "VAT Refund for Tourists (Revenue Department)",
        running: true,
        who: "non-Thai visitors staying 180 days or less, leaving by air from one of 10 international airports (Suvarnabhumi, Don Mueang, Chiang Mai, Phuket, Hat Yai, U-Tapao, Krabi, Samui, Surat Thani, Chiang Rai)",
        minimum: "฿2,000 including VAT per shop per day, with the P.P.10 (or e-P.P.10) form issued on the day",
        time_limit: "the goods leave within 60 days of purchase; claim on the departure day",
        how: "show the goods to customs before check-in when the purchases total ฿20,000 or more; luxury items of ฿40,000 or more each, and hand-carried goods of ฿100,000 or more each, are shown again at the refund office after immigration",
        paid: "up to ฿30,000 in cash (baht), Alipay or WeChat Pay; above that to a credit card; the refund follows the official table, net of fees (e.g. ฿500 back on ฿10,000)",
        unverified: "",
        links: &[
            ("Revenue Department — conditions", "https://vrtweb.rd.go.th/81.html"),
            ("Revenue Department — refund table", "https://vrtweb.rd.go.th/80.html"),
        ],
    },
    Scheme {
        iso: "VN",
        country: "Vietnam",
        scheme: "VAT refund for foreigners on departure (Circular 84/2026/TT-BTC, from 1 July 2026)",
        running: true,
        who: "foreigners and overseas Vietnamese with a foreign passport, leaving through an international airport or seaport refund gate",
        minimum: "check the official circular (a minimum spend per shop per day applies)",
        time_limit: "show the goods and the refund declaration to customs at least 30 minutes before departure",
        how: "the shop sends the invoice-cum-refund declaration electronically; at departure customs checks the goods, and a bank agent in the restricted area pays",
        paid: "by the bank agent in the departure area",
        unverified: "the minimum spend, the refunded share and the airports under the 2026 circular",
        links: &[
            ("Government legal portal — Circular 84/2026/TT-BTC", "https://vanban.chinhphu.vn/?pageid=27160&docid=218820&classid=1"),
        ],
    },
    Scheme {
        iso: "ID",
        country: "Indonesia",
        scheme: "VAT Refund for Tourists (Directorate General of Taxes)",
        running: true,
        who: "foreign passport holders, not Indonesian citizens or permanent residents, in Indonesia 60 days or less, leaving by air",
        minimum: "Rp500,000 of VAT (not of the price), each invoice carrying at least Rp50,000 of VAT, from TAX FREE SHOP retailers",
        time_limit: "purchases within 30 days before departure; one claim per visit, in person, on the departure day",
        how: "at Soekarno-Hatta, Ngurah Rai (Bali), Juanda, Kualanamu or Yogyakarta show the passport, boarding pass, the special tax invoices and the goods at the VAT refund counter",
        paid: "cash in rupiah up to Rp5,000,000 of VAT; above that by bank transfer within a month (transfer costs deducted)",
        unverified: "",
        links: &[
            ("DJP — VAT refund for tourists", "https://pajak.go.id/tax-refund-tourists"),
        ],
    },
    Scheme {
        iso: "PH",
        country: "Philippines",
        scheme: "VAT Refund for Non-Resident Tourists (RA 12079) — in law, not yet running",
        running: false,
        who: "non-resident foreign passport holders",
        minimum: "PHP 3,000 per purchase at an accredited shop (when the scheme starts)",
        time_limit: "the goods leave as accompanied baggage within 60 days of purchase (when the scheme starts)",
        how: "no refund operator or counter has been announced yet: there is nothing to claim at the airport today",
        paid: "",
        unverified: "the start date",
        links: &[
            ("Department of Finance — VAT refund for tourists", "https://www.dof.gov.ph/ph-is-now-vat-free-for-foreign-tourists/"),
        ],
    },
    Scheme {
        iso: "MY",
        country: "Malaysia",
        scheme: "no refund: the Tourist Refund Scheme ended with GST in 2018, and there is none under SST",
        running: false,
        who: "",
        minimum: "",
        time_limit: "",
        how: "duty-free shops are the only tax saving for visitors",
        paid: "",
        unverified: "",
        links: &[("Royal Malaysian Customs — travellers' guide", "https://www.customs.gov.my/en/individu/pengembara/travelers-guide")],
    },
    Scheme {
        iso: "HK",
        country: "Hong Kong",
        scheme: "no refund: Hong Kong has no sales tax or VAT (its airport departure tax refund is a different scheme)",
        running: false,
        who: "",
        minimum: "",
        time_limit: "",
        how: "",
        paid: "",
        unverified: "",
        links: &[("Brand Hong Kong — no sales tax", "https://www.brandhk.gov.hk/en/only-in-hong-kong/unique-advantages")],
    },
    Scheme {
        iso: "MO",
        country: "Macau",
        scheme: "no refund: Macau has no VAT or general sales tax",
        running: false,
        who: "",
        minimum: "",
        time_limit: "",
        how: "",
        paid: "",
        unverified: "",
        links: &[],
    },
];

/// The scheme of a country (ISO code, name or a city of the table).
pub fn scheme_of(place: &str) -> Option<&'static Scheme> {
    let iso = crate::places::country(place)
        .map(|c| c.iso)
        .or_else(|| Some(crate::places::iso_of(place)).filter(|c| !c.is_empty()))
        .or_else(|| crate::places::city_named(place).map(|c| c.country))?;
    SCHEMES.iter().find(|s| s.iso.eq_ignore_ascii_case(iso))
}

/// The tool's catalog entry.
pub fn schema() -> Value {
    super::tools::fn_(
        "tax_refund_rules",
        "The shopping tax refund for visitors (VAT / GST / consumption tax) in Asian countries, from OMuse's checked \
table: who can claim, the minimum spend, the time limits, how it works at the shop and on the way out, how the refund \
is paid, and the official links. Call it for any question about getting tax back on shopping; write the reply from the \
JSON result (code appends its show facts and links to the reply).",
        json!({"countries": {"type": "array", "items": {"type": "string"}, "description": "the countries (or cities) the user shops in"}}),
        &["countries"],
    )
}

/// `tax_refund_rules{countries}` → each country's scheme from the table, as JSON and show items.
pub fn tax_refund_rules(args: &Value) -> Value {
    let asked: Vec<String> = args["countries"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_else(|| vec![sget(args, "countries")]);
    let mut out = Vec::new();
    let mut show = Vec::new();
    let mut unknown = Vec::new();
    for place in asked.iter().filter(|p| !p.trim().is_empty()) {
        let Some(s) = scheme_of(place) else {
            unknown.push(place.clone());
            continue;
        };
        out.push(json!({"country": s.country, "scheme": s.scheme, "running": s.running, "who": s.who,
                        "minimum": s.minimum, "time_limit": s.time_limit, "how": s.how, "paid": s.paid,
                        "unverified": s.unverified,
                        "links": s.links.iter().map(|(l, u)| json!({"label": l, "url": u})).collect::<Vec<_>>()}));
        show.push(S::fact(s.country, s.country, s.scheme));
        for (label, v) in [
            ("Who", s.who),
            ("Minimum", s.minimum),
            ("Time limit", s.time_limit),
            ("How", s.how),
            ("Refund paid", s.paid),
        ] {
            if !v.is_empty() {
                show.push(S::fact(
                    &format!("{} — {label}", s.country),
                    &format!("{} — {label}", s.country),
                    v,
                ));
            }
        }
        if !s.unverified.is_empty() {
            show.push(S::fact(
                &format!("{} — not confirmed officially", s.country),
                &format!("{} — 未经官方确认", s.country),
                s.unverified,
            ));
        }
        for (label, url) in s.links {
            show.push(S::link(label, label, s.country, url));
        }
    }
    show.push(S::fact(
        "Checked",
        "核对日期",
        &format!("against the official pages on {CHECKED}"),
    ));
    json!({"countries": out, "unknown": unknown, "checked": CHECKED, "show": show,
           "hints": ["the requirements and the official links from the table; a country it does not have: say so and point to its tax authority"]})
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Each city guide's "Arrival card and tax" section carries its country's arrival card (the official site of
    /// `driver::CARDS`) and its tax refund's official links from this table, as written by the guides' generator (`tests/city_guides_border.py`)
    /// (the user, 2026-11-07: the requirement and the link in every city guide). A table change fails here until the
    /// guides are written again.
    #[test]
    fn every_city_guide_has_its_countrys_arrival_card_and_tax_refund_from_the_tables() {
        let skills = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../skills");
        let cities = [
            ("bangkok", "TH"),
            ("beijing", "CN"),
            ("chengdu", "CN"),
            ("guangzhou", "CN"),
            ("hanoi", "VN"),
            ("ho-chi-minh-city", "VN"),
            ("hong-kong", "HK"),
            ("kuala-lumpur", "MY"),
            ("osaka", "JP"),
            ("seoul", "KR"),
            ("shanghai", "CN"),
            ("shenzhen", "CN"),
            ("singapore", "SG"),
            ("taipei", "TW"),
            ("tokyo", "JP"),
        ];
        for (city, iso) in cities {
            let text = std::fs::read_to_string(skills.join(city).join("SKILL.md")).unwrap();
            let sec = text
                .split("## Arrival card and tax")
                .nth(1)
                .unwrap_or_else(|| panic!("{city}: no Arrival card and tax section"))
                .split("\n## ")
                .next()
                .unwrap()
                .to_string();
            let country = crate::places::country(iso).map(|c| c.name).unwrap_or("");
            match super::super::driver::CARDS
                .iter()
                .find(|c| crate::places::iso_of(c.country) == iso || c.country == country)
            {
                Some(card) => assert!(
                    sec.contains(card.start_url),
                    "{city}: {} missing",
                    card.start_url
                ),
                None => assert!(sec.contains("none to file"), "{city}: says no card"),
            }
            let s = scheme_of(iso).unwrap_or_else(|| panic!("{city}: no scheme for {iso}"));
            for (_, url) in s.links.iter().take(2) {
                assert!(sec.contains(url), "{city}: {url} missing");
            }
        }
    }

    #[test]
    fn the_table_answers_by_country_or_city_with_its_official_links() {
        let r = tax_refund_rules(&json!({"countries": ["Tokyo", "KR", "Malaysia", "Atlantis"]}));
        let c = r["countries"].as_array().unwrap();
        assert_eq!(c.len(), 3);
        assert_eq!(c[0]["country"], "Japan");
        assert!(c[0]["scheme"].as_str().unwrap().contains("1 November 2026"));
        assert_eq!(c[2]["running"], false);
        assert_eq!(r["unknown"], json!(["Atlantis"]));
        let lines = super::super::show::lines(&r, false).join("\n");
        assert!(
            lines.contains("https://www.customs.go.jp/kaigairyoko/taxrefund.htm"),
            "{lines}"
        );
        assert!(lines.contains("ended with GST in 2018"), "{lines}");
        // every link of the table is an https / http official address
        for s in SCHEMES {
            for (_, u) in s.links {
                assert!(u.starts_with("https://"), "{u}");
            }
        }
    }
}
