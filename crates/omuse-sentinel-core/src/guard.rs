//! The parts of `app/sentinel/guard.py` the policy engine needs: data classes, URL / network safety and the
//! risky-UI-word heuristics (click / money / checkout / order-change / search / filter detection, page totals,
//! card-box matching, "the user asked for it" checks).
//!
//! `omuse_common::guard` is the designated home of the full guard port (secret redaction, injection scan, …);
//! while it is still a stub this module carries byte-identical copies of the functions policy depends on, so
//! that the policy engine and its tests work on their own. Once `omuse_common::guard` is complete, `policy.rs`
//! and `store.rs` can switch their `use crate::guard` to `use omuse_common::guard` and this file can go.

use std::net::IpAddr;
use std::sync::LazyLock;

use regex::Regex;
use serde_json::Value;

use omuse_common::util::{get_str, take_chars, truthy};

// ---------------------------------------------------------------- data classes
pub const PUBLIC: &str = "PUBLIC";
pub const PERSONAL: &str = "PERSONAL";
pub const CONFIDENTIAL: &str = "CONFIDENTIAL";
pub const SECRET: &str = "SECRET";

/// `LEVELS.get(name, default)`.
pub fn level_or(name: &str, default: i64) -> i64 {
    match name {
        PUBLIC => 0,
        PERSONAL => 1,
        CONFIDENTIAL => 2,
        SECRET => 3,
        _ => default,
    }
}

/// `LEVELS.get(name, 0)`.
pub fn level(name: &str) -> i64 {
    level_or(name, 0)
}

/// The higher of two data classes (`a` on a tie).
pub fn max_class(a: &str, b: &str) -> String {
    if level(a) >= level(b) {
        a.to_string()
    } else {
        b.to_string()
    }
}

// ---------------------------------------------------------------- URL / network safety
const BLOCKED_SCHEMES: &[&str] = &[
    "file",
    "chrome",
    "chrome-extension",
    "javascript",
    "data",
    "about",
    "view-source",
    "devtools",
    "ftp",
    "ws",
    "wss",
];
const BLOCKED_HOST_SUFFIX: &[&str] = &[
    ".local",
    ".localhost",
    ".internal",
    ".svc",
    ".cluster.local",
    ".svc.cluster.local",
    ".lan",
    ".home.arpa",
];

fn ip_is_private(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let o = v4.octets();
            v4.is_private()
                || v4.is_loopback()
                || v4.is_link_local()
                || v4.is_broadcast()
                || v4.is_documentation()
                || v4.is_unspecified()
                || v4.is_multicast()
                || o[0] == 0
                || o[0] >= 240
                || (o[0] == 192 && o[1] == 0 && o[2] == 0)
                || (o[0] == 198 && (o[1] == 18 || o[1] == 19))
        }
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return ip_is_private(IpAddr::V4(v4));
            }
            let s = v6.segments();
            v6.is_loopback()
                || v6.is_unspecified()
                || v6.is_multicast()
                || v6.is_unique_local()
                || v6.is_unicast_link_local()
                || (s[0] == 0x2001 && s[1] == 0x0db8)
                || (s[0] & 0xfe00) == 0xfc00
                || s[0] == 0x0100 && s[1] == 0 && s[2] == 0 && s[3] == 0
                || v6.to_ipv4().is_some_and(|v4| ip_is_private(IpAddr::V4(v4)))
        }
    }
}

/// True for loopback / private / link-local / cluster-internal hosts.
pub fn host_is_private(host: &str) -> bool {
    let host = host.trim_matches(|c| c == '[' || c == ']').to_lowercase();
    let host = host.trim_end_matches('.');
    if host.is_empty() {
        return true;
    }
    if host == "localhost" || host == "metadata.google.internal" || host == "metadata" {
        return true;
    }
    if BLOCKED_HOST_SUFFIX.iter().any(|s| host.ends_with(s)) {
        return true;
    }
    if let Ok(ip) = host.parse::<IpAddr>() {
        return ip_is_private(ip);
    }
    if !host.contains('.') {
        return true; // bare service names inside the cluster
    }
    false
}

/// Python `urlparse(url).scheme` (lowercase), `""` when the text has no scheme.
pub fn scheme_of(url: &str) -> String {
    static RE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"^([A-Za-z][A-Za-z0-9+.\-]*):").unwrap());
    RE.captures(url)
        .map(|c| c[1].to_lowercase())
        .unwrap_or_default()
}

