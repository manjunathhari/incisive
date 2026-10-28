//! Screen (step 3): each record judged against the PICO, then the included
//! ones ranked by evidence level. Server only.
//!
//! * Retracted records are excluded first ("retracted"); they never reach the
//!   model.
//! * The rest go to the `screen` model in batches of [`BATCH`] (10). Each
//!   record is wrapped as `<source id="p-N">` (N counts across batches), with
//!   its title and abstract; the abstract is cut to [`MAX_ABSTRACT`] (3,000)
//!   characters at a character boundary and passed in whatever language it is
//!   in. A record with no abstract is screened on its title, labelled as such
//!   in the prompt, and counted in [`Screened::title_only`]; it is never
//!   excluded for lacking an abstract (Review Focus 4).
//! * The model answers `[{"id":"p-N","include":bool,"reason":"…"}]`. The reply
//!   is untrusted: ids outside the batch are ignored, the first verdict for an
//!   id wins, and a record with no verdict is excluded as "not assessed".
//!   Reasons are cleaned (whitespace collapsed, at most [`MAX_REASON`] (80)
//!   characters, never an echo of the question).
//! * A batch that fails (`Llm`, or `Parse` after the wrapper's own retry) is
//!   "not assessed" and screening goes on. `OverBudget`, or the wrap-up point
//!   (checked between batches), stops screening: what is left is "not
//!   assessed (stopped early)" and the step returns what it has. `Cancelled`
//!   and fatal errors propagate.
//! * Included records are ranked by [`EvidenceLevel`], then year (newest
//!   first, unknown last), then their original (database relevance) order.
//!
//! Privacy: nothing here logs titles, abstracts, reasons or the question;
//! only the run id, counts and fixed kinds.

use std::collections::HashMap;
use std::time::Instant;

use serde::Deserialize;

use super::frame::Pico;
use super::llm::{StepError, wrap_source};
use super::{AgentLlm, MAX_STEP_LINE, RunContext, add_line, clip, contains_question};
use crate::lit::Paper;
use crate::runs::StepKind;

/// Records per `screen` call.
pub const BATCH: usize = 10;
/// Characters of an abstract shown to the model.
pub const MAX_ABSTRACT: usize = 3_000;
/// Characters of a title shown to the model.
const MAX_TITLE: usize = 500;
/// Characters of a reason kept.
pub const MAX_REASON: usize = 80;
/// Characters of each reason in the step line.
const LINE_REASON: usize = 30;
/// Reasons shown in the step line.
const LINE_REASONS: usize = 3;
const MAX_TOKENS: u32 = 1_500;

pub const REASON_RETRACTED: &str = "retracted";
pub const REASON_NOT_ASSESSED: &str = "not assessed";
pub const REASON_STOPPED: &str = "not assessed (stopped early)";
const REASON_NONE: &str = "no reason given";
const REASON_WITHHELD: &str = "reason withheld";

// ---------- evidence level ----------

/// Strength of a study design, for ranking. Guideline, systematic review and
/// meta-analysis rank equal ([`EvidenceLevel::rank`]), above RCT, above
/// prospective cohort, above everything else.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EvidenceLevel {
    Guideline,
    SystematicReview,
    MetaAnalysis,
    Rct,
    ProspectiveCohort,
    Other,
}

const GUIDELINE_TYPES: &[&str] = &[
    "Practice Guideline",
    "Guideline",
    "Consensus Development Conference",
    "Consensus Development Conference, NIH",
];

impl EvidenceLevel {
    /// 0 for guideline / systematic review / meta-analysis, 1 RCT, 2
    /// prospective cohort, 3 other. Lower is stronger.
    pub fn rank(self) -> u8 {
        match self {
            EvidenceLevel::Guideline
            | EvidenceLevel::SystematicReview
            | EvidenceLevel::MetaAnalysis => 0,
            EvidenceLevel::Rct => 1,
            EvidenceLevel::ProspectiveCohort => 2,
            EvidenceLevel::Other => 3,
        }
    }

