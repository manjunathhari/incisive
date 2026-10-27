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
//! - A bare 6-digit number is a PIN code only with an address cue (a `pin` label,
//!   a capitalised place word right before it, or another address finding nearby).
//! - Digits are ASCII only; Devanagari-digit numbers are not scanned.
//! - Unicode spaces (NBSP, narrow NBSP, thin space), en/em dashes and the
//!   full-width `@` are accepted as separators; offsets stay on the original text.

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

type Check = fn(&str, &Captures, &[Finding]) -> bool;

struct Rule {
    kind: FindingKind,
    re: Regex,
    check: Check,
    /// Runs after all other rules and may look at their (Address) findings.
    deferred: bool,
}

// Pattern placeholders (expanded by `pat`).
const H: &str = r"[ \x{A0}\x{202F}\x{2009}\-\x{2013}\x{2014}]";
const A: &str = r"[ \x{A0}\x{202F}\x{2009}\-\x{2013}\x{2014}.]{0,2}";
const SP: &str = r"[ \x{A0}\x{202F}\x{2009}]";
const DS: &str = r"[/.\-\x{2013}\x{2014}]";
const DAY: &str = r"(?:0?[1-9]|[12][0-9]|3[01])";
const MON: &str = "jan(?:uary)?|feb(?:ruary)?|mar(?:ch)?|apr(?:il)?|june?|july?|aug(?:ust)?|sep(?:t(?:ember)?)?|oct(?:ober)?|nov(?:ember)?|dec(?:ember)?";

/// Expand placeholders and force ASCII digits.
fn pat(p: &str) -> String {
    p.replace("$DAY", DAY)
        .replace("$MON", MON)
        .replace("$DS", DS)
        .replace("$SP", SP)
        .replace("$H", H)
        .replace("$A", A)
        .replace(r"\d", "[0-9]")
}

fn rule(kind: FindingKind, pattern: &str, check: Check) -> Rule {
    Rule {
        kind,
        re: Regex::new(&pat(pattern)).expect("deid regex compiles"),
        check,
        deferred: false,
    }
}

fn always(_: &str, _: &Captures, _: &[Finding]) -> bool {
    true
}

// ---------- helpers ----------

fn span(c: &Captures) -> (usize, usize) {
    let m = c.get(0).expect("group 0");
    (m.start(), m.end())
}

fn floor_boundary(text: &str, mut i: usize) -> usize {
    while i > 0 && !text.is_char_boundary(i) {
        i -= 1;
    }
    i
}

fn ceil_boundary(text: &str, mut i: usize) -> usize {
    i = i.min(text.len());
    while !text.is_char_boundary(i) {
        i += 1;
    }
    i
}

/// True when the match is glued to neighbouring digits ("1234567890123",
/// "2345-6789-0123-4567", "1.150000"), i.e. it is part of a longer number.
fn inside_longer_number(text: &str, start: usize, end: usize) -> bool {
    let sep = |c: char| matches!(c, '.' | '-' | '/' | '\u{2013}' | '\u{2014}');
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

fn standalone_number(text: &str, c: &Captures, _: &[Finding]) -> bool {
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

/// Lab/count/analyte/quantity words: a 6-digit number near one is a measurement.
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
    "patients",
    "cases",
    "total",
    "size",
    "n",
    "cost",
    "rupees",
    "ferritin",
    "afp",
    "cea",
    "ca",
    "dimer",
    "d-dimer",
    "load",
    "copies",
    "creatinine",
    "glucose",
    "lipase",
    "amylase",
    "ldh",
    "crp",
    "esr",
    "wcc",
    "neutrophils",
    "lymphocytes",
];

const UNIT_PREFIXES: &[&str] = &[
    "/", "cells", "cell", "iu", "units", "unit", "mg", "ml", "mm", "cc", "mcg", "ug", "ng", "pg",
    "cu", "per", "%", "\u{b5}", "\u{3bc}", "copies", "patients", "cases", "rupees",
];

const PIN_LABELS: &[&str] = &[
    "pin",
    "pincode",
    "pin code",
    "pin-code",
    "postcode",
    "post code",
    "postal code",
    "zip",
    "zipcode",
];

static PLACE_BEFORE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"[A-Z][a-z]{2,}(?:\s*[,\-\x{2013}\x{2014}]\s*|\s+)$").expect("place regex")
});

fn pin_label_before(text: &str, s: usize) -> bool {
    let lo = floor_boundary(text, s.saturating_sub(16));
    let pre = text[lo..s]
        .trim_end_matches(|c: char| {
            c.is_whitespace() || matches!(c, ':' | '-' | '.' | '#' | ',' | '(' | '\u{2013}')
        })
        .to_lowercase();
    PIN_LABELS.iter().any(|l| {
        pre.strip_suffix(l).is_some_and(|rest| {
            !rest
                .chars()
                .next_back()
                .is_some_and(|c| c.is_alphanumeric())
        })
    })
}

