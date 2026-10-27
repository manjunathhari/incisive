//! Local patient-identifier scanner.
//!
//! Runs before any network call so identifiable patient data never leaves the
//! app. Pure Rust (no server-only dependencies): it may later run in the browser.
//!
//! Policy: ages, sex and relative clinical time ("POD 3", "6 weeks post-op") are
//! allowed. Aadhaar, PAN, phone numbers, email addresses, exact calendar dates,
//! hospital IDs, titled/contextual person names and street addresses are blocked.
//!
//! The scanner is deliberately heuristic. Known, documented limits:
//! - `d/m` without a year ("3/12") is not treated as a calendar date (it is also
//!   how clinicians write fractions and follow-up in months).
//! - Month-and-year ("March 2024") and year alone are allowed.
//! - Names are only caught with a title or patient-context word and an ASCII
//!   capitalised word; all-caps and non-Latin names are not caught.
//! - `#12` alone is not an address; it needs a street word nearby.

use regex::{Captures, Regex};
use serde::{Deserialize, Serialize};
use std::sync::LazyLock;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FindingKind {
    Aadhaar,
    Pan,
    Phone,
    Email,
    CalendarDate,
    HospitalId,
    PersonName,
    Address,
}

/// A blocked span. `start`/`end` are byte offsets into the scanned `&str`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Finding {
    pub kind: FindingKind,
    pub start: usize,
    pub end: usize,
}

/// User-facing label for a finding kind.
pub fn describe(kind: FindingKind) -> &'static str {
    match kind {
        FindingKind::Aadhaar => "Aadhaar number",
        FindingKind::Pan => "PAN number",
        FindingKind::Phone => "phone number",
        FindingKind::Email => "email address",
        FindingKind::CalendarDate => "calendar date",
        FindingKind::HospitalId => "hospital or patient ID",
        FindingKind::PersonName => "person's name",
        FindingKind::Address => "street address or PIN code",
    }
}

type Check = fn(&str, &Captures) -> bool;

struct Rule {
    kind: FindingKind,
    re: Regex,
    check: Check,
}

fn rule(kind: FindingKind, pattern: &str, check: Check) -> Rule {
    Rule {
        kind,
        re: Regex::new(pattern).expect("deid regex compiles"),
        check,
    }
}

fn always(_: &str, _: &Captures) -> bool {
    true
}

// ---------- helpers ----------

fn span(c: &Captures) -> (usize, usize) {
    let m = c.get(0).expect("group 0");
    (m.start(), m.end())
}

/// True when the match is glued to neighbouring digits ("1234567890123",
/// "2345-6789-0123-4567", "1.150000"), i.e. it is part of a longer number.
fn inside_longer_number(text: &str, start: usize, end: usize) -> bool {
    let sep = |c: char| matches!(c, '.' | '-' | '/');
    let mut before = text[..start].chars().rev();
    let prev = match before.next() {
        Some(c) if c.is_ascii_digit() => true,
        Some(c) if sep(c) => matches!(before.next(), Some(d) if d.is_ascii_digit()),
        _ => false,
    };
    let mut after = text[end..].chars();
    let next = match after.next() {
        Some(c) if c.is_ascii_digit() => true,
        Some(c) if sep(c) => matches!(after.next(), Some(d) if d.is_ascii_digit()),
        _ => false,
    };
    prev || next
}

fn standalone_number(text: &str, c: &Captures) -> bool {
    let (s, e) = span(c);
    !inside_longer_number(text, s, e)
}

fn word_key(w: &str) -> String {
    w.chars()
        .filter(|c| c.is_alphabetic())
        .collect::<String>()
        .to_lowercase()
}

// ---------- PIN code ----------

const LAB_WORDS: &[&str] = &[
    "platelets",
    "platelet",
    "plt",
    "plts",
    "count",
    "wbc",
    "tlc",
    "iu",
    "units",
    "cells",
    "rs",
    "inr",
    "\u{20b9}",
];

const UNIT_PREFIXES: &[&str] = &[
    "/", "cells", "cell", "iu", "units", "unit", "mg", "ml", "mm", "cc", "mcg", "ug", "ng", "pg",
    "cu", "per", "%", "\u{b5}", "\u{3bc}",
];

