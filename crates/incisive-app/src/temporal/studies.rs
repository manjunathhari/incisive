//! Steps 1–4: study kinds, linked reports, treatment years and eras.

use std::collections::HashMap;
use std::sync::LazyLock;

use regex::Regex;
use serde::{Deserialize, Serialize};

use super::input::{InStudy, TemporalInput};

/// What a study is to the table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StudyKind {
    /// A primary study: its numbers can be pooled.
    Primary,
    /// A meta-analysis, systematic review or network meta-analysis: shown
    /// as a marker, never pooled (it re-uses the primaries' patients).
    Marker,
    /// A guideline or consensus statement listed among the studies.
    Guideline,
}

static MARKER_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)meta[- ]?analy|systematic\s+review|network\s+meta|\bNMA\b|umbrella\s+review|pooled\s+analysis|individual[- ]patient[- ]data|\bIPD\b|literature\s+review")
        .expect("marker regex")
});
static GUIDELINE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\bguidelines?\b|consensus\s+statement|position\s+statement|standards?\s+of\s+practice")
        .expect("guideline regex")
});

/// Step 1: the kind of a study from its design (case-insensitive). A
/// design naming both (a systematic review for a guideline) is a marker.
pub fn study_kind(design: &str) -> StudyKind {
    if MARKER_RE.is_match(design) {
        StudyKind::Marker
    } else if GUIDELINE_RE.is_match(design) {
        StudyKind::Guideline
    } else {
        StudyKind::Primary
    }
}

/// Where a study's treatment years came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum YearsFrom {
    Setting,
    Design,
    Appraisal,
    KeyResults,
    /// "in 2019" in the setting.
    SettingSingleYear,
    /// Publication year minus 2 (assumed).
    PublicationMinusTwo,
    /// A linked report inherits its parent's years.
    Parent,
    /// No years and no publication year.
    Unknown,
}

/// A follow-up horizon ("5-year": 60 months).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Horizon {
    pub months: u32,
    /// As written, e.g. "5-year".
    pub text: String,
}

/// One study of the table.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StudyEntry {
    /// `s-<pmid>` (or `s-row<i>` for a study with no PMID).
    pub id: String,
    pub pmid: String,
    pub label: String,
    pub design: String,
    pub n: Option<i64>,
    pub kind: StudyKind,
    /// From the references.
    pub pub_year: Option<i32>,
    pub found_by_skeptic: bool,
    pub treatment_start: Option<i32>,
    pub treatment_end: Option<i32>,
    pub years_from: YearsFrom,
    /// The treatment years are a guess (publication year minus 2, or the
    /// parent's guess).
    pub assumed: bool,
    pub midpoint: Option<f64>,
    /// The era of the midpoint (primary studies and linked reports only).
    pub era_id: Option<String>,
    /// A linked report: the id of the trial's first report.
    pub parent_id: Option<String>,
    pub child_ids: Vec<String>,
    /// Why it was linked, in plain words.
    pub link_reason: Option<String>,
    /// When its result became known (its publication year).
    pub knowledge_year: Option<i32>,
    pub horizon: Option<Horizon>,
}

/// A skeptic's overlap note that is a caveat, not a link (e.g. "databases
/// likely share patients").
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OverlapCaveat {
    /// `v-<i>` (the index among the skeptic's overlaps).
    pub id: String,
    pub pmids: Vec<String>,
    /// The ids of the pmids that are studies of the table.
    pub study_ids: Vec<String>,
    pub note: String,
}

static RANGE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)\b((?:19|20)\d{2})\s*(?:[-–—]|to|until|through)\s*(?:[a-z]+\.?\s+)?((?:19|20)\d{2})\b",
    )
    .expect("range regex")
});
static SINGLE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(?:in|during)\s+(?:[a-z]+\.?\s+)?((?:19|20)\d{2})\b").expect("single regex")
});
static HORIZON_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(\d{1,3})\s*-?\s*(years?|yrs?|months?|mo)\b").expect("horizon regex")
});

/// The first year range in `text` (`2012–2021`, `2016 to April 2019`).
pub fn year_range(text: &str) -> Option<(i32, i32)> {
    RANGE_RE.captures_iter(text).find_map(|c| {
        let a: i32 = c[1].parse().ok()?;
        let b: i32 = c[2].parse().ok()?;
        (a <= b && b - a <= 40).then_some((a, b))
    })
}

