//! BLACKVAULT — the phrase matcher.
//!
//! Two independent signals are combined:
//!   1. an *acoustic* score — DTW distance between the live utterance's MFCCs
//!      and the enrolled template for a phrase, converted to a similarity;
//!   2. a *speaker* score — cosine similarity between the utterance's speaker
//!      vector and the enrolled voiceprint.
//!
//! The acoustic signal decides *which* command was spoken; the speaker signal
//! decides *whether it sounds like the enrolled user*. The speaker signal is
//! deliberately never sufficient on its own for a protected action — see
//! `auth.rs` — but it does gate which commands run at all.

use crate::dsp;

#[derive(Debug, Clone)]
pub struct Template {
    pub phrase: String,
    pub kind: String,
    pub mfcc: Vec<Vec<f32>>,
}

#[derive(Debug, Clone)]
pub struct Match {
    pub phrase: String,
    pub kind: String,
    /// 0..1, higher is better.
    pub score: f32,
    pub distance: f32,
}

/// Normalise a phrase for comparison: lower-case, strip punctuation, collapse
/// whitespace. This is what makes "Lock this PC!" and "lock this pc" equal.
pub fn normalise(phrase: &str) -> String {
    let mut out = String::with_capacity(phrase.len());
    let mut last_space = true;
    for ch in phrase.chars() {
        if ch.is_alphanumeric() {
            for lc in ch.to_lowercase() {
                out.push(lc);
            }
            last_space = false;
        } else if !last_space {
            out.push(' ');
            last_space = true;
        }
    }
    while out.ends_with(' ') {
        out.pop();
    }
    out
}

/// Levenshtein distance on characters, for a cheap text fallback and for
/// fuzzy phrase lookup (e.g. "next video" vs "next").
pub fn levenshtein(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    if a.is_empty() {
        return b.len();
    }
    if b.is_empty() {
        return a.len();
    }
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut cur = vec![0usize; b.len() + 1];
    for i in 1..=a.len() {
        cur[0] = i;
        for j in 1..=b.len() {
            let cost = if a[i - 1] == b[j - 1] { 0 } else { 1 };
            cur[j] = (prev[j] + 1).min(cur[j - 1] + 1).min(prev[j - 1] + cost);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[b.len()]
}

/// Convert a DTW distance to a 0..1 score. Distances above `worst` clamp to 0.
fn distance_to_score(distance: f32, best: f32, worst: f32) -> f32 {
    if !distance.is_finite() {
        return 0.0;
    }
    if distance <= best {
        return 1.0;
    }
    if distance >= worst {
        return 0.0;
    }
    1.0 - (distance - best) / (worst - best)
}

/// Score one utterance against every template and return the best match.
pub fn best_match(
    utterance_mfcc: &[Vec<f32>],
    templates: &[Template],
    best_dist: f32,
    worst_dist: f32,
) -> Option<Match> {
    let mut best: Option<Match> = None;
    for t in templates {
        let d = dsp::dtw(utterance_mfcc, &t.mfcc);
        let score = distance_to_score(d, best_dist, worst_dist);
        if best.as_ref().map(|b| score > b.score).unwrap_or(true) {
            best = Some(Match {
                phrase: t.phrase.clone(),
                kind: t.kind.clone(),
                score,
                distance: d,
            });
        }
    }
    best
}

/// Match free text against a phrase list (used by the command editor's "Test"
/// button and by the text fallback path).
pub fn match_text(text: &str, phrases: &[String]) -> Option<(String, f32)> {
    let norm = normalise(text);
    if norm.is_empty() {
        return None;
    }
    let mut best: Option<(String, f32)> = None;
    for p in phrases {
        let pn = normalise(p);
        let score = if pn == norm {
            1.0
        } else {
            let d = levenshtein(&norm, &pn);
            let longest = norm.chars().count().max(pn.chars().count()).max(1);
            (1.0 - d as f32 / longest as f32).max(0.0)
        };
        if best.as_ref().map(|(_, s)| score > *s).unwrap_or(true) {
            best = Some((p.clone(), score));
        }
    }
    best.filter(|(_, s)| *s >= 0.6)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalise_strips_case_and_punctuation() {
        assert_eq!(normalise("Lock this PC!"), "lock this pc");
        assert_eq!(normalise("  NEXT   VIDEO  "), "next video");
        assert_eq!(normalise("volume-up"), "volume up");
        assert_eq!(normalise(""), "");
    }

    #[test]
    fn levenshtein_basics() {
        assert_eq!(levenshtein("kitten", "sitting"), 3);
        assert_eq!(levenshtein("", "abc"), 3);
        assert_eq!(levenshtein("same", "same"), 0);
    }

    #[test]
    fn exact_text_matches() {
        let phrases = vec!["next".to_string(), "scroll down".to_string()];
        let m = match_text("NEXT", &phrases).unwrap();
        assert_eq!(m.0, "next");
        assert!((m.1 - 1.0).abs() < 1e-6);
    }

    #[test]
    fn near_text_matches_within_threshold() {
        let phrases = vec!["scroll down".to_string()];
        let m = match_text("scroll dowm", &phrases).unwrap();
        assert!(m.1 > 0.8);
    }

    #[test]
    fn unrelated_text_does_not_match() {
        let phrases = vec!["next".to_string()];
        assert!(match_text("completely different words", &phrases).is_none());
    }

    #[test]
    fn acoustic_scoring_is_monotonic() {
        assert_eq!(distance_to_score(0.5, 1.0, 5.0), 1.0);
        assert_eq!(distance_to_score(5.0, 1.0, 5.0), 0.0);
        assert!(distance_to_score(2.0, 1.0, 5.0) > distance_to_score(4.0, 1.0, 5.0));
    }
}
