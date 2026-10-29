//! The literature tools (Phase 3, T2). Server only.
//!
//! Five tools over the existing allowlisted clients ([`Literature`]) and the
//! paper cache. No other host can be reached: PubMed eutils (paced, with the
//! NCBI key when set) and Europe PMC's REST API are the only endpoints.
//!
//! | tool | input | result |
//! |---|---|---|
//! | `search_pubmed` | `{query, max_results ≤ 50}` | `{count, items:[{pmid, title, year, journal, pub_types}]}` |
//! | `search_europepmc` | `{query, max_results ≤ 50}` | the same, plus `pmcid`, `doi` |
//! | `get_abstracts` | `{pmids: [≤ 20]}` | `{papers:[{pmid, title, journal, year, abstract, pub_types, doi, pmcid, retracted}], not_found}` |
//! | `get_full_text` | `{pmcid}` | `{pmcid, sections:[{name, text}], truncated}` |
//! | `find_related` | `{pmid, max_results ≤ 30}` | like `search_pubmed` (`count` = neighbours returned) |
//!
//! * **Queries** pass the Phase 2 query check (`validate_query`: charset, no
//!   URLs or newlines, balanced, ≤ 1,000 characters) and the echo guard: a
//!   query holding eight consecutive words of the question is refused, so
//!   the question's free text never leaves for NCBI or Europe PMC (spec §5).
//! * **Abstracts** come from the cache first, then esummary + efetch; what
//!   efetch returned is written back to the cache (search results are not
//!   cached: a bare summary would shadow the abstract later).
//! * **Full text** is Europe PMC `fullTextXML` only (Ruling R-10a), sections
//!   in the order abstract, results, methods, discussion, cut from the end
//!   to [`FULL_TEXT_CHARS`] (R-11b's priority; the loop's per-result cap
//!   applies on top).
//! * Every result is JSON; the loop wraps it whole as `<source>` data.
//!   Paper text is never logged; logs carry the tool, counts and error kinds.

use std::collections::HashSet;
use std::sync::Arc;

use async_trait::async_trait;
use serde::Serialize;
use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;

use super::{Tool, ToolError, ToolRegistry};
use crate::agent::search::{lit_kind, validate_query};
use crate::agent::{Literature, contains_question};
use crate::lit::Paper;
use crate::lit::cache;
use crate::lit::http::LitError;

/// Most results a search tool returns.
pub const MAX_SEARCH_RESULTS: u64 = 50;
/// Most PMIDs one `get_abstracts` call takes.
pub const MAX_ABSTRACTS: usize = 20;
/// Most neighbours `find_related` returns.
pub const MAX_RELATED: u64 = 30;
/// Characters of full text one `get_full_text` call returns.
pub const FULL_TEXT_CHARS: usize = 28_000;

/// What the literature tools share: the clients, the cache and, for the
/// echo guard only, the question (never logged).
pub struct LitTools {
    lit: Arc<Literature>,
    pool: PgPool,
    run_id: Uuid,
    question: String,
}

impl LitTools {
    pub fn new(lit: Arc<Literature>, pool: PgPool, run_id: Uuid, question: &str) -> Arc<LitTools> {
        Arc::new(LitTools {
            lit,
            pool,
            run_id,
            question: question.to_string(),
        })
    }

    /// All five tools.
    pub fn registry(self: &Arc<Self>) -> ToolRegistry {
        self.registry_of(&[
            "search_pubmed",
            "search_europepmc",
            "get_abstracts",
            "get_full_text",
            "find_related",
        ])
    }

    /// The named tools only (e.g. Quick mode without full text). Unknown
    /// names are a programming error.
    pub fn registry_of(self: &Arc<Self>, names: &[&str]) -> ToolRegistry {
        let mut r = ToolRegistry::new();
        for n in names {
            let t: Arc<dyn Tool> = match *n {
                "search_pubmed" => Arc::new(SearchPubmed(self.clone())),
                "search_europepmc" => Arc::new(SearchEpmc(self.clone())),
                "get_abstracts" => Arc::new(GetAbstracts(self.clone())),
                "get_full_text" => Arc::new(GetFullText(self.clone())),
                "find_related" => Arc::new(FindRelated(self.clone())),
                other => panic!("no literature tool {other}"),
            };
            r = r.with(t);
        }
        r
    }

