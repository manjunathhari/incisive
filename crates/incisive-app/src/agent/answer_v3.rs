//! The Phase 3 structured answer (AnswerV3, spec §2), shared by server and
//! browser.
//!
//! The evidence team stores an [`AnswerV3`] as `run_answers.body` with
//! `run_answers.answer_version = 3` (Phase 2 answers are version 2 and keep
//! their own view). The writer's part ([`WrittenAnswer`]) is the model's
//! structured output after the citation check; the references, the search
//! trail and the integrity counts are built by code, never by the model.
//!
//! Numbers sit in structured fields ([`Number`]: value, unit, arm, CI, p and
//! the source study's PMID) so Phase 4 can chart them without re-parsing.
//!
//! Citations in text are `[PMID]` markers (`[12345678]` or
//! `[12345678; 23456789]`). [`clean_citations`] removes markers naming a PMID
//! outside the selected studies; [`split_citations`] cuts text into plain
//! runs and citation runs for the page, which shows them as numbered
//! reference links.
//!
//! Everything here compiles without `ssr` (serde, chrono, regex only).
//! Privacy: the answer holds the question; it is stored only in
//! `run_answers` (private to its user) and never logged.

use std::collections::HashMap;
use std::sync::LazyLock;

use chrono::{DateTime, Utc};
use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::runs::RunMode;

/// `run_answers.answer_version` of an [`AnswerV3`] (Phase 2 answers are 2).
pub const ANSWER_VERSION: i16 = 3;

/// GRADE-style certainty.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Certainty {
    High,
    Moderate,
    Low,
    #[serde(rename = "Very low")]
    VeryLow,
}

impl Certainty {
    /// The four values, as the schema's enum lists them.
    pub const ALL: [&'static str; 4] = ["High", "Moderate", "Low", "Very low"];

    pub fn as_str(self) -> &'static str {
        match self {
            Certainty::High => "High",
            Certainty::Moderate => "Moderate",
            Certainty::Low => "Low",
            Certainty::VeryLow => "Very low",
        }
    }

    /// CSS class for the badge (existing badge colours).
    pub fn class(self) -> &'static str {
        match self {
            Certainty::High => "badge done",
            Certainty::Moderate => "badge mode",
            Certainty::Low => "badge partial",
            Certainty::VeryLow => "badge failed",
        }
    }
}

/// 2–3 sentences and the overall certainty with its reason.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BottomLine {
    pub text: String,
    pub certainty: Certainty,
    pub certainty_reason: String,
}

/// One number, with its source study.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Number {
    pub label: String,
    pub value: f64,
    pub unit: Option<String>,
    /// The arm or group it belongs to, if any.
    pub arm: Option<String>,
    pub ci_low: Option<f64>,
    pub ci_high: Option<f64>,
    /// As written, e.g. "0.431" or "<0.001".
    pub p: Option<String>,
    pub pmid: String,
}

/// What kind of evidence an outcome row rests on (Stage 4 item 6): a label
/// only; it blocks nothing.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceType {
    Rct,
    RctMetaAnalysis,
    NetworkMetaAnalysis,
    Observational,
    #[default]
    Mixed,
}

impl EvidenceType {
    /// The schema's enum values.
    pub const ALL: [&'static str; 5] = [
        "rct",
        "rct_meta_analysis",
        "network_meta_analysis",
        "observational",
        "mixed",
    ];

    pub fn label(self) -> &'static str {
        match self {
            EvidenceType::Rct => "RCTs",
            EvidenceType::RctMetaAnalysis => "RCT meta-analysis",
            EvidenceType::NetworkMetaAnalysis => "network meta-analysis",
            EvidenceType::Observational => "observational",
            EvidenceType::Mixed => "mixed evidence",
        }
    }
}

