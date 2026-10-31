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
/// only; it blocks nothing. Since R-S4a it is derived in code from the
/// cited studies' designs (`writer_v3::evidence_types`); `Unknown` (the
/// default, and answers stored before Stage 4) draws no label.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceType {
    Rct,
    RctMetaAnalysis,
    NetworkMetaAnalysis,
    /// Meta-analyses (what they pooled is in the skeptic's notes).
    MetaAnalysis,
    /// RCTs and meta-analyses together (Stage 4 final review).
    RctsAndMetaAnalyses,
    /// Studies the researcher labelled `other`: not randomised.
    Other,
    Observational,
    Mixed,
    #[default]
    Unknown,
}

impl EvidenceType {
    /// The label drawn under the outcome; `None` draws nothing.
    pub fn label(self) -> Option<&'static str> {
        match self {
            EvidenceType::Rct => Some("RCTs"),
            EvidenceType::RctMetaAnalysis => Some("RCT meta-analysis"),
            EvidenceType::NetworkMetaAnalysis => Some("network meta-analysis"),
            EvidenceType::MetaAnalysis => Some("meta-analysis"),
            EvidenceType::RctsAndMetaAnalyses => Some("RCTs and meta-analyses"),
            EvidenceType::Other => Some("other / non-randomised"),
            EvidenceType::Observational => Some("observational"),
            EvidenceType::Mixed => Some("mixed evidence"),
            EvidenceType::Unknown => None,
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
    /// Stage 4 review 2: the writer cited a study outside the selection and
    /// the citation check removed it. Not in the writer's schema. With no
    /// PMID either way, the page says "no source among the studies; not
    /// verified".
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub pmid_removed: bool,
}

/// The label of a guideline with no source among the studies.
pub const NO_GUIDELINE_SOURCE: &str = "no source among the studies; not verified";

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
    /// Stage 4 schema fix (additive): the writer's text that sat under no
    /// label (the flat schema gives one text with labelled lines), drawn as
    /// written.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub text: String,
}

/// A notable finding outside the outcome tables (Stage 4 item 12).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NotableFinding {
    pub text: String,
    pub pmids: Vec<String>,
    /// Code-built items only: numbers in `text` (field `text`) that the
    /// skeptic's sources do not have, carried over from its review. Not in
    /// the writer's schema (its items are traced in `AnswerV3.untraced`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub spans: Vec<Span>,
}

/// The prefixes of the code-built notable findings that repeat the
/// skeptic's box (Stage 4 item 12). Stage 4b item 5: they are drawn in the
/// skeptic's box only; the notable findings keep the writer's items and the
/// extractor's counts line. Stored answers keep them (display only).
pub const NOTABLE_OVERLAP: &str = "Not independent evidence (the skeptic): ";
pub const NOTABLE_MISSED: &str =
    "A randomised trial the researcher's search missed (the skeptic): ";
pub const NOTABLE_POOLED: &str = "What a meta-analysis pooled (the skeptic): ";

impl NotableFinding {
    /// A code-built item that repeats the skeptic's box (not drawn among
    /// the notable findings, Stage 4b item 5).
    pub fn repeats_skeptic(&self) -> bool {
        [NOTABLE_OVERLAP, NOTABLE_MISSED, NOTABLE_POOLED]
            .iter()
            .any(|p| self.text.starts_with(p))
    }
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
    /// Stage 4 review 6: a trial registration the extractor gave that the
    /// source does not contain, shown labelled "not in the source".
    #[serde(default)]
    pub trial_registration_unverified: Option<String>,
    /// Stage 4b item 3: a study the skeptic found and picked for
    /// extraction (the second wave); labelled "found by the skeptic".
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub found_by_skeptic: bool,
}

impl EvidenceV3 {
    /// Review S2: what the second wave did for a pick, for its label:
    /// "extracted in a second wave" only when it has an extraction row.
    pub fn second_wave(&self) -> &'static str {
        if self.text == "not extracted" {
            "not extracted in the second wave; abstract only"
        } else {
            "extracted in a second wave"
        }
    }
}

/// The label of a study the skeptic picked for extraction.
pub const FOUND_BY_SKEPTIC: &str = "found by the skeptic";

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
    /// Stage 4 review 6: studies offered as contradicting evidence or a
    /// guideline whose quote was not found: PMID and kind only.
    #[serde(default)]
    pub leads: Vec<SkepticLead>,
    /// Stage 4 review 5: entries not shown (a study its searches did not
    /// return; before the review, also an unfound quote).
    #[serde(default)]
    pub dropped_unseen: u32,
    #[serde(default)]
    pub dropped_quotes: u32,
    /// Stage 4b item 3: studies it picked for extraction (second wave).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub extract_next: Vec<SkepticItem>,
}

/// An unverified lead of the skeptic (Stage 4 review 6).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkepticLead {
    pub pmid: String,
    /// "contradicting study" or "guideline".
    pub kind: String,
}

impl SkepticLead {
    /// What the page and the copy say: no claim, no quote, no numbers.
    pub fn text(&self) -> String {
        format!(
            "Possible {}: quote not found in the abstract, check the paper",
            self.kind
        )
    }
}

impl SkepticV3 {
    /// "The skeptic gave N entries whose quote or study could not be found;
    /// not shown." (Stage 4 review 5), when there were any.
    pub fn dropped_note(&self) -> Option<String> {
        let n = self.dropped_unseen + self.dropped_quotes;
        (n > 0).then(|| {
            format!(
                "The skeptic gave {n} {} whose quote or study could not be found; not shown.",
                if n == 1 { "entry" } else { "entries" }
            )
        })
    }
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
    /// Stage 4 schema fix (additive): certainty or evidence-type labels the
    /// writer gave that were not recognised, set to Very low / mixed.
    #[serde(default)]
    pub labels_defaulted: u32,
}

// ---------- Stage 4b item 1: the coverage check ----------

/// What a coverage gap is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GapKind {
    /// A verified structured number of an evidence row the answer does not
    /// write.
    Number,
    /// A verified extracted outcome none of whose numbers (or, with none,
    /// whose name) the answer has.
    Outcome,
    /// A verified number of a study the researcher labelled a meta-analysis.
    Pooled,
    /// A skeptic item (contradicting evidence, missed trial, meta-analysis
    /// note, guideline, overlap) whose study the answer's main sections
    /// never cite.
    SkepticItem,
}

impl GapKind {
    pub fn as_str(self) -> &'static str {
        match self {
            GapKind::Number => "number",
            GapKind::Outcome => "outcome",
            GapKind::Pooled => "pooled",
            GapKind::SkepticItem => "skeptic_item",
        }
    }
}

/// One thing the team verified that the written answer does not say.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CoverageGap {
    pub kind: GapKind,
    pub pmid: String,
    /// The extracted outcome's name (empty for a skeptic item).
    pub outcome: String,
    /// What is missing, in words (the row's number line or effect, or the
    /// skeptic's text).
    pub text: String,
    /// The missing values (empty for a skeptic item).
    #[serde(default)]
    pub numbers: Vec<f64>,
}

/// The coverage check's counts (all gaps, also those past the stored cap).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CoverageCounts {
    /// Verified structured numbers checked, and those the answer lacks
    /// (pooled ones included; numbers of a missing outcome included).
    pub numbers_checked: u32,
    pub numbers_missing: u32,
    /// Of `numbers_missing`, from studies labelled meta-analyses.
    pub pooled_missing: u32,
    pub outcomes_checked: u32,
    pub outcomes_missing: u32,
    pub skeptic_checked: u32,
    pub skeptic_missing: u32,
    /// Gaps not stored (past [`MAX_STORED_GAPS`]); counted above.
    #[serde(default)]
    pub gaps_cut: u32,
}