    fn failed(&self, tool: &'static str, e: &LitError) -> ToolError {
        tracing::warn!(run_id = %self.run_id, tool, error_kind = lit_kind(e), "literature tool failed");
        ToolError::Failed(match e {
            LitError::Http(404) | LitError::Parse => {
                "the database returned nothing usable for this input".to_string()
            }
            _ => {
                "the database could not be reached; try again later or use another tool".to_string()
            }
        })
    }

    fn check_query(&self, q: &str) -> Result<(), ToolError> {
        validate_query(q).map_err(|kind| {
            ToolError::Input(format!(
                "query rejected ({kind}): use 1 to 1000 characters of letters, digits, spaces and - \" ( ) [ ] * / : , . ' with balanced quotes and brackets, no URLs or newlines"
            ))
        })?;
        if contains_question(q, &self.question) {
            return Err(ToolError::Input(
                "query rejected (echo): it repeats the question's wording; use search terms and field tags instead".to_string(),
            ));
        }
        Ok(())
    }

    /// Summaries for `pmids`, cache first, in `pmids` order.
    async fn summaries(&self, pmids: &[String]) -> Result<Vec<Paper>, LitError> {
        let mut found = self.cached(pmids).await;
        let missing: Vec<String> = pmids
            .iter()
            .filter(|p| !found.iter().any(|f| f.pmid.as_ref() == Some(*p)))
            .cloned()
            .collect();
        if !missing.is_empty() {
            found.extend(self.lit.pubmed.summaries(&missing).await?);
        }
        Ok(pmids
            .iter()
            .filter_map(|id| found.iter().find(|p| p.pmid.as_ref() == Some(id)).cloned())
            .collect())
    }

    /// Fresh cached papers (a cache error is a miss).
    async fn cached(&self, pmids: &[String]) -> Vec<Paper> {
        match cache::get_many(&self.pool, pmids).await {
            Ok(p) => p,
            Err(_) => {
                tracing::warn!(run_id = %self.run_id, "paper cache read failed; treated as a miss");
                Vec::new()
            }
        }
    }
}

// ---------- input helpers ----------

fn object(input: &Value) -> Result<&serde_json::Map<String, Value>, ToolError> {
    input
        .as_object()
        .ok_or_else(|| ToolError::Input("input must be a JSON object".to_string()))
}

fn string_field<'a>(input: &'a Value, key: &str) -> Result<&'a str, ToolError> {
    object(input)?
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| ToolError::Input(format!("`{key}` must be a non-empty string")))
}

fn count_field(input: &Value, key: &str, max: u64) -> Result<u64, ToolError> {
    match object(input)?.get(key).and_then(Value::as_u64) {
        Some(n) if (1..=max).contains(&n) => Ok(n),
        _ => Err(ToolError::Input(format!(
            "`{key}` must be a whole number from 1 to {max}"
        ))),
    }
}

/// A PMID: 1 to 9 ASCII digits.
fn valid_pmid(s: &str) -> bool {
    (1..=9).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_digit())
}

/// `PMC` + 1 to 10 digits, upper-cased.
fn valid_pmcid(s: &str) -> Option<String> {
    let up = s.trim().to_ascii_uppercase();
    let digits = up.strip_prefix("PMC")?;
    ((1..=10).contains(&digits.len()) && digits.bytes().all(|b| b.is_ascii_digit())).then_some(up)
}

fn query_schema(max: u64, what: &str) -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "query": {"type": "string", "description": what},
            "max_results": {"type": "integer", "description": format!("How many records to return, 1 to {max}.")}
        },
        "required": ["query", "max_results"]
    })
}

// ---------- result shapes ----------

#[derive(Serialize)]
struct Item {
    pmid: Option<String>,
    title: String,
    year: Option<i32>,
    journal: Option<String>,
    pub_types: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pmcid: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    doi: Option<String>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    retracted: bool,
}

