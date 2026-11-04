//! `payment_options{countries}`: how a foreign visitor (a foreign passport, a foreign Visa / Mastercard, no local bank
//! account) pays in Asian countries, from OMuse's checked table — the ways to pay in order of what works best, what each
//! needs, its fees and limits where an official page states them, and the official sign-up / download links. Code
//! answers from the table (never the model's memory); what the official pages do not confirm is said as such.
//! Every link was opened and checked on 2026-11-04.

use serde_json::{json, Value};

use super::show as S;
use super::util::sget;

/// The date the table was checked against the official pages.
pub const CHECKED: &str = "2026-11-04";

/// One way to pay.
pub struct Method {
    pub name: &'static str,
    /// what a visitor needs to use it, and how it works
    pub how: &'static str,
    /// fees and limits from the official pages ("" when none are published)
    pub fees: &'static str,
    /// what the official pages do not confirm
    pub unverified: &'static str,
    /// (label, official link)
    pub links: &'static [(&'static str, &'static str)],
}

/// One country's ways to pay, best first.
pub struct Country {
    /// ISO 3166 code
    pub iso: &'static str,
    pub country: &'static str,
    /// the one-line advice
    pub summary: &'static str,
    pub methods: &'static [Method],
    /// (label, official guide)
    pub guides: &'static [(&'static str, &'static str)],
}

const ALIPAY_PLUS: (&str, &str) = (
    "Alipay+ (home e-wallets abroad)",
    "https://www.alipayplus.com/",
);