fn pin_check(text: &str, c: &Captures, addr: &[Finding]) -> bool {
    let (s, e) = span(c);
    if inside_longer_number(text, s, e) {
        return false;
    }
    // A lab/count word within the previous 3 tokens means this is a count.
    let lab_before = text[..s].split_whitespace().rev().take(3).any(|t| {
        let k = t
            .trim_matches(|ch: char| ch.is_ascii_punctuation())
            .to_lowercase();
        LAB_WORDS.contains(&k.as_str()) || t.contains('\u{20b9}')
    });
    if lab_before {
        return false;
    }
    // Followed by a unit means this is a measurement (bounded lookahead).
    let hi = ceil_boundary(text, e + 16);
    let rest = text[e..hi].trim_start().to_lowercase();
    if UNIT_PREFIXES.iter().any(|u| rest.starts_with(u)) {
        return false;
    }
    // Address cue: a pin label, a place word right before, or an address nearby.
    if pin_label_before(text, s) {
        return true;
    }
    let lo = floor_boundary(text, s.saturating_sub(40));
    if PLACE_BEFORE.is_match(&text[lo..s]) {
        return true;
    }
    let from = addr.partition_point(|f| f.start < s.saturating_sub(200));
    addr[from..]
        .iter()
        .take_while(|f| f.start <= e + 60)
        .any(|f| f.start <= e + 60 && f.end + 60 >= s)
}

// ---------- names ----------

/// Surgical/medical eponyms that look like surnames.
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
    "murphy",
    "courvoisier",
    "boerhaave",
    "mallory",
    "zollinger",
    "charlson",
    "caprini",
    "kaplan",
    "cox",
    "gleason",
    "breslow",
    "fontaine",
    "rutherford",
    "bosniak",
];

/// Words that follow a titled eponym in procedure context ("Hartmann's", "Hartmann pouch").
const PROC_WORDS: &[&str] = &[
    "procedure",
    "procedures",
    "pouch",
    "operation",
    "sign",
    "fundoplication",
    "repair",
    "triad",
    "classification",
    "score",
    "criteria",
    "colostomy",
    "reversal",
    "anastomosis",
    "resection",
    "myotomy",
    "technique",
    "disease",
    "stage",
    "staging",
];

/// Common nouns that follow Ms/Miss/Master etc. in non-name uses.
const TITLE_COMMON: &[&str] = &[
    "excel",
    "class",
    "classes",
    "rate",
    "rates",
    "plan",
    "list",
    "key",
    "mind",
    "thesis",
    "degree",
    "regimen",
    "blood",
    "controlled",
];

/// Common capitalised words that follow "patient"/"pt"/"name"/"c/o" in clinical prose.
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
    "blood",
    "controlled",
    "excel",
    "class",
    "rate",
    "wise",
    // symptoms / findings (c/o, w/o are common clinical abbreviations)
    "abdominal",
    "pain",
    "fever",
    "fevers",
    "vomiting",
    "nausea",
    "bleeding",
    "jaundice",
    "swelling",
    "breathlessness",
    "cough",
    "loose",
    "constipation",
    "weight",
    "loss",
    "chest",
    "headache",
    "dysphagia",
    "diarrhoea",
    "diarrhea",
    "burning",
    "difficulty",
    "lump",
    "mass",
    "distension",
    "giddiness",
    "weakness",
    "fatigue",
    "itching",
    "dyspnea",
    "dyspnoea",
    "abdomen",
    "right",
    "left",
    "upper",
    "lower",
    "epigastric",
    "generalised",
    "generalized",
    "recurrent",
    "persistent",
    "progressive",
    "acute",
    "chronic",
    "decreased",
    "reduced",
    "increased",
    "black",
    "yellowish",
    "yellow",
    "bloating",
    "anorexia",
    "palpitations",
    "syncope",
    "haematuria",
    "hematuria",
    "melena",
    "diabetes",
    "hypertension",
    "comorbidities",
    "complications",
    "symptoms",
    "evidence",
    "features",
    "signs",
    "any",
];

fn followed_by_procedure(rest: &str) -> bool {
    if rest.starts_with("'s") || rest.starts_with('\u{2019}') {
        return true;
    }
    let w: String = rest
        .trim_start()
        .chars()
        .take_while(|c| c.is_alphabetic())
        .collect::<String>()
        .to_lowercase();
    PROC_WORDS.contains(&w.as_str())
}

