//! The evidence team's pipelines (Phase 3). Server only.
//!
//! * **Quick** (T5): researcher → writer → citation check → [`AnswerV3`].
//! * **Full** (Stage 3, T9): researcher (Full settings: full text, 15
//!   turns) → [extractor ∥ skeptic] → writer (from the verified evidence
//!   rows, the skeptic's review and its missed trials) → citation check →
//!   [`AnswerV3`] with its additive `evidence`, `skeptic` and `notes`.
//!
//! The worker runs it when the mode's engine (`QUICK_ENGINE`, `FULL_ENGINE`)
//! is `team` (the default; [`super::run_engine`]); the run then reserves and
//! is limited by `agent_limit_paise` and `agent_time_limit_s`.
//!
//! **Full: money with concurrency.** Before research the finish reserve is
//! the Full writer's worst case ([`writer_v3::reserve_full`]) **plus** the
//! review phase's share ([`review_reserve`]: ten extractions at the
//! extractor's largest likely hold and the skeptic's first turn with its
//! final call's kept share), so research can never use the money the review
//! phase needs to start. When research ends, the reserve drops back to the
//! writer's. The skeptic and the extractor then run at once (`tokio::join!`,
//! the skeptic polled first so its first turn's hold is taken before any
//! extraction launches). Every call of both is a gathering call: it leaves
//! the writer's reserve **and** the skeptic's kept final-call share free,
//! and with others in flight counts every in-flight hold twice (R-FB3). The
//! skeptic's affordability check counts in-flight holds twice and its final
//! call is held from its share counting them once (the deferred Stage 1
//! issue, `RunBudget::try_hold_releasing`), so it is always affordable
//! while extraction calls are in flight. A refusal beside the
//! other step's calls waits for them (`bounded::OTHERS_POLL`,
//! `agent_loop::OTHERS_POLL`) instead of ending the step. So the extractor's
//! calls, the skeptic's turns and its kept share never together take the
//! writer's reserve; global, daily and monthly caps are unchanged.
//!
//! **Full: wrap-up.** Research wraps up at the time limit less the writer's
//! and the review's allowances (the worker); the review phase at the time
//! limit less the writer's. An extractor stopped by money or time leaves the
//! studies it did not read to the writer as abstracts, labelled; a skeptic
//! that fails (anything but cancel, hard stop or unrecorded money) leaves
//! the answer without its review, with a note. The trail line says "the
//! Full limit".
//!
//! **Money (Ruling R-13c, reused).** Before the researcher starts, the
//! **finish reserve** is set to the writer's hold for the largest prompt this
//! question can produce ([`writer_v3::reserve_for`]: ten studies, every
//! field at its byte cap, priced on the writer model at its `max_tokens`).
//! Every researcher call (turns and its final call) is a gathering call and
//! leaves the finish reserve free; the loop keeps its own final call
//! affordable with its kept share (Stage 1). The writer (step `write`, a
//! finishing step) holds without leaving the reserve, and its real prompt is
//! never larger than the worst case, so whenever the researcher stops the
//! writer fits: `spent + held ≤ limit − finish_reserve` throughout research,
//! and the writer's estimate is at most the finish reserve. Why the finish
//! reserve and not a second kept share: it is the existing mechanism for
//! "what the finishing step needs" (gathering holds, the loop's own
//! affordability check and the wrap-up point all count it already), it is
//! fixed for the run (the worst case does not change as research goes on),
//! and the kept-share slot is already the loop's own for its final call;
//! a second share would only duplicate the finish reserve's bookkeeping.
//! As in Phase 2, a JSON retry is not reserved for, and the input-side
//! estimate can be exceeded by token-dense text (documented in `llm`).
//!
//! **Time.** Research wraps up at the time limit less the writer's
//! allowance (`writer_time_allowance_s`, set by the worker). The writer is
//! not started with less than [`writer_min_time`] (Quick 150 s, Full 180 s)
//! left before the hard stop: the run ends `partial` (`writer_time`) with the
//! references.
//!
//! **Outcomes** (`partial` runs with a kind store it as `error_kind`):
//! * an unpriced writer model: `failed` (`llm`) before any research;
//! * researcher errors before any completed turn end the run `failed` with
//!   their kind (`over_budget` when not even the first turn fits,
//!   `cancelled`, `hard_stop`, `unrecorded`, `llm`, `rate_limited`);
//!   a failed final study-list call after research: `partial`
//!   (`researcher_incomplete`) with "No answer could be written: the
//!   search stopped early…";
//! * nothing valid selected after a complete search: `done`, "No relevant
//!   evidence found."; after research stopped early: `partial`;
//! * the selected studies' records cannot be fetched at all: `failed`
//!   (`lit`);
//! * the writer: truncated (and its one low-effort retry refused or cut
//!   off too): `partial` (`writer_truncated`) with the references;
//!   unreadable twice: `partial` (`writer_unreadable`); refused for budget:
//!   `partial` (`write_refused`); fatal errors: `failed` with their kind;
//! * an answer written: `done`, also when research was stopped by the
//!   Quick limit or the round cap (owner ruling R-S2b: the trail says
//!   "Search stopped at the Quick limit after N rounds" / "… at the round
//!   cap …"); `partial` only when a research turn failed.
//!
//! Privacy: nothing here logs the question, queries, abstracts or the
//! answer; step lines hold counts only.

use std::time::{Duration, Instant};

use chrono::Utc;

use super::answer::{NO_EVIDENCE, STOPPED_NO_ANSWER, WRITE_REFUSED};
use super::answer_v3::{
    ANSWER_VERSION, AnswerV3, EvidenceV3, IntegrityV3, NotableFinding, ReferenceV3, SkepticItem,
    SkepticLead, SkepticOverlap, SkepticV3, Span, UnverifiedOutcomeV3,
};
use super::answer_v3::{NOTABLE_MISSED, NOTABLE_OVERLAP, NOTABLE_POOLED, unverified_counts};
use super::extractor::{self, Extraction, StudyRef};
use super::llm::StepError;
use super::pipeline::gate;
use super::researcher::{self, Research};
use super::skeptic::{self, Picture, SkepticReview};
use super::tools::lit::LitTools;
use super::writer_v3::{
    self, FullInput, StudyInput, WriteFailure, check_citations, writer_user, writer_user_full,
};
use super::{PipelineOutcome, RunContext, add_line};
use crate::lit::Paper;
use crate::runs::{RunMode, StepKind};

/// Bottom line when none of the selected studies' records could be read.
pub const UNFETCHED: &str =
    "The selected studies' records could not be fetched, so no answer could be written.";
/// Bottom line when the writer's answer was cut off by its output limit.
pub const WRITER_TRUNCATED: &str = "The answer was longer than the writer's output limit and could not be finished. The studies found are listed below; please ask again.";
/// Bottom line when the writer's reply could not be read.
pub const WRITER_UNREADABLE: &str =
    "The writer's answer could not be read. The studies found are listed below; please ask again.";