/// Gaps stored per answer, at most (the counts cover them all).
pub const MAX_STORED_GAPS: usize = 80;

/// Stage 4b item 1: what the team verified that the written answer does
/// not say. Code only (no AI call); additive on [`AnswerV3`].
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CoverageV3 {
    pub gaps: Vec<CoverageGap>,
    pub counts: CoverageCounts,
}

/// "2 outcomes and 5 numbers", "5 numbers" or "1 outcome" (Stage 4b item
/// 5: number agreement, zero parts left out); `None` when both are zero.
pub fn unverified_counts(outcomes: u32, numbers: u32) -> Option<String> {
    let mut parts: Vec<String> = Vec::new();
    if outcomes > 0 {
        parts.push(count_noun(outcomes, "outcome", "outcomes"));
    }
    if numbers > 0 {
        parts.push(count_noun(numbers, "number", "numbers"));
    }
    (!parts.is_empty()).then(|| join_counts(&parts))
}

/// "1 number" / "2 numbers".
pub fn count_noun(n: u32, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// The parts of a count list that are not zero, joined "a, b and c".
pub fn join_counts(parts: &[String]) -> String {
    match parts {
        [] => String::new(),
        [a] => a.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

impl CoverageV3 {
    /// The step line and trail note: "Coverage: 14 verified numbers (2
    /// pooled) and 3 outcomes not in the draft; 1 skeptic item not cited",
    /// zero parts left out.
    pub fn line(&self) -> String {
        let c = self.counts;
        let mut parts: Vec<String> = Vec::new();
        if c.numbers_missing > 0 {
            let mut p = count_noun(c.numbers_missing, "verified number", "verified numbers");
            if c.pooled_missing > 0 {
                p.push_str(&format!(" ({} pooled)", c.pooled_missing));
            }
            parts.push(p);
        }
        if c.outcomes_missing > 0 {
            parts.push(count_noun(c.outcomes_missing, "outcome", "outcomes"));
        }
        let mut line = if parts.is_empty() {
            format!(
                "Coverage: all {} and {} are in the draft",
                count_noun(c.numbers_checked, "verified number", "verified numbers"),
                count_noun(c.outcomes_checked, "outcome", "outcomes")
            )
        } else {
            format!("Coverage: {} not in the draft", join_counts(&parts))
        };
        if c.skeptic_missing > 0 {
            line.push_str(&format!(
                "; {} not cited",
                count_noun(c.skeptic_missing, "skeptic item", "skeptic items")
            ));
        }
        line
    }
}

// ---------- Stage 4b item 2: the completeness review ----------

/// The label every addition of the completeness review carries.
pub const ADDED_LABEL: &str = "added on completeness review";

/// A new outcome row added by the completeness review.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AddedOutcome {
    pub outcome: String,
    pub effect_text: String,
    pub pmids: Vec<String>,
    /// `None`: not graded (a sentence for a row the answer does not have,
    /// shown as a new row).
    pub certainty: Option<Certainty>,
    pub why: String,
    /// The citation check left it with no study among the given ones:
    /// shown flagged, never removed.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub unsourced: bool,
}

/// A sentence appended to one of the writer's outcome rows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutcomeAddition {
    /// The row's name as the review gave it.
    pub outcome: String,
    /// Index of the writer's outcome row it was matched to.
    pub row: usize,
    pub text: String,
    pub pmids: Vec<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub unsourced: bool,
}

/// A conflict the review found the answer does not reflect.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AddedConflict {
    pub text: String,
    pub pmids: Vec<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub unsourced: bool,
}

/// A gap the review judged not relevant to the question, with why.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NotAdded {
    pub gap: String,
    pub reason: String,
}

/// Stage 4b item 2: what the add-only completeness review (one AI call
/// after the writer, Full only) added. The writer's answer is stored
/// unchanged; these are drawn beside it, each labelled [`ADDED_LABEL`].
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CompletenessV3 {
    /// Why the review did not run or could not be used ("time", "cost
    /// limit", "error", "unusable reply"); `None` when it ran. The answer
    /// then ships exactly as the writer wrote it.
    #[serde(default)]
    pub skipped: Option<String>,
    /// Gaps the coverage check gave it.
    #[serde(default)]
    pub gaps_found: u32,
    #[serde(default)]
    pub added_outcomes: Vec<AddedOutcome>,
    #[serde(default)]
    pub outcome_additions: Vec<OutcomeAddition>,
    #[serde(default)]
    pub added_conflicts: Vec<AddedConflict>,
    #[serde(default)]
    pub not_added: Vec<NotAdded>,
    /// Inconsistency notes (it never changes a rating).
    #[serde(default)]
    pub notes: Vec<String>,
    /// Numbers in its additions not traced to what the writer was given
    /// (paths `added_outcomes.0.effect_text`, `outcome_additions.1.text`,
    /// `added_conflicts.0.text`, `notes.0`, …): marked "not traced".
    #[serde(default)]
    pub untraced: Vec<Span>,
    /// Citations of studies outside the given ones, removed (text kept).
    #[serde(default)]
    pub removed_citations: u32,
    /// Items past the stored caps (counted, not shown).
    #[serde(default)]
    pub items_cut: u32,
    /// Certainty labels it gave that were not recognised (shown "not
    /// graded").
    #[serde(default)]
    pub labels_defaulted: u32,
}

impl CompletenessV3 {
    /// A review that did not run, for `reason`.
    pub fn skipped(reason: &str, gaps_found: u32) -> CompletenessV3 {
        CompletenessV3 {
            skipped: Some(reason.to_string()),
            gaps_found,
            ..CompletenessV3::default()
        }
    }

    /// Additions made (rows, sentences and conflicts).
    pub fn added(&self) -> usize {
        self.added_outcomes.len() + self.outcome_additions.len() + self.added_conflicts.len()
    }

    /// The checks line: "Completeness: 12 gaps found; 7 added, 5 judged not
    /// relevant (listed)", or "Completeness review skipped: time".
    pub fn line(&self) -> String {
        if let Some(r) = &self.skipped {
            return format!(
                "Completeness review skipped: {r}; the answer is as the writer wrote it"
            );
        }
        let mut parts = vec![format!(
            "{} added",
            u32::try_from(self.added()).unwrap_or(u32::MAX)
        )];
        if !self.not_added.is_empty() {
            parts.push(format!(
                "{} judged not relevant (listed)",
                self.not_added.len()
            ));
        }
        if !self.notes.is_empty() {
            parts.push(count_noun(
                u32::try_from(self.notes.len()).unwrap_or(u32::MAX),
                "note",
                "notes",
            ));
        }
        format!(
            "Completeness: {} found; {}",
            count_noun(self.gaps_found, "gap", "gaps"),
            join_counts(&parts)
        )
    }

    /// The additions of the writer's outcome row `row`, with their index.
    pub fn additions_to(&self, row: usize) -> impl Iterator<Item = (usize, &OutcomeAddition)> {
        self.outcome_additions
            .iter()
            .enumerate()
            .filter(move |(_, a)| a.row == row)
    }
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
    /// Stage 4b item 1 (additive): what the team verified that the written
    /// answer does not say. `None`: Quick, or answers stored before.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub coverage: Option<CoverageV3>,
    /// Stage 4b item 2 (additive): the add-only completeness review.
    /// `None`: Quick, `COMPLETENESS=off`, or answers stored before.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completeness: Option<CompletenessV3>,
}