fn pin_check(text: &str, c: &Captures) -> bool {
    let (s, e) = span(c);
    if inside_longer_number(text, s, e) {
        return false;
    }
    // A lab/count word within the previous 3 tokens means this is a count.
    let lab_before = text[..s].split_whitespace().rev().take(3).any(|t| {
        LAB_WORDS.contains(
            &t.trim_matches(|ch: char| ch.is_ascii_punctuation())
                .to_lowercase()
                .as_str(),
        ) || t.contains('\u{20b9}')
    });
    if lab_before {
        return false;
    }
    // Followed by a unit means this is a measurement.
    let rest = text[e..].trim_start().to_lowercase();
    !UNIT_PREFIXES.iter().any(|u| rest.starts_with(u))
}

// ---------- names ----------

/// Surgical/medical eponyms and procedure names that look like surnames.
const EPONYMS: &[&str] = &[
    "hartmann",
    "whipple",
    "nissen",
    "crohn",
    "roux",
    "child",
    "pugh",
    "billroth",
    "kocher",
    "pringle",
    "calot",
    "lichtenstein",
    "stoppa",
    "miles",
    "dukes",
    "clavien",
    "dindo",
    "heller",
    "kasai",
    "toupet",
    "dor",
    "rives",
    "shouldice",
    "bassini",
    "hasson",
    "veress",
    "puestow",
    "frey",
    "beger",
    "warshaw",
    "longmire",
    "mikulicz",
    "soave",
    "duhamel",
    "swenson",
    "barrett",
    "lynch",
    "trendelenburg",
    "glasgow",
    "ranson",
    "balthazar",
    "atlanta",
    "tokyo",
    "bismuth",
    "strasberg",
    "couinaud",
    "cattell",
    "braasch",
    "hinchey",
    "parks",
    "ivor",
    "lewis",
    "appleby",
    "cantlie",
    "gerota",
    "denonvilliers",
    "waldeyer",
    "toldt",
];

/// Common capitalised words that follow "patient"/"pt"/"name" in clinical prose.
const NOT_NAMES: &[&str] = &[
    "the",
    "this",
    "that",
    "these",
    "those",
    "and",
    "with",
    "without",
    "for",
    "from",
    "after",
    "before",
    "has",
    "had",
    "have",
    "was",
    "were",
    "who",
    "which",
    "male",
    "female",
    "man",
    "woman",
    "boy",
    "girl",
    "post",
    "pre",
    "day",
    "days",
    "week",
    "weeks",
    "known",
    "presented",
    "presenting",
    "presents",
    "underwent",
    "undergoing",
    "undergone",
    "information",
    "info",
    "age",
    "aged",
    "history",
    "safety",
    "selection",
    "outcome",
    "outcomes",
    "consent",
    "satisfaction",
    "education",
    "characteristics",
    "demographics",
    "management",
    "factors",
    "pathway",
    "care",
    "related",
    "reported",
    "positioning",
    "preparation",
    "recovery",
    "details",
    "data",
    "name",
    "number",
    "record",
    "records",
    "notes",
    "note",
    "report",
    "summary",
    "discharge",
    "admission",
    "profile",
    "population",
    "cohort",
    "group",
    "experience",
    "perspective",
    "perspectives",
    "preference",
    "preferences",
    "hct",
    "wbc",
    "plt",
    "cea",
    "bmi",
    "asa",
    "pod",
    "ecog",
    "hba",
    "inr",
    "ast",
    "alt",
    "alp",
    "ggt",
    "crp",
    "was",
    "developed",
    "required",
    "received",
    "tolerated",
    "remained",
    "became",
    "complained",
    "denied",
    "reports",
    "refused",
    "agreed",
    "died",
    "recovered",
    "improved",
    "deteriorated",
    "doing",
    "stable",
    "unstable",
    "febrile",
    "afebrile",
    "jaundiced",
    "icteric",
    "pale",
    "obese",
    "diabetic",
    "hypertensive",
    "allergic",
    "positive",
    "negative",
    "normal",
    "abnormal",
];

