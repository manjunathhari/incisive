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

/// `text` cut into plain runs and citation runs, in order.
pub fn split_citations(text: &str) -> Vec<Segment> {
    let mut out = Vec::new();
    let mut last = 0;
    for m in CITE.find_iter(text) {
        if m.start() > last {
            out.push(Segment::Text(text[last..m.start()].to_string()));
        }
        let ids = DIGITS
            .find_iter(m.as_str())
            .map(|d| d.as_str().to_string())
            .collect();
        out.push(Segment::Cite(ids));
        last = m.end();
    }
    if last < text.len() {
        out.push(Segment::Text(text[last..].to_string()));
    }
    out
}

/// `text` with every citation marker rewritten to keep only PMIDs for which
/// `known` is true (de-duplicated, in order, as `[a; b]`); a marker left
/// empty is removed with the blank before it. Returns the text and how many
/// PMID citations were removed.
pub fn clean_citations(text: &str, known: impl Fn(&str) -> bool) -> (String, u32) {
    let mut removed = 0u32;
    let mut out = String::with_capacity(text.len());
    for seg in split_citations(text) {
        match seg {
            Segment::Text(t) => out.push_str(&t),
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

    #[test]
    fn splits_markers_of_every_shape() {
        let s = split_citations("Pain was similar [123; 456]. Cost [PMID: 789], [1,2] end");
        assert_eq!(
            s,
            vec![
                Segment::Text("Pain was similar ".into()),
                Segment::Cite(vec!["123".into(), "456".into()]),
                Segment::Text(". Cost ".into()),
                Segment::Cite(vec!["789".into()]),
                Segment::Text(", ".into()),
                Segment::Cite(vec!["1".into(), "2".into()]),
                Segment::Text(" end".into()),
            ]
        );
        // Brackets that are not citations stay text.
        let s = split_citations("95% CI [0.2 to 1.39] and [n=12] and [a]");
        assert!(s.iter().all(|x| matches!(x, Segment::Text(_))), "{s:?}");
    }

    #[test]
    fn clean_keeps_known_and_removes_unknown() {
        let known = |p: &str| p == "111" || p == "222";
        let (t, n) = clean_citations("A [111; 999]. B [999]. C [222; 222; 111].", known);
        assert_eq!(t, "A [111]. B. C [222; 111].");
        assert_eq!(n, 2);
        let (t, n) = clean_citations("No markers here.", known);
        assert_eq!((t.as_str(), n), ("No markers here.", 0));
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