/// One row of the outcome table.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Outcome {
    pub outcome: String,
    pub rct_evidence: String,
    pub effect_text: String,
    pub numbers: Vec<Number>,
    pub certainty: Certainty,
    pub why: String,
    pub pmids: Vec<String>,
    /// Stage 4 (additive; old answers: mixed).
    #[serde(default)]
    pub evidence_type: EvidenceType,
}

/// A guideline or consensus statement (Stage 4 item 1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GuidelineV3 {
    pub pmid: Option<String>,
    pub body: String,
    pub year: Option<i32>,
    pub statement: String,
}

/// One key number of the at-a-glance box.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyNumber {
    pub label: String,
    pub value_text: String,
    pub pmid: String,
}

/// The at-a-glance box (Stage 4 item 8): about five lines at the top.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AtAGlance {
    pub verdict: String,
    pub key_numbers: Vec<KeyNumber>,
    pub main_caveat: String,
}

/// What the evidence means for clinical practice (Stage 4 item 9).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClinicalPractice {
    pub default_choice: String,
    pub who_may_benefit: String,
    pub counselling: String,
    /// A per-case range derived from verified trial numbers, or "not known".
    pub cost_per_case: String,
}

/// A notable finding outside the outcome tables (Stage 4 item 12).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NotableFinding {
    pub text: String,
    pub pmids: Vec<String>,
}

/// A marked run of stored text (Stage 4 items 5 and 7): `len` bytes from
/// byte `start` of the text at `field` (a path such as
/// `outcomes.2.effect_text`). The text itself is never changed; the page
/// draws a small mark after the run. A span with `len` 0 marks the whole
/// field (a structured number).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Span {
    pub field: String,
    pub start: u32,
    pub len: u32,
}

impl Span {
    pub fn new(field: impl Into<String>, start: usize, end: usize) -> Span {
        Span {
            field: field.into(),
            start: u32::try_from(start).unwrap_or(u32::MAX),
            len: u32::try_from(end.saturating_sub(start)).unwrap_or(u32::MAX),
        }
    }
}

/// `text` cut into runs, each with whether it is marked by one of `spans`
/// on `field`. A span outside the text or off a character boundary is
/// ignored (stored data is never trusted to be well formed); overlapping
/// spans are merged. A marked run is followed by the mark when drawn.
pub fn marked_runs(text: &str, field: &str, spans: &[Span]) -> Vec<(String, bool)> {
    let mut cuts: Vec<(usize, usize)> = spans
        .iter()
        .filter(|s| s.field == field && s.len > 0)
        .filter_map(|s| {
            let a = s.start as usize;
            let b = a.checked_add(s.len as usize)?;
            (b <= text.len() && text.is_char_boundary(a) && text.is_char_boundary(b))
                .then_some((a, b))
        })
        .collect();
    cuts.sort_unstable();
    let mut out: Vec<(String, bool)> = Vec::new();
    let mut at = 0;
    for (a, b) in cuts {
        if a < at {
            continue;
        }
        if a > at {
            out.push((text[at..a].to_string(), false));
        }
        out.push((text[a..b].to_string(), true));
        at = b;
    }
    if at < text.len() || out.is_empty() {
        out.push((text[at..].to_string(), false));
    }
    out
}

/// Whether `spans` marks the whole of `field` (a `len` 0 span).
pub fn whole_marked(field: &str, spans: &[Span]) -> bool {
    spans.iter().any(|s| s.field == field && s.len == 0)
}

/// One row of the trials table.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Study {
    pub pmid: String,
    /// Short name, e.g. "ROGER (2025, Br J Surg)".
    pub label: String,
    pub design: String,
    pub n: Option<i64>,
    pub setting: String,
    pub comparison: String,
    pub key_results: String,
    pub appraisal: String,
}

/// Conflicting evidence and why it differs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Conflict {
    pub text: String,
    pub pmids: Vec<String>,
}

/// Who and where the evidence applies to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Applicability {
    pub case_mix: String,
    pub india_cost_context: String,
    pub training: String,
}

