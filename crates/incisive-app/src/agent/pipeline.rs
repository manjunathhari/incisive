//! The real pipeline (§2): Frame → Search → Screen → [Retrieve, Focused only]
//! → Extract → Write → Check → Present, then the structured [`Answer`].
//! Server only.
//!
//! **Between steps** the run is checked for cancellation, the 6-minute hard
//! stop (`Failed("hard_stop")`) and its heartbeat (a run no longer `running`
//! stops as `Failed("interrupted")`; the worker sorts out why).
//!
//! **Graceful finish.** Before each gathering step (Search, Screen,
//! Retrieve, Extract) the wrap-up point is checked: 70% of the cost limit
//! spent, 3 minutes elapsed, or no headroom above the **finish reserve**
//! (Ruling R-13c). Once it is reached, the remaining gathering steps are
//! skipped (one Note line) and the run goes straight to Write and Check
//! with what it has; the trail says "search stopped early (cost limit / time
//! limit)". A step that stops itself (Screen between batches, Extract between
//! papers, each reporting how much it left unread) marks the same.
//! `OverBudget` from a gathering step wraps up the same way ("cost limit").
//!
//! The finish reserve is what Write and Check (at `MAX_CLAIMS`) would hold
//! for the findings so far ([`super::finish_reserve`]); it is set once the
//! question is framed and raised by Extract after each paper. Every
//! gathering call's hold must leave it free (`try_hold_gathering`), so the
//! gathering money is never spent at the cost of the write-up. If Write is
//! still refused, the run ends `partial` with a Note and no claims, never
//! `failed`; the trail records `write_refused` (not "search stopped early":
//! the search may well have finished) and the bottom line says the cost
//! limit was reached before the answer could be written.
//!
//! **Failures** (Ruling, Task 13 carry-over 2), as the run's `error_kind`:
//! * `Unrecorded` (money not booked): "unrecorded", at once.
//! * `Lit` (both databases down): "lit".
//! * `Cancelled`: "cancelled"; the worker treats the run as deleted.
//! * `Llm` / `Parse` in Frame, Search or Write: "llm" / "parse". (Search
//!   fails this way only when no valid query could be made at all: there is
//!   nothing to answer from, and "no evidence found" would be untrue.)
//! * `OverBudget` in Frame: "over_budget". (In Write: `partial`, above.)
//! * Check handles its own per-claim failures.
//!
//! **Answer.** References and key studies are the papers whose refs Check
//! verified live, renumbered 1..K in their original order (claims are
//! renumbered with them). Key studies are Focused only. `insufficient` is set
//! when Write said so (fewer than two papers, or the model's judgement) and
//! recomputed after Check: fewer than two distinct verified refs across the
//! surviving claims is insufficient (Ruling R-12e). If Check removed every
//! drafted claim, `no_verified_claims` is set and the bottom line reads "No
//! claims could be verified against the sources." (Review Focus 2). With no
//! claim drafted, the bottom line is "No relevant evidence found." after a
//! complete run, or says the search stopped early when it did.
//!
//! The outcome is `Partial` when a step degraded: a database was
//! unavailable, the search stopped early, or no claim survived Check;
//! otherwise `Done`.
//!
//! **Service failures** (final review I-1). Screen and Extract count their
//! failed Claude calls (`Llm`/`Parse`; budget stops and cancellation are not
//! failures) and write a Note with the count. If nothing was included, or
//! nothing extracted, while some of those calls failed, the empty result
//! cannot be read as "no evidence": the run is `partial`, `service_failed` is
//! set and the bottom line says the AI service failed. Failures beside
//! successful calls leave only their Note.
//!
//! Nothing found is two different outcomes (Ruling R-13a):
//! * wrap-up or the budget stopped gathering before anything was extracted:
//!   `Partial`, with the "Search stopped early" Note and `stopped_early` set
//!   (the search was cut short, so "no evidence" would overstate it);
//! * every step ran and nothing relevant was found (empty search, nothing
//!   included): `Done`, `insufficient`, "No relevant evidence found."
//!
//! Privacy: nothing here logs the question, PICO, claims or titles; only the
//! run id, counts and fixed kinds.

