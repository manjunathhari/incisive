//! Check (step 7): every drafted claim judged against the quotes it cites, in
//! a fresh context, then every remaining reference verified live. Server only.
//! This is the last safety layer before a surgeon reads the answer.
//!
//! **Claim check.**
//! * `check` calls in batches of at most [`CHECK_BATCH`] (6) claims. Each call
//!   is a new request with no drafting history: for each claim, its text
//!   (wrapped as `<source id="claim-i">`, i counting across batches from 1)
//!   and the verbatim quotes of every ref it cites (each wrapped as
//!   `<source id="ref-N">`). No PICO, question, paper text or Write reply.
//! * The prompt binds numbers to their measure and arm ([`MEASURE_RULE`],
//!   Ruling R-11c follow-up): Extract only proved each number appears in its
//!   quote, not that it is the right measure for the right groups.
//! * The reply `[{"claim":i,"verdict":"supported|partial|unsupported"}]` is
//!   untrusted and read entry by entry, as in Screen: the claim id may be a
//!   number, a digit string or `claim-i`; the verdict is read ignoring case
//!   ("partly supported" is partial); ids outside the batch are ignored and
//!   the first verdict per id wins. A missing or unknown verdict is
//!   unsupported. A failed batch (`Llm`, `Parse` after the wrapper's retry)
//!   is unsupported; `OverBudget` leaves the rest unsupported (the safe
//!   fallback: unchecked claims are removed). The wrap-up point never skips
//!   Check. `Cancelled` and fatal errors propagate.
//! * Unsupported claims are removed; partial claims are kept and marked.
//!
//! **Live citation verification** of every ref still cited:
//! * Refs with a PMID: one `PubMed::summaries` and one `PubMed::details`
//!   lookup for all of them. A ref is removed when its PMID is absent from
//!   the summaries (Review Focus 3: withdrawn, never shown) or from the
//!   details; when its title does not match the extracted paper's (lower-case
//!   alphanumerics only, first [`TITLE_PREFIX`] (60) characters), or both
//!   DOIs are present and differ (ignoring case and a `doi.org` prefix); or
//!   when it is retracted (the summary's flag OR `details().retracted`,
//!   Ruling R-6c). A lookup that fails removes every ref it covers (fail
//!   closed).
//! * Refs without a PMID are verified by DOI with
//!   `EuropePmc::search("DOI:\"<doi>\"")`: there must be a hit with the same
//!   DOI that is not retracted. A ref with neither PMID nor DOI is removed.
//! * A claim keeps its valid refs; a claim left with none is removed.
//! * Every claim removed gives an empty [`Checked::claims`]; the answer then
//!   says "No claims could be verified against the sources" (Task 13, Review
//!   Focus 2).
//!
//! Step line: "Checked N claims: S supported, P partly, R removed; V/T
//! references verified".
//!
//! Privacy: nothing here logs claims, quotes, titles or the question; only
//! the run id, counts and fixed kinds.

use std::collections::{BTreeSet, HashMap, HashSet};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::extract::Extracted;
use super::llm::{StepError, wrap_source};
use super::write::{Claim, Draft};
use super::{AgentLlm, Literature, RunContext, add_line};
use crate::lit::Paper;
use crate::runs::StepKind;

/// Claims per `check` call.
pub const CHECK_BATCH: usize = 6;
/// Normalised title characters compared.
pub const TITLE_PREFIX: usize = 60;
/// Europe PMC hits read per DOI lookup.
const DOI_HITS: u16 = 5;
const MAX_TOKENS: u32 = 600;

/// The rule that binds a claim's numbers to the right measure and groups.
pub const MEASURE_RULE: &str = "A claim is supported only if the quoted text states the same measure, direction and comparison groups; numbers that appear in the quote but refer to something else do not count.";

/// The checker's judgement of one claim.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ClaimVerdict {
    Supported,
    Partial,
    Unsupported,
}