/// The writer's answer (its structured output, after the citation check).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WrittenAnswer {
    pub bottom_line: BottomLine,
    pub outcomes: Vec<Outcome>,
    pub studies: Vec<Study>,
    pub conflicts: Vec<Conflict>,
    pub applicability: Applicability,
    pub would_change: Vec<String>,
    /// Stage 4 (additive, default empty or none: old answers and replies
    /// that omit them still load).
    #[serde(default)]
    pub at_a_glance: Option<AtAGlance>,
    #[serde(default)]
    pub clinical_practice: Option<ClinicalPractice>,
    #[serde(default)]
    pub guidelines: Vec<GuidelineV3>,
    #[serde(default)]
    pub notable_findings: Vec<NotableFinding>,
}

/// A numbered reference, built by code from the fetched metadata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReferenceV3 {
    /// 1-based, in the order the researcher selected the studies.
    pub n: u16,
    pub pmid: String,
    pub title: String,
    pub journal: Option<String>,
    pub year: Option<i32>,
    pub doi: Option<String>,
    pub retracted: bool,
    /// Full (Stage 3): a missed RCT the skeptic found; given to the writer
    /// from its abstract, not extracted. Additive (old answers: false).
    #[serde(default)]
    pub found_by_skeptic: bool,
}

/// Full (Stage 3): what the extractor did for one selected study.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvidenceV3 {
    pub pmid: String,
    /// "full text" or "abstract only" (what the row was read from), or
    /// "not extracted" (the writer used the abstract).
    pub text: String,
    /// Outcomes whose quote was found in the source.
    pub verified_quotes: u32,
    /// Before Stage 4: numbers dropped. Stage 4 keeps them, flagged
    /// (`numbers_unverified`); new answers store 0 here.
    pub numbers_dropped: u32,
    /// Outcomes reported whose quote was not found (names).
    pub unverified_outcomes: Vec<String>,
    /// Stage 4 (additive): numbers kept but not verified against their
    /// quote or the source.
    #[serde(default)]
    pub numbers_unverified: u32,
    /// Stage 4 (additive): each unverified outcome with its values, shown
    /// labelled "not verified".
    #[serde(default)]
    pub unverified_details: Vec<UnverifiedOutcomeV3>,
    /// Stage 4 (additive): funding and conflicts of interest as the source
    /// states them (`None`: not stated). Only meaningful when
    /// `funding_checked` (old answers never looked).
    #[serde(default)]
    pub funding: Option<String>,
    #[serde(default)]
    pub funding_checked: bool,
    /// Stage 4 (additive): numbers in `funding` the source does not have.
    #[serde(default)]
    pub spans: Vec<Span>,
}

/// An extracted outcome whose quote was not found in the paper (Stage 4
/// item 7): kept with its values, labelled "not verified".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnverifiedOutcomeV3 {
    pub outcome: String,
    pub effect_text: String,
}

/// Full (Stage 3): the skeptic's checked review, as the page shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkepticV3 {
    /// "holds", "holds with caveats" or "does not hold".
    pub verdict: String,
    pub must_mention: Vec<String>,
    /// (PMID, claim) of each contradiction with a verified quote.
    pub contradicting: Vec<SkepticItem>,
    pub missed_rcts: Vec<SkepticItem>,
    /// (PMID, body: statement) of each guideline with a verified quote.
    pub guidelines: Vec<SkepticItem>,
    pub overlaps: Vec<SkepticOverlap>,
    /// Stage 4 (additive): what a meta-analysis actually pooled.
    #[serde(default)]
    pub ma_notes: Vec<SkepticItem>,
    /// Stage 4 (additive): numbers in the skeptic's text that no text its
    /// tools returned has (paths such as `must_mention.0`,
    /// `contradicting.1.text`, `overlaps.0.note`).
    #[serde(default)]
    pub spans: Vec<Span>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkepticItem {
    pub pmid: String,
    pub text: String,
    /// The verified quote behind it (contradictions and guidelines).
    #[serde(default)]
    pub quote: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkepticOverlap {
    pub pmids: Vec<String>,
    pub note: String,
}

/// One search the researcher ran, as its tool returned it (recorded by
/// code, not reported by the model).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchEntry {
    pub query: String,
    /// "PubMed", "Europe PMC" or "PubMed related".
    pub source: String,
    pub hits: u64,
}

