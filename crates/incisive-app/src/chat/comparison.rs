//! Comparison tags: which intervention-comparator pairs a passage or a
//! question is about. Pure. A wrong tag lets evidence for one comparison
//! answer a question about another, so matching is strict about words
//! (whole words only) and ignores phrases that name an arm without being
//! one ("converted to open", "open-label", "open-console").

use std::sync::LazyLock;

use regex::Regex;

use crate::memory::extract::normalise_arm;

/// (surface form, canonical arm). Longest surface forms are tried first and
/// consume their words, so "laparoscopic Heller myotomy" is one arm (`lhm`),
/// not `laparoscopic` plus a stray word.
const ALIASES: &[(&str, &str)] = &[
    ("robotic", "robotic"),
    ("robot", "robotic"),
    ("robot-assisted", "robotic"),
    ("robot assisted", "robotic"),
    ("robotic-assisted", "robotic"),
    ("robotic assisted", "robotic"),
    ("r-tapp", "robotic"),
    ("rtapp", "robotic"),
    ("rtar", "robotic"),
    ("da vinci", "robotic"),
    ("davinci", "robotic"),
    ("laparoscopic", "laparoscopic"),
    ("laparoscopy", "laparoscopic"),
    ("lap", "laparoscopic"),
    ("keyhole", "laparoscopic"),
    ("l-tapp", "laparoscopic"),
    ("ltapp", "laparoscopic"),
    ("tep", "laparoscopic"),
    ("open", "open"),
    ("lhm", "lhm"),
    ("heller", "lhm"),
    ("heller myotomy", "lhm"),
    ("laparoscopic heller", "lhm"),
    ("laparoscopic heller myotomy", "lhm"),
    ("laparoscopic myotomy", "lhm"),
    ("laparoscopic cardiomyotomy", "lhm"),
    ("pd", "pd"),
    ("pneumatic dilation", "pd"),
    ("pneumatic dilatation", "pd"),
    ("pneumatic balloon dilation", "pd"),
    ("balloon dilation", "pd"),
    ("balloon dilatation", "pd"),
    ("dilation", "pd"),
    ("dilatation", "pd"),
    ("poem", "poem"),
    ("peroral endoscopic myotomy", "poem"),
    ("per-oral endoscopic myotomy", "poem"),
    ("per oral endoscopic myotomy", "poem"),
];

/// Phrases that contain an arm word without naming that arm.
static NOT_AN_ARM: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"\b(?:conver\w*|switch\w*|crossed?\s+over)\s+(?:\w+\s+){0,3}?to\s+open\b|\bopen[\s\-]+(?:console|label\w*|access|source|trial|randomi[sz]ed)\b|\bopen\s+conver\w*",
    )
    .expect("exclusion pattern is valid")
});

/// The canonical arm for an arm string (normalised first).
fn canonical(arm: &str) -> String {
    let n = normalise_arm(arm);
    ALIASES
        .iter()
        .find(|(s, _)| *s == n)
        .map_or(n, |(_, c)| (*c).to_string())
}

/// The tag for one comparison, whichever way round the arms are given.
pub fn key(a: &str, b: &str) -> String {
    let (a, b) = (canonical(a), canonical(b));
    if a <= b {
        format!("{a}|{b}")
    } else {
        format!("{b}|{a}")
    }
}

/// Every surface form to look for: the fixed list plus each arm the record
/// itself names.
fn vocabulary(pairs: &[(String, String)]) -> Vec<(String, String)> {
    let mut v: Vec<(String, String)> = ALIASES
        .iter()
        .map(|(s, c)| ((*s).to_string(), (*c).to_string()))
        .collect();
    for arm in pairs.iter().flat_map(|(a, b)| [a, b]) {
        let n = normalise_arm(arm);
        if n.is_empty() || v.iter().any(|(s, _)| *s == n) {
            continue;
        }
        v.push((n.clone(), n));
    }
    v.sort_by_key(|(s, _)| std::cmp::Reverse(s.len()));
    v
}

/// The arms `text` names (canonical, in order of first mention), taken from
/// the fixed list and the record's own arms. Whole words only.
pub fn arms_named(text: &str, pairs: &[(String, String)]) -> Vec<String> {
    let mut low = text.to_lowercase();
    let blanks: Vec<(usize, usize)> = NOT_AN_ARM
        .find_iter(&low)
        .map(|m| (m.start(), m.end()))
        .collect();
    for (a, b) in blanks {
        low.replace_range(a..b, &" ".repeat(b - a));
    }
    let mut taken = vec![false; low.len()];
    let mut found: Vec<(usize, String)> = Vec::new();
    let is_word = |c: char| c.is_alphanumeric();
    for (surface, canon) in vocabulary(pairs) {
        let mut from = 0;
        while let Some(i) = low[from..].find(&surface) {
            let a = from + i;
            let b = a + surface.len();
            from = a + surface.len().max(1);
            let before = low[..a].chars().next_back();
            let after = low[b..].chars().next();
            if before.is_some_and(is_word) || after.is_some_and(is_word) {
                continue;
            }
            if taken[a..b].iter().any(|t| *t) {
                continue;
            }
            taken[a..b].iter_mut().for_each(|t| *t = true);
            found.push((a, canon.clone()));
        }
    }
    found.sort();
    let mut out: Vec<String> = Vec::new();
    for (_, c) in found {
        if !out.contains(&c) {
            out.push(c);
        }
    }
    out
}

