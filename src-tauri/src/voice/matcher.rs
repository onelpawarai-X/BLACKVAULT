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

/// Similarity of two single words, 0.0..1.0.
///
/// Exact words score 1.0. A near-miss (one typo) scores by its edit distance.
/// A *negation* never matches fuzzily: "lock" and "unlock" must not be
/// confused, because on this desktop that is the difference between covering
/// the screen and opening it.
fn word_score(a: &str, b: &str) -> f32 {
    if a == b {
        return 1.0;
    }
    if negation_conflict(a, b) {
        return 0.0;
    }
    let longest = a.chars().count().max(b.chars().count()).max(1);
    let ratio = 1.0 - levenshtein(a, b) as f32 / longest as f32;
    if ratio >= 0.8 {
        ratio
    } else {
        0.0
    }
}

/// True when one word is the other with a negation prefix, e.g. lock/unlock or
/// arm/disarm. Such a pair must only ever match exactly.
fn negation_conflict(a: &str, b: &str) -> bool {
    const PREFIXES: [&str; 3] = ["un", "dis", "de"];
    for p in PREFIXES {
        if let Some(rest) = a.strip_prefix(p) {
            if rest == b {
                return true;
            }
        }
        if let Some(rest) = b.strip_prefix(p) {
            if rest == a {
                return true;
            }
        }
    }
    false
}

/// True when any word of the phrase is a negation variant of any word heard.
///
/// A negation is decisive for the whole phrase: hearing "unlock this pc" must
/// never be scored as the "lock this pc" command, even though two of its three
/// words match. Without this the recall score would let "unlock" run "lock".
fn phrase_negation_conflict(phrase: &str, text_words: &[&str]) -> bool {
    phrase
        .split(' ')
        .filter(|w| !w.is_empty())
        .any(|w| text_words.iter().any(|t| negation_conflict(w, t)))
}

/// How much of a command phrase was actually said.
///
/// This is the number that makes voice control work with a real recogniser.
/// A speech-to-text service rarely returns the bare command: "next" comes back
/// as "play the next", "open chrome" as "open google chrome". Scoring by *how
/// many of the command's own words are present* — rather than by whole-string
/// edit distance — is what lets the command be found inside the sentence.
fn phrase_recall(phrase: &str, text_words: &[&str]) -> f32 {
    let words: Vec<&str> = phrase.split(' ').filter(|w| !w.is_empty()).collect();
    if words.is_empty() {
        return 0.0;
    }
    let mut total = 0.0f32;
    for w in &words {
        let best = text_words
            .iter()
            .map(|t| word_score(w, t))
            .fold(0.0f32, f32::max);
        total += best;
    }
    total / words.len() as f32
}

/// Match free text against a phrase list.
///
/// Used by the command editor's "Test" button, by the speech-to-text path and
/// by the offline recogniser's text fallback. Two signals are combined and the
/// better one wins:
///
///   * **recall** — the fraction of the command's words present in what was
///     heard. This is what recognises a command spoken inside a sentence, which
///     is how a real recogniser reports it.
///   * **whole-string similarity** — the edit-distance ratio, which catches a
///     one-letter slip ("scroll dowm") and a command said on its own.
pub fn match_text(text: &str, phrases: &[String]) -> Option<(String, f32)> {
    let norm = normalise(text);
    if norm.is_empty() {
        return None;
    }
    let text_words: Vec<&str> = norm.split(' ').filter(|w| !w.is_empty()).collect();
    let mut best: Option<(String, f32)> = None;
    let mut best_rank = f32::MIN;
    for p in phrases {
        let pn = normalise(p);
        if pn.is_empty() {
            continue;
        }
        let score = if pn == norm {
            1.0
        } else if phrase_negation_conflict(&pn, &text_words) {
            // A negation ("unlock this pc" vs the "lock this pc" command) is a
            // hard no: never let two-thirds recall of a dangerous opposite win.
            0.0
        } else {
            let recall = phrase_recall(&pn, &text_words);
            let whole = {
                let d = levenshtein(&norm, &pn);
                let longest = norm.chars().count().max(pn.chars().count()).max(1);
                (1.0 - d as f32 / longest as f32).max(0.0)
            };
            recall.max(whole)
        };
        // A small bonus for a longer phrase breaks ties toward the more specific
        // command: "next video" wins over "next" when both were fully heard. It
        // only affects ranking; the score reported to the caller stays 0..1.
        let rank = score + (pn.split(' ').count() as f32) * 1e-3;
        if rank > best_rank {
            best_rank = rank;
            best = Some((p.clone(), score.min(1.0)));
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
    fn a_command_inside_a_sentence_is_found() {
        // What a real speech-to-text service returns for "next" and
        // "open chrome" — the whole point of the recall score.
        let phrases = vec![
            "next".to_string(),
            "open chrome".to_string(),
            "volume up".to_string(),
        ];
        assert_eq!(match_text("play the next", &phrases).unwrap().0, "next");
        assert_eq!(match_text("open google chrome", &phrases).unwrap().0, "open chrome");
        assert_eq!(match_text("could you turn the volume up", &phrases).unwrap().0, "volume up");
    }

    #[test]
    fn a_negation_never_runs_its_opposite_command() {
        // "unlock this pc" must not fire the "lock this pc" command.
        let phrases = vec!["lock this pc".to_string()];
        assert!(
            match_text("unlock this pc", &phrases).is_none(),
            "a negation must not match its opposite"
        );
        // The exact word still matches.
        assert_eq!(match_text("lock this pc", &phrases).unwrap().0, "lock this pc");
    }

    #[test]
    fn the_more_specific_phrase_wins_a_tie() {
        let phrases = vec!["next".to_string(), "next video".to_string()];
        assert_eq!(match_text("next video", &phrases).unwrap().0, "next video");
    }

    #[test]
    fn acoustic_scoring_is_monotonic() {
        assert_eq!(distance_to_score(0.5, 1.0, 5.0), 1.0);
        assert_eq!(distance_to_score(5.0, 1.0, 5.0), 0.0);
        assert!(distance_to_score(2.0, 1.0, 5.0) > distance_to_score(4.0, 1.0, 5.0));
    }
}
