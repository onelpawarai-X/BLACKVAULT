//! BLACKVAULT — decides what a recognised utterance should do.
//!
//! Kept pure and separate from the audio thread so the routing rules can be
//! tested without a microphone. The engine hands a `Match` in and acts on the
//! `RouteOutcome` it gets back.

use std::time::{Duration, Instant};

use super::matcher::Match;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RouteKind {
    /// The wake phrase was heard; commands are armed for a short window.
    Wake,
    /// A command should run.
    Command,
    /// An unlock attempt on the lock screen.
    Unlock,
    /// Nothing to do (below threshold, or not armed).
    Ignore,
}

#[derive(Debug, Clone)]
pub struct RouteOutcome {
    pub kind: RouteKind,
    pub phrase: String,
    pub score: f32,
}

pub struct Router {
    armed_until: Option<Instant>,
    wake_timeout: Duration,
    command_threshold: f32,
    unlock_threshold: f32,
}

impl Default for Router {
    fn default() -> Self {
        Router {
            armed_until: None,
            wake_timeout: Duration::from_secs(6),
            command_threshold: 0.55,
            unlock_threshold: 0.6,
        }
    }
}

impl Router {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_wake_timeout(&mut self, d: Duration) {
        self.wake_timeout = d;
    }

    pub fn armed(&self, now: Instant) -> bool {
        self.armed_until.map(|t| now < t).unwrap_or(false)
    }

    pub fn disarm(&mut self) {
        self.armed_until = None;
    }

    /// Arm the router for one command without waiting for the wake phrase.
    ///
    /// This is the push-to-talk path: the user pressed the microphone button, so
    /// their next command counts. Like the wake phrase it is consumed by the
    /// first command it lets through, and it still refuses to unlock.
    pub fn arm_once(&mut self, window: Duration) {
        self.armed_until = Some(Instant::now() + window);
    }

    /// Route a match. `locked` is the current security mode; `wake_required`
    /// mirrors the setting of the same name.
    pub fn route(
        &mut self,
        locked: bool,
        m: &Match,
        wake_required: bool,
        now: Instant,
    ) -> RouteOutcome {
        let ignore = |score: f32| RouteOutcome {
            kind: RouteKind::Ignore,
            phrase: m.phrase.clone(),
            score,
        };

        // The wake phrase is recognised regardless of mode and never executes.
        if m.kind == "wake" {
            if m.score >= self.command_threshold {
                self.armed_until = Some(now + self.wake_timeout);
                return RouteOutcome {
                    kind: RouteKind::Wake,
                    phrase: m.phrase.clone(),
                    score: m.score,
                };
            }
            return ignore(m.score);
        }

        if locked {
            // On the lock screen only the unlock phrase (or the literal
            // "unlock this pc" command) is meaningful. Everything else is
            // deliberately swallowed so a stray word cannot wake the desktop.
            let is_unlock = m.kind == "unlock" || m.phrase == "unlock this pc";
            if is_unlock && m.score >= self.unlock_threshold {
                return RouteOutcome {
                    kind: RouteKind::Unlock,
                    phrase: m.phrase.clone(),
                    score: m.score,
                };
            }
            return ignore(m.score);
        }

        // Unlocked: commands require the wake phrase when the user asked for it.
        if wake_required && !self.armed(now) {
            return ignore(m.score);
        }
        if m.score < self.command_threshold {
            return ignore(m.score);
        }
        self.disarm();
        RouteOutcome {
            kind: RouteKind::Command,
            phrase: m.phrase.clone(),
            score: m.score,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(phrase: &str, kind: &str, score: f32) -> Match {
        Match {
            phrase: phrase.into(),
            kind: kind.into(),
            score,
            distance: 0.0,
        }
    }

    #[test]
    fn wake_arms_commands() {
        let mut r = Router::new();
        let now = Instant::now();
        let out = r.route(false, &m("black vault", "wake", 0.9), true, now);
        assert_eq!(out.kind, RouteKind::Wake);
        assert!(r.armed(now));
        let out = r.route(false, &m("next", "command", 0.9), true, now);
        assert_eq!(out.kind, RouteKind::Command);
        assert!(!r.armed(now), "arming is consumed by one command");
    }

    #[test]
    fn commands_ignored_without_wake_when_required() {
        let mut r = Router::new();
        let now = Instant::now();
        let out = r.route(false, &m("next", "command", 0.9), true, now);
        assert_eq!(out.kind, RouteKind::Ignore);
    }

    #[test]
    fn commands_run_without_wake_when_not_required() {
        let mut r = Router::new();
        let now = Instant::now();
        let out = r.route(false, &m("next", "command", 0.9), false, now);
        assert_eq!(out.kind, RouteKind::Command);
    }

    #[test]
    fn lock_screen_accepts_only_unlock() {
        let mut r = Router::new();
        let now = Instant::now();
        assert_eq!(r.route(true, &m("next", "command", 0.99), true, now).kind, RouteKind::Ignore);
        assert_eq!(r.route(true, &m("unlock this pc", "command", 0.9), true, now).kind, RouteKind::Unlock);
        assert_eq!(r.route(true, &m("my secret words", "unlock", 0.9), true, now).kind, RouteKind::Unlock);
    }

    #[test]
    fn low_scores_are_ignored() {
        let mut r = Router::new();
        let now = Instant::now();
        assert_eq!(r.route(false, &m("next", "command", 0.2), false, now).kind, RouteKind::Ignore);
        assert_eq!(r.route(true, &m("unlock this pc", "command", 0.3), true, now).kind, RouteKind::Ignore);
    }

    #[test]
    fn wake_expires() {
        let mut r = Router::new();
        r.set_wake_timeout(Duration::from_millis(1));
        let now = Instant::now();
        r.route(false, &m("black vault", "wake", 0.9), true, now);
        let later = now + Duration::from_secs(2);
        assert!(!r.armed(later));
    }

    #[test]
    fn push_to_talk_runs_one_command_without_the_wake_phrase() {
        let mut r = Router::new();
        r.arm_once(Duration::from_secs(5));
        let now = Instant::now();
        let out = r.route(false, &m("next", "command", 0.9), true, now);
        assert_eq!(out.kind, RouteKind::Command);
        assert!(!r.armed(now), "push-to-talk is spent by the command it let through");
        assert_eq!(r.route(false, &m("next", "command", 0.9), true, now).kind, RouteKind::Ignore);
    }

    #[test]
    fn push_to_talk_expires_and_never_unlocks() {
        let mut r = Router::new();
        r.arm_once(Duration::from_millis(1));
        let later = Instant::now() + Duration::from_secs(2);
        assert_eq!(r.route(false, &m("next", "command", 0.9), true, later).kind, RouteKind::Ignore);

        let mut locked = Router::new();
        locked.arm_once(Duration::from_secs(5));
        let now = Instant::now();
        assert_eq!(
            locked.route(true, &m("next", "command", 0.9), true, now).kind,
            RouteKind::Ignore,
            "the microphone button must not release the lock"
        );
    }
}
