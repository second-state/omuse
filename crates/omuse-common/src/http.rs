//! Shared outbound HTTP helpers (reqwest clients with the timeouts the Python services used).

use std::time::Duration;

/// A reqwest client with a total timeout and a 10 s connect timeout (`httpx.Timeout(total, connect=10.0)`).
pub fn client(total_secs: f64) -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(Duration::from_secs_f64(total_secs))
        .connect_timeout(Duration::from_secs(10))
        .pool_idle_timeout(Duration::from_secs(3))
        .no_proxy()
        .build()
        .expect("reqwest client")
}

/// Same, with keep-alive connection reuse limited (`httpx.Limits(keepalive_expiry=3.0)` is the default above).
pub fn client_no_timeout() -> reqwest::Client {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .no_proxy()
        .build()
        .expect("reqwest client")
}

/// Internal service headers.
pub const H_RUNTIME: &str = "X-Persona-Runtime";
pub const H_BROWSER: &str = "X-Browser-Token";
pub const H_UI: &str = "X-Persona-UI";
