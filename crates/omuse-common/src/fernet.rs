//! Fernet (the `cryptography` package's symmetric token format), so the Rust port reads and writes
//! the same `vault.key` and `secrets` blobs as the Python version. AES-128-CBC + HMAC-SHA256.

use aes::cipher::{block_padding::Pkcs7, BlockDecryptMut, BlockEncryptMut, KeyIvInit};
use anyhow::{anyhow, bail, Result};
use base64::Engine;
use hmac::{Hmac, Mac};
use sha2::Sha256;

type Aes128CbcEnc = cbc::Encryptor<aes::Aes128>;
type Aes128CbcDec = cbc::Decryptor<aes::Aes128>;
type HmacSha256 = Hmac<Sha256>;

const B64: base64::engine::GeneralPurpose = base64::engine::general_purpose::URL_SAFE;

/// A Fernet key (32 bytes: 16 signing + 16 encryption), as the urlsafe-base64 text Python stores.
#[derive(Clone)]
pub struct Fernet {
    signing: [u8; 16],
    encryption: [u8; 16],
}

impl Fernet {
    /// Parse the base64 key text (`Fernet.generate_key()` output).
    pub fn new(key_b64: &str) -> Result<Self> {
        let raw = B64
            .decode(key_b64.trim())
            .or_else(|_| base64::engine::general_purpose::STANDARD.decode(key_b64.trim()))
            .map_err(|e| anyhow!("bad fernet key: {e}"))?;
        if raw.len() != 32 {
            bail!("Fernet key must be 32 url-safe base64-encoded bytes.");
        }
        let mut signing = [0u8; 16];
        let mut encryption = [0u8; 16];
        signing.copy_from_slice(&raw[..16]);
        encryption.copy_from_slice(&raw[16..]);
        Ok(Self {
            signing,
            encryption,
        })
    }

    /// `Fernet.generate_key()`.
    pub fn generate_key() -> String {
        use rand::RngCore;
        let mut k = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut k);
        B64.encode(k)
    }

    /// Encrypt with the current time and a random IV; returns the token text.
    pub fn encrypt(&self, data: &[u8]) -> String {
        use rand::RngCore;
        let mut iv = [0u8; 16];
        rand::thread_rng().fill_bytes(&mut iv);
        let ts = crate::util::now_ts() as u64;
        self.encrypt_at(data, ts, &iv)
    }

    fn encrypt_at(&self, data: &[u8], ts: u64, iv: &[u8; 16]) -> String {
        let ct = Aes128CbcEnc::new(&self.encryption.into(), iv.into())
            .encrypt_padded_vec_mut::<Pkcs7>(data);
        let mut token = Vec::with_capacity(1 + 8 + 16 + ct.len() + 32);
        token.push(0x80);
        token.extend_from_slice(&ts.to_be_bytes());
        token.extend_from_slice(iv);
        token.extend_from_slice(&ct);
        let mut mac = HmacSha256::new_from_slice(&self.signing).expect("hmac");
        mac.update(&token);
        token.extend_from_slice(&mac.finalize().into_bytes());
        B64.encode(token)
    }

    /// Decrypt a token (no TTL check, like `Fernet.decrypt(token)` without `ttl`).
    pub fn decrypt(&self, token: &str) -> Result<Vec<u8>> {
        let raw = B64
            .decode(token.trim())
            .map_err(|_| anyhow!("InvalidToken"))?;
        if raw.len() < 1 + 8 + 16 + 32 || raw[0] != 0x80 {
            bail!("InvalidToken");
        }
        let (body, sig) = raw.split_at(raw.len() - 32);
        let mut mac = HmacSha256::new_from_slice(&self.signing).expect("hmac");
        mac.update(body);
        mac.verify_slice(sig).map_err(|_| anyhow!("InvalidToken"))?;
        let iv: [u8; 16] = body[9..25].try_into().unwrap();
        let ct = &body[25..];
        let pt = Aes128CbcDec::new(&self.encryption.into(), &iv.into())
            .decrypt_padded_vec_mut::<Pkcs7>(ct)
            .map_err(|_| anyhow!("InvalidToken"))?;
        Ok(pt)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let k = Fernet::generate_key();
        let f = Fernet::new(&k).unwrap();
        let t = f.encrypt(b"hello \xe4\xbd\xa0");
        assert_eq!(f.decrypt(&t).unwrap(), b"hello \xe4\xbd\xa0");
        assert!(f.decrypt("garbage").is_err());
    }

    #[test]
    fn python_vector() {
        // cryptography.fernet test vector (key, token, plaintext "hello")
        let f = Fernet::new("cw_0x689RpI-jtRR7oE8h_eQsKImvJapLeSbXpwF4e4=").unwrap();
        let tok = "gAAAAAAdwJ6wAAECAwQFBgcICQoLDA0ODy021cpGVWKZ_eEwCGM4BLLF_5CV9dOPmrhuVUPgJobwOz7JcbmrR64jVmpU4IwqDA==";
        assert_eq!(f.decrypt(tok).unwrap(), b"hello");
        let iv: [u8; 16] = (0u8..16).collect::<Vec<_>>().try_into().unwrap();
        assert_eq!(f.encrypt_at(b"hello", 499162800, &iv), tok);
    }
}
