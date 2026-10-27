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
//! - Names are only caught after a title, a patient-context word or a relation
//!   (`s/o` ...), and only in ASCII letters; lowercase and non-Latin names are not caught.
//! - Name words are decided by two explicit lists: a CLINICAL lexicon (never a name) and a
//!   GIVEN-NAME allowlist (always a name candidate). Any other capitalised word of 3+
//!   letters is a candidate; word endings alone never decide.
//! - `#12` alone is not an address; it needs a street word nearby.
//! - A bare 6-digit number is a PIN code only with an address cue: a `pin` label (always
//!   wins), an "Address" label or another address finding nearby, or a capitalised place
//!   word right before it. A count noun after it ("150000 gastrectomies") overrides only
//!   the place-word cue.
//! - `c/o`, `w/o`, `h/o` ("complains of", "without", "history of") + a bare single word
//!   that is not on the given-name allowlist is NOT treated as a name ("c/o Puttaswamy"
//!   passes: accepted gap; "c/o Ramesh" blocks because Ramesh is allowlisted). They also
//!   block with a title, an age/sex marker after the name ("56M", "aged 56", "56 yrs/M";
//!   a bare duration such as "20 years" is not an age), or a second name-candidate word.
//!   `c/o` and `w/o` also block with a comma age ("c/o Puttaswamy, 56 yrs") unless a
//!   duration word follows; "56 years of age" and "56 years, history of ..." are ages,
//!   "12 years of history" is not. `s/o`, `d/o`, "son of", "wife of" + a name candidate
//!   always block, unless a clinical phrase follows (clinical words ending in a head
//!   noun, no label colon: "s/o Sebaceous Cyst"; not "S/O MUNIYAPPA OCCUPATION: ...").
//!   A field label (Diagnosis, Prognosis, Occupation, Admission, Religion, Residence,
//!   Address, Impression, Findings, Provisional, Final, Diagnosed) or a connective
//!   (With, Without, And, For) before the head noun stops the walk as NOT exempt, colon
//!   or not ("S/O MUNIYAPPA DIAGNOSIS HERNIA", "S/O MUNIYAPPA WITH INGUINAL HERNIA").
//! - A "Name" label ("Name", "NAME", "Patient Name", "Pt. Name", any case, optional space
//!   before ":" or "-") blocks a Title-case or ALL-CAPS value that is a name candidate
//!   (allowlisted given names always; clinical words never). It does not block after a
//!   non-person qualifier ("Brand name:", "Drug name:", "Trial name:"), for a placeholder
//!   value ("Name: Unknown"), or without a separator ("NAME OF SURGERY: ...").
//! - Digits are ASCII only. Any other numeral (Devanagari, Kannada, full-width ...),
//!   any Unicode format character (zero-width space, soft hyphen, bidi controls ...),
//!   anything in the Halfwidth/Fullwidth Forms block and look-alike separators are
//!   blocked as `UnusualCharacters` (rejected, never normalised). Exceptions: a short
//!   superscript/subscript unit mark ("kg/m\u{B2}", "\u{D7}10\u{2079}/L", "SpO\u{2082}"), one
//!   vulgar fraction, and look-alike dashes that are not between two digits.
//!   Zero-width joiners used in some Indic scripts are therefore blocked too.
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
    /// Hidden (format) characters, non-ASCII numerals, full-width forms or
    /// look-alike separators. Rejected, never normalised (ruling R-F1).
    UnusualCharacters,
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
        FindingKind::UnusualCharacters => "hidden or unusual characters",
    }
}

/// Extra advice for the user, for kinds where removing the span is not enough.
pub fn hint(kind: FindingKind) -> Option<&'static str> {
    match kind {
        FindingKind::UnusualCharacters => {
            Some("please retype the question without copy-pasted special characters")
        }
        _ => None,
    }
}

type Check = fn(&str, &Captures, &[Finding]) -> bool;

struct Rule {
    kind: FindingKind,
    re: Regex,
    check: Check,
    /// Runs after all other rules and may look at their (Address) findings.
    deferred: bool,
    /// Extend the finding over up to this many following name-like words.
    extra_words: usize,
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
        extra_words: 0,
    }
}

