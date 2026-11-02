//! Reply checks (spec section 5, steps 6-7). Pure. A check marks a reply and
//! never removes text.

use std::sync::LazyLock;

use regex::Regex;
use serde::{Deserialize, Serialize};

use super::comparison::arms_named;
use super::passages::{NOT_TRACED_MARK, RETRACTED_MARK};
use crate::agent::extractor::has_own_sign;
use crate::agent::temporal_writer::{text_numbers, traced};

/// What the checks found in one reply.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Checks {
    pub not_traced: Vec<String>,
    pub unknown_citations: Vec<u32>,
    pub outside_references: Vec<String>,
    pub comparison_mismatch: Vec<String>,
    pub you_should: bool,
    pub uncited: bool,
    /// A cited passage rests on a retracted paper.
    pub retracted_source: bool,
}

impl Checks {
    pub fn is_clean(&self) -> bool {
        self.not_traced.is_empty()
            && self.unknown_citations.is_empty()
            && self.outside_references.is_empty()
            && self.comparison_mismatch.is_empty()
            && !self.you_should
            && !self.uncited
            && !self.retracted_source
    }
}

/// Bare whole numbers up to this, with no `%` and no unit or rate word after
/// them, are counts, round numbers and codes, not findings: never flagged.
const SMALL_INT: f64 = 12.0;

/// What makes a small whole number a finding: a unit or rate word right after
/// it, or after the rest of its range ("3 days", "3-day", "7-10 days", "7 to
/// 10 days", "2 of 5"). Spec section 15, P13.
static UNIT_AFTER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)^(?:\s*(?:[\-\u{2013}\u{2014}]|to\b)\s*\d+(?:\.\d+)?)?[\s\-]*(?:%|(?:days?|weeks?|months?|years?|hours?|minutes?|mm|cm|ml|kg|percent|per\s+cent|of|in|out\s+of)\b)",
    )
    .expect("unit after")
});

/// `[1]`, `[2, 3]`, `[1-3]`: citation markers.
static MARKER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\[\s*\d+(?:\s*[,\-\u{2013}]\s*\d+)*\s*\]").expect("marker"));

/// "Lee et al. 2021", "Lee et al., (2021)", "Lee et al."
static AUTHOR_ETAL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\b([A-Z][A-Za-z'\u{2019}\-]+)\s+et\s+al\b\.?(?:,?\s*\(?((?:19|20)\d{2})\)?)?")
        .expect("author")
});

/// "J Robot Surg 2019;33:12", "Ann Surg 2020;271(3):450-6".
static JOURNAL_CITE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"\b(?:[A-Z][A-Za-z]*\.?\s+){1,4}(?:19|20)\d{2}\s*;\s*\d+(?:\s*\(\d+\))?(?:\s*:\s*[\dA-Za-z\-\u{2013}]+)?",
    )
    .expect("journal")
});

/// A DOI or a PMID written out.
static ID_CITE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\bPMID:?\s*\d{5,9}\b|\b10\.\d{4,9}/[^\s)\]]+").expect("id"));

static YOU_SHOULD: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)\byou\s+(?:should|shouldn['\u{2019}]t|must|mustn['\u{2019}]t|ought|need\s+to|have\s+to|had\s+better)\b|\bi\s+(?:recommend|advise|suggest)\b",
    )
    .expect("you should")
});

/// A marker and the space before it, for taking markers out of a text.
static MARKER_AND_SPACE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\s*\[\s*\d+(?:\s*[,\-\u{2013}]\s*\d+)*\s*\]").expect("marker and space")
});

/// `text` without its citation markers. An earlier reply's numbers point at
/// that turn's passages, so they must not reach a later prompt.
pub fn strip_citations(text: &str) -> String {
    MARKER_AND_SPACE.replace_all(text, "").trim().to_string()
}

fn marker_numbers(m: &str) -> Vec<u32> {
    let mut out = Vec::new();
    // Split into comma-separated parts; "a-b" expands to a..=b.
    for part in m.split(',') {
        let nums: Vec<u32> = part
            .split(|c: char| !c.is_ascii_digit())
            .filter(|d| !d.is_empty())
            .filter_map(|d| d.parse().ok())
            .collect();
        match nums.as_slice() {
            [a, b] if a <= b && b - a <= 50 => out.extend(*a..=*b),
            _ => out.extend(nums),
        }
    }
    out
}

