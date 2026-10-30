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
use super::answer_v3::{
    ANSWER_VERSION, AnswerV3, EvidenceV3, IntegrityV3, NotableFinding, ReferenceV3, SkepticItem,
    SkepticLead, SkepticOverlap, SkepticV3, Span, UnverifiedOutcomeV3,
};
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
    ctx.budget.set_finish_reserve(reserve + review);
    tracing::info!(run_id = %ctx.run_id, writer_reserve = reserve, review_reserve = review, limit = ctx.budget.limit_paise(), "team: writer reserved");
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
        reviewed = review_phase(ctx, &research, &fetched, &studies, reserve).await?;
        for (k, m) in reviewed.missed.iter().chain(&reviewed.cited).enumerate() {
            answer.references.push(ReferenceV3 {
                n: u16::try_from(studies.len() + k + 1).unwrap_or(u16::MAX),
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
                writer_v3::trace_numbers(&written, &allowed, |p| selected.iter().any(|s| s == p));
            integrity.numbers_checked = checked;
            integrity.numbers_untraced = count(untraced.len());
            answer.untraced = untraced;
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
    notes: Vec<String>,
}

/// What research leaves free for the review phase in a Full run: every
/// selected study (at most ten) extracted once at the extractor's largest
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
    let n = i64::try_from(super::researcher::MAX_SELECTED).unwrap_or(10);
    Some(per_study.saturating_mul(n).saturating_add(skeptic))
}

/// The extractor and the skeptic, concurrently (see the module docs).
async fn review_phase(
    ctx: &RunContext,
    research: &Research,
    fetched: &[StudyRef],
    studies: &[StudyInput],
    writer_reserve: i64,
) -> Result<Reviewed, Stop> {
    gate(ctx).await?;
    // Research is over: the review phase may use what research had to
    // leave for it; only the writer stays reserved. Its wrap-up point is
    // the time limit less the writer's allowance.
    ctx.budget.set_finish_reserve(writer_reserve);
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
    // The skeptic's missed RCTs: records and abstracts, cited and
    // referenced, never extracted.
    let missed_ids: Vec<String> = out
        .review
        .as_ref()
        .map(|r| r.missed_rcts.iter().map(|m| m.pmid.clone()).collect())
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
                "Not independent evidence (the skeptic): ",
                &o.note,
                &format!("overlaps.{i}.note"),
                o.pmids.clone(),
                &r.spans,
            ));
        }
        for (i, m) in r.missed_rcts.iter().enumerate() {
            out.push(item(
                "A randomised trial the researcher's search missed (the skeptic): ",
                &m.why,
                &format!("missed_rcts.{i}.text"),
                vec![m.pmid.clone()],
                &r.spans,
            ));
        }
        for (i, m) in r.ma_notes.iter().enumerate() {
            out.push(item(
                "What a meta-analysis pooled (the skeptic): ",
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
            text: format!(
                "The extractor could not match {} and {} to the papers' wording; they are kept and labelled \u{201c}not verified\u{201d} in the trials table.",
                plural(outcomes as usize, "outcome", "outcomes"),
                plural(numbers as usize, "number", "numbers")
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
    }
}