/// What the code checks removed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct IntegrityV3 {
    /// Studies the researcher selected that no tool returned in this run.
    pub dropped_unseen: u32,
    /// Selected studies whose record could not be fetched.
    pub dropped_unfetched: u32,
    /// `[PMID]` citations of a study outside the selection, removed.
    pub removed_citations: u32,
    /// Number entries whose source study is outside the selection.
    pub dropped_numbers: u32,
    /// Trial rows whose study is outside the selection.
    pub dropped_studies: u32,
    /// Stage 4 item 5 (additive): numbers written in the answer's text,
    /// and how many of them are not traced to a source (marked).
    #[serde(default)]
    pub numbers_checked: u32,
    #[serde(default)]
    pub numbers_untraced: u32,
    /// Stage 4 item 7 (additive): extractor numbers and outcomes kept but
    /// not verified.
    #[serde(default)]
    pub numbers_unverified: u32,
    #[serde(default)]
    pub outcomes_unverified: u32,
}

/// The stored Phase 3 answer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AnswerV3 {
    /// Always [`ANSWER_VERSION`].
    pub version: i16,
    pub question: String,
    pub mode: RunMode,
    pub created_at: DateTime<Utc>,
    /// `None` when nothing could be written (no evidence, or the write-up
    /// did not fit); `empty_reason` then says why.
    pub answer: Option<WrittenAnswer>,
    pub empty_reason: Option<String>,
    pub references: Vec<ReferenceV3>,
    pub searches: Vec<SearchEntry>,
    pub integrity: IntegrityV3,
    /// "cost limit" / "time limit" when the research stopped early.
    pub stopped_early: Option<String>,
    pub cost_paise: i64,
    pub elapsed_s: u64,
    /// Full (Stage 3, additive): the skeptic's review; `None` in Quick, or
    /// when the skeptic failed (`notes` says so).
    #[serde(default)]
    pub skeptic: Option<SkepticV3>,
    /// Full (Stage 3, additive): per selected study, what was extracted.
    #[serde(default)]
    pub evidence: Vec<EvidenceV3>,
    /// Full (Stage 3, additive): what went short (the skeptic failed, the
    /// extractor stopped early), in plain words.
    #[serde(default)]
    pub notes: Vec<String>,
    /// Stage 4 item 5 (additive): numbers in the writer's text not traced
    /// to a source, as spans of `answer`'s fields. The text is unchanged.
    #[serde(default)]
    pub untraced: Vec<Span>,
    /// Stage 4 item 12 (additive): notable findings built by code from the
    /// skeptic's and extractor's output (no AI call).
    #[serde(default)]
    pub notable_code: Vec<NotableFinding>,
}

impl AnswerV3 {
    /// Reference number per PMID.
    pub fn ref_numbers(&self) -> HashMap<String, u16> {
        self.references
            .iter()
            .map(|r| (r.pmid.clone(), r.n))
            .collect()
    }
}

/// A citation marker: `[` PMIDs separated by `;` or `,` `]`, each optionally
/// prefixed `PMID`/`PMID:`. Digits only (1–9 each).
static CITE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\[\s*(?:PMIDs?:?\s*)?\d{1,9}(?:\s*[;,]\s*(?:PMIDs?:?\s*)?\d{1,9})*\s*\]")
        .expect("citation pattern is valid")
});
static DIGITS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\d{1,9}").expect("digits pattern is valid"));

/// A run of answer text: plain, or a citation of these PMIDs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Segment {
    Text(String),
    Cite(Vec<String>),
}

/// Shortest id that counts as a PMID on its own in a bracket.
pub const MIN_PMID_DIGITS: usize = 5;