impl ClaimVerdict {
    /// A verdict word, read tolerantly; anything unknown is unsupported.
    fn read(v: Option<&Value>) -> ClaimVerdict {
        let Some(s) = v.and_then(Value::as_str) else {
            return ClaimVerdict::Unsupported;
        };
        let s = s.trim().to_ascii_lowercase().replace(['_', '-'], " ");
        match s.split_whitespace().collect::<Vec<_>>().join(" ").as_str() {
            "supported" | "support" | "fully supported" => ClaimVerdict::Supported,
            "partial" | "partly" | "partially" | "partly supported" | "partially supported" => {
                ClaimVerdict::Partial
            }
            _ => ClaimVerdict::Unsupported,
        }
    }
}

/// What Check kept.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Checked {
    /// Supported or partial claims, in draft order, each citing only verified
    /// refs. May be empty.
    pub claims: Vec<(Claim, ClaimVerdict)>,
    /// Claims removed (unsupported, or left with no verified ref).
    pub removed: u32,
    /// Refs verified live, of those still cited after the claim check.
    pub refs_verified: u32,
    /// Refs removed by live verification.
    pub refs_removed: u32,
}

const SYSTEM: &str = "You are an independent checker of an evidence summary for a practising surgeon. \
You are given claims, each inside <source id=\"claim-i\"> tags, followed by the verbatim quotes from the papers it cites, \
each inside <source id=\"ref-N\"> tags. Judge each claim only against its own quotes; use no other knowledge. \
A claim is supported when its quotes state everything it says; partial when its quotes support part of it \
or support it with less certainty or generality than it states; unsupported otherwise. ";

const SYSTEM_TAIL: &str = " Reply with a JSON array only, one object per claim, in this form: \
[{\"claim\": i, \"verdict\": \"supported\" or \"partial\" or \"unsupported\"}]";

fn system() -> String {
    format!("{SYSTEM}{MEASURE_RULE}{SYSTEM_TAIL}")
}

/// The claims of one batch with their cited quotes. `first` is the 1-based
/// number of the batch's first claim.
fn batch_prompt(first: usize, claims: &[Claim], by_ref: &HashMap<u8, &Extracted>) -> String {
    let mut body = String::new();
    for (k, c) in claims.iter().enumerate() {
        let i = first + k;
        body.push_str(&format!("Claim {i}:\n"));
        body.push_str(&wrap_source(&format!("claim-{i}"), &c.text));
        body.push_str(&format!("\nQuotes cited by claim {i}:\n"));
        for r in &c.refs {
            if let Some(e) = by_ref.get(r) {
                for f in &e.findings {
                    body.push_str(&wrap_source(&format!("ref-{r}"), &f.quote));
                    body.push('\n');
                }
            }
        }
        body.push('\n');
    }
    let ids = (first..first + claims.len())
        .map(|i| i.to_string())
        .collect::<Vec<_>>()
        .join(", ");
    format!("{body}Give one verdict for each of these claims: {ids}.")
}

/// A verdict's claim number: `3`, `"3"`, `"claim-3"` or `"c-3"`.
fn verdict_claim(v: &Value) -> Option<usize> {
    match v.get("claim")? {
        Value::Number(n) => n.as_u64().and_then(|n| usize::try_from(n).ok()),
        Value::String(s) => {
            let s = s.trim().to_ascii_lowercase();
            let s = s
                .strip_prefix("claim-")
                .or_else(|| s.strip_prefix("c-"))
                .unwrap_or(&s);
            if s.is_empty() || !s.chars().all(|c| c.is_ascii_digit()) {
                return None;
            }
            s.parse().ok()
        }
        _ => None,
    }
}

/// The verdict list: the array itself, or the first array in an object.
fn verdict_list(reply: Value) -> Option<Vec<Value>> {
    match reply {
        Value::Array(a) => Some(a),
        Value::Object(o) => o.into_iter().find_map(|(_, v)| match v {
            Value::Array(a) => Some(a),
            _ => None,
        }),
        _ => None,
    }
}

// ---------- reference matching ----------

/// Lower-case alphanumerics of `s`, first [`TITLE_PREFIX`] of them.
fn title_key(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .take(TITLE_PREFIX)
        .collect()
}

