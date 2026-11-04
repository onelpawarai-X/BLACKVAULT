//! BLACKVAULT — cryptography and secret storage.
//!
//! Design rules:
//!   * A PIN, pattern or passphrase is *never* stored, in any form, as plain
//!     text. We store a PBKDF2-HMAC-SHA256 hash with a per-install random salt
//!     and a high iteration count, and compare in constant time.
//!   * The per-install data-encryption key (used for anything reversible) is
//!     sealed with Windows DPAPI under the current user. On non-Windows hosts
//!     (development, CI) we fall back to an in-memory key so the crate builds
//!     and the pure logic stays testable — that fallback is never used to
//!     protect real secrets on Windows.

use base64::Engine;
use hmac::{Hmac, Mac};
use rand::RngCore;
use sha2::Sha256;
use subtle::ConstantTimeEq;
use zeroize::Zeroize;

use crate::error::{BvError, BvResult};

type HmacSha256 = Hmac<Sha256>;

const PBKDF2_ITERS: u32 = 210_000;
const KEY_LEN: usize = 32;
const SALT_LEN: usize = 16;

/// A salted PBKDF2 hash, base64-encoded for storage in a single TEXT column.
/// Format: `pbkdf2$<iters>$<salt_b64>$<hash_b64>`.
pub fn hash_secret(secret: &str) -> BvResult<String> {
    let mut salt = [0u8; SALT_LEN];
    rand::thread_rng().fill_bytes(&mut salt);
    let mut out = [0u8; KEY_LEN];
    pbkdf2::pbkdf2_hmac::<Sha256>(secret.as_bytes(), &salt, PBKDF2_ITERS, &mut out);
    let s = format!(
        "pbkdf2${}${}${}",
        PBKDF2_ITERS,
        base64::engine::general_purpose::STANDARD.encode(salt),
        base64::engine::general_purpose::STANDARD.encode(out)
    );
    out.zeroize();
    Ok(s)
}

/// Constant-time verification of a candidate against a stored hash.
pub fn verify_secret(candidate: &str, stored: &str) -> BvResult<bool> {
    let parts: Vec<&str> = stored.split('$').collect();
    if parts.len() != 4 || parts[0] != "pbkdf2" {
        return Err(BvError::Crypto);
    }
    let iters: u32 = parts[1].parse().map_err(|_| BvError::Crypto)?;
    let salt = base64::engine::general_purpose::STANDARD
        .decode(parts[2])
        .map_err(|_| BvError::Crypto)?;
    let expected = base64::engine::general_purpose::STANDARD
        .decode(parts[3])
        .map_err(|_| BvError::Crypto)?;
    let mut got = vec![0u8; expected.len()];
    pbkdf2::pbkdf2_hmac::<Sha256>(candidate.as_bytes(), &salt, iters, &mut got);
    let equal = got.ct_eq(&expected).into();
    got.zeroize();
    Ok(equal)
}

/// Random URL-safe token, used for one-shot unlock hand-off between the core
/// and the frontend so a stale UI cannot complete an unlock.
pub fn random_token() -> String {
    let mut b = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut b);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(b)
}

/// Keyed hash used for the voiceprint template so the raw feature vector is
/// never recoverable from the database.
pub fn keyed_digest(key: &[u8], data: &[u8]) -> Vec<u8> {
    let mut mac = <HmacSha256 as Mac>::new_from_slice(key).expect("hmac accepts any key length");
    mac.update(data);
    mac.finalize().into_bytes().to_vec()
}

// ------------------------------------------------------------------ DPAPI

#[cfg(windows)]
mod platform {
    use super::*;
    use windows::Win32::Foundation::LocalFree;
    use windows::Win32::Security::Cryptography::{
        CryptProtectData, CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
    };

    fn blob(data: &[u8]) -> CRYPT_INTEGER_BLOB {
        CRYPT_INTEGER_BLOB {
            cbData: data.len() as u32,
            pbData: data.as_ptr() as *mut u8,
        }
    }