pub static COUNTRIES: &[Country] = &[
    Country {
        iso: "CN",
        country: "Mainland China",
        summary: "set up Alipay and WeChat Pay with your foreign card before you fly — QR codes are how most of China pays; carry some cash and a card as back-up (shops must accept cash)",
        methods: &[
            Method {
                name: "Alipay with a foreign card",
                how: "download the app, sign up with your foreign phone number or email, link a Visa, Mastercard, JCB, Diners, Discover, UnionPay or Amex card; then scan the shop's QR code or show yours; the app asks for your passport once spending passes about US$2,000 a year",
                fees: "a 3% fee on each card payment over ¥200 (none at ¥200 or less), plus your bank's foreign-exchange charge; limits US$5,000 per payment and US$50,000 a year after the passport check (set by the PBOC in March 2024)",
                unverified: "Alipay publishes no fee or limit page of its own; travel blogs' ¥ figures are not confirmed",
                links: &[
                    ("Alipay for iPhone (App Store)", "https://apps.apple.com/us/app/alipay-simplify-your-life/id333206289"),
                    ("Alipay for Android (Google Play)", "https://play.google.com/store/apps/details?id=com.eg.android.AlipayGphone"),
                ],
            },
            Method {
                name: "WeChat Pay (Weixin Pay) with a foreign card",
                how: "download WeChat, sign up with your non-Chinese phone number, then Me → Services → Wallet → add a Visa, Mastercard, Amex, JCB, Diners, Discover or UnionPay card; a passport check raises the limits",
                fees: "no 3% fee on payments of ¥200 or less; a first-time card gets no fee on daily spending up to ¥1,000 for 90 days (Tencent, May 2026); limits US$5,000 per payment and US$50,000 a year after the passport check",
                unverified: "WeChat publishes no limit page of its own",
                links: &[
                    ("WeChat for iPhone (App Store)", "https://apps.apple.com/us/app/wechat/id414478124"),
                    ("WeChat for Android (Google Play)", "https://play.google.com/store/apps/details?id=com.tencent.mm"),
                    ("Tencent — paying with Weixin Pay as a visitor", "https://www.tencent.com/welcome-to-china-easy-pay-with-weixin-pay/"),
                ],
            },
            Method {
                name: "Your home e-wallet at Chinese QR codes",
                how: "AlipayHK, GCash, Kakao Pay, TrueMoney, Touch 'n Go and other Alipay+ wallets scan Chinese merchant QR codes directly — no new app",
                fees: "",
                unverified: "",
                links: &[ALIPAY_PLUS],
            },
            Method {
                name: "Nihao China (UnionPay's visitor app)",
                how: "sign up by email or phone; a prepaid Tour Wallet / Tour Card topped up with a foreign card; metro QR in 43 cities and buses in more than 1,760 counties",
                fees: "",
                unverified: "the Tour Wallet's fees and limits are not on an official page",
                links: &[
                    ("Nihao China (UnionPay International)", "https://www.unionpayintl.com/ZT/en/NHCAPP/"),
                    ("Nihao China for iPhone (App Store)", "https://apps.apple.com/us/app/nihao-china/id1187814659"),
                ],
            },
            Method {
                name: "A foreign card at the till",
                how: "accepted where the Visa / Mastercard logo is shown; hotels of 3 stars and up, 4A/5A attractions and national resorts must accept foreign cards (PBOC notice, April 2024)",
                fees: "your bank's foreign-transaction fee",
                unverified: "",
                links: &[("SAFE — card acceptance for visitors (April 2024)", "https://www.safe.gov.cn/en/2024/0409/2198.html")],
            },
            Method {
                name: "Cash (RMB)",
                how: "exchange at the airport, banks or hotels, or withdraw at ATMs that take international cards; since 1 February 2026 staffed shops may not refuse cash and must give change",
                fees: "the ATM's and your bank's charges",
                unverified: "",
                links: &[("The RMB cash rule (in force 1 February 2026)", "https://www.gov.cn/zhengce/zhengceku/202512/content_7055739.htm")],
            },
            Method {
                name: "Shanghai Metro and taxis",
                how: "Shanghai Metro: tap a contactless UnionPay, Visa, Mastercard, Amex or JCB card at every line's gate (turn on small payments without a PIN); other cities: Alipay's Transport QR code; DiDi takes foreign numbers and foreign cards in English",
                fees: "",
                unverified: "",
                links: &[
                    ("Shanghai — tap-to-ride with foreign cards", "https://english.shanghai.gov.cn/en-Latest-WhatsNew/20250627/d8f21f2e7d3b45a5b03757e35039b806.html"),
                    ("DiDi for iPhone (App Store)", "https://apps.apple.com/us/app/didi-china-ride-hailing/id554499054"),
                    ("DiDi for Android (Google Play)", "https://play.google.com/store/apps/details?id=com.sdu.didi.psnger"),
                ],
            },
        ],
        guides: &[
            ("Guide to Payment Services in China (PCAC, July 2026, PDF)", "https://perth.china-consulate.gov.cn/eng/zytz/202607/P020260703756386489119.pdf"),
            ("Beijing — how to pay in Beijing", "https://english.beijing.gov.cn/specials/paymentservices/howtopaybeijing/"),
        ],
    },
    Country {
        iso: "JP",
        country: "Japan",
        summary: "cards are taken widely and Tokyo's trains take a tap of your contactless card; Welcome Suica (iPhone) or a TOURIST PASMO covers the rest; you cannot open PayPay without a Japanese number, but many home wallets pay at PayPay shops",
        methods: &[
            Method {
                name: "Tap a contactless card on Tokyo's trains",
                how: "since 25 March 2026, 11 Kanto operators (Tokyo Metro, Toei, Odakyu, Keio, Keikyu, Tokyu, Seibu, Tobu, Sotetsu and others) take Visa, Mastercard, JCB, Amex, Diners, Discover and UnionPay at 729 stations; adult fares only",
                fees: "",
                unverified: "JR lines and the Shinkansen are not part of it",
                links: &[
                    ("Kanto tap-to-ride (Keio release, March 2026, PDF)", "https://www.keio.co.jp/news/update/news_release/news_release2025/pdf/nr20260324_creditcardtouch_english.pdf"),
                    ("Q-move — where contactless cards ride", "https://q-move.info/"),
                ],
            },
            Method {
                name: "Welcome Suica Mobile (iPhone)",
                how: "a transit and shop IC card in Apple Wallet for visitors; top up with your foreign card in Apple Pay; valid 180 days",
                fees: "the balance left is not refunded",
                unverified: "",
                links: &[("Welcome Suica Mobile (App Store)", "https://apps.apple.com/us/app/welcome-suica-mobile/id6738336566")],
            },
            Method {
                name: "TOURIST PASMO card",
                how: "a visitor IC card for trains, buses and shops, sold at Haneda and Narita; valid 28 days (a Welcome Suica card is the JR East equivalent)",
                fees: "no deposit; the balance left is not refunded",
                unverified: "",
                links: &[("TOURIST PASMO", "https://www.pasmo.co.jp/en/tourist-pasmo/")],
            },
            Method {
                name: "Your home e-wallet at PayPay shops",
                how: "36 overseas wallets from 17 countries and regions (Alipay+ wallets, the UnionPay app's QR) pay at PayPay merchants; a PayPay account itself needs a Japanese mobile number",
                fees: "",
                unverified: "",
                links: &[
                    ("PayPay — overseas wallets accepted (September 2026)", "https://about.paypay.ne.jp/en/pr/20260903/01/"),
                    ("PayPay — why a Japanese number is needed", "https://paypay.ne.jp/help/c0032/"),
                ],
            },
        ],
        guides: &[
            ("JNTO — cashless payments in Japan", "https://www.japan.travel/en/plan/cashless-payments-in-japan/"),
            ("JNTO — IC cards", "https://www.japan.travel/en/plan/ic-card/"),
        ],
    },
    Country {
        iso: "KR",
        country: "South Korea",
        summary: "cards work almost everywhere; for transit and small shops get a WOWPASS or NAMANE prepaid card (T-money built in); Seoul's ticket machines take foreign cards",
        methods: &[
            Method {
                name: "WOWPASS prepaid card",
                how: "get it at a WOWPASS kiosk with your passport; load foreign cash at the kiosk (or KRW cash out); T-money is built in for buses and the subway",
                fees: "the card costs ₩5,000",
                unverified: "topping it up with a foreign card",
                links: &[
                    ("WOWPASS", "https://wowpass.io/en"),
                    ("WOWPASS for iPhone (App Store)", "https://apps.apple.com/us/app/wowpass-go-cashless-in-korea/id1626691440"),
                    ("WOWPASS for Android (Google Play)", "https://play.google.com/store/apps/details?id=com.wow.wowpass"),
                ],
            },
            Method {
                name: "NAMANE prepaid card",
                how: "top up in the app with a credit card or an Alipay+ wallet, at CU stores with cash, or at kiosks; transit included (the site is namanecard.com — not namane.co.kr)",
                fees: "the card costs ₩7,000",
                unverified: "",
                links: &[
                    ("NAMANE", "https://namanecard.com/en"),
                    ("NAMANE for iPhone (App Store)", "https://apps.apple.com/us/app/namane/id1533784467"),
                    ("NAMANE for Android (Google Play)", "https://play.google.com/store/apps/details?id=com.iaurora.cardforme"),
                ],
            },
            Method {
                name: "Seoul subway: tickets with a foreign card, or a Climate Card pass",
                how: "since 17 March 2026, 440 ticket machines at 273 stations take overseas cards; the short-term Climate Card gives unlimited Seoul transit for 1, 2, 3, 5 or 7 days (₩5,000 to ₩20,000, plus the card)",
                fees: "",
                unverified: "tapping a foreign card at the gates",
                links: &[
                    ("Seoul — overseas cards at ticket machines (korea.net)", "https://www.korea.net/NewsFocus/Society/view?articleId=289001"),
                    ("Seoul — Climate Card", "https://english.seoul.go.kr/policy/transportation/climate-card/"),
                ],
            },
        ],
        guides: &[(
            "VisitKorea — transport cards",
            "https://english.visitkorea.or.kr/svc/contents/contentsView.do?vcontsId=140663",
        )],
    },
    Country {
        iso: "TW",
        country: "Taiwan",
        summary: "credit cards are widely taken; Taipei Metro's gates take a tap of your contactless card or phone wallet; an EasyCard or iPASS covers buses and convenience stores",
        methods: &[
            Method {
                name: "Tap a card or phone at Taipei Metro",
                how: "Visa, Mastercard, JCB, UnionPay, Diners and Discover contactless cards, and Apple Pay, Google Pay and Samsung Pay, at the gates",
                fees: "",
                unverified: "",
                links: &[("Taipei Metro — contactless payment", "https://english.metro.taipei/cp.aspx?n=BECC2E7AC426F659")],
            },
            Method {
                name: "EasyCard or iPASS",
                how: "stored-value cards for the MRT, buses, YouBike and convenience stores; buy and top up with cash at stations and stores",
                fees: "",
                unverified: "the card prices and balance limits",
                links: &[("EasyCard", "https://www.easycard.com.tw/en/"), ("iPASS", "https://www.i-pass.com.tw/")],
            },
        ],
        guides: &[("Taiwan Tourism Administration — money", "https://eng.taiwan.net.tw/m1.aspx?sNo=0002064")],
    },
    Country {
        iso: "HK",
        country: "Hong Kong",
        summary: "tap your Visa, Mastercard or UnionPay card on the MTR; Octopus for Tourists on an iPhone pays for buses, ferries and shops",
        methods: &[
            Method {
                name: "Tap a contactless card on the MTR",
                how: "Visa, Mastercard and UnionPay credit cards at the gates; adult fares; not on the Airport Express",
                fees: "",
                unverified: "",
                links: &[("MTR — contactless cards (August 2024, PDF)", "https://www.mtr.com.hk/archive/corporate/en/press_release/PR-24-050-E.pdf")],
            },
            Method {
                name: "Octopus for Tourists (iPhone)",
                how: "an Octopus card in Apple Wallet, topped up with a foreign Visa, Mastercard or UnionPay card in the app; buses, ferries, the MTR, convenience stores and many shops",
                fees: "HK$50 deposit, HK$3,000 maximum balance, HK$11 fee if cancelled within 90 days; refunds take 6–8 weeks",
                unverified: "",
                links: &[
                    ("Octopus for Tourists", "https://www.octopus.com.hk/en/consumer/tourist/apple-pay/index.html"),
                    ("Octopus for Tourists (App Store)", "https://apps.apple.com/us/app/octopus-for-tourists/id1505492952"),
                ],
            },
            Method {
                name: "Tourist Octopus card",
                how: "a physical Octopus card for visitors, topped up with cash at MTR stations and convenience stores",
                fees: "HK$39, with no stored value included",
                unverified: "",
                links: &[("Tourist Octopus", "https://www.octopus.com.hk/en/consumer/octopus-cards/products/sold-tourist/index.html")],
            },
        ],
        guides: &[("Octopus — choices for visitors", "https://www.octopus.com.hk/en/consumer/tourist/choices/index.html")],
    },
    Country {
        iso: "MO",
        country: "Macau",
        summary: "carry cash (patacas; Hong Kong dollars are widely taken); cards work at hotels and casinos; a Macau Pass card for buses and the LRT",
        methods: &[
            Method {
                name: "Cash and cards",
                how: "MOP cash (HKD widely accepted, change given in MOP); cards at hotels, casinos and larger shops",
                fees: "",
                unverified: "how widely smaller shops take foreign cards",
                links: &[("Macao Government Tourism Office", "https://www.macaotourism.gov.mo/en/")],
            },
            Method {
                name: "Macau Pass card",
                how: "a stored-value card for buses, the LRT and convenience stores, bought and topped up with cash",
                fees: "",
                unverified: "the card's price and deposit",
                links: &[("Macau Pass", "https://www.macaupass.com/")],
            },
        ],
        guides: &[("DSAT — Macau transport", "https://www.dsat.gov.mo/")],
    },
    Country {
        iso: "SG",
        country: "Singapore",
        summary: "tap your contactless card or phone on the MRT and buses (SimplyGo); cards everywhere else; Grab takes foreign cards",
        methods: &[
            Method {
                name: "SimplyGo: tap a contactless card or phone",
                how: "foreign Visa, Mastercard and Amex contactless cards, Apple Pay and Google Pay on the MRT, LRT and buses",
                fees: "a S$0.60 foreign card admin fee per day of use",
                unverified: "",
                links: &[("SimplyGo — contactless bank cards", "https://www.simplygo.com.sg/faqs/cards-and-charms/simplygo/contactless-bank-cards/")],
            },
            Method {
                name: "Singapore Tourist Pass",
                how: "unlimited MRT and basic bus rides",
                fees: "S$17 (1 day), S$24 (2 days), S$29 (3 days)",
                unverified: "",
                links: &[
                    ("Singapore Tourist Pass", "https://thesingaporetouristpass.com.sg/type-of-passes/"),
                    ("Where to buy the Tourist Pass", "https://thesingaporetouristpass.com.sg/where-to-buy/"),
                ],
            },
            Method {
                name: "Grab",
                how: "rides and food with a foreign card in the app",
                fees: "",
                unverified: "",
                links: &[
                    ("Grab for iPhone (App Store)", "https://apps.apple.com/us/app/grab-taxi-ride-food-delivery/id647268330"),
                    ("Grab for Android (Google Play)", "https://play.google.com/store/apps/details?id=com.grabtaxi.passenger"),
                ],
            },
        ],
        guides: &[("LTA — travel smart", "https://www.lta.gov.sg/travelsmart")],
    },
    Country {
        iso: "TH",
        country: "Thailand",
        summary: "cards at hotels and malls, cash at markets and street stalls; a TAGTHAi Easy Pay wallet pays any Thai QR code; Bangkok's MRT takes a tap of a foreign credit card",
        methods: &[
            Method {
                name: "TAGTHAi Easy Pay (KBank's visitor wallet)",
                how: "pays any Thai PromptPay QR code; sign up with your passport (age 20+), activate and top up with cash at a KBank exchange booth",
                fees: "up to THB 25,000 per payment and per top-up, THB 50,000 a month",
                unverified: "",
                links: &[("TAGTHAi — paying in Thailand as a visitor", "https://www.tagthai.com/explore/p/payment-for-foreign-tourist-in-thailand")],
            },
            Method {
                name: "Tap a card on Bangkok's MRT",
                how: "Blue and Purple lines: international Visa, Mastercard and UnionPay credit cards; Yellow line: international credit and debit cards",
                fees: "",
                unverified: "the BTS Skytrain does not take a card tap (use a Rabbit card)",
                links: &[
                    ("MRT Blue / Purple — EMV contactless", "https://metro.bemplc.co.th/EMV-Contactless?lang=en"),
                    ("MRT Yellow — EMV tickets", "https://ebm.co.th/en/ticket-emv/"),
                    ("Rabbit card (BTS)", "https://www.rabbit.co.th/"),
                ],
            },
            Method {
                name: "Your Chinese wallet at Thai QR codes",
                how: "Alipay, UnionPay and WeChat Pay users can scan Thai QR codes (Bank of Thailand, October 2025)",
                fees: "",
                unverified: "",
                links: &[("Bank of Thailand — visitors' QR payments", "https://www.bot.or.th/en/news-and-media/news/news-20251030.html")],
            },
        ],
        guides: &[],
    },
    Country {
        iso: "MY",
        country: "Malaysia",
        summary: "cards in malls and hotels; Touch 'n Go eWallet's tourist sign-up pays DuitNow QR codes; a Touch 'n Go card for Rapid KL",
        methods: &[
            Method {
                name: "Touch 'n Go eWallet (tourist sign-up)",
                how: "sign up with your phone number (code by WhatsApp or SMS), a passport valid 6+ months and a selfie; reload by card; pays DuitNow QR codes",
                fees: "minimum reload RM20; a convenience fee on card reloads; no cash withdrawal",
                unverified: "the fee rate on a foreign card",
                links: &[
                    ("Touch 'n Go eWallet for iPhone (App Store)", "https://apps.apple.com/app/id1344696702"),
                    ("Touch 'n Go eWallet for Android (Google Play)", "https://play.google.com/store/apps/details?id=my.com.tngdigital.ewallet"),
                ],
            },
            Method {
                name: "Touch 'n Go card",
                how: "still needed to tap in on Rapid KL trains and buses (a card tap is not live yet); top up with cash at stations and stores",
                fees: "",
                unverified: "",
                links: &[("Touch 'n Go", "https://www.touchngo.com.my/")],
            },
            Method {
                name: "Your home e-wallet at DuitNow QR codes",
                how: "linked wallets from abroad scan Malaysian DuitNow QR codes",
                fees: "",
                unverified: "which countries' wallets are linked",
                links: &[("PayNet (DuitNow)", "https://www.paynet.my/")],
            },
        ],
        guides: &[],
    },
    Country {
        iso: "VN",
        country: "Vietnam",
        summary: "carry cash (VND) for small shops and street food; cards at hotels and malls; Grab with a foreign card; Visa contactless on the metro",
        methods: &[
            Method {
                name: "Visa contactless on the metro",
                how: "Hanoi metro lines 2A and 3.1 take a tap of a Visa card since 23 March 2026",
                fees: "",
                unverified: "",
                links: &[("Visa — Hanoi metro open-loop payments", "https://www.visa.com.vn/en_VN/about-visa/newsroom/press-releases/visa-and-hanoi-metro-mark-a-new-milestone-in-urban-mobility-with-open-loop-payments-across-the-capitals-metro-network.html")],
            },
            Method {
                name: "Your home e-wallet at VietQR codes",
                how: "Weixin Pay (NAPAS, August 2026) and Singapore's PayNow-linked banks pay Vietnamese QR codes",
                fees: "",
                unverified: "Alipay at VietQR",
                links: &[
                    ("NAPAS — Weixin Pay at VietQR", "https://en.napas.com.vn/napas-bidv-and-weixin-pay-partner-to-expand-qr-payment-services-between-vietnam-and-china-184260820090000496.htm"),
                    ("State Bank of Vietnam — Vietnam–Singapore QR", "https://sbv.gov.vn/en/w/official-launch-of-the-vietnam-singapore-cross-border-qr-payment-service"),
                ],
            },
            Method {
                name: "Grab",
                how: "rides and food with a foreign card in the app",
                fees: "",
                unverified: "",
                links: &[
                    ("Grab for iPhone (App Store)", "https://apps.apple.com/us/app/grab-taxi-ride-food-delivery/id647268330"),
                    ("Grab for Android (Google Play)", "https://play.google.com/store/apps/details?id=com.grabtaxi.passenger"),
                ],
            },
        ],
        guides: &[],
    },
    Country {
        iso: "ID",
        country: "Indonesia",
        summary: "cash (rupiah) for most small payments, cards at hotels and malls; your home app may pay QRIS codes; MRT Jakarta takes a tap of a foreign card",
        methods: &[
            Method {
                name: "Your home app at QRIS codes",
                how: "linked apps from Thailand, Malaysia, Singapore and other countries scan Indonesian QRIS codes",
                fees: "up to Rp10 million per payment; merchants pay no extra fee",
                unverified: "",
                links: &[("Bank Indonesia — QRIS", "https://www.bi.go.id/en/fungsi-utama/sistem-pembayaran/ritel/kanal-layanan/qris/default.aspx")],
            },
            Method {
                name: "Tap a card on MRT Jakarta",
                how: "foreign Visa or Mastercard contactless cards at the gates; fares charged daily",
                fees: "Rp14,000 if you forget to tap out",
                unverified: "",
                links: &[("Bank Mandiri — EMV transit", "https://www.bankmandiri.co.id/en/emv-transit")],
            },
            Method {
                name: "e-money card for Transjakarta and KRL trains",
                how: "a bank's stored-value card, bought and topped up at stations and convenience stores",
                fees: "",
                unverified: "the card prices",
                links: &[("Transjakarta", "https://transjakarta.co.id/")],
            },
        ],
        guides: &[],
    },
    Country {
        iso: "PH",
        country: "Philippines",
        summary: "cash (pesos) for most small payments, cards at hotels and malls; a beep card for Manila's trains; GCash's GTourist only with a US number",
        methods: &[
            Method {
                name: "beep card",
                how: "a stored-value card for Manila's LRT, MRT and some buses, bought and topped up with cash at stations",
                fees: "",
                unverified: "",
                links: &[("beep", "https://beep.com.ph/")],
            },
            Method {
                name: "Mastercard contactless on MRT-3",
                how: "tap a Mastercard at MRT-3 gates since July 2025",
                fees: "",
                unverified: "whether foreign-issued cards are accepted",
                links: &[("Mastercard — tap & go at MRT-3", "https://newsroom.mastercard.com/news/ap/en/newsroom/press-releases/en/2025/commuters-can-now-use-their-mastercard-cards-to-tap-go-at-mrt-3-stations/")],
            },
            Method {
                name: "GCash GTourist",
                how: "a visitor GCash account for US (+1) numbers: passport and selfie, activated in the Philippines, valid 30 days",
                fees: "",
                unverified: "the terms (GCash's help centre could not be checked)",
                links: &[("GCash for iPhone (App Store)", "https://apps.apple.com/app/id520020791")],
            },
        ],
        guides: &[],
    },
];

