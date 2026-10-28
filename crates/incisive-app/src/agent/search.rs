//! Search (step 2): a PubMed query from the PICO, refined at most N times,
//! then run on PubMed and Europe PMC and merged. Server only.
//!
//! The `search` model proposes `{"query": …, "reason": …}`. Each proposal is
//! untrusted and goes through [`validate_query`] before it is sent anywhere;
//! an invalid one still uses up an attempt, and the search falls back to the
//! last valid query. After each valid query the model sees the PubMed hit
//! count and the first 20 titles (each wrapped as a `<source>`), and answers
//! `{"accept": true}` or a new query. That is the agent's only loop:
//! `1 + settings.refinements` proposals at most, stopped early by an accept,
//! a wrap-up (cost or time), a failed model call (the current query is kept),
//! or PubMed going down.
//!
//! The final query is run on PubMed (`settings.screen_n` records; the hits of
//! the last valid query are reused, not re-searched) and on Europe PMC
//! (`screen_n / 2`). Records are merged ([`merge_results`]), then abstracts
//! are filled cache first: cached papers are used as they are, the rest come
//! from efetch ([`PubMed::details`]), whose retraction flag is OR-ed into
//! `Paper.retracted` (Ruling R-6c), and are written back to the cache.
//!
//! One database down: a Note line "<db> unavailable" and the search goes on.
//! Both down: [`StepError::Lit`].
//!
//! Privacy: nothing here logs the question, the PICO, a query or a title;
//! only the run id, counts and fixed kinds.

use std::collections::{HashMap, HashSet};
use std::time::Instant;

use serde::Deserialize;

use super::frame::Pico;
use super::llm::{LlmCall, StepError, wrap_source};
use super::{AgentLlm, Literature, RunContext, add_line, clip};
use crate::lit::europepmc::merge_results;
use crate::lit::http::LitError;
use crate::lit::pubmed::SearchHits;
use crate::lit::{Paper, cache};
use crate::runs::StepKind;

/// Longest query accepted.
pub const MAX_QUERY: usize = 1000;
/// Titles shown to the model when it reviews a query.
pub const REVIEW_TITLES: usize = 20;
/// Characters of a refinement reason kept (step line and trail).
const MAX_REASON: usize = 120;
const MAX_TOKENS: u32 = 1000;

/// What Search produced.
#[derive(Debug, Clone, PartialEq)]
pub struct SearchResult {
    /// The query whose results were used (always valid).
    pub final_query: String,
    /// Each accepted refinement: (query, the model's reason, cleaned).
    pub refinements: Vec<(String, String)>,
    /// Total PubMed matches for the final query (0 if PubMed was down).
    pub pubmed_count: u64,
    /// Records Europe PMC returned for the final query (at most
    /// `screen_n / 2`; 0 if it was down). The client does not expose
    /// Europe PMC's total hit count.
    pub epmc_count: u64,
    /// Merged, de-duplicated records: PubMed's in relevance order, then
    /// Europe-PMC-only ones.
    pub papers: Vec<Paper>,
}

/// Check a model-proposed PubMed query. The error is a fixed kind:
/// `empty`, `too_long` (over 1,000 characters), `newline`, `url` (`http` or
/// `www.`, any case), `character` (outside `[A-Za-z0-9 -"()[]*/:,.']`) or
/// `unbalanced` (double quotes, parentheses or square brackets; brackets
/// inside a quoted phrase do not count).
pub fn validate_query(q: &str) -> Result<(), &'static str> {
    if q.trim().is_empty() {
        return Err("empty");
    }
    if q.chars().count() > MAX_QUERY {
        return Err("too_long");
    }
    if q.contains(['\n', '\r']) {
        return Err("newline");
    }
    let lower = q.to_ascii_lowercase();
    if lower.contains("http") || lower.contains("www.") {
        return Err("url");
    }
    let allowed = |c: char| {
        c.is_ascii_alphanumeric()
            || matches!(
                c,
                ' ' | '-' | '"' | '(' | ')' | '[' | ']' | '*' | '/' | ':' | ',' | '.' | '\''
            )
    };
    if !q.chars().all(allowed) {
        return Err("character");
    }
    let mut open: Vec<char> = Vec::new();
    let mut quoted = false;
    for c in q.chars() {
        match c {
            '"' => quoted = !quoted,
            _ if quoted => {}
            '(' | '[' => open.push(c),
            ')' if open.pop() != Some('(') => return Err("unbalanced"),
            ']' if open.pop() != Some('[') => return Err("unbalanced"),
            _ => {}
        }
    }
    if quoted || !open.is_empty() {
        return Err("unbalanced");
    }
    Ok(())
}

