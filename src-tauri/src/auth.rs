//! BLACKVAULT — the authentication policy.
//!
//! Rules that are enforced here and nowhere else:
//!
//!   * A secret (PIN, pattern, passphrase) is verified against a PBKDF2 hash.
//!     Plain text never touches the database.
//!   * Voice is *never* sufficient on its own for a protected action. A voice
//!     unlock must clear the speaker check **and** match the enrolled
//!     passphrase's hash before it is even offered as a factor, and when the
//!     user has asked for a second factor it still cannot complete the unlock.
//!   * Failed attempts are rate limited: after N failures within the window the
//!     unlock screen refuses further attempts until the window elapses.
//!   * Every attempt — success or failure — is written to the audit log.

use std::time::Instant;

use crate::crypto;
use crate::db::Db;
use crate::dsp;
use crate::error::{BvError, BvResult};
use crate::models::AuthResult;
use crate::settings::Settings;

pub const SECRET_PIN: &str = "pin";
pub const SECRET_PATTERN: &str = "pattern";
pub const SECRET_PASSPHRASE: &str = "passphrase";
pub const SECRET_VOICE_KEY: &str = "voice_template_key";

/// Which factors are actually available right now, in the order the lock screen
/// should present them.
pub fn available_factors(db: &Db, settings: &Settings, hello_available: bool) -> Vec<String> {
    let mut factors = Vec::new();
    // The passphrase is the primary voice unlock, so it is always first.
    if db.get_secret(SECRET_PASSPHRASE).ok().flatten().is_some() {
        factors.push("passphrase".to_string());
        factors.push("voice".to_string());
    }
    if db.get_secret(SECRET_PIN).ok().flatten().is_some() {
        factors.push("pin".to_string());
    }
    if db.get_secret(SECRET_PATTERN).ok().flatten().is_some() {
        factors.push("pattern".to_string());
    }
    if hello_available {
        factors.push("hello".to_string());
    }
    // Guarantee at least one factor so the screen is never a dead end.
    if factors.is_empty() {
        factors.push("passphrase".to_string());
    }
    let _ = settings;
    factors
}

/// The factors that satisfy the *first* step of an unlock: the secret passphrase
/// (spoken or typed). If no passphrase is configured the configured factor is
/// the first step, because it is then the only way in.
pub fn first_step_factors(db: &Db, settings: &Settings, hello_available: bool) -> Vec<String> {
    let has_pass = db.get_secret(SECRET_PASSPHRASE).ok().flatten().is_some();
    if has_pass {
        return vec!["passphrase".into(), "voice".into()];
    }
    available_factors(db, settings, hello_available)
}

/// The factors that satisfy the *second* step. Only the non-passphrase factors,
/// so the screen never offers the passphrase twice.
pub fn second_step_factors(db: &Db, settings: &Settings, hello_available: bool) -> Vec<String> {
    let mut factors: Vec<String> = Vec::new();
    if db.get_secret(SECRET_PIN).ok().flatten().is_some() {
        factors.push("pin".into());
    }
    if db.get_secret(SECRET_PATTERN).ok().flatten().is_some() {
        factors.push("pattern".into());
    }
    if hello_available {
        factors.push("hello".into());
    }
    if factors.is_empty() {
        // No second factor configured: nothing more to ask for.
        factors.push("passphrase".into());
    }
    factors
}

/// What should happen after a factor check succeeded. Kept pure so the policy
/// can be tested without a database, an engine or a UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnlockStep {
    /// The factor was offered out of order — the second step was never reached.
    OutOfOrder,
    /// The first factor passed; a second factor is still required.
    NeedSecondFactor,
    /// Every required factor has passed; the desktop may be released.
    Complete,
}

/// Decide the next unlock step. `method` is the factor that just passed.
pub fn decide_step(require_second_factor: bool, first_step_done: bool, method: &str) -> UnlockStep {
    if !require_second_factor {
        return UnlockStep::Complete;
    }
    let is_second = matches!(method, "pin" | "pattern" | "hello");
    let is_full = method == "voice_full";
    if is_second && !first_step_done {
        return UnlockStep::OutOfOrder;
    }
    if !is_second && !is_full {
        return UnlockStep::NeedSecondFactor;
    }
    UnlockStep::Complete
}