/// `text` cut into plain runs and citation runs, in order. A bracket of
/// numbers is a citation only if one of its ids is `known` (a selected
/// study) or any id has at least [`MIN_PMID_DIGITS`] digits; otherwise
/// (e.g. "LOS 2 [1, 3] days") it stays text.
pub fn split_citations(text: &str, known: impl Fn(&str) -> bool) -> Vec<Segment> {
    let mut out: Vec<Segment> = Vec::new();
    let mut plain = String::new();
    let mut last = 0;
    for m in CITE.find_iter(text) {
        let ids: Vec<String> = DIGITS
            .find_iter(m.as_str())
            .map(|d| d.as_str().to_string())
            .collect();
        let citation =
            ids.iter().any(|i| known(i)) || ids.iter().any(|i| i.len() >= MIN_PMID_DIGITS);
        plain.push_str(&text[last..m.start()]);
        if citation {
            if !plain.is_empty() {
                out.push(Segment::Text(std::mem::take(&mut plain)));
            }
            out.push(Segment::Cite(ids));
        } else {
            plain.push_str(m.as_str());
        }
        last = m.end();
    }
    plain.push_str(&text[last..]);
    if !plain.is_empty() {
        out.push(Segment::Text(plain));
    }
    out
}

/// The byte ranges of the citation markers in `text` (as
/// [`split_citations`] decides them): numbers inside them are PMIDs, not
/// findings.
pub fn citation_ranges(text: &str, known: impl Fn(&str) -> bool) -> Vec<(usize, usize)> {
    CITE.find_iter(text)
        .filter(|m| {
            let ids: Vec<&str> = DIGITS.find_iter(m.as_str()).map(|d| d.as_str()).collect();
            ids.iter().any(|i| known(i)) || ids.iter().any(|i| i.len() >= MIN_PMID_DIGITS)
        })
        .map(|m| (m.start(), m.end()))
        .collect()
}

/// Every text field of the writer's answer with its span path (Stage 4
/// item 5), in page order. Structured numbers are not text; see
/// `writer_v3::trace_numbers`.
pub fn text_fields(a: &WrittenAnswer) -> Vec<(String, &str)> {
    let mut v: Vec<(String, &str)> = Vec::new();
    if let Some(g) = &a.at_a_glance {
        v.push(("at_a_glance.verdict".into(), &g.verdict));
        for (i, k) in g.key_numbers.iter().enumerate() {
            v.push((format!("at_a_glance.key_numbers.{i}.label"), &k.label));
            v.push((
                format!("at_a_glance.key_numbers.{i}.value_text"),
                &k.value_text,
            ));
        }
        v.push(("at_a_glance.main_caveat".into(), &g.main_caveat));
    }
    v.push(("bottom_line.text".into(), &a.bottom_line.text));
    v.push((
        "bottom_line.certainty_reason".into(),
        &a.bottom_line.certainty_reason,
    ));
    if let Some(c) = &a.clinical_practice {
        v.push(("clinical_practice.default_choice".into(), &c.default_choice));
        v.push((
            "clinical_practice.who_may_benefit".into(),
            &c.who_may_benefit,
        ));
        v.push(("clinical_practice.counselling".into(), &c.counselling));
        v.push(("clinical_practice.cost_per_case".into(), &c.cost_per_case));
    }
    for (i, o) in a.outcomes.iter().enumerate() {
        v.push((format!("outcomes.{i}.outcome"), &o.outcome));
        v.push((format!("outcomes.{i}.rct_evidence"), &o.rct_evidence));
        v.push((format!("outcomes.{i}.effect_text"), &o.effect_text));
        v.push((format!("outcomes.{i}.why"), &o.why));
    }
    for (i, s) in a.studies.iter().enumerate() {
        v.push((format!("studies.{i}.label"), &s.label));
        v.push((format!("studies.{i}.design"), &s.design));
        v.push((format!("studies.{i}.setting"), &s.setting));
        v.push((format!("studies.{i}.comparison"), &s.comparison));
        v.push((format!("studies.{i}.key_results"), &s.key_results));
        v.push((format!("studies.{i}.appraisal"), &s.appraisal));
    }
    for (i, c) in a.conflicts.iter().enumerate() {
        v.push((format!("conflicts.{i}.text"), &c.text));
    }
    for (i, g) in a.guidelines.iter().enumerate() {
        v.push((format!("guidelines.{i}.body"), &g.body));
        v.push((format!("guidelines.{i}.statement"), &g.statement));
    }
    v.push(("applicability.case_mix".into(), &a.applicability.case_mix));
    v.push((
        "applicability.india_cost_context".into(),
        &a.applicability.india_cost_context,
    ));
    v.push(("applicability.training".into(), &a.applicability.training));
    for (i, w) in a.would_change.iter().enumerate() {
        v.push((format!("would_change.{i}"), w));
    }
    for (i, n) in a.notable_findings.iter().enumerate() {
        v.push((format!("notable_findings.{i}.text"), &n.text));
    }
    v
}