/// `check_url(url) -> (ok, reason)`: only http/https to public hosts.
pub fn check_url(url: &str) -> (bool, String) {
    let u = url.trim();
    let scheme = scheme_of(u);
    if BLOCKED_SCHEMES.contains(&scheme.as_str()) {
        return (false, format!("禁止的协议 scheme '{}'", scheme));
    }
    if scheme != "http" && scheme != "https" {
        return (
            false,
            "只允许 http/https (only http/https allowed)".to_string(),
        );
    }
    let host = url::Url::parse(u)
        .ok()
        .and_then(|p| p.host_str().map(|h| h.to_string()))
        .unwrap_or_default();
    if host_is_private(&host) {
        return (
            false,
            "禁止访问内网/本机地址 (private or cluster-internal address blocked)".to_string(),
        );
    }
    (true, String::new())
}

/// Lowercase host name of a URL (`""` when there is none).
pub fn hostname_of(url: &str) -> String {
    url::Url::parse(url)
        .ok()
        .and_then(|p| {
            p.host_str()
                .map(|h| h.trim_matches(|c| c == '[' || c == ']').to_lowercase())
        })
        .unwrap_or_default()
}

/// Registered domain of a URL (`omuse_common::guard::registered_domain`: amazon.com.sg, not com.sg), lowercase;
/// `""` when unparsable.
pub fn domain_of(url: &str) -> String {
    omuse_common::guard::registered_domain(&hostname_of(url))
}

// ---------------------------------------------------------------- risky UI words
macro_rules! re {
    ($name:ident, $pat:expr) => {
        static $name: LazyLock<Regex> = LazyLock::new(|| Regex::new($pat).unwrap());
    };
}