fn name_check_titled(_: &str, c: &Captures) -> bool {
    let w = word_key(c.name("name").map_or("", |m| m.as_str()));
    !EPONYMS.contains(&w.as_str())
}

fn name_check_context(_: &str, c: &Captures) -> bool {
    let w = word_key(c.name("name").map_or("", |m| m.as_str()));
    !EPONYMS.contains(&w.as_str()) && !NOT_NAMES.contains(&w.as_str())
}

// ---------- address ----------

fn digit_nearby(text: &str, s: usize, e: usize) -> bool {
    let mut lo = s.saturating_sub(20);
    while !text.is_char_boundary(lo) {
        lo -= 1;
    }
    let mut hi = (e + 20).min(text.len());
    while !text.is_char_boundary(hi) {
        hi += 1;
    }
    text[lo..s].chars().any(|c| c.is_ascii_digit())
        || text[e..hi].chars().any(|c| c.is_ascii_digit())
}

fn locality_check(text: &str, c: &Captures) -> bool {
    let (s, e) = span(c);
    digit_nearby(text, s, e)
}

// ---------- rules ----------

const MONTH_NO_MAY: &str = "jan(?:uary)?|feb(?:ruary)?|mar(?:ch)?|apr(?:il)?|june?|july?|aug(?:ust)?|sep(?:t(?:ember)?)?|oct(?:ober)?|nov(?:ember)?|dec(?:ember)?";
const DAY: &str = r"(?:0?[1-9]|[12]\d|3[01])";