/// Fixed kind of a literature error, for logs.
fn lit_kind(e: &LitError) -> &'static str {
    match e {
        LitError::Disallowed(_) => "disallowed",
        LitError::TooLarge => "too_large",
        LitError::TooManyRedirects => "too_many_redirects",
        LitError::Http(_) => "http",
        LitError::Timeout => "timeout",
        LitError::Parse => "parse",
        LitError::Network => "network",
    }
}

// ---------- prompts ----------

const SYSTEM: &str = "You build PubMed search queries for practising surgeons who want the best clinical evidence on a question. \
Use PubMed syntax: MeSH terms ([mh]), title/abstract terms ([tiab]), publication types ([pt]), date ranges ([dp]), quoted phrases, \
AND / OR / NOT, parentheses and truncation (*). Prefer a query that finds the strongest evidence (guidelines, systematic reviews, \
meta-analyses, randomised trials) without missing key studies. Apply the limits the surgeon stated. \
Query rules, enforced by code: one line; at most 1,000 characters; only letters, digits, spaces and - \" ( ) [ ] * / : , . ' ; \
balanced quotes and brackets; no URLs or web addresses. A query that breaks a rule is discarded. \
Reply with one JSON object only.";

/// Model reply proposing a query.
#[derive(Deserialize)]
struct Proposal {
    #[serde(default)]
    query: String,
    #[serde(default)]
    reason: String,
}

/// Model reply reviewing a query's results.
#[derive(Deserialize)]
struct Review {
    #[serde(default)]
    accept: bool,
    #[serde(default)]
    query: Option<String>,
    #[serde(default)]
    reason: Option<String>,
}

fn pico_block(pico: &Pico) -> String {
    let json = serde_json::to_string_pretty(pico).unwrap_or_default();
    format!("PICO:\n{json}")
}

fn rejected_note(rejected: Option<&'static str>) -> String {
    rejected.map_or_else(String::new, |k| {
        format!("\n\nYour last proposed query was discarded by the query rules (reason: {k}). Follow the rules exactly.")
    })
}

fn propose_prompt(pico: &Pico, rejected: Option<&'static str>) -> String {
    format!(
        "{}\n\nWrite the PubMed query for this PICO.{}\n\nReply exactly as {{\"query\": string, \"reason\": string}}, where reason is one short sentence.",
        pico_block(pico),
        rejected_note(rejected)
    )
}

fn review_prompt(
    pico: &Pico,
    query: &str,
    count: u64,
    titles: &[String],
    rejected: Option<&'static str>,
) -> String {
    let mut wrapped = String::new();
    for (i, t) in titles.iter().enumerate() {
        wrapped.push_str(&wrap_source(&format!("t-{}", i + 1), t));
        wrapped.push('\n');
    }
    if titles.is_empty() {
        wrapped.push_str("(no titles)\n");
    }
    format!(
        "{}\n\nCurrent query:\n{query}\n\nPubMed hits: {count}\n\nFirst {} titles, in relevance order:\n{wrapped}{}\n\n\
         If this query finds the evidence for this PICO well, reply {{\"accept\": true}}. \
         Otherwise reply {{\"accept\": false, \"query\": string, \"reason\": string}} with a better query and one short sentence on why.",
        pico_block(pico),
        titles.len(),
        rejected_note(rejected)
    )
}

// ---------- paper lookups ----------

/// Records seen in this search, by PMID, and which of them came from the cache.
#[derive(Default)]
struct Known {
    papers: HashMap<String, Paper>,
    cached: HashSet<String>,
}

