//! Retrieve (step 4): the text each used paper is read from. Server only.
//!
//! The top `settings.use_n` included papers are used. In Quick mode
//! (`settings.full_text == false`) nothing is fetched: each paper is read
//! from its abstract, or its title when it has none.
//!
//! Otherwise, per paper, in order (Ruling R-10a):
//!
//! 1. A PMCID: Europe PMC `fullTextXML` ([`EuropePmc::full_text`], on the
//!    allowlisted base host, so no open-access grant is needed). This is the
//!    only source of [`PaperText::FullText`] in Phase 2.
//! 2. An `oa_url` that is not a PDF: nothing is fetched. HTML landing pages
//!    are not parsed in Phase 2, so the paper is abstract-only.
//! 3. A PDF `oa_url`: abstract-only (PDFs are not parsed).
//!
//! Unpaywall is not called here; its only Phase 2 role is the OA status of a
//! PMCID-less paper in the trail. Every full-text failure falls back to the
//! abstract (or the title). Cancel and the wrap-up point are checked between
//! papers: once wrap-up is reached nothing more is fetched and the remaining
//! papers are read from their abstracts.
//!
//! Full texts are held in memory for Extract only, never stored.
//!
//! The "Reading K papers (…)" line is a Retrieve line in Focused mode; Quick
//! mode has no Retrieve step, so there it is written under Extract.
//!
//! Privacy: nothing here logs titles, identifiers or text; only the run id,
//! counts and fixed kinds.
//!
//! [`EuropePmc::full_text`]: crate::lit::europepmc::EuropePmc::full_text

use std::time::Instant;

use super::llm::StepError;
use super::screen::{EvidenceLevel, Screened};
use super::search::lit_kind;
use super::{Literature, RunContext, add_line};
use crate::lit::Paper;
use crate::lit::europepmc::FullText;
use crate::runs::StepKind;

/// The text a used paper is read from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PaperText {
    /// Europe PMC open-access sections (abstract, methods, results, discussion).
    FullText(FullText),
    /// The abstract only.
    AbstractOnly(String),
    /// No abstract: the title is all there is.
    TitleOnly,
}

/// One paper going on to Extract.
#[derive(Debug, Clone, PartialEq)]
pub struct UsedPaper {
    pub paper: Paper,
    pub level: EvidenceLevel,
    pub text: PaperText,
}

/// The abstract, or the title when there is no abstract.
fn fallback(p: &Paper) -> PaperText {
    match p.abstract_text.as_deref().map(str::trim) {
        Some(a) if !a.is_empty() => PaperText::AbstractOnly(a.to_string()),
        _ => PaperText::TitleOnly,
    }
}

/// Pick the top papers and their text (see the module docs). Only
/// `Cancelled` is an error.
pub async fn retrieve(
    ctx: &RunContext,
    lit: &Literature,
    screened: Screened,
) -> Result<Vec<UsedPaper>, StepError> {
    let run_id = ctx.run_id;
    let use_n = usize::from(ctx.settings.use_n);
    let mut fetching = ctx.settings.full_text;
    let mut used = Vec::new();

    for (paper, level, _reason) in screened.included.into_iter().take(use_n) {
        if ctx.cancel.is_cancelled() {
            return Err(StepError::Cancelled);
        }
        if fetching && ctx.budget.should_wrap_up(Instant::now()) {
            tracing::info!(%run_id, step = "retrieve", fetched = used.len(), "wrap-up reached; no more full texts");
            fetching = false;
        }
        let pmcid = paper.pmcid.as_deref().filter(|_| fetching);
        let text = match pmcid {
            Some(id) => match lit.epmc.full_text(id).await {
                Ok(ft) if !ft.sections.is_empty() => PaperText::FullText(ft),
                Ok(_) => fallback(&paper),
                Err(e) => {
                    tracing::info!(%run_id, step = "retrieve", error_kind = lit_kind(&e), "full text unavailable; using the abstract");
                    fallback(&paper)
                }
            },
            // No PMCID: an HTML landing page or a PDF is not parsed in Phase 2.
            None => fallback(&paper),
        };
        used.push(UsedPaper { paper, level, text });
    }

    let count = |f: fn(&PaperText) -> bool| used.iter().filter(|u| f(&u.text)).count();
    let full = count(|t| matches!(t, PaperText::FullText(_)));
    let abs = count(|t| matches!(t, PaperText::AbstractOnly(_)));
    let title = count(|t| matches!(t, PaperText::TitleOnly));
    let mut line = format!(
        "Reading {} papers ({full} full text, {abs} abstract only",
        used.len()
    );
    if title > 0 {
        line.push_str(&format!(", {title} title only"));
    }
    line.push(')');
    // Quick mode has no Retrieve step: its reading line belongs to Extract.
    let kind = if ctx.settings.full_text {
        StepKind::Retrieve
    } else {
        StepKind::Extract
    };
    add_line(ctx, kind, &line).await;
    Ok(used)
}