/// The ways to pay in a country (ISO code, name or a city of the table).
pub fn country_of(place: &str) -> Option<&'static Country> {
    let iso = crate::places::country(place)
        .map(|c| c.iso)
        .or_else(|| Some(crate::places::iso_of(place)).filter(|c| !c.is_empty()))
        .or_else(|| crate::places::city_named(place).map(|c| c.country))?;
    COUNTRIES.iter().find(|c| c.iso.eq_ignore_ascii_case(iso))
}

/// The tool's catalog entry.
pub fn schema() -> Value {
    super::tools::fn_(
        "payment_options",
        "How a foreign visitor pays in Asian countries (mainland China, Japan, Korea, Taiwan, Hong Kong, Macau, \
Singapore, Thailand, Malaysia, Vietnam, Indonesia, the Philippines), from OMuse's checked table: the ways to pay best \
first (apps such as Alipay and WeChat Pay with a foreign card, visitor transit cards, contactless cards, cash), what \
each needs, the fees and limits the official pages state, and the official sign-up and download links. Call it for any \
question about paying, cash, cards or payment apps in a country; write the reply from the JSON result (code appends \
its show facts and links to the reply).",
        json!({"countries": {"type": "array", "items": {"type": "string"}, "description": "the countries (or cities) the user will pay in"}}),
        &["countries"],
    )
}

