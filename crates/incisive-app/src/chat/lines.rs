//! The fixed lines the chat shows, and the two question tests that decide
//! when a safety line is added. Pure. These strings are the only place the
//! wording lives.

use std::sync::LazyLock;

use regex::Regex;

pub const NOT_COVERED: &str = "Not covered in this evidence.";
pub const SEE_YOUR_SURGEON: &str = "Your own surgeon can tell you what this means for you.";
pub const WARNING_LINE: &str =
    "If you have these symptoms now, contact your surgeon or hospital straight away.";
pub const UNAVAILABLE: &str = "Chat is unavailable right now.";
pub const BUSY: &str = "Gemma is busy, try again in a minute.";
pub const FOOTER: &str =
    "Answers come only from this run's evidence. Educational, not medical advice.";

/// Whether a reply is, as a whole, the not-covered sentence (with or without
/// its full stop, in any case).
pub fn is_not_covered(reply: &str) -> bool {
    let t = reply.trim();
    let t = t.strip_suffix('.').unwrap_or(t);
    t.eq_ignore_ascii_case(NOT_COVERED.trim_end_matches('.'))
}

/// Symptom words and phrases that mean "act now" when a patient names them.
/// Matched on the lower-cased question, whole words only. "pain" alone is
/// not a warning: only the qualified forms are (see [`QUALIFIED_PAIN`]).
const WARNING_TERMS: &[&str] = &[
    "fever",
    "fevers",
    "feverish",
    "vomit",
    "vomited",
    "vomits",
    "bled",
    "pain is increasing",
    "pain getting worse",
    "pain is getting worse",
    "pain is worse",
    "pain worsening",
    "unbearable pain",
    "discharge from the wound",
    "bleeding",
    "bleeds",
    "bleed",
    "severe pain",
    "increasing pain",
    "worsening pain",
    "worse pain",
    "chest pain",
    "breathlessness",
    "breathless",
    "short of breath",
    "shortness of breath",
    "difficulty breathing",
    "vomiting",
    "wound redness",
    "redness",
    "pus",
    "unable to pass urine",
    "cannot pass urine",
    "can't pass urine",
    "not passing urine",
    "discharge from my wound",
    "wound is red",
    "red wound",
    "cannot urinate",
    "can't urinate",
    "blood in stool",
    "blood in my stool",
    "coughing up blood",
    "can't breathe",
    "cannot breathe",
    "trouble breathing",
    "throwing up",
    "high temperature",
];

/// Pain with a qualifier on either side, some words apart ("severe abdominal
/// pain", "the pain is getting worse"). Run on the lower-cased question.
static QUALIFIED_PAIN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"\b(?:severe|bad|unbearable|worsening|increasing)(?:\s+\w+){0,2}\s+pain\b|\bpain\s+(?:is\s+|getting\s+)?(?:\w+\s+)?(?:severe|worse|worsening|increasing)\b",
    )
    .expect("qualified pain")
});

/// Words that fire on their own: the asker, or their family via "my".
const FIRST_PERSON: &[&str] = &[
    "i", "i'm", "i've", "i'd", "i'll", "me", "my", "mine", "myself",
];

/// "we", "our", "you", "your" are only about the asker when a decision is
/// being asked about.
const SHARED_PERSON: &[&str] = &[
    "we", "we're", "we've", "our", "ours", "you", "you're", "you've", "your", "yours",
];

const DECISION_WORDS: &[&str] = &["should", "will", "can", "need", "choose"];
const DECISION_PHRASES: &[&str] = &["better for", "go home"];

/// A label before "I" means the Roman numeral, not the pronoun.
const LABELS: &[&str] = &[
    "type", "grade", "phase", "class", "stage", "group", "level", "tier", "step", "part",
    "category", "level", "cohort", "arm", "trial", "factor", "zone", "tumour", "tumor",
];

fn norm(q: &str) -> String {
    q.to_lowercase().replace('\u{2019}', "'")
}

/// Whether `phrase` occurs in `text` bounded by non-letters/digits.
fn has_phrase(text: &str, phrase: &str) -> bool {
    let mut from = 0;
    while let Some(i) = text[from..].find(phrase) {
        let a = from + i;
        let b = a + phrase.len();
        let before = text[..a].chars().next_back();
        let after = text[b..].chars().next();
        if !before.is_some_and(char::is_alphanumeric) && !after.is_some_and(char::is_alphanumeric) {
            return true;
        }
        from = a + phrase.len().max(1);
    }
    false
}

/// Whether the question names a symptom that needs the safety line.
pub fn warning_symptoms(q: &str) -> bool {
    let q = norm(q);
    QUALIFIED_PAIN.is_match(&q) || WARNING_TERMS.iter().any(|t| has_phrase(&q, t))
}

