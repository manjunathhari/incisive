//! The coverage check (Phase 3 Stage 4b item 1). Server only; code, no AI.
//!
//! After the writer finishes (Full mode), compare what the team verified
//! with what the written answer says:
//!
//! * **Numbers.** Every verified structured number of every verified
//!   extracted outcome counts as covered when its value appears anywhere in
//!   the answer: a number written in any text field (outside citation
//!   markers) or any structured number's value or CI bound. The tokeniser
//!   and number matcher are the extractor's (`extractor::written_numbers`,
//!   ranges, COVID-19, middle dots, spaced minus), so there is one
//!   implementation; a sign difference still matches ("13.4 min shorter"
//!   covers "−13.4", [`super::extractor::same_magnitude`]).
//! * **Outcomes.** A verified extracted outcome is covered when any of its
//!   verified numbers is covered. With no verified number, it is covered
//!   when every content word of its name (4+ letters) appears in one of the
//!   answer's outcome rows (name, RCT evidence or effect text). An outcome
//!   that is not covered is one gap carrying all its numbers; a covered
//!   outcome's missing numbers are gaps of their own, "pooled" when the
//!   study is one the researcher labelled a meta-analysis.
//! * **Skeptic items.** Contradicting evidence, missed trials, meta-analysis
//!   notes, guidelines and overlaps count as covered when their PMID (an
//!   overlap: each of its PMIDs) is cited anywhere in the writer's own
//!   sections: a `[PMID]` marker in any text field, or a row's, number's,
//!   trial's, conflict's, key number's or notable finding's PMID. The
//!   skeptic box and the code-built notable findings are not the writer's
//!   and do not count.
//!
//! Outcomes and numbers the extractor could not verify are never gaps: the
//! writer is told not to rest anything on them. Nothing here changes the
//! answer; the gaps are the completeness writer's input and the page's
//! counts.

use std::collections::HashSet;

use super::answer_v3::{
    CoverageCounts, CoverageGap, CoverageV3, GapKind, MAX_STORED_GAPS, Segment, WrittenAnswer,
    citation_ranges, split_citations, text_fields,
};
use super::extractor::{EvidenceRow, RowNumber, same_magnitude, written_numbers};
use super::skeptic::SkepticReview;

/// Characters kept of a gap's text.
const MAX_GAP_TEXT: usize = 600;

/// Every number the answer writes: the numbers in its text fields outside
/// citation markers, and each structured number's value and CI bounds.
pub fn answer_numbers(w: &WrittenAnswer, known: impl Fn(&str) -> bool) -> Vec<f64> {
    let mut out = Vec::new();
    for (_, text) in text_fields(w) {
        let cites = citation_ranges(text, &known);
        for (s, e, v) in written_numbers(text) {
            if !cites.iter().any(|(a, b)| s >= *a && e <= *b) {
                out.extend(v);
            }
        }
    }
    for o in &w.outcomes {
        for n in &o.numbers {
            out.push(n.value);
            out.extend(n.ci_low);
            out.extend(n.ci_high);
        }
    }
    for s in &w.studies {
        out.extend(s.n.map(|n| n as f64));
    }
    out
}

/// Every PMID the writer's own sections cite (see the module docs).
pub fn cited_pmids(w: &WrittenAnswer, known: impl Fn(&str) -> bool) -> HashSet<String> {
    let mut out: HashSet<String> = HashSet::new();
    for (_, text) in text_fields(w) {
        for seg in split_citations(text, &known) {
            if let Segment::Cite(ids) = seg {
                out.extend(ids);
            }
        }
    }
    for o in &w.outcomes {
        out.extend(o.pmids.iter().cloned());
        out.extend(o.numbers.iter().map(|n| n.pmid.clone()));
    }
    out.extend(w.studies.iter().map(|s| s.pmid.clone()));
    for c in &w.conflicts {
        out.extend(c.pmids.iter().cloned());
    }
    for n in &w.notable_findings {
        out.extend(n.pmids.iter().cloned());
    }
    if let Some(g) = &w.at_a_glance {
        out.extend(g.key_numbers.iter().map(|k| k.pmid.clone()));
    }
    for g in &w.guidelines {
        out.extend(g.pmid.clone());
    }
    out.into_iter()
        .map(|p| p.trim().to_string())
        .filter(|p| !p.is_empty())
        .collect()
}

