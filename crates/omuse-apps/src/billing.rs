//! Hosted installs: a "Manage subscription" link to the Stripe customer portal.
//!
//! Enabled only when `[stripe] api_key`, `customer_id` and `subscription_id` are all set (the host that sells this
//! box sets them; Python: OMUSE_STRIPE_API_KEY / _CUSTOMER_ID / _SUBSCRIPTION_ID). OMuse only opens the portal: what
//! happens after the user cancels or renews is handled by whoever receives Stripe's webhooks. The API key stays in
//! Sentinel; the browser only ever gets the short-lived portal URL.

use std::time::Duration;

use omuse_common::config::Config;
use omuse_common::util::{get_str, get_truthy, py_str, take_chars};
use serde_json::{json, Value};

use crate::py::httpx_name;

/// Python `BillingError`.
#[derive(Debug, Clone, thiserror::Error)]
#[error("{0}")]
pub struct BillingError(pub String);

/// `{api_key, customer, subscription}` plus the API base (`[stripe] api`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BillingConfig {
    pub api_key: String,
    pub customer: String,
    pub subscription: String,
    pub api: String,
}

/// The billing settings when all three values are set (trimmed), else `None` (feature off).
pub fn config(cfg: &Config) -> Option<BillingConfig> {
    let s = &cfg.stripe;
    let c = BillingConfig {
        api_key: s.api_key.trim().to_string(),
        customer: s.customer_id.trim().to_string(),
        subscription: s.subscription_id.trim().to_string(),
        api: s.api.trim_end_matches('/').to_string(),
    };
    (!c.api_key.is_empty() && !c.customer.is_empty() && !c.subscription.is_empty()).then_some(c)
}

fn not_configured() -> BillingError {
    BillingError("subscription management is not configured".into())
}

async fn error_text(r: reqwest::Response) -> String {
    let status = r.status().as_u16();
    let body = r.bytes().await.unwrap_or_default();
    match serde_json::from_slice::<Value>(&body) {
        Ok(j) => {
            let msg = j
                .get("error")
                .filter(|e| omuse_common::util::truthy(e))
                .and_then(|e| e.get("message"))
                .map(py_str)
                .unwrap_or_default();
            let msg = if msg == "None" { String::new() } else { msg };
            let m = take_chars(&msg, 300);
            if m.is_empty() {
                format!("HTTP {}", status)
            } else {
                m.to_string()
            }
        }
        Err(_) => format!("HTTP {}", status),
    }
}

fn unreachable(e: reqwest::Error) -> BillingError {
    BillingError(format!(
        "Stripe 无法访问 Stripe unreachable: {}",
        httpx_name(&e)
    ))
}

/// The subscription as Stripe has it: `{status, cancel_at_period_end, current_period_end}`.
pub async fn status(
    client: &reqwest::Client,
    cfg: Option<&BillingConfig>,
) -> Result<Value, BillingError> {
    let cfg = cfg.ok_or_else(not_configured)?;
    let r = client
        .get(format!("{}/v1/subscriptions/{}", cfg.api, cfg.subscription))
        .basic_auth(&cfg.api_key, Some(""))
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .map_err(unreachable)?;
    if r.status().as_u16() >= 400 {
        return Err(BillingError(format!("Stripe: {}", error_text(r).await)));
    }
    let s: Value = r.json().await.map_err(unreachable)?;
    match s.get("customer") {
        None | Some(Value::Null) => {}
        Some(c) if c.as_str() == Some(cfg.customer.as_str()) => {}
        Some(_) => {
            return Err(BillingError(
                "subscription does not belong to the configured customer".into(),
            ))
        }
    }
    let first_item = s
        .get("items")
        .filter(|i| omuse_common::util::truthy(i))
        .and_then(|i| i.get("data"))
        .and_then(Value::as_array)
        .and_then(|a| a.first())
        .cloned()
        .unwrap_or(json!({}));
    let period_end = match s
        .get("current_period_end")
        .filter(|v| omuse_common::util::truthy(v))
    {
        Some(v) => v.clone(),
        None => first_item
            .get("current_period_end")
            .cloned()
            .unwrap_or(Value::Null),
    };
    Ok(
        json!({"status": get_str(&s, "status"), "cancel_at_period_end": get_truthy(&s, "cancel_at_period_end"), "current_period_end": period_end}),
    )
}

/// A fresh customer-portal session URL for the configured customer.
pub async fn portal_url(
    client: &reqwest::Client,
    cfg: Option<&BillingConfig>,
    return_url: &str,
) -> Result<String, BillingError> {
    let cfg = cfg.ok_or_else(not_configured)?;
    let r = client
        .post(format!("{}/v1/billing_portal/sessions", cfg.api))
        .basic_auth(&cfg.api_key, Some(""))
        .form(&[
            ("customer", cfg.customer.as_str()),
            ("return_url", return_url),
        ])
        .timeout(Duration::from_secs(20))
        .send()
        .await
        .map_err(unreachable)?;
    if r.status().as_u16() >= 400 {
        return Err(BillingError(format!("Stripe: {}", error_text(r).await)));
    }
    let j: Value = r.json().await.map_err(unreachable)?;
    let url = get_str(&j, "url");
    if !url.starts_with("https://")
        && !(cfg.api.starts_with("http://") && url.starts_with("http://"))
    {
        return Err(BillingError("Stripe returned no portal URL".into()));
    }
    Ok(url)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn needs_all_three_values() {
        let mut cfg = Config::default();
        cfg.stripe.api = "https://api.stripe.com".into();
        assert!(config(&cfg).is_none());
        for missing in 0..3 {
            let mut c = cfg.clone();
            c.stripe.api_key = if missing == 0 { "".into() } else { "x".into() };
            c.stripe.customer_id = if missing == 1 { "".into() } else { "x".into() };
            c.stripe.subscription_id = if missing == 2 {
                "  ".into()
            } else {
                "x".into()
            };
            assert!(config(&c).is_none(), "{}", missing);
        }
        cfg.stripe.api_key = "sk_test_1 ".into();
        cfg.stripe.customer_id = "cus_1".into();
        cfg.stripe.subscription_id = "sub_1".into();
        let c = config(&cfg).unwrap();
        assert_eq!(
            (
                c.api_key.as_str(),
                c.customer.as_str(),
                c.subscription.as_str()
            ),
            ("sk_test_1", "cus_1", "sub_1")
        );
    }

    #[tokio::test]
    async fn off_when_not_configured() {
        let e = status(&reqwest::Client::new(), None).await.unwrap_err();
        assert_eq!(e.0, "subscription management is not configured");
    }
}