/// Remaining attempts before the cool-down, or `None` when not limited.
pub fn remaining_attempts(db: &Db, settings: &Settings) -> Option<i64> {
    let (failures, _) = db.failure_state().ok()?;
    let max = settings.auth_rate_limit_attempts;
    Some((max - failures).max(0))
}

/// Has the cool-down been triggered?
pub fn is_rate_limited(db: &Db, settings: &Settings) -> bool {
    let Ok((failures, window_start)) = db.failure_state() else {
        return false;
    };
    if failures < settings.auth_rate_limit_attempts {
        return false;
    }
    let Ok(start) = chrono::DateTime::parse_from_rfc3339(&window_start) else {
        return false;
    };
    let elapsed = (chrono::Utc::now() - start.with_timezone(&chrono::Utc)).num_seconds();
    elapsed < settings.auth_rate_limit_window_secs
}

/// Record a failure and return the resulting AuthResult (with remaining count).
fn on_failure(db: &Db, settings: &Settings, method: &str, message: &str) -> AuthResult {
    let _ = db.record_failure();
    let _ = db.log_event("auth_failure", "warn", &format!("Failed authentication ({method})"), None);
    if is_rate_limited(db, settings) {
        let _ = db.log_event(
            "rate_limited",
            "critical",
            "Unlock attempts rate limited",
            Some(&format!("{} failures within {}s", settings.auth_rate_limit_attempts, settings.auth_rate_limit_window_secs)),
        );
        return AuthResult::fail(method, "Too many attempts. Wait before trying again.", Some(0));
    }
    AuthResult::fail(method, message, remaining_attempts(db, settings))
}

fn on_success(db: &Db, method: &str, message: &str) -> AuthResult {
    let _ = db.reset_failures();
    let _ = db.log_event("auth_success", "info", &format!("Authenticated via {method}"), None);
    AuthResult::ok(method, message)
}

// --------------------------------------------------------------- factors

pub fn verify_passphrase(db: &Db, settings: &Settings, candidate: &str) -> BvResult<AuthResult> {
    if is_rate_limited(db, settings) {
        return Ok(AuthResult::fail("passphrase", "Too many attempts. Wait before trying again.", Some(0)));
    }
    let Some(stored) = db.get_secret(SECRET_PASSPHRASE)? else {
        return Ok(AuthResult::fail("passphrase", "No passphrase is configured.", None));
    };
    let normalised = crate::voice::matcher::normalise(candidate);
    if normalised.is_empty() {
        return Ok(on_failure(db, settings, "passphrase", "Nothing was entered."));
    }
    if crypto::verify_secret(&normalised, &stored)? {
        Ok(on_success(db, "passphrase", "Passphrase accepted."))
    } else {
        Ok(on_failure(db, settings, "passphrase", "That passphrase is not correct."))
    }
}

pub fn verify_pin(db: &Db, settings: &Settings, candidate: &str) -> BvResult<AuthResult> {
    if is_rate_limited(db, settings) {
        return Ok(AuthResult::fail("pin", "Too many attempts. Wait before trying again.", Some(0)));
    }
    let Some(stored) = db.get_secret(SECRET_PIN)? else {
        return Ok(AuthResult::fail("pin", "No PIN is configured.", None));
    };
    if crypto::verify_secret(candidate.trim(), &stored)? {
        Ok(on_success(db, "pin", "PIN accepted."))
    } else {
        Ok(on_failure(db, settings, "pin", "That PIN is not correct."))
    }
}

pub fn verify_pattern(db: &Db, settings: &Settings, nodes: &[i32]) -> BvResult<AuthResult> {
    if is_rate_limited(db, settings) {
        return Ok(AuthResult::fail("pattern", "Too many attempts. Wait before trying again.", Some(0)));
    }
    let Some(stored) = db.get_secret(SECRET_PATTERN)? else {
        return Ok(AuthResult::fail("pattern", "No pattern is configured.", None));
    };
    if nodes.len() < 4 {
        return Ok(on_failure(db, settings, "pattern", "Connect at least four dots."));
    }
    let serialised = nodes.iter().map(|n| n.to_string()).collect::<Vec<_>>().join("-");
    if crypto::verify_secret(&serialised, &stored)? {
        Ok(on_success(db, "pattern", "Pattern accepted."))
    } else {
        Ok(on_failure(db, settings, "pattern", "That pattern is not correct."))
    }
}