    /// Copy a DPAPI output blob into a `Vec` and release the system buffer.
    unsafe fn take(output: CRYPT_INTEGER_BLOB) -> Vec<u8> {
        let out = std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec();
        let _ = LocalFree(windows::Win32::Foundation::HLOCAL(output.pbData as *mut _));
        out
    }

    /// Seal bytes with DPAPI (current user scope, no UI prompt).
    pub fn protect(plain: &[u8]) -> BvResult<Vec<u8>> {
        unsafe {
            let input = blob(plain);
            let mut output = CRYPT_INTEGER_BLOB::default();
            CryptProtectData(
                &input,
                windows::core::PCWSTR::null(),
                None,
                None,
                None,
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
            .map_err(|_| BvError::Crypto)?;
            Ok(take(output))
        }
    }

    /// Reverse of `protect`.
    pub fn unprotect(sealed: &[u8]) -> BvResult<Vec<u8>> {
        unsafe {
            let input = blob(sealed);
            let mut output = CRYPT_INTEGER_BLOB::default();
            CryptUnprotectData(
                &input,
                None,
                None,
                None,
                None,
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
            .map_err(|_| BvError::Crypto)?;
            Ok(take(output))
        }
    }

    pub fn available() -> bool {
        true
    }
}

#[cfg(not(windows))]
mod platform {
    use super::*;
    use parking_lot::Mutex;

    // Development-only stand-in. Never protects real user secrets: it exists so
    // the crate compiles and the pure logic is testable off-Windows.
    fn key() -> &'static Mutex<Vec<u8>> {
        static K: std::sync::OnceLock<Mutex<Vec<u8>>> = std::sync::OnceLock::new();
        K.get_or_init(|| {
            let mut k = vec![0u8; 32];
            rand::thread_rng().fill_bytes(&mut k);
            Mutex::new(k)
        })
    }

    pub fn protect(plain: &[u8]) -> BvResult<Vec<u8>> {
        let k = key().lock();
        let mut out = plain.to_vec();
        for (i, b) in out.iter_mut().enumerate() {
            *b ^= k[i % k.len()];
        }
        Ok(out)
    }

    pub fn unprotect(sealed: &[u8]) -> BvResult<Vec<u8>> {
        protect(sealed)
    }

    pub fn available() -> bool {
        false
    }
}

pub use platform::{available as dpapi_available, protect as dpapi_protect, unprotect as dpapi_unprotect};

/// Seal a string and base64 it, for storage in a TEXT column.
pub fn seal_string(s: &str) -> BvResult<String> {
    let sealed = dpapi_protect(s.as_bytes())?;
    Ok(base64::engine::general_purpose::STANDARD.encode(sealed))
}

/// Reverse of `seal_string`.
pub fn open_string(s: &str) -> BvResult<String> {
    let raw = base64::engine::general_purpose::STANDARD
        .decode(s)
        .map_err(|_| BvError::Crypto)?;
    let plain = dpapi_unprotect(&raw)?;
    String::from_utf8(plain).map_err(|_| BvError::Crypto)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_round_trip() {
        let h = hash_secret("correct horse battery staple").unwrap();
        assert!(h.starts_with("pbkdf2$"));
        assert!(verify_secret("correct horse battery staple", &h).unwrap());
        assert!(!verify_secret("wrong", &h).unwrap());
    }

    #[test]
    fn hashes_are_salted() {
        let a = hash_secret("same input").unwrap();
        let b = hash_secret("same input").unwrap();
        assert_ne!(a, b, "each hash must use a fresh salt");
    }

    #[test]
    fn sealed_strings_round_trip() {
        let s = "voice-identity-key";
        let sealed = seal_string(s).unwrap();
        assert_ne!(sealed, s);
        assert_eq!(open_string(&sealed).unwrap(), s);
    }

    #[test]
    fn malformed_hash_is_rejected() {
        assert!(verify_secret("x", "not-a-hash").is_err());
    }
}
