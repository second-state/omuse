//! Policy engine: ALLOW / DENY / ASK_USER for every proposed action.
//!
//! The phone and MCP checks live in `omuse-sentinel` (`phone.py`, `mcp_hub.py`), which depends on this crate; they
//! are reached through the [`Hooks`] trait that the sentinel installs once at start with [`set_hooks`]. Without
//! hooks, phone tools are "not configured" and MCP tools are "removed" — exactly what the Python code answers when
//! those modules have nothing configured.

use std::sync::{Arc, LazyLock};

use anyhow::Result;
use parking_lot::RwLock;
use serde::Serialize;
use serde_json::{json, Value};

use omuse_common::pyjson::py_repr;
use omuse_common::util::{as_f64, get_str, py_str, truthy};

use crate::store::{field_f64, field_i64, field_str, Store};
use crate::{catalog, guard, vault};

pub const ALLOW: &str = "ALLOW";
pub const DENY: &str = "DENY";
pub const ASK: &str = "ASK_USER";

/// Tools that are approved one by one — never covered by a standing grant; approval scope forced to ONCE.
pub const PER_USE_TOOLS: &[&str] = &["browser_fill_secret", "purchase_confirm"];
/// Archiving / trashing more emails than this in one call needs approval.
pub const BULK_MAIL: usize = 5;
/// `app/sentinel/ical.py: READ_ONLY` — the refusal for write tools on an iCal (read-only) calendar connection.
pub const ICAL_READ_ONLY: &str = "这是只读的 iCal 日历连接，不能新建、修改或删除日程。要写入日程，请在「连接 → Google 日历」用 Google 登录连接 (read-only iCal connection: sign in with Google to add or change events)";

/// `RISK_ORDER` (unknown risks rank lowest).
pub fn risk_order(risk: &str) -> i64 {
    match risk {
        "low" => 0,
        "medium" => 1,
        "high" => 2,
        "critical" => 3,
        _ => 0,
    }
}

/// The outcome of [`decide`] (the Python dataclass).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Default)]
pub struct Decision {
    pub decision: String,
    pub risk: String,
    pub reason: String,
    pub destination: String,
    pub grant_id: String,
    pub notes: Vec<String>,
    /// Approved one by one, never by a standing grant or the browser auto-approval (vault fills, purchases, money
    /// and order-change clicks).
    #[serde(default)]
    pub per_use: bool,
    /// Raised to ASK because the task read content that looks like a prompt injection.
    #[serde(default)]
    pub injected: bool,
}

impl Decision {
    /// `Decision(decision, risk, reason)`.
    pub fn new(decision: &str, risk: &str, reason: impl Into<String>) -> Self {
        Self {
            decision: decision.to_string(),
            risk: risk.to_string(),
            reason: reason.into(),
            ..Default::default()
        }
    }

    /// `Decision(decision, risk, reason, destination)`.
    pub fn with_dest(decision: &str, risk: &str, reason: impl Into<String>, dest: &str) -> Self {
        let mut d = Self::new(decision, risk, reason);
        d.destination = dest.to_string();
        d
    }

    fn grant(mut self, gid: &str) -> Self {
        self.grant_id = gid.to_string();
        self
    }

    pub fn is_allow(&self) -> bool {
        self.decision == ALLOW
    }
    pub fn is_deny(&self) -> bool {
        self.decision == DENY
    }
    pub fn is_ask(&self) -> bool {
        self.decision == ASK
    }

    /// Whether "auto-approve browser actions" may approve this ASK on the user's behalf: a browser tool, not a
    /// vault fill or an upload (the user's own information leaving the machine), not a per-use decision (money,
    /// order changes) and not after a suspected prompt injection. Logins and takeovers never come through here.
    pub fn browser_auto_ok(&self, tool: &str) -> bool {
        self.decision == ASK
            && tool.starts_with("browser_")
            && tool != "browser_fill_secret"
            && tool != "browser_upload"
            && !self.per_use
            && !self.injected
    }

    /// `dataclasses.asdict(d)`.
    pub fn to_json(&self) -> Value {
        serde_json::to_value(self).unwrap_or(Value::Null)
    }
}

// ---------------------------------------------------------------- hooks into omuse-sentinel (phone, MCP hub)

/// What the policy engine needs from modules that live above this crate. Every method has the "nothing configured"
/// default, so a `()`-like implementation is not required for tests or a sentinel without phone / MCP.
pub trait Hooks: Send + Sync {
    /// `mcp_hub.tool_info(store, tool) -> (server, tool_record)`; `None` when the tool / server is gone.
    fn mcp_tool_info(&self, store: &Store, tool: &str) -> Option<(Value, Value)> {
        let _ = (store, tool);
        None
    }
    /// `phone.ready(store)`.
    fn phone_ready(&self, store: &Store) -> bool {
        let _ = store;
        false
    }
    /// `phone.config(store)` (`daily_limit` is read from it).
    fn phone_config(&self, store: &Store) -> Value {
        let _ = store;
        json!({})
    }
    /// `phone.check_call(store, number, cfg) -> (normalized, line, reason)`; an empty `normalized` means refused.
    fn phone_check_call(
        &self,
        store: &Store,
        number: &str,
        cfg: &Value,
    ) -> (String, String, String) {
        let _ = (store, number, cfg);
        (String::new(), String::new(), String::new())
    }
    /// `phone.calls_today(store)`.
    fn phone_calls_today(&self, store: &Store) -> i64 {
        let _ = store;
        0
    }
}

struct NoHooks;
impl Hooks for NoHooks {}

static HOOKS: LazyLock<RwLock<Arc<dyn Hooks>>> = LazyLock::new(|| RwLock::new(Arc::new(NoHooks)));

/// Install the sentinel's phone / MCP hooks (once, at start).
pub fn set_hooks(h: Arc<dyn Hooks>) {
    *HOOKS.write() = h;
}

fn hooks() -> Arc<dyn Hooks> {
    HOOKS.read().clone()
}

// ---------------------------------------------------------------- helpers

/// `phone.normalize(number)`: strip spaces, dashes, dots and parentheses; `00` prefix → `+`.
pub fn phone_normalize(number: &str) -> String {
    let n: String = number
        .chars()
        .filter(|c| !(c.is_whitespace() || matches!(c, '-' | '(' | ')' | '.')))
        .collect();
    match n.strip_prefix("00") {
        Some(rest) => format!("+{rest}"),
        None => n,
    }
}

fn bump(risk: &mut String, to: &str) {
    if risk_order(to) > risk_order(risk) {
        *risk = to.to_string();
    }
}

fn opt_str(v: Option<&Value>, key: &str) -> String {
    v.map(|x| get_str(x, key)).unwrap_or_default()
}

fn opt_flag(v: Option<&Value>, key: &str) -> bool {
    v.and_then(|x| x.get(key)).is_some_and(truthy)
}

fn str_list(v: Option<&Value>) -> Vec<String> {
    v.and_then(Value::as_array)
        .map(|a| a.iter().map(py_str).collect())
        .unwrap_or_default()
}

fn list_len(v: Option<&Value>) -> usize {
    v.and_then(Value::as_array).map(|a| a.len()).unwrap_or(0)
}

fn is_password(elem: &Value) -> bool {
    get_str(elem, "input_type").to_lowercase() == "password"
        || guard::elem_flag(elem, "is_password")
}

fn searchy(elem: &Value) -> bool {
    guard::looks_like_search(&get_str(elem, "role"), &get_str(elem, "name"))
        || guard::elem_flag(elem, "searchy")
}

fn title_case(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + &c.as_str().to_lowercase(),
        None => String::new(),
    }
}

fn blocked_domain(conn: &Value, dom: &str) -> bool {
    str_list(conn["config"].get("blocked_domains"))
        .iter()
        .any(|d| d == dom)
}

fn url_query_and_path(url: &str) -> (String, String) {
    match url::Url::parse(url) {
        Ok(u) => (u.query().unwrap_or("").to_string(), u.path().to_string()),
        Err(_) => (String::new(), String::new()),
    }
}

fn unquote(s: &str) -> String {
    percent_encoding::percent_decode_str(s)
        .decode_utf8_lossy()
        .into_owned()
}

/// Where an action sends data: recipients, `reply:<id>`, a domain, `slack:<channel>`, `notion:<page>`, `tel:<number>`.
pub fn destination(
    tool: &str,
    args: &Value,
    _elem: Option<&Value>,
    page: Option<&Value>,
) -> String {
    if ["gmail_send", "gmail_create_draft", "gmail_forward"].contains(&tool) {
        let joined = format!("{},{}", get_str(args, "to"), get_str(args, "cc"));
        let mut rec: Vec<String> = joined
            .split(',')
            .filter(|x| !x.trim().is_empty())
            .map(|x| x.trim().to_lowercase())
            .collect();
        rec.sort();
        return rec.join(",");
    }
    if tool == "gmail_reply" {
        return format!("reply:{}", get_str(args, "message_id"));
    }
    if tool.starts_with("browser_") {
        let url = get_str(args, "url");
        let url = if url.is_empty() {
            opt_str(page, "url")
        } else {
            url
        };
        return guard::domain_of(&url);
    }
    if tool == "slack_send_message" {
        return format!(
            "slack:{}",
            get_str(args, "channel")
                .trim_start_matches('#')
                .to_lowercase()
        );
    }
    if tool.starts_with("notion_")
        && !["notion_search", "notion_get_page", "notion_query_database"].contains(&tool)
    {
        let id = get_str(args, "parent_id");
        let id = if id.is_empty() {
            get_str(args, "page_id")
        } else {
            id
        };
        return format!("notion:{id}");
    }
    if tool == "phone_call" {
        return format!("tel:{}", phone_normalize(&get_str(args, "to")));
    }
    String::new()
}

