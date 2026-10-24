//! Email connectors (`app/sentinel/{gmail,mailboxes,mailproviders}.py`): IMAP/SMTP for Gmail and other providers,
//! OAuth2 mail providers, the multi-mailbox layer.

pub mod gmail;
pub mod imap;
pub mod mailboxes;
pub mod mailproviders;
pub mod mime;
pub mod smtp;

pub use gmail::{Gmail, GmailError};