/// Lower-cased words of 4+ letters, minus a few that carry no outcome.
fn content_words(s: &str) -> Vec<String> {
    const STOP: [&str; 10] = [
        "with", "from", "after", "rate", "rates", "time", "than", "that", "this", "were",
    ];
    let mut v: Vec<String> = s
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.chars().filter(|c| c.is_alphabetic()).count() >= 4)
        .map(str::to_lowercase)
        .filter(|w| !STOP.contains(&w.as_str()))
        .collect();
    v.dedup();
    v
}

fn clip_text(s: &str) -> String {
    let s = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if s.len() <= MAX_GAP_TEXT {
        return s;
    }
    let mut end = MAX_GAP_TEXT;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &s[..end])
}

fn fmt_num(v: f64) -> String {
    if v.fract() == 0.0 && v.abs() < 1e15 {
        format!("{v:.0}")
    } else {
        format!("{v}")
    }
}

/// One structured number in words: "label = value unit (arm), CI a to b, p x".
pub fn number_words(n: &RowNumber) -> String {
    let mut t = format!("{} = {}", n.label.trim(), fmt_num(n.value));
    if let Some(u) = n.unit.as_deref().filter(|u| !u.trim().is_empty()) {
        t.push_str(&format!(" {}", u.trim()));
    }
    if let Some(a) = n.arm.as_deref().filter(|a| !a.trim().is_empty()) {
        t.push_str(&format!(" ({})", a.trim()));
    }
    if let (Some(l), Some(h)) = (n.ci_low, n.ci_high) {
        t.push_str(&format!(", CI {} to {}", fmt_num(l), fmt_num(h)));
    }
    if let Some(p) = n.p.as_deref().filter(|p| !p.trim().is_empty()) {
        t.push_str(&format!(", p {}", p.trim()));
    }
    t
}

fn values(n: &RowNumber) -> Vec<f64> {
    [Some(n.value), n.ci_low, n.ci_high]
        .into_iter()
        .flatten()
        .collect()
}

