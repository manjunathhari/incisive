//! The structured answer (§4), shared by server and browser.
//!
//! The pipeline stores an [`Answer`] as `run_answers.body`; the run page
//! deserializes it and draws it as HTML, and [`render_text`] gives the same
//! content as plain text (tests and copy-to-clipboard). Everything here
//! compiles without `ssr`: it depends only on serde, chrono and the shared run
//! types.
//!
//! Privacy: the answer holds the question and the PICO. It is stored only in
//! `run_answers` (private to its user) and is never logged or audited.

use chrono::{DateTime, FixedOffset, Utc};
use serde::{Deserialize, Serialize};

use crate::runs::RunMode;

/// First line of every answer.
pub const DECISION_SUPPORT: &str = "Decision support \u{2014} the clinician decides.";
/// Bottom line when Check removed every claim (Review Focus 2).
pub const NO_VERIFIED_CLAIMS: &str = "No claims could be verified against the sources.";
/// Bottom line when nothing relevant was found (empty search, nothing included
/// or nothing extracted).
pub const NO_EVIDENCE: &str = "No relevant evidence found.";
/// Shown above the bottom line when the evidence is too thin for a conclusion.
pub const INSUFFICIENT: &str = "Insufficient evidence \u{2014} no conclusion is drawn.";
/// Marks a partly supported claim.
pub const PARTIAL_MARK: &str = "\u{25d0}";

/// The question as Population, Intervention, Comparison, Outcomes, plus any
/// limits the user stated (design, date range). Written by Frame.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pico {
    pub population: String,
    pub intervention: String,
    #[serde(default)]
    pub comparison: Option<String>,
    #[serde(default)]
    pub outcomes: Vec<String>,
    #[serde(default)]
    pub limits: Vec<String>,
}

/// One sentence of the bottom line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnswerClaim {
    pub text: String,
    /// Numbers in [`Answer::references`].
    pub refs: Vec<u8>,
    /// "strong", "moderate" or "limited".
    pub confidence: String,
    /// Check found it only partly supported (shown with ◐).
    pub partial: bool,
}

/// A row of the key-studies table (Focused only).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyStudy {
    pub ref_id: u8,
    /// First author, journal, year.
    pub citation: String,
    pub design: String,
    pub n: Option<u32>,
    /// The first extracted finding: outcome and result with its effect size.
    pub main_finding: String,
    /// "full text" or "abstract only".
    pub source: String,
}

/// A numbered reference. Only references verified live are listed; retracted
/// or non-resolving papers never appear.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reference {
    pub ref_id: u8,
    pub title: String,
    pub journal: Option<String>,
    pub year: Option<i32>,
    pub pmid: Option<String>,
    pub doi: Option<String>,
    /// Evidence level, e.g. "RCT", "meta-analysis".
    pub study_type: String,
}

/// What Check did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckSummary {
    /// Claims drafted and checked.
    pub checked: u32,
    pub supported: u32,
    pub partial: u32,
    pub removed: u32,
    pub refs_verified: u32,
    pub refs_total: u32,
}

/// One accepted query refinement.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Refinement {
    pub query: String,
    pub reason: String,
}

/// How the evidence was found.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Trail {
    pub pubmed_count: u64,
    pub epmc_count: u64,
    /// Records screened (after merging).
    pub screened: u32,
    pub included: u32,
    /// The most common exclusion reasons with their counts.
    pub excluded_top: Vec<(String, u32)>,
    pub refinements: Vec<Refinement>,
    /// "cost limit" or "time limit" when the search stopped early.
    pub stopped_early: Option<String>,
    /// Databases that could not be reached ("PubMed", "Europe PMC").
    pub unavailable: Vec<String>,
}

/// The stored answer (§4).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Answer {
    pub question: String,
    pub mode: RunMode,
    pub created_at: DateTime<Utc>,
    pub pico: Pico,
    pub final_query: String,
    pub bottom_line: Vec<AnswerClaim>,
    pub insufficient: bool,
    /// Claims were drafted (or evidence extracted) but Check kept none.
    pub no_verified_claims: bool,
    /// Empty in Quick mode.
    pub key_studies: Vec<KeyStudy>,
    pub references: Vec<Reference>,
    pub check: CheckSummary,
    pub trail: Trail,
    pub cost_paise: i64,
    pub elapsed_s: u64,
}