impl Known {
    /// Look `pmids` up in the cache (an error is a miss).
    async fn load_cached(&mut self, ctx: &RunContext, pmids: &[String]) {
        let unknown: Vec<String> = pmids
            .iter()
            .filter(|p| !self.papers.contains_key(*p))
            .cloned()
            .collect();
        if unknown.is_empty() {
            return;
        }
        match cache::get_many(&ctx.state.pool, &unknown).await {
            Ok(hits) => {
                for p in hits {
                    if let Some(id) = p.pmid.clone() {
                        self.cached.insert(id.clone());
                        self.papers.insert(id, p);
                    }
                }
            }
            Err(_) => {
                tracing::warn!(run_id = %ctx.run_id, step = "search", "paper cache read failed; treated as a miss");
            }
        }
    }

    /// Summaries for `pmids`, cache first, in `pmids` order. Ids PubMed does
    /// not know are left out. On a PubMed error, what is known is returned
    /// with the error.
    async fn summaries(
        &mut self,
        ctx: &RunContext,
        lit: &Literature,
        pmids: &[String],
    ) -> (Vec<Paper>, Option<LitError>) {
        self.load_cached(ctx, pmids).await;
        let missing: Vec<String> = pmids
            .iter()
            .filter(|p| !self.papers.contains_key(*p))
            .cloned()
            .collect();
        let mut err = None;
        if !missing.is_empty() {
            match lit.pubmed.summaries(&missing).await {
                Ok(found) => {
                    // Only fill gaps: a cached record (with its abstract) is
                    // never replaced by a bare summary.
                    for p in found {
                        if let Some(id) = p.pmid.clone() {
                            self.papers.entry(id).or_insert(p);
                        }
                    }
                }
                Err(e) => err = Some(e),
            }
        }
        let out = pmids
            .iter()
            .filter_map(|p| self.papers.get(p).cloned())
            .collect();
        (out, err)
    }
}

/// The query whose results are current.
struct Current {
    query: String,
    /// `None` when PubMed failed on this query.
    hits: Option<SearchHits>,
    titles: Vec<String>,
}

/// Clean a model's reason for the step line and trail.
fn clean_reason(ctx: &RunContext, reason: &str) -> String {
    let r = clip(reason, MAX_REASON);
    let q = ctx.question.trim().to_lowercase();
    if r.is_empty() {
        "no reason given".into()
    } else if !q.is_empty() && r.to_lowercase().contains(&q) {
        // Never echo the question into a step line.
        "query adjusted".into()
    } else {
        r
    }
}

// ---------- the step ----------