    /// Short label for the trail and answer.
    pub fn as_str(self) -> &'static str {
        match self {
            EvidenceLevel::Guideline => "guideline",
            EvidenceLevel::SystematicReview => "systematic review",
            EvidenceLevel::MetaAnalysis => "meta-analysis",
            EvidenceLevel::Rct => "RCT",
            EvidenceLevel::ProspectiveCohort => "prospective cohort",
            EvidenceLevel::Other => "other",
        }
    }

    /// The level of `p`, from its PubMed publication types (exact PubMed
    /// values, compared ignoring ASCII case) and its title's keywords. The
    /// strongest match wins.
    ///
    /// * Guideline: `Practice Guideline`, `Guideline`, `Consensus Development
    ///   Conference[, NIH]`; title "guideline" or "consensus".
    /// * Systematic review: `Systematic Review`; title "systematic review".
    /// * Meta-analysis: `Meta-Analysis`; title "meta-analysis" / "meta analysis".
    /// * RCT: `Randomized Controlled Trial`; title "randomized" / "randomised"
    ///   (not "non-randomized" or "quasi-randomized").
    /// * Prospective cohort: title "prospective"; or `Observational Study`
    ///   when the title does not say "retrospective".
    pub fn of(p: &Paper) -> EvidenceLevel {
        let has_type = |names: &[&str]| {
            p.pub_types
                .iter()
                .any(|t| names.iter().any(|n| t.trim().eq_ignore_ascii_case(n)))
        };
        let title = p.title.to_lowercase();
        let in_title = |words: &[&str]| words.iter().any(|w| title.contains(w));

        if has_type(GUIDELINE_TYPES) || in_title(&["guideline", "consensus"]) {
            EvidenceLevel::Guideline
        } else if has_type(&["Systematic Review"]) || in_title(&["systematic review"]) {
            EvidenceLevel::SystematicReview
        } else if has_type(&["Meta-Analysis"])
            || in_title(&["meta-analysis", "meta analysis", "metaanalysis"])
        {
            EvidenceLevel::MetaAnalysis
        } else if has_type(&["Randomized Controlled Trial"]) || title_says_randomised(&title) {
            EvidenceLevel::Rct
        } else if in_title(&["prospective"])
            || (has_type(&["Observational Study"]) && !in_title(&["retrospective"]))
        {
            EvidenceLevel::ProspectiveCohort
        } else {
            EvidenceLevel::Other
        }
    }
}

/// "randomized" / "randomised" in a lower-cased title, ignoring the negated
/// forms ("non-randomized", "nonrandomised", "quasi-randomized", …).
fn title_says_randomised(title: &str) -> bool {
    let mut t = title.to_string();
    for neg in [
        "non-random",
        "non random",
        "nonrandom",
        "quasi-random",
        "quasi random",
    ] {
        t = t.replace(neg, " ");
    }
    t.contains("randomized") || t.contains("randomised")
}

// ---------- result ----------

/// What Screen produced.
#[derive(Debug, Clone, PartialEq)]
pub struct Screened {
    /// Included records with their level and the model's (cleaned) reason,
    /// ranked strongest first.
    pub included: Vec<(Paper, EvidenceLevel, String)>,
    /// Exclusion reasons with their counts, most frequent first (ties in the
    /// order first seen).
    pub excluded_counts: Vec<(String, u32)>,
    /// Records with no abstract that were screened on their title.
    pub title_only: u32,
}

impl Screened {
    /// Total excluded.
    pub fn excluded(&self) -> u32 {
        self.excluded_counts.iter().map(|(_, n)| n).sum()
    }
}

#[derive(Default)]
struct Tally(Vec<(String, u32)>);

impl Tally {
    fn add(&mut self, reason: &str, n: u32) {
        if n == 0 {
            return;
        }
        match self.0.iter_mut().find(|(r, _)| r == reason) {
            Some((_, c)) => *c += n,
            None => self.0.push((reason.to_string(), n)),
        }
    }

    fn sorted(mut self) -> Vec<(String, u32)> {
        // Stable: equal counts keep the order first seen.
        self.0.sort_by_key(|e| std::cmp::Reverse(e.1));
        self.0
    }
}

// ---------- prompts ----------