impl AnswerV3 {
    /// Stage 4b item 5: whether the skeptic's lead on `pmid` is shown on its
    /// study's trial row: the study was selected or extracted (it has
    /// evidence) and the writer gave it a row. Otherwise the lead stays in
    /// the leads list.
    pub fn lead_on_row(&self, pmid: &str) -> bool {
        self.evidence.iter().any(|e| e.pmid == pmid)
            && self
                .answer
                .as_ref()
                .is_some_and(|w| w.studies.iter().any(|s| s.pmid.trim() == pmid))
    }

    /// The skeptic's leads drawn on the trial row of `pmid`.
    pub fn leads_on_row(&self, pmid: &str) -> Vec<&SkepticLead> {
        if !self.lead_on_row(pmid) {
            return Vec::new();
        }
        self.skeptic
            .iter()
            .flat_map(|k| &k.leads)
            .filter(|l| l.pmid == pmid)
            .collect()
    }

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

/// Stage 4b item 5 (doubled citations): of `pmids`, those the citation
/// marker `text` ends with does not already cite (only spaces and `.` `,`
/// `;` `)` may follow that marker). Drawing a text and then its PMIDs as
/// links would otherwise show "… [4]. [4]".
pub fn not_cited_at_end(text: &str, pmids: &[String]) -> Vec<String> {
    let t =
        text.trim_end_matches(|c: char| c.is_whitespace() || matches!(c, '.' | ',' | ';' | ')'));
    let known = |p: &str| pmids.iter().any(|x| x.trim() == p);
    let tail: Vec<String> = citation_ranges(t, known)
        .into_iter()
        .filter(|(_, b)| *b == t.len())
        .flat_map(|(a, b)| {
            DIGITS
                .find_iter(&t[a..b])
                .map(|d| d.as_str().to_string())
                .collect::<Vec<_>>()
        })
        .collect();
    pmids
        .iter()
        .filter(|p| !tail.iter().any(|t| t == p.trim()))
        .cloned()
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
        v.push(("clinical_practice.text".into(), &c.text));
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

// ---------- Stage 4 item 10: arms in a fixed order ----------

/// One number as the page writes it: value, unit, arm, CI and p.
pub fn number_text(n: &Number) -> String {
    number_text_p(n, false)
}

/// [`number_text`], with `clean_p` its p without a leading `P` or `=`
/// (Part A item 12: "p P < .001"; display only).
fn number_text_p(n: &Number, clean_p: bool) -> String {
    let v = |x: f64| {
        if x.fract() == 0.0 && x.abs() < 1e15 {
            format!("{x:.0}")
        } else {
            format!("{x}")
        }
    };
    let mut t = v(n.value);
    if let Some(u) = n.unit.as_deref().filter(|u| !u.trim().is_empty()) {
        if u.trim() == "%" {
            t.push('%');
        } else {
            t.push(' ');
            t.push_str(u.trim());
        }
    }
    if let Some(a) = n.arm.as_deref().filter(|a| !a.trim().is_empty()) {
        t.push_str(&format!(" ({})", a.trim()));
    }
    if let (Some(l), Some(h)) = (n.ci_low, n.ci_high) {
        t.push_str(&format!(", CI {} to {}", v(l), v(h)));
    }
    if clean_p {
        if let Some(p) = crate::answer_page::p_label(n.p.as_deref()) {
            t.push_str(&format!(", {p}"));
        }
    } else if let Some(p) = n.p.as_deref().filter(|p| !p.trim().is_empty()) {
        t.push_str(&format!(", p {}", p.trim()));
    }
    t
}

fn words(s: &str) -> Vec<String> {
    s.split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.len() > 1)
        .map(str::to_lowercase)
        .collect()
}

/// The intervention side ("X" of "X vs Y") of a comparison, if it has one.
fn sides(comparison: &str) -> Option<(String, String)> {
    let low = comparison.to_lowercase();
    for sep in [" vs. ", " vs ", " versus ", " compared with "] {
        if let Some(i) = low.find(sep) {
            return Some((low[..i].to_string(), low[i + sep.len()..].to_string()));
        }
    }
    None
}

/// How strongly `arm` names the intervention side of `comparison`: words
/// shared with the left side minus words shared with the right.
fn arm_score(arm: &str, comparison: &str) -> i32 {
    let Some((left, right)) = sides(comparison) else {
        return 0;
    };
    let (l, r) = (words(&left), words(&right));
    words(arm)
        .iter()
        .map(|w| i32::from(l.contains(w)) - i32::from(r.contains(w)))
        .sum()
}

/// One line of an outcome's structured numbers: those with the same label
/// and source study, the intervention arm first.
#[derive(Debug, Clone, PartialEq)]
pub struct ArmLine {
    pub label: String,
    pub pmid: String,
    /// Indexes into the outcome's `numbers`, in display order.
    pub numbers: Vec<usize>,
}

/// The outcome's structured numbers grouped by label and study, each group
/// with the intervention arm first (Stage 4 item 10): the arm that shares
/// more words with the left side of the study's `comparison` ("Robotic
/// TAPP vs laparoscopic TEP") comes first, whatever order the writer used.
/// Groups keep their first-appearance order; numbers without an arm keep
/// theirs. `comparison_of` gives a study's comparison by PMID.
pub fn arm_lines<'a>(o: &Outcome, comparison_of: impl Fn(&str) -> Option<&'a str>) -> Vec<ArmLine> {
    let mut lines: Vec<ArmLine> = Vec::new();
    for (i, n) in o.numbers.iter().enumerate() {
        let label = n.label.trim().to_string();
        let pmid = n.pmid.trim().to_string();
        match lines
            .iter_mut()
            .find(|l| l.label == label && l.pmid == pmid)
        {
            Some(l) => l.numbers.push(i),
            None => lines.push(ArmLine {
                label,
                pmid,
                numbers: vec![i],
            }),
        }
    }
    for l in &mut lines {
        let Some(c) = comparison_of(&l.pmid) else {
            continue;
        };
        // Stable: equal scores keep the writer's order.
        l.numbers
            .sort_by_key(|&i| -o.numbers[i].arm.as_deref().map_or(0, |a| arm_score(a, c)));
    }
    lines
}

/// The arm line as text: "label: 80.3 min (robotic TAPP) vs 64.2 min
/// (laparoscopic TEP)".
pub fn arm_line_text(o: &Outcome, l: &ArmLine) -> String {
    arm_line_text_p(o, l, false)
}

/// [`arm_line_text`], with `clean_p` as [`number_text_p`].
fn arm_line_text_p(o: &Outcome, l: &ArmLine, clean_p: bool) -> String {
    let parts: Vec<String> = l
        .numbers
        .iter()
        .map(|&i| number_text_p(&o.numbers[i], clean_p))
        .collect();
    if l.label.is_empty() {
        parts.join(" vs ")
    } else {
        format!("{}: {}", l.label, parts.join(" vs "))
    }
}

// ---------- Stage 4 item 10: Copy as text ----------

/// `text` for the plain-text copy: citations as reference numbers ("[1,
/// 3]") and a "(not traced)" or "(not verified)" note after each marked
/// number. Unknown citations are dropped as on the page.
fn plain(
    text: &str,
    refs: &HashMap<String, u16>,
    field: &str,
    spans: &[Span],
    note: &str,
) -> String {
    let mut out = String::new();
    for (run, marked) in marked_runs(text, field, spans) {
        for seg in split_citations(&run, |p| refs.contains_key(p)) {
            match seg {
                Segment::Text(t) => out.push_str(&t),
                Segment::Cite(ids) => {
                    let mut nums: Vec<u16> =
                        ids.iter().filter_map(|p| refs.get(p).copied()).collect();
                    nums.sort_unstable();
                    nums.dedup();
                    if !nums.is_empty() {
                        let n: Vec<String> = nums.iter().map(u16::to_string).collect();
                        out.push_str(&format!("[{}]", n.join(", ")));
                    }
                }
            }
        }
        if marked {
            out.push_str(&format!(" ({note})"));
        }
    }
    out
}