/// Bottom line when too little time was left to write.
pub const WRITER_NO_TIME: &str = "Not enough of the time limit was left to write the answer. The studies found are listed below; please ask again.";
/// The writer is not started with less time than this before the hard stop:
/// a writer that could not finish ends the run `partial` (`writer_time`) with
/// its references, instead of losing everything at the hard stop. The live
/// Full writer (31 Oct) took 109 s for about 13,500 output tokens, so a reply
/// at the 20,000-token ceiling takes about 160 s: Full keeps 180 s. A Quick
/// reply has the same ceiling but fewer, shorter inputs: Quick keeps 150 s.
pub const WRITER_MIN_TIME_QUICK: Duration = Duration::from_secs(150);
pub const WRITER_MIN_TIME_FULL: Duration = Duration::from_secs(180);

/// [`WRITER_MIN_TIME_QUICK`] or [`WRITER_MIN_TIME_FULL`] for `mode`.
pub fn writer_min_time(mode: RunMode) -> Duration {
    match mode {
        RunMode::Quick => WRITER_MIN_TIME_QUICK,
        RunMode::Focused => WRITER_MIN_TIME_FULL,
    }
}

type Stop = &'static str;

/// How the run ends with an answer body.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Status {
    Done,
    Partial,
    PartialKind(&'static str),
}

/// Run the Quick evidence team for `ctx` (see the module docs).
pub async fn run_team(ctx: &RunContext) -> PipelineOutcome {
    match run(ctx).await {
        Ok((answer, status)) => match serde_json::to_value(&answer) {
            Ok(v) => match status {
                Status::Done => PipelineOutcome::Done(v),
                Status::Partial => PipelineOutcome::Partial(v),
                Status::PartialKind(k) => PipelineOutcome::PartialKind(v, k),
            },
            Err(_) => {
                tracing::error!(run_id = %ctx.run_id, "answer could not be serialised");
                PipelineOutcome::Failed("internal")
            }
        },
        Err(kind) => {
            tracing::info!(run_id = %ctx.run_id, error_kind = kind, "team pipeline stopped");
            PipelineOutcome::Failed(kind)
        }
    }
}