/// Search for `pico` (see the module docs).
pub async fn search(
    ctx: &RunContext,
    llm: &AgentLlm,
    lit: &Literature,
    pico: &Pico,
) -> Result<SearchResult, StepError> {
    let call = llm.call(ctx, &llm.models.search, "search");
    let run_id = ctx.run_id;
    let hits_max = ctx.settings.screen_n.max(REVIEW_TITLES as u16);
    let attempts = 1 + u32::from(ctx.settings.refinements);

    let mut known = Known::default();
    let mut current: Option<Current> = None;
    let mut refinements: Vec<(String, String)> = Vec::new();
    let mut pubmed_up = true;
    let mut rejected: Option<&'static str> = None;

    let first: Proposal = call
        .json(SYSTEM, &propose_prompt(pico, None), MAX_TOKENS)
        .await?;
    let mut proposal = Some((first.query, first.reason));

    for attempt in 0..attempts {
        if let Some((query, reason)) = proposal.take() {
            match validate_query(&query) {
                Ok(()) => {
                    rejected = None;
                    if attempt > 0 {
                        let reason = clean_reason(ctx, &reason);
                        add_line(ctx, StepKind::Search, &format!("Refined search: {reason}")).await;
                        refinements.push((query.clone(), reason));
                    }
                    match lit.pubmed.search(&query, hits_max).await {
                        Ok(hits) => {
                            add_line(
                                ctx,
                                StepKind::Search,
                                &format!("Searched PubMed: {} hits", hits.count),
                            )
                            .await;
                            let first_ids: Vec<String> =
                                hits.pmids.iter().take(REVIEW_TITLES).cloned().collect();
                            let (papers, err) = known.summaries(ctx, lit, &first_ids).await;
                            if let Some(e) = err {
                                tracing::warn!(%run_id, step = "search", error_kind = lit_kind(&e), "pubmed summaries failed; reviewing with fewer titles");
                            }
                            current = Some(Current {
                                query,
                                hits: Some(hits),
                                titles: papers.into_iter().map(|p| p.title).collect(),
                            });
                        }
                        Err(e) => {
                            tracing::warn!(%run_id, step = "search", error_kind = lit_kind(&e), "pubmed search failed");
                            pubmed_up = false;
                            add_line(ctx, StepKind::Note, "PubMed unavailable").await;
                            // The query is valid; Europe PMC still runs it.
                            current = Some(Current {
                                query,
                                hits: None,
                                titles: Vec::new(),
                            });
                            break;
                        }
                    }
                }
                Err(kind) => {
                    tracing::debug!(%run_id, step = "search", reason_kind = kind, "model query rejected");
                    add_line(
                        ctx,
                        StepKind::Note,
                        &format!("Proposed query rejected ({kind})"),
                    )
                    .await;
                    rejected = Some(kind);
                }
            }
        }

        if attempt + 1 >= attempts {
            break;
        }
        if ctx.cancel.is_cancelled() {
            return Err(StepError::Cancelled);
        }
        if ctx.budget.should_wrap_up(Instant::now()) {
            tracing::info!(%run_id, step = "search", attempt, "wrap-up reached; refinement stopped");
            break;
        }

        match next_proposal(&call, pico, current.as_ref(), rejected).await {
            Ok(Some(p)) => proposal = Some(p),
            Ok(None) => break, // accepted
            Err(e) if current.is_none() || e == StepError::Cancelled || e.is_fatal() => {
                return Err(e);
            }
            Err(e) => {
                tracing::info!(%run_id, step = "search", error_kind = e.kind(), "review failed; keeping the current query");
                break;
            }
        }
    }

    let Some(current) = current else {
        tracing::debug!(%run_id, step = "search", reason_kind = rejected.unwrap_or("none"), "no valid query");
        return Err(StepError::Parse);
    };
    if ctx.cancel.is_cancelled() {
        return Err(StepError::Cancelled);
    }

    finish(ctx, lit, current, refinements, pubmed_up, known).await
}

/// Ask for the next query: a fresh proposal when no query is valid yet,
/// otherwise a review of the current one. `Ok(None)` means accepted.
async fn next_proposal(
    call: &LlmCall,
    pico: &Pico,
    current: Option<&Current>,
    rejected: Option<&'static str>,
) -> Result<Option<(String, String)>, StepError> {
    let Some(cur) = current else {
        let p: Proposal = call
            .json(SYSTEM, &propose_prompt(pico, rejected), MAX_TOKENS)
            .await?;
        return Ok(Some((p.query, p.reason)));
    };
    let count = cur.hits.as_ref().map_or(0, |h| h.count);
    let prompt = review_prompt(pico, &cur.query, count, &cur.titles, rejected);
    let r: Review = call.json(SYSTEM, &prompt, MAX_TOKENS).await?;
    if r.accept {
        return Ok(None);
    }
    // Not accepted and no query: an empty proposal, rejected as `empty`.
    Ok(Some((
        r.query.unwrap_or_default(),
        r.reason.unwrap_or_default(),
    )))
}