impl AnswerV3 {
    /// The search trail's notes (what the checks found), shared by the page
    /// and Copy as text (Stage 4 review 7).
    pub fn trail_notes(&self) -> Vec<String> {
        let i = self.integrity;
        let mut notes: Vec<String> = Vec::new();
        if let Some(r) = &self.stopped_early {
            notes.push(format!(
                "Research stopped early ({r}); the answer uses what was found by then."
            ));
        }
        if i.dropped_unseen > 0 {
            notes.push(format!(
                "{} selected by the researcher but not returned by this run's searches: left out.",
                count_noun(i.dropped_unseen, "study", "studies")
            ));
        }
        if i.dropped_unfetched > 0 {
            notes.push(format!(
                "{} selected but could not be fetched: left out.",
                count_noun(i.dropped_unfetched, "study", "studies")
            ));
        }
        // Stage 4b item 5: number agreement, and no "0 …" parts.
        let mut parts: Vec<String> = Vec::new();
        if i.removed_citations > 0 {
            parts.push(format!(
                "{} removed",
                count_noun(i.removed_citations, "unknown citation", "unknown citations")
            ));
        }
        if i.dropped_numbers > 0 {
            parts.push(format!(
                "{} dropped",
                count_noun(i.dropped_numbers, "number", "numbers")
            ));
        }
        if i.dropped_studies > 0 {
            parts.push(format!(
                "{} dropped",
                count_noun(i.dropped_studies, "trial row", "trial rows")
            ));
        }
        if !parts.is_empty() {
            notes.push(format!("Citation check: {}.", join_counts(&parts)));
        }
        if i.numbers_checked > 0 {
            notes.push(if i.numbers_untraced == 0 {
            format!(
                "Number check: all {} numbers in the answer traced to a verified quote, structured number or abstract.",
                i.numbers_checked
            )
        } else {
            format!(
                "Number check: {} of {} numbers in the answer not traced to a verified quote, structured number or abstract; each is marked \u{201c}not traced\u{201d}. Nothing was removed.",
                i.numbers_untraced, i.numbers_checked
            )
        });
        }
        if let Some(what) = unverified_counts(i.outcomes_unverified, i.numbers_unverified) {
            notes.push(format!(
                "Extraction: {what} could not be matched to the paper's wording; they are kept and labelled \u{201c}not verified\u{201d}."
            ));
        }
        if i.labels_defaulted > 0 {
            notes.push(format!(
                "{} from the writer {} not recognised; set to \u{201c}Very low\u{201d} or \u{201c}mixed evidence\u{201d}.",
                count_noun(
                    i.labels_defaulted,
                    "certainty or evidence label",
                    "certainty or evidence labels"
                ),
                if i.labels_defaulted == 1 { "was" } else { "were" }
            ));
        }
        if let Some(n) = self.skeptic.as_ref().and_then(SkepticV3::dropped_note) {
            notes.push(n);
        }
        if let Some(c) = &self.coverage {
            notes.push(format!("{}.", c.line()));
        }
        if let Some(c) = &self.completeness {
            notes.push(format!("{}.", c.line()));
        }
        notes
    }

    /// The answer as plain text (Stage 4 item 10): the page's sections in
    /// its order, citations as reference numbers, marks as words, and the
    /// numbered references. Nothing is left out. The completeness review is
    /// given this text, so it is kept exactly as it was; the Copy button
    /// uses [`AnswerV3::copy_text`].
    pub fn plain_text(&self) -> String {
        self.plain_text_with(false)
    }

    /// "Copy as text" (Part A): [`AnswerV3::plain_text`] with each p
    /// printed without a leading `P` or `=` (item 12). Same renderer.
    pub fn copy_text(&self) -> String {
        self.plain_text_with(true)
    }

