//! Literature tools: the allowlisted HTTP client and the paper model.
//!
//! This whole module is `ssr`-only. The answer views (Task 13) define their own
//! plain view types, so nothing here has to compile for the browser build.

pub mod cache;
pub mod europepmc;
pub mod http;
pub mod pubmed;
pub mod unpaywall;

use serde::{Deserialize, Serialize};

/// Which database(s) a record came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PaperSource {
    PubMed,
    EuropePmc,
    Both,
}

/// One bibliographic record (public data only; no full text).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Paper {
    pub pmid: Option<String>,
    pub pmcid: Option<String>,
    pub doi: Option<String>,
    pub title: String,
    pub authors: Vec<String>,
    pub journal: Option<String>,
    pub year: Option<i32>,
    pub pub_types: Vec<String>,
    pub abstract_text: Option<String>,
    /// Open-access full-text URL, only ever taken from a Europe PMC or Unpaywall response.
    pub oa_url: Option<String>,
    pub retracted: bool,
    pub source: PaperSource,
}

/// Identity of a paper across databases.
pub struct PaperId;

impl PaperId {
    /// The PMID if present, else the lower-cased DOI. A record with neither
    /// (rare: preprints without a DOI) falls back to `title:<lower-cased title>`
    /// so it still de-duplicates against itself.
    pub fn key(p: &Paper) -> String {
        if let Some(pmid) = p.pmid.as_deref().filter(|s| !s.trim().is_empty()) {
            return pmid.trim().to_string();
        }
        if let Some(doi) = p.doi.as_deref().filter(|s| !s.trim().is_empty()) {
            return doi.trim().to_lowercase();
        }
        format!("title:{}", p.title.trim().to_lowercase())
    }
}