/// Decide one proposed tool call. `elem` / `page` are the browser element and page the runtime attached
/// (snapshot element dict, `{"url", "title", "text"}`); `gmail_ready` is whether a mailbox is connected.
pub fn decide(
    store: &Store,
    tool: &str,
    args: &Value,
    task_id: &str,
    elem: Option<&Value>,
    page: Option<&Value>,
    gmail_ready: bool,
) -> Result<Decision> {
    let Some(t) = catalog::get(tool) else {
        return Ok(Decision::new(
            DENY,
            "high",
            format!("未知工具 unknown tool: {tool}"),
        ));
    };
    let connector = get_str(&t, "connector");
    let capability = get_str(&t, "capability");
    let t_risk = get_str(&t, "risk");
    if connector.starts_with("mcp:") {
        return decide_mcp(store, tool, &t, args, task_id);
    }
    let conn = store.connection(&connector)?;
    // web search is read-only and always on: the search services when configured, else the browser's own engines
    if connector == "search" {
        return Ok(Decision::new(ALLOW, &t_risk, String::new()));
    }
    // the searches always run (flights: Kiwi.com first; hotels and places fall back to web search without the SerpApi
    // key); the detail tools read Google ids, so they need the key (the user's, or the server's default)
    let keyed_tool = tool == "hotel_rooms" || tool == "place_details";
    if connector == "travel"
        && keyed_tool
        && !store.has_secret("cred_travel_1")?
        && !truthy(&conn["config"]["default_key"])
    {
        return Ok(Decision::new(
            DENY,
            &t_risk,
            "酒店房价和餐厅详情需要「机票与酒店搜索」的 SerpApi 密钥：请在「连接 Connections」页填写；没有密钥时搜索结果来自网页搜索，请用 browser_read 读酒店或餐厅的页面 (hotel rates and place details need the SerpApi key of Flights & hotels; without it the searches come from web search — browser_read the hotel's or restaurant's page; flights work without it)".to_string(),
        ));
    }
    if connector == "travel" && !truthy(&conn["enabled"]) && !keyed_tool {
        // the connection off still leaves the free flight data: the switch is about the key
        return Ok(Decision::new(ALLOW, &t_risk, String::new()));
    }
    if !truthy(&conn["enabled"]) {
        return Ok(Decision::new(
            DENY,
            &t_risk,
            format!("连接「{connector}」未启用 (connector disabled)"),
        ));
    }
    if !conn["permissions"].get(&capability).is_some_and(truthy) {
        return Ok(Decision::new(
            DENY,
            &t_risk,
            format!("权限「{connector}.{capability}」已关闭 (permission off in Connections)"),
        ));
    }
    if connector == "gmail" && !gmail_ready {
        return Ok(Decision::new(
            DENY,
            &t_risk,
            "邮箱尚未连接：请在「连接 Connections」页添加邮箱 (no mailbox connected)",
        ));
    }

    let mut risk = t_risk.clone();
    let mut reasons: Vec<String> = Vec::new();
    let ctx = store.task_ctx(task_id)?;
    let injection = str_list(ctx.get("injection"));
    let domains = str_list(ctx.get("domains"));
    let tainted = guard::level(&field_str(&ctx, "taint")) >= guard::level(guard::CONFIDENTIAL);
    let dest = destination(tool, args, elem, page);
    let is_click = tool == "browser_click" || tool == "browser_click_at";
    let elem_name = opt_str(elem, "name");
    let page_url = opt_str(page, "url");

    // ---------------------------------------------------------------- browser rules
    if tool == "browser_navigate" {
        let url = get_str(args, "url");
        let (ok, why) = guard::check_url(&url);
        if !ok {
            return Ok(Decision::new(DENY, "high", why));
        }
        let dom = guard::domain_of(&url);
        if blocked_domain(&conn, &dom) {
            return Ok(Decision::new(
                DENY,
                "high",
                format!("域名 {dom} 在黑名单中 (blocked domain)"),
            ));
        }
        let (query, path) = url_query_and_path(&url);
        let carries_data = !query.is_empty() || path.chars().count() > 60;
        // a site the user named in their own request is where they want the data to go (2026-10-04 V3-03: searching
        // fairprice.com.sg for the items of the attached shopping list needed an approval per item)
        let allowed = str_list(conn["config"].get("allowed_domains"));
        let trusted = allowed.contains(&dom)
            || (injection.is_empty()
                && guard::named_in_request(&store.user_request(task_id)?, &dom)); // never after an injection
        if tainted && !domains.contains(&dom) && !trusted && carries_data {
            bump(&mut risk, "high");
            reasons.push("本任务已读取机密数据（邮件等），且此网址带有参数，可能造成数据外泄 (data egress check)".into());
        } else if !injection.is_empty()
            && carries_data
            && (!domains.contains(&dom)
                || unquote(&query).contains('@')
                || query.chars().count() > 60)
        {
            // after a suspected injection, a link that carries data needs the user's OK — also on the SAME site, since the
            // injected page can name an address on its own domain (httpbin.org/anything?owner_email=…)
            bump(&mut risk, "high");
            reasons.push("本任务读到疑似提示注入的内容后，要访问一个带参数的新网址 (possible exfiltration after injection)".into());
        }
    }
    if tool == "browser_read" {
        let urls: Vec<String> = match args.get("urls") {
            Some(Value::String(s)) if !s.is_empty() => vec![s.clone()],
            Some(Value::Array(a)) if !a.is_empty() => a.iter().map(py_str).collect(),
            _ => {
                let u = get_str(args, "url");
                if u.is_empty() {
                    vec![]
                } else {
                    vec![u]
                }
            }
        };
        if urls.is_empty() {
            return Ok(Decision::new(DENY, "low", "缺少参数 urls (give 1–4 URLs)"));
        }
        let allowed = str_list(conn["config"].get("allowed_domains"));
        for u in urls.iter().take(4) {
            let (ok, why) = guard::check_url(u);
            if !ok {
                return Ok(Decision::new(
                    DENY,
                    "high",
                    format!("{why}: {}", omuse_common::util::take_chars(u, 80)),
                ));
            }
            let dom = guard::domain_of(u);
            if blocked_domain(&conn, &dom) {
                return Ok(Decision::new(
                    DENY,
                    "high",
                    format!("域名 {dom} 在黑名单中 (blocked domain)"),
                ));
            }
            let (query, path) = url_query_and_path(u);
            let carries_data = !query.is_empty() || path.chars().count() > 60;
            if tainted
                && !domains.contains(&dom)
                && !allowed.contains(&dom)
                && carries_data
                && !(injection.is_empty()
                    && guard::named_in_request(&store.user_request(task_id)?, &dom))
            {
                bump(&mut risk, "high");
                reasons.push(
                    "本任务已读取机密数据，且此网址带有参数，可能造成数据外泄 (data egress check)"
                        .into(),
                );
                break;
            }
            if !injection.is_empty()
                && carries_data
                && (!domains.contains(&dom)
                    || unquote(&query).contains('@')
                    || query.chars().count() > 60)
            {
                bump(&mut risk, "high");
                reasons.push("本任务读到疑似提示注入的内容后，要读取一个带参数的网址 (possible exfiltration after injection)".into());
                break;
            }
        }
    }
    if tool == "browser_save_media" {
        let url = get_str(args, "url");
        if truthy(&args["url"]) && !url.starts_with("data:") {
            let (ok, why) = guard::check_url(&url);
            if !ok {
                return Ok(Decision::new(DENY, "high", why));
            }
            let mdom = guard::domain_of(&url);
            if blocked_domain(&conn, &mdom) {
                return Ok(Decision::new(
                    DENY,
                    "high",
                    format!("域名 {mdom} 在黑名单中 (blocked domain)"),
                ));
            }
        }
        if !truthy(&args["ref"]) && !truthy(&args["url"]) {
            return Ok(Decision::new(DENY, "low", "缺少参数：要给出图片/视频元素的 ref（如 e12）或者它的 url。Missing argument: pass ref (the [eN] of the photo or video in the snapshot) or url (its address). For charts, use make_chart instead"));
        }
    }
    if [
        "browser_click",
        "browser_click_at",
        "browser_type",
        "browser_select",
        "browser_press",
        "browser_upload",
        "browser_save_media",
    ]
    .contains(&tool)
        && page.is_some()
    {
        let dom = guard::domain_of(&page_url);
        if !dom.is_empty() && blocked_domain(&conn, &dom) {
            return Ok(Decision::new(
                DENY,
                "high",
                format!("域名 {dom} 在黑名单中 (blocked domain)"),
            ));
        }
    }
    if tool == "browser_click_at" && !opt_flag(elem, "tag") {
        bump(&mut risk, "high");
        reasons.push("无法确认这个位置上是什么元素 (can't tell what is at this position)".into());
    }
    let mut per_use = false;
    if tool == "purchase_confirm" {
        let raw = get_str(args, "site");
        let raw = raw.trim();
        let raw = raw.strip_prefix("https://").unwrap_or(raw);
        let raw = raw.strip_prefix("http://").unwrap_or(raw);
        let site = guard::domain_of(&format!("https://{raw}"));
        let total = args.get("total").and_then(as_f64).unwrap_or(0.0);
        if site.is_empty() || total <= 0.0 || !truthy(&args["items"]) {
            return Ok(Decision::new(DENY, "high", "purchase_confirm 需要 site、items 和页面上的总价 total (site, items and the page total are required)"));
        }
        if truthy(&args["card_item_id"])
            && vault::item(store, &get_str(args, "card_item_id"))?.is_none()
        {
            return Ok(Decision::new(
                DENY,
                "high",
                "保险箱里没有这张卡，请先用 vault_list 查看 (unknown vault item)",
            ));
        }
        return Ok(Decision::with_dest(
            ASK,
            "high",
            format!(
                "确认这次购买：{site}，总价 {} {total:.2}；批准后本网站这笔订单的结账、填卡、下单不再逐个审批（30 分钟内，金额不超过这个总价） (one approval for the whole purchase)",
                get_str(args, "currency")
            ),
            &site,
        ));
    }
    let mut purchase: Option<Value> = None;
    if injection.is_empty() && page.is_some() && (is_click || tool == "browser_fill_secret") {
        purchase = store.active_purchase(task_id, &guard::domain_of(&page_url))?;
    }
    if is_click && elem.is_some() && guard::order_change_click(&elem_name) {
        bump(&mut risk, "high");
        per_use = true;
        reasons.push(format!(
            "「{elem_name}」会取消/退掉一个订单或预订，撤不回来：每次都要你单独批准，请先核对页面上的订单号 (changes an order — approved one by one; check the order number on this page: {})",
            opt_str(page, "title")
        ));
    }
    let checkout_step = is_click
        && elem.is_some()
        && !per_use
        && guard::checkout_step_click(&elem_name)
        && store.count_done(task_id, &["browser_fill_secret"])? == 0;
    if is_click
        && elem.is_some()
        && !per_use
        && !checkout_step
        && guard::money_click(&elem_name, &opt_str(elem, "input_type"), &page_url)
    {
        bump(&mut risk, "high");
        per_use = true;
        reasons.push(format!(
            "「{elem_name}」会花钱或确认付款：每次都要你单独批准，任何长期授权都不覆盖 (spends money — approved one by one, never by a standing grant)"
        ));
        if let Some(p) = purchase
            .as_ref()
            .filter(|p| field_i64(p, "clicks_left") > 0)
        {
            let shown = guard::page_total(&opt_str(page, "text"));
            let max_total = field_f64(p, "max_total");
            let limit = max_total * 1.02 + 0.5;
            match shown {
                Some(s) if s > limit => reasons.push(format!(
                    "页面显示的总价 {s:.2} 超过了你确认的 {max_total:.2}，需要重新确认 (page total {s:.2} exceeds the confirmed {max_total:.2})"
                )),
                _ => {
                    let pid = field_str(p, "id");
                    store.use_purchase(&pid, "click")?;
                    return Ok(Decision::with_dest(
                        ALLOW,
                        "high",
                        format!("已在购买确认 {pid} 中批准 (covered by the purchase you confirmed)"),
                        &dest,
                    )
                    .grant(&pid));
                }
            }
        }
    }
    if let Some(e) = elem.filter(|_| is_click && !per_use && !checkout_step) {
        // a <button> reports type "submit" by default; outside a <form> it submits nothing (chat launchers, menus)
        let in_form = e.get("in_form").map(truthy).unwrap_or(true);
        let itype = if in_form {
            get_str(e, "input_type")
        } else {
            String::new()
        };
        let cart_edit = guard::cart_edit_click(&elem_name, &page_url);
        if !cart_edit
            && guard::click_is_risky(&get_str(e, "role"), &elem_name, &itype)
            && !guard::user_named_click(&store.user_request(task_id)?, &elem_name)
        {
            bump(&mut risk, "high");
            reasons.push(format!(
                "点击的按钮「{elem_name}」可能提交/购买/发送/删除 (consequential click)"
            ));
        }
    }
    if tool == "browser_type" || (tool == "browser_click_at" && truthy(&args["text"])) {
        if elem.is_some_and(is_password) {
            return Ok(Decision::new(DENY, "high", "Agent 不能输入密码。请调用 browser_request_takeover 让用户接管输入 (use takeover for passwords)"));
        }
        let text = get_str(args, "text");
        let is_search = elem.is_some_and(searchy);
        if truthy(&args["submit"]) && !is_search && !guard::filter_submit(elem, &text, &page_url) {
            bump(&mut risk, "high");
            reasons.push("输入后会按回车提交表单 (submits a form)".into());
        } else if tainted && text.chars().count() > 40 && !is_search {
            bump(&mut risk, "high");
            reasons
                .push("本任务读取过机密数据，正在把较长文本输入网页 (possible data egress)".into());
        } else if !is_search {
            bump(&mut risk, "medium");
        }
    }
    if tool == "browser_fill_secret" {
        if let Some(e) = elem {
            let field = get_str(args, "field");
            let label = format!("{} {}", get_str(e, "name"), get_str(e, "placeholder"));
            let other = guard::card_box_mismatch(&field, &label);
            if !other.is_empty() {
                return Ok(Decision::new(DENY, "high", format!(
                    "这个输入框「{elem_name}」是填 {other} 的，不是 {field}；请在快照里找对应的输入框（银行卡的每一栏常在各自的 iframe 里，如 f8e1、f9e1）(this box is for the card {other}, not the {field} — find the right box; card fields often sit in separate iframes)"
                )));
            }
        }
    }
    if tool == "browser_fill_secret" {
        if let Some(p) = purchase.as_ref() {
            let card_item = field_str(p, "card_item");
            if field_i64(p, "fills_left") > 0
                && !card_item.is_empty()
                && card_item == get_str(args, "item_id")
            {
                let it0 = vault::item(store, &card_item)?;
                if let (Some(it0), Some(e)) = (it0, elem) {
                    if vault::item_fields(&it0).contains(&get_str(args, "field")) && !is_password(e)
                    {
                        let pid = field_str(p, "id");
                        store.use_purchase(&pid, "fill")?;
                        return Ok(Decision::with_dest(
                            ALLOW,
                            "high",
                            format!(
                                "已在购买确认 {pid} 中批准 (covered by the purchase you confirmed)"
                            ),
                            &dest,
                        )
                        .grant(&pid));
                    }
                }
            }
        }
    }
    if tool == "browser_fill_secret" {
        let it = vault::item(store, &get_str(args, "item_id"))?;
        let field = get_str(args, "field");
        let Some(it) = it else {
            return Ok(Decision::new(
                DENY,
                "high",
                "保险箱里没有这一项，请先用 vault_list 查看 (unknown vault item)",
            ));
        };
        let fields = vault::item_fields(&it);
        let label = field_str(&it, "label");
        if !fields.contains(&field) {
            return Ok(Decision::new(
                DENY,
                "high",
                format!(
                    "「{label}」没有字段 {field}，可用字段：{} (unknown field)",
                    fields.join(", ")
                ),
            ));
        }
        let (Some(e), Some(_)) = (elem, page) else {
            return Ok(Decision::new(
                DENY,
                "high",
                "找不到要填写的输入框，请先获取页面快照 (input not found — take a snapshot first)",
            ));
        };
        let tag = get_str(e, "tag").to_lowercase();
        if is_password(e) {
            return Ok(Decision::new(
                DENY,
                "high",
                "不能把保险箱内容填进密码框 (never into a password field)",
            ));
        }
        if tag != "input" && tag != "textarea" && !guard::elem_flag(e, "editable") {
            return Ok(Decision::new(
                DENY,
                "high",
                format!("目标元素不是输入框（{tag}）(target is not a text input)"),
            ));
        }
        let dom = guard::domain_of(&page_url);
        if dom.is_empty() {
            return Ok(Decision::new(
                DENY,
                "high",
                "无法确定当前网站 (unknown site)",
            ));
        }
        if blocked_domain(&conn, &dom) {
            return Ok(Decision::new(
                DENY,
                "high",
                format!("域名 {dom} 在黑名单中 (blocked domain)"),
            ));
        }
        if !vault::domain_ok(&it, &dom) {
            let doms = str_list(it.get("domains")).join(", ");
            return Ok(Decision::new(DENY, "high", format!("「{label}」只允许在 {doms} 使用，当前网站是 {dom} (item restricted to other sites)")));
        }
        if field_str(&it, "kind") == "card" && purchase.is_none() && injection.is_empty() {
            // 2026-10-06 golden G04: the card was filled before the purchase was confirmed, so the user would get a
            // card-fill approval and then the purchase card. One card for the whole purchase: confirm first.
            return Ok(Decision::new(DENY, "high", "填银行卡之前先调用 purchase_confirm：在显示最终总价的页面写清商品、运费、总价、送货方式和这张卡（card_item_id）。用户批准这一张卡之后，填卡和下单都不用再单独批准。 (Call purchase_confirm first, on the page that shows the final total; once the user approves it, the card fills and the order click go through without more approvals.)"));
        }
        risk = "high".to_string();
        reasons.push(format!(
            "把保险箱里的「{}」填到 {dom} 的输入框（每次使用都需要你批准）(fills a vault value — approved per use)",
            vault::summary_text(&it, &field)
        ));
        if guard::scheme_of(&page_url) != "https"
            && !(dom.ends_with(".test") || dom == "localhost" || dom == "127.0.0.1")
        {
            reasons.push("⚠ 这个页面不是 HTTPS 加密连接 (page is not HTTPS)".into());
        }
    }
    if tool == "browser_press"
        && ["enter", "return"].contains(&get_str(args, "key").to_lowercase().as_str())
    {
        if let Some(e) = elem {
            if !searchy(e) && guard::elem_flag(e, "in_form") {
                bump(&mut risk, "high");
                reasons.push("回车会提交表单 (Enter submits a form)".into());
            }
        }
    }
    if tool == "browser_upload" {
        if page.is_some()
            && guard::user_asked_upload(
                &store.user_request(task_id)?,
                &page_url,
                &get_str(args, "path"),
            )
        {
            risk = "medium".to_string(); // the user attached this file and asked for it to be uploaded on this very site
        } else {
            reasons.push("上传本地文件到网站 (uploads a local file)".into());
        }
    }

    // ---------------------------------------------------------------- gmail rules
    if ["gmail_send", "gmail_reply", "gmail_forward"].contains(&tool) {
        reasons.push("对外发送邮件 (sends email on your behalf)".into());
    }
    if tool == "gmail_archive" || tool == "gmail_label" {
        // 2026-10-02 test "delete all my emails": 35 emails in 4 mailboxes were archived without asking. Bulk changes
        // that take mail out of the inbox (or trash it) need the user's OK; one approval can cover the task.
        let n = list_len(args.get("message_ids"));
        let mut labels = str_list(args.get("remove_labels"));
        labels.extend(str_list(args.get("add_labels")));
        let moves = tool == "gmail_archive"
            || labels.iter().any(|x| {
                ["\\inbox", "inbox", "\\trash", "trash", "\\spam", "spam"]
                    .contains(&x.to_lowercase().as_str())
            });
        if moves
            && (n > BULK_MAIL || store.count_done(task_id, &["gmail_archive", "gmail_label"])? >= 2)
        {
            bump(&mut risk, "high");
            reasons.push(format!("将批量移动 {n} 封邮件（移出收件箱/垃圾箱）(bulk change: {n} emails leave the inbox)"));
        }
    }
    if tool == "gmail_unsubscribe" {
        reasons.push(format!(
            "将代表你退订 {} 个发件方（发送退订请求/退订邮件）(unsubscribes on your behalf)",
            list_len(args.get("message_ids"))
        ));
    }

    // ---------------------------------------------------------------- slack / notion rules
    if (connector == "notion" || connector == "slack")
        && !store.has_secret(&format!("cred_{connector}_1"))?
    {
        return Ok(Decision::new(
            DENY,
            &t_risk,
            format!(
                "{} 尚未连接：请在「连接 Connections」页设置 (not connected)",
                title_case(&connector)
            ),
        ));
    }
    if tool == "slack_send_message" {
        reasons.push("代表你在 Slack 发送消息 (posts to Slack on your behalf)".into());
    }
    if tool == "notion_update_page" && truthy(&args["archived"]) {
        bump(&mut risk, "high");
        reasons.push("将归档（删除）一个 Notion 页面 (archives a Notion page)".into());
    }

    // ---------------------------------------------------------------- calendar rules
    if connector == "calendar" {
        if !store.has_secret("cred_calendar_1")? {
            return Ok(Decision::new(
                DENY,
                &t_risk,
                "Google 日历尚未连接：请在「连接 Connections」页设置 (not connected)",
            ));
        }
        if capability == "write"
            && store
                .get_secret("cred_calendar_1")?
                .is_some_and(|s| truthy(&s["ical_url"]))
        {
            return Ok(Decision::new(DENY, &t_risk, ICAL_READ_ONLY));
        }
        if tool == "calendar_create_event" && truthy(&args["attendees"]) {
            bump(&mut risk, "high");
            reasons.push("会给参会人发送日历邀请邮件 (sends calendar invitations)".into());
        } else if tool == "calendar_create_event"
            && tainted
            && get_str(args, "description").chars().count() > 300
        {
            bump(&mut risk, "high");
            reasons.push(
                "本任务读取过机密数据，这次要把较长的内容写进日程 (possible data egress)".into(),
            );
        }
        if tool == "calendar_update_event" {
            reasons.push("修改已有日程，参会人会收到更新 (changes an existing event)".into());
        }
        if tool == "calendar_delete_event" {
            reasons.push("删除日程，参会人会收到取消通知 (deletes an event)".into());
        }
    }

    // ---------------------------------------------------------------- Google Drive rules
    if connector == "drive" {
        if !store.has_secret("cred_drive_1")? {
            return Ok(Decision::new(
                DENY,
                &t_risk,
                "Google Drive 尚未连接：请在「连接 Connections」页用 Google 登录并勾选 Drive (not connected)",
            ));
        }
        if tool == "drive_trash" {
            let n = args["file_ids"].as_array().map(|a| a.len()).unwrap_or(0);
            reasons.push(format!(
                "把 {n} 个文件移到 Google Drive 回收站，30 天内可恢复 (moves {n} files to the Drive bin; recoverable for 30 days)"
            ));
        }
        if tool == "drive_upload" && tainted {
            bump(&mut risk, "high");
            reasons.push(
                "本任务读取过机密数据，正在把文件上传到 Google Drive (possible data egress)".into(),
            );
        }
    }

    // ---------------------------------------------------------------- phone rules
    if connector == "phone" {
        let h = hooks();
        if !h.phone_ready(store) {
            return Ok(Decision::new(DENY, &t_risk, "电话功能还没配置好：请在「连接 → 电话」连接 DialMCP 或填写 Telnyx 设置 (phone not configured)"));
        }
        if tool == "phone_call" {
            let cfg = h.phone_config(store);
            let (n, line, why) = h.phone_check_call(store, &get_str(args, "to"), &cfg);
            if n.is_empty() {
                return Ok(Decision::with_dest(DENY, "high", why, &dest));
            }
            let daily_limit = field_i64(&cfg, "daily_limit");
            if h.phone_calls_today(store) >= daily_limit {
                return Ok(Decision::with_dest(
                    DENY,
                    "high",
                    format!("今天已达到拨打上限 {daily_limit} 通 (daily call limit reached)"),
                    &dest,
                ));
            }
            if get_str(args, "purpose").trim().chars().count() < 10 {
                return Ok(Decision::with_dest(
                    DENY,
                    "high",
                    "请写清楚这通电话要做什么 (purpose is required)",
                    &dest,
                ));
            }
            reasons.push(if line == "dialmcp" {
                "用你自己的号码通过 DialMCP 打电话，对方看到的是你的号码，通话会录音 (places a call from your own number via DialMCP; recorded)".into()
            } else {
                "用你的名义打电话，语音 AI 会和对方实时对话（按通话时长计费）(places a phone call on your behalf)".into()
            });
        }
    }

    // ---------------------------------------------------------------- prompt-injection escalation
    if !injection.is_empty() && risk_order(&risk) >= risk_order("medium") {
        bump(&mut risk, "high");
        reasons.push(format!(
            "本任务读到的外部内容疑似包含提示注入，所有写操作需要人工确认 (prompt-injection suspected: {})",
            injection.join(", ")
        ));
    }

    finish(
        store, tool, task_id, &risk, &reasons, &dest, &injection, per_use,
    )
}