use std::collections::{BTreeSet, HashMap};
use std::time::Instant;

use chrono::Utc;

use super::answer::{
    Answer, AnswerClaim, CheckSummary, KeyStudy, Pico, Reference, Refinement, Trail,
};
use super::check::{ClaimVerdict, check};
use super::extract::{Extracted, extract};
use super::frame::frame;
use super::llm::StepError;
use super::retrieve::{UsedPaper, retrieve};
use super::screen::{REASON_STOPPED, screen};
use super::search::search;
use super::write::{Draft, MAX_CLAIMS, MIN_PAPERS, write};
use super::{PipelineOutcome, RunContext, add_line};
use crate::lit::Paper;
use crate::runs::{RunMode, StepKind, worker};

/// Exclusion reasons kept in the trail.
const EXCLUDED_TOP: usize = 5;

/// Why the run ended without an answer: its `error_kind`.
type Stop = &'static str;

/// Run the pipeline for `ctx` (see the module docs).
pub async fn run_pipeline(ctx: &RunContext) -> PipelineOutcome {
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
            tracing::info!(run_id = %ctx.run_id, error_kind = kind, "pipeline stopped");
            PipelineOutcome::Failed(kind)
        }
    }
}

/// The error kind for a step error that ends the run.
fn failure(e: StepError) -> Stop {
    e.kind()
}

/// A gathering step's error: `OverBudget` wraps up (`Ok(None)`); anything
/// else ends the run.
fn gathering<T>(r: Result<T, StepError>) -> Result<Option<T>, Stop> {
    match r {
        Ok(v) => Ok(Some(v)),
        Err(StepError::OverBudget) => Ok(None),
        Err(e) => Err(failure(e)),
    }
}

/// Before every step: cancelled, past the hard stop, or no longer running.
async fn gate(ctx: &RunContext) -> Result<(), Stop> {
    if ctx.cancel.is_cancelled() {
        return Err("cancelled");
    }
    if ctx.budget.past_hard_stop(Instant::now()) {
        return Err("hard_stop");
    }
    if !worker::heartbeat(ctx).await {
        return Err("interrupted");
    }
    Ok(())
}

/// What the gathering steps found.
#[derive(Default)]
struct Gathered {
    final_query: String,
    trail: Trail,
    extracted: Vec<Extracted>,
    /// "cost limit" / "time limit" once gathering stopped early.
    stopped: Option<&'static str>,
    /// Write was refused for budget (despite the finish reserve).
    write_refused: bool,
    /// Nothing was included or extracted and some Claude calls of that step
    /// failed: the empty result may be the outage's, not the literature's.
    service_failed: bool,
}

impl Gathered {
    /// Before a gathering step: true when it should run. At the wrap-up point
    /// the step is skipped, a Note is written (once) and `stopped` is set.
    async fn may_gather(&mut self, ctx: &RunContext) -> bool {
        if self.stopped.is_some() {
            return false;
        }
        match ctx.budget.wrap_up_reason(Instant::now()) {
            None => true,
            Some(reason) => {
                self.stop(ctx, reason).await;
                false
            }
        }
    }

    async fn stop(&mut self, ctx: &RunContext, reason: &'static str) {
        if self.stopped.is_none() {
            self.stopped = Some(reason);
            tracing::info!(run_id = %ctx.run_id, reason, "wrap-up: going straight to write and check");
            add_line(
                ctx,
                StepKind::Note,
                &format!("Search stopped early ({reason}); writing the answer from what was found"),
            )
            .await;
        }
    }

    /// The reason to record when a step stopped itself early.
    fn stopped_by_step(ctx: &RunContext) -> &'static str {
        ctx.budget
            .wrap_up_reason(Instant::now())
            .unwrap_or("cost limit")
    }
}