    fn plain_text_with(&self, clean_p: bool) -> String {
        let refs = self.ref_numbers();
        let u = &self.untraced;
        let t = |text: &str, field: &str| plain(text, &refs, field, u, "not traced");
        // Stage 4b: the completeness review's text, with its own marks.
        let no_spans: Vec<Span> = Vec::new();
        let cu = self
            .completeness
            .as_ref()
            .map_or(&no_spans, |c| &c.untraced);
        let ct = |text: &str, field: &str| plain(text, &refs, field, cu, "not traced");
        let unsourced = |flag: bool| {
            if flag {
                format!(" ({NO_GUIDELINE_SOURCE})")
            } else {
                String::new()
            }
        };
        let mut o = format!(
            "{}\n\nDecision support \u{2014} the clinician decides.\n",
            self.question.trim()
        );
        let h = |o: &mut String, head: &str| o.push_str(&format!("\n{}\n", head.to_uppercase()));
        match &self.answer {
            None => {
                h(&mut o, "Bottom line");
                o.push_str(
                    self.empty_reason
                        .as_deref()
                        .unwrap_or("No answer was written."),
                );
                o.push('\n');
            }
            Some(w) => {
                if let Some(g) = &w.at_a_glance {
                    h(&mut o, "At a glance");
                    o.push_str(&format!("{}\n", t(&g.verdict, "at_a_glance.verdict")));
                    for (i, k) in g.key_numbers.iter().enumerate() {
                        let cite = refs
                            .get(&k.pmid)
                            .map(|n| format!(" [{n}]"))
                            .unwrap_or_default();
                        let label = if k.label.is_empty() {
                            String::new()
                        } else {
                            format!(
                                "{}: ",
                                t(&k.label, &format!("at_a_glance.key_numbers.{i}.label"))
                            )
                        };
                        o.push_str(&format!(
                            "- {label}{}{cite}\n",
                            t(
                                &k.value_text,
                                &format!("at_a_glance.key_numbers.{i}.value_text")
                            )
                        ));
                    }
                    if !g.main_caveat.is_empty() {
                        o.push_str(&format!(
                            "Main caveat: {}\n",
                            t(&g.main_caveat, "at_a_glance.main_caveat")
                        ));
                    }
                }
                h(&mut o, "Bottom line");
                o.push_str(&format!(
                    "{}\nCertainty: {} \u{2014} {}\n",
                    t(&w.bottom_line.text, "bottom_line.text"),
                    w.bottom_line.certainty.as_str(),
                    t(
                        &w.bottom_line.certainty_reason,
                        "bottom_line.certainty_reason"
                    )
                ));
                if let Some(c) = &w.clinical_practice {
                    h(&mut o, "What this means for clinical practice");
                    for (label, text, f) in [
                        ("Default choice", &c.default_choice, "default_choice"),
                        ("Who may benefit", &c.who_may_benefit, "who_may_benefit"),
                        ("Counselling", &c.counselling, "counselling"),
                        ("Cost per case", &c.cost_per_case, "cost_per_case"),
                    ] {
                        if !text.is_empty() {
                            o.push_str(&format!(
                                "{label}: {}\n",
                                t(text, &format!("clinical_practice.{f}"))
                            ));
                        }
                    }
                    if !c.text.is_empty() {
                        o.push_str(&format!("{}\n", t(&c.text, "clinical_practice.text")));
                    }
                }
                h(&mut o, "Outcome by outcome");
                for (i, x) in w.outcomes.iter().enumerate() {
                    o.push_str(&format!(
                        "- {} ({}certainty {}): {}\n  RCT evidence: {}\n  Why: {}\n",
                        t(&x.outcome, &format!("outcomes.{i}.outcome")),
                        x.evidence_type
                            .label()
                            .map(|l| format!("{l}; "))
                            .unwrap_or_default(),
                        x.certainty.as_str(),
                        t(&x.effect_text, &format!("outcomes.{i}.effect_text")),
                        t(&x.rct_evidence, &format!("outcomes.{i}.rct_evidence")),
                        t(&x.why, &format!("outcomes.{i}.why")),
                    ));
                    let comparison = |p: &str| {
                        w.studies
                            .iter()
                            .find(|s| s.pmid == p)
                            .map(|s| s.comparison.as_str())
                    };
                    for l in arm_lines(x, comparison) {
                        let cite = refs
                            .get(&l.pmid)
                            .map(|n| format!(" [{n}]"))
                            .unwrap_or_default();
                        let untraced = l
                            .numbers
                            .iter()
                            .any(|j| whole_marked(&format!("outcomes.{i}.numbers.{j}"), u));
                        o.push_str(&format!(
                            "  Numbers: {}{cite}{}\n",
                            arm_line_text_p(x, &l, clean_p),
                            if untraced { " (not traced)" } else { "" }
                        ));
                    }
                    // Stage 4b: sentences the completeness review appended.
                    if let Some(c) = &self.completeness {
                        for (k, a) in c.additions_to(i) {
                            o.push_str(&format!(
                                "  Added: {}{}{} ({ADDED_LABEL})\n",
                                ct(&a.text, &format!("outcome_additions.{k}.text")),
                                cites(&not_cited_at_end(&a.text, &a.pmids), &refs),
                                unsourced(a.unsourced)
                            ));
                        }
                    }
                }
                // Stage 4b: rows the completeness review added.
                if let Some(c) = &self.completeness {
                    for (k, x) in c.added_outcomes.iter().enumerate() {
                        o.push_str(&format!(
                            "- {} ({ADDED_LABEL}; certainty {}): {}{}{}\n  Why: {}\n",
                            ct(&x.outcome, &format!("added_outcomes.{k}.outcome")),
                            x.certainty.map_or("not graded", Certainty::as_str),
                            ct(&x.effect_text, &format!("added_outcomes.{k}.effect_text")),
                            cites(&not_cited_at_end(&x.effect_text, &x.pmids), &refs),
                            unsourced(x.unsourced),
                            ct(&x.why, &format!("added_outcomes.{k}.why")),
                        ));
                    }
                }
                h(&mut o, "The trials");
                for (i, s) in w.studies.iter().enumerate() {
                    let cite = refs
                        .get(&s.pmid)
                        .map(|n| format!(" [{n}]"))
                        .unwrap_or_default();
                    let n = s.n.map(|n| format!(" \u{b7} n = {n}")).unwrap_or_default();
                    o.push_str(&format!(
                        "- {}{cite}: {}{n} \u{b7} {}\n  Comparison: {}\n  Key results: {}\n  Appraisal: {}\n",
                        t(&s.label, &format!("studies.{i}.label")),
                        t(&s.design, &format!("studies.{i}.design")),
                        t(&s.setting, &format!("studies.{i}.setting")),
                        t(&s.comparison, &format!("studies.{i}.comparison")),
                        t(&s.key_results, &format!("studies.{i}.key_results")),
                        t(&s.appraisal, &format!("studies.{i}.appraisal")),
                    ));
                    for l in self.leads_on_row(&s.pmid) {
                        o.push_str(&format!("  Skeptic: {}\n", l.text()));
                    }
                    if let Some(e) = self.evidence.iter().find(|e| e.pmid == s.pmid) {
                        if e.found_by_skeptic {
                            o.push_str(&format!("  ({FOUND_BY_SKEPTIC}; {})\n", e.second_wave()));
                        }
                        if e.funding_checked {
                            let f = e.funding.as_deref().map_or_else(
                                || {
                                    format!(
                                        "not stated in the {}",
                                        if e.text == "full text" {
                                            "text"
                                        } else {
                                            "abstract"
                                        }
                                    )
                                },
                                |f| plain(f, &refs, "funding", &e.spans, "not verified"),
                            );
                            o.push_str(&format!("  Funding: {f}\n"));
                        }
                        if e.numbers_unverified > 0 {
                            let n = e.numbers_unverified;
                            let what = if n == 1 { "number" } else { "numbers" };
                            o.push_str(&format!("  {n} {what} not verified\n"));
                        }
                        if let Some(r) = &e.trial_registration_unverified {
                            o.push_str(&format!("  Registration {r} (not in the source)\n"));
                        }
                        for x in &e.unverified_details {
                            o.push_str(&format!(
                                "  Not verified against the paper's wording: {}: {}\n",
                                x.outcome, x.effect_text
                            ));
                        }
                    }
                }
                h(&mut o, "Conflicting evidence");
                let added_conflicts = self
                    .completeness
                    .as_ref()
                    .map_or(&[][..], |c| c.added_conflicts.as_slice());
                if w.conflicts.is_empty() && added_conflicts.is_empty() {
                    o.push_str("None identified.\n");
                }
                for (i, c) in w.conflicts.iter().enumerate() {
                    o.push_str(&format!(
                        "- {}\n",
                        t(&c.text, &format!("conflicts.{i}.text"))
                    ));
                }
                for (k, c) in added_conflicts.iter().enumerate() {
                    o.push_str(&format!(
                        "- {}{}{} ({ADDED_LABEL})\n",
                        ct(&c.text, &format!("added_conflicts.{k}.text")),
                        cites(&not_cited_at_end(&c.text, &c.pmids), &refs),
                        unsourced(c.unsourced)
                    ));
                }
                let no_spans: Vec<Span> = Vec::new();
                let sk_spans = self.skeptic.as_ref().map_or(&no_spans, |k| &k.spans);
                let skeptic_g: Vec<(usize, &SkepticItem)> = self
                    .skeptic
                    .iter()
                    .flat_map(|k| k.guidelines.iter().enumerate())
                    .filter(|(_, g)| {
                        !w.guidelines
                            .iter()
                            .any(|x| x.pmid.as_deref() == Some(g.pmid.as_str()))
                    })
                    .collect();
                if !w.guidelines.is_empty() || !skeptic_g.is_empty() {
                    h(&mut o, "Guidelines and consensus statements");
                    for (i, g) in w.guidelines.iter().enumerate() {
                        let cite = g
                            .pmid
                            .as_ref()
                            .and_then(|p| refs.get(p))
                            .map(|n| format!(" [{n}]"))
                            .unwrap_or_default();
                        let year = g.year.map(|y| format!(" ({y})")).unwrap_or_default();
                        // Stage 4 review 2: no source, said in words.
                        let flag = if g.pmid.is_none() {
                            format!(" ({NO_GUIDELINE_SOURCE})")
                        } else {
                            String::new()
                        };
                        o.push_str(&format!(
                            "- {}{year}: {}{cite}{flag}\n",
                            t(&g.body, &format!("guidelines.{i}.body")),
                            t(&g.statement, &format!("guidelines.{i}.statement"))
                        ));
                    }
                    for (i, g) in skeptic_g {
                        let cite = refs
                            .get(&g.pmid)
                            .map(|n| format!(" [{n}]"))
                            .unwrap_or_default();
                        let q = g
                            .quote
                            .as_deref()
                            .map(|q| format!(" \u{2014} \u{201c}{q}\u{201d}"))
                            .unwrap_or_default();
                        // Review 3: the skeptic's marks carried over.
                        let body = plain(
                            &g.text,
                            &refs,
                            &format!("guidelines.{i}.text"),
                            sk_spans,
                            "not verified",
                        );
                        o.push_str(&format!("- {body}{q}{cite}\n"));
                    }
                }
                h(&mut o, "Applicability");
                o.push_str(&format!(
                    "Case mix: {}\nIndia and cost: {}\nTraining: {}\n",
                    t(&w.applicability.case_mix, "applicability.case_mix"),
                    t(
                        &w.applicability.india_cost_context,
                        "applicability.india_cost_context"
                    ),
                    t(&w.applicability.training, "applicability.training")
                ));
                h(&mut o, "What would change this answer");
                for (i, x) in w.would_change.iter().enumerate() {
                    o.push_str(&format!("- {}\n", t(x, &format!("would_change.{i}"))));
                }
            }
        }
        let writer_notable = self
            .answer
            .as_ref()
            .map(|w| w.notable_findings.as_slice())
            .unwrap_or(&[]);
        // Stage 4b item 5: the skeptic's items are in its own section only;
        // a citation the text already ends with is not appended again.
        let code_notable: Vec<&NotableFinding> = self
            .notable_code
            .iter()
            .filter(|n| !n.repeats_skeptic())
            .collect();
        if !code_notable.is_empty() || !writer_notable.is_empty() {
            h(&mut o, "Other notable findings from this run");
            for n in code_notable {
                o.push_str(&format!(
                    "- {}{}\n",
                    plain(&n.text, &refs, "text", &n.spans, "not verified"),
                    cites(&not_cited_at_end(&n.text, &n.pmids), &refs)
                ));
            }
            for (i, n) in writer_notable.iter().enumerate() {
                o.push_str(&format!(
                    "- {}{}\n",
                    t(&n.text, &format!("notable_findings.{i}.text")),
                    cites(&not_cited_at_end(&n.text, &n.pmids), &refs)
                ));
            }
        }
        if let Some(k) = &self.skeptic {
            h(&mut o, "Challenged by the skeptic");
            o.push_str(&format!("Verdict: the conclusion {}.\n", k.verdict));
            let sk = |text: &str, field: &str| plain(text, &refs, field, &k.spans, "not verified");
            for (i, m) in k.must_mention.iter().enumerate() {
                o.push_str(&format!(
                    "- Caveat: {}\n",
                    sk(m, &format!("must_mention.{i}"))
                ));
            }
            for (i, x) in k.overlaps.iter().enumerate() {
                o.push_str(&format!(
                    "- Not independent: {}{}\n",
                    sk(&x.note, &format!("overlaps.{i}.note")),
                    cites(&x.pmids, &refs)
                ));
            }
            let items = [
                ("Picked for extraction", &k.extract_next, "extract_next"),
                ("Missed trial", &k.missed_rcts, "missed_rcts"),
                ("What a meta-analysis pooled", &k.ma_notes, "ma_notes"),
                ("Evidence against", &k.contradicting, "contradicting"),
                ("Guideline", &k.guidelines, "guidelines"),
            ];
            for (label, list, path) in items {
                for (i, x) in list.iter().enumerate() {
                    let q = x
                        .quote
                        .as_deref()
                        .map(|q| format!(" \u{2014} \u{201c}{q}\u{201d}"))
                        .unwrap_or_default();
                    o.push_str(&format!(
                        "- {label}: {}{q}{}\n",
                        sk(&x.text, &format!("{path}.{i}.text")),
                        cites(std::slice::from_ref(&x.pmid), &refs)
                    ));
                }
            }
        }
        if let Some(k) = &self.skeptic {
            // Unverified leads (review 6): PMID and kind only. Stage 4b
            // item 5: a lead on a study with a trial row is noted there.
            for l in k.leads.iter().filter(|l| !self.lead_on_row(&l.pmid)) {
                o.push_str(&format!(
                    "- {}{}\n",
                    l.text(),
                    cites(std::slice::from_ref(&l.pmid), &refs)
                ));
                if !refs.contains_key(&l.pmid) {
                    o.push_str(&format!("  PubMed {}\n", l.pmid));
                }
            }
        }
        // Stage 4b: the completeness review's notes and what it judged not
        // relevant (its line is with the run's notes below).
        if let Some(c) = &self.completeness
            && (!c.notes.is_empty() || !c.not_added.is_empty())
        {
            h(&mut o, "Completeness review");
            for (k, n) in c.notes.iter().enumerate() {
                o.push_str(&format!(
                    "- Note ({ADDED_LABEL}): {}\n",
                    ct(n, &format!("notes.{k}"))
                ));
            }
            for n in &c.not_added {
                o.push_str(&format!(
                    "- Judged not relevant: {} \u{2014} {}\n",
                    cited_or_pmid(&n.gap, &refs),
                    cited_or_pmid(&n.reason, &refs)
                ));
            }
        }
        // The run's notes and the search trail's (review 7).
        let trail = self.trail_notes();
        if !self.notes.is_empty() || !trail.is_empty() {
            o.push('\n');
            for n in self.notes.iter().chain(&trail) {
                o.push_str(&format!("Note: {n}\n"));
            }
        }
        h(&mut o, "References");
        if self.references.is_empty() {
            o.push_str("None.\n");
        }
        for r in &self.references {
            let mut line = format!("[{}] ", r.n);
            if r.retracted {
                line.push_str("RETRACTED \u{2014} do not rely on this paper. ");
            }
            line.push_str(r.title.trim_end_matches('.'));
            line.push('.');
            match (&r.journal, r.year) {
                (Some(j), Some(y)) => line.push_str(&format!(" {j}, {y}.")),
                (Some(j), None) => line.push_str(&format!(" {j}.")),
                (None, Some(y)) => line.push_str(&format!(" {y}.")),
                (None, None) => {}
            }
            line.push_str(&format!(" PMID {}.", r.pmid));
            if let Some(d) = &r.doi {
                line.push_str(&format!(" DOI {d}."));
            }
            o.push_str(&format!("{line}\n"));
        }
        if !self.untraced.is_empty()
            || self
                .completeness
                .as_ref()
                .is_some_and(|c| !c.untraced.is_empty())
        {
            o.push_str("\n(not traced): the number was not found in a verified quote, a structured number or an abstract given to the writer.\n");
        }
        o
    }
}

