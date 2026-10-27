//! Europe PMC: search (`resultType=core`), full-text XML, and the PubMed merge.
//!
//! * `search` maps each hit to a [`Paper`] (PMID, PMCID, DOI, open-access flag).
//!   For an open-access hit it picks one https full-text URL from the hit's own
//!   `fullTextUrlList` (entries marked open access / free), or falls back to the
//!   Europe PMC render URL when the hit has a PMCID and no list at all, and
//!   registers it with [`OaPolicy::allow_once`]. That call (in `search`) is the
//!   only grant site in this module, and it only ever sees parsed API fields.
//! * `full_text` downloads `/{PMCID}/fullTextXML` from the allowlisted Europe
//!   PMC host and returns the abstract, methods, results and discussion in that
//!   order, capped at [`FULL_TEXT_CHARS`] characters in total.
//! * [`merge_results`] de-duplicates PubMed and Europe PMC records.

use std::collections::HashMap;

use quick_xml::Reader;
use quick_xml::events::{BytesStart, Event};
use serde::Deserialize;
use url::Url;

use super::http::{AllowlistedClient, FULL_TEXT_MAX, LitError, OaPolicy};
use super::{OaLink, Paper, PaperSource, url_is_pdf};

const REST: &str = "https://www.ebi.ac.uk/europepmc/webservices/rest/";
/// Characters of full text kept per paper (spec §5).
pub const FULL_TEXT_CHARS: usize = 60_000;
/// Europe PMC allows up to 1,000; we never need more than a page of 100.
const MAX_PAGE: u16 = 100;
const RETRACTED_TYPE: &str = "Retracted Publication";

/// Sections of one article. Names are `abstract`, `methods`, `results` and
/// `discussion`, in that order; empty sections are omitted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FullText {
    pub sections: Vec<(String, String)>,
}

pub struct EuropePmc {
    client: AllowlistedClient,
    oa: OaPolicy,
}

impl std::fmt::Debug for EuropePmc {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EuropePmc").finish_non_exhaustive()
    }
}

impl EuropePmc {
    /// `oa` must be the policy shared with `client` (clones share state).
    pub fn new(client: AllowlistedClient, oa: OaPolicy) -> Self {
        Self { client, oa }
    }

    /// Core search, up to `max` records (clamped to 1..=100).
    pub async fn search(&self, query: &str, max: u16) -> Result<Vec<Paper>, LitError> {
        let mut u = Url::parse(REST)
            .and_then(|b| b.join("search"))
            .expect("static europe pmc url");
        u.query_pairs_mut()
            .append_pair("query", query)
            .append_pair("resultType", "core")
            .append_pair("format", "json")
            .append_pair("pageSize", &max.clamp(1, MAX_PAGE).to_string());
        let body: SearchResponse = self.client.get_json(u.as_str()).await?;
        let results = body.result_list.ok_or(LitError::Parse)?.result;
        let mut papers = Vec::with_capacity(results.len());
        for r in results {
            let paper = to_paper(r);
            if let Some(url) = &paper.oa_url {
                // The one grant site: a URL chosen from Europe PMC's own response.
                self.oa.allow_once(url);
            }
            papers.push(paper);
        }
        Ok(papers)
    }

    /// Full text of an open-access article by PMCID (`PMC` + digits).
    pub async fn full_text(&self, pmcid: &str) -> Result<FullText, LitError> {
        if !is_pmcid(pmcid) {
            return Err(LitError::Parse);
        }
        let url = format!("{REST}{pmcid}/fullTextXML");
        let bytes = self.client.get_bytes_capped(&url, FULL_TEXT_MAX).await?;
        full_text_from_xml(&String::from_utf8_lossy(&bytes))
    }
}

fn is_pmcid(s: &str) -> bool {
    s.strip_prefix("PMC")
        .is_some_and(|d| !d.is_empty() && d.bytes().all(|b| b.is_ascii_digit()))
}

// ---------------------------------------------------------------------------
// search response
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct SearchResponse {
    #[serde(rename = "resultList")]
    result_list: Option<ResultList>,
}