impl Item {
    fn of(p: Paper, ids: bool) -> Item {
        Item {
            pmid: p.pmid,
            title: p.title,
            year: p.year,
            journal: p.journal,
            pub_types: p.pub_types,
            pmcid: if ids { p.pmcid } else { None },
            doi: if ids { p.doi } else { None },
            retracted: p.retracted,
        }
    }
}

fn to_json<T: Serialize>(v: &T) -> Result<String, ToolError> {
    serde_json::to_string(v).map_err(|_| ToolError::Failed("internal error".to_string()))
}

// ---------- the tools ----------

struct SearchPubmed(Arc<LitTools>);

#[async_trait]
impl Tool for SearchPubmed {
    fn name(&self) -> &'static str {
        "search_pubmed"
    }
    fn description(&self) -> &'static str {
        "Search PubMed (relevance order). Use PubMed syntax: field tags such as [tiab], [mh], [pt], [dp], AND/OR/NOT and quoted phrases. Returns the total match count and records with pmid, title, year, journal and publication types. No abstracts: use get_abstracts for those."
    }
    fn input_schema(&self) -> Value {
        query_schema(MAX_SEARCH_RESULTS, "A PubMed query.")
    }
    async fn run(&self, input: &Value) -> Result<String, ToolError> {
        let t = &self.0;
        let query = string_field(input, "query")?;
        let max = count_field(input, "max_results", MAX_SEARCH_RESULTS)?;
        t.check_query(query)?;
        let hits = t
            .lit
            .pubmed
            .search(query, max as u16)
            .await
            .map_err(|e| t.failed(self.name(), &e))?;
        let papers = t
            .summaries(&hits.pmids)
            .await
            .map_err(|e| t.failed(self.name(), &e))?;
        tracing::info!(run_id = %t.run_id, tool = self.name(), count = hits.count, items = papers.len(), "literature tool done");
        let items: Vec<Item> = papers.into_iter().map(|p| Item::of(p, false)).collect();
        to_json(&json!({"count": hits.count, "items": items}))
    }
}

struct SearchEpmc(Arc<LitTools>);

#[async_trait]
impl Tool for SearchEpmc {
    fn name(&self) -> &'static str {
        "search_europepmc"
    }
    fn description(&self) -> &'static str {
        "Search Europe PMC (relevance order). Use Europe PMC syntax, e.g. TITLE:\"...\", ABSTRACT:\"...\", PUB_TYPE:\"randomized controlled trial\", PUB_YEAR:[2015 TO 2026], AND/OR. Finds records PubMed may miss (preprints, PMC full texts). Returns the total match count and records with pmid (if any), pmcid, doi, title, year, journal and publication types."
    }
    fn input_schema(&self) -> Value {
        query_schema(MAX_SEARCH_RESULTS, "A Europe PMC query.")
    }
    async fn run(&self, input: &Value) -> Result<String, ToolError> {
        let t = &self.0;
        let query = string_field(input, "query")?;
        let max = count_field(input, "max_results", MAX_SEARCH_RESULTS)?;
        t.check_query(query)?;
        let hits = t
            .lit
            .epmc
            .search(query, max as u16)
            .await
            .map_err(|e| t.failed(self.name(), &e))?;
        tracing::info!(run_id = %t.run_id, tool = self.name(), count = hits.count, items = hits.papers.len(), "literature tool done");
        let items: Vec<Item> = hits
            .papers
            .into_iter()
            .take(max as usize)
            .map(|p| Item::of(p, true))
            .collect();
        to_json(&json!({"count": hits.count, "items": items}))
    }
}

struct GetAbstracts(Arc<LitTools>);

#[derive(Serialize)]
struct AbstractRecord {
    pmid: String,
    title: String,
    journal: Option<String>,
    year: Option<i32>,
    #[serde(rename = "abstract")]
    abstract_text: Option<String>,
    pub_types: Vec<String>,
    doi: Option<String>,
    pmcid: Option<String>,
    retracted: bool,
}