re!(
    RISKY_CLICK,
    r"(?i)(submit|send|pay|purchase|buy|checkout|check out|place order|order now|confirm|delete|remove|cancel (my )?(subscription|order|account|plan)|unsubscribe|transfer|withdraw|donate|book now|reserve|sign up|register|apply|agree|accept|authorize|approve|refund|post|publish|提交|发送|支付|付款|购买|下单|确认|删除|移除|取消订阅|注销|转账|提现|预订|注册|申请|同意|授权|发布|退款|送信|購入|注文|確定|削除|支払|申し込)"
);
re!(
    SEARCH_WORDS,
    r"(?i)(search|find|query|filter|搜索|查找|搜寻|検索)"
);
re!(
    CART_ADD,
    r"(?i)^\s*(add to (cart|basket|bag|trolley)|add to shopping (cart|bag)|加入购物车|加入購物車|添加到购物车|放入购物车|カートに入れる|장바구니 담기)(\s*,\s*(shift|alt|ctrl|control|cmd|command|option|meta|[a-z0-9]))*\s*$"
);
re!(
    COOKIE_DECLINE,
    r"(?i)^\s*(reject( all)?( cookies)?|decline( all)?( cookies)?|deny( all)?|refuse( all)?|(use |accept )?(only )?(strictly )?(necessary|essential)( cookies)?( only)?|continue without accepting|拒绝(全部|所有)?(cookie|Cookie)?|仅(接受)?必要(的)?(cookie|Cookie)?|只接受必要|全部拒绝|すべて拒否|拒否する|必要なもののみ)\s*$"
);
re!(
    BADGES,
    r"(?i)(buyer|purchase|payment) protection|free (returns|delivery)|apply (coupon|voucher)s? at checkout"
);
re!(
    READONLY_SUBMIT,
    r"(?i)^(?:(display|show( results| rates| data)?|view( results)?|search|find|filter|apply filters?|get (rates|data|results|quote)|look ?up|refresh|reload|load( more)?|more|next|previous|prev|calculate|convert|check( availability)?|compare|sort|显示|查看|查询|搜索|筛选|计算|换算|刷新|下一页|上一页|更多|比较|检查)\s*[»›>→]*)$"
);
re!(
    AGREE,
    r"(?i)agree|accept|consent|terms|authori[sz]e|同意|接受|授权|条款"
);
re!(
    MONEY_CLICK,
    r"(?i)place (your )?order|order now|buy now|pay( now|ment)?\b|purchase|checkout|check out|proceed to (checkout|payment|pay)|complete (order|purchase|booking|payment)|confirm (order|purchase|payment|booking)|subscribe|start (my )?(subscription|trial)|upgrade|donate|transfer|top ?up|book now|reserve now|付款|支付|购买|立即购买|下单|提交订单|结账|结算|确认支付|确认订单|订阅|升级|捐|转账|充值|预订|購入|注文|支払|決済"
);
re!(
    MONEY_URL,
    r"(?i)/(checkout|buy|payment|pay|billing|cart/checkout|order/confirm|subscribe|upgrade)\b"
);
re!(
    CHECKOUT_DETAIL,
    r"(?i)^\s*(\+\s*)?(save|add|edit|change|update|use this|select|choose|confirm)( new| this| my)?\s*(address|delivery address|shipping address|billing address|contact( details)?|phone|details|store|pick-?up (point|store|location)|collection point|location|outlet)\s*$|^\s*apply( (voucher|coupon|promo( code)?|code))?\s*$|^\s*remove (voucher|coupon|promo( code)?|code)\s*$|^\s*(保存|添加|新增|修改|编辑|选择|确认)(收货|送货|配送|账单)?(地址|门店|自取点|取货点|联系方式)\s*$"
);
re!(
    CART_EDIT,
    r"(?i)^\s*(remove|delete|remove item|delete item|移除|删除|删掉)\s*$"
);
re!(
    CHECKOUT_ENTER,
    r"(?i)^\s*((proceed|continue|go) to )?(secure )?(check ?out)( now| securely)?(\s*\(\d+\))?\s*$|^\s*(去|前往)?(结算|结账)(\s*\(\d+\))?\s*$"
);
re!(
    CHECKOUT_STEP,
    r"(?i)^\s*(next( step)?|continue|save and continue|save & continue|(continue|proceed|go) to (shipping|delivery|address|payment|billing|review|summary)( method| options| details)?|下一步|继续|继续付款|去付款页)\s*[»›>→]*\s*$"
);
re!(
    ORDER_CHANGE,
    r"(?i)request cancel(l)?ation|cancel(l)?ation|cancel (this |my |the )?(items?|selected( items)?|order|booking|reservation|ticket|subscription|plan|membership|request|delivery)|return (items?|order|this)|request (a )?(refund|return)|start (a )?return|refund|取消订单|取消预订|取消预约|申请取消|取消商品|取消所选|退货|退款|退订|申请退|キャンセル|返品|返金"
);
re!(
    NAMED_NEVER,
    r"(?i)pay|payment|buy|purchase|order|checkout|check out|place|send|transfer|donate|subscribe|book|reserve|confirm|sign|agree|accept|delete account|close account|withdraw|付款|支付|购买|下单|结账|结算|发送|转账|捐|订阅|预订|预约|订位|确认|签|同意|注销|提现"
);
re!(
    NEGATED,
    r#"(?i)(不要|别|不用|勿|先别|don'?t|do not|never|not)\s*(点|按|click|press|hit|tap)?\s*(击)?\s*[「『"'“]?\s*$"#
);
re!(
    NO_SUBMIT,
    r#"(?i)(不要|别|先别|不用|勿|don'?t|do not|never)\s*(点|按|click|press)?\s*[「『"'“*]*\s*(提交|submit)"#
);
re!(
    TOTAL_LINE,
    r"(?i)(order total|grand total|total payable|amount payable|total to pay|total amount|\btotal\b|合计|总计|总价|应付|实付|合計)"
);
re!(SUBTOTAL, r"(?i)sub-?total|小计|items? total");
re!(GRAND, r"(?i)grand|order total");
re!(
    AMOUNT,
    r"(?:S\$|SGD|US\$|USD|HK\$|HKD|RM|¥|￥|€|£|\$)\s?(\d{1,3}(?:[,\s]\d{3})*(?:\.\d{1,2})?|\d+(?:\.\d{1,2})?)"
);
re!(
    FILTER_NAME,
    r"(?i)price|min|max|filter|sort|range|quantity|qty|size|zip|postal|budget|价格|最低|最高|筛选|数量|尺码|邮编"
);
re!(FILTER_NUMBER, r"^\s*[\d.,]+\s*$");
re!(WS, r"\s+");
re!(CART_URL, r"(?i)/(cart|basket|bag|trolley)\b");