#[derive(Deserialize)]
struct ResultList {
    #[serde(default)]
    result: Vec<Raw>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Raw {
    id: Option<String>,
    source: Option<String>,
    pmid: Option<String>,
    pmcid: Option<String>,
    doi: Option<String>,
    title: Option<String>,
    #[serde(rename = "authorString")]
    author_string: Option<String>,
    #[serde(rename = "authorList")]
    author_list: Option<AuthorList>,
    #[serde(rename = "journalInfo")]
    journal_info: Option<JournalInfo>,
    #[serde(rename = "pubYear")]
    pub_year: Option<String>,
    #[serde(rename = "firstPublicationDate")]
    first_publication_date: Option<String>,
    #[serde(rename = "pubTypeList")]
    pub_type_list: Option<PubTypeList>,
    #[serde(rename = "abstractText")]
    abstract_text: Option<String>,
    #[serde(rename = "isOpenAccess")]
    is_open_access: Option<String>,
    #[serde(rename = "fullTextUrlList")]
    full_text_url_list: Option<UrlList>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct AuthorList {
    author: Vec<Author>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Author {
    #[serde(rename = "fullName")]
    full_name: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct JournalInfo {
    #[serde(rename = "yearOfPublication")]
    year: Option<serde_json::Value>,
    journal: Option<Journal>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Journal {
    title: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct PubTypeList {
    #[serde(rename = "pubType")]
    pub_type: Vec<String>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct UrlList {
    #[serde(rename = "fullTextUrl")]
    full_text_url: Vec<FullTextUrl>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct FullTextUrl {
    availability: Option<String>,
    #[serde(rename = "availabilityCode")]
    availability_code: Option<String>,
    #[serde(rename = "documentStyle")]
    document_style: Option<String>,
    site: Option<String>,
    url: Option<String>,
}

fn non_empty(s: Option<String>) -> Option<String> {
    s.map(|v| v.trim().to_string()).filter(|v| !v.is_empty())
}

fn to_paper(r: Raw) -> Paper {
    // Records from the MEDLINE feed use the PMID as their id.
    let pmid = non_empty(r.pmid).or_else(|| {
        (r.source.as_deref() == Some("MED"))
            .then(|| non_empty(r.id.clone()))
            .flatten()
    });
    let pmcid = non_empty(r.pmcid).filter(|p| is_pmcid(p));
    let oa = open_access_link(
        r.is_open_access.as_deref() == Some("Y"),
        r.full_text_url_list.as_ref(),
    );
    let authors = match r.author_list {
        Some(l) if !l.author.is_empty() => l
            .author
            .into_iter()
            .filter_map(|a| non_empty(a.full_name))
            .collect(),
        _ => r
            .author_string
            .as_deref()
            .map(|s| {
                s.trim_end_matches('.')
                    .split(", ")
                    .map(str::trim)
                    .filter(|a| !a.is_empty())
                    .map(String::from)
                    .collect()
            })
            .unwrap_or_default(),
    };
    let year = r
        .pub_year
        .as_deref()
        .and_then(|y| y.trim().parse::<i32>().ok())
        .or_else(|| {
            let v = r.journal_info.as_ref()?.year.as_ref()?;
            v.as_i64()
                .or_else(|| v.as_str().and_then(|s| s.trim().parse().ok()))
                .filter(|y| *y > 0)
                .and_then(|y| i32::try_from(y).ok())
        })
        .or_else(|| {
            r.first_publication_date
                .as_deref()
                .and_then(|d| d.get(..4))
                .and_then(|y| y.parse().ok())
        });
    let pub_types: Vec<String> = r
        .pub_type_list
        .map(|l| l.pub_type)
        .unwrap_or_default()
        .into_iter()
        .filter(|t| !t.trim().is_empty())
        .collect();
    let retracted = pub_types
        .iter()
        .any(|t| t.eq_ignore_ascii_case(RETRACTED_TYPE));
    Paper {
        pmid,
        pmcid,
        doi: non_empty(r.doi),
        title: r
            .title
            .as_deref()
            .map(strip_markup)
            .unwrap_or_default()
            .trim_end_matches('.')
            .to_string(),
        authors,
        journal: r
            .journal_info
            .and_then(|j| j.journal)
            .and_then(|j| non_empty(j.title)),
        year,
        pub_types,
        abstract_text: r
            .abstract_text
            .as_deref()
            .map(strip_markup)
            .filter(|a| !a.is_empty()),
        oa_is_pdf: oa.as_ref().is_some_and(|l| l.is_pdf),
        oa_url: oa.map(|l| l.url),
        retracted,
        source: PaperSource::EuropePmc,
    }
}

/// One https full-text link for an open-access hit, from its own response
/// fields. HTML is preferred to PDF; there is no synthesised fallback (full text
/// comes from `fullTextXML` by PMCID, which needs no grant).
fn open_access_link(is_oa: bool, list: Option<&UrlList>) -> Option<OaLink> {
    if !is_oa {
        return None;
    }
    list?
        .full_text_url
        .iter()
        .filter(|e| {
            let code = e.availability_code.as_deref().unwrap_or_default();
            let word = e.availability.as_deref().unwrap_or_default();
            code.eq_ignore_ascii_case("OA")
                || code.eq_ignore_ascii_case("F")
                || word.eq_ignore_ascii_case("Open access")
                || word.eq_ignore_ascii_case("Free")
        })
        .filter_map(|e| {
            let url = e.url.as_deref()?.trim();
            let parsed = Url::parse(url).ok()?;
            (parsed.scheme() == "https" && parsed.host_str().is_some()).then(|| {
                let is_pdf = e.document_style.as_deref() == Some("pdf") || url_is_pdf(url);
                let epmc = e.site.as_deref() == Some("Europe_PMC");
                let rank = match (epmc, is_pdf) {
                    (true, false) => 0,
                    (false, false) => 1,
                    (true, true) => 2,
                    (false, true) => 3,
                };
                (
                    rank,
                    OaLink {
                        url: url.to_string(),
                        is_pdf,
                    },
                )
            })
        })
        .min_by_key(|(rank, _)| *rank)
        .map(|(_, link)| link)
}

/// Removes markup from an abstract or title: inline tags (`<i>`, `<sub>`) vanish,
/// block tags (`<h4>`, `<p>`, `<br>`) become a space, entities are decoded and
/// whitespace is collapsed.
fn strip_markup(s: &str) -> String {
    const INLINE: [&str; 11] = [
        "i", "b", "u", "em", "strong", "sub", "sup", "italic", "bold", "span", "a",
    ];
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(start) = rest.find('<') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        match after.find('>') {
            Some(end) => {
                let tag = after[..end].trim_start_matches('/');
                let name = tag
                    .split(|c: char| c.is_whitespace() || c == '/')
                    .next()
                    .unwrap_or_default();
                let looks_like_tag = name.chars().next().is_some_and(|c| c.is_ascii_alphabetic());
                if looks_like_tag {
                    if !INLINE.contains(&name.to_ascii_lowercase().as_str()) {
                        out.push(' ');
                    }
                } else {
                    // A stray `<` (e.g. "p < 0.05"): keep it.
                    out.push('<');
                    out.push_str(&after[..=end]);
                }
                rest = &after[end + 1..];
            }
            None => {
                out.push('<');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    decode_entities(&out)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn decode_entities(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        let after = &rest[i + 1..];
        let decoded = after.find(';').filter(|e| *e <= 10).and_then(|e| {
            let name = &after[..e];
            let c = match name {
                "amp" => '&',
                "lt" => '<',
                "gt" => '>',
                "quot" => '"',
                "apos" => '\'',
                "nbsp" => ' ',
                n if n.starts_with("#x") || n.starts_with("#X") => {
                    char::from_u32(u32::from_str_radix(&n[2..], 16).ok()?)?
                }
                n if n.starts_with('#') => char::from_u32(n[1..].parse().ok()?)?,
                _ => return None,
            };
            Some((c, e + 1))
        });
        match decoded {
            Some((c, used)) => {
                out.push(c);
                rest = &after[used..];
            }
            None => {
                out.push('&');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

// ---------------------------------------------------------------------------
// full text XML
// ---------------------------------------------------------------------------

/// Marks a paragraph boundary inside a section buffer.
const BREAK: char = '\u{1f}';
/// Subtrees whose text is not prose we want.
const SKIPPED: [&str; 6] = [
    "table-wrap",
    "fig",
    "ref-list",
    "back",
    "supplementary-material",
    "graphic",
];

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Methods,
    Results,
    Discussion,
}

struct Sec {
    sec_type: Option<String>,
    title: String,
    text: String,
    depth: usize,
}

fn attr(e: &BytesStart<'_>, key: &str) -> Option<String> {
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

/// Which of the wanted sections a heading or `sec-type` names, if any.
fn kind_of(s: &str) -> Option<Kind> {
    let s = s.to_lowercase();
    if s.contains("result") {
        Some(Kind::Results)
    } else if s.contains("discussion") {
        Some(Kind::Discussion)
    } else if s.contains("method")
        || s.contains("material")
        || s.contains("patients and")
        || s.contains("study design")
        || s.contains("experimental procedure")
    {
        Some(Kind::Methods)
    } else {
        None
    }
}

fn classify(sec: &Sec) -> Option<Kind> {
    kind_of(&sec.title).or_else(|| {
        sec.sec_type
            .as_deref()
            .and_then(|t| t.split(['|', ',', ' ']).find_map(kind_of))
    })
}

fn normalise(raw: &str) -> String {
    raw.split(BREAK)
        .map(|p| p.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// Parses a Europe PMC / JATS `fullTextXML` document. Never panics on odd
/// input; a document with no usable section is `Err(Parse)`.
pub fn full_text_from_xml(xml: &str) -> Result<FullText, LitError> {
    let mut reader = Reader::from_str(xml);
    let mut stack: Vec<String> = Vec::new();
    let mut skip: Option<usize> = None;

    let mut abstract_text: Option<String> = None;
    let mut abstract_open: Option<(usize, String)> = None;
    let mut sec: Option<Sec> = None;
    let mut in_title = false;
    let mut title_depth = 0usize;
    let mut found: Vec<(Kind, String)> = Vec::new();

    // A read error ends the parse; sections that closed cleanly are kept.
    while let Ok(ev) = reader.read_event() {
        let (start, empty) = match &ev {
            Event::Start(e) => (Some(e), false),
            Event::Empty(e) => (Some(e), true),
            _ => (None, false),
        };
        if let Some(e) = start {
            let name = e.local_name().as_ref().to_string();
            let depth = stack.len(); // depth of this element's parent chain
            if skip.is_none() {
                let bibr = name == "xref" && attr(e, "ref-type").as_deref() == Some("bibr");
                if SKIPPED.contains(&name.as_str()) || bibr {
                    if !empty {
                        skip = Some(depth);
                    }
                } else if name == "abstract"
                    && abstract_text.is_none()
                    && abstract_open.is_none()
                    && attr(e, "abstract-type").is_none()
                    && stack.iter().any(|s| s == "front")
                {
                    if !empty {
                        abstract_open = Some((depth, String::new()));
                    }
                } else if name == "sec"
                    && sec.is_none()
                    && stack.last().is_some_and(|p| p == "body")
                {
                    if !empty {
                        sec = Some(Sec {
                            sec_type: attr(e, "sec-type"),
                            title: String::new(),
                            text: String::new(),
                            depth,
                        });
                    }
                } else if name == "title"
                    && !empty
                    && sec.as_ref().is_some_and(|s| s.depth + 1 == depth)
                {
                    in_title = true;
                    title_depth = depth;
                }
            }
            if !empty {
                stack.push(name);
            }
            continue;
        }
        match ev {
            Event::End(_) => {
                let Some(name) = stack.pop() else { continue };
                let depth = stack.len();
                if skip == Some(depth) {
                    skip = None;
                    continue;
                }
                if skip.is_some() {
                    continue;
                }
                if in_title && name == "title" && depth == title_depth {
                    in_title = false;
                    continue;
                }
                if matches!(name.as_str(), "p" | "title" | "sec" | "label") {
                    if let Some((_, t)) = abstract_open.as_mut() {
                        t.push(BREAK);
                    } else if let Some(s) = sec.as_mut() {
                        s.text.push(BREAK);
                    }
                }
                if name == "abstract" && abstract_open.as_ref().is_some_and(|(d, _)| *d == depth) {
                    if let Some((_, t)) = abstract_open.take() {
                        abstract_text = Some(t);
                    }
                } else if name == "sec"
                    && sec.as_ref().is_some_and(|s| s.depth == depth)
                    && let Some(s) = sec.take()
                    && let Some(kind) = classify(&s)
                {
                    found.push((kind, s.text));
                }
            }
            Event::Text(t) => {
                if skip.is_none() {
                    push_text(&t.xml10_content(), in_title, &mut abstract_open, &mut sec);
                }
            }
            Event::CData(c) => {
                if skip.is_none() {
                    let s = c.as_ref().to_string();
                    push_text(&s, in_title, &mut abstract_open, &mut sec);
                }
            }
            Event::GeneralRef(r) => {
                if skip.is_none() {
                    let text = match r.resolve_char_ref() {
                        Ok(Some(c)) => c.to_string(),
                        Ok(None) => match r.xml10_content().as_ref() {
                            "amp" => "&".into(),
                            "lt" => "<".into(),
                            "gt" => ">".into(),
                            "quot" => "\"".into(),
                            "apos" => "'".into(),
                            "nbsp" => " ".into(),
                            other => format!("&{other};"),
                        },
                        Err(_) => String::new(),
                    };
                    push_text(&text, in_title, &mut abstract_open, &mut sec);
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }

    // Assemble in fixed order.
    let mut sections: Vec<(&'static str, String)> = Vec::new();
    if let Some(a) = abstract_text {
        let a = normalise(&a);
        // Drop a leading "Abstract" heading.
        let a = a
            .strip_prefix("Abstract\n\n")
            .map(str::to_string)
            .unwrap_or(a);
        sections.push(("abstract", a));
    }
    for (name, want) in [
        ("methods", Kind::Methods),
        ("results", Kind::Results),
        ("discussion", Kind::Discussion),
    ] {
        let joined = found
            .iter()
            .filter(|(k, _)| *k == want)
            .map(|(_, t)| normalise(t))
            .filter(|t| !t.is_empty())
            .collect::<Vec<_>>()
            .join("\n\n");
        sections.push((name, joined));
    }
    sections.retain(|(_, t)| !t.is_empty());
    if sections.is_empty() {
        return Err(LitError::Parse);
    }
    truncate_sections(&mut sections, FULL_TEXT_CHARS);
    Ok(FullText {
        sections: sections
            .into_iter()
            .map(|(n, t)| (n.to_string(), t))
            .collect(),
    })
}

fn push_text(
    text: &str,
    in_title: bool,
    abstract_open: &mut Option<(usize, String)>,
    sec: &mut Option<Sec>,
) {
    if let Some((_, t)) = abstract_open.as_mut() {
        t.push_str(text);
    } else if let Some(s) = sec.as_mut() {
        if in_title {
            s.title.push_str(text);
        } else {
            s.text.push_str(text);
        }
    }
}

/// Cuts the total to `max` characters: discussion first, then results, then
/// (only if still over) methods and abstract. Sections left empty are dropped.
fn truncate_sections(sections: &mut Vec<(&'static str, String)>, max: usize) {
    let total =
        |s: &[(&'static str, String)]| -> usize { s.iter().map(|(_, t)| t.chars().count()).sum() };
    let mut over = total(sections).saturating_sub(max);
    for victim in ["discussion", "results", "methods", "abstract"] {
        if over == 0 {
            break;
        }
        if let Some((_, text)) = sections.iter_mut().find(|(n, _)| *n == victim) {
            let len = text.chars().count();
            let keep = len.saturating_sub(over);
            over -= len - keep;
            let cut: String = text.chars().take(keep).collect();
            *text = cut.trim_end().to_string();
        }
    }
    sections.retain(|(_, t)| !t.is_empty());
}

// ---------------------------------------------------------------------------
// merge
// ---------------------------------------------------------------------------

fn clean(s: &Option<String>) -> Option<&str> {
    s.as_deref().map(str::trim).filter(|s| !s.is_empty())
}

fn doi_key(p: &Paper) -> Option<String> {
    clean(&p.doi).map(str::to_lowercase)
}

/// Fills `base`'s missing fields from `other`; `base` wins when both have a value.
fn merge_into(base: &mut Paper, other: Paper) {
    fn fill(a: &mut Option<String>, b: Option<String>) {
        if clean(a).is_none() {
            *a = b.filter(|v| !v.trim().is_empty());
        }
    }
    fill(&mut base.pmid, other.pmid);
    fill(&mut base.pmcid, other.pmcid);
    fill(&mut base.doi, other.doi);
    fill(&mut base.journal, other.journal);
    fill(&mut base.abstract_text, other.abstract_text);
    // The link and its PDF flag travel together; an HTML link beats a PDF one.
    if clean(&base.oa_url).is_none()
        || (base.oa_is_pdf && !other.oa_is_pdf && other.oa_url.is_some())
    {
        base.oa_url = other.oa_url;
        base.oa_is_pdf = other.oa_is_pdf;
    }
    if base.title.trim().is_empty() {
        base.title = other.title;
    }
    if base.authors.is_empty() {
        base.authors = other.authors;
    }
    if base.year.is_none() {
        base.year = other.year;
    }
    for t in other.pub_types {
        if !base.pub_types.contains(&t) {
            base.pub_types.push(t);
        }
    }
    base.retracted |= other.retracted;
    if base.source != other.source {
        base.source = PaperSource::Both;
    }
}

#[derive(Default)]
struct Index {
    pmid: HashMap<String, usize>,
    doi: HashMap<String, usize>,
    pmcid: HashMap<String, usize>,
}

impl Index {
    fn add(&mut self, i: usize, p: &Paper) {
        if let Some(k) = clean(&p.pmid) {
            self.pmid.entry(k.to_string()).or_insert(i);
        }
        if let Some(k) = doi_key(p) {
            self.doi.entry(k).or_insert(i);
        }
        if let Some(k) = clean(&p.pmcid) {
            self.pmcid.entry(k.to_string()).or_insert(i);
        }
    }

    /// The existing record `p` duplicates: by PMID, then lower-cased DOI, then
    /// PMCID. Two records that both have a PMID and differ are never the same.
    fn find(&self, p: &Paper, all: &[Paper]) -> Option<usize> {
        if let Some(i) = clean(&p.pmid).and_then(|k| self.pmid.get(k)) {
            return Some(*i);
        }
        let candidate = doi_key(p)
            .and_then(|k| self.doi.get(&k))
            .or_else(|| clean(&p.pmcid).and_then(|k| self.pmcid.get(k)))
            .copied()?;
        let conflict = matches!(
            (clean(&p.pmid), clean(&all[candidate].pmid)),
            (Some(a), Some(b)) if a != b
        );
        (!conflict).then_some(candidate)
    }
}

/// Merges PubMed and Europe PMC results. Records are the same paper when the
/// PMID matches, else the lower-cased DOI matches (else the PMCID). A merged
/// record keeps PubMed's values, takes Europe PMC's where PubMed has none, and
/// has `source = Both`. PubMed's order is kept and Europe-PMC-only records are
/// appended in their order, duplicates within either list collapsed.
pub fn merge_results(pubmed: Vec<Paper>, epmc: Vec<Paper>) -> Vec<Paper> {
    let mut out: Vec<Paper> = Vec::with_capacity(pubmed.len() + epmc.len());
    let mut index = Index::default();
    for p in pubmed.into_iter().chain(epmc) {
        match index.find(&p, &out) {
            Some(i) => {
                merge_into(&mut out[i], p);
                let merged = out[i].clone();
                index.add(i, &merged);
            }
            None => {
                index.add(out.len(), &p);
                out.push(p);
            }
        }
    }
    out
}