async fn gather(ctx: &RunContext, pico: &Pico) -> Result<Gathered, Stop> {
    let mut g = Gathered::default();

    // 2. Search.
    gate(ctx).await?;
    let mut papers: Vec<Paper> = Vec::new();
    if g.may_gather(ctx).await {
        match gathering(search(ctx, &ctx.llm, &ctx.lit, pico).await)? {
            Some(s) => {
                g.final_query = s.final_query;
                g.trail.pubmed_count = s.pubmed_count;
                g.trail.epmc_count = s.epmc_count;
                g.trail.refinements = s
                    .refinements
                    .into_iter()
                    .map(|(query, reason)| Refinement { query, reason })
                    .collect();
                g.trail.unavailable = s.unavailable.iter().map(|d| d.to_string()).collect();
                papers = s.papers;
            }
            None => g.stop(ctx, "cost limit").await,
        }
    }
    g.trail.screened = u32::try_from(papers.len()).unwrap_or(u32::MAX);

    // 3. Screen.
    if papers.is_empty() {
        if g.stopped.is_none() {
            add_line(ctx, StepKind::Screen, "No records to screen").await;
        }
        return Ok(g);
    }
    gate(ctx).await?;
    if !g.may_gather(ctx).await {
        return Ok(g);
    }
    let Some(screened) = gathering(screen(ctx, &ctx.llm, pico, papers).await)? else {
        g.stop(ctx, "cost limit").await;
        return Ok(g);
    };
    g.trail.included = u32::try_from(screened.included.len()).unwrap_or(u32::MAX);
    // Records left unscreened at the wrap-up point were not assessed, not
    // excluded: counted apart.
    g.trail.not_assessed = screened
        .excluded_counts
        .iter()
        .filter(|(r, _)| r == REASON_STOPPED)
        .map(|(_, n)| *n)
        .sum();
    g.trail.excluded_top = screened
        .excluded_counts
        .iter()
        .filter(|(r, _)| r != REASON_STOPPED)
        .take(EXCLUDED_TOP)
        .cloned()
        .collect();
    if g.trail.not_assessed > 0 {
        // Screen stopped between batches at the wrap-up point.
        let reason = Gathered::stopped_by_step(ctx);
        g.stop(ctx, reason).await;
    }
    if screened.included.is_empty() {
        // Some batches failed: "nothing included" is not a finding.
        g.service_failed = screened.failed_batches > 0;
        return Ok(g);
    }

    // 4. Retrieve (Focused; in Quick it only picks the papers, no fetching).
    gate(ctx).await?;
    if !g.may_gather(ctx).await {
        return Ok(g);
    }
    let Some(used): Option<Vec<UsedPaper>> = gathering(retrieve(ctx, &ctx.lit, screened).await)?
    else {
        g.stop(ctx, "cost limit").await;
        return Ok(g);
    };
    if used.is_empty() {
        return Ok(g);
    }

    // 5. Extract.
    gate(ctx).await?;
    if !g.may_gather(ctx).await {
        return Ok(g);
    }
    let Some(extraction) = gathering(extract(ctx, &ctx.llm, pico, used).await)? else {
        g.stop(ctx, "cost limit").await;
        return Ok(g);
    };
    if extraction.stopped > 0 {
        // Extract left papers unread (wrap-up point or budget).
        let reason = Gathered::stopped_by_step(ctx);
        g.stop(ctx, reason).await;
    }
    // Some extraction calls failed and nothing was extracted: not a finding.
    g.service_failed = extraction.papers.is_empty() && extraction.failed > 0;
    g.extracted = extraction.papers;
    Ok(g)
}

/// At most [`MAX_CLAIMS`] claims, each citing every ref once (Check relies
/// on Write for both; enforced again here).
fn bound_draft(mut d: Draft) -> Draft {
    d.bottom_line.truncate(MAX_CLAIMS);
    for c in &mut d.bottom_line {
        let mut seen = BTreeSet::new();
        c.refs.retain(|r| seen.insert(*r));
    }
    d
}