fn count(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

pub(crate) fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// The answer before anything is written.
fn base(ctx: &RunContext, research: &Research, integrity: IntegrityV3) -> AnswerV3 {
    AnswerV3 {
        version: ANSWER_VERSION,
        question: ctx.question.clone(),
        mode: ctx.mode,
        created_at: Utc::now(),
        answer: None,
        empty_reason: None,
        references: Vec::new(),
        searches: research.searches.clone(),
        integrity,
        stopped_early: research.stopped_early().map(str::to_string),
        cost_paise: 0,
        elapsed_s: 0,
        skeptic: None,
        evidence: Vec::new(),
        notes: Vec::new(),
        untraced: Vec::new(),
        notable_code: Vec::new(),
        coverage: None,
        completeness: None,
    }
}

/// Cost and time, the closing line, and the log.
async fn finish(ctx: &RunContext, mut a: AnswerV3, status: Status) -> (AnswerV3, Status) {
    add_line(ctx, StepKind::Present, "Answer ready").await;
    a.cost_paise = ctx.budget.spent();
    a.elapsed_s = ctx.budget.elapsed(Instant::now()).as_secs();
    let i = a.integrity;
    tracing::info!(
        run_id = %ctx.run_id,
        written = a.answer.is_some(), references = a.references.len(),
        dropped_unseen = i.dropped_unseen, dropped_unfetched = i.dropped_unfetched,
        removed_citations = i.removed_citations, dropped_numbers = i.dropped_numbers,
        dropped_studies = i.dropped_studies, status = ?status, "team answer assembled"
    );
    (a, status)
}

/// An answer with references but nothing written, for `reason`.
async fn unwritten(
    ctx: &RunContext,
    mut answer: AnswerV3,
    message: &str,
    kind: &'static str,
) -> (AnswerV3, Status) {
    answer.empty_reason = Some(message.to_string());
    finish(ctx, answer, Status::PartialKind(kind)).await
}

async fn run(ctx: &RunContext) -> Result<(AnswerV3, Status), Stop> {
    gate(ctx).await?;
    let full = ctx.mode == RunMode::Focused;
    // The writer is reserved for before any research (see the module docs);
    // an unpriced writer could never be paid for, so nothing starts.
    let writer_reserve = if full {
        writer_v3::reserve_full(
            &ctx.llm,
            &ctx.budget,
            &ctx.question,
            ctx.settings.writer_max_tokens,
        )
    } else {
        writer_v3::reserve_for(
            &ctx.llm,
            &ctx.budget,
            &ctx.question,
            ctx.settings.writer_max_tokens,
        )
    };
    let Some(reserve) = writer_reserve else {
        tracing::error!(run_id = %ctx.run_id, model = %ctx.llm.models.writer, "no price for the writer model; run not started");
        return Err("llm");
    };
    // Full: research also leaves the review phase its share (see the module
    // docs); unpriced extractor or skeptic models fail the run up front.
    let review = if full {
        let Some(r) = review_reserve(ctx) else {
            tracing::error!(run_id = %ctx.run_id, "no price for the extractor or skeptic model; run not started");
            return Err("llm");
        };
        r
    } else {
        0
    };
    // Stage 4b: the completeness review's own share, after the writer's
    // (Full, unless COMPLETENESS=off). An unpriced model gets no share: its
    // call then fails and the review is skipped, never the run.
    let completeness_reserve = if full && ctx.state.completeness {
        super::completeness::reserve(&ctx.llm, &ctx.budget, &ctx.question).unwrap_or(0)
    } else {
        0
    };
    // What the finishing steps keep through the review phase.
    let finishing = reserve + completeness_reserve;
    ctx.budget.set_finish_reserve(finishing + review);
    tracing::info!(run_id = %ctx.run_id, writer_reserve = reserve, completeness_reserve, review_reserve = review, limit = ctx.budget.limit_paise(), "team: writer reserved");
    add_line(
        ctx,
        StepKind::Search,
        "Researcher: searching PubMed and Europe PMC",
    )
    .await;

    let research = researcher::research(ctx, &ctx.llm, ctx.lit.clone())
        .await
        .map_err(StepError::kind)?;
    let stopped = research.stopped_early();
    if let Some(at) = research.stopped_at(ctx.mode) {
        add_line(
            ctx,
            StepKind::Note,
            &format!(
                "Search stopped at {at} after {}",
                plural(research.rounds, "round", "rounds")
            ),
        )
        .await;
    } else if let Some(reason) = stopped {
        add_line(
            ctx,
            StepKind::Note,
            &format!("Research stopped early ({reason}); writing the answer from what was found"),
        )
        .await;
    }
    let mut integrity = IntegrityV3 {
        dropped_unseen: count(research.dropped_unseen),
        ..IntegrityV3::default()
    };
    let mut answer = base(ctx, &research, integrity);
    if research.final_failed {
        return Ok(unwritten(ctx, answer, STOPPED_NO_ANSWER, "researcher_incomplete").await);
    }
    if research.selected.is_empty() {
        answer.empty_reason = Some(
            if stopped.is_some() {
                STOPPED_NO_ANSWER
            } else {
                NO_EVIDENCE
            }
            .to_string(),
        );
        let status = if stopped.is_some() {
            Status::Partial
        } else {
            Status::Done
        };
        return Ok(finish(ctx, answer, status).await);
    }

    // The selected studies' records and abstracts: cache, then efetch.
    gate(ctx).await?;
    let ids: Vec<String> = research.selected.iter().map(|s| s.pmid.clone()).collect();
    let lit = LitTools::new(
        ctx.lit.clone(),
        ctx.state.pool.clone(),
        ctx.run_id,
        &ctx.question,
    );
    let (papers, _) = lit
        .fetch_papers(&ids, "writer_input")
        .await
        .map_err(|_| "lit")?;
    let mut studies: Vec<StudyInput> = Vec::new();
    let mut fetched: Vec<StudyRef> = Vec::new();
    for s in &research.selected {
        match papers.iter().find(|p| p.pmid.as_deref() == Some(&s.pmid)) {
            Some(p) => {
                fetched.push(StudyRef {
                    pmid: s.pmid.clone(),
                    paper: p.clone(),
                });
                studies.push(StudyInput {
                    pmid: s.pmid.clone(),
                    title: p.title.clone(),
                    journal: p.journal.clone(),
                    year: p.year,
                    pub_types: p.pub_types.clone(),
                    doi: p.doi.clone(),
                    retracted: p.retracted,
                    abstract_text: p.abstract_text.clone(),
                    design: s.design.as_str().to_string(),
                    why: s.why.clone(),
                })
            }
            None => integrity.dropped_unfetched += 1,
        }
    }
    if integrity.dropped_unfetched > 0 {
        add_line(
            ctx,
            StepKind::Note,
            &format!(
                "{} could not be fetched from PubMed and {} left out",
                plural(
                    integrity.dropped_unfetched as usize,
                    "selected study",
                    "selected studies"
                ),
                if integrity.dropped_unfetched == 1 {
                    "was"
                } else {
                    "were"
                }
            ),
        )
        .await;
    }
    answer.integrity = integrity;
    // References are code's, from the fetched records, in selection order.
    answer.references = studies
        .iter()
        .enumerate()
        .map(|(i, s)| ReferenceV3 {
            n: u16::try_from(i + 1).unwrap_or(u16::MAX),
            pmid: s.pmid.clone(),
            title: s.title.clone(),
            journal: s.journal.clone(),
            year: s.year,
            doi: s.doi.clone(),
            retracted: s.retracted,
            found_by_skeptic: false,
        })
        .collect();
    if studies.is_empty() {
        answer.empty_reason = Some(UNFETCHED.to_string());
        return Ok(finish(ctx, answer, Status::Partial).await);
    }

    // Full: the extractor and the skeptic, side by side.
    let mut reviewed = Reviewed::default();
    if full {
        reviewed = review_phase(ctx, &research, &fetched, &studies, finishing).await?;
        // Stage 4b item 3: the skeptic's picks join the studies (their rows,
        // or their abstracts when not extracted), referenced as found by
        // the skeptic, before its missed and quoted studies.
        let picked: Vec<String> = reviewed.picks.iter().map(|p| p.pmid.clone()).collect();
        for p in &reviewed.picks {
            answer.references.push(ReferenceV3 {
                n: u16::try_from(answer.references.len() + 1).unwrap_or(u16::MAX),
                pmid: p.pmid.clone(),
                title: p.title.clone(),
                journal: p.journal.clone(),
                year: p.year,
                doi: p.doi.clone(),
                retracted: p.retracted,
                found_by_skeptic: true,
            });
            studies.push(p.clone());
        }
        for m in reviewed.missed.iter().chain(&reviewed.cited) {
            answer.references.push(ReferenceV3 {
                n: u16::try_from(answer.references.len() + 1).unwrap_or(u16::MAX),
                pmid: m.pmid.clone(),
                title: m.title.clone(),
                journal: m.journal.clone(),
                year: m.year,
                doi: m.doi.clone(),
                retracted: m.retracted,
                found_by_skeptic: true,
            });
        }
        answer.evidence = evidence_summary(&studies, &reviewed.extraction);
        for e in &mut answer.evidence {
            e.found_by_skeptic = picked.contains(&e.pmid);
        }
        integrity.numbers_unverified = reviewed.extraction.numbers_unverified();
        integrity.outcomes_unverified = reviewed.extraction.outcomes_unverified();
        answer.integrity = integrity;
        answer.skeptic = reviewed.review.as_ref().map(skeptic_summary);
        answer.notable_code = notable_code(reviewed.review.as_ref(), &reviewed.extraction);
        answer.notes = reviewed.notes.clone();
    }

    // The writer: only with time for it before the hard stop.
    gate(ctx).await?;
    let left = ctx.budget.until_hard_stop(Instant::now());
    if left < writer_min_time(ctx.mode) {
        tracing::info!(run_id = %ctx.run_id, left_s = left.as_secs(), "too little time left for the writer");
        add_line(
            ctx,
            StepKind::Note,
            "Not enough time was left to write the answer",
        )
        .await;
        return Ok(unwritten(ctx, answer, WRITER_NO_TIME, "writer_time").await);
    }
    let with_abstract = studies
        .iter()
        .filter(|s| {
            s.abstract_text
                .as_deref()
                .is_some_and(|a| !a.trim().is_empty())
        })
        .count();
    add_line(
        ctx,
        StepKind::Write,
        &format!(
            "Writer: writing the answer from {} ({} with an abstract)",
            plural(studies.len(), "study", "studies"),
            with_abstract
        ),
    )
    .await;
    let full_input = FullInput {
        rows: &reviewed.extraction.rows,
        skeptic: reviewed.review.as_ref(),
        skeptic_note: reviewed.skeptic_note,
        missed: &reviewed.missed,
    };
    let (system, user) = if full {
        (
            writer_v3::SYSTEM_FULL,
            writer_user_full(
                &ctx.question,
                &studies,
                &full_input,
                &research.contradictions,
                &research.searches,
            ),
        )
    } else {
        (
            writer_v3::SYSTEM,
            writer_user(
                &ctx.question,
                &studies,
                &research.contradictions,
                &research.searches,
            ),
        )
    };
    let selected: Vec<String> = studies
        .iter()
        .chain(&reviewed.missed)
        .chain(&reviewed.cited)
        .map(|s| s.pmid.clone())
        .collect();
    match writer_v3::write_counted(ctx, &ctx.llm, system, &user).await {
        Ok((mut written, labels_defaulted)) => {
            integrity.labels_defaulted = labels_defaulted;
            check_citations(&mut written, &selected, &mut integrity);
            // R-S4a: the evidence label per outcome, from the cited studies'
            // designs (the researcher's labels; missed trials are RCTs).
            let designs: Vec<(String, String)> = studies
                .iter()
                .chain(&reviewed.missed)
                .map(|s| (s.pmid.clone(), s.design.clone()))
                .collect();
            writer_v3::evidence_types(&mut written, |p| {
                designs
                    .iter()
                    .find(|(id, _)| id == p)
                    .and_then(|(_, d)| writer_v3::design_class(d))
            });
            // Stage 4 item 5: mark (never change) the numbers not traced to
            // what the writer was given.
            let allowed = writer_v3::allowed_numbers(&studies, full.then_some(&full_input));
            let (untraced, checked) =
                // Review S1: Quick keeps the exact check; Full lets a number
                // written with no sign of its own match up to the sign.
                writer_v3::trace_numbers(&written, &allowed, |p| selected.iter().any(|s| s == p), full);
            integrity.numbers_checked = checked;
            integrity.numbers_untraced = count(untraced.len());
            answer.untraced = untraced;
            answer.integrity = integrity;
            // Stage 4b item 1 (Full): what the team verified that the
            // answer does not say (code; the answer is unchanged).
            if full {
                let cov = super::coverage::check(
                    &written,
                    &reviewed.extraction.rows,
                    |p| studies.iter().any(|s| s.pmid == p && s.design == "SR_MA"),
                    reviewed.review.as_ref(),
                    |p| selected.iter().any(|s| s == p),
                );
                let k = cov.counts;
                tracing::info!(
                    run_id = %ctx.run_id, numbers_checked = k.numbers_checked,
                    numbers_missing = k.numbers_missing, pooled_missing = k.pooled_missing,
                    outcomes_checked = k.outcomes_checked, outcomes_missing = k.outcomes_missing,
                    skeptic_checked = k.skeptic_checked, skeptic_missing = k.skeptic_missing,
                    "coverage checked"
                );
                answer.coverage = Some(cov);
            }
            answer.answer = Some(written);
            let line = if integrity.removed_citations == 0
                && integrity.dropped_numbers == 0
                && integrity.dropped_studies == 0
            {
                "Citation check: every citation is one of the selected studies".to_string()
            } else {
                format!(
                    "Citation check: removed {}, dropped {} and {}",
                    plural(
                        integrity.removed_citations as usize,
                        "unknown citation",
                        "unknown citations"
                    ),
                    plural(integrity.dropped_numbers as usize, "number", "numbers"),
                    plural(
                        integrity.dropped_studies as usize,
                        "trial row",
                        "trial rows"
                    )
                )
            };
            add_line(ctx, StepKind::Check, &line).await;
            let line = if integrity.numbers_untraced == 0 {
                format!(
                    "Number check: all {} traced to a verified quote, structured number or abstract",
                    plural(integrity.numbers_checked as usize, "number", "numbers")
                )
            } else {
                format!(
                    "Number check: {} of {} not traced to a verified quote, structured number or abstract (marked \"not traced\"; nothing removed)",
                    integrity.numbers_untraced,
                    plural(integrity.numbers_checked as usize, "number", "numbers")
                )
            };
            add_line(ctx, StepKind::Check, &line).await;
            if let Some(c) = &answer.coverage {
                add_line(ctx, StepKind::Check, &c.line()).await;
            }
            // Stage 4b item 2 (Full, COMPLETENESS on): the add-only review.
            // It never fails the run: anything short of cancellation or
            // unrecorded money gives a skipped review and the answer ships
            // as the writer wrote it (stored unchanged either way).
            if full && ctx.state.completeness {
                let cov = answer.coverage.clone().unwrap_or_default();
                let gaps_found = count(cov.gaps.len()).saturating_add(cov.counts.gaps_cut);
                add_line(
                    ctx,
                    StepKind::Check,
                    &format!(
                        "Completeness review: checking the answer against {}",
                        plural(gaps_found as usize, "gap", "gaps")
                    ),
                )
                .await;
                let user = super::completeness::user(
                    &ctx.question,
                    &answer.plain_text(),
                    &cov,
                    &writer_v3::full_blocks(&studies, &full_input),
                );
                let outcomes = answer
                    .answer
                    .as_ref()
                    .map(|w| w.outcomes.clone())
                    .unwrap_or_default();
                let cx = super::completeness::Context {
                    selected: &selected,
                    allowed: &allowed,
                    outcomes: &outcomes,
                    gaps_found,
                };
                let c = super::completeness::review(ctx, &ctx.llm, &user, &cx)
                    .await
                    .map_err(StepError::kind)?;
                add_line(ctx, StepKind::Check, &c.line()).await;
                answer.completeness = Some(c);
            }
            // R-S2b: an answer after research stopped by a limit is done; a
            // failed research turn still marks it partial.
            let status = if research.stop == super::agent_loop::LoopStop::TurnFailed {
                Status::Partial
            } else {
                Status::Done
            };
            Ok(finish(ctx, answer, status).await)
        }
        Err(WriteFailure::Truncated) => {
            add_line(
                ctx,
                StepKind::Note,
                "The writer's answer was cut off by its output limit",
            )
            .await;
            Ok(unwritten(ctx, answer, WRITER_TRUNCATED, "writer_truncated").await)
        }
        Err(WriteFailure::Unreadable) => {
            add_line(ctx, StepKind::Note, "The writer's answer could not be read").await;
            Ok(unwritten(ctx, answer, WRITER_UNREADABLE, "writer_unreadable").await)
        }
        Err(WriteFailure::Step(StepError::OverBudget)) => {
            // The finish reserve covers the writer's estimate; this means
            // earlier calls cost more than they were held for (input
            // tokens are estimated) and used up the headroom.
            tracing::warn!(run_id = %ctx.run_id, writer_reserve = reserve, spent = ctx.budget.spent(), "writer refused: over budget (earlier calls cost more than their estimates)");
            add_line(
                ctx,
                StepKind::Note,
                "Not enough of the cost limit was left to write the answer",
            )
            .await;
            Ok(unwritten(ctx, answer, WRITE_REFUSED, "write_refused").await)
        }
        Err(WriteFailure::Step(e)) => Err(e.kind()),
    }
}

// ---------- Full: the review phase (Stage 3) ----------

/// Studies quoted by the skeptic (contradictions, guidelines) given
/// references, at most (Stage 3 review 3).
pub const MAX_SKEPTIC_REFS: usize = 5;

/// Bottom-line note when the skeptic's review failed.
pub const SKEPTIC_FAILED: &str =
    "The skeptic's review could not be completed, so this answer was written without it.";

/// What the review phase produced.
#[derive(Default)]
struct Reviewed {
    extraction: Extraction,
    review: Option<SkepticReview>,
    skeptic_note: Option<&'static str>,
    /// The skeptic's missed RCTs, fetched.
    missed: Vec<StudyInput>,
    /// The records of the other studies the skeptic names (contradicting
    /// evidence, guidelines, overlaps), fetched for their references (at
    /// most [`MAX_SKEPTIC_REFS`]). One whose fetch fails has no reference;
    /// the page then links it to PubMed directly.
    cited: Vec<StudyInput>,
    /// Stage 4b item 3: the skeptic's picks, fetched (extracted in the
    /// second wave when time and money allowed; their rows are in
    /// `extraction`). They join the studies given to the writer.
    picks: Vec<StudyInput>,
    notes: Vec<String>,
}

/// Stage 4b item 3: the second extraction wave is not started with less
/// than this before the review phase's wrap-up point (one extraction at
/// concurrency 3, about 20–40 s, with its one correction).
pub const WAVE2_MIN_TIME: Duration = Duration::from_secs(45);

/// What research leaves free for the review phase in a Full run: every
/// selected study (at most twelve, Stage 4b) and the skeptic's picks (at
/// most three, the second wave) extracted once at the extractor's largest
/// likely hold, plus what the skeptic needs to start (its first turn and its
/// final call's kept share). Corrections and JSON retries are not reserved
/// for, as in Phase 2. `None`: an unpriced extractor or skeptic model.
pub fn review_reserve(ctx: &RunContext) -> Option<i64> {
    let per_study = extractor::typical_max_hold(
        &ctx.llm,
        &ctx.budget,
        &ctx.question,
        ctx.settings.extractor_max_tokens,
    )?;
    let skeptic = skeptic::min_need(ctx, &ctx.llm, ctx.lit.clone())?;
    let n = i64::try_from(super::researcher::MAX_SELECTED_FULL).unwrap_or(12);
    Some(
        per_study
            .saturating_mul(n)
            .saturating_add(skeptic)
            .saturating_add(pick_reserve(ctx)?),
    )
}

/// Stage 4b item 3: the second wave's share (the skeptic's picks, each at
/// the extractor's largest likely hold), kept free while the first wave and
/// the skeptic run. `None`: an unpriced extractor model.
pub fn pick_reserve(ctx: &RunContext) -> Option<i64> {
    let per_study = extractor::typical_max_hold(
        &ctx.llm,
        &ctx.budget,
        &ctx.question,
        ctx.settings.extractor_max_tokens,
    )?;
    Some(per_study.saturating_mul(i64::try_from(skeptic::MAX_PICKS).unwrap_or(3)))
}

/// The extractor and the skeptic, concurrently (see the module docs).
async fn review_phase(
    ctx: &RunContext,
    research: &Research,
    fetched: &[StudyRef],
    studies: &[StudyInput],
    finishing: i64,
) -> Result<Reviewed, Stop> {
    gate(ctx).await?;
    // Research is over: the review phase may use what research had to
    // leave for it; only the writer (and, Stage 4b, the completeness
    // review) stays reserved. Its wrap-up point is the time limit less the
    // writer's allowance.
    // Stage 4b: the second wave's share stays free until the skeptic has
    // picked (it is released when the second wave starts or is skipped).
    ctx.budget
        .set_finish_reserve(finishing.saturating_add(pick_reserve(ctx).unwrap_or(0)));
    let s = ctx.settings;
    ctx.budget.set_wrap_up_after(Duration::from_secs(u64::from(
        s.agent_time_limit_s
            .saturating_sub(s.writer_time_allowance_s),
    )));
    add_line(
        ctx,
        StepKind::Extract,
        &format!(
            "Extractor and skeptic: extracting {} and challenging the conclusion",
            plural(fetched.len(), "study", "studies")
        ),
    )
    .await;
    let picture: Vec<Picture> = studies
        .iter()
        .map(|s| Picture {
            pmid: s.pmid.clone(),
            title: s.title.clone(),
            design: s.design.clone(),
            why: s.why.clone(),
        })
        .collect();
    // The skeptic first: its first turn's hold is taken on the first poll,
    // before the extractor launches anything (see the module docs).
    let skeptic_fut = skeptic::review(
        ctx,
        &ctx.llm,
        ctx.lit.clone(),
        &picture,
        &research.contradictions,
    );
    let extract_fut = async {
        let texts = extractor::fetch_full_texts(ctx, &ctx.lit, fetched).await?;
        extractor::extract_rows(ctx, &ctx.llm, fetched, &texts).await
    };
    let (sk, ex) = tokio::join!(skeptic_fut, extract_fut);
    let extraction = ex.map_err(StepError::kind)?;
    let mut out = Reviewed {
        extraction,
        ..Reviewed::default()
    };
    match sk {
        Ok(r) => out.review = Some(r),
        Err(e @ (StepError::Cancelled | StepError::Unrecorded | StepError::HardStop)) => {
            return Err(e.kind());
        }
        Err(e) => {
            tracing::warn!(run_id = %ctx.run_id, error_kind = e.kind(), "skeptic failed; writing without it");
            add_line(
                ctx,
                StepKind::Note,
                "Skeptic: the review could not be completed; the answer is written without it",
            )
            .await;
            out.skeptic_note = Some(SKEPTIC_FAILED);
            out.notes.push(SKEPTIC_FAILED.to_string());
        }
    }
    let x = &out.extraction;
    tracing::info!(
        run_id = %ctx.run_id, rows = x.rows.len(), not_extracted = x.not_extracted.len(),
        stopped = x.stopped, failed = x.failed, "review phase done"
    );
    if x.stopped > 0 {
        let line = format!(
            "Extraction stopped at the Full limit; {} the abstract only",
            if x.stopped == 1 {
                "1 study uses".to_string()
            } else {
                format!("{} studies use", x.stopped)
            }
        );
        add_line(ctx, StepKind::Note, &line).await;
        out.notes.push(format!("{line}, labelled as such."));
    }
    if x.failed > 0 {
        let line = format!(
            "Extraction failed for {}; the abstract is used instead",
            plural(x.failed, "study", "studies")
        );
        add_line(ctx, StepKind::Note, &line).await;
        out.notes.push(format!("{line}."));
    }
    // Stage 4b item 3: the skeptic's picks, extracted in a second wave.
    second_wave(ctx, studies, finishing, &mut out).await?;
    // The skeptic's missed RCTs: records and abstracts, cited and
    // referenced, never extracted (a pick that joined the studies is not
    // repeated here).
    let missed_ids: Vec<String> = out
        .review
        .as_ref()
        .map(|r| {
            r.missed_rcts
                .iter()
                .map(|m| m.pmid.clone())
                .filter(|p| !out.picks.iter().any(|x| &x.pmid == p))
                .collect()
        })
        .unwrap_or_default();
    // The studies the skeptic quotes, so its citations of them have
    // references (review 3): not selected, not missed, at most five.
    let mut cited_ids: Vec<String> = Vec::new();
    if let Some(r) = &out.review {
        // Overlap PMIDs too (re-review 3), under the same cap.
        let quoted = r
            .contradicting
            .iter()
            .map(|c| &c.pmid)
            .chain(r.guidelines.iter().map(|g| &g.pmid))
            .chain(r.overlaps.iter().flat_map(|o| &o.pmids))
            .chain(r.ma_notes.iter().map(|m| &m.pmid));
        for p in quoted {
            if !studies.iter().any(|s| &s.pmid == p)
                && !out.picks.iter().any(|s| &s.pmid == p)
                && !missed_ids.contains(p)
                && !cited_ids.contains(p)
                && cited_ids.len() < MAX_SKEPTIC_REFS
            {
                cited_ids.push(p.clone());
            }
        }
    }
    let wanted: Vec<String> = missed_ids.iter().chain(&cited_ids).cloned().collect();
    if !wanted.is_empty() {
        gate(ctx).await?;
        let lit = LitTools::new(
            ctx.lit.clone(),
            ctx.state.pool.clone(),
            ctx.run_id,
            &ctx.question,
        );
        match lit.fetch_papers(&wanted, "skeptic_missed").await {
            Ok((papers, _)) => {
                let why = |id: &str| {
                    out.review
                        .as_ref()
                        .and_then(|r| r.missed_rcts.iter().find(|m| m.pmid == id))
                        .map(|m| m.why.clone())
                        .unwrap_or_default()
                };
                for id in &missed_ids {
                    if let Some(p) = papers.iter().find(|p| p.pmid.as_deref() == Some(id)) {
                        out.missed.push(missed_input(id, p, why(id)));
                    }
                }
                for id in &cited_ids {
                    if let Some(p) = papers.iter().find(|p| p.pmid.as_deref() == Some(id)) {
                        let mut c = missed_input(id, p, String::new());
                        c.design = "quoted by the skeptic".into();
                        out.cited.push(c);
                    }
                }
            }
            Err(_) => {
                tracing::warn!(run_id = %ctx.run_id, "skeptic's missed trials could not be fetched");
            }
        }
    }
    Ok(out)
}

/// The design label of a study the skeptic picked, from its publication
/// types (the researcher's labels: `RCT`, `SR_MA`, `other`).
pub fn pick_design(pub_types: &[String]) -> &'static str {
    let has = |w: &str| pub_types.iter().any(|t| t.to_lowercase().contains(w));
    if has("meta-analysis") || has("systematic review") {
        "SR_MA"
    } else if has("randomized controlled trial") || has("randomised controlled trial") {
        "RCT"
    } else {
        "other"
    }
}