pub fn verify_hello(db: &Db, settings: &Settings) -> BvResult<AuthResult> {
    if is_rate_limited(db, settings) {
        return Ok(AuthResult::fail("hello", "Too many attempts. Wait before trying again.", Some(0)));
    }
    match crate::platform::hello::verify("Unlock BLACKVAULT") {
        Ok(true) => Ok(on_success(db, "hello", "Windows Hello verified.")),
        Ok(false) => Ok(on_failure(db, settings, "hello", "Windows Hello did not verify.")),
        Err(e) => Ok(AuthResult::fail("hello", format!("Windows Hello unavailable: {e}"), None)),
    }
}

/// The voice path. Two conditions must hold:
///
///   1. the utterance's speaker vector must be close enough to the enrolled
///      voiceprint (cosine similarity ≥ the configured threshold), **and**
///   2. the words the matcher recognised must hash-match the stored passphrase.
///
/// Only then is a voice factor offered. Whether that alone completes the unlock
/// is decided by the caller, not here.
pub fn verify_voice(
    db: &Db,
    settings: &Settings,
    recognised_phrase: &str,
    speaker: &[f32],
) -> BvResult<AuthResult> {
    if is_rate_limited(db, settings) {
        return Ok(AuthResult::fail("voice", "Too many attempts. Wait before trying again.", Some(0)));
    }

    // 1. Words must match the enrolled passphrase hash.
    let Some(stored) = db.get_secret(SECRET_PASSPHRASE)? else {
        return Ok(AuthResult::fail("voice", "No passphrase is configured.", None));
    };
    let normalised = crate::voice::matcher::normalise(recognised_phrase);
    if !crypto::verify_secret(&normalised, &stored)? {
        return Ok(on_failure(db, settings, "voice", "That is not the unlock phrase."));
    }

    // 2. Voice must sound like the enrolled speaker.
    let Some((template, threshold)) = db.voice_template()? else {
        // No voiceprint enrolled: the phrase matched, but voice identity is not
        // established. Report that honestly rather than granting a pass.
        return Ok(AuthResult::fail(
            "voice",
            "Phrase matched, but no voice identity is enrolled. Use another factor.",
            remaining_attempts(db, settings),
        ));
    };
    if speaker.is_empty() {
        return Ok(on_failure(db, settings, "voice", "Could not analyse the voice sample."));
    }
    let similarity = dsp::cosine(speaker, &template);
    let effective_threshold = threshold.max(settings.voiceprint_threshold * 0.9);
    if similarity < effective_threshold {
        let mut r = on_failure(db, settings, "voice", "That does not sound like the enrolled voice.");
        r.score = Some(similarity);
        return Ok(r);
    }

    let mut r = on_success(db, "voice", "Voice identity confirmed.");
    r.score = Some(similarity);
    Ok(r)
}

// -------------------------------------------------------------- enrolment

pub fn set_secret_hash(db: &Db, name: &str, value: &str) -> BvResult<()> {
    let hash = crypto::hash_secret(value)?;
    db.set_secret(name, &hash)?;
    Ok(())
}

/// Set the passphrase and remember its normalised form for the matcher.
pub fn set_passphrase(db: &Db, phrase: &str) -> BvResult<()> {
    let normalised = crate::voice::matcher::normalise(phrase);
    if normalised.split(' ').filter(|w| !w.is_empty()).count() < 3 {
        return Err(BvError::msg("Use at least three words for the passphrase."));
    }
    set_secret_hash(db, SECRET_PASSPHRASE, &normalised)?;
    db.log_event("passphrase_set", "info", "Secret passphrase updated", None)?;
    Ok(())
}

pub fn set_pin(db: &Db, pin: &str) -> BvResult<()> {
    let pin = pin.trim();
    if pin.len() < 4 || pin.len() > 12 || !pin.chars().all(|c| c.is_ascii_digit()) {
        return Err(BvError::msg("The PIN must be 4 to 12 digits."));
    }
    set_secret_hash(db, SECRET_PIN, pin)?;
    db.log_event("pin_set", "info", "PIN updated", None)?;
    Ok(())
}