/// Step 3: the treatment years stated by a study: a range from the
/// setting, then the design, the appraisal and the key results; else a
/// single year in the setting ("in 2019").
pub fn stated_years(s: &InStudy) -> Option<(i32, i32, YearsFrom)> {
    let fields = [
        (&s.setting, YearsFrom::Setting),
        (&s.design, YearsFrom::Design),
        (&s.appraisal, YearsFrom::Appraisal),
        (&s.key_results, YearsFrom::KeyResults),
    ];
    for (text, from) in fields {
        if let Some((a, b)) = year_range(text) {
            return Some((a, b, from));
        }
    }
    let y: i32 = SINGLE_RE.captures(&s.setting)?[1].parse().ok()?;
    Some((y, y, YearsFrom::SettingSingleYear))
}

/// The follow-up horizon named in `text` ("5-year", "10-yr", "1- and
/// 2-year" (the longest), "12 months"), if any.
pub fn horizon(text: &str) -> Option<Horizon> {
    HORIZON_RE
        .captures_iter(text)
        .filter_map(|c| {
            let k: u32 = c[1].parse().ok()?;
            let unit = c[2].to_ascii_lowercase();
            let months = if unit.starts_with('y') {
                k.checked_mul(12)?
            } else {
                k
            };
            (k > 0 && months <= 360).then(|| Horizon {
                months,
                text: c[0].to_string(),
            })
        })
        .max_by_key(|h| h.months)
}

/// A study's horizon: from its label, else its design.
pub fn study_horizon(s: &InStudy) -> Option<Horizon> {
    horizon(&s.label).or_else(|| horizon(&s.design))
}

/// The id of a study.
pub fn study_id(pmid: &str, row: usize) -> String {
    if pmid.is_empty() {
        format!("s-row{row}")
    } else {
        format!("s-{pmid}")
    }
}

// ---- Step 2: linked reports -------------------------------------------

static LINK_PHRASE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)\bsame\s+(?:[a-z-]+\s+)?(?:trial|cohort|patients|population)\b|\bsecondary\s+analysis\s+of\b|\bpost[- ]hoc\s+analysis\s+of\b|\bfollow[- ]?up\s+(?:report\s+)?of\b|\bextension\s+of\b",
    )
    .expect("link phrase regex")
});
static OVERLAP_LINK_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\bsame\s+(?:[\w+-]+\s+){0,6}?(?:trial|cohort|patients|population|reports?|study)\b|\bsingle\s+trial\b|\b(?:duplicate|companion)\s+reports?\b")
        .expect("overlap link regex")
});
static OVERLAP_HEDGE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\blikely\b|\bmay\b|\bmight\b|\bpossibly\b|\bprobably\b|\bcould\b")
        .expect("overlap hedge regex")
});

/// Words that never name a trial on their own.
const NAME_STOP: &[&str] = &[
    "rct",
    "rcts",
    "trial",
    "study",
    "cohort",
    "single",
    "single-centre",
    "single-center",
    "multicentre",
    "multicenter",
    "multi-centre",
    "the",
    "randomised",
    "randomized",
    "pilot",
    "observational",
    "retrospective",
    "prospective",
    "matched",
    "meta-analysis",
    "network",
    "systematic",
    "registry",
    "national",
    "acute",
    "emergency",
    "robotic",
    "laparoscopic",
];

fn valid_name(s: &str) -> bool {
    s.chars().count() >= 3
        && s.chars().any(char::is_uppercase)
        && !NAME_STOP.contains(&s.to_lowercase().as_str())
}

/// The names a study goes by: its label up to the first bracket or comma
/// (less a trailing year), and that stem's first word when it is a proper
/// name ("Werner").
pub fn study_names(label: &str) -> Vec<String> {
    let stem = label.split(['(', ',', ';']).next().unwrap_or("").trim();
    // A trailing publication year is not part of the name ("RIVAL 2020").
    let stem = match stem.rsplit_once(' ') {
        Some((head, y)) if y.len() == 4 && y.chars().all(|c| c.is_ascii_digit()) => head.trim(),
        _ => stem,
    };
    let mut out = Vec::new();
    if valid_name(stem) {
        out.push(stem.to_string());
    }
    let mut words = stem.split_whitespace();
    if let (Some(first), Some(_)) = (words.next(), words.next()) {
        let first = first.trim_matches(|c: char| !c.is_alphanumeric());
        if valid_name(first) && !out.iter().any(|n| n == first) {
            out.push(first.to_string());
        }
    }
    out
}