const SYSTEM: &str = "You screen research records for a practising surgeon's clinical question, given as a PICO. \
Each record's title and abstract appear inside <source id=\"p-N\"> tags. \
Include a record if it reports clinical evidence (a study, review or guideline) that addresses the PICO's population and intervention; \
the comparison or outcomes may differ. Exclude it otherwise. \
Records may be in any language; judge them as they are. \
A record marked as having no abstract must be judged on its title alone: include it if the title suggests it is relevant, \
and never exclude it only because the abstract is missing. \
Reply with a JSON array only, one object per record, in this form: \
[{\"id\": \"p-N\", \"include\": true or false, \"reason\": \"one short phrase\"}]";

/// One model verdict. Every field is optional so one odd entry does not void
/// the batch: a missing `include` is an exclude.
#[derive(Deserialize)]
struct Verdict {
    #[serde(default)]
    id: String,
    #[serde(default)]
    include: bool,
    #[serde(default)]
    reason: String,
}

/// The source text for one record: its title, then its abstract (cut to
/// [`MAX_ABSTRACT`] characters) or a no-abstract label.
fn record_text(p: &Paper) -> (String, bool) {
    let title = clip(&p.title, MAX_TITLE);
    match p.abstract_text.as_deref().map(str::trim) {
        Some(a) if !a.is_empty() => {
            let a: String = a.chars().take(MAX_ABSTRACT).collect();
            (format!("Title: {title}\nAbstract: {a}"), false)
        }
        _ => (
            format!("Title: {title}\nAbstract: none (judge this record on its title alone)"),
            true,
        ),
    }
}

