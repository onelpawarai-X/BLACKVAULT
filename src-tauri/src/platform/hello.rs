//! BLACKVAULT — Windows Hello integration.
//!
//! Uses the WinRT `UserConsentVerifier`, which is the supported way for a
//! desktop application to raise the Windows Hello prompt (fingerprint, face or
//! Windows PIN) for a single verification. We never touch the biometric data
//! itself — Windows performs the match and hands back only a result code.
//!
//! When Windows Hello is unavailable the caller falls back to a local factor
//! (PIN, pattern or passphrase); availability is reported honestly so the UI can
//! say so.

use crate::error::{BvError, BvResult};

#[derive(Debug, Clone)]
pub struct HelloAvailability {
    pub available: bool,
    pub method: Option<String>,
}

#[cfg(windows)]
pub fn availability() -> HelloAvailability {
    use windows::Security::Credentials::UI::{
        UserConsentVerifier, UserConsentVerifierAvailability,
    };
    match UserConsentVerifier::CheckAvailabilityAsync().and_then(|op| op.get()) {
        Ok(a) => {
            let available = a == UserConsentVerifierAvailability::Available;
            HelloAvailability {
                available,
                method: if available {
                    Some("biometric or Windows PIN".into())
                } else {
                    None
                },
            }
        }
        Err(_) => HelloAvailability {
            available: false,
            method: None,
        },
    }
}

#[cfg(not(windows))]
pub fn availability() -> HelloAvailability {
    HelloAvailability {
        available: false,
        method: None,
    }
}

/// Raise the Windows Hello prompt and block until the user answers.
/// Returns `Ok(true)` only on a verified match.
#[cfg(windows)]
pub fn verify(message: &str) -> BvResult<bool> {
    use windows::core::HSTRING;
    use windows::Security::Credentials::UI::{
        UserConsentVerificationResult, UserConsentVerifier,
    };
    let msg = HSTRING::from(message);
    let result = UserConsentVerifier::RequestVerificationAsync(&msg)
        .and_then(|op| op.get())
        .map_err(|e| BvError::msg(format!("Windows Hello prompt failed: {e}")))?;
    match result {
        UserConsentVerificationResult::Verified => Ok(true),
        UserConsentVerificationResult::Canceled => Ok(false),
        UserConsentVerificationResult::RetriesExhausted => Ok(false),
        UserConsentVerificationResult::DeviceBusy => {
            Err(BvError::msg("Windows Hello device is busy"))
        }
        UserConsentVerificationResult::DeviceNotPresent => {
            Err(BvError::Unsupported("no Windows Hello device present".into()))
        }
        UserConsentVerificationResult::NotConfiguredForUser => Err(BvError::msg(
            "Windows Hello is not configured for this user",
        )),
        UserConsentVerificationResult::DisabledByPolicy => {
            Err(BvError::msg("Windows Hello is disabled by policy"))
        }
        _ => Ok(false),
    }
}

#[cfg(not(windows))]
pub fn verify(_message: &str) -> BvResult<bool> {
    Err(BvError::Unsupported("Windows Hello".into()))
}