impl Rule {
    fn names(mut self, extra: usize) -> Self {
        self.extra_words = extra;
        self
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
    "leucocytes",
    "leukocytes",
];

const UNIT_PREFIXES: &[&str] = &[
    "/\u{b5}", "/\u{3bc}", "/ul", "/cumm", "/mm", "/cu", "cells", "cell", "iu", "units", "unit",
    "mg", "ml", "mm", "cc", "mcg", "ug", "ng", "pg", "cu", "per", "%", "\u{b5}", "\u{3bc}",
    "copies", "patients", "cases", "rupees",
];

/// Narrower lab list for phone-vs-range vetoes ("platelets 65000-80000").
const PHONE_VETO_WORDS: &[&str] = &[
    "platelets",
    "platelet",
    "plt",
    "plts",
    "count",
    "wbc",
    "tlc",
    "iu",
    "cells",
    "cumm",
    "ferritin",
    "afp",
    "cea",
    "dimer",
    "d-dimer",
    "load",
    "copies",
    "esr",
    "crp",
    "ldh",
    "lipase",
    "amylase",
    "creatinine",
    "glucose",
    "wcc",
    "neutrophils",
    "lymphocytes",
    "leucocytes",
    "leukocytes",
];

/// Real lab units that follow a count (case-sensitive, word boundary required).
const PHONE_UNITS: &[&str] = &[
    "/cumm", "/mm3", "/\u{b5}L", "/\u{b5}l", "/uL", "/ul", "/cu", "cells", "copies",
];

/// Words that label a number as a phone number.
const PHONE_LABELS: &[&str] = &[
    "ph", "phone", "mob", "mobile", "call", "contact", "tel", "whatsapp", "attender", "cell",
];

fn token_key(t: &str) -> String {
    t.trim_matches(|ch: char| ch.is_ascii_punctuation())
        .to_lowercase()
}

/// Starts with one of `units` (case-sensitive) followed by a non-alphanumeric char.
fn starts_with_unit(rest: &str, units: &[&str]) -> bool {
    units.iter().any(|u| {
        rest.strip_prefix(u)
            .is_some_and(|r| !r.chars().next().is_some_and(|c| c.is_alphanumeric()))
    })
}

/// A phone label within the previous 3 tokens. "pH" (acidity) is never a label.
fn phone_label_before(text: &str, s: usize) -> bool {
    text[..s].split_whitespace().rev().take(3).any(|t| {
        let raw = t.trim_matches(|ch: char| ch.is_ascii_punctuation());
        raw != "pH" && PHONE_LABELS.contains(&raw.to_lowercase().as_str())
    })
}

/// "65000-80000": two dash-separated round numbers in ascending order are a range.
fn looks_like_range(m: &str) -> bool {
    let parts: Vec<&str> = m
        .split(['-', '\u{2013}', '\u{2014}'])
        .map(str::trim)
        .collect();
    let [a, b] = parts.as_slice() else {
        return false;
    };
    match (a.parse::<u64>(), b.parse::<u64>()) {
        (Ok(x), Ok(y)) => x < y && x % 100 == 0 && y % 100 == 0,
        _ => false,
    }
}

fn phone_check(text: &str, c: &Captures, _: &[Finding]) -> bool {
    let (s, e) = span(c);
    if inside_longer_number(text, s, e) {
        return false;
    }
    // A phone label nearby always wins over lab-value heuristics.
    if phone_label_before(text, s) {
        return true;
    }
    let before: Vec<String> = text[..s]
        .split_whitespace()
        .rev()
        .take(3)
        .map(token_key)
        .collect();
    // A hyphen/en-dash range after a lab word is a lab range ("platelets 65000-80000").
    let m = &text[s..e];
    let ranged = m.contains(['-', '\u{2013}', '\u{2014}']);
    if ranged
        && (looks_like_range(m)
            || before
                .iter()
                .any(|k| PHONE_VETO_WORDS.contains(&k.as_str())))
    {
        return false;
    }
    // Followed by a real unit: a count, not a phone.
    let hi = ceil_boundary(text, e + 12);
    let rest = text[e..hi].trim_start();
    let iu = rest.get(..2).is_some_and(|u| u.eq_ignore_ascii_case("iu"))
        && !rest[2..].chars().next().is_some_and(char::is_alphanumeric);
    !(iu || starts_with_unit(rest, PHONE_UNITS))
}

/// Any 10-digit mobile grouping ("98450 12 345"), only right after a phone label.
fn labelled_phone(text: &str, c: &Captures, _: &[Finding]) -> bool {
    let (s, e) = span(c);
    !inside_longer_number(text, s, e) && phone_label_before(text, s)
}

/// Multi-letter units, matched case-insensitively ("8 mm", "2 CM", "500 MG").
const UNITS_AFTER_NUMBER: &[&str] = &["mmhg", "mcg", "mm", "cm", "mg", "ml", "kg", "iu"];

/// True when the text right after `e` is a measurement unit. Single-letter `g` counts
/// only in lower case ("1 g"), so "45 G Block" stays an address.
fn unit_follows(text: &str, e: usize) -> bool {
    let hi = ceil_boundary(text, e + 8);
    let rest = text[e..hi].trim_start();
    if rest.starts_with('%') {
        return true;
    }
    let lower = rest.to_ascii_lowercase();
    starts_with_unit(&lower, UNITS_AFTER_NUMBER) || starts_with_unit(rest, &["g"])
}

fn no_unit_after(text: &str, c: &Captures, _: &[Finding]) -> bool {
    let (s, e) = span(c);
    // "Flat 30-degree scope": the ID pattern swallows "-degree".
    if text[s..e].to_lowercase().contains("-degree") {
        return false;
    }
    !unit_follows(text, e)
}

/// "Case No. 2, A 60-year-old..." is a numbered case, not an address.
fn numbered_label_ok(text: &str, c: &Captures, _: &[Finding]) -> bool {
    let (s, _) = span(c);
    let prev: String = text[..s]
        .trim_end()
        .chars()
        .rev()
        .take_while(|ch| ch.is_alphabetic())
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<String>()
        .to_lowercase();
    !matches!(
        prev.as_str(),
        "case" | "table" | "fig" | "figure" | "group" | "arm" | "study" | "patient"
    )
}

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

/// Nouns that make a preceding 6-digit number a count ("150000 gastrectomies").
const COUNT_NOUNS: &[&str] = &[
    "patients",
    "procedures",
    "admissions",
    "beneficiaries",
    "cases",
    "gastrectomies",
    "cholecystectomies",
    "resections",
    "operations",
    "subjects",
    "participants",
    "copies",
    "cells",
    "surgeries",
    "deaths",
    "births",
    "people",
    "individuals",
    "adults",
    "children",
    "samples",
    "specimens",
    "records",
    "encounters",
    "discharges",
];

fn count_noun(w: &str) -> bool {
    COUNT_NOUNS.contains(&w)
        || ["ectomies", "otomies", "ostomies", "plasties", "scopies"]
            .iter()
            .any(|s| w.ends_with(s))
}

/// "Address" / "addr" within the previous 60 bytes.
fn address_label_before(text: &str, s: usize) -> bool {
    let lo = floor_boundary(text, s.saturating_sub(60));
    text[lo..s]
        .split(|c: char| !c.is_alphabetic())
        .any(|w| w.eq_ignore_ascii_case("address") || w.eq_ignore_ascii_case("addr"))
}

fn pin_check(text: &str, c: &Captures, addr: &[Finding]) -> bool {
    let (s, e) = span(c);
    if inside_longer_number(text, s, e) {
        return false;
    }
    // A pin label always wins.
    if pin_label_before(text, s) {
        return true;
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
    let rest_full = &text[e..hi];
    let rest = rest_full.trim_start().to_lowercase();
    if UNIT_PREFIXES.iter().any(|u| rest.starts_with(u)) {
        return false;
    }
    // Strong address cues: an "Address" label or another address finding nearby.
    let from = addr.partition_point(|f| f.start < s.saturating_sub(200));
    let addr_near = addr[from..]
        .iter()
        .take_while(|f| f.start <= e + 60)
        .any(|f| f.end + 60 >= s);
    if addr_near || address_label_before(text, s) {
        return true;
    }
    // "Japan 150000 gastrectomies", "Overall 120000 robotic cases": a count noun in the
    // next two words outweighs a capitalised word before. Verbs ("presents", "visits")
    // are not count nouns, so "Bengaluru 560038 attends clinic" stays an address.
    let counted = rest_full
        .split_whitespace()
        .take(2)
        .map(|w| {
            w.chars()
                .take_while(|ch| ch.is_alphabetic())
                .collect::<String>()
        })
        .any(|w| count_noun(&w));
    if counted {
        return false;
    }
    let lo = floor_boundary(text, s.saturating_sub(40));
    PLACE_BEFORE.is_match(&text[lo..s])
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
    "klatskin",
    "mirizzi",
    "meckel",
    "budd",
    "chiari",
    "caroli",
    "wilms",
    "zenker",
    "morgagni",
    "bochdalek",
    "amyand",
    "littre",
    "richter",
    "spigelian",
    "krukenberg",
    "virchow",
    "rovsing",
    "mcburney",
    "pfannenstiel",
    "fournier",
    "ogilvie",
    "ladd",
    "bouveret",
    "frantz",
    "vater",
    "oddi",
    "treitz",
    "morison",
    "winslow",
    "peutz",
    "jeghers",
    "dieulafoy",
    "menetrier",
    "hodgkin",
    "burkitt",
    "kaposi",
    "hashimoto",
    "paget",
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
    "manoeuvre",
    "maneuver",
    "incision",
    "triangle",
    "syndrome",
    "test",
    "point",
    "ligament",
    "node",
    "nodes",
    "line",
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

fn name_check_context(text: &str, c: &Captures, _: &[Finding]) -> bool {
    let w = word_key(c.name("name").map_or("", |m| m.as_str()));
    let (s, _) = span(c);
    let is_name_label = text[s..]
        .get(..4)
        .is_some_and(|l| l.eq_ignore_ascii_case("name"));
    if is_name_label && !name_label_ok(text, s, &w) {
        return false;
    }
    name_candidate(&w)
}

/// Words that make a following "name:" label a non-person field ("Brand name:",
/// "Drug name:", "Trial name:", "Procedure name:").
const NON_PERSON_NAME_QUALIFIERS: &[&str] = &[
    "brand",
    "drug",
    "trade",
    "generic",
    "proprietary",
    "trial",
    "study",
    "device",
    "product",
    "implant",
    "mesh",
    "instrument",
    "stapler",
    "suture",
    "procedure",
    "surgery",
    "operation",
    "test",
    "investigation",
    "medicine",
    "medication",
    "molecule",
    "compound",
    "vaccine",
    "antibiotic",
    "scheme",
    "programme",
    "program",
    "protocol",
    "file",
    "kit",
    "score",
    "scale",
    "classification",
    "technique",
    "disease",
    "diagnosis",
    "syndrome",
    "gene",
    "organism",
    "software",
    "journal",
];

/// Placeholder values after a name label, not a person ("Name: Unknown").
const NAME_PLACEHOLDERS: &[&str] = &[
    "unknown",
    "withheld",
    "anonymous",
    "anonymised",
    "anonymized",
    "redacted",
    "nil",
    "none",
    "not",
];

/// A "name" label starting at `s` names a person unless a non-person qualifier word
/// directly precedes it on the same line, or its value `k` is a placeholder.
fn name_label_ok(text: &str, s: usize, k: &str) -> bool {
    if NAME_PLACEHOLDERS.contains(&k) {
        return false;
    }
    let line = &text[..s];
    let line = &line[line.rfind('\n').map_or(0, |i| i + 1)..];
    let prev = line
        .trim_end_matches(|c: char| c.is_whitespace() || c == '.')
        .rsplit(|c: char| !c.is_alphabetic())
        .next()
        .unwrap_or("")
        .to_lowercase();
    !NON_PERSON_NAME_QUALIFIERS.contains(&prev.as_str())
}

/// Case-insensitive "Name:" / "PATIENT NAME -" label with a Title-case or ALL-CAPS value.
fn name_check_label(text: &str, c: &Captures, _: &[Finding]) -> bool {
    let w = word_key(c.name("name").map_or("", |m| m.as_str()));
    let s = c.name("label").map_or(0, |m| m.start());
    name_label_ok(text, s, &w) && name_candidate(&w)
}

/// Suffixes that make a word a diagnosis/procedure/finding, never a name.
/// (Weaker suffixes such as -al, -ia, -ion, -an also end real names: Gopal, Sonia.)
const STRONG_SUFFIXES: &[&str] = &[
    "itis", "oma", "osis", "ectomy", "emia", "aemia", "uria", "rrhoea", "rrhea", "rrhage",
    "rrhoids", "ation", "graphy", "scopy", "pathy", "plasty", "otomy", "ostomy", "ancy", "ncy",
    "sis", "gia", "ism", "cele",
];

/// More clinical words that are never names (diagnoses, findings, report headings).
const NOT_NAMES_2: &[&str] = &[
    "mucocele",
    "abscess",
    "ascites",
    "fistula",
    "volvulus",
    "leak",
    "obstruction",
    "perforation",
    "malignancy",
    "grade",
    "small",
    "bowel",
    "wall",
    "thickening",
    "hirschsprung",
    "injury",
    "conversion",
    "drain",
    "contrast",
    "mild",
    "pain",
    "vomiting",
    "acute",
    "stricture",
    "polyp",
    "tumour",
    "tumor",
    "lesion",
    "cyst",
    "calculus",
    "stone",
    "collection",
    "seen",
    "admitted",
    "operated",
    "referred",
    "scheduled",
    "advised",
    "discharged",
    "planned",
    "brought",
    "came",
    "dyspepsia",
    "heartburn",
    "haematemesis",
    "hematemesis",
    "malena",
    "pruritus",
    "reflux",
    "regurgitation",
    "obstipation",
    "hernia",
    "haemorrhoids",
    "piles",
    "severe",
    "moderate",
    "sudden",
    "intermittent",
    "inguinal",
    "altered",
    "painless",
    "fullness",
    "passing",
    "robotic",
    "laparoscopic",
    "open",
    "elective",
    "emergency",
];

/// CLINICAL lexicon (ruling R-2d): anatomical adjectives, diagnoses, findings and
/// history words common in Indian surgical notes and imaging impressions. Never names.
const CLINICAL_WORDS: &[&str] = &[
    // anatomy / adjectives
    "intestinal",
    "duodenal",
    "rectal",
    "umbilical",
    "paraumbilical",
    "periumbilical",
    "perianal",
    "perineal",
    "peritoneal",
    "retroperitoneal",
    "hepatic",
    "pancreatic",
    "peripancreatic",
    "gastric",
    "appendicular",
    "appendiceal",
    "diverticular",
    "mesenteric",
    "omental",
    "anastomotic",
    "peptic",
    "pyloric",
    "anorectal",
    "colorectal",
    "splenic",
    "sigmoid",
    "rectosigmoid",
    "cardia",
    "inguinal",
    "femoral",
    "incisional",
    "ventral",
    "abdominal",
    "pelvic",
    "renal",
    "biliary",
    "cystic",
    "colonic",
    "caecal",
    "cecal",
    "ileal",
    "jejunal",
    "ileocaecal",
    "ileocecal",
    "oesophageal",
    "esophageal",
    "hiatal",
    "hiatus",
    "pilonidal",
    "scrotal",
    "groin",
    "flank",
    "loin",
    "iliac",
    "fossa",
    "quadrant",
    "region",
    "diaphragm",
    "subphrenic",
    "subhepatic",
    "liver",
    "colon",
    "caecum",
    "cecum",
    "appendix",
    "gallbladder",
    "bile",
    "duct",
    "outlet",
    "wall",
    "bowel",
    "stump",
    "port",
    "site",
    "wound",
    // findings / diagnoses
    "ileus",
    "gallstones",
    "gallstone",
    "stones",
    "calculi",
    "sludge",
    "pneumoperitoneum",
    "fatty",
    "hydatid",
    "amoebic",
    "pyogenic",
    "malignant",
    "benign",
    "metastatic",
    "neoplastic",
    "cancer",
    "carcinoma",
    "carcinomatosis",
    "free",
    "fluid",
    "gas",
    "air",
    "pus",
    "subacute",
    "koch",
    "kochs",
    "calculous",
    "acalculous",
    "achalasia",
    "intussusception",
    "ulcer",
    "ulcers",
    "perforation",
    "perforated",
    "growth",
    "hernia",
    "fistula",
    "fissure",
    "sinus",
    "prolapse",
    "obstruction",
    "obstructed",
    "strangulated",
    "incarcerated",
    "gangrenous",
    "volvulus",
    "abscess",
    "ascites",
    "mucocele",
    "pseudocyst",
    "necrosis",
    "necrotising",
    "necrotizing",
    "collection",
    "effusion",
    "leak",
    "bleed",
    "thickening",
    "thickened",
    "dilated",
    "distended",
    "enlarged",
    "impacted",
    "inflamed",
    "inflammatory",
    "infected",
    "tubercular",
    "tuberculous",
    "adhesions",
    "adhesive",
    "stricture",
    "narrowing",
    "nodule",
    "nodules",
    "varices",
    "injury",
    "trauma",
    "blunt",
    "infection",
    "sepsis",
    "septic",
    "shock",
    "dehiscence",
    "lump",
    "mass",
    "drain",
    "contrast",
    "conversion",
    "complications",
    "complication",
    // history / complaints
    "previous",
    "prior",
    "similar",
    "complaints",
    "complaint",
    "heaviness",
    "swelling",
    "pain",
    "sided",
    "side",
    "left",
    "right",
    "bilateral",
    "unilateral",
    "upper",
    "lower",
    "lateral",
    "medial",
    "anterior",
    "posterior",
    "proximal",
    "distal",
    "patient",
    "patients",
    "surgery",
    "surgeries",
    "operation",
    "procedure",
    "smoking",
    "smoker",
    "alcohol",
    "alcoholic",
    "tobacco",
    "chewing",
    "intake",
    "asthma",
    "copd",
    "thyroid",
    "stroke",
    "over",
    "under",
    "below",
    "above",
    "around",
    "within",
    "near",
    "acute",
    "chronic",
    "mild",
    "moderate",
    "severe",
    "sudden",
    "gradual",
    "intermittent",
    "continuous",
    "colicky",
    "dull",
    "recurrent",
    "grade",
    "stage",
    "type",
    "small",
    "large",
    "giant",
    "huge",
    "minimal",
    "gross",
    "multiple",
    "single",
    "early",
    "delayed",
    "partial",
    "complete",
    "focal",
    "diffuse",
    "residual",
    "healed",
    "healing",
    "surgical",
    "operative",
    "postoperative",
    "preoperative",
    "findings",
    "impression",
    "annular",
    "recurrence",
    "backache",
    "signet",
    "ring",
    // organs and further adjectives
    "stomach",
    "oesophagus",
    "esophagus",
    "duodenum",
    "jejunum",
    "ileum",
    "anus",
    "omentum",
    "mesentery",
    "peritoneum",
    "spleen",
    "kidney",
    "ureter",
    "bladder",
    "uterus",
    "ovary",
    "ovarian",
    "breast",
    "lymph",
    "node",
    "nodes",
    "vein",
    "artery",
    "arterial",
    "venous",
    "portal",
    "hollow",
    "viscus",
    "gastrointestinal",
    "stromal",
    "hepatocellular",
    "periampullary",
    "ampullary",
    "choledochal",
    "subdiaphragmatic",
    "ureteric",
    "pulmonary",
    "ischemic",
    "ischaemic",
    "heart",
    "calcific",
    "ulcerative",
    "sliding",
    "simple",
    "complex",
    "solid",
    "mucinous",
    "serous",
    "twisted",
    "walled",
    "retained",
    "space",
    "occupying",
    "typhoid",
    "fibroid",
    "neoplasm",
    "metastases",
    "metastasis",
    "torsion",
    "gangrene",
    "infarct",
    "infarction",
    "disease",
    "section",
    "caesarean",
    "cesarean",
    "allergy",
    "drug",
    "urine",
    "stools",
    "stool",
    "dark",
    "clay",
    "coloured",
    "colored",
    "episodes",
    "episode",
    "transfusion",
    "accident",
    "traffic",
    "road",
    "lap",
    "chole",
    // abbreviations written in capitals
    "cbd",
    "gerd",
    "hcc",
    "gist",
    "sol",
    "ibd",
    "ibs",
    "sbo",
    "ercp",
    "mrcp",
    "usg",
    "cect",
    "hpe",
    "fnac",
    "rta",
    "ihd",
    "cad",
    "htn",
    "ckd",
    "cva",
    "ptb",
    "uti",
    "dvt",
    "pud",
    "nafld",
    "nash",
    "lscs",
    "tah",
    "bso",
    "rif",
    "ruq",
    "luq",
];

/// Clinical head nouns: when one closes the capitalised run after `s/o`
/// ("Hollow Viscus Perforation", "Ovarian Cyst"), the run is a diagnosis, not a name.
const HEAD_NOUNS: &[&str] = &[
    "perforation",
    "growth",
    "cyst",
    "stone",
    "stones",
    "calculus",
    "lump",
    "mass",
    "tumour",
    "tumor",
    "carcinoma",
    "cancer",
    "lesion",
    "stricture",
    "obstruction",
    "abscess",
    "collection",
    "leak",
    "hernia",
    "fistula",
    "ulcer",
    "nodule",
    "nodules",
    "polyp",
    "neoplasm",
    "syndrome",
    "disease",
    "diverticulum",
    "infarct",
    "injury",
    "fluid",
    "effusion",
    "volvulus",
    "ileus",
    "ischemia",
    "ischaemia",
    "liver",
    "abdomen",
    "uterus",
];

/// Case-sheet labels that end in clinical-looking suffixes but head a field, not a
/// diagnosis ("DIAGNOSIS:", "Occupation", "Admission").
const LABEL_STOP: &[&str] = &[
    "diagnosis",
    "prognosis",
    "occupation",
    "admission",
    "religion",
    "residence",
    "address",
];

/// A word that can close a clinical phrase ("Cyst", "Malformation", "Appendicitis").
/// "-ation" words (Station, Occupation) and field labels never do.
fn exemption_head(k: &str) -> bool {
    HEAD_NOUNS.contains(&k)
        || k == "malformation"
        || (strong_medical(k) && !k.ends_with("ation") && !LABEL_STOP.contains(&k))
}

/// Words that end the `s/o` walk as NOT exempt, colon or not: case-sheet field labels
/// ("S/O MUNIYAPPA DIAGNOSIS HERNIA", "IMPRESSION ...") and connectives that come before
/// any head noun ("S/O MUNIYAPPA WITH INGUINAL HERNIA").
fn walk_stop(k: &str) -> bool {
    LABEL_STOP.contains(&k)
        || matches!(
            k,
            "impression"
                | "findings"
                | "provisional"
                | "final"
                | "diagnosed"
                | "with"
                | "without"
                | "and"
                | "for"
        )
}

/// True when the run of capitalised words after the first relation word is a clinical
/// phrase: clinical words ending in a head noun, with no label colon and no label or
/// connective word before the head noun. Walked in order.
fn diagnosis_phrase(run: &[(&str, bool)]) -> bool {
    for &(w, colon) in run {
        if colon {
            return false;
        }
        let k = word_key(w);
        if walk_stop(&k) {
            return false;
        }
        if exemption_head(&k) {
            return true;
        }
        if !clinical_word(&k) {
            return false;
        }
    }
    false
}

static COMMA_AGE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\s*,\s*([0-9]{2,3})\s*(?i:y|yrs?|years?)\b\.?").expect("comma age regex")
});

/// "c/o Puttaswamy, 56 yrs": a comma then an age, unless a duration word follows
/// ("c/o Swelling, 10 years duration", "15 years back"). "56 years of age" is an age;
/// "12 years of history" is not. "56 years, history of ..." (comma before "history")
/// is an age: the caller has already required the first word to be a name candidate.
fn comma_age_follows(rest: &str) -> bool {
    let Some(m) = COMMA_AGE.find(rest) else {
        return false;
    };
    let after = &rest[m.end()..];
    let tail = after.trim_start_matches(|c: char| !c.is_alphanumeric());
    let word = |t: &str| -> String {
        t.chars()
            .take_while(|c| c.is_alphabetic())
            .collect::<String>()
            .to_lowercase()
    };
    let next = word(tail);
    match next.as_str() {
        "of" => word(tail[2..].trim_start()) == "age",
        "history" => after[..after.len() - tail.len()].contains(','),
        "ago" | "back" | "before" | "since" | "duration" => false,
        _ => true,
    }
}

fn not_name(k: &str) -> bool {
    NOT_NAMES.contains(&k) || NOT_NAMES_2.contains(&k) || CLINICAL_WORDS.contains(&k)
}

/// A capitalised word that may be a person's name: an allowlisted given name (always),
/// or a word of 3+ letters that is not clinical. Weak suffixes alone never decide.
fn name_candidate(k: &str) -> bool {
    GIVEN_NAMES.contains(&k) || (k.len() >= 3 && !clinical_word(k))
}

/// Imaging / report-heading words that follow "MR", "CT" etc.
const IMAGING_WORDS: &[&str] = &[
    "abdomen", "pelvis", "liver", "brain", "spine", "chest", "knee", "neck", "rectum", "pancreas",
    "contrast", "imaging", "scan", "study", "score", "protocol", "with", "without", "and", "for",
    "the", "grade",
];

fn strong_medical(k: &str) -> bool {
    STRONG_SUFFIXES.iter().any(|s| k.ends_with(s))
}

fn imaging_word(k: &str) -> bool {
    IMAGING_WORDS.contains(&k) || k.starts_with("angio") || k.ends_with("gram")
}

/// A word that is clearly clinical (diagnosis suffix, finding, imaging term, eponym).
fn clinical_word(k: &str) -> bool {
    strong_medical(k) || EPONYMS.contains(&k) || not_name(k) || imaging_word(k)
}

fn name_check_caps_title(_: &str, c: &Captures, _: &[Finding]) -> bool {
    let w = word_key(c.name("name").map_or("", |m| m.as_str()));
    !TITLE_COMMON.contains(&w.as_str()) && name_candidate(&w)
}

/// GIVEN-NAME allowlist (ruling R-2d): common Indian given names, including ones that
/// look like ordinary words or end in clinical-looking suffixes (Gopal, Kamal, Sonia).
/// An allowlisted word is always a name candidate.
const GIVEN_NAMES: &[&str] = &[
    "gopal",
    "vishal",
    "kunal",
    "sonal",
    "kamal",
    "komal",
    "nirmal",
    "vimal",
    "amal",
    "kushal",
    "ujjwal",
    "dhaval",
    "jamal",
    "bilal",
    "faisal",
    "iqbal",
    "sonia",
    "rajan",
    "kamala",
    "ramesh",
    "suresh",
    "mahesh",
    "rajesh",
    "ganesh",
    "lakshmi",
    "anjali",
    "pooja",
    "arjun",
    "prashant",
    "ramaiah",
    "nancy",
    "soma",
    "roma",
    // two-letter names (only reachable through this list)
    "om",
    // South Indian
    "manjunath",
    "nagaraj",
    "basavaraj",
    "venkatesh",
    "srinivas",
    "srinivasa",
    "murali",
    "mohan",
    "krishna",
    "shankar",
    "shiva",
    "siva",
    "subramanya",
    "raghavendra",
    "narayana",
    "narayan",
    "gowda",
    "reddy",
    "naidu",
    "nair",
    "menon",
    "pillai",
    "iyer",
    "rao",
    "ravi",
    "anil",
    "sunil",
    "kiran",
    "naveen",
    "praveen",
    "prakash",
    "anand",
    "arun",
    "vijay",
    "ajay",
    "kumar",
    "raju",
    "ramu",
    "babu",
    "anitha",
    "kavitha",
    "kavita",
    "sunitha",
    "geetha",
    "shanthi",
    "saroja",
    "sarojamma",
    "padma",
    "nirmala",
    "vimala",
    "shyamala",
    "mangala",
    "sushila",
    "parvathi",
    "bhavani",
    "yashoda",
    "savitri",
    "radha",
    "rekha",
    "usha",
    "asha",
    "uma",
    "devi",
    "meena",
    "priya",
    "divya",
    "kavya",
    // North Indian
    "rahul",
    "rohit",
    "amit",
    "sanjay",
    "ashok",
    "manoj",
    "deepak",
    "raj",
    "ram",
    "sunita",
    "neha",
    "sharma",
    "verma",
    "gupta",
    "singh",
    "patel",
    "shah",
    "gurpreet",
    "harpreet",
    // other
    "mohammed",
    "mohammad",
    "abdul",
    "ahmed",
    "imran",
    "salim",
    "ali",
    "fatima",
    "ayesha",
    "joseph",
    "thomas",
    "mary",
];

static AGE_SEX_AFTER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"^[,\s]*(?:aged\s+[0-9]+|(?P<n1>[0-9]{1,3})\s*[MFmf]\b|(?P<n2>[0-9]{1,3})\s*(?:yo\b|y/o\b|(?:y|yrs?|years?)\s*(?:old\b|/\s*[MFmf]\b|[MFmf]\b)))",
    )
    .expect("age regex")
});