/// What the bottom line says when it has no claim to show, if anything.
pub fn empty_bottom_line(a: &Answer) -> Option<&'static str> {
    if !a.bottom_line.is_empty() {
        None
    } else if a.no_verified_claims {
        Some(NO_VERIFIED_CLAIMS)
    } else {
        Some(NO_EVIDENCE)
    }
}

/// Whether to show [`INSUFFICIENT`] above the claims: only when there are
/// claims (with none, the empty-bottom-line message says it all).
pub fn show_insufficient(a: &Answer) -> bool {
    a.insufficient && !a.bottom_line.is_empty()
}

/// `₹12.34`.
pub fn rupees(paise: i64) -> String {
    let sign = if paise < 0 { "-" } else { "" };
    let p = paise.unsigned_abs();
    format!("{sign}\u{20b9}{}.{:02}", p / 100, p % 100)
}

/// `2 min 41 s`, or `41 s` under a minute.
pub fn duration_text(s: u64) -> String {
    if s >= 60 {
        format!("{} min {} s", s / 60, s % 60)
    } else {
        format!("{s} s")
    }
}

/// The answer date in IST, e.g. `28 Oct 2026, 14:05 IST`.
pub fn date_ist(t: DateTime<Utc>) -> String {
    let ist = FixedOffset::east_opt(5 * 3600 + 30 * 60).expect("IST offset is valid");
    t.with_timezone(&ist)
        .format("%d %b %Y, %H:%M IST")
        .to_string()
}

pub fn mode_label(m: RunMode) -> &'static str {
    match m {
        RunMode::Quick => "Quick",
        RunMode::Focused => "Focused",
    }
}

/// `[1, 2]`.
pub fn refs_text(refs: &[u8]) -> String {
    let r: Vec<String> = refs.iter().map(u8::to_string).collect();
    format!("[{}]", r.join(", "))
}

/// The check result line, e.g. "14 claims checked: 13 supported, 1 partly
/// supported ◐, 0 removed. 9/9 references verified."
pub fn check_text(c: &CheckSummary) -> String {
    format!(
        "{} claims checked: {} supported, {} partly supported {PARTIAL_MARK}, {} removed. {}/{} references verified.",
        c.checked, c.supported, c.partial, c.removed, c.refs_verified, c.refs_total
    )
}

/// "Screened 15, included 2, excluded 13".
pub fn screened_text(t: &Trail) -> String {
    format!(
        "Screened {}, included {}, excluded {}",
        t.screened,
        t.included,
        t.screened.saturating_sub(t.included)
    )
}

/// "Top exclusion reasons: wrong population (3); …", or `None`.
pub fn exclusions_text(t: &Trail) -> Option<String> {
    if t.excluded_top.is_empty() {
        return None;
    }
    let r: Vec<String> = t
        .excluded_top
        .iter()
        .map(|(r, n)| format!("{r} ({n})"))
        .collect();
    Some(format!("Top exclusion reasons: {}", r.join("; ")))
}

/// "Search stopped early (cost limit)."
pub fn stopped_text(reason: &str) -> String {
    format!("Search stopped early ({reason}).")
}

/// "Unavailable: Europe PMC.", or `None`.
pub fn unavailable_text(t: &Trail) -> Option<String> {
    (!t.unavailable.is_empty()).then(|| format!("Unavailable: {}.", t.unavailable.join(", ")))
}

/// One reference line: "Title. Journal, 2025. PMID 123. DOI 10.x. (RCT)".
pub fn reference_text(r: &Reference) -> String {
    let mut s = r.title.trim_end_matches('.').to_string();
    s.push('.');
    match (&r.journal, r.year) {
        (Some(j), Some(y)) => s.push_str(&format!(" {j}, {y}.")),
        (Some(j), None) => s.push_str(&format!(" {j}.")),
        (None, Some(y)) => s.push_str(&format!(" {y}.")),
        (None, None) => {}
    }
    if let Some(p) = &r.pmid {
        s.push_str(&format!(" PMID {p}."));
    }
    if let Some(d) = &r.doi {
        s.push_str(&format!(" DOI {d}."));
    }
    s.push_str(&format!(" ({})", r.study_type));
    s
}