async fn run(ctx: &RunContext) -> Result<(Answer, bool), Stop> {
    // 1. Frame.
    gate(ctx).await?;
    let pico = frame(ctx, &ctx.llm, &ctx.question).await.map_err(failure)?;
    // From here on, gathering leaves Write and Check their share (R-13c).
    ctx.budget
        .set_finish_reserve(super::finish_reserve(&ctx.llm, &ctx.budget, &pico, &[]));

    // 2–5. Gathering.
    let mut g = gather(ctx, &pico).await?;

    // 6. Write: always, with what was found. Refused for budget despite the
    // reserve: the run ends partial with what was gathered, never failed.
    gate(ctx).await?;
    let draft = match write(ctx, &ctx.llm, &pico, &g.extracted).await {
        Ok(d) => bound_draft(d),
        Err(StepError::OverBudget) => {
            tracing::info!(run_id = %ctx.run_id, "write refused: over budget; answer without claims");
            add_line(
                ctx,
                StepKind::Note,
                "Not enough of the cost limit was left to write the answer",
            )
            .await;
            g.write_refused = true;
            Draft {
                bottom_line: vec![],
                insufficient: true,
            }
        }
        Err(e) => return Err(failure(e)),
    };
    let drafted = draft.bottom_line.len();
    let draft_insufficient = draft.insufficient;

    // 7. Check: always, when there is a claim to check.
    let checked = if draft.bottom_line.is_empty() {
        None
    } else {
        gate(ctx).await?;
        Some(
            check(ctx, &ctx.llm, &ctx.lit, draft, &g.extracted)
                .await
                .map_err(failure)?,
        )
    };

    // 8. Present.
    gate(ctx).await?;
    let (answer, degraded) = assemble(ctx, pico, g, drafted, draft_insufficient, checked);
    add_line(ctx, StepKind::Present, "Answer ready").await;
    Ok((answer, degraded))
}

/// "Smith J et al., JAMA Surg, 2025".
fn citation(p: &Paper) -> String {
    let mut parts: Vec<String> = Vec::new();
    match p.authors.as_slice() {
        [] => {}
        [one] => parts.push(one.clone()),
        [first, ..] => parts.push(format!("{first} et al.")),
    }
    if let Some(j) = p.journal.as_deref().filter(|j| !j.is_empty()) {
        parts.push(j.to_string());
    }
    if let Some(y) = p.year {
        parts.push(y.to_string());
    }
    if parts.is_empty() {
        parts.push(p.title.clone());
    }
    parts.join(", ")
}

