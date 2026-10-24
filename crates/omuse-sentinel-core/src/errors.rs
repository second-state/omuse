//! `ActionError` of `app/sentinel/actions.py`: the error a connector action raises; `status` is the
//! machine-readable status the runtime sees (`"error"` by default, e.g. `"denied"`, `"blocked"`).

use std::fmt;

#[derive(Debug, Clone)]
pub struct ActionError {
    pub msg: String,
    pub status: String,
}

impl ActionError {
    pub fn new(msg: impl Into<String>) -> Self {
        Self {
            msg: msg.into(),
            status: "error".into(),
        }
    }
    pub fn with_status(msg: impl Into<String>, status: impl Into<String>) -> Self {
        Self {
            msg: msg.into(),
            status: status.into(),
        }
    }
}

impl fmt::Display for ActionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.msg)
    }
}
impl std::error::Error for ActionError {}