fn blank(text: &str, spans: &[(usize, usize)]) -> String {
    let mut out = text.to_string();
    for (a, b) in spans {
        out.replace_range(*a..*b, &" ".repeat(b - a));
    }
    out
}

fn squash(s: &str) -> String {
    s.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// Reference-like strings in `reply` that no passage contains.
fn references(reply: &str, passages: &[&str]) -> Vec<(usize, usize, String)> {
    let hay = squash(&passages.join(" "));
    let mut found: Vec<(usize, usize, String)> = Vec::new();
    for m in AUTHOR_ETAL.captures_iter(reply) {
        let whole = m.get(0).expect("whole");
        let surname = m[1].to_lowercase();
        let year_ok = m.get(2).is_none_or(|y| hay.contains(y.as_str()));
        if hay.contains(&format!("{surname} et al")) && year_ok {
            continue;
        }
        found.push((
            whole.start(),
            whole.end(),
            whole.as_str().trim().to_string(),
        ));
    }
    for m in JOURNAL_CITE
        .find_iter(reply)
        .chain(ID_CITE.find_iter(reply))
    {
        if hay.contains(&squash(m.as_str())) {
            continue;
        }
        found.push((m.start(), m.end(), m.as_str().trim().to_string()));
    }
    found.sort();
    found.dedup_by(|b, a| {
        b.0 < a.1 && {
            a.1 = a.1.max(b.1);
            true
        }
    });
    found
}

/// Check one reply. `passages` are numbered from 1 in citations; `asked` is
/// the comparison key of the question (`laparoscopic|robotic`), if any.
pub fn run(
    reply: &str,
    passages: &[&str],
    asked: Option<&str>,
    pairs: &[(String, String)],
) -> Checks {
    let mut out = Checks::default();

    // Citations.
    let mut cited: Vec<u32> = MARKER
        .find_iter(reply)
        .flat_map(|m| marker_numbers(m.as_str()))
        .collect();
    cited.sort_unstable();
    cited.dedup();
    out.uncited = cited.is_empty();
    out.unknown_citations = cited
        .iter()
        .copied()
        .filter(|n| *n == 0 || *n as usize > passages.len())
        .collect();

    // Markers are not numbers: blank them in the reply and in the passages.
    let spans: Vec<(usize, usize)> = MARKER
        .find_iter(reply)
        .map(|m| (m.start(), m.end()))
        .collect();
    let body = blank(reply, &spans);

    // References that no passage holds; their years are not stray numbers.
    let refs = references(&body, passages);
    out.outside_references = refs.iter().map(|r| r.2.clone()).collect();
    let ref_spans: Vec<(usize, usize)> = refs.iter().map(|r| (r.0, r.1)).collect();

    // Numbers, against the union of the cited passages.
    let mut allowed: Vec<f64> = Vec::new();
    for n in &cited {
        if let Some(p) = n.checked_sub(1).and_then(|i| passages.get(i as usize)) {
            out.retracted_source |= p.contains(RETRACTED_MARK);
            // The mark does not say how far back its run goes, and it can
            // follow a whole field, so such a passage vouches for no number.
            if p.contains(NOT_TRACED_MARK) {
                continue;
            }
            let clean = MARKER.replace_all(p, " ");
            for (_, _, vals, _) in text_numbers(&clean) {
                allowed.extend(vals);
            }
        }
    }
    for (s, e, vals, dp) in text_numbers(&body) {
        if ref_spans.iter().any(|(a, b)| s >= *a && e <= *b) {
            continue;
        }
        let token = &body[s..e];
        let whole = dp == 0 && !token.ends_with('%') && !UNIT_AFTER.is_match(&body[e..]);
        let own = has_own_sign(&body, s);
        let ok = vals.iter().all(|v| {
            (whole && v.fract() == 0.0 && v.abs() <= SMALL_INT) || traced(*v, dp, own, &allowed)
        });
        if !ok {
            out.not_traced.push(token.to_string());
        }
    }

    // Arms outside the asked comparison.
    if let Some(key) = asked {
        let want: Vec<&str> = key.split('|').collect();
        out.comparison_mismatch = arms_named(&body, pairs)
            .into_iter()
            .filter(|a| !want.contains(&a.as_str()))
            .collect();
    }

    out.you_should = YOU_SHOULD.is_match(&body);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_citations_removes_every_marker_form() {
        assert_eq!(
            strip_citations("Lower [1]. Stay [2, 3] fell [1-2]"),
            "Lower. Stay fell"
        );
        assert_eq!(strip_citations("[1] First."), "First.");
        assert_eq!(
            strip_citations("Grade [a] and 5 [ 4 \u{2013} 6 ]"),
            "Grade [a] and 5"
        );
        assert_eq!(strip_citations("none"), "none");
    }

    fn pairs() -> Vec<(String, String)> {
        Vec::new()
    }

    const P1: &str = "Mean length of stay was 4.17 days with robotic repair; recurrence 3.5%.";
    const P2: &str = "Operating time rose by 18 minutes; conversion in 15 of 200 cases.";

    #[test]
    fn number_in_cited_passage_passes() {
        let c = run("Stay was 4.17 days [1].", &[P1, P2], None, &pairs());
        assert!(c.not_traced.is_empty(), "{:?}", c.not_traced);
        assert!(c.is_clean());
    }

    #[test]
    fn number_in_uncited_passage_is_not_traced() {
        // 18 is only in passage 2, which the reply does not cite.
        let c = run(
            "Operating time rose by 18 minutes [1].",
            &[P1, P2],
            None,
            &pairs(),
        );
        assert_eq!(c.not_traced, vec!["18".to_string()]);
    }

    #[test]
    fn union_of_cited_passages_is_used() {
        let c = run(
            "Stay 4.17 days [1]; 18 minutes longer [2].",
            &[P1, P2],
            None,
            &pairs(),
        );
        assert!(c.not_traced.is_empty(), "{:?}", c.not_traced);
    }

    #[test]
    fn citation_markers_are_not_numbers() {
        let c = run(
            "Stay was short [1] and the time longer [2][3].",
            &[P1, P2, "x"],
            None,
            &pairs(),
        );
        assert!(c.not_traced.is_empty(), "{:?}", c.not_traced);
        assert!(c.unknown_citations.is_empty());
        // A marker that is a number in the passage text does not leak either.
        let c = run("Rate 3.5% [1].", &["Rate 3.5% [3]."], None, &pairs());
        assert!(c.not_traced.is_empty());
    }

    #[test]
    fn rounded_number_traces() {
        let c = run("About 4.2 days [1].", &[P1], None, &pairs());
        assert!(c.not_traced.is_empty(), "{:?}", c.not_traced);
        let c = run("About 4.3 days [1].", &[P1], None, &pairs());
        assert_eq!(c.not_traced, vec!["4.3".to_string()]);
    }

    #[test]
    fn small_counts_are_not_flagged() {
        let c = run("Two trials and 3 studies agree [1].", &[P1], None, &pairs());
        assert!(c.not_traced.is_empty(), "{:?}", c.not_traced);
        // A percentage is never a small count.
        let c = run("Recurrence was 5% [1].", &[P1], None, &pairs());
        assert_eq!(c.not_traced, vec!["5%".to_string()]);
    }

    #[test]
    fn range_is_checked() {
        let c = run("Stay was 15-20 days [1].", &[P2], None, &pairs());
        assert_eq!(c.not_traced, vec!["20".to_string()]);
        // Both ends of a range carry its unit.
        for t in ["Stay was 7\u{2013}10 days [1].", "Stay was 7-10 days [1]."] {
            let c = run(t, &[P1], None, &pairs());
            assert_eq!(c.not_traced, vec!["7".to_string(), "10".to_string()], "{t}");
        }
        let c = run("Stay was 7 to 10 days [1].", &[P1], None, &pairs());
        assert_eq!(c.not_traced, vec!["7".to_string(), "10".to_string()]);
        let c = run("Stay was 7\u{2013}14 days [1].", &[P1], None, &pairs());
        assert_eq!(c.not_traced, vec!["7".to_string(), "14".to_string()]);
        // Held by the cited passage: nothing to flag.
        let c = run("Stay was 7-10 days [1].", &["Stay 7 to 10 days."], None, &pairs());
        assert!(c.not_traced.is_empty(), "{:?}", c.not_traced);
    }

    #[test]
    fn a_small_number_with_a_unit_or_rate_word_is_a_finding() {
        for (t, n) in [
            ("You go home 3 days sooner [1].", "3"),
            ("Back at work in 2 weeks [1].", "2"),
            ("Recovery took 1 week [1].", "1"),
            ("After 6 months [1].", "6"),
            ("At 5 years [1].", "5"),
            ("A 3-year follow-up [1].", "3"),
            ("About 2 hours [1].", "2"),
            ("Saved 9 minutes [1].", "9"),
            ("Mesh of 5 mm [1].", "5"),
            ("Defect of 4cm [1].", "4"),
            ("Blood loss of 10 ml [1].", "10"),
            ("Lost 2 kg [1].", "2"),
            ("A rate of 5 percent [1].", "5"),
            ("A rate of 5 per cent [1].", "5"),
            ("A rate of 5 % [1].", "5"),
            ("Seen in 4 of the patients [1].", "4"),
            ("About 1 in 5 [1].", "1"),
            ("About 3 out of 8 [1].", "3"),
            ("3 DAYS sooner [1].", "3"),
        ] {
            let c = run(t, &[P2], None, &pairs());
            assert!(c.not_traced.contains(&n.to_string()), "{t}: {:?}", c.not_traced);
        }
    }

    #[test]
    fn a_small_number_counting_things_stays_exempt() {
        for t in [
            "Two trials agree [1].",
            "3 studies agree [1].",
            "3 trials agree [1].",
            "It was grade 2 [1].",
            "Round 3 [1].",
            "3 injuries and 4 officers [1].",
        ] {
            let c = run(t, &[P2], None, &pairs());
            assert!(c.not_traced.is_empty(), "{t}: {:?}", c.not_traced);
        }
    }

    #[test]
    fn citing_a_passage_on_a_retracted_paper_is_flagged() {
        let bad = format!("Stay 4.17 days. {RETRACTED_MARK}");
        let c = run("Stay 4.17 days [1].", &[&bad], None, &pairs());
        assert!(c.retracted_source);
        assert!(c.not_traced.is_empty(), "{:?}", c.not_traced);
        assert!(!c.is_clean());
        // Retrieved but not cited: nothing to flag.
        let c = run("Mean stay 4.17 days [1].", &[P1, &bad], None, &pairs());
        assert!(!c.retracted_source);
        assert!(c.is_clean(), "{c:?}");
    }

    #[test]
    fn numbers_in_a_cited_passage_marked_not_traced_are_not_allowed() {
        let marked = "Stay 4.5 days (not traced); 18 minutes longer.";
        let c = run("Stay was 4.5 days [1].", &["Stay 4.5 days (not traced)"], None, &pairs());
        assert_eq!(c.not_traced, vec!["4.5".to_string()]);
        // The mark does not say how far back its run goes, so nothing counts.
        let c = run("18 minutes longer [1].", &[marked], None, &pairs());
        assert_eq!(c.not_traced, vec!["18".to_string()]);
        // Another cited passage still holds it, and text is kept either way.
        let c = run("Stay was 4.5 days [1, 2].", &[marked, "Stay 4.5 days."], None, &pairs());
        assert!(c.not_traced.is_empty(), "{:?}", c.not_traced);
    }

    #[test]
    fn no_citation_means_uncited_and_untraced() {
        // No cited passage: nothing to trace against, so a number is flagged.
        let c = run("Stay was 99 days.", &[P1], None, &pairs());
        assert!(c.uncited);
        assert_eq!(c.not_traced, vec!["99".to_string()]);
        assert!(!c.is_clean());
    }

    #[test]
    fn reply_without_citation_flagged_uncited() {
        assert!(run("Robotic repair is fine.", &[P1], None, &pairs()).uncited);
        assert!(!run("Robotic repair is fine [1].", &[P1], None, &pairs()).uncited);
    }

    #[test]
    fn citation_range_cites_every_passage_in_it() {
        let c = run(
            "Stay 18 minutes longer [1-3].",
            &[P1, P2, "x"],
            None,
            &pairs(),
        );
        assert!(c.not_traced.is_empty(), "{:?}", c.not_traced);
        assert!(!c.uncited);
        let c = run(
            "Longer [1\u{2013}2], and [2-5].",
            &[P1, P2, "x"],
            None,
            &pairs(),
        );
        assert_eq!(c.unknown_citations, vec![4, 5]);
    }

    #[test]
    fn unknown_citation_flagged() {
        let c = run("See [4] and [1] and [0].", &[P1, P2, "x"], None, &pairs());
        assert_eq!(c.unknown_citations, vec![0, 4]);
        assert!(!c.uncited);
    }

    #[test]
    fn year_is_checked_like_any_whole_number() {
        // A year in a cited passage traces; one that is not there is flagged.
        let c = run("Done in 2019 [1].", &["Enrolled in 2019."], None, &pairs());
        assert!(c.not_traced.is_empty());
        let c = run("Done in 2018 [1].", &["Enrolled in 2019."], None, &pairs());
        assert_eq!(c.not_traced, vec!["2018".to_string()]);
    }

    #[test]
    fn author_year_flagged() {
        let c = run("Shown by Lee et al. 2021 [1].", &[P1], None, &pairs());
        assert_eq!(c.outside_references.len(), 1, "{:?}", c.outside_references);
        assert!(c.outside_references[0].contains("Lee et al"));
        let c = run(
            "Reported in J Robot Surg 2019;33:12 [1].",
            &[P1],
            None,
            &pairs(),
        );
        assert_eq!(c.outside_references.len(), 1, "{:?}", c.outside_references);
        assert!(c.outside_references[0].contains("2019;33:12"));
        // The year inside a reference is not also a stray number.
        assert!(c.not_traced.is_empty(), "{:?}", c.not_traced);
    }

    #[test]
    fn reference_present_in_cited_text_passes() {
        let c = run(
            "Lee et al. 2021 found this [1].",
            &["Lee et al. 2021 found a shorter stay."],
            None,
            &pairs(),
        );
        assert!(
            c.outside_references.is_empty(),
            "{:?}",
            c.outside_references
        );
    }

    #[test]
    fn other_arm_flagged() {
        let c = run(
            "Laparoscopic repair had less pain, robotic had less bleeding [1].",
            &[P1],
            Some("open|robotic"),
            &pairs(),
        );
        assert_eq!(c.comparison_mismatch, vec!["laparoscopic".to_string()]);
    }

    #[test]
    fn no_asked_comparison_no_arm_flag() {
        let c = run(
            "Laparoscopic and robotic both work [1].",
            &[P1],
            None,
            &pairs(),
        );
        assert!(c.comparison_mismatch.is_empty());
        let c = run(
            "Robotic beat open [1].",
            &[P1],
            Some("open|robotic"),
            &pairs(),
        );
        assert!(c.comparison_mismatch.is_empty());
    }

    #[test]
    fn you_should_flagged_any_case() {
        for t in [
            "You should wait [1].",
            "YOU SHOULD wait [1].",
            "You must rest [1].",
            "I recommend surgery [1].",
        ] {
            assert!(run(t, &[P1], None, &pairs()).you_should, "{t}");
        }
        assert!(
            !run(
                "Patients should be selected carefully [1].",
                &[P1],
                None,
                &pairs()
            )
            .you_should
        );
    }

    #[test]
    fn clean_reply_is_clean() {
        let c = run(
            "Stay was 4.17 days [1].",
            &[P1],
            Some("open|robotic"),
            &pairs(),
        );
        assert!(c.is_clean(), "{c:?}");
    }

    #[test]
    fn regression_invented_references_are_flagged() {
        // The 2 Nov reply written with no passage at all.
        let reply = "Robotic TAR shortens stay, 7-10 days vs 10-14 days (Lee et al. 2021; \
J Robot Surg 2019;33:12). A second series found 7.5 vs 10.2 days.";
        let c = run(reply, &[], None, &pairs());
        assert!(!c.outside_references.is_empty(), "{c:?}");
        assert!(c.uncited);
        for n in ["14", "7.5", "10.2"] {
            assert!(
                c.not_traced.contains(&n.to_string()),
                "{n}: {:?}",
                c.not_traced
            );
        }
        let cited = format!("{reply} [1]");
        let with_passage = run(&cited, &["Stay was 4 days."], None, &pairs());
        assert!(!with_passage.outside_references.is_empty());
        let c = with_passage;
        for n in ["14", "7.5", "10.2"] {
            assert!(
                c.not_traced.contains(&n.to_string()),
                "{n}: {:?}",
                c.not_traced
            );
        }
    }

    #[test]
    fn serde_round_trip() {
        let c = run("You should [9].", &[P1], None, &pairs());
        let j = serde_json::to_string(&c).unwrap();
        let d: Checks = serde_json::from_str(&j).unwrap();
        assert_eq!(c, d);
    }
}