/// Whether the question is about the asker's own case (or family's).
pub fn personal(q: &str) -> bool {
    let q = norm(q);
    let words: Vec<&str> = q
        .split(|c: char| !(c.is_alphanumeric() || c == '\''))
        .filter(|w| !w.is_empty())
        .collect();
    let first = words.iter().enumerate().any(|(i, w)| {
        FIRST_PERSON.contains(w) && !(*w == "i" && i > 0 && LABELS.contains(&words[i - 1]))
    });
    if first {
        return true;
    }
    let decision = words.iter().any(|w| DECISION_WORDS.contains(w))
        || DECISION_PHRASES.iter().any(|p| has_phrase(&q, p));
    decision && words.iter().any(|w| SHARED_PERSON.contains(w))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn not_covered_is_the_whole_reply_only() {
        for r in [
            "Not covered in this evidence.",
            "Not covered in this evidence",
            "not covered in this evidence",
            " NOT COVERED IN THIS EVIDENCE.\n",
        ] {
            assert!(is_not_covered(r), "{r:?}");
        }
        for r in [
            "",
            "Not covered",
            "Not covered in this evidence, but [1] says",
            "Not covered in this evidence..",
        ] {
            assert!(!is_not_covered(r), "{r:?}");
        }
    }

    #[test]
    fn exact_strings() {
        assert_eq!(NOT_COVERED, "Not covered in this evidence.");
        assert_eq!(
            SEE_YOUR_SURGEON,
            "Your own surgeon can tell you what this means for you."
        );
        assert_eq!(
            WARNING_LINE,
            "If you have these symptoms now, contact your surgeon or hospital straight away."
        );
        assert_eq!(UNAVAILABLE, "Chat is unavailable right now.");
        assert_eq!(BUSY, "Gemma is busy, try again in a minute.");
        assert_eq!(
            FOOTER,
            "Answers come only from this run's evidence. Educational, not medical advice."
        );
    }

    #[test]
    fn warning_words_detected() {
        for q in [
            "I have a fever after surgery",
            "There is bleeding from the wound",
            "Severe pain since last night",
            "my pain is increasing pain by the day",
            "Is chest pain normal?",
            "Breathlessness when walking",
            "I keep vomiting",
            "wound redness is spreading",
            "There is pus at the site",
            "He is unable to pass urine",
            "What if I'm short of breath?",
        ] {
            assert!(warning_symptoms(q), "{q}");
        }
    }

    #[test]
    fn warning_inflections_detected() {
        for q in [
            "she vomited twice",
            "he vomits",
            "I vomit after meals",
            "fevers at night",
            "feverish since morning",
            "he bled overnight",
            "it bleeds when I cough",
            "she is breathless",
            "I am short of breath",
            "my pain is increasing",
            "pain getting worse",
            "the pain is getting worse",
            "the pain is worse today",
            "pain worsening each day",
            "unbearable pain",
            "some redness near the cut",
            "discharge from the wound",
            "I can't pass urine",
            "he cannot pass urine",
        ] {
            assert!(warning_symptoms(q), "{q}");
        }
    }

    #[test]
    fn qualified_pain_is_a_warning_in_any_order() {
        for q in [
            "severe abdominal pain",
            "I have bad pain",
            "really bad stomach pain",
            "severe pain",
            "unbearable belly pain",
            "worsening groin pain",
            "increasing wound site pain",
            "the pain is severe",
            "pain is worse",
            "pain getting worse",
            "pain is getting worse",
            "the pain is really bad and pain severe",
            "pain worsening",
            "pain is increasing",
            "Severe\u{a0}pain",
        ] {
            assert!(warning_symptoms(q), "{q}");
        }
        // Too far apart, or not about pain.
        for q in [
            "bad outcomes and a lot of pain",
            "painless and severe",
            "Is severe painting safe?",
        ] {
            assert!(!warning_symptoms(q), "{q}");
        }
    }

    #[test]
    fn further_warning_phrases_detected() {
        for q in [
            "there is discharge from my wound",
            "discharge from the wound",
            "my wound is red",
            "a red wound",
            "I cannot urinate",
            "I can't urinate",
            "I can\u{2019}t urinate",
            "blood in my stool",
            "blood in stool",
            "coughing up blood",
            "I can't breathe",
            "I cannot breathe",
            "trouble breathing",
            "throwing up all night",
            "a high temperature",
        ] {
            assert!(warning_symptoms(q), "{q}");
        }
    }

    #[test]
    fn should_you_have_is_personal() {
        assert!(personal("Should you have the robotic operation?"));
        assert!(personal("Can we choose robotic repair?"));
        assert!(personal("Is robotic better for your recovery?"));
        assert!(personal("Will our mother go home sooner?"));
    }

    #[test]
    fn what_do_we_know_is_not_personal() {
        assert!(!personal("What do we know about recurrence?"));
        assert!(!personal("What does your evidence say about stay?"));
    }

    #[test]
    fn type_i_error_is_not_personal() {
        assert!(!personal("What is the type I error here?"));
        assert!(!personal("Were stage I patients included?"));
        assert!(personal("Can I go home sooner?"));
    }

    #[test]
    fn pain_alone_is_not_warning() {
        assert!(!warning_symptoms("How much pain after a hernia repair?"));
        assert!(!warning_symptoms("Does robotic surgery reduce pain?"));
        // Whole words only.
        assert!(!warning_symptoms("The campus feverfew study"));
        assert!(!warning_symptoms("Is the bleedingedge approach better?"));
    }

    #[test]
    fn personal_detected() {
        assert!(personal("Should I have the robotic operation?"));
        assert!(personal("Will I go home sooner?"));
        assert!(personal("What about my mother's hernia?"));
        assert!(personal("I'm worried about recovery"));
    }

    #[test]
    fn general_question_is_not_personal() {
        assert!(!personal("Does robotic repair shorten hospital stay?"));
        assert!(!personal("What is the recurrence rate at one year?"));
        assert!(!personal("Is it safe in older patients?"));
    }
}
