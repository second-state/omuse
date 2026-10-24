//! Sentinel's state and policy (`app/sentinel/{store,vault,passwd,catalog,policy,ledger,suggest}.py`):
//! the SQLite store with approvals, grants, taint, connections, secrets and the hash-chained audit log;
//! the vault; the login password; the tool catalog; the ALLOW / DENY / ASK policy engine; the purchase
//! ledger; grant suggestions. No HTTP in here.

pub mod catalog;
pub mod errors;
pub use errors::ActionError;
pub mod guard;
pub mod ledger;
pub mod passwd;
pub mod policy;
pub mod store;
pub mod suggest;
pub mod vault;

pub use policy::{decide, Decision, ALLOW, ASK, DENY};
pub use store::{Audit, Store};