/// Byte offsets of whole-word occurrences of `word` in `text`.
fn word_hits(text: &str, word: &str) -> Vec<(usize, usize)> {
    if word.is_empty() {
        return Vec::new();
    }
    text.match_indices(word)
        .filter(|(i, _)| {
            let before = text[..*i].chars().next_back();
            let after = text[i + word.len()..].chars().next();
            !before.is_some_and(char::is_alphanumeric) && !after.is_some_and(char::is_alphanumeric)
        })
        .map(|(i, _)| (i, i + word.len()))
        .collect()
}

/// The other study a link phrase in `text` points at: a PMID mention wins,
/// else the nearest name mention within 100 bytes of the phrase.
fn link_target(
    text: &str,
    phrase: (usize, usize),
    me: usize,
    pmids: &[String],
    names: &[Vec<String>],
) -> Option<usize> {
    let lo = phrase.0.saturating_sub(100);
    let hi = (phrase.1 + 100).min(text.len());
    let near = |(a, b): (usize, usize)| -> Option<usize> {
        if a >= lo && b <= hi {
            Some(if a >= phrase.1 {
                a - phrase.1
            } else {
                phrase.0.saturating_sub(b)
            })
        } else {
            None
        }
    };
    let pick = |by: &dyn Fn(usize) -> Vec<(usize, usize)>| -> Option<usize> {
        (0..pmids.len())
            .filter(|&j| j != me)
            .filter_map(|j| by(j).into_iter().filter_map(near).min().map(|d| (d, j)))
            .min()
            .map(|(_, j)| j)
    };
    pick(&|j| word_hits(text, &pmids[j])).or_else(|| {
        pick(&|j| {
            // A report's own full name is not a pointer to another study;
            // its first word may be ("RIVAL 2-year" points at "RIVAL").
            names[j]
                .iter()
                .filter(|n| names[me].first() != Some(*n))
                .flat_map(|n| word_hits(text, n))
                .collect()
        })
    })
}

struct Groups(Vec<usize>);

impl Groups {
    fn find(&mut self, i: usize) -> usize {
        let mut r = i;
        while self.0[r] != r {
            r = self.0[r];
        }
        self.0[i] = r;
        r
    }
    fn join(&mut self, a: usize, b: usize) {
        let (ra, rb) = (self.find(a), self.find(b));
        if ra != rb {
            self.0[ra.max(rb)] = ra.min(rb);
        }
    }
}

/// Step 2: links between reports of one trial, as (child, parent, reason)
/// study indices, and the skeptic's overlap notes kept as caveats.
pub fn link_reports(
    studies: &[InStudy],
    pub_years: &[Option<i32>],
    input: &TemporalInput,
) -> (Vec<(usize, usize, String)>, Vec<OverlapCaveat>) {
    let pmids: Vec<String> = studies.iter().map(|s| s.pmid.trim().to_string()).collect();
    let names: Vec<Vec<String>> = studies.iter().map(|s| study_names(&s.label)).collect();
    let mut groups = Groups((0..studies.len()).collect());
    let mut reason: HashMap<usize, String> = HashMap::new();

    for (i, s) in studies.iter().enumerate() {
        for text in [&s.label, &s.design, &s.appraisal] {
            for m in LINK_PHRASE_RE.find_iter(text) {
                if let Some(j) = link_target(text, (m.start(), m.end()), i, &pmids, &names) {
                    groups.join(i, j);
                    let why = format!("the study table says \"{}\"", m.as_str());
                    reason.entry(i).or_insert_with(|| why.clone());
                    reason.entry(j).or_insert(why);
                }
            }
        }
    }

    let mut caveats = Vec::new();
    let overlaps = input.skeptic.iter().flat_map(|k| k.overlaps.iter());
    for (n, o) in overlaps.enumerate() {
        let idx: Vec<usize> = o
            .pmids
            .iter()
            .filter_map(|p| pmids.iter().position(|q| !q.is_empty() && q == p.trim()))
            .collect();
        if OVERLAP_LINK_RE.is_match(&o.note)
            && !OVERLAP_HEDGE_RE.is_match(&o.note)
            && idx.len() >= 2
        {
            for w in idx.windows(2) {
                groups.join(w[0], w[1]);
            }
            for &i in &idx {
                reason
                    .entry(i)
                    .or_insert_with(|| format!("the skeptic says: {}", o.note.trim()));
            }
        } else {
            caveats.push(OverlapCaveat {
                id: format!("v-{n}"),
                pmids: o.pmids.iter().map(|p| p.trim().to_string()).collect(),
                study_ids: idx.iter().map(|&i| study_id(&pmids[i], i)).collect(),
                note: o.note.clone(),
            });
        }
    }

    let mut members: HashMap<usize, Vec<usize>> = HashMap::new();
    for i in 0..studies.len() {
        let r = groups.find(i);
        members.entry(r).or_default().push(i);
    }
    let mut links = Vec::new();
    let mut roots: Vec<_> = members.into_values().filter(|m| m.len() > 1).collect();
    roots.sort();
    for m in roots {
        // The parent is the earliest publication (unknown years last;
        // ties: the earlier row).
        let parent = *m
            .iter()
            .min_by_key(|&&i| (pub_years[i].unwrap_or(i32::MAX), i))
            .expect("non-empty group");
        for &c in m.iter().filter(|&&c| c != parent) {
            let why = reason
                .get(&c)
                .or_else(|| reason.get(&parent))
                .cloned()
                .unwrap_or_default();
            links.push((c, parent, why));
        }
    }
    (links, caveats)
}