const SUBMIT_VERBS: &[&str] = &[
    "提交",
    "submit",
    "添加",
    "新增",
    "加一条",
    "add a",
    "add the",
    "保存",
    "save",
];
const GENERIC_BTN: &[&str] = &[
    "submit", "save", "add", "ok", "apply", "done", "提交", "保存", "添加",
];

fn squash_ws(s: &str) -> String {
    WS.replace_all(s, " ").into_owned()
}

/// "Proceed to checkout" and friends: opens the next page, pays nothing.
pub fn checkout_enter_click(name: &str) -> bool {
    CHECKOUT_ENTER.is_match(&squash_ws(name))
}

/// "Next step" / "Continue to payment" between checkout pages.
pub fn checkout_step_click(name: &str) -> bool {
    CHECKOUT_STEP.is_match(&squash_ws(name))
}

/// Clicks that cancel / return / refund an existing order or booking.
pub fn order_change_click(name: &str) -> bool {
    let label = squash_ws(name);
    let label = label.trim();
    !label.is_empty() && label.chars().count() <= 80 && ORDER_CHANGE.is_match(label)
}

/// Buttons that spend money (or commit to paying).
pub fn money_click(name: &str, input_type: &str, page_url: &str) -> bool {
    if CHECKOUT_DETAIL.is_match(name.trim()) || checkout_enter_click(name) {
        return false;
    }
    if MONEY_CLICK.is_match(name) {
        return true;
    }
    input_type.to_lowercase() == "submit" && MONEY_URL.is_match(page_url)
}

/// Is this a consequential click (submit / pay / send / delete …)?
pub fn click_is_risky(role: &str, name: &str, input_type: &str) -> bool {
    let mut label = BADGES.replace_all(name, " ").into_owned();
    let r = role.to_lowercase();
    if ["radio", "tab", "option", "switch", "menuitemradio", "day"].contains(&r.as_str())
        || (r == "checkbox" && !AGREE.is_match(&label))
    {
        return false;
    }
    if r == "link" && (label.trim().contains('\n') || label.chars().count() > 80) {
        label = take_chars(label.trim().split('\n').next().unwrap_or(""), 80).to_string();
    }
    if CART_ADD.is_match(&label) || COOKIE_DECLINE.is_match(&label) {
        return false;
    }
    if CHECKOUT_DETAIL.is_match(label.trim()) || checkout_enter_click(&label) {
        return false;
    }
    if input_type.to_lowercase() == "submit" {
        return !READONLY_SUBMIT.is_match(label.trim());
    }
    if SEARCH_WORDS.is_match(&label) {
        return false;
    }
    RISKY_CLICK.is_match(&label)
}

/// `remove` / `delete` buttons (cart line edits when on a cart page).
pub fn cart_edit_click(name: &str, page_url: &str) -> bool {
    CART_EDIT.is_match(name) && CART_URL.is_match(page_url)
}

fn find_chars(hay: &[char], needle: &[char], from: usize) -> Option<usize> {
    if needle.is_empty() || needle.len() > hay.len() {
        return None;
    }
    (from..=hay.len() - needle.len()).find(|&i| hay[i..i + needle.len()] == *needle)
}

fn asked_verb(request: &str, verbs: &[&str], submit_verbs: bool) -> bool {
    let low: Vec<char> = request.to_lowercase().chars().collect();
    let low_s: String = low.iter().collect();
    if submit_verbs && NO_SUBMIT.is_match(&low_s) {
        return false;
    }
    for v in verbs {
        let needle: Vec<char> = v.chars().collect();
        let mut from = 0;
        while let Some(i) = find_chars(&low, &needle, from) {
            let before: String = low[i.saturating_sub(12)..i].iter().collect();
            if !NEGATED.is_match(&before) {
                return true;
            }
            from = i + 1;
        }
    }
    false
}

/// The user's own request asks to upload one of their attached files on the site they named.
pub fn user_asked_upload(request: &str, page_url: &str, path: &str) -> bool {
    let dom = domain_of(page_url);
    if dom.is_empty() || !path.starts_with("uploads/") {
        return false;
    }
    let low = request.to_lowercase();
    let named = low.contains(&dom) || low.contains(dom.strip_prefix("www.").unwrap_or(&dom));
    named
        && asked_verb(
            request,
            &[
                "上传",
                "upload",
                "选择附件",
                "选好文件",
                "选择文件",
                "attach",
            ],
            false,
        )
}