/// An age/sex marker right after the name ("Kunal, 23M", "Kamala, aged 45",
/// "Ravi 45 yrs/M"). A bare duration ("Smoking 20 years") is not an age, and small
/// numbers before M/F are months ("Pain 3m"), not ages.
fn age_follows(rest: &str) -> bool {
    let hi = ceil_boundary(rest, 28);
    let Some(m) = AGE_SEX_AFTER.captures(&rest[..hi]) else {
        return false;
    };
    let n: u32 = m
        .name("n1")
        .or(m.name("n2"))
        .and_then(|x| x.as_str().parse().ok())
        .unwrap_or(99);
    n >= 10
}

/// The next word if it is a capitalised word of 3+ letters on the same line.
fn next_cap_word(rest: &str) -> Option<&str> {
    let t = rest.trim_start_matches([' ', '\t']);
    if t.len() == rest.len() {
        return None;
    }
    let w: &str = &t[..t
        .find(|c: char| !c.is_ascii_alphabetic())
        .unwrap_or(t.len())];
    (w.len() >= 3 && w.chars().next().is_some_and(|c| c.is_ascii_uppercase())).then_some(w)
}

/// s/o, d/o, h/o, w/o, c/o (and "son of" ...) + name.
///
/// The first word must be a name candidate. `s/o`, `d/o` and the long forms then block
/// unless a clinical phrase follows (clinical words ending in a head noun, no label colon). `c/o` ("complains of"), `w/o` ("without")
/// and `h/o` ("history of") are mostly clinical, so a bare word passes unless it is an
/// allowlisted given name; they also block with a title, an age/sex after the name, or a
/// second name-candidate word. "Late" counts as a title only before a name candidate.
fn relation_check(text: &str, c: &Captures, _: &[Finding]) -> bool {
    let rel = c
        .name("rel")
        .map_or_else(|| "s".to_string(), |m| m.as_str().to_lowercase());
    let strict = rel == "s" || rel == "d";
    let k1 = word_key(c.name("w1").map_or("", |m| m.as_str()));
    // Every path needs the first word to be a NAME candidate ("Intestinal", "Lt",
    // "Patient" never are).
    if !name_candidate(&k1) {
        return false;
    }
    let (_, e) = span(c);
    // Titles are evidence; "late" ("s/o late Ramaiah") only counts because the word
    // after it has already been checked as a name candidate above.
    let titled = c
        .name("title")
        .is_some_and(|m| !m.as_str().trim().is_empty());
    let given = GIVEN_NAMES.contains(&k1.as_str());
    // The capitalised words that follow on the same line ("Viscus Perforation").
    let run = cap_run(&text[e..], 3);
    if titled || given {
        return true;
    }
    if strict {
        // "s/o Sebaceous Cyst" is a clinical phrase; "S/O MUNIYAPPA OCCUPATION:" is not.
        return !diagnosis_phrase(&run);
    }
    // c/o, w/o, h/o: need more evidence than a bare word: an age/sex after it, a comma
    // age for c/o and w/o ("c/o Puttaswamy, 56 yrs"), or a second name word
    // ("c/o Vishal Kumar") that does not start a clinical phrase.
    age_follows(&text[e..])
        || ((rel == "c" || rel == "w") && comma_age_follows(&text[e..]))
        || (run
            .first()
            .is_some_and(|(w2, _)| name_candidate(&word_key(w2)))
            && !diagnosis_phrase(&run))
}