/// `text` with every citation marker ([`split_citations`]) rewritten to keep only PMIDs for which
/// `known` is true (de-duplicated, in order, as `[a; b]`); a marker left
/// empty is removed with the blank before it. Returns the text and how many
/// PMID citations were removed.
pub fn clean_citations(text: &str, known: impl Fn(&str) -> bool) -> (String, u32) {
    let mut removed = 0u32;
    let mut out = String::with_capacity(text.len());
    for seg in split_citations(text, &known) {
        match seg {
            Segment::Text(t) => out.push_str(&t),
            Segment::Cite(ids) if !ids.iter().any(|i| known(i)) => {
                // No selected study in it: unknown PMIDs (5+ digits) are
                // removed and counted; short numbers stay as text.
                let mut short: Vec<String> = Vec::new();
                for id in ids {
                    if id.len() >= MIN_PMID_DIGITS {
                        removed += 1;
                    } else {
                        short.push(id);
                    }
                }
                if short.is_empty() {
                    let trimmed = out.trim_end_matches([' ', '\t']).len();
                    out.truncate(trimmed);
                } else {
                    out.push('[');
                    out.push_str(&short.join("; "));
                    out.push(']');
                }
            }
            Segment::Cite(ids) => {
                let mut keep: Vec<String> = Vec::new();
                for id in ids {
                    if !known(&id) {
                        removed += 1;
                    } else if !keep.contains(&id) {
                        keep.push(id);
                    }
                }
                if keep.is_empty() {
                    let trimmed = out.trim_end_matches([' ', '\t']).len();
                    out.truncate(trimmed);
                } else {
                    out.push('[');
                    out.push_str(&keep.join("; "));
                    out.push(']');
                }
            }
        }
    }
    (out, removed)
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: &str = "41060645";
    const B: &str = "39979619";

    fn known(p: &str) -> bool {
        p == A || p == B
    }

    #[test]
    fn splits_markers_of_every_shape() {
        let s = split_citations(
            &format!("Pain was similar [{A}; {B}]. Cost [PMID: {A}], [{A},{B}] end"),
            known,
        );
        assert_eq!(
            s,
            vec![
                Segment::Text("Pain was similar ".into()),
                Segment::Cite(vec![A.into(), B.into()]),
                Segment::Text(". Cost ".into()),
                Segment::Cite(vec![A.into()]),
                Segment::Text(", ".into()),
                Segment::Cite(vec![A.into(), B.into()]),
                Segment::Text(" end".into()),
            ]
        );
        // Brackets that are not citations stay text.
        let s = split_citations("95% CI [0.2 to 1.39] and [n=12] and [a]", known);
        assert!(s.iter().all(|x| matches!(x, Segment::Text(_))), "{s:?}");
    }

    /// Review finding 5: short numbers in brackets are not citations unless
    /// one of them is a selected study.
    #[test]
    fn short_bracketed_numbers_stay_text() {
        let t = "LOS 2 [1, 3] days";
        let (c, n) = clean_citations(t, known);
        assert_eq!((c.as_str(), n), (t, 0));
        assert_eq!(split_citations(t, known), vec![Segment::Text(t.into())]);
        // An unknown long id is a citation (and removed); a short one beside
        // a selected study is too.
        let (c, n) = clean_citations(&format!("x [99999999] y [{A}; 12]"), known);
        assert_eq!(c, format!("x y [{A}]"));
        assert_eq!(n, 2);
    }

    /// Minor review 1: an unknown long PMID beside short numbers is still a
    /// citation candidate: the PMID is removed and counted, the short
    /// numbers stay as text.
    #[test]
    fn unknown_long_ids_beside_short_numbers_are_removed() {
        assert_eq!(
            split_citations("x [12; 99999999] y", known),
            vec![
                Segment::Text("x ".into()),
                Segment::Cite(vec!["12".into(), "99999999".into()]),
                Segment::Text(" y".into()),
            ]
        );
        let (c, n) = clean_citations("x [12; 99999999] y", known);
        assert_eq!((c.as_str(), n), ("x [12] y", 1));
        let (c, n) = clean_citations("x [1, 3; 88888888; 99999999] y", known);
        assert_eq!((c.as_str(), n), ("x [1; 3] y", 2));
    }

    #[test]
    fn clean_keeps_known_and_removes_unknown() {
        let (t, n) = clean_citations(
            &format!("A [{A}; 99999999]. B [88888888]. C [{B}; {B}; {A}]."),
            known,
        );
        assert_eq!(t, format!("A [{A}]. B. C [{B}; {A}]."));
        assert_eq!(n, 2);
        let (t, n) = clean_citations("No markers here.", known);
        assert_eq!((t.as_str(), n), ("No markers here.", 0));
    }

    /// Stage 4: marked runs never change the text; spans that do not fit
    /// the text (stored data) are ignored.
    #[test]
    fn marked_runs_split_text_and_ignore_bad_spans() {
        let t = "Pain 2.1 vs 2.4 é 7%";
        let spans = vec![
            Span::new("f", 5, 8),
            Span::new("f", 6, 9),
            Span::new("g", 0, 4),
            Span::new("f", 17, 18),
            Span::new("f", 19, 99),
            Span::new("f", 20, 22),
        ];
        let runs = marked_runs(t, "f", &spans);
        let joined: String = runs.iter().map(|(x, _)| x.as_str()).collect();
        assert_eq!(joined, t);
        let marked: Vec<&str> = runs
            .iter()
            .filter(|(_, m)| *m)
            .map(|(x, _)| x.as_str())
            .collect();
        assert_eq!(
            marked,
            ["2.1"],
            "overlap, off-boundary and out-of-range spans ignored"
        );
        assert_eq!(marked_runs("", "f", &spans), vec![(String::new(), false)]);
        assert!(whole_marked("n", &[Span::new("n", 0, 0)]));
        assert!(!whole_marked("n", &[Span::new("n", 0, 1)]));
    }

    #[test]
    fn citation_ranges_follow_the_citation_rule() {
        let t = format!("LOS 2 [1, 3] days [{A}; 12] and [99999999].");
        let r: Vec<&str> = citation_ranges(&t, known)
            .into_iter()
            .map(|(a, b)| &t[a..b])
            .collect();
        assert_eq!(r, [format!("[{A}; 12]").as_str(), "[99999999]"]);
    }

    #[test]
    fn certainty_reads_the_schema_words() {
        for w in Certainty::ALL {
            let c: Certainty = serde_json::from_str(&format!("\"{w}\"")).unwrap();
            assert_eq!(c.as_str(), w);
        }
        assert!(serde_json::from_str::<Certainty>("\"very low\"").is_err());
    }
}