/// `payment_options{countries}` → each country's ways to pay from the table, as JSON and show items.
pub fn payment_options(args: &Value) -> Value {
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
        let Some(c) = country_of(place) else {
            unknown.push(place.clone());
            continue;
        };
        if out.iter().any(|o: &Value| o["country"] == c.country) {
            continue;
        }
        let links = |l: &[(&str, &str)]| {
            l.iter()
                .map(|(label, url)| json!({"label": label, "url": url}))
                .collect::<Vec<_>>()
        };
        out.push(json!({"country": c.country, "summary": c.summary,
            "methods": c.methods.iter().map(|m| json!({"name": m.name, "how": m.how, "fees": m.fees,
                "unverified": m.unverified, "links": links(m.links)})).collect::<Vec<_>>(),
            "guides": links(c.guides)}));
        show.push(S::fact(c.country, c.country, c.summary));
        for m in c.methods {
            let mut text = m.how.to_string();
            if !m.fees.is_empty() {
                text.push_str(&format!(" — fees and limits: {}", m.fees));
            }
            if !m.unverified.is_empty() {
                text.push_str(&format!(" — not confirmed officially: {}", m.unverified));
            }
            let label = format!("{} — {}", c.country, m.name);
            show.push(S::fact(&label, &label, &text));
            for (l, url) in m.links {
                show.push(S::link(l, l, c.country, url));
            }
        }
        for (l, url) in c.guides {
            show.push(S::link(l, l, c.country, url));
        }
    }
    show.push(S::fact(
        "Checked",
        "核对日期",
        &format!("against the official pages on {CHECKED}"),
    ));
    json!({"countries": out, "unknown": unknown, "checked": CHECKED, "show": show,
           "hints": ["the ways to pay best first and the official links from the table; a country it does not have: say so and suggest a card plus some cash"]})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn china_gets_alipay_and_wechat_pay_with_their_store_links_and_the_fee_rule() {
        let r = payment_options(&json!({"countries": ["Shanghai", "China", "JP", "Atlantis"]}));
        let c = r["countries"].as_array().unwrap();
        assert_eq!(c.len(), 2, "Shanghai and China are one country: {c:?}");
        assert_eq!(c[0]["country"], "Mainland China");
        assert!(c[0]["methods"][0]["name"]
            .as_str()
            .unwrap()
            .starts_with("Alipay"));
        assert!(c[0]["methods"][1]["name"]
            .as_str()
            .unwrap()
            .starts_with("WeChat Pay"));
        assert_eq!(c[1]["country"], "Japan");
        assert_eq!(r["unknown"], json!(["Atlantis"]));
        let lines = super::super::show::lines(&r, false).join("\n");
        for want in [
            "https://apps.apple.com/us/app/alipay-simplify-your-life/id333206289",
            "https://play.google.com/store/apps/details?id=com.tencent.mm",
            "3% fee on each card payment over ¥200",
            "https://apps.apple.com/us/app/welcome-suica-mobile/id6738336566",
            "2026-11-04",
        ] {
            assert!(lines.contains(want), "{want} missing:\n{lines}");
        }
    }

    #[test]
    fn every_country_of_the_table_is_found_and_has_official_links() {
        for c in COUNTRIES {
            assert!(
                country_of(c.iso).is_some_and(|f| f.iso == c.iso),
                "{}",
                c.iso
            );
            assert!(!c.methods.is_empty(), "{}", c.iso);
            for m in c.methods {
                assert!(!m.links.is_empty(), "{} {}", c.iso, m.name);
                for (_, u) in m.links.iter().chain(c.guides) {
                    assert!(u.starts_with("https://"), "{u}");
                }
            }
        }
    }
}