#[async_trait]
impl Tool for GetAbstracts {
    fn name(&self) -> &'static str {
        "get_abstracts"
    }
    fn description(&self) -> &'static str {
        "Fetch abstracts for up to 20 PubMed ids. Returns per paper: pmid, title, journal, year, abstract (null if PubMed has none), publication types, doi, pmcid (when the paper is in PMC; then get_full_text may work) and whether it is retracted. Ids PubMed does not know are listed in not_found."
    }
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "additionalProperties": false,
            "properties": {
                "pmids": {"type": "array", "items": {"type": "string"}, "description": "1 to 20 PubMed ids (digits only)."}
            },
            "required": ["pmids"]
        })
    }
    async fn run(&self, input: &Value) -> Result<String, ToolError> {
        let t = &self.0;
        let bad = || {
            ToolError::Input(
                "`pmids` must be a list of 1 to 20 PubMed ids (digits only)".to_string(),
            )
        };
        let list = object(input)?
            .get("pmids")
            .and_then(Value::as_array)
            .ok_or_else(bad)?;
        if list.is_empty() || list.len() > MAX_ABSTRACTS {
            return Err(bad());
        }
        let mut pmids: Vec<String> = Vec::new();
        for v in list {
            let id = v
                .as_str()
                .map(str::trim)
                .filter(|s| valid_pmid(s))
                .ok_or_else(bad)?;
            if !pmids.iter().any(|p| p == id) {
                pmids.push(id.to_string());
            }
        }

        let mut papers = t.cached(&pmids).await;
        let cached: HashSet<String> = papers.iter().filter_map(|p| p.pmid.clone()).collect();
        let missing: Vec<String> = pmids
            .iter()
            .filter(|p| !cached.contains(*p))
            .cloned()
            .collect();
        if !missing.is_empty() {
            let summaries = match t.lit.pubmed.summaries(&missing).await {
                Ok(s) => s,
                Err(e) if papers.is_empty() => return Err(t.failed(self.name(), &e)),
                Err(e) => {
                    let _ = t.failed(self.name(), &e);
                    Vec::new()
                }
            };
            // Only the ids asked for (a reply may carry others).
            let summaries: Vec<Paper> = summaries
                .into_iter()
                .filter(|p| p.pmid.as_ref().is_some_and(|id| missing.contains(id)))
                .collect();
            if !summaries.is_empty() {
                let ids: Vec<String> = summaries.iter().filter_map(|p| p.pmid.clone()).collect();
                match t.lit.pubmed.details(&ids).await {
                    Ok(details) => {
                        let mut fetched = Vec::new();
                        for mut p in summaries {
                            if let Some(rec) = p.pmid.as_ref().and_then(|id| details.get(id)) {
                                if p.abstract_text.is_none() {
                                    p.abstract_text.clone_from(&rec.abstract_text);
                                }
                                if p.pub_types.is_empty() {
                                    p.pub_types.clone_from(&rec.pub_types);
                                }
                                p.retracted |= rec.retracted;
                                fetched.push(p.clone());
                            }
                            papers.push(p);
                        }
                        if cache::put_many(&t.pool, &fetched).await.is_err() {
                            tracing::warn!(run_id = %t.run_id, tool = self.name(), "paper cache write failed");
                        }
                    }
                    Err(e) => {
                        // Summaries without abstracts; not cached.
                        let _ = t.failed(self.name(), &e);
                        papers.extend(summaries);
                    }
                }
            }
        }

        let mut out = Vec::new();
        let mut not_found = Vec::new();
        for id in &pmids {
            match papers.iter().find(|p| p.pmid.as_ref() == Some(id)) {
                Some(p) => out.push(AbstractRecord {
                    pmid: id.clone(),
                    title: p.title.clone(),
                    journal: p.journal.clone(),
                    year: p.year,
                    abstract_text: p.abstract_text.clone(),
                    pub_types: p.pub_types.clone(),
                    doi: p.doi.clone(),
                    pmcid: p.pmcid.clone(),
                    retracted: p.retracted,
                }),
                None => not_found.push(id.clone()),
            }
        }
        tracing::info!(run_id = %t.run_id, tool = self.name(), asked = pmids.len(), cached = cached.len(), returned = out.len(), "literature tool done");
        to_json(&json!({"papers": out, "not_found": not_found}))
    }
}