/// Review B1: kept free before the writer's start gate when the second wave
/// is bounded.
pub const WAVE2_MARGIN: Duration = Duration::from_secs(15);

/// How long the second wave may run from `now`: until the writer's start
/// gate (the hard stop less [`WRITER_MIN_TIME_FULL`]) less
/// [`WAVE2_MARGIN`]; zero when that point has passed. With the defaults the
/// wave ends by 465 s.
pub fn wave2_time(ctx: &RunContext, now: Instant) -> Duration {
    ctx.budget
        .until_hard_stop(now)
        .saturating_sub(WRITER_MIN_TIME_FULL)
        .saturating_sub(WAVE2_MARGIN)
}

/// The note when the second wave did not extract the picks.
fn picks_note(n: usize, why: &str) -> String {
    format!(
        "{} picked by the skeptic {} not extracted ({why}); the writer was given {} abstract",
        plural(n, "study", "studies"),
        if n == 1 { "was" } else { "were" },
        if n == 1 { "its" } else { "their" }
    )
}

/// Stage 4b item 3: the second extraction wave. The skeptic's picks (its
/// own results, not selected, at most three) are fetched and, when time and
/// money allow, go through the same extractor, the same verification and
/// the same one correction, at the same concurrency. Otherwise (or for a
/// pick whose extraction fails) the writer gets the abstract, with a note.
/// Only cancellation and fatal errors are errors, as in the first wave.
async fn second_wave(
    ctx: &RunContext,
    studies: &[StudyInput],
    finishing: i64,
    out: &mut Reviewed,
) -> Result<(), Stop> {
    // The writer and the completeness review keep their share; the second
    // wave may use the rest. Released first, also when the skeptic failed
    // (review minor 1).
    ctx.budget.set_finish_reserve(finishing);
    let Some(r) = out.review.as_ref() else {
        return Ok(());
    };
    let picks: Vec<(String, String)> = r
        .extract_next
        .iter()
        .filter(|p| !studies.iter().any(|s| s.pmid == p.pmid))
        .map(|p| (p.pmid.clone(), p.why.clone()))
        .collect();
    if picks.is_empty() {
        return Ok(());
    }
    gate(ctx).await?;
    let lit = LitTools::new(
        ctx.lit.clone(),
        ctx.state.pool.clone(),
        ctx.run_id,
        &ctx.question,
    );
    let ids: Vec<String> = picks.iter().map(|(p, _)| p.clone()).collect();
    let papers = match lit.fetch_papers(&ids, "skeptic_picks").await {
        Ok((papers, _)) => papers,
        Err(_) => {
            tracing::warn!(run_id = %ctx.run_id, "skeptic's picks could not be fetched");
            let line = format!(
                "{} picked by the skeptic could not be fetched",
                plural(ids.len(), "study", "studies")
            );
            add_line(ctx, StepKind::Note, &line).await;
            out.notes.push(format!("{line}."));
            return Ok(());
        }
    };
    let mut refs: Vec<StudyRef> = Vec::new();
    for (id, why) in &picks {
        if let Some(p) = papers.iter().find(|p| p.pmid.as_deref() == Some(id)) {
            refs.push(StudyRef {
                pmid: id.clone(),
                paper: p.clone(),
            });
            out.picks.push(StudyInput {
                pmid: id.clone(),
                title: p.title.clone(),
                journal: p.journal.clone(),
                year: p.year,
                pub_types: p.pub_types.clone(),
                doi: p.doi.clone(),
                retracted: p.retracted,
                abstract_text: p.abstract_text.clone(),
                design: pick_design(&p.pub_types).to_string(),
                why: format!("Found by the skeptic: {why}"),
            });
        }
    }
    // Review S4: a pick the fetch did not return is said, never dropped
    // silently.
    let missing = picks.len() - refs.len();
    if missing > 0 {
        let line = format!(
            "{} picked by the skeptic could not be fetched",
            plural(missing, "study", "studies")
        );
        add_line(ctx, StepKind::Note, &line).await;
        out.notes.push(format!("{line}."));
    }
    if refs.is_empty() {
        return Ok(());
    }
    // Time and money for the second wave; otherwise the writer gets the
    // abstracts, labelled. Review B1: the whole wave must end before the
    // writer's start gate, less a margin.
    let now = Instant::now();
    let deadline = wave2_time(ctx, now);
    let per_study = extractor::typical_max_hold(
        &ctx.llm,
        &ctx.budget,
        &ctx.question,
        ctx.settings.extractor_max_tokens,
    )
    .unwrap_or(i64::MAX);
    let k = ctx.budget.committed();
    let need = i128::from(k.spent)
        + i128::from(k.held)
        + i128::from(k.finish_reserve)
        + i128::from(k.in_flight_reserve)
        + i128::from(per_study);
    let skip = if ctx.budget.until_wrap_up(now) < WAVE2_MIN_TIME || deadline.is_zero() {
        Some("time")
    } else if need > i128::from(ctx.budget.limit_paise()) || ctx.budget.should_wrap_up(now) {
        Some("cost limit")
    } else {
        None
    };
    if let Some(why) = skip {
        tracing::info!(run_id = %ctx.run_id, picks = refs.len(), reason = why, "second extraction wave skipped");
        let line = picks_note(refs.len(), why);
        add_line(ctx, StepKind::Note, &line).await;
        out.notes.push(format!("{line}."));
        out.extraction
            .not_extracted
            .extend(refs.iter().map(|r| r.pmid.clone()));
        return Ok(());
    }
    add_line(
        ctx,
        StepKind::Extract,
        &format!(
            "Extractor: second wave, extracting {} found by the skeptic",
            plural(refs.len(), "study", "studies")
        ),
    )
    .await;
    let wave = async {
        let texts = extractor::fetch_full_texts(ctx, &ctx.lit, &refs).await?;
        extractor::extract_rows(ctx, &ctx.llm, &refs, &texts).await
    };
    let x = match tokio::time::timeout(deadline, wave).await {
        Ok(x) => x.map_err(StepError::kind)?,
        Err(_) => {
            // Calls already made are detached: their charges are still
            // booked and their holds stay counted until they settle.
            tracing::info!(run_id = %ctx.run_id, picks = refs.len(), "second extraction wave timed out");
            let line = picks_note(refs.len(), "time");
            add_line(ctx, StepKind::Note, &line).await;
            out.notes.push(format!("{line}."));
            out.extraction
                .not_extracted
                .extend(refs.iter().map(|r| r.pmid.clone()));
            return Ok(());
        }
    };
    tracing::info!(
        run_id = %ctx.run_id, rows = x.rows.len(), stopped = x.stopped, failed = x.failed,
        "second extraction wave done"
    );
    if x.stopped > 0 {
        let line = picks_note(x.stopped, "the Full limit");
        add_line(ctx, StepKind::Note, &line).await;
        out.notes.push(format!("{line}."));
    }
    if x.failed > 0 {
        let line = picks_note(x.failed, "extraction failed");
        add_line(ctx, StepKind::Note, &line).await;
        out.notes.push(format!("{line}."));
    }
    out.extraction.rows.extend(x.rows);
    out.extraction.not_extracted.extend(x.not_extracted);
    out.extraction.stopped += x.stopped;
    out.extraction.failed += x.failed;
    Ok(())
}