fn name_check_titled(text: &str, c: &Captures, _: &[Finding]) -> bool {
    let w = word_key(c.name("name").map_or("", |m| m.as_str()));
    if TITLE_COMMON.contains(&w.as_str()) {
        return false;
    }
    if EPONYMS.contains(&w.as_str()) {
        let (_, e) = span(c);
        return !followed_by_procedure(&text[e..]);
    }
    true
}

fn name_check_context(_: &str, c: &Captures, _: &[Finding]) -> bool {
    let w = word_key(c.name("name").map_or("", |m| m.as_str()));
    !EPONYMS.contains(&w.as_str()) && !NOT_NAMES.contains(&w.as_str())
}

// ---------- address ----------

fn digit_nearby(text: &str, s: usize, e: usize) -> bool {
    let lo = floor_boundary(text, s.saturating_sub(20));
    let hi = ceil_boundary(text, e + 20);
    text[lo..s].chars().any(|c| c.is_ascii_digit())
        || text[e..hi].chars().any(|c| c.is_ascii_digit())
}

fn locality_check(text: &str, c: &Captures, _: &[Finding]) -> bool {
    let (s, e) = span(c);
    digit_nearby(text, s, e)
}

// ---------- rules ----------

static RULES: LazyLock<Vec<Rule>> = LazyLock::new(|| {
    use FindingKind::*;
    let mut pin = rule(Address, r"\b[1-8]\d{5}\b|\b[1-8]\d{2}$SP\d{3}\b", pin_check);
    pin.deferred = true;

    vec![
        // Aadhaar: 12 digits, first 2-9, 4-4-4 with space/hyphen/dot or contiguous.
        rule(Aadhaar, r"\b[2-9]\d{3}$A\d{4}$A\d{4}\b", standalone_number),
        // PAN: AAAAA9999A.
        rule(Pan, r"\b[A-Z]{5}\d{4}[A-Z]\b", always),
        // Indian mobile: optional +91 / 91 / 0, 10 digits starting 6-9.
        rule(
            Phone,
            r"(?:\+91$H?|\b91$H|\b0)?(?:[6-9]\d{4}$H?\d{5}|[6-9]\d{2}$H\d{3}$H\d{4}|[6-9]\d{3}$H\d{3}$H\d{3}|[6-9]\d$H\d{3}$H\d{5})\b",
            standalone_number,
        ),
        // Landline with STD code (0-prefixed, or +91 prefixed).
        rule(
            Phone,
            r"(?:\+91$H?)?\(?\b0\d{2,4}\)?$H?\d{3,4}$H?\d{3,4}\b",
            standalone_number,
        ),
        rule(
            Phone,
            r"\+91$H?\(?\d{2,5}\)?$H?\d{3,4}$H?\d{3,4}\b",
            standalone_number,
        ),
        rule(
            Email,
            r"[A-Za-z0-9._%+\-]+[@\x{FF20}][A-Za-z0-9\-]+(?:\.[A-Za-z0-9\-]+)*\.[A-Za-z]{2,}",
            always,
        ),
        // Dates.
        rule(
            CalendarDate,
            r"\b$DAY$DS(?:0?[1-9]|1[0-2])$DS(?:\d{4}|\d{2})\b",
            standalone_number,
        ),
        rule(
            CalendarDate,
            r"\b(?:19|20)\d{2}$DS(?:0?[1-9]|1[0-2])$DS$DAY\b",
            standalone_number,
        ),
        rule(
            CalendarDate,
            r"(?i)\b$DAY(?:st|nd|rd|th)?$H+(?:of\s+)?(?:$MON)\b\.?(?:,?$H+\d{4}\b|[\-\x{2013}\x{2014}']\d{2}\b)?",
            always,
        ),
        rule(
            CalendarDate,
            r"(?i)\b$DAY(?:st|nd|rd|th)$H+(?:of\s+)?may\b|\b$DAY$H+may,?$H+\d{4}\b",
            always,
        ),
        rule(
            CalendarDate,
            r"\b(?:Jan(?:uary)?|Feb(?:ruary)?|Mar(?:ch)?|Apr(?:il)?|May|June?|July?|Aug(?:ust)?|Sep(?:t(?:ember)?)?|Oct(?:ober)?|Nov(?:ember)?|Dec(?:ember)?)\.?\s+$DAY(?:st|nd|rd|th)?\b(?:,?\s+\d{4}\b)?",
            always,
        ),
        // Hospital IDs: label then an ID containing at least one digit.
        rule(
            HospitalId,
            r"(?i)\b(?:(?:UHID|MRN|IPD|Patient\s*ID)(?:\s*(?:no|number)\b)?|(?:MRD?|IP|OP|Reg(?:istration)?\.?|Hosp(?:ital)?\.?)\s*(?:no|number)\b)\.?\s*[:#.\-\x{2013}]*\s*#?\s*(?:[A-Za-z]{1,6}[\-/ ]?)?\d[A-Za-z0-9/\-]*",
            always,
        ),
        rule(HospitalId, r"\bIP\s+\d+(?:/\d+)?\b", always),
        // Names: title + capitalised word (eponyms only exempt in procedure context).
        rule(
            PersonName,
            r"\b(?:Mr|Mrs|Ms|Miss|Shri|Smt|Kumari|Master|Baby\s+of)\b\.?\s*(?P<name>[A-Z][A-Za-z]+)",
            name_check_titled,
        ),
        // All-caps titles need an all-caps name ("MR" alone is MR imaging).
        rule(
            PersonName,
            r"\b(?:MR|MRS|SHRI|SMT|KUMARI)\b\.?\s*(?P<name>[A-Z]{3,})\b",
            name_check_titled,
        ),
        // Names: patient-context word + capitalised word.
        rule(
            PersonName,
            r"\b(?:[Pp]atient|[Pp]t|[Nn]ame|named|called)\b\.?(?:\s*[:,\-\x{2013}\x{2014}]\s*|\s+)(?:(?:is|was)\s+)?(?P<name>[A-Z][a-z]{2,})\b",
            name_check_context,
        ),
        // Relations: s/o, w/o, d/o, c/o + name.
        rule(
            PersonName,
            r"(?i:\b[sdwc]/o\b)\.?\s*(?:(?:Mr|Mrs|Shri|Smt)\.?\s*)?(?P<name>[A-Z][A-Za-z]{2,})",
            name_check_context,
        ),
        // Addresses.
        rule(
            Address,
            r"(?i)\b(?:door|flat|house|plot)\s*(?:no\.?|number|#)\s*[A-Za-z]?\d[A-Za-z0-9/\-]*",
            always,
        ),
        rule(
            Address,
            r"\b(?:Door|Flat|House|Plot)\s+\d[A-Za-z0-9/\-]*",
            always,
        ),
        rule(Address, r"\bNo\.\s*\d+[A-Za-z]?,\s*[A-Z]", always),
        rule(Address, r"\bSector\s+\d{1,3}\b", always),
        rule(
            Address,
            r"(?i)\B#\s?\d{1,5}[A-Za-z]?\b[^\n.;]{0,40}?\b(?:cross,|cross\s+(?:road|rd)\b|main\s+(?:road|rd)\b|road\b|rd\b|layout\b|nagar\b|street\b|colony\b|sector\b)",
            always,
        ),
        rule(
            Address,
            r"(?i)\b\d{1,3}(?:st|nd|rd|th)\s+cross\b(?:\s*,|\s+(?:road|rd|main|street)\b|\.?\s*$)|\b\d{1,3}(?:st|nd|rd|th)?\s+(?:cross|main)\s+(?:road|rd)\b",
            always,
        ),
        rule(
            Address,
            r"\b\d{1,4}[A-Za-z]?,?\s+(?:[A-Z][A-Za-z.]*\s+){1,3}(?:Road|Rd|Street|Marg|Lane|Avenue)\b",
            always,
        ),
        rule(
            Address,
            r"\b[A-Z][a-z]+\s+\d{1,2}(?:st|nd|rd|th)\s+(?:Block|Phase)\b",
            always,
        ),
        rule(Address, r"\b[A-Za-z]*[nN]agar\b", locality_check),
        rule(Address, r"\b[A-Z][A-Za-z]+\s+Layout\b", locality_check),
        pin,
    ]
});

fn run_rule(r: &Rule, text: &str, ctx: &[Finding], out: &mut Vec<Finding>) {
    // Manual iteration: when a candidate is rejected we restart one char after
    // its start, so a rejected "Patient Name" cannot swallow "Name: Ramesh".
    let mut pos = 0;
    while pos <= text.len() {
        let Some(caps) = r.re.captures_at(text, pos) else {
            break;
        };
        let (start, end) = span(&caps);
        if (r.check)(text, &caps, ctx) {
            out.push(Finding {
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

/// Scan `text` for patient identifiers. Findings are sorted by `start`;
/// overlapping findings are merged into one (kind of the earliest/longest).
pub fn scan(text: &str) -> Vec<Finding> {
    let mut found: Vec<Finding> = Vec::new();
    for r in RULES.iter().filter(|r| !r.deferred) {
        run_rule(r, text, &[], &mut found);
    }
    let mut addr: Vec<Finding> = found
        .iter()
        .copied()
        .filter(|f| f.kind == FindingKind::Address)
        .collect();
    addr.sort_by_key(|f| f.start);
    for r in RULES.iter().filter(|r| r.deferred) {
        run_rule(r, text, &addr, &mut found);
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
