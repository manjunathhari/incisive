//! PubMed through NCBI E-utilities: esearch (ids), esummary (bibliographic
//! records) and efetch (abstracts and retraction notices).
//!
//! * Every request carries `tool=incisive`, plus `email` when configured and
//!   `api_key` when set. The key lives in a [`SecretString`], is excluded from
//!   `Debug`, and is never logged: this module does not log URLs at all, and the
//!   HTTP client logs refusals by host only.
//! * Requests are paced at 3/s (10/s with a key), the NCBI limits. The gap is
//!   injectable ([`PubMed::with_min_interval`]) so tests do not sleep.
//! * A record is `retracted` when its publication types contain
//!   "Retracted Publication", or its efetch record carries a
//!   `CommentsCorrections` entry with `RefType="RetractionIn"`.

use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use quick_xml::Reader;
use quick_xml::events::Event;
use secrecy::{ExposeSecret, SecretString};
use serde::Deserialize;
use tokio::sync::Mutex;
use tokio::time::Instant;
use url::Url;

use super::http::{AllowlistedClient, LitError};
use super::{Paper, PaperSource};

const EUTILS: &str = "https://eutils.ncbi.nlm.nih.gov/entrez/eutils/";
const TOOL: &str = "incisive";
const RETRACTED_TYPE: &str = "Retracted Publication";
/// ids per esummary request (keeps the URL short).
const SUMMARY_BATCH: usize = 100;
/// ids per efetch request (responses carry full records).
const FETCH_BATCH: usize = 50;
/// Upper bound on esearch `retmax`.
const MAX_HITS: u16 = 1000;

/// NCBI limits: 3 requests/s without a key, 10/s with one.
pub fn default_min_interval(has_key: bool) -> Duration {
    if has_key {
        Duration::from_millis(100)
    } else {
        Duration::from_millis(334)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchHits {
    /// Total matches in PubMed (may exceed `pmids.len()`).
    pub count: u64,
    pub pmids: Vec<String>,
}

/// What efetch tells us about one record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FetchedRecord {
    pub pmid: String,
    /// Structured sections joined as `LABEL: text`, blank line between sections.
    pub abstract_text: Option<String>,
    pub pub_types: Vec<String>,
    /// Pub type "Retracted Publication" or a `RetractionIn` comment.
    pub retracted: bool,
}

/// Spaces requests at least `gap` apart. Waiters queue on the mutex, so
/// concurrent callers share one schedule.
///
/// The schedule is **per instance** (final review M-8): each Cloud Run
/// instance paces its own requests, so with `--max-instances=3` the service
/// as a whole can send up to three times NCBI's limit (9 a second without a
/// key, 30 with one, against NCBI's 3 and 10). That needs concurrent runs on
/// several instances at once, which is unlikely at current use, and NCBI's
/// 429s are retried with backoff. The NCBI API key raises the limit.
struct Pacer {
    gap: Duration,
    next: Mutex<Instant>,
}

impl Pacer {
    fn new(gap: Duration) -> Self {
        Self {
            gap,
            next: Mutex::new(Instant::now()),
        }
    }

    async fn wait(&self) {
        if self.gap.is_zero() {
            return;
        }
        let mut next = self.next.lock().await;
        let now = Instant::now();
        if *next > now {
            tokio::time::sleep_until(*next).await;
        }
        *next = Instant::now().max(*next) + self.gap;
    }
}

/// Paces every network attempt to NCBI (retries and redirect hops included)
/// through the client's per-attempt hook. Other hosts are not delayed.
fn paced(client: AllowlistedClient, pacer: &Arc<Pacer>) -> AllowlistedClient {
    let pacer = Arc::clone(pacer);
    client.with_before_attempt(Arc::new(move |host: &str| {
        let pacer = Arc::clone(&pacer);
        let ncbi = host.eq_ignore_ascii_case("eutils.ncbi.nlm.nih.gov");
        Box::pin(async move {
            if ncbi {
                pacer.wait().await;
            }
        })
    }))
}

pub struct PubMed {
    client: AllowlistedClient,
    api_key: Option<SecretString>,
    email: Option<String>,
    pacer: Arc<Pacer>,
}

impl fmt::Debug for PubMed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PubMed")
            .field("api_key", &self.api_key.as_ref().map(|_| "[REDACTED]"))
            .field("email", &self.email.is_some())
            .field("min_interval", &self.pacer.gap)
            .finish_non_exhaustive()
    }
}