/// Review minor 4: `text` with each citation marker as reference numbers
/// ("[1]"), or "PMID 12345678" for a study with no reference: no PMID is
/// dropped or left as a raw marker.
pub fn cited_or_pmid(text: &str, refs: &HashMap<String, u16>) -> String {
    let mut out = String::new();
    for seg in split_citations(text, |p| refs.contains_key(p)) {
        match seg {
            Segment::Text(t) => out.push_str(&t),
            Segment::Cite(ids) => {
                let parts: Vec<String> = ids
                    .iter()
                    .map(|p| refs.get(p).map_or(format!("PMID {p}"), |n| n.to_string()))
                    .collect();
                out.push_str(&format!("[{}]", parts.join(", ")));
            }
        }
    }
    out
}

/// " [1, 3]" for the PMIDs that have references.
fn cites(pmids: &[String], refs: &HashMap<String, u16>) -> String {
    let mut n: Vec<u16> = pmids
        .iter()
        .filter_map(|p| refs.get(p.trim()).copied())
        .collect();
    n.sort_unstable();
    n.dedup();
    if n.is_empty() {
        return String::new();
    }
    let n: Vec<String> = n.iter().map(u16::to_string).collect();
    format!(" [{}]", n.join(", "))
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

    fn n(label: &str, value: f64, unit: &str, arm: &str, pmid: &str) -> Number {
        Number {
            label: label.into(),
            value,
            unit: Some(unit.into()),
            arm: Some(arm.into()),
            ci_low: None,
            ci_high: None,
            p: None,
            pmid: pmid.into(),
        }
    }

    fn outcome(numbers: Vec<Number>) -> Outcome {
        Outcome {
            outcome: "Operative time".into(),
            rct_evidence: "ROGER".into(),
            effect_text: "64.2 vs 80.3 min".into(),
            numbers,
            certainty: Certainty::Low,
            why: "w".into(),
            pmids: vec![A.into()],
            evidence_type: EvidenceType::Rct,
        }
    }

    /// Stage 4 item 10: the live ROGER row read "64.2 vs 80.3 min" (TEP
    /// first) under "Robotic TAPP vs laparoscopic TEP". From the
    /// structured arms, the intervention arm is drawn first and every
    /// number names its arm.
    #[test]
    fn arm_lines_put_the_intervention_arm_first() {
        let o = outcome(vec![
            n(
                "Unilateral operative time",
                64.2,
                "min",
                "laparoscopic TEP",
                A,
            ),
            n("Unilateral operative time", 80.3, "min", "robotic TAPP", A),
            n("Procedural cost", 2732.0, "USD", "lTEP", A),
            n("Procedural cost", 3327.0, "USD", "rTAPP (robotic)", A),
            n("Complications", 11.0, "%", "laparoscopic", B),
        ]);
        let cmp = |p: &str| (p == A).then_some("Robotic TAPP vs laparoscopic TEP");
        let lines = arm_lines(&o, cmp);
        let text: Vec<String> = lines.iter().map(|l| arm_line_text(&o, l)).collect();
        assert_eq!(
            text,
            [
                "Unilateral operative time: 80.3 min (robotic TAPP) vs 64.2 min (laparoscopic TEP)",
                "Procedural cost: 3327 USD (rTAPP (robotic)) vs 2732 USD (lTEP)",
                "Complications: 11% (laparoscopic)",
            ]
        );
        // No comparison to go by: the writer's order is kept.
        let lines = arm_lines(&o, |_| None);
        assert_eq!(lines[0].numbers, [0, 1]);
        let mut x = n("RR", 0.76, "", "", A);
        x.ci_low = Some(0.49);
        x.ci_high = Some(1.17);
        x.p = Some("0.21".into());
        x.arm = None;
        x.unit = None;
        assert_eq!(number_text(&x), "0.76, CI 0.49 to 1.17, p 0.21");
    }

    fn sample() -> AnswerV3 {
        AnswerV3 {
            version: ANSWER_VERSION,
            question: "Robotic vs lap?".into(),
            mode: RunMode::Focused,
            created_at: Utc::now(),
            answer: Some(WrittenAnswer {
                bottom_line: BottomLine {
                    text: format!("No benefit; 37% fewer [{A}; 99999999]."),
                    certainty: Certainty::Low,
                    certainty_reason: "small".into(),
                },
                outcomes: vec![outcome(vec![
                    n("Time", 64.2, "min", "laparoscopic TEP", A),
                    n("Time", 80.3, "min", "robotic TAPP", A),
                ])],
                studies: vec![Study {
                    pmid: A.into(),
                    label: "ROGER (2025)".into(),
                    design: "RCT".into(),
                    n: Some(182),
                    setting: "Switzerland".into(),
                    comparison: "Robotic TAPP vs laparoscopic TEP".into(),
                    key_results: format!("Longer [{A}]"),
                    appraisal: "blinded".into(),
                }],
                conflicts: vec![],
                applicability: Applicability {
                    case_mix: "primary".into(),
                    india_cost_context: "not known".into(),
                    training: "none".into(),
                },
                would_change: vec!["a big RCT".into()],
                at_a_glance: Some(AtAGlance {
                    verdict: "No patient benefit; costs more.".into(),
                    key_numbers: vec![KeyNumber {
                        label: "Cost".into(),
                        value_text: "$3327 robotic vs $2732 laparoscopic".into(),
                        pmid: A.into(),
                    }],
                    main_caveat: "Small trials.".into(),
                }),
                clinical_practice: Some(ClinicalPractice {
                    default_choice: "Laparoscopic repair.".into(),
                    who_may_benefit: "Not shown.".into(),
                    counselling: "Similar recovery.".into(),
                    cost_per_case: "not known".into(),
                    text: String::new(),
                }),
                guidelines: vec![],
                notable_findings: vec![NotableFinding {
                    text: format!("Withdrawals 2 vs 9 [{A}]."),
                    pmids: vec![A.into()],
                    spans: vec![],
                }],
            }),
            empty_reason: None,
            references: vec![ReferenceV3 {
                n: 1,
                pmid: A.into(),
                title: "ROGER trial.".into(),
                journal: Some("Br J Surg".into()),
                year: Some(2025),
                doi: Some("10.1093/bjs/znaf283".into()),
                retracted: false,
                found_by_skeptic: false,
            }],
            searches: vec![],
            integrity: IntegrityV3::default(),
            stopped_early: None,
            cost_paise: 0,
            elapsed_s: 0,
            skeptic: Some(SkepticV3 {
                verdict: "holds with caveats".into(),
                must_mention: vec!["Recurrence 7% in 2019.".into()],
                contradicting: vec![],
                missed_rcts: vec![],
                guidelines: vec![],
                overlaps: vec![],
                ma_notes: vec![],
                spans: vec![Span::new("must_mention.0", 11, 13)],
                leads: vec![],
                dropped_unseen: 0,
                dropped_quotes: 0,
                extract_next: vec![],
            }),
            evidence: vec![],
            notes: vec![],
            untraced: vec![Span::new("bottom_line.text", 12, 15)],
            notable_code: vec![NotableFinding {
                text: "Not independent evidence (the skeptic): same centre.".into(),
                pmids: vec![A.into()],
                spans: vec![],
            }],
            coverage: None,
            completeness: None,
        }
    }

    /// Stage 4b: `coverage` and `completeness` are additive. An answer
    /// stored before (without them) loads with neither, and one without
    /// them is stored without the keys, so old answers round-trip as they
    /// were.
    #[test]
    fn coverage_and_completeness_are_additive() {
        let a = sample();
        let mut v = serde_json::to_value(&a).unwrap();
        assert!(v.get("coverage").is_none() && v.get("completeness").is_none());
        v.as_object_mut().unwrap().remove("coverage");
        let back: AnswerV3 = serde_json::from_value(v).unwrap();
        assert_eq!(back, a);
        assert!(back.coverage.is_none() && back.completeness.is_none());
        // A skipped review alone (old stored shape of its fields) loads.
        let c: CompletenessV3 =
            serde_json::from_value(serde_json::json!({"skipped": "time"})).unwrap();
        assert_eq!(
            c.line(),
            "Completeness review skipped: time; the answer is as the writer wrote it"
        );
        assert_eq!(c.added(), 0);
    }

    /// Stage 4b item 5: counts agree in number and zero parts are left out
    /// ("1 number", not "1 numbers"; not "0 outcomes and 5 numbers").
    #[test]
    fn counts_agree_and_drop_zero_parts() {
        assert_eq!(unverified_counts(0, 5).as_deref(), Some("5 numbers"));
        assert_eq!(unverified_counts(1, 0).as_deref(), Some("1 outcome"));
        assert_eq!(
            unverified_counts(2, 1).as_deref(),
            Some("2 outcomes and 1 number")
        );
        assert_eq!(unverified_counts(0, 0), None);
        let mut a = sample();
        a.integrity = IntegrityV3 {
            dropped_unseen: 1,
            removed_citations: 1,
            numbers_unverified: 1,
            labels_defaulted: 1,
            ..IntegrityV3::default()
        };
        let n = a.trail_notes().join("\n");
        for want in [
            "1 study selected by the researcher but not returned",
            "Citation check: 1 unknown citation removed.",
            "Extraction: 1 number could not be matched",
            "1 certainty or evidence label from the writer was not recognised",
        ] {
            assert!(n.contains(want), "{want:?} in {n}");
        }
        assert!(!n.contains(" 0 "), "{n}");
    }

    /// Stage 4b item 5: a lead on a selected study that has a trial row is
    /// noted on that row, not listed as a PMID; other leads stay listed.
    #[test]
    fn leads_on_selected_studies_go_to_their_row() {
        let mut a = sample();
        a.evidence = vec![EvidenceV3 {
            pmid: A.into(),
            text: "abstract only".into(),
            verified_quotes: 1,
            numbers_dropped: 0,
            unverified_outcomes: vec![],
            numbers_unverified: 0,
            unverified_details: vec![],
            funding: None,
            funding_checked: false,
            spans: vec![],
            trial_registration_unverified: None,
            found_by_skeptic: false,
        }];
        let k = a.skeptic.as_mut().unwrap();
        k.leads = vec![
            SkepticLead {
                pmid: A.into(),
                kind: "contradicting study".into(),
            },
            SkepticLead {
                pmid: "55555555".into(),
                kind: "guideline".into(),
            },
        ];
        assert!(a.lead_on_row(A) && !a.lead_on_row("55555555"));
        let t = a.plain_text();
        assert!(t.contains(
            "  Skeptic: Possible contradicting study: quote not found in the abstract, check the paper\n"
        ), "{t}");
        assert!(
            t.contains("- Possible guideline: quote not found in the abstract, check the paper")
        );
        assert!(t.contains("PubMed 55555555"));
        assert!(!t.contains(&format!("PubMed {A}")));
        assert_eq!(t.matches("Possible contradicting study").count(), 1, "{t}");
    }

    /// Stage 4b item 5: a text that already ends with a citation gets no
    /// second one; other PMIDs are still appended.
    #[test]
    fn citations_already_at_the_end_are_not_appended() {
        let p = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<String>>();
        assert!(not_cited_at_end(&format!("Pain fell [{A}]."), &p(&[A])).is_empty());
        assert!(not_cited_at_end(&format!("Pain fell [{A}; {B}]. "), &p(&[A, B])).is_empty());
        assert_eq!(
            not_cited_at_end(&format!("Pain fell [{A}]."), &p(&[A, B])),
            p(&[B])
        );
        assert_eq!(
            not_cited_at_end(&format!("[{A}] pain fell."), &p(&[A])),
            p(&[A])
        );
        assert_eq!(not_cited_at_end("No citation.", &p(&[A])), p(&[A]));
    }

    /// Part A item 12: Copy as text prints "p < .001", never "p P < .001";
    /// the plain text the completeness review is given is unchanged.
    #[test]
    fn copy_text_normalises_p_and_plain_text_is_unchanged() {
        let mut a = sample();
        let w = a.answer.as_mut().unwrap();
        w.outcomes[0].numbers[0].p = Some("P < .001".into());
        w.outcomes[0].numbers[1].p = Some("p=0.03".into());
        let plain = a.plain_text();
        let copy = a.copy_text();
        assert!(plain.contains("p P < .001") && plain.contains("p p=0.03"));
        assert!(copy.contains(", p < .001") && copy.contains(", p 0.03"));
        assert!(!copy.contains("p P <"));
        // Only the p values differ.
        assert_eq!(
            plain
                .replace("p P < .001", "p < .001")
                .replace("p p=0.03", "p 0.03"),
            copy
        );
        // Without p values the two are byte-identical.
        let b = sample();
        assert_eq!(b.plain_text(), b.copy_text());
    }

    /// Stage 4 item 10: Copy as text holds every section, citations as
    /// reference numbers, marks as words, the arms in order and the
    /// references; no raw PMID markers.
    #[test]
    fn plain_text_has_every_section_in_page_order() {
        let t = sample().plain_text();
        let mut at = 0;
        for want in [
            "Robotic vs lap?",
            "Decision support",
            "AT A GLANCE",
            "No patient benefit; costs more.",
            "- Cost: $3327 robotic vs $2732 laparoscopic [1]",
            "BOTTOM LINE",
            "No benefit; 37% (not traced) fewer [1].",
            "Certainty: Low",
            "WHAT THIS MEANS FOR CLINICAL PRACTICE",
            "Cost per case: not known",
            "OUTCOME BY OUTCOME",
            "- Operative time (RCTs; certainty Low): 64.2 vs 80.3 min",
            "Numbers: Time: 80.3 min (robotic TAPP) vs 64.2 min (laparoscopic TEP) [1]",
            "THE TRIALS",
            "- ROGER (2025) [1]: RCT \u{b7} n = 182",
            "Key results: Longer [1]",
            "APPLICABILITY",
            "WHAT WOULD CHANGE THIS ANSWER",
            "OTHER NOTABLE FINDINGS FROM THIS RUN",
            // Stage 4b item 5: the citation is not appended twice.
            "- Withdrawals 2 vs 9 [1].\n",
            "CHALLENGED BY THE SKEPTIC",
            "- Caveat: Recurrence 7% (not verified) in 2019.",
            "REFERENCES",
            "[1] ROGER trial. Br J Surg, 2025. PMID 41060645. DOI 10.1093/bjs/znaf283.",
        ] {
            let i = t[at..]
                .find(want)
                .unwrap_or_else(|| panic!("{want:?} missing or out of order in:\n{t}"));
            at += i + want.len();
        }
        assert!(!t.contains("[41060645"), "{t}");
        assert!(!t.contains("99999999"));
        // Stage 4b item 5: the skeptic's items are in its section only.
        assert!(!t.contains("[1]. [1]"), "{t}");
        assert!(!t.contains("Not independent evidence (the skeptic)"), "{t}");
    }

    /// Stage 4 review 2, 3, 5, 6 and 7 in Copy as text: a guideline with no
    /// source is flagged in words; a skeptic guideline keeps its mark; the
    /// trail notes, per-study unverified counts, the unverified
    /// registration and the leads (no claim, no quote) are all there.
    #[test]
    fn plain_text_carries_the_review_flags_and_notes() {
        let mut a = sample();
        let w = a.answer.as_mut().unwrap();
        w.guidelines = vec![GuidelineV3 {
            pmid: None,
            body: "A society".into(),
            year: Some(2020),
            statement: "Use either.".into(),
            pmid_removed: true,
        }];
        let k = a.skeptic.as_mut().unwrap();
        k.guidelines = vec![SkepticItem {
            pmid: A.into(),
            text: "ACG 2020 guideline".into(),
            quote: Some("We recommend POEM or LHM.".into()),
        }];
        k.spans.push(Span::new("guidelines.0.text", 4, 8));
        k.leads = vec![SkepticLead {
            pmid: "39999999".into(),
            kind: "contradicting study".into(),
        }];
        k.dropped_unseen = 2;
        a.evidence = vec![EvidenceV3 {
            pmid: A.into(),
            text: "abstract only".into(),
            verified_quotes: 3,
            numbers_dropped: 0,
            unverified_outcomes: vec![],
            numbers_unverified: 3,
            unverified_details: vec![],
            funding: None,
            funding_checked: true,
            spans: vec![],
            trial_registration_unverified: Some("NCT9999".into()),
            found_by_skeptic: false,
        }];
        a.integrity.numbers_checked = 40;
        a.integrity.numbers_untraced = 1;
        let t = a.plain_text();
        for want in [
            "- A society (2020): Use either. (no source among the studies; not verified)",
            "- ACG 2020 (not verified) guideline \u{2014} \u{201c}We recommend POEM or LHM.\u{201d} [1]",
            "  3 numbers not verified",
            "  Registration NCT9999 (not in the source)",
            "- Possible contradicting study: quote not found in the abstract, check the paper",
            "  PubMed 39999999",
            "Note: Number check: 1 of 40 numbers in the answer not traced",
            "Note: The skeptic gave 2 entries whose quote or study could not be found; not shown.",
        ] {
            assert!(t.contains(want), "{want:?} missing in:\n{t}");
        }
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