// ---- Step 4: eras -------------------------------------------------------

/// A fixed five-year window (the latest is six years: it ends in the run's
/// year, e.g. 2021–26).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Era {
    /// `e-<start>-<end>`.
    pub id: String,
    pub start: i32,
    pub end: i32,
    /// "2021–26".
    pub label: String,
    /// Primary studies whose treatment midpoint falls in it.
    pub n_studies: usize,
    /// No primary study: kept, and flagged.
    pub empty: bool,
}

/// At most this many eras (stored years are never trusted).
const MAX_ERAS: usize = 30;

/// Era bounds, oldest first: the latest ends in `run_year` and starts five
/// years before it; earlier ones are five-year windows, back to the one
/// holding `earliest`.
pub fn era_bounds(run_year: i32, earliest: i32) -> Vec<(i32, i32)> {
    let mut v = vec![(run_year.saturating_sub(5), run_year)];
    while v.len() < MAX_ERAS && earliest < v[v.len() - 1].0 {
        let end = v[v.len() - 1].0.saturating_sub(1);
        v.push((end.saturating_sub(4), end));
    }
    v.reverse();
    v
}

pub fn era_id(start: i32, end: i32) -> String {
    format!("e-{start}-{end}")
}

pub fn era_label(start: i32, end: i32) -> String {
    if start / 100 == end / 100 {
        format!("{start}–{:02}", end % 100)
    } else {
        format!("{start}–{end}")
    }
}