fn missed_input(pmid: &str, p: &Paper, why: String) -> StudyInput {
    StudyInput {
        pmid: pmid.to_string(),
        title: p.title.clone(),
        journal: p.journal.clone(),
        year: p.year,
        pub_types: p.pub_types.clone(),
        doi: p.doi.clone(),
        retracted: p.retracted,
        abstract_text: p.abstract_text.clone(),
        design: "found by the skeptic; not extracted".into(),
        why,
    }
}

/// Per selected study, what the extractor did (for the page).
pub fn evidence_summary(studies: &[StudyInput], x: &Extraction) -> Vec<EvidenceV3> {
    studies
        .iter()
        .map(|s| match x.rows.iter().find(|r| r.pmid == s.pmid) {
            Some(r) => EvidenceV3 {
                pmid: s.pmid.clone(),
                text: r.text.as_str().to_string(),
                verified_quotes: r.outcomes_verified,
                numbers_dropped: 0,
                unverified_outcomes: r.unverified_outcomes.clone(),
                numbers_unverified: r.numbers_unverified,
                unverified_details: r
                    .outcomes
                    .iter()
                    .filter(|o| !o.verified)
                    .map(|o| UnverifiedOutcomeV3 {
                        outcome: o.outcome.clone(),
                        effect_text: o.effect_text.clone(),
                    })
                    .collect(),
                funding: r.funding.clone(),
                funding_checked: true,
                trial_registration_unverified: r.trial_registration_unverified.clone(),
                found_by_skeptic: false,
                spans: r
                    .spans
                    .iter()
                    .filter(|s| s.field == "funding")
                    .cloned()
                    .collect(),
            },
            None => EvidenceV3 {
                pmid: s.pmid.clone(),
                text: "not extracted".to_string(),
                verified_quotes: 0,
                numbers_dropped: 0,
                unverified_outcomes: Vec::new(),
                numbers_unverified: 0,
                unverified_details: Vec::new(),
                funding: None,
                funding_checked: false,
                spans: Vec::new(),
                trial_registration_unverified: None,
                found_by_skeptic: false,
            },
        })
        .collect()
}