/// The user's own request explicitly asks to click this (non-money) button.
pub fn user_named_click(request: &str, label: &str) -> bool {
    let lab = squash_ws(label);
    let lab = lab.trim();
    let n = lab.chars().count();
    if !(2..=40).contains(&n) || NAMED_NEVER.is_match(lab) {
        return false;
    }
    let lab_low = lab.to_lowercase();
    if GENERIC_BTN.contains(&lab_low.as_str()) && asked_verb(request, SUBMIT_VERBS, true) {
        return true;
    }
    let low: Vec<char> = request.to_lowercase().chars().collect();
    let needle: Vec<char> = lab_low.chars().collect();
    let mut from = 0;
    while let Some(i) = find_chars(&low, &needle, from) {
        let before: String = low[i.saturating_sub(12)..i].iter().collect();
        if !NEGATED.is_match(&before) {
            return true;
        }
        from = i + 1;
    }
    false
}

/// A search box (by role or by its name).
pub fn looks_like_search(role: &str, name: &str) -> bool {
    let r = role.to_lowercase();
    r == "searchbox" || r == "combobox" || SEARCH_WORDS.is_match(name)
}

/// The user's own request names this site.
pub fn named_in_request(request: &str, dom: &str) -> bool {
    let d = dom.to_lowercase();
    let d = d.strip_prefix("www.").unwrap_or(&d);
    !d.is_empty() && d.chars().count() > 4 && request.to_lowercase().contains(d)
}

/// The order total a checkout page shows (largest amount on a "total" line or the line after it).
pub fn page_total(text: &str) -> Option<f64> {
    let lines: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    let mut found: Vec<f64> = Vec::new();
    for (i, ln) in lines.iter().enumerate() {
        if !TOTAL_LINE.is_match(ln) || (SUBTOTAL.is_match(ln) && !GRAND.is_match(ln)) {
            continue;
        }
        let next = lines.get(i + 1).copied().unwrap_or("");
        for cand in [*ln, next] {
            for m in AMOUNT.captures_iter(cand) {
                if let Ok(f) = m[1].replace([',', ' '], "").parse::<f64>() {
                    found.push(f);
                }
            }
            if !found.is_empty() {
                break;
            }
        }
    }
    found.into_iter().reduce(f64::max)
}

static CARD_BOX: LazyLock<Vec<(&'static str, Regex)>> = LazyLock::new(|| {
    vec![
        ("number", Regex::new(r"(?i)card ?number|card no|number on (the )?card|卡号|カード番号|^\s*number\s*$|1234 5678").unwrap()),
        ("expiry", Regex::new(r"(?i)expir|valid (thru|until)|mm ?/ ?yy|有效期|到期|有効期限").unwrap()),
        ("cvc", Regex::new(r"(?i)\bcv[cv2]\b|security code|card code|安全码|验证码|セキュリティコード").unwrap()),
        ("holder", Regex::new(r"(?i)name on (the )?card|card ?holder|holder name|持卡人|名义|カード名義").unwrap()),
    ]
});

/// The card detail this box is for, when it is clearly a different one than `field` (`""` when it fits or is unclear).
pub fn card_box_mismatch(field: &str, name: &str) -> String {
    if name.trim().is_empty() {
        return String::new();
    }
    let Some((_, own)) = CARD_BOX.iter().find(|(k, _)| *k == field) else {
        return String::new();
    };
    if own.is_match(name) {
        return String::new();
    }
    for (kind, rx) in CARD_BOX.iter() {
        if *kind != field && rx.is_match(name) {
            return kind.to_string();
        }
    }
    String::new()
}

/// Enter in a price / size / quantity filter box only narrows a list.
pub fn filter_submit(elem: Option<&Value>, text: &str, page_url: &str) -> bool {
    let Some(elem) = elem else { return false };
    if text.chars().count() > 20 || MONEY_URL.is_match(page_url) {
        return false;
    }
    let kind = get_str(elem, "input_type").to_lowercase();
    if (kind == "number" || kind == "range") && FILTER_NUMBER.is_match(text) {
        return true;
    }
    FILTER_NAME.is_match(&get_str(elem, "name"))
        && !["password", "email", "tel"].contains(&kind.as_str())
}

