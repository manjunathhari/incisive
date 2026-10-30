//! The evidence team's Quick pipeline (Phase 3, T5): researcher → writer →
//! citation check → [`AnswerV3`]. Server only.
//!
//! The worker runs it for Quick runs when `QUICK_ENGINE` is `team` (the
//! default; [`super::run_engine`]); the run then reserves and is limited by
//! `agent_limit_paise` and `agent_time_limit_s` (`RunsConfig::limits`).
//! Focused runs keep the Phase 2 pipeline.
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
//! not started with less than [`WRITER_MIN_TIME`] left before the hard
//! stop: the run ends `partial` (`writer_time`) with the references.
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
use super::answer_v3::{ANSWER_VERSION, AnswerV3, IntegrityV3, ReferenceV3};
use super::llm::StepError;
use super::pipeline::gate;
use super::researcher::{self, Research};
use super::tools::lit::LitTools;
use super::writer_v3::{self, StudyInput, WriteFailure, check_citations, writer_user};
use super::{PipelineOutcome, RunContext, add_line};
use crate::runs::StepKind;

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
/// The writer is not started with less time than this before the hard stop.
// 120 s: the live smoke (30 Oct) had the Opus writer take 44 s for 3 studies
// (4,768 output tokens); 10 studies is roughly double.
pub const WRITER_MIN_TIME: Duration = Duration::from_secs(120);

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

fn plural(n: usize, one: &str, many: &str) -> String {
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
    // The writer is reserved for before any research (see the module docs);
    // an unpriced writer could never be paid for, so nothing starts.
    let Some(reserve) = writer_v3::reserve_for(
        &ctx.llm,
        &ctx.budget,
        &ctx.question,
        ctx.settings.writer_max_tokens,
    ) else {
        tracing::error!(run_id = %ctx.run_id, model = %ctx.llm.models.writer, "no price for the writer model; run not started");
        return Err("llm");
    };
    ctx.budget.set_finish_reserve(reserve);
    tracing::info!(run_id = %ctx.run_id, writer_reserve = reserve, limit = ctx.budget.limit_paise(), "team: writer reserved");
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
    for s in &research.selected {
        match papers.iter().find(|p| p.pmid.as_deref() == Some(&s.pmid)) {
            Some(p) => studies.push(StudyInput {
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
            }),
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
        })
        .collect();
    if studies.is_empty() {
        answer.empty_reason = Some(UNFETCHED.to_string());
        return Ok(finish(ctx, answer, Status::Partial).await);
    }

    // The writer: only with time for it before the hard stop.
    gate(ctx).await?;
    let left = ctx.budget.until_hard_stop(Instant::now());
    if left < WRITER_MIN_TIME {
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
    let user = writer_user(
        &ctx.question,
        &studies,
        &research.contradictions,
        &research.searches,
    );
    let selected: Vec<String> = studies.iter().map(|s| s.pmid.clone()).collect();
    match writer_v3::write(ctx, &ctx.llm, &user).await {
        Ok(mut written) => {
            check_citations(&mut written, &selected, &mut integrity);
            answer.integrity = integrity;
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