impl PubMed {
    pub fn new(client: AllowlistedClient, api_key: Option<SecretString>) -> Self {
        let gap = default_min_interval(api_key.is_some());
        let pacer = Arc::new(Pacer::new(gap));
        Self {
            client: paced(client, &pacer),
            api_key,
            email: None,
            pacer,
        }
    }

    /// Contact address NCBI asks for (`email` query parameter).
    pub fn with_email(mut self, email: Option<String>) -> Self {
        self.email = email.filter(|e| !e.trim().is_empty());
        self
    }

    /// Overrides the pacing gap (tests).
    pub fn with_min_interval(mut self, gap: Duration) -> Self {
        self.pacer = Arc::new(Pacer::new(gap));
        self.client = paced(self.client, &self.pacer);
        self
    }

    fn url(&self, endpoint: &str, params: &[(&str, &str)]) -> String {
        let mut u = Url::parse(EUTILS)
            .and_then(|b| b.join(endpoint))
            .expect("static eutils url");
        {
            let mut q = u.query_pairs_mut();
            q.append_pair("db", "pubmed");
            for (k, v) in params {
                q.append_pair(k, v);
            }
            q.append_pair("tool", TOOL);
            if let Some(e) = &self.email {
                q.append_pair("email", e);
            }
            if let Some(k) = &self.api_key {
                q.append_pair("api_key", k.expose_secret());
            }
        }
        u.into()
    }

    /// esearch, relevance-sorted. `max` is clamped to 1..=1000.
    pub async fn search(&self, query: &str, max: u16) -> Result<SearchHits, LitError> {
        let retmax = max.clamp(1, MAX_HITS).to_string();
        let url = self.url(
            "esearch.fcgi",
            &[
                ("term", query),
                ("retmax", &retmax),
                ("sort", "relevance"),
                ("retmode", "json"),
            ],
        );
        let body: ESearch = self.client.get_json(&url).await?;
        let r = body.esearchresult.ok_or(LitError::Parse)?;
        if r.error.is_some() {
            return Err(LitError::Parse);
        }
        let count = r
            .count
            .as_deref()
            .and_then(|c| c.trim().parse().ok())
            .ok_or(LitError::Parse)?;
        Ok(SearchHits {
            count,
            pmids: r.idlist.unwrap_or_default(),
        })
    }

    /// esummary. Ids PubMed does not know are absent from the result.
    pub async fn summaries(&self, pmids: &[String]) -> Result<Vec<Paper>, LitError> {
        let mut out = Vec::new();
        for chunk in pmids.chunks(SUMMARY_BATCH) {
            let joined = chunk.join(",");
            let url = self.url("esummary.fcgi", &[("id", &joined), ("retmode", "json")]);
            let body: serde_json::Value = self.client.get_json(&url).await?;
            out.extend(parse_esummary(&body, chunk)?);
        }
        Ok(out)
    }

    /// efetch details per PMID. Ids PubMed does not know are absent.
    pub async fn details(
        &self,
        pmids: &[String],
    ) -> Result<HashMap<String, FetchedRecord>, LitError> {
        let mut out = HashMap::new();
        for chunk in pmids.chunks(FETCH_BATCH) {
            let joined = chunk.join(",");
            let url = self.url(
                "efetch.fcgi",
                &[("id", &joined), ("rettype", "abstract"), ("retmode", "xml")],
            );
            let xml = self.client.get_text(&url).await?;
            for rec in parse_efetch(&xml)? {
                out.insert(rec.pmid.clone(), rec);
            }
        }
        Ok(out)
    }

    /// efetch abstracts by PMID. Records without an abstract, and unknown ids,
    /// are absent.
    pub async fn abstracts(&self, pmids: &[String]) -> Result<HashMap<String, String>, LitError> {
        Ok(self
            .details(pmids)
            .await?
            .into_iter()
            .filter_map(|(k, r)| r.abstract_text.map(|a| (k, a)))
            .collect())
    }
}