/// "Other notable findings from this run", the part built by code (Stage 4
/// item 12; no AI call, no search): non-independent trials, trials the
/// search missed and what meta-analyses pooled (from the skeptic, its text
/// unchanged, its unverified numbers still marked), and how many extracted
/// outcomes and numbers could not be verified. Second-hand appraisal claims
/// are not structured in the evidence rows, so they are not listed here.
pub fn notable_code(review: Option<&SkepticReview>, x: &Extraction) -> Vec<NotableFinding> {
    let mut out = Vec::new();
    let item = |prefix: &str, text: &str, field: &str, pmids: Vec<String>, spans: &[Span]| {
        let shift = prefix.len();
        NotableFinding {
            text: format!("{prefix}{text}"),
            pmids,
            spans: spans
                .iter()
                .filter(|s| s.field == field)
                .map(|s| Span {
                    field: "text".into(),
                    start: s
                        .start
                        .saturating_add(u32::try_from(shift).unwrap_or(u32::MAX)),
                    len: s.len,
                })
                .collect(),
        }
    };
    if let Some(r) = review {
        for (i, o) in r.overlaps.iter().enumerate() {
            out.push(item(
                NOTABLE_OVERLAP,
                &o.note,
                &format!("overlaps.{i}.note"),
                o.pmids.clone(),
                &r.spans,
            ));
        }
        for (i, m) in r.missed_rcts.iter().enumerate() {
            out.push(item(
                NOTABLE_MISSED,
                &m.why,
                &format!("missed_rcts.{i}.text"),
                vec![m.pmid.clone()],
                &r.spans,
            ));
        }
        for (i, m) in r.ma_notes.iter().enumerate() {
            out.push(item(
                NOTABLE_POOLED,
                &m.note,
                &format!("ma_notes.{i}.text"),
                vec![m.pmid.clone()],
                &r.spans,
            ));
        }
    }
    let outcomes = x.outcomes_unverified();
    let numbers = x.numbers_unverified();
    if outcomes + numbers > 0 {
        let pmids: Vec<String> = x
            .rows
            .iter()
            .filter(|r| r.numbers_unverified > 0 || !r.unverified_outcomes.is_empty())
            .map(|r| r.pmid.clone())
            .collect();
        out.push(NotableFinding {
            // Stage 4b item 5: no "0 outcomes and …" part.
            text: format!(
                "The extractor could not match {} to the papers' wording; they are kept and labelled \u{201c}not verified\u{201d} in the trials table.",
                unverified_counts(outcomes, numbers).unwrap_or_default()
            ),
            pmids,
            spans: Vec::new(),
        });
    }
    out
}

