//! App connectors (app/sentinel/{notion,slack,gcal,ical,market,billing}.py): Notion, Slack, Google Calendar,
//! iCal feeds, market data, Stripe billing.

pub mod billing;
pub mod gcal;
pub mod gdrive;
pub mod ical;
pub mod market;
pub mod notion;
pub mod py;
pub mod pydt;
pub mod slack;