impl PubMed {
    /// PubMed's related articles for `pmid` (elink `cmd=neighbor_score`,
    /// link `pubmed_pubmed`, on the allowlisted eutils host, paced like every
    /// eutils call), best first, without `pmid` itself, at most `max`. A
    /// record with no neighbours gives an empty list; an unknown id or an
    /// error reply is [`LitError::Parse`].
    pub async fn related(&self, pmid: &str, max: usize) -> Result<Vec<String>, LitError> {
        let url = self.url(
            "elink.fcgi",
            &[
                ("dbfrom", "pubmed"),
                ("id", pmid),
                ("cmd", "neighbor_score"),
                ("linkname", "pubmed_pubmed"),
                ("retmode", "json"),
            ],
        );
        let body: serde_json::Value = self.client.get_json(&url).await?;
        parse_elink(&body, pmid, max)
    }
}

/// Neighbour ids from an elink JSON reply (see [`PubMed::related`]). Links
/// are objects `{"id", "score"}` (`neighbor_score`) or plain id strings
/// (`neighbor`); both are read, in the order given (NCBI sorts by score).
/// Only digit ids are kept.
pub fn parse_elink(
    body: &serde_json::Value,
    pmid: &str,
    max: usize,
) -> Result<Vec<String>, LitError> {
    if body.get("ERROR").is_some() {
        return Err(LitError::Parse);
    }
    let set = body
        .get("linksets")
        .and_then(|l| l.as_array())
        .and_then(|a| a.first())
        .ok_or(LitError::Parse)?;
    if set.get("ERROR").is_some() {
        return Err(LitError::Parse);
    }
    let dbs: &[serde_json::Value] = set
        .get("linksetdbs")
        .and_then(|d| d.as_array())
        .map_or(&[], Vec::as_slice);
    let db = dbs
        .iter()
        .find(|d| d.get("linkname").and_then(|n| n.as_str()) == Some("pubmed_pubmed"))
        .or_else(|| dbs.first());
    let Some(links) = db.and_then(|d| d.get("links")).and_then(|l| l.as_array()) else {
        return Ok(Vec::new());
    };
    let mut out: Vec<String> = Vec::new();
    for link in links {
        let id = match link {
            serde_json::Value::String(s) => Some(s.as_str()),
            serde_json::Value::Object(o) => o.get("id").and_then(|v| v.as_str()),
            _ => None,
        };
        let Some(id) = id.map(str::trim) else {
            continue;
        };
        if id.is_empty()
            || !id.bytes().all(|b| b.is_ascii_digit())
            || id == pmid
            || out.iter().any(|o| o == id)
        {
            continue;
        }
        out.push(id.to_string());
        if out.len() >= max {
            break;
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// esearch / esummary
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct ESearch {
    esearchresult: Option<ESearchResult>,
}

#[derive(Deserialize)]
struct ESearchResult {
    count: Option<String>,
    idlist: Option<Vec<String>>,
    #[serde(rename = "ERROR")]
    error: Option<serde_json::Value>,
}

/// Papers for the requested ids, in the order NCBI listed them. Entries with an
/// `error` field (unknown ids) or no usable title are skipped.
fn parse_esummary(body: &serde_json::Value, requested: &[String]) -> Result<Vec<Paper>, LitError> {
    let result = body
        .get("result")
        .and_then(|r| r.as_object())
        .ok_or(LitError::Parse)?;
    let uids: Vec<String> = match result.get("uids").and_then(|u| u.as_array()) {
        Some(a) => a
            .iter()
            .filter_map(|v| v.as_str().map(str::to_string))
            .collect(),
        // Some error replies have no `uids`; fall back to what was asked.
        None => requested.to_vec(),
    };
    let mut papers = Vec::new();
    for uid in uids {
        let Some(rec) = result.get(&uid).and_then(|r| r.as_object()) else {
            continue;
        };
        if rec.contains_key("error") {
            continue;
        }
        let text = |k: &str| {
            rec.get(k)
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
        };
        let Some(title) = text("title") else { continue };
        let pub_types: Vec<String> = rec
            .get("pubtype")
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        let article_id = |kind: &str| -> Option<String> {
            rec.get("articleids")?
                .as_array()?
                .iter()
                .find(|a| a.get("idtype").and_then(|t| t.as_str()) == Some(kind))
                .and_then(|a| a.get("value")?.as_str())
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
        };
        let authors = rec
            .get("authors")
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.get("name")?.as_str())
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect()
            })
            .unwrap_or_default();
        let year = text("pubdate")
            .and_then(leading_year)
            .or_else(|| text("epubdate").and_then(leading_year));
        let retracted = pub_types.iter().any(|t| is_retracted_type(t));
        papers.push(Paper {
            pmid: Some(uid.clone()),
            pmcid: article_id("pmc")
                .into_iter()
                .chain(article_id("pmcid"))
                .find_map(|v| extract_pmcid(&v)),
            doi: article_id("doi"),
            title: title.to_string(),
            authors,
            journal: text("fulljournalname")
                .or_else(|| text("source"))
                .map(str::to_string),
            year,
            pub_types,
            abstract_text: None,
            oa_url: None,
            oa_is_pdf: false,
            retracted,
            source: PaperSource::PubMed,
        });
    }
    Ok(papers)
}