/// Run the final query on both databases, merge, and fill abstracts.
async fn finish(
    ctx: &RunContext,
    lit: &Literature,
    current: Current,
    refinements: Vec<(String, String)>,
    mut pubmed_up: bool,
    mut known: Known,
) -> Result<SearchResult, StepError> {
    let run_id = ctx.run_id;
    let screen_n = usize::from(ctx.settings.screen_n);

    let mut pubmed_count = 0;
    let mut pubmed_papers = Vec::new();
    if let Some(hits) = current.hits.as_ref().filter(|_| pubmed_up) {
        pubmed_count = hits.count;
        let ids: Vec<String> = hits.pmids.iter().take(screen_n).cloned().collect();
        let (papers, err) = known.summaries(ctx, lit, &ids).await;
        if let Some(e) = err {
            tracing::warn!(%run_id, step = "search", error_kind = lit_kind(&e), "pubmed summaries failed");
            pubmed_up = false;
            add_line(ctx, StepKind::Note, "PubMed unavailable").await;
        }
        pubmed_papers = papers;
    }

    let epmc_max = (ctx.settings.screen_n / 2).max(1);
    let (epmc_papers, epmc_up) = match lit.epmc.search(&current.query, epmc_max).await {
        Ok(v) => {
            add_line(
                ctx,
                StepKind::Search,
                &format!("Searched Europe PMC: {} hits", v.len()),
            )
            .await;
            (v, true)
        }
        Err(e) => {
            tracing::warn!(%run_id, step = "search", error_kind = lit_kind(&e), "europe pmc search failed");
            add_line(ctx, StepKind::Note, "Europe PMC unavailable").await;
            (Vec::new(), false)
        }
    };
    if !pubmed_up && pubmed_papers.is_empty() && !epmc_up {
        tracing::warn!(%run_id, step = "search", "both literature databases unavailable");
        return Err(StepError::Lit);
    }
    let epmc_count = epmc_papers.len() as u64;

    let mut papers = merge_results(pubmed_papers, epmc_papers);
    fill_abstracts(ctx, lit, &mut papers, &mut known, pubmed_up).await;
    add_line(
        ctx,
        StepKind::Search,
        &format!("Merged: {} unique records", papers.len()),
    )
    .await;

    Ok(SearchResult {
        final_query: current.query,
        refinements,
        pubmed_count,
        epmc_count,
        papers,
    })
}

/// Abstracts, cache first. Cached papers keep their cached abstract (and
/// retraction flag); the rest are fetched with efetch when PubMed is up, have
/// efetch's abstract, publication types and retraction merged in, and are
/// written back to the cache. Failures degrade to no abstract.
async fn fill_abstracts(
    ctx: &RunContext,
    lit: &Literature,
    papers: &mut [Paper],
    known: &mut Known,
    pubmed_up: bool,
) {
    let run_id = ctx.run_id;
    let ids: Vec<String> = papers.iter().filter_map(|p| p.pmid.clone()).collect();
    known.load_cached(ctx, &ids).await;
    for p in papers.iter_mut() {
        let Some(c) = p.pmid.as_ref().filter(|id| known.cached.contains(*id)) else {
            continue;
        };
        let c = &known.papers[c];
        if p.abstract_text.is_none() {
            p.abstract_text.clone_from(&c.abstract_text);
        }
        p.retracted |= c.retracted;
    }
    if !pubmed_up {
        return;
    }
    let need: Vec<String> = ids
        .into_iter()
        .filter(|id| !known.cached.contains(id))
        .collect();
    if need.is_empty() {
        return;
    }
    let details = match lit.pubmed.details(&need).await {
        Ok(d) => d,
        Err(e) => {
            tracing::warn!(%run_id, step = "search", error_kind = lit_kind(&e), "pubmed efetch failed; abstracts missing");
            return;
        }
    };
    let mut fetched = Vec::new();
    for p in papers.iter_mut() {
        let Some(rec) = p.pmid.as_ref().and_then(|id| details.get(id)) else {
            continue;
        };
        if p.abstract_text.is_none() {
            p.abstract_text.clone_from(&rec.abstract_text);
        }
        if p.pub_types.is_empty() {
            p.pub_types.clone_from(&rec.pub_types);
        }
        p.retracted |= rec.retracted;
        fetched.push(p.clone());
    }
    if cache::put_many(&ctx.state.pool, &fetched).await.is_err() {
        tracing::warn!(%run_id, step = "search", "paper cache write failed");
    }
}