/// A DOI compared ignoring case, surrounding blanks and a resolver prefix.
fn doi_key(s: &str) -> Option<String> {
    let s = s.trim().to_ascii_lowercase();
    let s = [
        "https://doi.org/",
        "http://doi.org/",
        "https://dx.doi.org/",
        "doi:",
    ]
    .iter()
    .find_map(|p| s.strip_prefix(p))
    .unwrap_or(&s)
    .trim()
    .to_string();
    (!s.is_empty()).then_some(s)
}

/// The live record `live` is the extracted paper `ours`: titles agree and
/// the DOIs do not differ.
fn same_paper(ours: &Paper, live: &Paper) -> bool {
    let ours_t = title_key(&ours.title);
    if ours_t.is_empty() || ours_t != title_key(&live.title) {
        return false;
    }
    match (
        ours.doi.as_deref().and_then(doi_key),
        live.doi.as_deref().and_then(doi_key),
    ) {
        (Some(a), Some(b)) => a == b,
        _ => true,
    }
}

fn pmid_of(p: &Paper) -> Option<String> {
    p.pmid
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// The DOI as a safe Europe PMC phrase (no quote or control character).
fn searchable_doi(p: &Paper) -> Option<String> {
    p.doi
        .as_deref()
        .map(str::trim)
        .filter(|d| !d.is_empty() && !d.chars().any(|c| c == '"' || c.is_control()))
        .map(str::to_string)
}

/// The refs of `cited` that verify live, and how many lookups failed.
async fn verify_refs(
    ctx: &RunContext,
    lit: &Literature,
    cited: &BTreeSet<u8>,
    by_ref: &HashMap<u8, &Extracted>,
) -> (HashSet<u8>, usize) {
    let run_id = ctx.run_id;
    let mut valid = HashSet::new();
    let mut failed = 0usize;

    let mut by_pmid: Vec<(u8, String)> = Vec::new();
    let mut by_doi: Vec<u8> = Vec::new();
    for r in cited {
        let Some(e) = by_ref.get(r) else { continue };
        match pmid_of(&e.paper) {
            Some(pmid) => by_pmid.push((*r, pmid)),
            None => by_doi.push(*r),
        }
    }

    if !by_pmid.is_empty() {
        let pmids: Vec<String> = by_pmid.iter().map(|(_, p)| p.clone()).collect();
        let summaries = lit.pubmed.summaries(&pmids).await;
        let details = lit.pubmed.details(&pmids).await;
        match (summaries, details) {
            (Ok(summaries), Ok(details)) => {
                for (r, pmid) in &by_pmid {
                    let Some(live) = summaries
                        .iter()
                        .find(|p| p.pmid.as_deref() == Some(pmid.as_str()))
                    else {
                        continue;
                    };
                    // Absent from efetch: retraction unknown, so not shown.
                    let Some(rec) = details.get(pmid) else {
                        continue;
                    };
                    if live.retracted || rec.retracted {
                        continue;
                    }
                    if same_paper(&by_ref[r].paper, live) {
                        valid.insert(*r);
                    }
                }
            }
            _ => {
                tracing::warn!(%run_id, step = "check", refs = by_pmid.len(), "pubmed verification failed; refs removed");
                failed += by_pmid.len();
            }
        }
    }

    for r in by_doi {
        if ctx.cancel.is_cancelled() {
            break;
        }
        let Some(doi) = searchable_doi(&by_ref[&r].paper) else {
            continue;
        };
        let want = doi_key(&doi);
        match lit.epmc.search(&format!("DOI:\"{doi}\""), DOI_HITS).await {
            Ok(hits) => {
                if hits.papers.iter().any(|p| {
                    !p.retracted && want.is_some() && p.doi.as_deref().and_then(doi_key) == want
                }) {
                    valid.insert(r);
                }
            }
            Err(_) => {
                tracing::warn!(%run_id, step = "check", "europe pmc verification failed; ref removed");
                failed += 1;
            }
        }
    }
    (valid, failed)
}

// ---------- the step ----------

fn count(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

/// Check `draft` against the quotes of `extracted`, then verify its refs
/// live (see the module docs).
pub async fn check(
    ctx: &RunContext,
    llm: &AgentLlm,
    lit: &Literature,
    draft: Draft,
    extracted: &[Extracted],
) -> Result<Checked, StepError> {
    let run_id = ctx.run_id;
    let by_ref: HashMap<u8, &Extracted> = extracted.iter().map(|e| (e.ref_id, e)).collect();
    let claims = draft.bottom_line;
    let total = claims.len();
    let system = system();
    let call = llm.call(ctx, &llm.models.check, "check");

    let mut verdicts = vec![ClaimVerdict::Unsupported; total];
    let mut failed = 0usize;
    let mut stopped = 0usize;
    let mut start = 0;
    while start < total {
        if ctx.cancel.is_cancelled() {
            return Err(StepError::Cancelled);
        }
        let end = (start + CHECK_BATCH).min(total);
        let prompt = batch_prompt(start + 1, &claims[start..end], &by_ref);
        let reply: Value = match call.json(&system, &prompt, MAX_TOKENS).await {
            Ok(v) => v,
            Err(e @ (StepError::Llm | StepError::Parse)) => {
                tracing::warn!(%run_id, step = "check", error_kind = e.kind(), claims = end - start, "check batch failed; claims unsupported");
                failed += end - start;
                start = end;
                continue;
            }
            Err(StepError::OverBudget) => {
                tracing::info!(%run_id, step = "check", checked = start, "over budget; remaining claims unsupported");
                stopped = total - start;
                break;
            }
            Err(e) => return Err(e),
        };
        let Some(list) = verdict_list(reply) else {
            tracing::warn!(%run_id, step = "check", claims = end - start, "check reply held no verdict list; claims unsupported");
            failed += end - start;
            start = end;
            continue;
        };
        let mut seen = HashSet::new();
        for v in &list {
            let Some(i) = verdict_claim(v).filter(|i| (start + 1..=end).contains(i)) else {
                continue;
            };
            if seen.insert(i) {
                verdicts[i - 1] = ClaimVerdict::read(v.get("verdict"));
            }
        }
        start = end;
    }
    if ctx.cancel.is_cancelled() {
        return Err(StepError::Cancelled);
    }

    let kept: Vec<(Claim, ClaimVerdict)> = claims
        .into_iter()
        .zip(verdicts)
        .filter(|(_, v)| *v != ClaimVerdict::Unsupported)
        .collect();
    let cited: BTreeSet<u8> = kept
        .iter()
        .flat_map(|(c, _)| c.refs.iter().copied())
        .collect();
    let (valid, lookup_failed) = verify_refs(ctx, lit, &cited, &by_ref).await;
    if ctx.cancel.is_cancelled() {
        return Err(StepError::Cancelled);
    }

    let claims: Vec<(Claim, ClaimVerdict)> = kept
        .into_iter()
        .filter_map(|(mut c, v)| {
            c.refs.retain(|r| valid.contains(r));
            (!c.refs.is_empty()).then_some((c, v))
        })
        .collect();
    let supported = claims
        .iter()
        .filter(|(_, v)| *v == ClaimVerdict::Supported)
        .count();
    let partly = claims.len() - supported;
    let removed = total - claims.len();
    let refs_total = cited.len();
    let refs_verified = cited.iter().filter(|r| valid.contains(r)).count();
    tracing::info!(
        %run_id, step = "check", claims = total, supported, partly, removed,
        refs_total, refs_verified, failed, stopped, lookup_failed, "check done"
    );

    add_line(
        ctx,
        StepKind::Check,
        &format!(
            "Checked {total} claims: {supported} supported, {partly} partly, {removed} removed; {refs_verified}/{refs_total} references verified"
        ),
    )
    .await;
    let notes = [
        (
            failed,
            format!("Check failed for {failed} claims; they were removed"),
        ),
        (
            stopped,
            format!("Check stopped early; {stopped} claims could not be checked and were removed"),
        ),
        (
            lookup_failed,
            format!(
                "{lookup_failed} references could not be verified (database unavailable); they were removed"
            ),
        ),
    ];
    for (n, line) in notes {
        if n > 0 {
            add_line(ctx, StepKind::Note, &line).await;
        }
    }
    Ok(Checked {
        claims,
        removed: count(removed),
        refs_verified: count(refs_verified),
        refs_removed: count(refs_total - refs_verified),
    })
}
