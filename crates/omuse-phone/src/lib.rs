//! Phone line (app/sentinel/{phone,voice_app,dialmcp}.py): Telnyx calls bridged to the OpenAI realtime API, the voice
//! websocket app, DialMCP calls.
//!
//! * [`phone::Phone`] is the service (one per sentinel, behind `Arc`): config, number rules, call records, dialling,
//!   the Telnyx ⇄ OpenAI Realtime bridge, webhooks and setup checks. `Phone::dial` is the DialMCP line.
//! * [`voice_app::router`] is the public voice app the sentinel serves on `[voice] port`.
//! * The policy hooks (`omuse_sentinel_core::policy::Hooks`) map 1:1 onto `Phone::ready`, `Phone::config`,
//!   `Phone::check_call` and `Phone::calls_today`.

pub mod dialmcp;
pub mod phone;
pub mod voice_app;

pub use dialmcp::{Dial, DialError};
pub use phone::{Phone, PhoneConfig, PhoneError};