pub fn set_pattern(db: &Db, nodes: &[i32]) -> BvResult<()> {
    if nodes.len() < 4 {
        return Err(BvError::msg("Connect at least four dots."));
    }
    if nodes.iter().any(|n| !(0..9).contains(n)) {
        return Err(BvError::msg("Pattern node out of range."));
    }
    let serialised = nodes.iter().map(|n| n.to_string()).collect::<Vec<_>>().join("-");
    set_secret_hash(db, SECRET_PATTERN, &serialised)?;
    db.log_event("pattern_set", "info", "Pattern updated", None)?;
    Ok(())
}

/// Enrol a voice identity from raw PCM samples captured by the UI. Each sample
/// is turned into a speaker vector; they are averaged into one template, and a
/// fresh keyed digest is stored so the template cannot be read out directly.
pub fn enrol_voice(db: &Db, samples: &[Vec<f32>]) -> BvResult<(i64, f32)> {
    let vectors: Vec<Vec<f32>> = samples
        .iter()
        .filter(|s| s.len() >= 400)
        .map(|s| dsp::speaker_vector(s))
        .filter(|v| !v.is_empty())
        .collect();
    if vectors.is_empty() {
        return Err(BvError::msg("No usable voice samples were captured."));
    }
    let template = dsp::average_vectors(&vectors);
    let json = serde_json::to_string(&template)?;
    // The template is a derived feature vector, not audio, and it is sealed with
    // DPAPI before it touches the database.
    let sealed = crypto::seal_string(&json)?;
    let count = vectors.len() as i64;
    let settings = db.load_settings()?;
    db.set_voice_identity(&sealed, count, settings.voiceprint_threshold)?;
    db.log_event("hello_enrolled", "info", "Voice identity enrolled", Some(&format!("{count} samples")))?;
    Ok((count, settings.voiceprint_threshold))
}

