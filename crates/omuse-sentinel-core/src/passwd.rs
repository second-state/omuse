//! The login password of a standalone (Docker) install.
//!
//! The container starts with the password the host chose (`[auth] password`, formerly `OMUSE_PASSWORD`). The user
//! changes it in Settings; the new one is stored hashed in `<sentinel data>/auth.json` and wins from then on. Until
//! that file exists the install still runs on the default password and the UI says so. On Olares there is no
//! password (the entrance does the login) and none of this applies.

use std::path::{Path, PathBuf};

use pbkdf2::pbkdf2_hmac;
use serde_json::{json, Value};
use sha2::Sha256;
use subtle::ConstantTimeEq;

use omuse_common::pyjson::dumps;
use omuse_common::util::write_atomic;
use omuse_common::Config;

/// PBKDF2 rounds.
pub const ITERATIONS: u32 = 200_000;
/// Shortest / longest password accepted by [`Passwd::set_password`].
pub const MIN_LEN: usize = 8;
pub const MAX_LEN: usize = 128;

/// Python `PasswordError` (message is the bilingual text shown to the user).
#[derive(Debug, thiserror::Error)]
pub enum PasswordError {
    /// Too short / too long.
    #[error("{0}")]
    Invalid(String),
    /// `auth.json` could not be written.
    #[error("{0}")]
    Io(#[from] std::io::Error),
}

/// The login password: the initial one from the config plus the user's own in `auth.json`.
#[derive(Debug, Clone)]
pub struct Passwd {
    enabled: bool,
    user: String,
    initial: String,
    path: PathBuf,
}

impl Passwd {
    /// From the config: `enabled = cfg.auth_enabled()`, user / initial password from `[auth]`,
    /// file `<sentinel data>/auth.json`.
    pub fn from_config(cfg: &Config) -> Self {
        Self::new(
            cfg.auth_enabled(),
            &cfg.auth.user,
            &cfg.auth.password,
            cfg.sentinel_data().join("auth.json"),
        )
    }

    /// Explicit construction (tests, embedding). An empty `user` means `omuse`.
    pub fn new(enabled: bool, user: &str, initial_password: &str, path: PathBuf) -> Self {
        Self {
            enabled,
            user: if user.is_empty() {
                "omuse".to_string()
            } else {
                user.to_string()
            },
            initial: initial_password.to_string(),
            path,
        }
    }

    /// Python `passwd.enabled()`: a password is configured and auth is not off.
    pub fn enabled(&self) -> bool {
        self.enabled
    }

    /// The login name.
    pub fn user(&self) -> &str {
        &self.user
    }

    /// `auth.json`.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// True while the install still runs on the initial password (the user has not set their own).
    pub fn is_default(&self) -> bool {
        !self.path.exists()
    }

    /// Changes whenever the stored password changes (the gate uses it to drop its cache): mtime in ns, 0 without a file.
    pub fn version(&self) -> i64 {
        std::fs::metadata(&self.path)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_nanos() as i64)
            .unwrap_or(0)
    }

    fn load(&self) -> Option<Value> {
        let text = std::fs::read_to_string(&self.path).ok()?;
        let rec: Value = serde_json::from_str(&text).ok()?;
        let ok = rec.get("algo").and_then(Value::as_str) == Some("pbkdf2_sha256")
            && rec
                .get("salt")
                .and_then(Value::as_str)
                .is_some_and(|s| !s.is_empty())
            && rec.get("hash").is_some_and(omuse_common::util::truthy);
        ok.then_some(rec)
    }