/// Up to `n` capitalised words directly following, separated only by spaces/tabs, each
/// with whether a `:` follows it (a case-sheet label such as "DIAGNOSIS:").
fn cap_run(rest: &str, n: usize) -> Vec<(&str, bool)> {
    let mut out = Vec::new();
    let mut pos = 0;
    while out.len() < n {
        let Some(w) = next_cap_word(&rest[pos..]) else {
            break;
        };
        let skipped = rest[pos..].len() - rest[pos..].trim_start_matches([' ', '\t']).len();
        pos += skipped + w.len();
        let colon = rest[pos..].trim_start_matches([' ', '\t']).starts_with(':');
        out.push((w, colon));
    }
    out
}

/// Words that start a different identifier; a name never extends over them.
const EXT_STOP: &[&str] = &[
    "uhid",
    "mrn",
    "ipd",
    "ip",
    "op",
    "reg",
    "hosp",
    "hospital",
    "patient",
    "pt",
    "age",
    "aged",
    "dob",
    "phone",
    "ph",
    "mob",
    "mobile",
    "pan",
    "aadhaar",
    "address",
    "contact",
    "tel",
    "call",
    "email",
    "date",
    "name",
    "door",
    "flat",
    "house",
    "plot",
    "sector",
    "january",
    "february",
    "march",
    "april",
    "may",
    "june",
    "july",
    "august",
    "september",
    "october",
    "november",
    "december",
    "jan",
    "feb",
    "mar",
    "apr",
    "jun",
    "jul",
    "aug",
    "sep",
    "sept",
    "oct",
    "nov",
    "dec",
];