static RULES: LazyLock<Vec<Rule>> = LazyLock::new(|| {
    use FindingKind::*;
    let day_first = format!(
        r"(?i)\b{DAY}(?:st|nd|rd|th)?[ \-]+(?:of\s+)?(?:{MONTH_NO_MAY})\b\.?(?:,?\s+\d{{4}}\b)?"
    );
    let day_first_may =
        format!(r"(?i)\b{DAY}(?:st|nd|rd|th)\s+(?:of\s+)?may\b|\b{DAY}\s+may,?\s+\d{{4}}\b");
    let month_first = format!(
        r"\b(?:Jan(?:uary)?|Feb(?:ruary)?|Mar(?:ch)?|Apr(?:il)?|May|June?|July?|Aug(?:ust)?|Sep(?:t(?:ember)?)?|Oct(?:ober)?|Nov(?:ember)?|Dec(?:ember)?)\.?\s+{DAY}(?:st|nd|rd|th)?\b(?:,?\s+\d{{4}}\b)?"
    );
    let numeric_date = format!(r"\b{DAY}[/.\-](?:0?[1-9]|1[0-2])[/.\-](?:\d{{4}}|\d{{2}})\b");

    vec![
        // Aadhaar: 12 digits, first 2-9, 4-4-4 with space/hyphen or contiguous.
        rule(
            Aadhaar,
            r"\b[2-9]\d{3}[ -]?\d{4}[ -]?\d{4}\b",
            standalone_number,
        ),
        // PAN: AAAAA9999A.
        rule(Pan, r"\b[A-Z]{5}\d{4}[A-Z]\b", always),
        // Indian mobile: optional +91 / 91 / 0, 10 digits starting 6-9.
        rule(
            Phone,
            r"(?:\+91[ -]?|\b91[ -]|\b0)?(?:[6-9]\d{4}[ -]?\d{5}|[6-9]\d{2}[ -]\d{3}[ -]\d{4}|[6-9]\d{3}[ -]\d{3}[ -]\d{3})\b",
            standalone_number,
        ),
        // Landline with STD code (0-prefixed, or +91 prefixed).
        rule(
            Phone,
            r"(?:\+91[ -]?)?\(?\b0\d{2,4}\)?[ -]?\d{3,4}[ -]?\d{3,4}\b",
            standalone_number,
        ),
        rule(
            Phone,
            r"\+91[ -]?\(?\d{2,5}\)?[ -]?\d{3,4}[ -]?\d{3,4}\b",
            standalone_number,
        ),
        rule(
            Email,
            r"[A-Za-z0-9._%+\-]+@[A-Za-z0-9\-]+(?:\.[A-Za-z0-9\-]+)*\.[A-Za-z]{2,}",
            always,
        ),
        // Dates.
        rule(CalendarDate, &numeric_date, standalone_number),
        rule(
            CalendarDate,
            r"\b(?:19|20)\d{2}[/.\-](?:0?[1-9]|1[0-2])[/.\-]{1}(?:0?[1-9]|[12]\d|3[01])\b",
            standalone_number,
        ),
        rule(CalendarDate, &day_first, always),
        rule(CalendarDate, &day_first_may, always),
        rule(CalendarDate, &month_first, always),
        // Hospital IDs: label then an ID containing at least one digit.
        rule(
            HospitalId,
            r"(?i)\b(?:UHID|MRN|IPD|IP\s*(?:no|number)|OP\s*(?:no|number)|Reg(?:istration)?\.?\s*(?:no|number)|Hospital\s*(?:no|number)|Patient\s*ID)\b\.?\s*[:#.\-]*\s*#?\s*(?:[A-Za-z]{1,6}[\-/]?)?\d[A-Za-z0-9/\-]*",
            always,
        ),
        // Names: title + capitalised word (eponyms excepted).
        rule(
            PersonName,
            r"\b(?:Mr|Mrs|Ms|Miss|Shri|Smt|Kumari|Master|Baby\s+of)\b\.?\s+(?P<name>[A-Z][A-Za-z]+)",
            name_check_titled,
        ),
        // Names: patient-context word + capitalised word.
        rule(
            PersonName,
            r"\b(?:[Pp]atient|[Pp]t|[Nn]ame|named|called)\b\.?(?:\s*:\s*|\s+)(?:(?:is|was)\s+)?(?P<name>[A-Z][a-z]{2,})\b",
            name_check_context,
        ),
        // Addresses.
        rule(
            Address,
            r"(?i)\b(?:door|flat|house|plot)\s*(?:no\.?|number|#)\s*[A-Za-z]?\d[\w/\-]*",
            always,
        ),
        rule(
            Address,
            r"(?i)\B#\s?\d{1,5}[A-Za-z]?\b[^\n.;]{0,40}?\b(?:cross|main|road|rd|layout|nagar|street|colony|sector)\b",
            always,
        ),
        rule(
            Address,
            r"(?i)\b\d{1,3}(?:st|nd|rd|th)?\s+(?:cross|main\s+(?:road|rd))\b|\b\d{1,3}(?:st|nd|rd|th)\s+main\s*,",
            always,
        ),
        rule(Address, r"\b[A-Za-z]*[nN]agar\b", locality_check),
        rule(Address, r"\b[A-Z][A-Za-z]+\s+Layout\b", locality_check),
        rule(Address, r"\b[1-8]\d{5}\b", pin_check),
    ]
});

/// Scan `text` for patient identifiers. Findings are sorted by `start`;
/// overlapping findings are merged into one (kind of the earliest/longest).
pub fn scan(text: &str) -> Vec<Finding> {
    let mut found: Vec<Finding> = Vec::new();
    for r in RULES.iter() {
        // Manual iteration: when a candidate is rejected we restart one char after
        // its start, so a rejected "Patient Name" cannot swallow "Name: Ramesh".
        let mut pos = 0;
        while pos <= text.len() {
            let Some(caps) = r.re.captures_at(text, pos) else {
                break;
            };
            let (start, end) = span(&caps);
            if (r.check)(text, &caps) {
                found.push(Finding {
                    kind: r.kind,
                    start,
                    end,
                });
                pos = end.max(start + 1);
            } else {
                pos = start + 1;
            }
            while pos < text.len() && !text.is_char_boundary(pos) {
                pos += 1;
            }
        }
    }
    found.sort_by(|a, b| a.start.cmp(&b.start).then(b.end.cmp(&a.end)));
    let mut merged: Vec<Finding> = Vec::with_capacity(found.len());
    for f in found {
        match merged.last_mut() {
            Some(last) if f.start < last.end => last.end = last.end.max(f.end),
            _ => merged.push(f),
        }
    }
    merged
}