/// The skeptic's review as the page shows it.
pub fn skeptic_summary(r: &SkepticReview) -> SkepticV3 {
    SkepticV3 {
        verdict: r.verdict.as_str().to_string(),
        must_mention: r.must_mention.clone(),
        contradicting: r
            .contradicting
            .iter()
            .map(|c| SkepticItem {
                pmid: c.pmid.clone(),
                text: c.claim.clone(),
                quote: Some(c.quote.clone()),
            })
            .collect(),
        missed_rcts: r
            .missed_rcts
            .iter()
            .map(|m| SkepticItem {
                pmid: m.pmid.clone(),
                text: m.why.clone(),
                quote: None,
            })
            .collect(),
        guidelines: r
            .guidelines
            .iter()
            .map(|g| SkepticItem {
                pmid: g.pmid.clone(),
                text: g.body.clone(),
                quote: Some(g.statement_quote.clone()),
            })
            .collect(),
        overlaps: r
            .overlaps
            .iter()
            .map(|o| SkepticOverlap {
                pmids: o.pmids.clone(),
                note: o.note.clone(),
            })
            .collect(),
        ma_notes: r
            .ma_notes
            .iter()
            .map(|m| SkepticItem {
                pmid: m.pmid.clone(),
                text: m.note.clone(),
                quote: None,
            })
            .collect(),
        // The paths are the page's (the review's spans use them).
        spans: r.spans.clone(),
        leads: r
            .leads
            .iter()
            .map(|l| SkepticLead {
                pmid: l.pmid.clone(),
                kind: l.kind.clone(),
            })
            .collect(),
        dropped_unseen: r.dropped_unseen,
        dropped_quotes: r.dropped_quotes,
        extract_next: r
            .extract_next
            .iter()
            .map(|p| SkepticItem {
                pmid: p.pmid.clone(),
                text: p.why.clone(),
                quote: None,
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::extractor::EvidenceRow;
    use crate::agent::skeptic::{MaNote, MissedRct, Overlap, Verdict};

    fn row(pmid: &str, unverified: &[&str], numbers: u32) -> EvidenceRow {
        serde_json::from_value(serde_json::json!({
            "pmid": pmid, "text": "abstract", "design": "RCT", "n_randomised": null,
            "n_analysed": null, "setting": "", "centres": "single", "country": "", "population": "",
            "intervention": "", "comparator": "", "follow_up": "", "blinding": "",
            "primary_endpoint": "", "powered_for_primary": null, "surgeon_experience": "",
            "outcomes": [], "appraisal": {"bias": "", "power": "", "learning_curve": "", "applicability": ""},
            "independence": {"trial_registration": null, "group_or_centre": "", "note": ""},
            "unverified_outcomes": unverified, "outcomes_verified": 0,
            "numbers_unverified": numbers, "corrections": 0
        }))
        .unwrap()
    }

    /// Stage 4 item 12: the code-built notable findings come from the
    /// skeptic's overlaps, missed trials and meta-analysis notes (text
    /// unchanged, its unverified numbers still marked) and the extractor's
    /// unverified counts. No AI call; nothing when there is nothing.
    #[test]
    fn notable_findings_are_built_by_code_from_the_checks() {
        let review = SkepticReview {
            verdict: Verdict::HoldsWithCaveats,
            must_mention: vec![],
            contradicting: vec![],
            missed_rcts: vec![MissedRct {
                pmid: "333".into(),
                why: "a 2026 trial".into(),
            }],
            guidelines: vec![],
            overlaps: vec![Overlap {
                pmids: vec!["111".into(), "222".into()],
                note: "DIRECT and ROLAIS share one centre (Aabenraa).".into(),
            }],
            ma_notes: vec![MaNote {
                pmid: "444".into(),
                note: "Its 7-RCT pool counts an observational study.".into(),
            }],
            extract_next: vec![],
            searches: vec![],
            leads: vec![],
            dropped_unseen: 0,
            dropped_quotes: 0,
            numbers_flagged: 1,
            spans: vec![Span::new("missed_rcts.0.text", 2, 6)],
        };
        let x = Extraction {
            rows: vec![row("111", &["Recurrence"], 3), row("222", &[], 0)],
            ..Extraction::default()
        };
        let items = notable_code(Some(&review), &x);
        let texts: Vec<&str> = items.iter().map(|n| n.text.as_str()).collect();
        assert_eq!(
            texts,
            [
                "Not independent evidence (the skeptic): DIRECT and ROLAIS share one centre (Aabenraa).",
                "A randomised trial the researcher's search missed (the skeptic): a 2026 trial",
                "What a meta-analysis pooled (the skeptic): Its 7-RCT pool counts an observational study.",
                "The extractor could not match 1 outcome and 3 numbers to the papers' wording; they are kept and labelled \u{201c}not verified\u{201d} in the trials table.",
            ]
        );
        assert_eq!(items[0].pmids, ["111", "222"]);
        assert_eq!(items[3].pmids, ["111"]);
        // The skeptic's mark moves with its text.
        let s = &items[1].spans[0];
        assert_eq!(
            &items[1].text[s.start as usize..(s.start + s.len) as usize],
            "2026"
        );
        assert!(notable_code(None, &Extraction::default()).is_empty());
        // Stage 4b item 5: no "0 outcomes and" part.
        let x = Extraction {
            rows: vec![row("111", &[], 5)],
            ..Extraction::default()
        };
        assert_eq!(
            notable_code(None, &x)[0].text,
            "The extractor could not match 5 numbers to the papers' wording; they are kept and labelled \u{201c}not verified\u{201d} in the trials table."
        );
        // The skeptic's items are recognised as repeats of its box.
        assert!(items[..3].iter().all(NotableFinding::repeats_skeptic));
        assert!(!items[3].repeats_skeptic());
    }
}