/// Python truthiness of `elem.get(key)` (helper for policy).
pub fn elem_flag(elem: &Value, key: &str) -> bool {
    elem.get(key).is_some_and(truthy)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_safety() {
        assert!(!check_url("http://192.168.1.1/").0);
        assert!(!check_url("http://127.0.0.1/").0);
        assert!(!check_url("file:///etc/passwd").0);
        assert!(!check_url("javascript:alert(1)").0);
        assert!(check_url("https://news.com/").0);
        assert_eq!(domain_of("https://www.bad.com/x"), "bad.com");
        // two shops under com.sg are two sites (a purchase approved on one never covers the other)
        assert_eq!(domain_of("https://www.shop-a.com.sg/pay"), "shop-a.com.sg");
        assert_ne!(
            domain_of("https://shop-a.com.sg/"),
            domain_of("https://shop-b.com.sg/")
        );
        assert_eq!(domain_of("nonsense"), "");
        assert!(host_is_private("[::1]"));
        assert!(host_is_private("svc"));
        assert!(!host_is_private("example.com"));
    }

    #[test]
    fn clicks() {
        assert!(!click_is_risky("button", "加入购物车", "submit"));
        assert!(!click_is_risky(
            "button",
            "Add to cart, shift, Alt, K",
            "submit"
        ));
        assert!(click_is_risky("button", "Buy Now, shift, Alt, B", "submit"));
        assert!(click_is_risky("button", "Buy Now", "submit"));
        assert!(click_is_risky(
            "button",
            "Add to Cart and checkout",
            "submit"
        ));
        assert!(!click_is_risky("button", "Display", "submit"));
        assert!(!click_is_risky("button", "Show rates", "submit"));
        assert!(click_is_risky("button", "Submit", "submit"));
        assert!(named_in_request(
            "在 FairPrice 网上超市（fairprice.com.sg）把这些东西找到",
            "www.fairprice.com.sg"
        ));
        assert!(!named_in_request("帮我查价格", "www.fairprice.com.sg"));
        assert!(order_change_click("Request cancellation") && order_change_click("Cancel items"));
        assert!(
            order_change_click("取消订单")
                && order_change_click("Return items")
                && order_change_click("申请退款")
        );
        assert!(
            !order_change_click("Cancel")
                && !order_change_click("取消")
                && !order_change_click("Continue shopping")
        );
        let co = "https://www.decathlon.sg/checkout/48b560a0";
        assert!(
            !money_click("Save Address", "submit", co) && !money_click("+ Add Address", "", co)
        );
        assert!(!money_click("Select store", "submit", co) && !money_click("Apply", "submit", co));
        assert!(!money_click("保存地址", "submit", co));
        assert!(money_click("Place Order", "submit", co) && money_click("Pay now", "submit", co));
        assert!(money_click("Next Step", "submit", co));
        for name in [
            "Buy now",
            "Place Order",
            "Pay now",
            "Proceed to payment and place order",
            "Checkout and pay",
        ] {
            assert!(
                money_click(name, "submit", "https://www.decathlon.sg/cart"),
                "{name}"
            );
        }
        assert!(!money_click(
            "Add to Cart",
            "submit",
            "https://www.amazon.sg/dp/B0X"
        ));
        assert!(!money_click("Search", "submit", "https://www.amazon.sg/"));
    }

    #[test]
    fn totals_and_card_boxes() {
        assert_eq!(
            page_total("Items total S$9.00\nDelivery S$4.99\nGrand Total S$13.99"),
            Some(13.99)
        );
        assert_eq!(page_total("no money here"), None);
        assert_eq!(card_box_mismatch("expiry", "Card number"), "number");
        assert_eq!(
            card_box_mismatch("cvc", "Card number 1234 5678 9012 3456"),
            "number"
        );
        assert_eq!(card_box_mismatch("number", "Card number"), "");
        assert_eq!(card_box_mismatch("expiry", "Expiry date MM/YY"), "");
        assert_eq!(card_box_mismatch("cvc", "Security code 3 digits"), "");
        assert_eq!(card_box_mismatch("number", ""), "");
    }
}