    /// Constant-time check against the stored password, else against the initial one.
    pub fn verify(&self, given_user: &str, given_password: &str) -> bool {
        if !self.enabled || !bool::from(given_user.as_bytes().ct_eq(self.user.as_bytes())) {
            return false;
        }
        if let Some(rec) = self.load() {
            let Ok(salt) = hex::decode(rec["salt"].as_str().unwrap_or("")) else {
                return false;
            };
            let iters = rec
                .get("iters")
                .and_then(omuse_common::util::as_i64)
                .filter(|i| *i > 0)
                .unwrap_or(ITERATIONS as i64);
            let mut dk = [0u8; 32];
            pbkdf2_hmac::<Sha256>(given_password.as_bytes(), &salt, iters as u32, &mut dk);
            let want = omuse_common::util::py_str(&rec["hash"]);
            return bool::from(hex::encode(dk).as_bytes().ct_eq(want.as_bytes()));
        }
        bool::from(given_password.as_bytes().ct_eq(self.initial.as_bytes()))
    }

    /// Store a new password (hashed, file readable by this user only). Errors when it is too short / too long.
    pub fn set_password(&self, new: &str) -> Result<(), PasswordError> {
        let n = new.chars().count();
        if n < MIN_LEN {
            return Err(PasswordError::Invalid(format!(
                "密码至少 {MIN_LEN} 个字符 (at least {MIN_LEN} characters)"
            )));
        }
        if n > MAX_LEN {
            return Err(PasswordError::Invalid(format!(
                "密码最多 {MAX_LEN} 个字符 (at most {MAX_LEN} characters)"
            )));
        }
        let salt = {
            use rand::RngCore;
            let mut b = [0u8; 16];
            rand::thread_rng().fill_bytes(&mut b);
            b
        };
        let mut dk = [0u8; 32];
        pbkdf2_hmac::<Sha256>(new.as_bytes(), &salt, ITERATIONS, &mut dk);
        let rec = json!({"algo": "pbkdf2_sha256", "iters": ITERATIONS, "salt": hex::encode(salt), "hash": hex::encode(dk)});
        write_atomic(&self.path, dumps(&rec).as_bytes(), 0o600)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_then_own() {
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("auth.json");
        let off = Passwd::new(false, "omuse", "", path.clone());
        assert!(!off.enabled() && !off.verify("omuse", "x")); // Olares: no login of its own
        let p = Passwd::new(true, "omuse", "initial-pw", path.clone());
        assert!(p.enabled() && p.is_default());
        assert!(
            p.verify("omuse", "initial-pw")
                && !p.verify("omuse", "nope")
                && !p.verify("admin", "initial-pw")
        );
        assert!(
            matches!(p.set_password("short"), Err(PasswordError::Invalid(m)) if m == "密码至少 8 个字符 (at least 8 characters)")
        );
        assert!(
            matches!(p.set_password(&"x".repeat(129)), Err(PasswordError::Invalid(m)) if m == "密码最多 128 个字符 (at most 128 characters)")
        );
        let v0 = p.version();
        p.set_password("my own password").unwrap();
        assert!(!p.is_default() && p.version() != v0);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(!text.contains("my own password")); // hashed, not stored
        assert!(text.starts_with("{\"algo\": \"pbkdf2_sha256\", \"iters\": 200000, \"salt\": \""));
        assert!(p.verify("omuse", "my own password") && !p.verify("omuse", "initial-pw"));
        let off = Passwd::new(false, "omuse", "initial-pw", path);
        assert!(!off.enabled() && !off.verify("omuse", "my own password"));
    }

    #[test]
    fn reads_python_auth_json() {
        // hashlib.pbkdf2_hmac("sha256", b"my own password", bytes.fromhex("00112233445566778899aabbccddeeff"), 200000)
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("auth.json");
        std::fs::write(
            &path,
            r#"{"algo": "pbkdf2_sha256", "iters": 200000, "salt": "00112233445566778899aabbccddeeff", "hash": "16a66fc919058ab240c8dacd1805d081c930eab0aeb18510eca02710766ebf02"}"#,
        )
        .unwrap();
        let p = Passwd::new(true, "", "initial", path);
        assert_eq!(p.user(), "omuse");
        assert!(p.verify("omuse", "my own password"));
        assert!(!p.verify("omuse", "initial"));
    }
}