/// Tags for a text: every pair among the arms it names (so a three-arm
/// study carries all three pairs). Fewer than two arms: no tags.
pub fn tags(text: &str, pairs: &[(String, String)]) -> Vec<String> {
    let arms = arms_named(text, pairs);
    let mut out: Vec<String> = Vec::new();
    for (i, a) in arms.iter().enumerate() {
        for b in &arms[i + 1..] {
            let k = key(a, b);
            if !out.contains(&k) {
                out.push(k);
            }
        }
    }
    out
}

/// The comparison a question asks about: exactly two arms named, whether or
/// not the record has that pair. One arm, or three or more (too ambiguous
/// to pick a pair), gives `None`.
pub fn asked(question: &str, pairs: &[(String, String)]) -> Option<String> {
    let arms = arms_named(question, pairs);
    match arms.as_slice() {
        [a, b] => Some(key(a, b)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(a: &str, b: &str) -> (String, String) {
        (a.to_string(), b.to_string())
    }
    fn rl() -> Vec<(String, String)> {
        vec![p("robotic", "laparoscopic")]
    }
    fn ach() -> Vec<(String, String)> {
        vec![p("lhm", "pd"), p("poem", "lhm"), p("poem", "pd")]
    }

    #[test]
    fn key_ignores_order() {
        assert_eq!(key("robotic", "open"), key("Open", "Robotic"));
        assert_eq!(
            key("keyhole", "robot-assisted"),
            key("robotic", "laparoscopic")
        );
        assert_ne!(key("robotic", "open"), key("robotic", "laparoscopic"));
    }

    #[test]
    fn keyhole_means_laparoscopic() {
        assert_eq!(
            asked("Is robotic better than keyhole surgery?", &rl()),
            Some(key("robotic", "laparoscopic"))
        );
        assert_eq!(
            arms_named("L-TAPP and r-TAPP", &rl()),
            ["laparoscopic", "robotic"]
        );
        assert_eq!(
            arms_named("TEP versus da Vinci", &rl()),
            ["laparoscopic", "robotic"]
        );
    }

    #[test]
    fn asked_finds_arms_not_in_the_record() {
        let q = "If I have robotic instead of open, will I go home sooner?";
        assert_eq!(asked(q, &rl()), Some(key("robotic", "open")));
    }

    #[test]
    fn asked_is_none_with_one_arm() {
        assert_eq!(asked("Will robotic surgery hurt less?", &rl()), None);
        assert_eq!(asked("How long is the stay?", &rl()), None);
        // Three arms: no single pair to hold the answer to.
        assert_eq!(asked("POEM or Heller or dilation?", &ach()), None);
    }

    #[test]
    fn multi_arm_study_gets_both_tags() {
        let t = tags("POEM vs LHM vs PD", &ach());
        assert!(t.contains(&key("poem", "lhm")));
        assert!(t.contains(&key("poem", "pd")));
        let t = tags("POEM vs pneumatic dilation (30/35 mm)", &ach());
        assert_eq!(t, [key("poem", "pd")]);
        let t = tags(
            "POEM vs laparoscopic myotomy + partial fundoplication",
            &ach(),
        );
        assert_eq!(t, [key("poem", "lhm")]);
    }

    #[test]
    fn untaggable_text_has_no_tags() {
        assert!(tags("Operative time was shorter.", &rl()).is_empty());
        assert!(tags("Robotic surgery costs more.", &rl()).is_empty());
        assert!(tags("", &rl()).is_empty());
    }

    #[test]
    fn words_must_be_whole() {
        assert!(arms_named("The opening was wide; robotics grew; a lapse", &rl()).is_empty());
    }

    #[test]
    fn phrases_that_are_not_arms_are_ignored() {
        // A robotic-vs-laparoscopic row that mentions conversion to open.
        let t = tags(
            "Conversion to open was lower with robotic than laparoscopic",
            &rl(),
        );
        assert_eq!(t, [key("robotic", "laparoscopic")]);
        assert_eq!(
            tags("Open-console robotic vs laparoscopic repair", &rl()),
            [key("robotic", "laparoscopic")]
        );
        assert_eq!(
            arms_named("an open-label trial of robotic surgery", &rl()),
            ["robotic"]
        );
        assert_eq!(
            arms_named("converted to open surgery (robotic)", &rl()),
            ["robotic"]
        );
    }

    #[test]
    fn open_is_an_arm_when_it_is_one() {
        assert_eq!(
            tags("Robotic vs laparoscopic or open emergency surgery", &rl()),
            [
                key("robotic", "laparoscopic"),
                key("robotic", "open"),
                key("laparoscopic", "open")
            ]
        );
    }

    #[test]
    fn record_arms_outside_the_fixed_list_are_matched() {
        let pairs = vec![p("tar", "ventral rectopexy")];
        assert_eq!(
            asked("TAR or ventral rectopexy, which lasts longer?", &pairs),
            Some(key("tar", "ventral rectopexy"))
        );
    }
}