struct GetFullText(Arc<LitTools>);

/// Section order for the agent: results before methods; discussion is cut
/// first (R-11b).
const SECTION_ORDER: [&str; 4] = ["abstract", "results", "methods", "discussion"];

#[async_trait]
impl Tool for GetFullText {
    fn name(&self) -> &'static str {
        "get_full_text"
    }
    fn description(&self) -> &'static str {
        "Fetch the open-access full text of a paper in PubMed Central by its pmcid (e.g. PMC1234567, from get_abstracts or search_europepmc). Returns the abstract, results, methods and discussion sections, cut to about 28,000 characters (discussion first). Only open-access PMC papers have full text."
    }
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "additionalProperties": false,
            "properties": {
                "pmcid": {"type": "string", "description": "A PMC id: PMC followed by digits."}
            },
            "required": ["pmcid"]
        })
    }
    async fn run(&self, input: &Value) -> Result<String, ToolError> {
        let t = &self.0;
        let pmcid = valid_pmcid(string_field(input, "pmcid")?).ok_or_else(|| {
            ToolError::Input("`pmcid` must be PMC followed by 1 to 10 digits".to_string())
        })?;
        let ft = t
            .lit
            .epmc
            .full_text(&pmcid)
            .await
            .map_err(|e| t.failed(self.name(), &e))?;
        if ft.sections.is_empty() {
            return Err(ToolError::Failed(
                "no full text sections found for this pmcid; use the abstract".to_string(),
            ));
        }
        let mut sections: Vec<(String, String)> = Vec::new();
        for name in SECTION_ORDER {
            for (n, text) in &ft.sections {
                if n == name {
                    sections.push((n.clone(), text.clone()));
                }
            }
        }
        let mut left = FULL_TEXT_CHARS;
        let mut truncated = false;
        let mut out = Vec::new();
        for (name, text) in sections {
            if left == 0 {
                truncated = true;
                break;
            }
            let n = text.chars().count();
            let text = if n > left {
                truncated = true;
                text.chars().take(left).collect()
            } else {
                text
            };
            left -= text.chars().count();
            out.push(json!({"name": name, "text": text}));
        }
        tracing::info!(run_id = %t.run_id, tool = self.name(), sections = out.len(), truncated, "literature tool done");
        to_json(&json!({"pmcid": pmcid, "sections": out, "truncated": truncated}))
    }
}

struct FindRelated(Arc<LitTools>);

#[async_trait]
impl Tool for FindRelated {
    fn name(&self) -> &'static str {
        "find_related"
    }
    fn description(&self) -> &'static str {
        "PubMed's similar articles for one PubMed id (best match first): a quick way to find trials and reviews close to a key paper. Returns records with pmid, title, year, journal and publication types."
    }
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "additionalProperties": false,
            "properties": {
                "pmid": {"type": "string", "description": "One PubMed id (digits only)."},
                "max_results": {"type": "integer", "description": format!("How many related records to return, 1 to {MAX_RELATED}.")}
            },
            "required": ["pmid", "max_results"]
        })
    }
    async fn run(&self, input: &Value) -> Result<String, ToolError> {
        let t = &self.0;
        let pmid = string_field(input, "pmid")?;
        if !valid_pmid(pmid) {
            return Err(ToolError::Input(
                "`pmid` must be a PubMed id (1 to 9 digits)".to_string(),
            ));
        }
        let max = count_field(input, "max_results", MAX_RELATED)?;
        let ids = t
            .lit
            .pubmed
            .related(pmid, max as usize)
            .await
            .map_err(|e| t.failed(self.name(), &e))?;
        let papers = if ids.is_empty() {
            Vec::new()
        } else {
            t.summaries(&ids)
                .await
                .map_err(|e| t.failed(self.name(), &e))?
        };
        tracing::info!(run_id = %t.run_id, tool = self.name(), items = papers.len(), "literature tool done");
        let items: Vec<Item> = papers.into_iter().map(|p| Item::of(p, false)).collect();
        to_json(&json!({"count": items.len(), "items": items}))
    }
}