/// The era of a midpoint (by its whole year); a year after the latest era
/// goes in the latest, a year before the first in none.
pub fn era_of(bounds: &[(i32, i32)], midpoint: f64) -> Option<usize> {
    if !midpoint.is_finite() {
        return None;
    }
    let y = midpoint.floor() as i32;
    if let Some(i) = bounds.iter().position(|&(a, b)| a <= y && y <= b) {
        return Some(i);
    }
    match bounds.last() {
        Some(&(_, end)) if y > end => Some(bounds.len() - 1),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::temporal::input::{InOverlap, InSkeptic};

    fn st(pmid: &str, label: &str, design: &str) -> InStudy {
        InStudy {
            pmid: pmid.into(),
            label: label.into(),
            design: design.into(),
            ..InStudy::default()
        }
    }

    #[test]
    fn kinds_from_design() {
        assert_eq!(study_kind("Meta-analysis of 7 RCTs"), StudyKind::Marker);
        assert_eq!(
            study_kind("systematic review and META ANALYSIS"),
            StudyKind::Marker
        );
        assert_eq!(
            study_kind("Network meta-analysis, 15 RCTs"),
            StudyKind::Marker
        );
        assert_eq!(study_kind("NMA"), StudyKind::Marker);
        assert_eq!(study_kind("Pooled analysis of two RCTs"), StudyKind::Marker);
        assert_eq!(
            study_kind("Individual patient data analysis"),
            StudyKind::Marker
        );
        assert_eq!(study_kind("IPD meta"), StudyKind::Marker);
        assert_eq!(study_kind("Narrative literature review"), StudyKind::Marker);
        assert_eq!(study_kind("SAGES guideline"), StudyKind::Guideline);
        assert_eq!(study_kind("multicentre RCT"), StudyKind::Primary);
        assert_eq!(
            study_kind("cohort (Intuitive network database)"),
            StudyKind::Primary
        );
    }

    #[test]
    fn treatment_years_order_and_forms() {
        let mut s = st("1", "A", "RCT 2010-2012");
        s.setting = "USA, April 2016 to April 2019".into();
        assert_eq!(stated_years(&s), Some((2016, 2019, YearsFrom::Setting)));
        s.setting = "USA".into();
        assert_eq!(stated_years(&s), Some((2010, 2012, YearsFrom::Design)));
        s.design = "RCT".into();
        s.appraisal = "enrolled 2014–2016".into();
        assert_eq!(stated_years(&s), Some((2014, 2016, YearsFrom::Appraisal)));
        s.appraisal.clear();
        s.key_results = "2001 - 2003".into();
        assert_eq!(stated_years(&s), Some((2001, 2003, YearsFrom::KeyResults)));
        s.key_results.clear();
        s.setting = "single centre in 2019".into();
        assert_eq!(
            stated_years(&s),
            Some((2019, 2019, YearsFrom::SettingSingleYear))
        );
        s.setting = "OR 1.61 (1.28-2.03)".into();
        assert_eq!(stated_years(&s), None);
        // Reversed or absurd ranges are not years.
        assert_eq!(year_range("2020-2010"), None);
        assert_eq!(year_range("1950-2020"), None);
    }

    #[test]
    fn horizons() {
        assert_eq!(horizon("RIVAL 2-year").unwrap().months, 24);
        assert_eq!(horizon("10-yr follow-up").unwrap().months, 120);
        assert_eq!(horizon("1- and 2-year outcomes").unwrap().months, 24);
        assert_eq!(horizon("5 years").unwrap().months, 60);
        assert_eq!(horizon("12 months").unwrap().months, 12);
        assert_eq!(horizon("30-day outcomes"), None);
        assert_eq!(horizon("ROGER"), None);
    }

    #[test]
    fn names() {
        assert_eq!(study_names("ROGER (2025, Br J Surg)"), vec!["ROGER"]);
        assert_eq!(study_names("Werner (POEM vs LHM)"), vec!["Werner"]);
        assert_eq!(study_names("RIVAL 2-year"), vec!["RIVAL 2-year", "RIVAL"]);
        assert_eq!(study_names("RIVAL 2020, JAMA Surg"), vec!["RIVAL"]);
        assert_eq!(
            study_names("European Achalasia Trial 2011, NEJM"),
            vec!["European Achalasia Trial", "European"]
        );
        assert!(
            study_names("Single-centre RCT")
                .iter()
                .all(|n| n != "Single-centre")
        );
        assert!(
            study_names("US commercial claims")
                .iter()
                .all(|n| n != "US")
        );
    }

    #[test]
    fn links_by_name_pmid_and_skeptic() {
        let studies = vec![
            st("1", "RIVAL (2020)", "multicentre pilot RCT"),
            st("2", "RIVAL 2-year (2023)", "follow-up of RIVAL"),
            st("3", "ROLAIS (2025)", "single-centre RCT"),
            st("4", "DIRECT (2025)", "RCT"),
            st("5", "ROGER (2025)", "RCT"),
            st("6", "A claims study", "cohort"),
            st("7", "B claims study", "cohort"),
        ];
        let mut studies = studies;
        studies[3].appraisal = "A secondary analysis of 3, unlike ROGER".into();
        let years = vec![
            Some(2020),
            Some(2023),
            Some(2025),
            Some(2025),
            Some(2025),
            Some(2024),
            Some(2025),
        ];
        let input = TemporalInput {
            skeptic: Some(InSkeptic {
                overlaps: vec![
                    InOverlap {
                        pmids: vec!["6".into(), "7".into()],
                        note: "Both databases likely share patients".into(),
                    },
                    InOverlap {
                        pmids: vec!["5".into(), "9".into()],
                        note: "same trial".into(),
                    },
                ],
            }),
            ..TemporalInput::default()
        };
        let (links, caveats) = link_reports(&studies, &years, &input);
        let pairs: Vec<(usize, usize)> = links.iter().map(|l| (l.0, l.1)).collect();
        assert!(pairs.contains(&(1, 0)), "{pairs:?}");
        assert!(pairs.contains(&(3, 2)), "{pairs:?}");
        assert_eq!(pairs.len(), 2, "ROGER is not linked: {pairs:?}");
        // "likely share patients" is a caveat; a group with one known study
        // cannot link and is kept as a caveat too.
        assert_eq!(caveats.len(), 2);
        assert_eq!(caveats[0].study_ids, vec!["s-6", "s-7"]);
    }

    #[test]
    fn skeptic_same_trial_links_with_earliest_parent() {
        let studies = vec![
            st("1", "Later report", "RCT"),
            st("2", "First report", "RCT"),
        ];
        let input = TemporalInput {
            skeptic: Some(InSkeptic {
                overlaps: vec![InOverlap {
                    pmids: vec!["1".into(), "2".into()],
                    note: "These report the same trial".into(),
                }],
            }),
            ..TemporalInput::default()
        };
        let (links, caveats) = link_reports(&studies, &[Some(2024), Some(2020)], &input);
        assert!(caveats.is_empty());
        assert_eq!((links[0].0, links[0].1), (0, 1));
        assert!(links[0].2.contains("skeptic"));
    }

    #[test]
    fn eras_end_in_the_run_year() {
        assert_eq!(
            era_bounds(2026, 2014),
            vec![(2011, 2015), (2016, 2020), (2021, 2026)]
        );
        assert_eq!(era_bounds(2026, 2021), vec![(2021, 2026)]);
        assert_eq!(era_bounds(2026, 2009).len(), 4);
        assert_eq!(era_bounds(2026, 1000).len(), MAX_ERAS);
        assert_eq!(era_label(2021, 2026), "2021–26");
        assert_eq!(era_label(1996, 2000), "1996–2000");
        let b = era_bounds(2026, 2014);
        assert_eq!(era_of(&b, 2016.5), Some(1));
        assert_eq!(era_of(&b, 2015.9), Some(0));
        assert_eq!(era_of(&b, 2030.0), Some(2));
        assert_eq!(era_of(&b, 2000.0), None);
        assert_eq!(era_of(&b, f64::NAN), None);
    }

    #[test]
    fn bare_follow_up_horizon_does_not_link() {
        let mut roger = st("2", "ROGER 2025", "Single-centre RCT");
        roger.appraisal = "Comparator TEP as in RIVAL; 1-year follow-up.".into();
        let studies = vec![st("1", "RIVAL 2020", "Pilot RCT"), roger];
        let (links, _) = link_reports(
            &studies,
            &[Some(2020), Some(2025)],
            &TemporalInput::default(),
        );
        assert!(links.is_empty(), "{links:?}");
        // "follow-up of <name>" still links.
        let mut r2 = st("3", "RIVAL 2-year", "2-year follow-up");
        r2.appraisal = "Long-term follow-up of RIVAL.".into();
        let studies = vec![st("1", "RIVAL 2020", "Pilot RCT"), r2];
        let (links, _) = link_reports(
            &studies,
            &[Some(2020), Some(2023)],
            &TemporalInput::default(),
        );
        assert_eq!(links.len(), 1);
    }

    #[test]
    fn only_modal_words_hedge_a_skeptic_link() {
        let studies = vec![
            st("1", "A", "RCT"),
            st("2", "B", "RCT"),
            st("3", "C", "RCT"),
        ];
        let mk = |note: &str| TemporalInput {
            skeptic: Some(InSkeptic {
                overlaps: vec![InOverlap {
                    pmids: vec!["1".into(), "2".into()],
                    note: note.into(),
                }],
            }),
            ..TemporalInput::default()
        };
        let years = [Some(2020), Some(2022), Some(2023)];
        for note in [
            "These share the same trial population.",
            "Both report the same cohort; pooled in later reviews.",
        ] {
            let (links, caveats) = link_reports(&studies, &years, &mk(note));
            assert_eq!(links.len(), 1, "{note}");
            assert!(caveats.is_empty(), "{note}");
        }
        for note in [
            "These may be the same trial.",
            "Probably the same cohort.",
            "Likely the same patients.",
        ] {
            let (links, caveats) = link_reports(&studies, &years, &mk(note));
            assert!(links.is_empty(), "{note}");
            assert_eq!(caveats.len(), 1, "{note}");
        }
    }

    #[test]
    fn era_bounds_never_overflow() {
        let b = era_bounds(i32::MIN + 2, i32::MIN);
        assert!(!b.is_empty());
        let b = era_bounds(i32::MAX, i32::MAX);
        assert_eq!(b.len(), 1);
    }
}