/// Extend a name over up to `extra` following capitalised words that are not clinical
/// words or the start of another identifier ("Kunal" -> "Kunal Shah").
fn extend_name(text: &str, mut end: usize, extra: usize) -> usize {
    for _ in 0..extra {
        let Some(w) = next_cap_word(&text[end..]) else {
            break;
        };
        let k = word_key(w);
        if clinical_word(&k) || EXT_STOP.contains(&k.as_str()) {
            break;
        }
        let skipped = text[end..].len() - text[end..].trim_start_matches([' ', '\t']).len();
        end += skipped + w.len();
    }
    end
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
        rule(
            Pan,
            r"\b[A-Z]{5}\d{4}[A-Z]\b|(?i:\bPAN\b)(?i:\s*(?:no|number)\b)?\.?[\s:#\-]*[A-Za-z]{5}\d{4}[A-Za-z]\b",
            always,
        ),
        // Indian mobile: optional +91 / 91 / 0, 10 digits starting 6-9.
        rule(
            Phone,
            r"(?:\+91$H?|\b91$H|\b0)?(?:[6-9]\d{4}$H?\d{5}|[6-9]\d{2}$H\d{3}$H\d{4}|[6-9]\d{3}$H\d{3}$H\d{3}|[6-9]\d$H\d{3}$H\d{5})\b",
            phone_check,
        ),
        // Any other grouping of a 10-digit mobile, only after a phone label.
        rule(
            Phone,
            r"(?:\+91$H?|\b0$H?|\b)[6-9](?:$H?\d){9}\b",
            labelled_phone,
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
        rule(HospitalId, r"\bIP\s+\d+(?:/\d+)?\b", no_unit_after),
        // Names: title + capitalised word (eponyms only exempt in procedure context).
        rule(
            PersonName,
            r"\b(?:Mr|Mrs|Ms|Miss|Shri|Smt|Kumari|Master|Baby\s+of)\b\.?\s*(?P<name>[A-Z][A-Za-z]+)",
            name_check_titled,
        )
        .names(2),
        // All-caps titles need an all-caps name ("MR" alone is MR imaging).
        rule(
            PersonName,
            r"\b(?:MR|MRS|SHRI|SMT|KUMARI)\b\.?\s*(?P<name>[A-Z]{3,})\b",
            name_check_caps_title,
        )
        .names(2),
        // Names: patient-context word + capitalised word.
        rule(
            PersonName,
            r"\b(?:[Pp]atient|[Pp]t|[Nn]ame|named|called)\b\.?(?:\s*[:,]\s*|\s+[\-\x{2013}\x{2014}]\s*|\s*[\x{2013}\x{2014}]\s+|\s+)(?:(?:is|was)\s+)?(?P<name>[A-Z][a-z]+)\b",
            name_check_context,
        )
        .names(2),
        // Names: case-insensitive name label ("NAME: RAMAKKA", "Pt. Name - Ramakka").
        rule(
            PersonName,
            r"(?i:\b(?:(?:patient|pt)\.?[ \t]*)?(?P<label>name))[ \t]*[:\-\x{2013}\x{2014}][ \t]*(?P<name>[A-Z][a-z]+|[A-Z]{2,})\b",
            name_check_label,
        )
        .names(2),
        // Relations: s/o, w/o, d/o, c/o + name.
        rule(
            PersonName,
            r"(?i:\b(?:(?P<rel>[sdwch])/o\b|(?:son|daughter|wife|husband)[ \t]+of\b))[ \t]*[:.\-\x{2013}\x{2014}]?[ \t]*(?P<title>(?:(?i:dr|late|sri|shri|smt|mr|mrs|ms)\b[.:]?[ \t]*)*)(?P<w1>[A-Z][A-Za-z]+)\b",
            relation_check,
        )
        .names(2),
        // Addresses.
        rule(
            Address,
            r"(?i)\b(?:door|flat|house|plot)\s*(?:no\.?|number|#)\s*[A-Za-z]?\d[A-Za-z0-9/\-]*",
            no_unit_after,
        ),
        rule(
            Address,
            r"\b(?:Door|Flat|House|Plot)\s+\d[A-Za-z0-9/\-]*",
            no_unit_after,
        ),
        rule(
            Address,
            r"\bNo\.\s*\d+[A-Za-z]?,\s*[A-Z]",
            numbered_label_ok,
        ),
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
            let end = if r.extra_words > 0 {
                extend_name(text, end, r.extra_words)
            } else {
                end
            };
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

// ---------- hidden and look-alike characters ----------

/// Unicode general category Cf (format), from the regex crate's Unicode tables.
static FORMAT_CHAR: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\p{Cf}$").expect("Cf pattern compiles"));

fn is_format(c: char) -> bool {
    let mut buf = [0u8; 4];
    FORMAT_CHAR.is_match(c.encode_utf8(&mut buf))
}

/// Look-alike slashes, colons and dots: flagged wherever they appear.
fn lookalike_separator(c: char) -> bool {
    matches!(
        c,
        '\u{2215}' | '\u{2044}' | '\u{29F8}' | '\u{2236}' | '\u{FE55}' | '\u{FE13}' | '\u{2027}'
    )
}

/// Look-alike dashes, flagged only between two digits. En and em dashes
/// (U+2013, U+2014) are left out: the phone and date rules already accept them
/// as separators, and "65000\u{2013}80000" style ranges must pass.
fn lookalike_dash(c: char) -> bool {
    matches!(
        c,
        '\u{2010}' | '\u{2011}' | '\u{2012}' | '\u{2015}' | '\u{2212}'
    )
}

/// Superscript and subscript digits (as in "kg/m\u{B2}", "\u{D7}10\u{2079}/L", "SpO\u{2082}").
fn script_digit(c: char) -> bool {
    matches!(c, '\u{B2}' | '\u{B3}' | '\u{B9}' | '\u{2070}'..='\u{2079}' | '\u{2080}'..='\u{2089}')
}

/// Vulgar fractions ("1\u{BD} hours").
fn vulgar_fraction(c: char) -> bool {
    matches!(c, '\u{BC}'..='\u{BE}' | '\u{2150}'..='\u{215F}' | '\u{2189}')
}

fn odd_numeral(c: char) -> bool {
    c.is_numeric() && !c.is_ascii_digit()
}

/// A short run of non-ASCII numerals that reads as a unit or an exponent, not
/// as digits of a number: one or two superscript/subscript digits after a letter,
/// a closing bracket, a superscript sign or an exact "10" ("m\u{B2}",
/// "\u{D7}10\u{B9}\u{B2}/L", "SpO\u{2082}"), or one vulgar fraction. Never when
/// another digit follows, so "98450\u{2079}2345" is still flagged.
fn unit_mark(text: &str, start: usize, end: usize) -> bool {
    let run: Vec<char> = text[start..end].chars().collect();
    if run.len() > 2 || text[end..].chars().next().is_some_and(|c| c.is_numeric()) {
        return false;
    }
    let before = &text[..start];
    if run.iter().all(|&c| script_digit(c)) {
        let prev = before.chars().next_back();
        let exponent = before
            .strip_suffix("10")
            .is_some_and(|b| !b.chars().next_back().is_some_and(|c| c.is_numeric()));
        return exponent
            || prev
                .is_some_and(|c| c.is_alphabetic() || matches!(c, ')' | '\u{207A}' | '\u{207B}'));
    }
    run.len() == 1
        && vulgar_fraction(run[0])
        && !before.chars().next_back().is_some_and(odd_numeral)
}

/// Every maximal run of hidden or look-alike characters (ruling R-F1: such text
/// is rejected, never normalised, so nothing can slip past the rules above):
/// (a) Unicode format characters (Cf: zero-width, soft hyphen, word joiner, BOM,
/// bidi controls ...); (b) numerals other than ASCII 0-9 (full-width, Devanagari,
/// Kannada ...), except a short unit mark (see [`unit_mark`]); (c) the
/// Halfwidth and Fullwidth Forms block; (d) look-alike separators, and look-alike
/// dashes between digits.
fn unusual_characters(text: &str) -> Vec<Finding> {
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let flagged = |i: usize| -> bool {
        let c = chars[i].1;
        if is_format(c) || lookalike_separator(c) || ('\u{FF00}'..='\u{FFEF}').contains(&c) {
            return true;
        }
        if lookalike_dash(c) {
            let digit = |j: Option<usize>| {
                j.and_then(|j| chars.get(j))
                    .is_some_and(|(_, c)| c.is_numeric())
            };
            return digit(i.checked_sub(1)) && digit(Some(i + 1));
        }
        odd_numeral(c)
    };
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        if !flagged(i) {
            i += 1;
            continue;
        }
        let mut j = i;
        while j < chars.len() && flagged(j) {
            j += 1;
        }
        let start = chars[i].0;
        let end = chars.get(j).map_or(text.len(), |(b, _)| *b);
        let all_numerals = text[start..end].chars().all(odd_numeral);
        if !(all_numerals && unit_mark(text, start, end)) {
            out.push(Finding {
                kind: FindingKind::UnusualCharacters,
                start,
                end,
            });
        }
        i = j;
    }
    out
}

/// Scan `text` for patient identifiers. Findings are sorted by `start`;
/// overlapping findings are merged into one (kind of the earliest/longest).
pub fn scan(text: &str) -> Vec<Finding> {
    let mut found: Vec<Finding> = unusual_characters(text);
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