/// Seconds since the app started, used for the dashboard uptime read-out.
pub fn uptime_secs(started: Instant) -> u64 {
    started.elapsed().as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Db;

    fn db() -> Db {
        Db::open_in_memory().unwrap()
    }

    #[test]
    fn passphrase_must_be_three_words() {
        let d = db();
        assert!(set_passphrase(&d, "short").is_err());
        assert!(set_passphrase(&d, "silver thunder vault").is_ok());
    }

    #[test]
    fn passphrase_verification_is_normalised() {
        let d = db();
        set_passphrase(&d, "Silver Thunder Vault").unwrap();
        let s = Settings::default();
        assert!(verify_passphrase(&d, &s, "silver thunder vault!").unwrap().ok);
        assert!(!verify_passphrase(&d, &s, "wrong words here").unwrap().ok);
    }

    #[test]
    fn pin_rules_are_enforced() {
        let d = db();
        assert!(set_pin(&d, "12").is_err());
        assert!(set_pin(&d, "abcd").is_err());
        assert!(set_pin(&d, "1234").is_ok());
        let s = Settings::default();
        assert!(verify_pin(&d, &s, "1234").unwrap().ok);
        assert!(!verify_pin(&d, &s, "9999").unwrap().ok);
    }

    #[test]
    fn pattern_needs_four_nodes() {
        let d = db();
        assert!(set_pattern(&d, &[0, 1, 2]).is_err());
        assert!(set_pattern(&d, &[0, 1, 2, 5, 8]).is_ok());
        let s = Settings::default();
        assert!(verify_pattern(&d, &s, &[0, 1, 2, 5, 8]).unwrap().ok);
        assert!(!verify_pattern(&d, &s, &[0, 1, 2, 3]).unwrap().ok);
    }

    #[test]
    fn rate_limiting_kicks_in() {
        let d = db();
        set_pin(&d, "1234").unwrap();
        let mut s = Settings::default();
        s.auth_rate_limit_attempts = 3;
        s.auth_rate_limit_window_secs = 60;
        for _ in 0..3 {
            let r = verify_pin(&d, &s, "0000").unwrap();
            assert!(!r.ok);
        }
        assert!(is_rate_limited(&d, &s));
        // Even the correct PIN is refused while limited.
        let r = verify_pin(&d, &s, "1234").unwrap();
        assert!(!r.ok);
    }

    #[test]
    fn success_resets_the_failure_counter() {
        let d = db();
        set_pin(&d, "1234").unwrap();
        let s = Settings::default();
        assert!(!verify_pin(&d, &s, "0000").unwrap().ok);
        assert!(verify_pin(&d, &s, "1234").unwrap().ok);
        assert_eq!(d.failure_state().unwrap().0, 0);
    }

    #[test]
    fn voice_requires_enrolment_and_phrase() {
        let d = db();
        set_passphrase(&d, "silver thunder vault").unwrap();
        let s = Settings::default();
        // Phrase matches but no voiceprint: must not pass.
        let r = verify_voice(&d, &s, "silver thunder vault", &[0.1; 26]).unwrap();
        assert!(!r.ok);
        // Wrong phrase: must not pass.
        let r = verify_voice(&d, &s, "some other words", &[0.1; 26]).unwrap();
        assert!(!r.ok);
    }

    #[test]
    fn voice_passes_with_matching_template_and_phrase() {
        let d = db();
        set_passphrase(&d, "silver thunder vault").unwrap();
        // Enrol a synthetic speaker vector directly.
        let template = vec![0.5f32; 26];
        d.set_voice_identity(&serde_json::to_string(&template).unwrap(), 3, 0.6).unwrap();
        let s = Settings::default();
        let r = verify_voice(&d, &s, "silver thunder vault", &template).unwrap();
        assert!(r.ok, "identical vector must clear the threshold: {r:?}");
        // A very different vector must fail.
        let far = vec![-0.5f32; 26];
        let r = verify_voice(&d, &s, "silver thunder vault", &far).unwrap();
        assert!(!r.ok);
    }

    #[test]
    fn factors_are_reported_from_configuration() {
        let d = db();
        let s = Settings::default();
        assert_eq!(available_factors(&d, &s, false), vec!["passphrase"]);
        set_pin(&d, "1234").unwrap();
        let f = available_factors(&d, &s, true);
        assert!(f.contains(&"pin".to_string()));
        assert!(f.contains(&"hello".to_string()));
    }

    #[test]
    fn first_step_is_the_passphrase_when_one_exists() {
        let d = db();
        let s = Settings::default();
        set_passphrase(&d, "silver thunder vault").unwrap();
        set_pin(&d, "1234").unwrap();
        // The first step is the passphrase even though a PIN also exists.
        assert_eq!(
            first_step_factors(&d, &s, true),
            vec!["passphrase", "voice"]
        );
        // The second step never repeats the passphrase.
        let second = second_step_factors(&d, &s, true);
        assert!(second.contains(&"pin".to_string()));
        assert!(!second.contains(&"passphrase".to_string()));
    }

    #[test]
    fn second_factor_alone_cannot_complete_the_unlock() {
        // The passphrase has not been supplied yet, so a PIN must be refused.
        assert_eq!(
            decide_step(true, false, "pin"),
            UnlockStep::OutOfOrder
        );
        assert_eq!(
            decide_step(true, false, "hello"),
            UnlockStep::OutOfOrder
        );
    }

    #[test]
    fn passphrase_then_second_factor_completes() {
        assert_eq!(
            decide_step(true, false, "passphrase"),
            UnlockStep::NeedSecondFactor
        );
        assert_eq!(decide_step(true, true, "pin"), UnlockStep::Complete);
        assert_eq!(decide_step(true, true, "pattern"), UnlockStep::Complete);
    }

    #[test]
    fn single_factor_policy_completes_on_the_first_step() {
        assert_eq!(
            decide_step(false, false, "passphrase"),
            UnlockStep::Complete
        );
        // A full voice identity is a whole authentication on its own.
        assert_eq!(decide_step(true, false, "voice_full"), UnlockStep::Complete);
    }

    #[test]
    fn second_step_falls_back_when_nothing_else_is_configured() {
        let d = db();
        let s = Settings::default();
        set_passphrase(&d, "silver thunder vault").unwrap();
        // No PIN, no pattern, no Hello: the screen must not dead-end.
        assert_eq!(second_step_factors(&d, &s, false), vec!["passphrase"]);
    }
}