fn count(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

/// The coverage check (see the module docs). `is_meta` says whether a
/// study is one the researcher labelled a meta-analysis; `known` whether a
/// PMID is a given study (for citation markers).
pub fn check(
    w: &WrittenAnswer,
    rows: &[EvidenceRow],
    is_meta: impl Fn(&str) -> bool,
    review: Option<&SkepticReview>,
    known: impl Fn(&str) -> bool,
) -> CoverageV3 {
    let have = answer_numbers(w, &known);
    let covered = |v: f64| have.iter().any(|a| same_magnitude(*a, v));
    let rows_text: Vec<String> = w
        .outcomes
        .iter()
        .map(|o| format!("{} {} {}", o.outcome, o.rct_evidence, o.effect_text).to_lowercase())
        .collect();
    let mut c = CoverageCounts::default();
    let mut outcome_gaps: Vec<CoverageGap> = Vec::new();
    let mut pooled_gaps: Vec<CoverageGap> = Vec::new();
    let mut number_gaps: Vec<CoverageGap> = Vec::new();
    for r in rows {
        let meta = is_meta(&r.pmid);
        for o in r.outcomes.iter().filter(|o| o.verified) {
            c.outcomes_checked += 1;
            let nums: Vec<&RowNumber> = o.numbers.iter().filter(|n| n.verified).collect();
            c.numbers_checked += count(nums.len());
            let missing: Vec<&RowNumber> =
                nums.iter().copied().filter(|n| !covered(n.value)).collect();
            let outcome_covered = if nums.is_empty() {
                let words = content_words(&o.outcome);
                words.is_empty()
                    || rows_text
                        .iter()
                        .any(|t| words.iter().all(|wd| t.contains(wd.as_str())))
            } else {
                missing.len() < nums.len()
            };
            c.numbers_missing += count(missing.len());
            if meta {
                c.pooled_missing += count(missing.len());
            }
            if !outcome_covered {
                c.outcomes_missing += 1;
                let mut text = o.effect_text.clone();
                if !missing.is_empty() {
                    let words: Vec<String> = missing.iter().map(|n| number_words(n)).collect();
                    text.push_str(&format!(" ({})", words.join("; ")));
                }
                outcome_gaps.push(CoverageGap {
                    kind: GapKind::Outcome,
                    pmid: r.pmid.clone(),
                    outcome: clip_text(&o.outcome),
                    text: clip_text(&text),
                    numbers: missing.iter().flat_map(|n| values(n)).collect(),
                });
                continue;
            }
            for n in missing {
                let gap = CoverageGap {
                    kind: if meta {
                        GapKind::Pooled
                    } else {
                        GapKind::Number
                    },
                    pmid: r.pmid.clone(),
                    outcome: clip_text(&o.outcome),
                    text: clip_text(&number_words(n)),
                    numbers: values(n),
                };
                if meta {
                    pooled_gaps.push(gap);
                } else {
                    number_gaps.push(gap);
                }
            }
        }
    }
    let mut skeptic_gaps: Vec<CoverageGap> = Vec::new();
    if let Some(k) = review {
        let cited = cited_pmids(w, &known);
        let mut item = |pmids: &[&String], text: String| {
            c.skeptic_checked += 1;
            if pmids.is_empty() || pmids.iter().all(|p| cited.contains(p.as_str())) {
                return;
            }
            c.skeptic_missing += 1;
            let first = pmids
                .iter()
                .find(|p| !cited.contains(p.as_str()))
                .map_or(String::new(), |p| (*p).clone());
            skeptic_gaps.push(CoverageGap {
                kind: GapKind::SkepticItem,
                pmid: first,
                outcome: String::new(),
                text: clip_text(&text),
                numbers: Vec::new(),
            });
        };
        for x in &k.contradicting {
            item(
                &[&x.pmid],
                format!(
                    "Evidence against (the skeptic): {} \u{2014} quote: \u{201c}{}\u{201d}",
                    x.claim, x.quote
                ),
            );
        }
        for x in &k.missed_rcts {
            item(
                &[&x.pmid],
                format!(
                    "A randomised trial the search missed (the skeptic): {}",
                    x.why
                ),
            );
        }
        for x in &k.ma_notes {
            item(
                &[&x.pmid],
                format!("What a meta-analysis pooled (the skeptic): {}", x.note),
            );
        }
        for x in &k.guidelines {
            item(
                &[&x.pmid],
                format!(
                    "Guideline (the skeptic): {} \u{2014} \u{201c}{}\u{201d}",
                    x.body, x.statement_quote
                ),
            );
        }
        for x in &k.overlaps {
            let ids: Vec<&String> = x.pmids.iter().collect();
            item(&ids, format!("Not independent (the skeptic): {}", x.note));
        }
    }
    // Stored in this order (outcomes, pooled estimates, skeptic items, other
    // numbers), at most MAX_STORED_GAPS; the counts cover them all.
    let mut gaps: Vec<CoverageGap> = outcome_gaps
        .into_iter()
        .chain(pooled_gaps)
        .chain(skeptic_gaps)
        .chain(number_gaps)
        .collect();
    if gaps.len() > MAX_STORED_GAPS {
        c.gaps_cut = count(gaps.len() - MAX_STORED_GAPS);
        gaps.truncate(MAX_STORED_GAPS);
    }
    CoverageV3 { gaps, counts: c }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::answer_v3::{
        Applicability, BottomLine, Certainty, Conflict, EvidenceType, Number, Outcome,
    };
    use crate::agent::extractor::{QuoteSource, VerifiedOutcome};
    use crate::agent::skeptic::{Contradicting, MaNote, MissedRct, Overlap, Verdict};

    fn rn(label: &str, value: f64) -> RowNumber {
        RowNumber {
            label: label.into(),
            value,
            unit: None,
            arm: None,
            ci_low: None,
            ci_high: None,
            p: None,
            verified: true,
        }
    }

    fn vo(name: &str, numbers: Vec<RowNumber>) -> VerifiedOutcome {
        VerifiedOutcome {
            outcome: name.into(),
            effect_text: format!("{name} effect"),
            numbers,
            quote: "q".into(),
            source: QuoteSource::Abstract,
            verified: true,
        }
    }

    fn row(pmid: &str, outcomes: Vec<VerifiedOutcome>) -> EvidenceRow {
        let mut r: EvidenceRow = serde_json::from_value(serde_json::json!({
            "pmid": pmid, "text": "abstract", "design": "RCT", "n_randomised": null,
            "n_analysed": null, "setting": "", "centres": "single", "country": "", "population": "",
            "intervention": "", "comparator": "", "follow_up": "", "blinding": "",
            "primary_endpoint": "", "powered_for_primary": null, "surgeon_experience": "",
            "outcomes": [], "appraisal": {"bias": "", "power": "", "learning_curve": "", "applicability": ""},
            "independence": {"trial_registration": null, "group_or_centre": "", "note": ""},
            "unverified_outcomes": [], "outcomes_verified": 0,
            "numbers_unverified": 0, "corrections": 0
        }))
        .unwrap();
        r.outcomes = outcomes;
        r
    }

    fn answer(outcomes: Vec<Outcome>, bottom: &str) -> WrittenAnswer {
        WrittenAnswer {
            bottom_line: BottomLine {
                text: bottom.into(),
                certainty: Certainty::Low,
                certainty_reason: "small trials".into(),
            },
            outcomes,
            studies: vec![],
            conflicts: vec![],
            applicability: Applicability {
                case_mix: String::new(),
                india_cost_context: String::new(),
                training: String::new(),
            },
            would_change: vec![],
            at_a_glance: None,
            clinical_practice: None,
            guidelines: vec![],
            notable_findings: vec![],
        }
    }

    fn out(name: &str, effect: &str, numbers: Vec<Number>, pmids: &[&str]) -> Outcome {
        Outcome {
            outcome: name.into(),
            rct_evidence: "RCT".into(),
            effect_text: effect.into(),
            numbers,
            certainty: Certainty::Low,
            why: String::new(),
            pmids: pmids.iter().map(|p| p.to_string()).collect(),
            evidence_type: EvidenceType::Unknown,
        }
    }

    fn known(p: &str) -> bool {
        ["11111111", "22222222", "33333333"].contains(&p)
    }

    /// Numbers match through ranges, the minus sign (U+2212 or hyphen),
    /// prose without the sign ("13.4 min shorter"), and structured numbers;
    /// a citation's PMID is not a number of the answer.
    #[test]
    fn numbers_match_ranges_signs_and_structured_values() {
        let rows = vec![row(
            "11111111",
            vec![
                vo(
                    "Operative time",
                    vec![rn("MD", -13.4), rn("CI low", 0.48), rn("CI high", 1.13)],
                ),
                vo("Pain", vec![rn("VAS", -2.3)]),
                vo("Cost", vec![rn("USD", 3258.0)]),
                vo("Stay", vec![rn("days", 22222222.0)]),
            ],
        )];
        let w = answer(
            vec![
                out(
                    "Operative time",
                    "13.4 min shorter (CI 0.48-1.13) [11111111]",
                    vec![],
                    &["11111111"],
                ),
                out("Pain", "VAS \u{2212}2.3 [11111111]", vec![], &["11111111"]),
                out(
                    "Cost",
                    "higher [22222222]",
                    vec![Number {
                        label: "cost".into(),
                        value: 3258.0,
                        unit: None,
                        arm: None,
                        ci_low: None,
                        ci_high: None,
                        p: None,
                        pmid: "11111111".into(),
                    }],
                    &["11111111"],
                ),
            ],
            "Shorter [11111111].",
        );
        let c = check(&w, &rows, |_| false, None, known);
        assert_eq!(c.counts.numbers_checked, 6);
        // Only the "Stay" number: the PMID inside "[22222222]" is no number.
        assert_eq!(c.counts.numbers_missing, 1, "{:?}", c.gaps);
        assert_eq!(c.counts.outcomes_missing, 1);
        assert_eq!(c.gaps[0].kind, GapKind::Outcome);
        assert_eq!(c.gaps[0].outcome, "Stay");
        assert!(
            c.line()
                .starts_with("Coverage: 1 verified number and 1 outcome not in the draft")
        );
        // U+2212 and the hyphen-minus are one sign.
        assert!(answer_numbers(&w, known).contains(&-2.3));
    }

    /// An outcome with no number is covered by its name's words in an
    /// outcome row; a covered outcome's missing numbers are their own gaps,
    /// "pooled" for a meta-analysis.
    #[test]
    fn outcome_words_and_pooled_numbers() {
        let rows = vec![
            row(
                "11111111",
                vec![
                    vo("Stray-energy injury", vec![]),
                    vo("Same-day discharge", vec![]),
                ],
            ),
            row(
                "22222222",
                vec![vo(
                    "Recurrence",
                    vec![
                        rn("RD", 0.01),
                        rn("RR", 1.07),
                        rn("operative time MD", 10.91),
                    ],
                )],
            ),
        ];
        let w = answer(
            vec![out(
                "Recurrence and stray energy injury",
                "RR 1.07 [22222222]",
                vec![],
                &[],
            )],
            "x",
        );
        let c = check(&w, &rows, |p| p == "22222222", None, known);
        let kinds: Vec<(GapKind, &str)> = c
            .gaps
            .iter()
            .map(|g| (g.kind, g.outcome.as_str()))
            .collect();
        assert_eq!(
            kinds,
            [
                (GapKind::Outcome, "Same-day discharge"),
                (GapKind::Pooled, "Recurrence"),
                (GapKind::Pooled, "Recurrence"),
            ]
        );
        assert_eq!(c.gaps[2].numbers, [10.91]);
        assert_eq!(c.counts.pooled_missing, 2);
        assert_eq!(
            c.line(),
            "Coverage: 2 verified numbers (2 pooled) and 1 outcome not in the draft"
        );
    }

    /// Skeptic items are covered by a citation in the writer's own
    /// sections; an overlap needs each of its PMIDs.
    #[test]
    fn skeptic_items_need_a_citation_in_the_main_sections() {
        let review = SkepticReview {
            verdict: Verdict::HoldsWithCaveats,
            must_mention: vec!["one trial".into()],
            contradicting: vec![Contradicting {
                pmid: "33333333".into(),
                claim: "open repair differs".into(),
                quote: "Open repair was better.".into(),
                strength_vs_rcts: "older".into(),
            }],
            missed_rcts: vec![MissedRct {
                pmid: "44444444".into(),
                why: "a 2026 trial".into(),
            }],
            guidelines: vec![],
            overlaps: vec![Overlap {
                pmids: vec!["11111111".into(), "22222222".into()],
                note: "same centre".into(),
            }],
            ma_notes: vec![MaNote {
                pmid: "22222222".into(),
                note: "pools a cohort".into(),
            }],
            extract_next: vec![],
            searches: vec![],
            leads: vec![],
            dropped_unseen: 0,
            dropped_quotes: 0,
            numbers_flagged: 0,
            spans: vec![],
        };
        let mut w = answer(vec![], "Shorter [11111111].");
        w.conflicts.push(Conflict {
            text: "x".into(),
            pmids: vec!["33333333".into()],
        });
        let c = check(&w, &[], |_| false, Some(&review), known);
        assert_eq!(c.counts.skeptic_checked, 4);
        // Missed (44444444), the overlap (22222222 uncited) and the MA note.
        assert_eq!(c.counts.skeptic_missing, 3);
        let pmids: Vec<&str> = c.gaps.iter().map(|g| g.pmid.as_str()).collect();
        assert_eq!(pmids, ["44444444", "22222222", "22222222"]);
        assert!(c.gaps[0].text.contains("a 2026 trial"));
        assert!(
            c.line().ends_with("; 3 skeptic items not cited"),
            "{}",
            c.line()
        );
    }

    #[test]
    fn the_line_drops_zero_parts() {
        let c = CoverageV3 {
            gaps: vec![],
            counts: CoverageCounts {
                numbers_checked: 5,
                outcomes_checked: 2,
                ..CoverageCounts::default()
            },
        };
        assert_eq!(
            c.line(),
            "Coverage: all 5 verified numbers and 2 outcomes are in the draft"
        );
        let c = CoverageV3 {
            gaps: vec![],
            counts: CoverageCounts {
                outcomes_missing: 3,
                ..CoverageCounts::default()
            },
        };
        assert_eq!(c.line(), "Coverage: 3 outcomes not in the draft");
    }
}