fn count(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

fn assemble(
    ctx: &RunContext,
    pico: Pico,
    g: Gathered,
    drafted: usize,
    draft_insufficient: bool,
    checked: Option<super::check::Checked>,
) -> (Answer, bool) {
    let Gathered {
        final_query,
        mut trail,
        extracted,
        stopped,
        write_refused,
        service_failed,
    } = g;
    let by_ref: HashMap<u8, &Extracted> = extracted.iter().map(|e| (e.ref_id, e)).collect();
    let verified: Vec<u8> = checked
        .as_ref()
        .map(|c| {
            c.verified
                .iter()
                .copied()
                .filter(|r| by_ref.contains_key(r))
                .collect()
        })
        .unwrap_or_default();
    // Verified refs renumbered 1..K in their original order.
    let renumber: HashMap<u8, u8> = verified
        .iter()
        .enumerate()
        .map(|(i, r)| (*r, u8::try_from(i + 1).unwrap_or(u8::MAX)))
        .collect();

    let references: Vec<Reference> = verified
        .iter()
        .map(|r| {
            let e = by_ref[r];
            Reference {
                ref_id: renumber[r],
                title: e.paper.title.clone(),
                journal: e.paper.journal.clone(),
                year: e.paper.year,
                pmid: e.paper.pmid.clone(),
                doi: e.paper.doi.clone(),
                study_type: e.level.as_str().to_string(),
            }
        })
        .collect();

    let key_studies: Vec<KeyStudy> = if ctx.mode == RunMode::Focused {
        verified
            .iter()
            .map(|r| {
                let e = by_ref[r];
                let main_finding = e
                    .findings
                    .first()
                    .map(|f| format!("{}: {}", f.outcome, f.result))
                    .unwrap_or_default();
                KeyStudy {
                    ref_id: renumber[r],
                    citation: citation(&e.paper),
                    design: if e.design.is_empty() {
                        e.level.as_str().to_string()
                    } else {
                        e.design.clone()
                    },
                    n: e.n,
                    main_finding,
                    source: e.source.as_str().to_string(),
                }
            })
            .collect()
    } else {
        Vec::new()
    };

    let mut check_summary = CheckSummary {
        checked: count(drafted),
        ..CheckSummary::default()
    };
    let mut bottom_line: Vec<AnswerClaim> = Vec::new();
    let mut draft_says_insufficient = draft_insufficient;
    if let Some(c) = &checked {
        draft_says_insufficient |= c.insufficient;
        for (claim, verdict) in &c.claims {
            bottom_line.push(AnswerClaim {
                text: claim.text.clone(),
                refs: claim
                    .refs
                    .iter()
                    .filter_map(|r| renumber.get(r).copied())
                    .collect(),
                confidence: claim.confidence.as_str().to_string(),
                partial: *verdict == ClaimVerdict::Partial,
            });
        }
        check_summary.supported = count(
            c.claims
                .iter()
                .filter(|(_, v)| *v == ClaimVerdict::Supported)
                .count(),
        );
        check_summary.partial = count(
            c.claims
                .iter()
                .filter(|(_, v)| *v == ClaimVerdict::Partial)
                .count(),
        );
        check_summary.removed = c.removed;
        check_summary.refs_verified = c.refs_verified;
        check_summary.refs_total = c.refs_verified.saturating_add(c.refs_removed);
    }
    // A claim with no verified ref cannot survive Check; drop defensively.
    bottom_line.retain(|c| !c.refs.is_empty());

    // Ruling R-12e: thin evidence is judged on what survived Check.
    let distinct: BTreeSet<u8> = bottom_line
        .iter()
        .flat_map(|c| c.refs.iter().copied())
        .collect();
    let insufficient =
        draft_says_insufficient || extracted.len() < MIN_PAPERS || distinct.len() < MIN_PAPERS;
    // Claims were drafted and Check kept none. With none drafted there was
    // nothing to verify: the insufficient / no-evidence wording applies.
    let no_verified_claims = drafted > 0 && bottom_line.is_empty();

    trail.stopped_early = stopped.map(str::to_string);
    trail.write_refused = write_refused;
    trail.service_failed = service_failed;
    let degraded = !trail.unavailable.is_empty()
        || stopped.is_some()
        || write_refused
        || service_failed
        || no_verified_claims;

    let answer = Answer {
        question: ctx.question.clone(),
        mode: ctx.mode,
        created_at: Utc::now(),
        pico,
        final_query,
        bottom_line,
        insufficient,
        no_verified_claims,
        key_studies,
        references,
        check: check_summary,
        trail,
        cost_paise: ctx.budget.spent(),
        elapsed_s: ctx.budget.elapsed(Instant::now()).as_secs(),
    };
    tracing::info!(
        run_id = %ctx.run_id, claims = answer.bottom_line.len(), references = answer.references.len(),
        insufficient, no_verified_claims, degraded, "answer assembled"
    );
    (answer, degraded)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::write::{Claim, Confidence};

    #[test]
    fn bound_draft_caps_claims_and_dedupes_refs() {
        let claim = |i: u8| Claim {
            text: format!("claim {i}"),
            refs: vec![2, 1, 2, 1, 3],
            confidence: Confidence::Moderate,
        };
        let d = bound_draft(Draft {
            bottom_line: (1..=8).map(claim).collect(),
            insufficient: false,
        });
        assert_eq!(d.bottom_line.len(), MAX_CLAIMS);
        assert_eq!(d.bottom_line[5].text, "claim 6", "the first six, in order");
        assert!(d.bottom_line.iter().all(|c| c.refs == [2, 1, 3]));
    }
}