/// One key-study line: "Smith, JAMA Surg, 2025 · RCT · n=1136 · … · abstract only".
pub fn key_study_text(k: &KeyStudy) -> String {
    let n =
        k.n.map_or_else(|| "n not stated".to_string(), |n| format!("n={n}"));
    format!(
        "{} \u{b7} {} \u{b7} {n} \u{b7} {} \u{b7} {}",
        k.citation, k.design, k.main_finding, k.source
    )
}

/// One claim line: "◐ text [1, 2] (moderate, partly supported)".
pub fn claim_text(c: &AnswerClaim) -> String {
    if c.partial {
        format!(
            "{PARTIAL_MARK} {} {} ({}, partly supported)",
            c.text,
            refs_text(&c.refs),
            c.confidence
        )
    } else {
        format!("{} {} ({})", c.text, refs_text(&c.refs), c.confidence)
    }
}

/// The answer as plain text, in the §4 order, starting with
/// [`DECISION_SUPPORT`].
pub fn render_text(a: &Answer) -> String {
    let mut o = String::new();
    let mut line = |s: &str| {
        o.push_str(s);
        o.push('\n');
    };
    line(DECISION_SUPPORT);
    line("");
    line(&a.question);
    line(&format!(
        "{} \u{b7} {}",
        mode_label(a.mode),
        date_ist(a.created_at)
    ));
    line("");

    line("How I read your question");
    line(&format!("Population: {}", a.pico.population));
    line(&format!("Intervention: {}", a.pico.intervention));
    line(&format!(
        "Comparison: {}",
        a.pico.comparison.as_deref().unwrap_or("none stated")
    ));
    if !a.pico.outcomes.is_empty() {
        line(&format!("Outcomes: {}", a.pico.outcomes.join("; ")));
    }
    if !a.pico.limits.is_empty() {
        line(&format!("Limits: {}", a.pico.limits.join("; ")));
    }
    if !a.final_query.is_empty() {
        line(&format!("PubMed query: {}", a.final_query));
    }
    line("");

    line("Bottom line");
    if show_insufficient(a) {
        line(INSUFFICIENT);
    }
    match empty_bottom_line(a) {
        Some(msg) => line(msg),
        None => {
            for c in &a.bottom_line {
                line(&format!("- {}", claim_text(c)));
            }
        }
    }
    line("");

    if !a.key_studies.is_empty() {
        line("Key studies");
        for k in &a.key_studies {
            line(&format!("{}. {}", k.ref_id, key_study_text(k)));
        }
        line("");
    }

    line("References");
    if a.references.is_empty() {
        line("None verified.");
    }
    for r in &a.references {
        line(&format!("{}. {}", r.ref_id, reference_text(r)));
    }
    line("");

    line("Check");
    line(&check_text(&a.check));
    line("");

    let t = &a.trail;
    line("Search trail");
    line(&format!(
        "PubMed: {} hits \u{b7} Europe PMC: {} hits",
        t.pubmed_count, t.epmc_count
    ));
    line(&screened_text(t));
    if let Some(x) = exclusions_text(t) {
        line(&x);
    }
    for r in &t.refinements {
        line(&format!("Refined: {} ({})", r.reason, r.query));
    }
    if let Some(s) = &t.stopped_early {
        line(&stopped_text(s));
    }
    if let Some(u) = unavailable_text(t) {
        line(&u);
    }
    line("");

    line(&format!(
        "Cost and time: {} \u{b7} {}",
        rupees(a.cost_paise),
        duration_text(a.elapsed_s)
    ));
    o
}

/// The Edit & re-run link: `/?q=<question>&mode=<mode>`, percent-encoded.
pub fn rerun_href(question: &str, mode: RunMode) -> String {
    format!("/?q={}&mode={}", percent_encode(question), mode.as_str())
}

/// Percent-encode everything but the URL-unreserved characters.
pub fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(char::from(b));
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}