/// "PMC123" or the older "pmc-id: PMC123;" form -> "PMC123".
fn extract_pmcid(v: &str) -> Option<String> {
    let start = v.find("PMC")?;
    let id: String = v[start..]
        .chars()
        .enumerate()
        .take_while(|(i, c)| *i < 3 || c.is_ascii_digit())
        .map(|(_, c)| c)
        .collect();
    (id.len() > 3).then_some(id)
}

fn leading_year(s: &str) -> Option<i32> {
    let digits: String = s.chars().take_while(char::is_ascii_digit).collect();
    (digits.len() == 4).then(|| digits.parse().ok()).flatten()
}

fn is_retracted_type(t: &str) -> bool {
    t.trim().eq_ignore_ascii_case(RETRACTED_TYPE)
}

// ---------------------------------------------------------------------------
// efetch XML
// ---------------------------------------------------------------------------

/// Which element's character data we are collecting.
#[derive(PartialEq)]
enum Capture {
    None,
    Pmid,
    PubType,
    AbstractText,
}

#[derive(Default)]
struct Building {
    pmid: Option<String>,
    sections: Vec<(Option<String>, String)>,
    pub_types: Vec<String>,
    retraction_comment: bool,
}

/// Parses an efetch `PubmedArticleSet`. Never panics; anything malformed is
/// [`LitError::Parse`].
pub fn parse_efetch(xml: &str) -> Result<Vec<FetchedRecord>, LitError> {
    let mut reader = Reader::from_str(xml);
    let mut stack: Vec<String> = Vec::new();
    let mut records = Vec::new();
    let mut cur: Option<Building> = None;
    let mut capture = Capture::None;
    let mut text = String::new();
    let mut label: Option<String> = None;
    let mut saw_root = false;
    let mut skipped = 0usize;

    loop {
        let ev = reader.read_event().map_err(|_| LitError::Parse)?;
        let (start, empty) = match &ev {
            Event::Start(e) => (Some(e), false),
            Event::Empty(e) => (Some(e), true),
            _ => (None, false),
        };
        if let Some(e) = start {
            saw_root = true;
            let name = e.local_name().as_ref().to_string();
            let parent = stack.last().map(String::as_str);
            match name.as_str() {
                "PubmedArticle" | "PubmedBookArticle" if parent == Some("PubmedArticleSet") => {
                    cur = Some(Building::default());
                }
                "PMID" if parent == Some("MedlineCitation") || parent == Some("BookDocument") => {
                    capture = Capture::Pmid;
                    text.clear();
                }
                "PublicationType" if parent == Some("PublicationTypeList") => {
                    capture = Capture::PubType;
                    text.clear();
                }
                "AbstractText" if parent == Some("Abstract") => {
                    capture = Capture::AbstractText;
                    text.clear();
                    label = attr(e, "Label");
                }
                "CommentsCorrections" => {
                    if let Some(b) = cur.as_mut()
                        && attr(e, "RefType").as_deref() == Some("RetractionIn")
                    {
                        b.retraction_comment = true;
                    }
                }
                _ => {}
            }
            if empty {
                // An empty element ends immediately (no End event follows).
                finish_element(&name, &mut capture, &mut text, &mut label, &mut cur);
            } else {
                stack.push(name);
            }
            continue;
        }
        match ev {
            Event::End(_) => {
                let name = stack.pop().ok_or(LitError::Parse)?;
                if name == "PubmedArticle" || name == "PubmedBookArticle" {
                    if let Some(b) = cur.take() {
                        match finish_record(b) {
                            Some(r) => records.push(r),
                            None => skipped += 1,
                        }
                    }
                } else {
                    finish_element(&name, &mut capture, &mut text, &mut label, &mut cur);
                }
            }
            Event::Text(t) => {
                if capture != Capture::None {
                    text.push_str(&t.xml10_content());
                }
            }
            Event::GeneralRef(r) => {
                if capture != Capture::None {
                    if let Some(c) = r.resolve_char_ref().map_err(|_| LitError::Parse)? {
                        text.push(c);
                    } else {
                        let name = r.xml10_content();
                        match name.as_ref() {
                            "amp" => text.push('&'),
                            "lt" => text.push('<'),
                            "gt" => text.push('>'),
                            "quot" => text.push('"'),
                            "apos" => text.push('\''),
                            "nbsp" => text.push(' '),
                            other => {
                                // Unknown named entity (DTD-defined): keep it visible.
                                text.push('&');
                                text.push_str(other);
                                text.push(';');
                            }
                        }
                    }
                }
            }
            Event::CData(c) => {
                if capture != Capture::None {
                    text.push_str(c.as_ref());
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    if !stack.is_empty() || !saw_root {
        return Err(LitError::Parse);
    }
    if skipped > 0 {
        tracing::debug!(skipped, "pubmed: efetch records without a PMID skipped");
    }
    Ok(records)
}

fn attr(e: &quick_xml::events::BytesStart<'_>, key: &str) -> Option<String> {
    e.try_get_attribute(key)
        .ok()
        .flatten()
        .and_then(|a| {
            a.normalized_value(quick_xml::XmlVersion::Implicit1_0)
                .ok()
                .map(|v| v.into_owned())
        })
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Closes the currently captured element when `name` is its end tag.
fn finish_element(
    name: &str,
    capture: &mut Capture,
    text: &mut String,
    label: &mut Option<String>,
    cur: &mut Option<Building>,
) {
    let done = matches!(
        (&*capture, name),
        (Capture::Pmid, "PMID")
            | (Capture::PubType, "PublicationType")
            | (Capture::AbstractText, "AbstractText")
    );

    if !done {
        return;
    }
    let value = collapse_ws(text);
    if let Some(b) = cur.as_mut() {
        match *capture {
            Capture::Pmid => {
                if b.pmid.is_none() && !value.is_empty() {
                    b.pmid = Some(value);
                }
            }
            Capture::PubType => {
                if !value.is_empty() {
                    b.pub_types.push(value);
                }
            }
            Capture::AbstractText => {
                if !value.is_empty() {
                    b.sections.push((label.take(), value));
                }
            }
            Capture::None => {}
        }
    }
    *label = None;
    text.clear();
    *capture = Capture::None;
}

fn collapse_ws(s: &str) -> String {
    // ASCII whitespace only: thin and no-break spaces in the text are meaningful.
    s.split([' ', '\t', '\n', '\r'])
        .filter(|w| !w.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

fn finish_record(b: Building) -> Option<FetchedRecord> {
    let pmid = b.pmid?;
    let abstract_text = if b.sections.is_empty() {
        None
    } else {
        Some(
            b.sections
                .into_iter()
                .map(|(label, text)| match label {
                    Some(l) => format!("{l}: {text}"),
                    None => text,
                })
                .collect::<Vec<_>>()
                .join("\n\n"),
        )
    };
    let retracted = b.retraction_comment || b.pub_types.iter().any(|t| is_retracted_type(t));
    Some(FetchedRecord {
        pmid,
        abstract_text,
        pub_types: b.pub_types,
        retracted,
    })
}