fn batch_prompt(pico: &Pico, records: &[(String, &Paper)]) -> (String, u32) {
    let pico = serde_json::to_string_pretty(pico).unwrap_or_default();
    let mut body = String::new();
    let mut title_only = 0;
    for (id, p) in records {
        let (text, no_abstract) = record_text(p);
        title_only += u32::from(no_abstract);
        body.push_str(&wrap_source(id, &text));
        body.push('\n');
    }
    let ids = records
        .iter()
        .map(|(id, _)| id.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    (
        format!(
            "PICO:\n{pico}\n\nRecords ({}):\n{body}\nGive one verdict for each of these ids: {ids}.",
            records.len()
        ),
        title_only,
    )
}

/// A model reason fit for the trail: cleaned, at most 80 characters, never
/// empty and never an echo of the question.
fn clean_reason(reason: &str, question: &str) -> String {
    let r = clip(reason, MAX_REASON);
    let q = question.trim().to_lowercase();
    if r.is_empty() {
        REASON_NONE.into()
    } else if contains_question(reason, question)
        || contains_question(&r, question)
        || (!q.is_empty() && r.to_lowercase().contains(&q))
    {
        REASON_WITHHELD.into()
    } else {
        r
    }
}

/// "Screened N: included I, excluded E (top reasons: r (c); …)", kept within
/// the step-line limit with the closing bracket intact.
fn summary_line(total: usize, included: usize, excluded: &[(String, u32)]) -> String {
    let e: u32 = excluded.iter().map(|(_, n)| n).sum();
    let head = format!("Screened {total}: included {included}, excluded {e}");
    if excluded.is_empty() {
        return head;
    }
    let reasons = excluded
        .iter()
        .take(LINE_REASONS)
        .map(|(r, n)| format!("{} ({n})", clip(r, LINE_REASON)))
        .collect::<Vec<_>>()
        .join("; ");
    let prefix = format!("{head} (top reasons: ");
    let room = MAX_STEP_LINE.saturating_sub(prefix.chars().count() + 1);
    format!("{prefix}{})", clip(&reasons, room))
}

// ---------- the step ----------

/// Screen `papers` against `pico` (see the module docs).
pub async fn screen(
    ctx: &RunContext,
    llm: &AgentLlm,
    pico: &Pico,
    papers: Vec<Paper>,
) -> Result<Screened, StepError> {
    let run_id = ctx.run_id;
    let call = llm.call(ctx, &llm.models.screen, "screen");
    let total = papers.len();

    let mut tally = Tally::default();
    let (retracted, pending): (Vec<Paper>, Vec<Paper>) =
        papers.into_iter().partition(|p| p.retracted);
    tally.add(
        REASON_RETRACTED,
        u32::try_from(retracted.len()).unwrap_or(u32::MAX),
    );

    // (original position, paper, reason) for each include.
    let mut included: Vec<(usize, Paper, String)> = Vec::new();
    let mut title_only = 0u32;
    let mut failed = 0u32;
    let mut stopped = 0u32;

    let mut start = 0;
    while start < pending.len() {
        let end = (start + BATCH).min(pending.len());
        if ctx.cancel.is_cancelled() {
            return Err(StepError::Cancelled);
        }
        if start > 0 && ctx.budget.should_wrap_up(Instant::now()) {
            tracing::info!(%run_id, step = "screen", screened = start, "wrap-up reached; screening stopped");
            stopped = u32::try_from(pending.len() - start).unwrap_or(u32::MAX);
            break;
        }
        let records: Vec<(String, &Paper)> = pending[start..end]
            .iter()
            .enumerate()
            .map(|(i, p)| (format!("p-{}", start + i + 1), p))
            .collect();
        let n = u32::try_from(records.len()).unwrap_or(u32::MAX);
        let (prompt, batch_title_only) = batch_prompt(pico, &records);

        let verdicts: Vec<Verdict> = match call.json(SYSTEM, &prompt, MAX_TOKENS).await {
            Ok(v) => v,
            Err(e @ (StepError::Llm | StepError::Parse)) => {
                tracing::warn!(%run_id, step = "screen", error_kind = e.kind(), records = n, "screening batch failed; not assessed");
                failed += n;
                start = end;
                continue;
            }
            Err(StepError::OverBudget) => {
                tracing::info!(%run_id, step = "screen", screened = start, "over budget; screening stopped");
                stopped = u32::try_from(pending.len() - start).unwrap_or(u32::MAX);
                break;
            }
            Err(e) => return Err(e),
        };
        title_only += batch_title_only;

        // First verdict per id wins; ids outside the batch are ignored.
        let mut by_id: HashMap<&str, &Verdict> = HashMap::new();
        for v in &verdicts {
            let id = v.id.trim();
            if records.iter().any(|(r, _)| r == id) {
                by_id.entry(id).or_insert(v);
            }
        }
        for (i, (id, p)) in records.iter().enumerate() {
            match by_id.get(id.as_str()) {
                Some(v) if v.include => {
                    included.push((
                        start + i,
                        (*p).clone(),
                        clean_reason(&v.reason, &ctx.question),
                    ));
                }
                Some(v) => tally.add(&clean_reason(&v.reason, &ctx.question), 1),
                None => tally.add(REASON_NOT_ASSESSED, 1),
            }
        }
        start = end;
    }
    tally.add(REASON_NOT_ASSESSED, failed);
    tally.add(REASON_STOPPED, stopped);

    // Level, then year (newest first, unknown last), then original order.
    let mut ranked: Vec<(usize, Paper, EvidenceLevel, String)> = included
        .into_iter()
        .map(|(i, p, r)| {
            let l = EvidenceLevel::of(&p);
            (i, p, l, r)
        })
        .collect();
    ranked
        .sort_by_key(|(i, p, l, _)| (l.rank(), std::cmp::Reverse(p.year.unwrap_or(i32::MIN)), *i));

    let screened = Screened {
        included: ranked.into_iter().map(|(_, p, l, r)| (p, l, r)).collect(),
        excluded_counts: tally.sorted(),
        title_only,
    };
    add_line(
        ctx,
        StepKind::Screen,
        &summary_line(total, screened.included.len(), &screened.excluded_counts),
    )
    .await;
    if title_only > 0 {
        add_line(
            ctx,
            StepKind::Note,
            &format!("{title_only} records had no abstract and were screened on title only"),
        )
        .await;
    }
    if failed > 0 {
        add_line(
            ctx,
            StepKind::Note,
            &format!("Screening failed for {failed} records; they were not assessed"),
        )
        .await;
    }
    if stopped > 0 {
        add_line(
            ctx,
            StepKind::Note,
            &format!("Screening stopped early; {stopped} records not assessed"),
        )
        .await;
    }
    Ok(screened)
}