#[allow(clippy::too_many_arguments)]
fn finish(
    store: &Store,
    tool: &str,
    task_id: &str,
    risk: &str,
    reasons: &[String],
    dest: &str,
    injection: &[String],
    per_use: bool,
) -> Result<Decision> {
    let reason = reasons.join("；");
    if risk_order(risk) < risk_order("high") {
        return Ok(Decision::with_dest(ALLOW, risk, reason, dest));
    }

    // ---------------------------------------------------------------- grants for high-risk actions
    // vault fills are approved one by one, never by a standing grant
    if injection.is_empty() && !PER_USE_TOOLS.contains(&tool) && !per_use {
        for g in store.active_grants()? {
            if field_str(&g, "tool") != tool {
                continue;
            }
            if field_str(&g, "scope") == "TASK" && field_str(&g, "task_id") != task_id {
                continue;
            }
            let want = get_str(&g["match"], "destination");
            if !want.is_empty() && want != dest {
                continue;
            }
            return Ok(Decision::with_dest(
                ALLOW,
                risk,
                format!("已有授权 (granted: {})", field_str(&g, "scope")),
                dest,
            )
            .grant(&field_str(&g, "id")));
        }
    }
    let reason = if reason.is_empty() {
        "高风险操作需要你的批准 (high-risk action)".to_string()
    } else {
        reason
    };
    let mut d = Decision::with_dest(ASK, risk, reason, dest);
    d.per_use = per_use || PER_USE_TOOLS.contains(&tool);
    d.injected = !injection.is_empty();
    Ok(d)
}

