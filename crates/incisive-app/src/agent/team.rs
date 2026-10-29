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
//! **Outcomes** (Ruling R-13a's two "nothing found" cases kept):
//! * researcher errors end the run `failed` with their kind (`over_budget`
//!   when not even the first turn fits, `cancelled`, `hard_stop`,
//!   `unrecorded`, `llm`, `parse`, `rate_limited`);
//! * nothing valid selected after a complete search: `done`, "No relevant
//!   evidence found."; after research stopped early: `partial`;
//! * the selected studies' records cannot be fetched at all: `failed`
//!   (`lit`);
//! * the writer refused for budget (never expected, see above): `partial`
//!   with the references and "The cost limit was reached before the answer
//!   could be written."; other writer errors: `failed` with their kind;
//! * otherwise `done`, or `partial` when research stopped early.
//!
//! Privacy: nothing here logs the question, queries, abstracts or the
//! answer; step lines hold counts only.

use std::time::Instant;

use chrono::Utc;

use super::answer::{NO_EVIDENCE, STOPPED_NO_ANSWER, WRITE_REFUSED};
use super::answer_v3::{ANSWER_VERSION, AnswerV3, IntegrityV3, ReferenceV3};
use super::llm::StepError;
use super::pipeline::gate;
use super::researcher::{self, Research};
use super::tools::lit::LitTools;
use super::writer_v3::{self, StudyInput, check_citations, writer_user};
use super::{PipelineOutcome, RunContext, add_line};
use crate::runs::StepKind;

/// Bottom line when none of the selected studies' records could be read.
pub const UNFETCHED: &str =
    "The selected studies' records could not be fetched, so no answer could be written.";

type Stop = &'static str;

/// Run the Quick evidence team for `ctx` (see the module docs).
pub async fn run_team(ctx: &RunContext) -> PipelineOutcome {
    match run(ctx).await {
        Ok((answer, degraded)) => match serde_json::to_value(&answer) {
            Ok(v) if degraded => PipelineOutcome::Partial(v),
            Ok(v) => PipelineOutcome::Done(v),
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
async fn finish(ctx: &RunContext, mut a: AnswerV3, degraded: bool) -> (AnswerV3, bool) {
    add_line(ctx, StepKind::Present, "Answer ready").await;
    a.cost_paise = ctx.budget.spent();
    a.elapsed_s = ctx.budget.elapsed(Instant::now()).as_secs();
    let i = a.integrity;
    tracing::info!(
        run_id = %ctx.run_id,
        written = a.answer.is_some(), references = a.references.len(),
        dropped_unseen = i.dropped_unseen, dropped_unfetched = i.dropped_unfetched,
        removed_citations = i.removed_citations, dropped_numbers = i.dropped_numbers,
        dropped_studies = i.dropped_studies, degraded, "team answer assembled"
    );
    (a, degraded)
}

async fn run(ctx: &RunContext) -> Result<(AnswerV3, bool), Stop> {
    gate(ctx).await?;
    // The writer is reserved for before any research (see the module docs).
    let reserve = writer_v3::reserve_for(
        &ctx.llm,
        &ctx.budget,
        &ctx.question,
        ctx.settings.writer_max_tokens,
    )
    .unwrap_or(0);
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
    if let Some(reason) = stopped {
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
    if research.selected.is_empty() {
        answer.empty_reason = Some(
            if stopped.is_some() {
                STOPPED_NO_ANSWER
            } else {
                NO_EVIDENCE
            }
            .to_string(),
        );
        return Ok(finish(ctx, answer, stopped.is_some()).await);
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
        return Ok(finish(ctx, answer, true).await);
    }

    // The writer.
    gate(ctx).await?;
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
            Ok(finish(ctx, answer, stopped.is_some()).await)
        }
        Err(StepError::OverBudget) => {
            tracing::warn!(run_id = %ctx.run_id, "writer refused: over budget despite the finish reserve");
            add_line(
                ctx,
                StepKind::Note,
                "Not enough of the cost limit was left to write the answer",
            )
            .await;
            answer.empty_reason = Some(WRITE_REFUSED.to_string());
            Ok(finish(ctx, answer, true).await)
        }
        Err(e) => Err(e.kind()),
    }
}