/// MCP tools: the per-tool mode the user set (auto / ask / off) plus taint & injection escalation.
fn decide_mcp(
    store: &Store,
    tool: &str,
    t: &Value,
    args: &Value,
    task_id: &str,
) -> Result<Decision> {
    let t_risk = get_str(t, "risk");
    let Some((s, rec)) = hooks().mcp_tool_info(store, tool) else {
        return Ok(Decision::new(
            DENY,
            "high",
            "这个 MCP 工具已被移除 (tool removed)",
        ));
    };
    let s_name = get_str(&s, "name");
    let rec_name = get_str(&rec, "name");
    if !s.get("enabled").map(truthy).unwrap_or(true) {
        return Ok(Decision::new(
            DENY,
            &t_risk,
            format!("MCP 服务器「{s_name}」已停用 (server disabled in Connections)"),
        ));
    }
    if get_str(&rec, "mode") == "off" {
        return Ok(Decision::new(
            DENY,
            &t_risk,
            format!("MCP 工具「{rec_name}」已关闭 (tool off in Connections)"),
        ));
    }
    if get_str(&rec, "status") != "ok" {
        return Ok(Decision::new(DENY, "high", format!("MCP 工具「{rec_name}」的定义有变化，需要你在「连接」页检查后才能使用 (tool definition changed — review it in Connections)")));
    }
    let read = get_str(&rec, "kind") == "read";
    let mut risk = if read {
        "low".to_string()
    } else {
        "medium".to_string()
    };
    let mut reasons: Vec<String> = Vec::new();
    let ctx = store.task_ctx(task_id)?;
    let injection = str_list(ctx.get("injection"));
    let taint = guard::level(&field_str(&ctx, "taint"));
    let tainted = taint >= guard::level(guard::CONFIDENTIAL);
    let size = if truthy(args) {
        py_repr(args).chars().count()
    } else {
        0
    };
    let data_class = {
        let d = get_str(&s, "data_class");
        if d.is_empty() {
            guard::CONFIDENTIAL.to_string()
        } else {
            d
        }
    };
    let class_level = guard::level_or(&data_class, 2);
    if get_str(&rec, "mode") == "ask" {
        risk = "high".to_string();
        reasons.push(format!(
            "你设置了「{s_name} · {rec_name}」每次都需要审批 (set to ask every time)"
        ));
    }
    if !read {
        reasons.push(format!("会修改「{s_name}」里的数据 (writes to {s_name})"));
        if tainted && taint > class_level {
            bump(&mut risk, "high");
            reasons.push(
                "本任务读取过更机密的数据，现在要写入一个外部服务 (data egress check)".into(),
            );
        } else if tainted && size > 300 {
            bump(&mut risk, "high");
            reasons.push(
                "本任务读取过机密数据（邮件等），且这次要写入较多内容 (possible data egress)"
                    .into(),
            );
        }
    } else if tainted && size > 500 && taint > class_level {
        bump(&mut risk, "high");
        reasons.push("本任务读取过机密数据，这次查询参数很长，可能把数据带出去 (possible data egress in query)".into());
    }
    if !injection.is_empty() && !read {
        bump(&mut risk, "high");
        reasons.push(format!(
            "本任务读到的外部内容疑似包含提示注入，所有写操作需要人工确认 (prompt-injection suspected: {})",
            injection.join(", ")
        ));
    }
    finish(
        store,
        tool,
        task_id,
        &risk,
        &reasons,
        &get_str(&s, "id"),
        &injection,
        false,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Audit;

    fn store() -> (tempfile::TempDir, Store) {
        let d = tempfile::tempdir().unwrap();
        let s = Store::new(d.path()).unwrap();
        (d, s)
    }

    fn dec(
        store: &Store,
        tool: &str,
        args: Value,
        task: &str,
        elem: Option<Value>,
        page: Option<Value>,
    ) -> Decision {
        decide(store, tool, &args, task, elem.as_ref(), page.as_ref(), true).unwrap()
    }

    fn d(store: &Store, tool: &str, args: Value, task: &str) -> String {
        dec(store, tool, args, task, None, None).decision
    }

    fn card(store: &Store, label: &str, domains: &str) -> String {
        vault::save_item(
            store,
            &json!({"kind": "card", "label": label, "domains": domains,
                    "values": {"number": "4242 4242 4242 4242", "expiry": "12/28", "cvc": "123", "holder": "LU LIANG"}}),
            None,
        )
        .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_string()
    }

    #[test]
    fn gmail() {
        let (_t, store) = store();
        assert_eq!(
            decide(
                &store,
                "gmail_search",
                &json!({"query": "x"}),
                "t1",
                None,
                None,
                false
            )
            .unwrap()
            .decision,
            DENY
        );
        assert_eq!(
            d(&store, "gmail_search", json!({"query": "x"}), "t1"),
            ALLOW
        );
        let r = dec(
            &store,
            "gmail_send",
            json!({"to": "john@x.com", "body": "hi"}),
            "t1",
            None,
            None,
        );
        assert_eq!(r.decision, ASK);
        assert_eq!(r.risk, "high");
        assert_eq!(r.destination, "john@x.com");
        assert_eq!(r.reason, "对外发送邮件 (sends email on your behalf)");
        assert_eq!(
            d(
                &store,
                "gmail_create_draft",
                json!({"to": "a@b.c", "body": "x"}),
                "t1"
            ),
            ALLOW
        );
        store
            .save_connection("gmail", None, Some(&json!({"send": false})), None)
            .unwrap();
        let r = dec(
            &store,
            "gmail_send",
            json!({"to": "john@x.com", "body": "hi"}),
            "t1",
            None,
            None,
        );
        assert_eq!(r.decision, DENY);
        assert_eq!(
            r.reason,
            "权限「gmail.send」已关闭 (permission off in Connections)"
        );
        assert_eq!(
            dec(&store, "nope", json!({}), "t1", None, None).reason,
            "未知工具 unknown tool: nope"
        );
        store
            .save_connection("gmail", None, Some(&json!({"send": true})), None)
            .unwrap();
        let r = dec(
            &store,
            "gmail_unsubscribe",
            json!({"message_ids": ["1", "2"]}),
            "t1",
            None,
            None,
        );
        assert_eq!(r.decision, ASK);
        assert!(r.reason.contains('2'));
        assert_eq!(
            d(&store, "notify_telegram", json!({"text": "x"}), "t1"),
            DENY
        ); // connector disabled
    }

    #[test]
    fn bulk_archive_needs_approval() {
        let (_t, store) = store();
        let few = json!(["1", "2", "3"]);
        let many: Value = (0..12).map(|i| json!(i.to_string())).collect();
        assert_eq!(
            d(&store, "gmail_archive", json!({"message_ids": few}), "t1"),
            ALLOW
        );
        let r = dec(
            &store,
            "gmail_archive",
            json!({"message_ids": many}),
            "t1",
            None,
            None,
        );
        assert_eq!(r.decision, ASK);
        assert!(r.reason.contains("12"));
        assert_eq!(
            d(
                &store,
                "gmail_label",
                json!({"message_ids": many, "remove_labels": ["\\Inbox"]}),
                "t1"
            ),
            ASK
        );
        assert_eq!(
            d(
                &store,
                "gmail_label",
                json!({"message_ids": many, "add_labels": ["Receipts"]}),
                "t1"
            ),
            ALLOW
        );
        for _ in 0..2 {
            // small batches add up: the third archive call in a task asks
            store
                .audit(
                    "sentinel",
                    "gmail_archive",
                    Audit {
                        task_id: "t9".into(),
                        result: "success".into(),
                        ..Default::default()
                    },
                )
                .unwrap();
        }
        assert_eq!(
            d(&store, "gmail_archive", json!({"message_ids": few}), "t9"),
            ASK
        );
    }

    #[test]
    fn grants() {
        let (_t, store) = store();
        let gid = store
            .add_grant(
                "gmail_send",
                "TASK",
                Some("t1"),
                Some(&json!({"destination": "john@x.com"})),
                None,
                "",
            )
            .unwrap();
        let r = dec(
            &store,
            "gmail_send",
            json!({"to": "john@x.com", "body": "x"}),
            "t1",
            None,
            None,
        );
        assert_eq!(r.decision, ALLOW);
        assert_eq!(r.grant_id, gid);
        assert_eq!(r.reason, "已有授权 (granted: TASK)");
        assert_eq!(
            d(
                &store,
                "gmail_send",
                json!({"to": "john@x.com", "body": "x"}),
                "t2"
            ),
            ASK
        );
        assert_eq!(
            d(
                &store,
                "gmail_send",
                json!({"to": "eve@x.com", "body": "x"}),
                "t1"
            ),
            ASK
        );
        store
            .update_task_ctx("t1", None, Some(&["ignore-previous".to_string()]), None)
            .unwrap();
        let r = dec(
            &store,
            "gmail_send",
            json!({"to": "john@x.com", "body": "x"}),
            "t1",
            None,
            None,
        );
        assert_eq!(r.decision, ASK); // grants ignored
        assert!(r
            .reason
            .contains("prompt-injection suspected: ignore-previous"));
    }

    #[test]
    fn browser() {
        let (_t, store) = store();
        let page = json!({"url": "https://shop.com/cart", "title": "Cart"});
        assert_eq!(
            dec(
                &store,
                "browser_click",
                json!({"ref": "e1"}),
                "t",
                Some(json!({"name": "Buy now", "role": "button"})),
                Some(page.clone())
            )
            .decision,
            ASK
        );
        assert_eq!(
            dec(
                &store,
                "browser_click",
                json!({"ref": "e1"}),
                "t",
                Some(json!({"name": "Details", "role": "link"})),
                Some(page.clone())
            )
            .decision,
            ALLOW
        );
        assert_eq!(
            dec(
                &store,
                "browser_type",
                json!({"ref": "e2", "text": "x"}),
                "t",
                Some(json!({"input_type": "password"})),
                Some(page.clone())
            )
            .decision,
            DENY
        );
        assert_eq!(
            dec(
                &store,
                "browser_type",
                json!({"ref": "e2", "text": "shoes", "submit": true}),
                "t",
                Some(json!({"role": "searchbox", "name": "Search"})),
                Some(page.clone())
            )
            .decision,
            ALLOW
        );
        let r = dec(
            &store,
            "browser_type",
            json!({"ref": "e2", "text": "hello", "submit": true}),
            "t",
            Some(json!({"role": "textbox", "name": "Message"})),
            Some(page.clone()),
        );
        assert_eq!(r.decision, ASK);
        assert_eq!(r.reason, "输入后会按回车提交表单 (submits a form)");
        assert_eq!(r.destination, "shop.com");
        // a <button> defaults to type=submit; outside a form (chat launcher) it's an ordinary click, inside a form it's not
        assert_eq!(dec(&store, "browser_click", json!({"ref": "e3"}), "t", Some(json!({"name": "Open chat widget", "role": "button", "input_type": "submit", "in_form": false})), Some(page.clone())).decision, ALLOW);
        assert_eq!(
            dec(
                &store,
                "browser_click",
                json!({"ref": "e3"}),
                "t",
                Some(
                    json!({"name": "Go", "role": "button", "input_type": "submit", "in_form": true})
                ),
                Some(page.clone())
            )
            .decision,
            ASK
        );
        let r = dec(
            &store,
            "browser_navigate",
            json!({"url": "http://192.168.1.1/"}),
            "t",
            None,
            None,
        );
        assert_eq!(r.decision, DENY);
        assert_eq!(
            r.reason,
            "禁止访问内网/本机地址 (private or cluster-internal address blocked)"
        );
        // taint: after reading email, navigating to a new domain with data in the URL asks
        store
            .update_task_ctx("t", Some("CONFIDENTIAL"), None, None)
            .unwrap();
        assert_eq!(
            d(
                &store,
                "browser_navigate",
                json!({"url": "https://evil.com/?q=secret"}),
                "t"
            ),
            ASK
        );
        assert_eq!(
            d(
                &store,
                "browser_navigate",
                json!({"url": "https://news.com/"}),
                "t"
            ),
            ALLOW
        );
        store
            .save_connection(
                "browser",
                Some(&json!({"blocked_domains": ["bad.com"]})),
                None,
                None,
            )
            .unwrap();
        let r = dec(
            &store,
            "browser_navigate",
            json!({"url": "https://www.bad.com/"}),
            "t",
            None,
            None,
        );
        assert_eq!(r.decision, DENY);
        assert_eq!(r.reason, "域名 bad.com 在黑名单中 (blocked domain)");
        assert_eq!(
            d(
                &store,
                "browser_read",
                json!({"urls": ["https://news.com/a"]}),
                "t"
            ),
            ALLOW
        );
        assert_eq!(
            d(
                &store,
                "browser_read",
                json!({"urls": ["https://evil.com/?q=secret"]}),
                "t"
            ),
            ASK
        );
        assert_eq!(d(&store, "browser_read", json!({}), "t"), DENY);
        assert_eq!(d(&store, "browser_save_media", json!({}), "t"), DENY);
        assert_eq!(
            d(&store, "browser_save_media", json!({"ref": "e1"}), "t"),
            ALLOW
        );
        assert_eq!(
            dec(
                &store,
                "browser_click_at",
                json!({"x": 1, "y": 2}),
                "t",
                None,
                Some(page.clone())
            )
            .decision,
            ASK
        );
        assert_eq!(
            dec(
                &store,
                "browser_press",
                json!({"key": "Enter"}),
                "t",
                Some(json!({"role": "textbox", "name": "Msg", "in_form": true})),
                Some(page.clone())
            )
            .decision,
            ASK
        );
        assert_eq!(
            dec(
                &store,
                "browser_press",
                json!({"key": "Enter"}),
                "t",
                Some(json!({"role": "searchbox", "name": "Search", "in_form": true})),
                Some(page.clone())
            )
            .decision,
            ALLOW
        );
        let r = dec(
            &store,
            "browser_upload",
            json!({"ref": "e1", "path": "uploads/a.pdf"}),
            "t",
            None,
            Some(page.clone()),
        );
        assert_eq!(r.decision, ASK);
        assert_eq!(r.reason, "上传本地文件到网站 (uploads a local file)");
        store
            .set_user_request("tu", "请在 shop.com 上传我刚附上的文件")
            .unwrap();
        assert_eq!(
            dec(
                &store,
                "browser_upload",
                json!({"ref": "e1", "path": "uploads/a.pdf"}),
                "tu",
                None,
                Some(page.clone())
            )
            .decision,
            ALLOW
        );
    }

    #[test]
    fn standing_click_grant_never_covers_payment() {
        let (_t, store) = store();
        // 2026-10-04: PERMANENT browser_click grant on amazon.sg + "Place your order" -> must still ask, every time
        store
            .add_grant(
                "browser_click",
                "PERMANENT",
                None,
                Some(&json!({"destination": "amazon.sg"})),
                None,
                "",
            )
            .unwrap();
        let page =
            json!({"url": "https://www.amazon.sg/checkout/p/p-251-123/spc", "title": "Checkout"});
        let pay = json!({"tag": "input", "role": "", "name": "Place your order", "input_type": "submit", "in_form": true});
        let r = dec(
            &store,
            "browser_click",
            json!({"ref": "e22"}),
            "t1",
            Some(pay),
            Some(page),
        );
        assert_eq!(r.decision, ASK);
        assert!(r.grant_id.is_empty());
        let ok = json!({"tag": "a", "role": "link", "name": "Your Orders", "input_type": "", "in_form": false});
        assert_eq!(
            dec(
                &store,
                "browser_click",
                json!({"ref": "e5"}),
                "t1",
                Some(ok),
                Some(json!({"url": "https://www.amazon.sg/"}))
            )
            .decision,
            ALLOW
        );
    }

    #[test]
    fn same_site_exfil_after_injection() {
        let (_t, store) = store();
        store
            .update_task_ctx(
                "tj",
                None,
                Some(&["hidden instruction".to_string()]),
                Some("httpbin.org"),
            )
            .unwrap();
        let r = dec(
            &store,
            "browser_navigate",
            json!({"url": "https://httpbin.org/anything/collect?owner_email=a@b.com"}),
            "tj",
            None,
            None,
        );
        assert_eq!(r.decision, ASK);
        assert_eq!(
            d(
                &store,
                "browser_navigate",
                json!({"url": "https://httpbin.org/get"}),
                "tj"
            ),
            ALLOW
        );
    }

    #[test]
    fn order_changes_need_their_own_approval() {
        let (_t, store) = store();
        store
            .add_grant(
                "browser_click",
                "PERMANENT",
                None,
                Some(&json!({"destination": "amazon.sg"})),
                None,
                "",
            )
            .unwrap();
        store
            .set_user_request("tc", "能否把这个订单给取消了？帮忙把这处理一下")
            .unwrap();
        let page = json!({"url": "https://www.amazon.sg/progress-tracker/package/preship/cancel-items", "title": "Cancel items 503-4373109"});
        let btn = json!({"tag": "input", "role": "button", "name": "Request cancellation", "input_type": "submit", "in_form": true});
        let r = dec(
            &store,
            "browser_click",
            json!({"ref": "e62"}),
            "tc",
            Some(btn),
            Some(page),
        );
        assert_eq!(r.decision, ASK);
        assert!(r.grant_id.is_empty());
        assert!(
            r.reason.contains("changes an order") && r.reason.contains("Cancel items 503-4373109")
        );
    }

    #[test]
    fn checkout_details_and_cart_edits_are_not_payments() {
        let (_t, store) = store();
        let rm = json!({"tag": "button", "role": "button", "name": "Remove", "input_type": "", "in_form": false});
        assert_eq!(
            dec(
                &store,
                "browser_click",
                json!({"ref": "e20"}),
                "tr",
                Some(rm.clone()),
                Some(json!({"url": "https://www.decathlon.sg/cart"}))
            )
            .decision,
            ALLOW
        );
        assert_eq!(
            dec(
                &store,
                "browser_click",
                json!({"ref": "e20"}),
                "tr",
                Some(rm),
                Some(json!({"url": "https://mail.example/inbox"}))
            )
            .decision,
            ASK
        );
    }

    #[test]
    fn one_card_purchase_confirmation() {
        let (_t, store) = store();
        let v1 = card(&store, "Visa", "");
        let args = json!({"site": "decathlon.sg", "items": [{"name": "Water flask 0.8L", "qty": 1, "price": 9.0}], "shipping": 4.99,
                          "total": 13.99, "currency": "SGD", "delivery": "Home delivery to 1C Tyersall Rd", "card_item_id": v1});
        let r = dec(&store, "purchase_confirm", args.clone(), "tp", None, None);
        assert_eq!(r.decision, ASK);
        assert_eq!(r.destination, "decathlon.sg");
        assert!(r
            .reason
            .starts_with("确认这次购买：decathlon.sg，总价 SGD 13.99；"));
        let mut bad = args.clone();
        bad["card_item_id"] = json!("v9");
        assert_eq!(
            dec(&store, "purchase_confirm", bad, "tp", None, None).decision,
            DENY
        );
        assert_eq!(
            dec(
                &store,
                "purchase_confirm",
                json!({"site": "x.sg", "total": "abc", "items": [1]}),
                "tp",
                None,
                None
            )
            .decision,
            DENY
        );
        let page = json!({"url": "https://www.decathlon.sg/checkout/abc", "title": "Checkout", "text": "Subtotal $9.00\nShipping $4.99\nOrder total\n$13.99"});
        let pay = json!({"tag": "button", "role": "button", "name": "Place Order", "input_type": "submit", "in_form": true});
        assert_eq!(
            dec(
                &store,
                "browser_click",
                json!({"ref": "e9"}),
                "tp",
                Some(pay.clone()),
                Some(page.clone())
            )
            .decision,
            ASK
        ); // not confirmed yet
        let pid = store
            .add_purchase("tp", "decathlon.sg", &v1, 13.99, "SGD", &json!({}))
            .unwrap();
        let r = dec(
            &store,
            "browser_click",
            json!({"ref": "e9"}),
            "tp",
            Some(pay.clone()),
            Some(page.clone()),
        );
        assert_eq!(r.decision, ALLOW);
        assert_eq!(r.grant_id, pid);
        let box_ = json!({"tag": "input", "role": "textbox", "name": "Card number", "input_type": "text", "in_form": true});
        assert_eq!(
            dec(
                &store,
                "browser_fill_secret",
                json!({"ref": "f1e2", "item_id": v1, "field": "number"}),
                "tp",
                Some(box_.clone()),
                Some(page.clone())
            )
            .decision,
            ALLOW
        );
        assert_ne!(
            dec(
                &store,
                "browser_fill_secret",
                json!({"ref": "f1e2", "item_id": "v2", "field": "number"}),
                "tp",
                Some(box_.clone()),
                Some(page.clone())
            )
            .decision,
            ALLOW
        );
        let mut dear = page.clone();
        dear["text"] = json!("Order total S$48.00");
        let r = dec(
            &store,
            "browser_click",
            json!({"ref": "e9"}),
            "tp",
            Some(pay.clone()),
            Some(dear),
        );
        assert_eq!(r.decision, ASK); // total went up
        assert!(r
            .reason
            .contains("page total 48.00 exceeds the confirmed 13.99"));
        let other = json!({"url": "https://www.lazada.sg/checkout", "title": "x", "text": ""});
        assert_eq!(
            dec(
                &store,
                "browser_click",
                json!({"ref": "e9"}),
                "tp",
                Some(pay.clone()),
                Some(other)
            )
            .decision,
            ASK
        ); // other site
        assert_eq!(
            dec(
                &store,
                "browser_click",
                json!({"ref": "e9"}),
                "tq",
                Some(pay),
                Some(page.clone())
            )
            .decision,
            ASK
        ); // other task
        let cancel = json!({"tag": "button", "role": "button", "name": "Request cancellation", "input_type": "submit", "in_form": true});
        assert_eq!(
            dec(
                &store,
                "browser_click",
                json!({"ref": "e3"}),
                "tp",
                Some(cancel),
                Some(page)
            )
            .decision,
            ASK
        ); // never covered
    }

    #[test]
    fn going_to_checkout_is_not_a_payment() {
        let (_t, store) = store();
        let cart = json!({"url": "https://www.decathlon.sg/cart", "title": "My Cart", "text": ""});
        let co = json!({"url": "https://www.decathlon.sg/checkout/abc", "title": "Checkout", "text": ""});
        for name in [
            "Proceed to Checkout",
            "Checkout",
            "Check out (1)",
            "去结算",
            "结算(2)",
            "Go to checkout",
        ] {
            let b = json!({"tag": "button", "role": "button", "name": name, "input_type": "submit", "in_form": true});
            assert_eq!(
                dec(
                    &store,
                    "browser_click",
                    json!({"ref": "e1"}),
                    "tc",
                    Some(b),
                    Some(cart.clone())
                )
                .decision,
                ALLOW,
                "{name}"
            );
        }
        let nxt = json!({"tag": "button", "role": "button", "name": "Next Step", "input_type": "submit", "in_form": true});
        let mut cont = nxt.clone();
        cont["name"] = json!("Continue to payment");
        assert_eq!(
            dec(
                &store,
                "browser_click",
                json!({"ref": "e2"}),
                "tc",
                Some(nxt.clone()),
                Some(co.clone())
            )
            .decision,
            ALLOW
        );
        assert_eq!(
            dec(
                &store,
                "browser_click",
                json!({"ref": "e2"}),
                "tc",
                Some(cont),
                Some(co.clone())
            )
            .decision,
            ALLOW
        );
        // once a card detail has been filled, the same buttons can pay: approval (or a purchase confirmation) again
        store
            .audit(
                "sentinel",
                "browser_fill_secret",
                Audit {
                    task_id: "tc".into(),
                    resource: "browser".into(),
                    result: "success".into(),
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(
            dec(
                &store,
                "browser_click",
                json!({"ref": "e2"}),
                "tc",
                Some(nxt),
                Some(co)
            )
            .decision,
            ASK
        );
    }

    #[test]
    fn card_details_go_into_their_own_boxes() {
        let (_t, store) = store();
        let v1 = card(&store, "Visa", "");
        let page =
            json!({"url": "https://www.decathlon.sg/checkout/x", "title": "Checkout", "text": ""});
        let box_ = json!({"tag": "input", "role": "textbox", "name": "Card number", "input_type": "text", "in_form": true});
        let r = dec(
            &store,
            "browser_fill_secret",
            json!({"ref": "f7e1", "item_id": v1, "field": "expiry"}),
            "tw",
            Some(box_.clone()),
            Some(page.clone()),
        );
        assert_eq!(r.decision, DENY);
        assert!(r.reason.contains("iframe"));
        store
            .add_purchase("tw", "decathlon.sg", &v1, 6.8, "SGD", &json!({}))
            .unwrap();
        for _ in 0..2 {
            for (f, name) in [
                ("holder", "Name on card"),
                ("number", "Card number"),
                ("expiry", "Expiry date"),
                ("cvc", "Security code"),
            ] {
                let mut b = box_.clone();
                b["name"] = json!(name);
                assert_eq!(
                    dec(
                        &store,
                        "browser_fill_secret",
                        json!({"ref": "f8e1", "item_id": v1, "field": f}),
                        "tw",
                        Some(b),
                        Some(page.clone())
                    )
                    .decision,
                    ALLOW,
                    "{f}"
                );
            }
        }
    }

    #[test]
    fn filter_boxes_need_no_approval() {
        let (_t, store) = store();
        let page = json!({"url": "https://www.decathlon.sg/c/bottles.html", "title": "Bottles"});
        let price = json!({"tag": "input", "role": "spinbutton", "name": "", "input_type": "number", "in_form": true});
        assert_eq!(
            dec(
                &store,
                "browser_type",
                json!({"ref": "e67", "text": "15", "submit": true}),
                "tf",
                Some(price.clone()),
                Some(page.clone())
            )
            .decision,
            ALLOW
        );
        let maxp = json!({"tag": "input", "role": "textbox", "name": "Max price", "input_type": "text", "in_form": true});
        assert_eq!(
            dec(
                &store,
                "browser_type",
                json!({"ref": "e9", "text": "S$15", "submit": true}),
                "tf",
                Some(maxp),
                Some(page.clone())
            )
            .decision,
            ALLOW
        );
        let other = json!({"tag": "input", "role": "textbox", "name": "Message", "input_type": "text", "in_form": true});
        assert_eq!(
            dec(
                &store,
                "browser_type",
                json!({"ref": "e3", "text": "hello", "submit": true}),
                "tf",
                Some(other),
                Some(page)
            )
            .decision,
            ASK
        );
        let co = json!({"url": "https://www.decathlon.sg/checkout/x", "title": "Checkout"});
        assert_eq!(
            dec(
                &store,
                "browser_type",
                json!({"ref": "e67", "text": "2", "submit": true}),
                "tf",
                Some(price),
                Some(co)
            )
            .decision,
            ASK
        );
    }

    #[test]
    fn card_fill_needs_the_purchase_confirmed_first() {
        let (_t, store) = store();
        let v1 = card(&store, "Visa", "");
        let page =
            json!({"url": "https://www.decathlon.sg/checkout/x", "title": "Checkout", "text": ""});
        let box_ = json!({"tag": "input", "role": "textbox", "name": "Card number", "input_type": "text", "in_form": true});
        let r = dec(
            &store,
            "browser_fill_secret",
            json!({"ref": "f9e1", "item_id": v1, "field": "number"}),
            "tk",
            Some(box_.clone()),
            Some(page.clone()),
        );
        assert_eq!(r.decision, DENY);
        assert!(r.reason.contains("purchase_confirm"));
        store
            .add_purchase("tk", "decathlon.sg", &v1, 6.8, "SGD", &json!({}))
            .unwrap();
        assert_eq!(
            dec(
                &store,
                "browser_fill_secret",
                json!({"ref": "f9e1", "item_id": v1, "field": "number"}),
                "tk",
                Some(box_.clone()),
                Some(page.clone())
            )
            .decision,
            ALLOW
        );
        // a passport number (not a card) still asks per use
        let p1 = vault::save_item(
            &store,
            &json!({"kind": "id_document", "label": "Passport", "values": {"number": "E1234512"}}),
            None,
        )
        .unwrap();
        let mut pp = box_;
        pp["name"] = json!("Passport number");
        let r = dec(
            &store,
            "browser_fill_secret",
            json!({"ref": "e4", "item_id": p1["id"], "field": "number"}),
            "tk2",
            Some(pp),
            Some(page),
        );
        assert_eq!(r.decision, ASK);
        assert_eq!(r.reason, "把保险箱里的「Passport · 号码 number · •••• 4512」填到 decathlon.sg 的输入框（每次使用都需要你批准）(fills a vault value — approved per use)");
    }

    #[test]
    fn vault_fill_policy() {
        let (_t, store) = store();
        let it = card(&store, "Visa", "shop.test");
        let args = json!({"ref": "e1", "item_id": it, "field": "number"});
        let el = json!({"tag": "input", "input_type": "text", "name": "Card number"});
        let r = dec(
            &store,
            "browser_fill_secret",
            args.clone(),
            "t1",
            Some(el.clone()),
            Some(json!({"url": "https://shop.test/pay", "title": ""})),
        );
        assert_eq!(r.decision, DENY); // a card needs the purchase confirmed first (0.2.68)
        assert!(r.reason.contains("purchase_confirm"));
        let mem = vault::save_item(&store, &json!({"kind": "membership", "label": "KrisFlyer", "domains": "shop.test",
                                                  "values": {"number": "8800 1111 2222 1234", "name": "LU LIANG"}}), None).unwrap();
        let margs = json!({"ref": "e1", "item_id": mem["id"], "field": "number"});
        let mel = json!({"tag": "input", "input_type": "text", "name": "Membership number"});
        let r = dec(
            &store,
            "browser_fill_secret",
            margs.clone(),
            "t1",
            Some(mel.clone()),
            Some(json!({"url": "https://shop.test/pay", "title": ""})),
        );
        assert_eq!(r.decision, ASK);
        assert!(r.reason.contains("•••• 1234") && !r.reason.contains("8800"));
        let r = dec(
            &store,
            "browser_fill_secret",
            args.clone(),
            "t1",
            Some(el.clone()),
            Some(json!({"url": "https://evil.example/pay"})),
        );
        assert_eq!(r.decision, DENY);
        assert_eq!(r.reason, "「Visa」只允许在 shop.test 使用，当前网站是 evil.example (item restricted to other sites)");
        assert_eq!(
            dec(
                &store,
                "browser_fill_secret",
                args.clone(),
                "t1",
                Some(json!({"tag": "input", "input_type": "password"})),
                Some(json!({"url": "https://shop.test/"}))
            )
            .decision,
            DENY
        );
        let mut pin = args.clone();
        pin["field"] = json!("pin");
        let r = dec(
            &store,
            "browser_fill_secret",
            pin,
            "t1",
            Some(el.clone()),
            Some(json!({"url": "https://shop.test/"})),
        );
        assert_eq!(r.decision, DENY);
        assert_eq!(
            r.reason,
            "「Visa」没有字段 pin，可用字段：number, expiry, cvc, holder (unknown field)"
        );
        assert_eq!(
            dec(
                &store,
                "browser_fill_secret",
                args.clone(),
                "t1",
                None,
                None
            )
            .decision,
            DENY
        );
        assert_eq!(
            dec(
                &store,
                "browser_fill_secret",
                args,
                "t1",
                Some(json!({"tag": "div", "name": "Card number"})),
                Some(json!({"url": "https://shop.test/"}))
            )
            .decision,
            DENY
        );
        store
            .add_grant(
                "browser_fill_secret",
                "PERMANENT",
                None,
                Some(&json!({})),
                None,
                "",
            )
            .unwrap(); // grants never cover vault fills
        assert!(PER_USE_TOOLS.contains(&"browser_fill_secret"));
        assert_eq!(
            dec(
                &store,
                "browser_fill_secret",
                margs.clone(),
                "t1",
                Some(mel.clone()),
                Some(json!({"url": "https://shop.test/pay"}))
            )
            .decision,
            ASK
        );
        let r = dec(
            &store,
            "browser_fill_secret",
            margs,
            "t1",
            Some(mel),
            Some(json!({"url": "http://shop.example/pay"})),
        );
        assert_eq!(r.decision, DENY); // restricted to shop.test
        let any = vault::save_item(
            &store,
            &json!({"kind": "other", "label": "Code", "values": {"value": "abcdef123"}}),
            None,
        )
        .unwrap();
        let r = dec(
            &store,
            "browser_fill_secret",
            json!({"ref": "e1", "item_id": any["id"], "field": "value"}),
            "t1",
            Some(json!({"tag": "textarea"})),
            Some(json!({"url": "http://shop.example/pay"})),
        );
        assert_eq!(r.decision, ASK);
        assert!(r
            .reason
            .ends_with("；⚠ 这个页面不是 HTTPS 加密连接 (page is not HTTPS)"));
    }

    #[test]
    fn calendar() {
        let (_t, store) = store();
        assert_eq!(d(&store, "calendar_list_events", json!({}), "t1"), DENY); // connector disabled
        store
            .save_connection(
                "calendar",
                Some(&json!({"email": "me@x.com"})),
                None,
                Some(true),
            )
            .unwrap();
        let r = dec(&store, "calendar_list_events", json!({}), "t1", None, None);
        assert_eq!(r.decision, DENY); // not connected
        assert_eq!(
            r.reason,
            "Google 日历尚未连接：请在「连接 Connections」页设置 (not connected)"
        );
        store
            .put_secret(
                "calendar",
                &json!({"client_id": "c", "client_secret": "s", "refresh_token": "r"}),
                None,
            )
            .unwrap();
        assert_eq!(d(&store, "calendar_list_events", json!({}), "t1"), ALLOW);
        assert_eq!(
            d(
                &store,
                "calendar_create_event",
                json!({"title": "x", "start": "2026-10-03 19:00"}),
                "t1"
            ),
            ALLOW
        );
        assert_eq!(
            d(
                &store,
                "calendar_create_event",
                json!({"title": "x", "start": "2026-10-03 19:00", "attendees": ["a@b.com"]}),
                "t1"
            ),
            ASK
        );
        assert_eq!(
            d(
                &store,
                "calendar_update_event",
                json!({"event_id": "e"}),
                "t1"
            ),
            ASK
        );
        assert_eq!(
            d(
                &store,
                "calendar_delete_event",
                json!({"event_id": "e"}),
                "t1"
            ),
            ASK
        );
        store
            .save_connection("calendar", None, Some(&json!({"write": false})), None)
            .unwrap();
        assert_eq!(
            d(
                &store,
                "calendar_create_event",
                json!({"title": "x", "start": "2026-10-03"}),
                "t1"
            ),
            DENY
        );
        store
            .save_connection("calendar", None, Some(&json!({"write": true})), None)
            .unwrap();
        store
            .put_secret("calendar", &json!({"ical_url": "https://x/cal.ics"}), None)
            .unwrap();
        let r = dec(
            &store,
            "calendar_create_event",
            json!({"title": "x", "start": "2026-10-03"}),
            "t1",
            None,
            None,
        );
        assert_eq!(r.decision, DENY);
        assert_eq!(r.reason, ICAL_READ_ONLY);
        assert_eq!(d(&store, "calendar_list_events", json!({}), "t1"), ALLOW);
    }

    #[test]
    fn slack_notion_phone() {
        let (_t, store) = store();
        store
            .save_connection("slack", None, None, Some(true))
            .unwrap();
        store
            .save_connection("notion", None, None, Some(true))
            .unwrap();
        store
            .save_connection("phone", None, None, Some(true))
            .unwrap();
        let r = dec(
            &store,
            "slack_send_message",
            json!({"channel": "#General", "text": "hi"}),
            "t1",
            None,
            None,
        );
        assert_eq!(r.decision, DENY);
        assert_eq!(
            r.reason,
            "Slack 尚未连接：请在「连接 Connections」页设置 (not connected)"
        );
        store
            .put_secret("slack", &json!({"token": "x"}), None)
            .unwrap();
        let r = dec(
            &store,
            "slack_send_message",
            json!({"channel": "#General", "text": "hi"}),
            "t1",
            None,
            None,
        );
        assert_eq!(r.decision, ASK);
        assert_eq!(r.destination, "slack:general");
        assert_eq!(
            dec(&store, "notion_search", json!({}), "t1", None, None).reason,
            "Notion 尚未连接：请在「连接 Connections」页设置 (not connected)"
        );
        store
            .put_secret("notion", &json!({"token": "x"}), None)
            .unwrap();
        assert_eq!(
            d(&store, "notion_update_page", json!({"page_id": "p"}), "t1"),
            ALLOW
        );
        let r = dec(
            &store,
            "notion_update_page",
            json!({"page_id": "p", "archived": true}),
            "t1",
            None,
            None,
        );
        assert_eq!(r.decision, ASK);
        assert_eq!(r.destination, "notion:p");
        let r = dec(
            &store,
            "phone_call",
            json!({"to": "+65 6123 4567", "purpose": "book a table for two tonight"}),
            "t1",
            None,
            None,
        );
        assert_eq!(r.decision, DENY);
        assert_eq!(r.reason, "电话功能还没配置好：请在「连接 → 电话」连接 DialMCP 或填写 Telnyx 设置 (phone not configured)");
        assert_eq!(
            destination(
                "phone_call",
                &json!({"to": "00 65 (6123)-4567"}),
                None,
                None
            ),
            "tel:+6561234567"
        );
        assert_eq!(phone_normalize("+1 (415) 555.0100"), "+14155550100");
        assert_eq!(
            destination(
                "gmail_send",
                &json!({"to": "B@x.com, a@x.com", "cc": ""}),
                None,
                None
            ),
            "a@x.com,b@x.com"
        );
        assert_eq!(
            destination("gmail_reply", &json!({"message_id": "m1"}), None, None),
            "reply:m1"
        );
        assert_eq!(
            destination(
                "browser_click",
                &json!({}),
                None,
                Some(&json!({"url": "https://www.shop.sg/x"}))
            ),
            "shop.sg"
        );
    }

    #[test]
    fn mcp_branch_without_hooks() {
        let (_t, store) = store();
        catalog::insert(
            "mcp_t__x",
            json!({"connector": "mcp:t", "capability": "read", "operation": "read", "risk": "low",
                                           "data_class": "CONFIDENTIAL", "description": "d", "parameters": {"type": "object", "properties": {}}}),
        );
        let r = dec(&store, "mcp_t__x", json!({}), "t1", None, None);
        assert_eq!(r.decision, DENY);
        assert_eq!(r.reason, "这个 MCP 工具已被移除 (tool removed)");
        catalog::remove("mcp_t__x");
    }

    #[test]
    fn travel_without_a_key_says_how_to_connect_and_a_server_default_counts() {
        let (_t, store) = store();
        let args = json!({"origin": "SIN", "destination": "NRT", "date": "2026-11-20"});
        // the searches run without a key (flights: Kiwi.com; hotels and places: web search); the detail tools need it
        let r = decide(&store, "flights_search", &args, "t1", None, None, false).unwrap();
        assert_eq!(r.decision, ALLOW, "{}", r.reason);
        let hargs = json!({"city": "Tokyo", "check_in": "2026-11-20", "check_out": "2026-11-23"});
        let r = decide(&store, "hotels_search", &hargs, "t1", None, None, false).unwrap();
        assert_eq!(r.decision, ALLOW, "{}", r.reason);
        let r = decide(
            &store,
            "places_search",
            &json!({"query": "ramen", "city": "Tokyo"}),
            "t1",
            None,
            None,
            false,
        )
        .unwrap();
        assert_eq!(r.decision, ALLOW, "{}", r.reason);
        let r = decide(
            &store,
            "hotel_rooms",
            &json!({"hotel_id": "ht_1", "check_in": "2026-11-20", "check_out": "2026-11-23"}),
            "t1",
            None,
            None,
            false,
        )
        .unwrap();
        assert_eq!(r.decision, DENY);
        assert!(
            r.reason.contains("SerpApi")
                && r.reason.contains("web search")
                && r.reason.contains("flights work without it")
        );
        // the server's default key (seeded into the connection config at startup) connects the hotels too
        store
            .save_connection(
                "travel",
                Some(&json!({"default_key": true})),
                None,
                Some(true),
            )
            .unwrap();
        let r = decide(
            &store,
            "hotel_rooms",
            &json!({"hotel_id": "ht_1", "check_in": "2026-11-20", "check_out": "2026-11-23"}),
            "t1",
            None,
            None,
            false,
        )
        .unwrap();
        assert_eq!(r.decision, ALLOW, "{}", r.reason);
    }
}

#[cfg(test)]
mod browser_auto_tests {
    use super::*;

    #[test]
    fn browser_auto_ok_scope() {
        let ask = |tool: &str, per_use: bool, injected: bool| {
            let mut d = Decision::new(ASK, "high", "r");
            d.per_use = per_use;
            d.injected = injected;
            d.browser_auto_ok(tool)
        };
        assert!(ask("browser_click", false, false));
        assert!(ask("browser_navigate", false, false));
        assert!(ask("browser_type", false, false));
        assert!(!ask("browser_fill_secret", true, false));
        assert!(!ask("browser_upload", false, false));
        assert!(!ask("browser_click", true, false)); // pay / cancel-order clicks
        assert!(!ask("browser_click", false, true)); // after a suspected prompt injection
        assert!(!ask("gmail_send", false, false));
        assert!(!ask("purchase_confirm", true, false));
        assert!(!Decision::new(ALLOW, "low", "").browser_auto_ok("browser_click"));
    }
}
