//! Part A: the rules of the interactive answer page, with no Leptos in them
//! (so they are unit-tested here and shared by the server and the browser).
//!
//! Everything here is display only: it reads a stored [`AnswerV3`] and
//! decides what the page draws and how (which chart a group of numbers
//! gets, which arms are compared, which marks are drawn). The stored text
//! is never changed, and nothing here is used by the engine.
//!
//! The template rules follow the owner's mockup (31 Oct): the chart-kind
//! choice, grouping by label and study, the comparison normalisation, the
//! "Out of 100" selection, the source types, the p normalisation, the
//! `[<pmid> as <n>]` citations, the Patient-mode sentence and the plain
//! certainty lines.

use std::collections::HashSet;
use std::sync::LazyLock;

use regex::Regex;

use crate::agent::answer_v3::{
    AnswerV3, Certainty, CompletenessV3, Number, Span, WrittenAnswer, arm_lines, citation_ranges,
    marked_runs, text_fields,
};

// ---------- the page order (item 1) ----------

/// The page's blocks. Their order is [`PAGE_ORDER`], in one place.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Block {
    Header,
    Glance,
    BottomLine,
    Practice,
    OutOf100,
    Forest,
    Outcomes,
    Compare,
    Details,
    /// The earlier layout, collapsed, last (item 13). Removing it later is
    /// this entry and its arm in the page view.
    Original,
}

/// The fixed page order (owner decision 1: no Arrange mode).
pub const PAGE_ORDER: [Block; 10] = [
    Block::Header,
    Block::Glance,
    Block::BottomLine,
    Block::Practice,
    Block::OutOf100,
    Block::Forest,
    Block::Outcomes,
    Block::Compare,
    Block::Details,
    Block::Original,
];

/// The Details sections, in their order (item 10).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Section {
    Conflicts,
    Guidelines,
    Applicability,
    WouldChange,
    Skeptic,
    Trials,
    Notable,
    Completeness,
    Checks,
    References,
    SearchTrail,
    Cost,
}

pub const SECTION_ORDER: [Section; 12] = [
    Section::Conflicts,
    Section::Guidelines,
    Section::Applicability,
    Section::WouldChange,
    Section::Skeptic,
    Section::Trials,
    Section::Notable,
    Section::Completeness,
    Section::Checks,
    Section::References,
    Section::SearchTrail,
    Section::Cost,
];

// ---------- p values (item 12) ----------

/// `p` as written, without a leading `P`/`p` or `=` (the stored "P < .001"
/// would otherwise print as "p P < .001"). Display only.
pub fn normalise_p(p: &str) -> String {
    let t = p.trim_start();
    let t = t.strip_prefix(['P', 'p']).map_or(t, str::trim_start);
    let t = t.strip_prefix('=').map_or(t, str::trim_start);
    t.trim_end().to_string()
}

/// "p <normalised>", or `None` when nothing is left.
pub fn p_label(p: Option<&str>) -> Option<String> {
    let n = normalise_p(p?);
    (!n.is_empty()).then(|| format!("p {n}"))
}

// ---------- chart kinds (items 6 and 7) ----------

/// A ratio: unit RR, OR or HR.
pub fn is_ratio(n: &Number) -> bool {
    matches!(n.unit.as_deref().map(str::trim), Some("RR" | "OR" | "HR"))
}

static DIFF: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)difference|\bMD\b|minus").expect("valid"));

/// A difference: its label or arm reads as one (MD, "difference", "minus").
pub fn is_diff(n: &Number) -> bool {
    DIFF.is_match(&format!("{} {}", n.label, n.arm.as_deref().unwrap_or("")))
}

/// Both CI bounds stored (and finite).
pub fn has_ci(n: &Number) -> bool {
    matches!((n.ci_low, n.ci_high), (Some(l), Some(h)) if l.is_finite() && h.is_finite())
}

/// How a group of numbers (one label, one study) is drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChartKind {
    /// All ratios with a CI: small log forest rows.
    Ratio,
    /// One difference with a CI: a bar centred on 0.
    Diff,
    /// Two or more distinct arms, no ratio: paired bars.
    Pair,
    /// Anything else: one text line.
    Text,
}

/// The chart kind of a group, by the shape of its numbers.
pub fn chart_kind(nums: &[&Number]) -> ChartKind {
    if nums.is_empty() {
        return ChartKind::Text;
    }
    if nums.iter().all(|n| is_ratio(n) && has_ci(n)) {
        return ChartKind::Ratio;
    }
    if nums.len() == 1 && has_ci(nums[0]) && is_diff(nums[0]) {
        return ChartKind::Diff;
    }
    let arms: HashSet<Option<&str>> = nums
        .iter()
        .map(|n| n.arm.as_deref().map(str::trim))
        .collect();
    if nums.len() >= 2 && arms.len() == nums.len() && !nums.iter().any(|n| is_ratio(n)) {
        return ChartKind::Pair;
    }
    ChartKind::Text
}

/// The numbers of one outcome with the same label and study.
#[derive(Debug, Clone, PartialEq)]
pub struct NumGroup {
    /// Index of the writer's outcome.
    pub outcome: usize,
    pub label: String,
    pub pmid: String,
    /// Indexes into the outcome's `numbers`, the intervention arm first.
    pub numbers: Vec<usize>,
    pub kind: ChartKind,
}

impl NumGroup {
    /// The group's numbers.
    pub fn nums<'a>(&self, w: &'a WrittenAnswer) -> Vec<&'a Number> {
        let o = &w.outcomes[self.outcome];
        self.numbers.iter().map(|&j| &o.numbers[j]).collect()
    }

    /// The distinct p values, normalised, joined " · " (`None`: none).
    pub fn p_text(&self, w: &WrittenAnswer) -> Option<String> {
        let mut ps: Vec<String> = Vec::new();
        for n in self.nums(w) {
            if let Some(p) = p_label(n.p.as_deref())
                && !ps.contains(&p)
            {
                ps.push(p);
            }
        }
        (!ps.is_empty()).then(|| ps.join(" \u{b7} "))
    }

    /// Counts as reported (unit "patients"), not rates.
    pub fn has_counts(&self, w: &WrittenAnswer) -> bool {
        self.nums(w)
            .iter()
            .any(|n| n.unit.as_deref().map(str::trim) == Some("patients"))
    }
}

/// The study row of `pmid`, if the writer gave one.
pub fn study_of<'a>(
    w: &'a WrittenAnswer,
    pmid: &str,
) -> Option<&'a crate::agent::answer_v3::Study> {
    w.studies.iter().find(|s| s.pmid.trim() == pmid.trim())
}

/// Outcome `i`'s numbers grouped by label and study (the earlier page's
/// arm lines: first-appearance order, the intervention arm first), each
/// with its chart kind.
pub fn outcome_groups(w: &WrittenAnswer, i: usize) -> Vec<NumGroup> {
    let o = &w.outcomes[i];
    let comparison = |p: &str| study_of(w, p).map(|s| s.comparison.as_str());
    arm_lines(o, comparison)
        .into_iter()
        .map(|l| {
            let nums: Vec<&Number> = l.numbers.iter().map(|&j| &o.numbers[j]).collect();
            NumGroup {
                outcome: i,
                kind: chart_kind(&nums),
                label: l.label,
                pmid: l.pmid,
                numbers: l.numbers,
            }
        })
        .collect()
}

/// Every outcome's groups, in page order.
pub fn all_groups(w: &WrittenAnswer) -> Vec<NumGroup> {
    (0..w.outcomes.len())
        .flat_map(|i| outcome_groups(w, i))
        .collect()
}

// ---------- compare the arms (item 8) ----------

static PARENS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\(.*?\)").expect("valid"));
static SPACES: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s+").expect("valid"));

/// A comparison as a grouping key: lower case, parentheticals removed,
/// whitespace squashed.
pub fn normalise_comparison(c: &str) -> String {
    let low = c.to_lowercase();
    let no_parens = PARENS.replace_all(&low, "");
    SPACES.replace_all(&no_parens, " ").trim().to_string()
}

/// Paired groups that share a (normalised) comparison.
#[derive(Debug, Clone, PartialEq)]
pub struct ComparisonGroup {
    /// The first study's comparison as written ("Other" without a study).
    pub label: String,
    pub key: String,
    /// Indexes into the `pairs` given.
    pub items: Vec<usize>,
}

/// The paired groups by their source study's comparison, in first-appearance
/// order.
pub fn comparison_groups(w: &WrittenAnswer, pairs: &[NumGroup]) -> Vec<ComparisonGroup> {
    let mut out: Vec<ComparisonGroup> = Vec::new();
    for (k, g) in pairs.iter().enumerate() {
        let label = study_of(w, &g.pmid)
            .map(|s| s.comparison.trim())
            .filter(|c| !c.is_empty())
            .unwrap_or("Other")
            .to_string();
        let key = normalise_comparison(&label);
        match out.iter_mut().find(|c| c.key == key) {
            Some(c) => c.items.push(k),
            None => out.push(ComparisonGroup {
                label,
                key,
                items: vec![k],
            }),
        }
    }
    out
}

// ---------- out of 100 patients (item 5) ----------

/// Of `groups`, those the "Out of 100" block shows: paired (2–3 distinct
/// arms, no ratio) with every unit `%`.
pub fn out_of_100(w: &WrittenAnswer, groups: &[NumGroup]) -> Vec<usize> {
    groups
        .iter()
        .enumerate()
        .filter(|(_, g)| {
            g.kind == ChartKind::Pair
                && g.numbers.len() <= 3
                && g.nums(w)
                    .iter()
                    .all(|n| n.unit.as_deref().map(str::trim) == Some("%"))
        })
        .map(|(k, _)| k)
        .collect()
}

/// The filled cells of a 10×10 grid for a percentage.
pub fn out_of_100_count(v: f64) -> u8 {
    if !v.is_finite() {
        return 0;
    }
    v.round().clamp(0.0, 100.0) as u8
}

/// The measure as the sentence names it ("Dysphagia relief" → "dysphagia
/// relief").
pub fn measure(label: &str) -> String {
    let mut c = label.trim().chars();
    match c.next() {
        Some(f) => f.to_lowercase().chain(c).collect(),
        None => String::new(),
    }
}

/// Patient mode (owner decision 2): "Out of 100 people like those in the
/// study, about A after X, compared with about B after Y, had this:
/// <label>."
pub fn patient_sentence(label: &str, arms: &[(u8, String)]) -> String {
    let parts: Vec<String> = arms
        .iter()
        .map(|(n, arm)| format!("about {n} after {arm}"))
        .collect();
    format!(
        "Out of 100 people like those in the study, {}, had this: {}.",
        parts.join(", compared with "),
        measure(label)
    )
}

/// Patient mode's plain-words certainty line.
pub fn plain_certainty(c: Certainty) -> &'static str {
    match c {
        Certainty::High => "We are quite sure about this.",
        Certainty::Moderate => "We are fairly sure; new studies could change the numbers a little.",
        Certainty::Low => "We are not very sure; new studies could change this.",
        Certainty::VeryLow => "We are very unsure; treat these numbers as a rough guide.",
    }
}

// ---------- source types (item 6) ----------

/// The forest filter's source type of a study, from its design.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SrcType {
    Trials,
    MetaAnalyses,
    Observational,
    Models,
}

impl SrcType {
    pub const ALL: [SrcType; 4] = [
        SrcType::Trials,
        SrcType::MetaAnalyses,
        SrcType::Observational,
        SrcType::Models,
    ];

    pub fn label(self) -> &'static str {
        match self {
            SrcType::Trials => "Trials",
            SrcType::MetaAnalyses => "Meta-analyses",
            SrcType::Observational => "Observational",
            SrcType::Models => "Models",
        }
    }

    /// The CSS class of its colour (forest marks).
    pub fn class(self) -> &'static str {
        match self {
            SrcType::Trials => "src-trials",
            SrcType::MetaAnalyses => "src-ma",
            SrcType::Observational => "src-obs",
            SrcType::Models => "src-models",
        }
    }
}

static OBSERVATIONAL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)observational|cohort|registr").expect("valid"));
static MODEL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)model").expect("valid"));
static META: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)meta-analysis|systematic review").expect("valid"));

/// The mockup's `srcType`: observational, then models, then meta-analyses,
/// otherwise trials.
pub fn src_type(design: &str) -> SrcType {
    if OBSERVATIONAL.is_match(design) {
        SrcType::Observational
    } else if MODEL.is_match(design) {
        SrcType::Models
    } else if META.is_match(design) {
        SrcType::MetaAnalyses
    } else {
        SrcType::Trials
    }
}

/// The source type of the study `pmid` (its row's design; no row: trials).
pub fn src_type_of(w: &WrittenAnswer, pmid: &str) -> SrcType {
    src_type(study_of(w, pmid).map_or("", |s| s.design.as_str()))
}

// ---------- numbers as text and on axes ----------

/// A value as the charts print it: as stored (no rounding), a true minus,
/// thousands separated for whole numbers of 1,000 or more.
pub fn fmt_value(v: f64) -> String {
    if !v.is_finite() {
        return v.to_string();
    }
    let body = if v.fract() == 0.0 && v.abs() < 1e15 {
        let digits = format!("{:.0}", v.abs());
        if v.abs() >= 1000.0 {
            let mut out = String::new();
            for (i, c) in digits.chars().enumerate() {
                if i > 0 && (digits.len() - i) % 3 == 0 {
                    out.push(',');
                }
                out.push(c);
            }
            out
        } else {
            digits
        }
    } else {
        format!("{}", v.abs())
    };
    if v < 0.0 {
        format!("\u{2212}{body}")
    } else {
        body
    }
}

/// A value with its unit: "44%", "RR 0.4", "80.3 min".
pub fn with_unit(v: f64, unit: Option<&str>) -> String {
    match unit.map(str::trim).filter(|u| !u.is_empty()) {
        None => fmt_value(v),
        Some("%") => format!("{}%", fmt_value(v)),
        Some(u @ ("RR" | "OR" | "HR")) => format!("{u} {}", fmt_value(v)),
        Some(u) => format!("{} {u}", fmt_value(v)),
    }
}

/// One number as a text line: value and unit, arm, CI and p.
pub fn number_line(n: &Number) -> String {
    let mut t = with_unit(n.value, n.unit.as_deref());
    if let Some(a) = n.arm.as_deref().map(str::trim).filter(|a| !a.is_empty()) {
        t.push_str(&format!(" ({a})"));
    }
    if let (true, Some(l), Some(h)) = (has_ci(n), n.ci_low, n.ci_high) {
        t.push_str(&format!(", 95% CI {} to {}", fmt_value(l), fmt_value(h)));
    }
    if let Some(p) = p_label(n.p.as_deref()) {
        t.push_str(&format!(", {p}"));
    }
    t
}

/// A ratio with its CI: "RR 0.4 (0.08–2.03)".
pub fn ratio_estimate(n: &Number) -> String {
    format!(
        "{} ({}\u{2013}{})",
        with_unit(n.value, n.unit.as_deref()),
        fmt_value(n.ci_low.unwrap_or(f64::NAN)),
        fmt_value(n.ci_high.unwrap_or(f64::NAN))
    )
}

/// A difference with its CI: "0.33 days (−0.03 to 0.7)".
pub fn diff_estimate(n: &Number) -> String {
    let unit = n.unit.as_deref().map(str::trim).unwrap_or("");
    let v = if unit.is_empty() || unit == "%" {
        with_unit(n.value, n.unit.as_deref())
    } else {
        format!("{} {unit}", fmt_value(n.value))
    };
    format!(
        "{v} ({} to {})",
        fmt_value(n.ci_low.unwrap_or(f64::NAN)),
        fmt_value(n.ci_high.unwrap_or(f64::NAN))
    )
}

/// The log axis's domain (item 6).
pub const LOG_MIN: f64 = 0.05;
pub const LOG_MAX: f64 = 50.0;
/// Its ticks.
pub const LOG_TICKS: [f64; 5] = [0.1, 0.5, 1.0, 2.0, 10.0];

/// Where `x` sits on the log axis, in percent (clamped to 0–100).
pub fn log_pos(x: f64) -> f64 {
    let (lo, hi) = (LOG_MIN.log10(), LOG_MAX.log10());
    ((x.max(0.0001).log10() - lo) / (hi - lo) * 100.0).clamp(0.0, 100.0)
}

/// A difference's own half-scale: the largest of |value|, |lo| and |hi|,
/// plus 15% (1 when all are 0).
pub fn diff_scale(n: &Number) -> f64 {
    let m = [n.value, n.ci_low.unwrap_or(0.0), n.ci_high.unwrap_or(0.0)]
        .iter()
        .map(|v| v.abs())
        .fold(0.0, f64::max)
        * 1.15;
    if m > 0.0 && m.is_finite() { m } else { 1.0 }
}

/// Where `v` sits on an axis centred on 0 with half-scale `m`, in percent.
pub fn diff_pos(v: f64, m: f64) -> f64 {
    (50.0 + v / m * 50.0).clamp(0.0, 100.0)
}

/// The full scale of paired bars: 100 for percentages, otherwise the
/// largest value or upper bound plus 12%.
pub fn pair_max(nums: &[&Number]) -> f64 {
    if nums
        .first()
        .is_some_and(|n| n.unit.as_deref().map(str::trim) == Some("%"))
    {
        return 100.0;
    }
    let m = nums
        .iter()
        .map(|n| n.value.max(n.ci_high.unwrap_or(0.0)))
        .fold(0.0, f64::max)
        * 1.12;
    if m > 0.0 && m.is_finite() { m } else { 1.0 }
}

/// A percentage of a scale, for a bar (0–100).
pub fn pct(v: f64, max: f64) -> f64 {
    if !v.is_finite() || max <= 0.0 {
        return 0.0;
    }
    (v / max * 100.0).clamp(0.0, 100.0)
}

/// A study's short name ("ROGER (2025, Br J Surg)" → its part before the
/// first comma).
pub fn study_short(label: &str) -> String {
    label.split(',').next().unwrap_or("").trim().to_string()
}

// ---------- text: first sentences, citations, marks ----------

static BRACKETS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\[[^\]]*\]").expect("valid"));
static SPACE_PUNCT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s+([.,;])").expect("valid"));
static SENTENCE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^.*?[.!?](?:\s|$)").expect("valid"));

/// `text` without bracketed citations, spaces before punctuation removed.
pub fn strip_citations(text: &str) -> String {
    let t = BRACKETS.replace_all(text, "");
    let t = SPACE_PUNCT.replace_all(&t, "$1");
    SPACES.replace_all(&t, " ").trim().to_string()
}

/// The first sentence of `text`, citations stripped.
pub fn first_sentence(text: &str) -> String {
    let t = strip_citations(text);
    SENTENCE
        .find(&t)
        .map_or(t.as_str(), |m| m.as_str())
        .trim()
        .to_string()
}

/// `[<pmid> as <n>]`: writer text citing a PMID and a reference number
/// together (item 12), drawn as the citation of the PMID.
static AS_CITE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\[\s*(\d{5,9})\s+as\s+(\d{1,4})\s*\]").expect("valid"));
static IDS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\d{1,9}").expect("valid"));

/// The `[<pmid> as <n>]` citations of `text`: (start, end, pmid, n).
pub fn as_citations(text: &str) -> Vec<(usize, usize, String, String)> {
    AS_CITE
        .captures_iter(text)
        .filter_map(|c| {
            let m = c.get(0)?;
            Some((m.start(), m.end(), c[1].to_string(), c[2].to_string()))
        })
        .collect()
}

/// A trial or review registration ID, optionally closed by a bracket.
static REGISTRATION: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(?:NCT|NTR|ISRCTN|CRD)\d+\)?$").expect("valid"));

/// Whether the marked run `text[a..b]` is (the digits of) a registration
/// ID such as NTR3593 or CRD420261370145 (item 12: its "not traced" mark
/// is not drawn; the run is counted).
pub fn is_registration_run(text: &str, a: usize, b: usize) -> bool {
    if a > b || b > text.len() || !text.is_char_boundary(a) || !text.is_char_boundary(b) {
        return false;
    }
    let bytes = text.as_bytes();
    let mut start = a;
    while start > 0 && bytes[start - 1].is_ascii_uppercase() {
        start -= 1;
    }
    if start > 0 && bytes[start - 1].is_ascii_alphanumeric() {
        return false;
    }
    REGISTRATION.is_match(&text[start..b])
}

/// A piece of drawn text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Piece {
    Text(String),
    /// A citation of these PMIDs (drawn as reference numbers).
    Cite(Vec<String>),
    /// A mark after the run before it.
    Mark,
    /// A "not traced" mark on a registration ID, not drawn (counted).
    Suppressed,
}

/// `text` as drawn: plain runs, citations (`[PMID]` markers as the
/// earlier page decides them, and `[<pmid> as <n>]` when the PMID is
/// `known`) and a mark after each run `spans` marks on `field`. A mark
/// whose run ends inside a citation is drawn after the citation, never
/// dropped. With `suppress_registration`, a marked registration ID gets
/// [`Piece::Suppressed`] instead of a mark.
pub fn pieces(
    text: &str,
    field: &str,
    spans: &[Span],
    known: impl Fn(&str) -> bool,
    suppress_registration: bool,
) -> Vec<Piece> {
    // Citation ranges with their PMIDs.
    let mut cites: Vec<(usize, usize, Vec<String>)> = as_citations(text)
        .into_iter()
        .filter(|(_, _, p, _)| known(p))
        .map(|(s, e, p, _)| (s, e, vec![p]))
        .collect();
    for (s, e) in citation_ranges(text, &known) {
        if cites.iter().any(|(a, b, _)| s < *b && *a < e) {
            continue;
        }
        let ids = IDS
            .find_iter(&text[s..e])
            .map(|d| d.as_str().to_string())
            .collect();
        cites.push((s, e, ids));
    }
    cites.sort_by_key(|c| c.0);
    // Marks at the end of each marked run (after a citation it ends in).
    let mut marks: Vec<(usize, Piece)> = Vec::new();
    let mut at = 0;
    for (run, marked) in marked_runs(text, field, spans) {
        let (a, b) = (at, at + run.len());
        at = b;
        if !marked {
            continue;
        }
        let pos = cites
            .iter()
            .find(|(s, e, _)| *s < b && b < *e)
            .map_or(b, |(_, e, _)| *e);
        let piece = if suppress_registration && is_registration_run(text, a, b) {
            Piece::Suppressed
        } else {
            Piece::Mark
        };
        marks.push((pos, piece));
    }
    marks.sort_by_key(|m| m.0);
    let mut out: Vec<Piece> = Vec::new();
    let mut cursor = 0;
    let mut mi = 0;
    let push_text = |out: &mut Vec<Piece>, t: &str| {
        if t.is_empty() {
            return;
        }
        if let Some(Piece::Text(last)) = out.last_mut() {
            last.push_str(t);
        } else {
            out.push(Piece::Text(t.to_string()));
        }
    };
    let mut until = |to: usize, out: &mut Vec<Piece>, cursor: &mut usize| {
        while mi < marks.len() && marks[mi].0 <= to {
            let pos = marks[mi].0.max(*cursor);
            push_text(out, &text[*cursor..pos]);
            *cursor = pos;
            out.push(marks[mi].1.clone());
            mi += 1;
        }
        if to > *cursor {
            push_text(out, &text[*cursor..to]);
            *cursor = to;
        }
    };
    for (s, e, ids) in cites {
        until(s, &mut out, &mut cursor);
        out.push(Piece::Cite(ids));
        cursor = e;
        until(e, &mut out, &mut cursor);
    }
    until(text.len(), &mut out, &mut cursor);
    out
}

/// The completeness review's text fields with their span paths.
pub fn completeness_fields(c: &CompletenessV3) -> Vec<(String, &str)> {
    let mut v: Vec<(String, &str)> = Vec::new();
    for (k, x) in c.added_outcomes.iter().enumerate() {
        v.push((format!("added_outcomes.{k}.outcome"), &x.outcome));
        v.push((format!("added_outcomes.{k}.effect_text"), &x.effect_text));
        v.push((format!("added_outcomes.{k}.why"), &x.why));
    }
    for (k, x) in c.outcome_additions.iter().enumerate() {
        v.push((format!("outcome_additions.{k}.text"), &x.text));
    }
    for (k, x) in c.added_conflicts.iter().enumerate() {
        v.push((format!("added_conflicts.{k}.text"), &x.text));
    }
    for (k, x) in c.notes.iter().enumerate() {
        v.push((format!("notes.{k}"), x));
    }
    v
}

/// How many "not traced" marks the page does not draw because they sit on
/// a registration ID (item 12), over the writer's and the completeness
/// review's text.
pub fn suppressed_registration_marks(a: &AnswerV3) -> usize {
    let count = |fields: Vec<(String, &str)>, spans: &[Span]| -> usize {
        fields
            .iter()
            .map(|(f, t)| {
                let mut at = 0;
                marked_runs(t, f, spans)
                    .into_iter()
                    .filter(|(run, m)| {
                        let (s, e) = (at, at + run.len());
                        at = e;
                        *m && is_registration_run(t, s, e)
                    })
                    .count()
            })
            .sum()
    };
    let w = a
        .answer
        .as_ref()
        .map_or(0, |w| count(text_fields(w), &a.untraced));
    let c = a
        .completeness
        .as_ref()
        .map_or(0, |c| count(completeness_fields(c), &c.untraced));
    w + c
}

// ---------- guidelines (items 10 and 12) ----------

/// A statement as a comparison key: quotes and a final full stop removed,
/// lower case, whitespace squashed.
pub fn statement_key(s: &str) -> String {
    let t = s
        .trim()
        .trim_matches(|c: char| matches!(c, '"' | '\u{201c}' | '\u{201d}' | '\'' | ' '))
        .trim_end_matches('.')
        .to_lowercase();
    SPACES.replace_all(&t, " ").trim().to_string()
}

/// Where a guideline entry comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GuidelineSource {
    /// The writer's `guidelines[i]` (answers stored before R-S4a).
    Writer(usize),
    /// The skeptic's verified `guidelines[i]`.
    Skeptic(usize),
    /// An unverified guideline lead (`leads[i]`).
    Lead(usize),
}

/// One guideline line and its body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GuidelineEntry {
    pub body: String,
    pub source: GuidelineSource,
}

/// The heading of the unverified guideline leads.
pub const LEAD_BODY: &str = "Unverified leads";

/// The body a skeptic guideline names: its text before the first ": "
/// when that is short, otherwise "Other".
pub fn skeptic_body(text: &str) -> String {
    match text.split_once(": ") {
        Some((b, _)) if !b.trim().is_empty() && b.len() <= 80 => b.trim().to_string(),
        _ => "Other".to_string(),
    }
}

/// The guideline lines, grouped by body (first-appearance order), each
/// statement once (item 12): a later line repeating an earlier statement is
/// left out unless it carries a mark of its own (flag, never delete).
/// `on_rows` are the PMIDs whose leads are drawn on a trial (as today).
pub fn guideline_entries(a: &AnswerV3, on_rows: &HashSet<String>) -> Vec<GuidelineEntry> {
    let mut seen: HashSet<String> = HashSet::new();
    let mut flat: Vec<GuidelineEntry> = Vec::new();
    let w = a.answer.as_ref();
    let marked = |field: &str, text: &str, spans: &[Span]| {
        marked_runs(text, field, spans).iter().any(|(_, m)| *m)
    };
    for (i, g) in w
        .map_or(&[][..], |w| w.guidelines.as_slice())
        .iter()
        .enumerate()
    {
        let key = statement_key(&g.statement);
        let has_marks = marked(
            &format!("guidelines.{i}.statement"),
            &g.statement,
            &a.untraced,
        ) || marked(&format!("guidelines.{i}.body"), &g.body, &a.untraced);
        if !key.is_empty() && !seen.insert(key) && !has_marks {
            continue;
        }
        flat.push(GuidelineEntry {
            body: g.body.trim().to_string(),
            source: GuidelineSource::Writer(i),
        });
    }
    if let Some(k) = &a.skeptic {
        for (i, g) in k.guidelines.iter().enumerate() {
            let statement = g.text.split_once(": ").map_or(g.text.as_str(), |(_, s)| s);
            let keys: Vec<String> = [Some(statement), g.quote.as_deref()]
                .into_iter()
                .flatten()
                .map(statement_key)
                .filter(|k| !k.is_empty())
                .collect();
            let has_marks = marked(&format!("guidelines.{i}.text"), &g.text, &k.spans);
            let repeat = !keys.is_empty() && keys.iter().any(|k| seen.contains(k));
            if repeat && !has_marks {
                continue;
            }
            seen.extend(keys);
            flat.push(GuidelineEntry {
                body: skeptic_body(&g.text),
                source: GuidelineSource::Skeptic(i),
            });
        }
        for (i, l) in k.leads.iter().enumerate() {
            if l.kind == "guideline" && !on_rows.contains(&l.pmid) {
                flat.push(GuidelineEntry {
                    body: LEAD_BODY.to_string(),
                    source: GuidelineSource::Lead(i),
                });
            }
        }
    }
    // Grouped by body, keeping first appearance (stable).
    let mut bodies: Vec<String> = Vec::new();
    for e in &flat {
        if !bodies.contains(&e.body) {
            bodies.push(e.body.clone());
        }
    }
    bodies
        .iter()
        .flat_map(|b| flat.iter().filter(move |e| &e.body == b).cloned())
        .collect()
}

/// The guideline statements (writer statements, skeptic guideline
/// statements and quotes) as keys.
pub fn guideline_keys(a: &AnswerV3) -> HashSet<String> {
    let mut keys: HashSet<String> = HashSet::new();
    if let Some(w) = &a.answer {
        keys.extend(w.guidelines.iter().map(|g| statement_key(&g.statement)));
    }
    if let Some(k) = &a.skeptic {
        for g in &k.guidelines {
            let statement = g.text.split_once(": ").map_or(g.text.as_str(), |(_, s)| s);
            keys.insert(statement_key(statement));
            if let Some(q) = &g.quote {
                keys.insert(statement_key(q));
            }
        }
    }
    keys.remove("");
    keys
}

/// Per skeptic `contradicting[i]`: whether it is shown under Guidelines
/// only (item 12: its quote is a guideline statement). An item with a mark
/// of its own stays in "Evidence against" (flag, never delete).
pub fn against_moved(a: &AnswerV3) -> Vec<bool> {
    let keys = guideline_keys(a);
    a.skeptic.as_ref().map_or_else(Vec::new, |k| {
        k.contradicting
            .iter()
            .enumerate()
            .map(|(i, c)| {
                let quote_is_guideline = c
                    .quote
                    .as_deref()
                    .is_some_and(|q| keys.contains(&statement_key(q)));
                let marked = marked_runs(&c.text, &format!("contradicting.{i}.text"), &k.spans)
                    .iter()
                    .any(|(_, m)| *m);
                quote_is_guideline && !marked
            })
            .collect()
    })
}

// ---------- the study sheet's "Cited in" (item 11) ----------

/// Where a study is cited among the outcomes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CitedIn {
    /// The writer's outcome `i`.
    Outcome(usize),
    /// The completeness review's added outcome `k`.
    Added(usize),
}

/// Whether `text` cites `pmid` (a `[PMID]` marker or `[<pmid> as <n>]`).
pub fn cites_pmid(text: &str, pmid: &str) -> bool {
    let known = |p: &str| p == pmid;
    citation_ranges(text, known)
        .into_iter()
        .any(|(s, e)| IDS.find_iter(&text[s..e]).any(|d| d.as_str() == pmid))
        || as_citations(text).iter().any(|(_, _, p, _)| p == pmid)
}

/// The outcomes citing `pmid`: in their effect, their numbers or their
/// cited studies.
pub fn cited_in(a: &AnswerV3, pmid: &str) -> Vec<CitedIn> {
    let mut out: Vec<CitedIn> = Vec::new();
    if let Some(w) = &a.answer {
        for (i, o) in w.outcomes.iter().enumerate() {
            if cites_pmid(&o.effect_text, pmid)
                || o.numbers.iter().any(|n| n.pmid.trim() == pmid)
                || o.pmids.iter().any(|p| p.trim() == pmid)
            {
                out.push(CitedIn::Outcome(i));
            }
        }
    }
    if let Some(c) = &a.completeness {
        for (k, x) in c.added_outcomes.iter().enumerate() {
            if cites_pmid(&x.effect_text, pmid) || x.pmids.iter().any(|p| p.trim() == pmid) {
                out.push(CitedIn::Added(k));
            }
        }
    }
    out
}

// ---------- what the page shows ----------

/// The blocks shown for an answer (any block with no data is hidden).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Visible {
    pub glance: bool,
    pub practice: bool,
    pub out_of_100: bool,
    pub forest: bool,
    pub outcomes: bool,
    pub compare: bool,
    /// The "no structured numbers" note, in place of blocks 5, 6 and 8.
    pub no_numbers: bool,
}

/// The CI rows of the forest: (group index, number index) of every ratio
/// or difference with both CI bounds.
pub fn forest_rows(w: &WrittenAnswer, groups: &[NumGroup]) -> Vec<(usize, usize)> {
    let mut rows: Vec<(usize, usize)> = Vec::new();
    for (k, g) in groups.iter().enumerate() {
        let o = &w.outcomes[g.outcome];
        for &j in &g.numbers {
            let n = &o.numbers[j];
            if has_ci(n) && (is_ratio(n) || is_diff(n)) {
                rows.push((k, j));
            }
        }
    }
    rows
}

pub fn visible(a: &AnswerV3) -> Visible {
    let Some(w) = &a.answer else {
        return Visible {
            glance: false,
            practice: false,
            out_of_100: false,
            forest: false,
            outcomes: false,
            compare: false,
            no_numbers: false,
        };
    };
    let groups = all_groups(w);
    let added = a
        .completeness
        .as_ref()
        .is_some_and(|c| !c.added_outcomes.is_empty());
    Visible {
        glance: w.at_a_glance.is_some(),
        practice: w.clinical_practice.is_some(),
        out_of_100: !out_of_100(w, &groups).is_empty(),
        forest: !forest_rows(w, &groups).is_empty(),
        outcomes: !w.outcomes.is_empty() || added,
        compare: groups.iter().any(|g| g.kind == ChartKind::Pair),
        no_numbers: groups.is_empty(),
    }
}

/// The debug line's counts (item 13): outcomes, numbers, studies,
/// references, untraced spans (the writer's, then the completeness
/// review's).
pub fn debug_counts(a: &AnswerV3) -> (usize, usize, usize, usize, usize, usize) {
    let w = a.answer.as_ref();
    let added = a
        .completeness
        .as_ref()
        .map_or(0, |c| c.added_outcomes.len());
    (
        w.map_or(0, |w| w.outcomes.len()) + added,
        w.map_or(0, |w| w.outcomes.iter().map(|o| o.numbers.len()).sum()),
        w.map_or(0, |w| w.studies.len()),
        a.references.len(),
        a.untraced.len(),
        a.completeness.as_ref().map_or(0, |c| c.untraced.len()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::answer_v3::{Applicability, BottomLine, Outcome, Study, WrittenAnswer};

    fn num(label: &str, v: f64, unit: Option<&str>, arm: Option<&str>, pmid: &str) -> Number {
        Number {
            label: label.into(),
            value: v,
            unit: unit.map(String::from),
            arm: arm.map(String::from),
            ci_low: None,
            ci_high: None,
            p: None,
            pmid: pmid.into(),
        }
    }

    fn ci(mut n: Number, l: f64, h: f64) -> Number {
        n.ci_low = Some(l);
        n.ci_high = Some(h);
        n
    }

    fn outcome(name: &str, numbers: Vec<Number>) -> Outcome {
        Outcome {
            outcome: name.into(),
            rct_evidence: String::new(),
            effect_text: String::new(),
            numbers,
            certainty: Certainty::Low,
            why: String::new(),
            pmids: vec![],
            evidence_type: Default::default(),
        }
    }

    fn study(pmid: &str, design: &str, comparison: &str) -> Study {
        Study {
            pmid: pmid.into(),
            label: format!("Trial {pmid}, 2020"),
            design: design.into(),
            n: None,
            setting: String::new(),
            comparison: comparison.into(),
            key_results: String::new(),
            appraisal: String::new(),
        }
    }

    fn written(outcomes: Vec<Outcome>, studies: Vec<Study>) -> WrittenAnswer {
        WrittenAnswer {
            bottom_line: BottomLine {
                text: "x".into(),
                certainty: Certainty::Low,
                certainty_reason: String::new(),
            },
            outcomes,
            studies,
            conflicts: vec![],
            applicability: Applicability {
                case_mix: String::new(),
                india_cost_context: String::new(),
                training: String::new(),
            },
            would_change: vec![],
            at_a_glance: None,
            clinical_practice: None,
            guidelines: vec![],
            notable_findings: vec![],
        }
    }

    #[test]
    fn page_order_is_fixed_and_the_original_view_is_last() {
        assert_eq!(PAGE_ORDER.first(), Some(&Block::Header));
        assert_eq!(PAGE_ORDER.last(), Some(&Block::Original));
        assert_eq!(SECTION_ORDER[0], Section::Conflicts);
        assert_eq!(SECTION_ORDER[11], Section::Cost);
    }

    #[test]
    fn p_is_normalised_for_display() {
        assert_eq!(normalise_p("P < .001"), "< .001");
        assert_eq!(normalise_p("p=0.03"), "0.03");
        assert_eq!(normalise_p(" P = 0.431 "), "0.431");
        assert_eq!(normalise_p("<0.001"), "<0.001");
        assert_eq!(normalise_p("0.012"), "0.012");
        assert_eq!(p_label(Some("P < .001")).as_deref(), Some("p < .001"));
        assert_eq!(p_label(Some("p")), None);
        assert_eq!(p_label(Some("  ")), None);
        assert_eq!(p_label(None), None);
    }

    #[test]
    fn chart_kinds_follow_the_shape_of_the_numbers() {
        let rr = ci(num("Seroma", 0.4, Some("RR"), None, "1"), 0.08, 2.03);
        let rr2 = ci(num("Seroma", 0.6, Some("OR"), Some("x"), "1"), 0.1, 3.0);
        assert_eq!(chart_kind(&[&rr, &rr2]), ChartKind::Ratio);
        // A ratio without a CI is not drawn on a forest.
        let rr_no_ci = num("Seroma", 0.4, Some("HR"), None, "1");
        assert_eq!(chart_kind(&[&rr_no_ci]), ChartKind::Text);
        let md = ci(num("LOS", 0.33, Some("days"), Some("MD"), "1"), -0.03, 0.7);
        assert_eq!(chart_kind(&[&md]), ChartKind::Diff);
        let diff = ci(
            num("Mean difference in pain", -1.2, None, None, "1"),
            -2.0,
            -0.4,
        );
        assert_eq!(chart_kind(&[&diff]), ChartKind::Diff);
        let minus = ci(
            num("Robotic minus lap", 4.0, Some("min"), None, "1"),
            1.0,
            7.0,
        );
        assert_eq!(chart_kind(&[&minus]), ChartKind::Diff);
        // "md" inside a word is not a difference.
        let word = ci(num("Amd score", 4.0, None, None, "1"), 1.0, 7.0);
        assert_eq!(chart_kind(&[&word]), ChartKind::Text);
        let a = num("Discharge", 95.9, Some("%"), Some("R-TAPP"), "1");
        let b = num("Discharge", 81.5, Some("%"), Some("L-TAPP"), "1");
        assert_eq!(chart_kind(&[&a, &b]), ChartKind::Pair);
        // Repeated arms are not a pair.
        let c = num("Discharge", 80.0, Some("%"), Some("R-TAPP"), "1");
        assert_eq!(chart_kind(&[&a, &b, &c]), ChartKind::Text);
        // Two numbers without arms are not a pair.
        let n1 = num("Pain", 2.0, None, None, "1");
        let n2 = num("Pain", 3.0, None, None, "1");
        assert_eq!(chart_kind(&[&n1, &n2]), ChartKind::Text);
        // A ratio among arms makes it text.
        let r = num("Discharge", 1.2, Some("RR"), Some("pooled"), "1");
        assert_eq!(chart_kind(&[&a, &r]), ChartKind::Text);
        assert_eq!(chart_kind(&[&a]), ChartKind::Text);
        assert_eq!(chart_kind(&[]), ChartKind::Text);
    }

    #[test]
    fn numbers_group_by_label_and_study_intervention_first() {
        let w = written(
            vec![outcome(
                "Pain",
                vec![
                    num("VAS 24 h", 3.0, None, Some("laparoscopic TEP"), "11111"),
                    num("VAS 24 h", 2.0, None, Some("robotic TAPP"), "11111"),
                    num("VAS 24 h", 4.0, None, Some("robotic TAPP"), "22222"),
                    num(" VAS 24 h ", 5.0, None, Some("laparoscopic TEP"), "22222 "),
                    num("VAS 7 d", 1.0, None, None, "11111"),
                ],
            )],
            vec![study("11111", "RCT", "Robotic TAPP vs laparoscopic TEP")],
        );
        let g = outcome_groups(&w, 0);
        assert_eq!(g.len(), 3);
        assert_eq!(
            (g[0].label.as_str(), g[0].pmid.as_str()),
            ("VAS 24 h", "11111")
        );
        assert_eq!(g[0].numbers, vec![1, 0], "robotic first by the comparison");
        assert_eq!(g[0].kind, ChartKind::Pair);
        assert_eq!(g[1].numbers, vec![2, 3], "no study row: the writer's order");
        assert_eq!(g[2].kind, ChartKind::Text);
        assert_eq!(all_groups(&w).len(), 3);
    }

    #[test]
    fn comparisons_group_after_normalisation() {
        assert_eq!(
            normalise_comparison("  Robotic TAPP (Xi)  vs   Laparoscopic TAPP "),
            "robotic tapp vs laparoscopic tapp"
        );
        let pair = |pmid: &str| {
            vec![
                num("x", 1.0, None, Some("a"), pmid),
                num("x", 2.0, None, Some("b"), pmid),
            ]
        };
        let w = written(
            vec![
                outcome("A", pair("1")),
                outcome("B", pair("2")),
                outcome("C", pair("3")),
                outcome("D", pair("4")),
            ],
            vec![
                study("1", "RCT", "Robotic TAPP (da Vinci) vs laparoscopic TAPP"),
                study("2", "RCT", "robotic TAPP vs  Laparoscopic TAPP"),
                study("3", "RCT", "POEM vs LHM"),
            ],
        );
        let groups = all_groups(&w);
        let pairs: Vec<NumGroup> = groups
            .into_iter()
            .filter(|g| g.kind == ChartKind::Pair)
            .collect();
        let c = comparison_groups(&w, &pairs);
        assert_eq!(c.len(), 3);
        assert_eq!(c[0].label, "Robotic TAPP (da Vinci) vs laparoscopic TAPP");
        assert_eq!(c[0].items, vec![0, 1]);
        assert_eq!(c[1].items, vec![2]);
        assert_eq!(
            (c[2].label.as_str(), c[2].items.as_slice()),
            ("Other", &[3][..])
        );
    }

    #[test]
    fn out_of_100_takes_all_percent_groups_of_two_or_three_arms() {
        let p = |v, arm| num("Relief", v, Some("%"), Some(arm), "1");
        let w = written(
            vec![
                outcome("Two arms", vec![p(44.0, "POEM"), p(30.0, "LHM")]),
                outcome("Three arms", vec![p(1.0, "a"), p(2.0, "b"), p(3.0, "c")]),
                outcome(
                    "Four arms",
                    vec![p(1.0, "a"), p(2.0, "b"), p(3.0, "c"), p(4.0, "d")],
                ),
                outcome(
                    "Mixed units",
                    vec![
                        p(44.0, "POEM"),
                        num("Relief", 30.0, Some("patients"), Some("LHM"), "1"),
                    ],
                ),
                outcome("One arm", vec![p(44.0, "POEM")]),
            ],
            vec![],
        );
        let groups = all_groups(&w);
        let picked: Vec<usize> = out_of_100(&w, &groups)
            .into_iter()
            .map(|k| groups[k].outcome)
            .collect();
        assert_eq!(picked, vec![0, 1]);
        assert_eq!(out_of_100_count(43.6), 44);
        assert_eq!(out_of_100_count(120.0), 100);
        assert_eq!(out_of_100_count(-3.0), 0);
        assert_eq!(out_of_100_count(f64::NAN), 0);
        assert_eq!(
            patient_sentence(
                "Dysphagia relief",
                &[(44, "POEM".into()), (30, "LHM".into())]
            ),
            "Out of 100 people like those in the study, about 44 after POEM, compared with about 30 after LHM, had this: dysphagia relief."
        );
        assert_eq!(
            plain_certainty(Certainty::VeryLow),
            "We are very unsure; treat these numbers as a rough guide."
        );
    }

    #[test]
    fn source_types_come_from_the_design() {
        assert_eq!(src_type("Multicentre RCT"), SrcType::Trials);
        assert_eq!(
            src_type("Systematic review and meta-analysis of RCTs"),
            SrcType::MetaAnalyses
        );
        assert_eq!(
            src_type("SR/MA"),
            SrcType::Trials,
            "the mockup's rule, as written"
        );
        assert_eq!(src_type("Retrospective cohort"), SrcType::Observational);
        assert_eq!(
            src_type("National registry analysis"),
            SrcType::Observational
        );
        assert_eq!(src_type("Markov cost-effectiveness model"), SrcType::Models);
        // Observational wins over meta-analysis (the mockup's order).
        assert_eq!(
            src_type("Meta-analysis of observational studies"),
            SrcType::Observational
        );
        assert_eq!(src_type(""), SrcType::Trials);
    }

    #[test]
    fn values_print_as_stored() {
        assert_eq!(fmt_value(0.431), "0.431");
        assert_eq!(fmt_value(-0.03), "\u{2212}0.03");
        assert_eq!(fmt_value(3258.0), "3,258");
        assert_eq!(fmt_value(1234567.0), "1,234,567");
        assert_eq!(fmt_value(80.0), "80");
        assert_eq!(with_unit(44.0, Some("%")), "44%");
        assert_eq!(with_unit(0.4, Some("RR")), "RR 0.4");
        assert_eq!(with_unit(80.3, Some("min")), "80.3 min");
        let mut n = ci(num("LOS", 0.33, Some("days"), Some("MD"), "1"), -0.03, 0.7);
        n.p = Some("P = 0.08".into());
        assert_eq!(
            number_line(&n),
            "0.33 days (MD), 95% CI \u{2212}0.03 to 0.7, p 0.08"
        );
        assert_eq!(diff_estimate(&n), "0.33 days (\u{2212}0.03 to 0.7)");
        let r = ci(num("Seroma", 0.4, Some("RR"), None, "1"), 0.08, 2.03);
        assert_eq!(ratio_estimate(&r), "RR 0.4 (0.08\u{2013}2.03)");
        // 1 sits at log(1/0.05) / log(50/0.05) of the axis.
        assert!((log_pos(1.0) - 100.0 * 20f64.log10() / 1000f64.log10()).abs() < 1e-9);
        assert_eq!(log_pos(0.001), 0.0);
        assert_eq!(log_pos(1000.0), 100.0);
        assert!((diff_pos(0.0, diff_scale(&n)) - 50.0).abs() < 1e-9);
        assert_eq!(study_short("ROGER (2025), Br J Surg"), "ROGER (2025)");
    }

    #[test]
    fn first_sentences_drop_citations() {
        assert_eq!(
            first_sentence("No difference in pain [12345678; 23456789]. Costs were higher [1]."),
            "No difference in pain."
        );
        assert_eq!(
            first_sentence("p 0.03 favoured robotic"),
            "p 0.03 favoured robotic"
        );
        assert_eq!(
            strip_citations("Primary unilateral [42927985 as 10] hernias ."),
            "Primary unilateral hernias."
        );
    }

    #[test]
    fn pmid_as_n_is_drawn_as_the_citation_of_the_pmid() {
        let known = |p: &str| p == "42927985" || p == "11111111";
        let t = "Seroma RR 0.40 [42927985 as 10]. Conversion [10]; more [11111111].";
        let p = pieces(t, "f", &[], known, true);
        assert_eq!(
            p,
            vec![
                Piece::Text("Seroma RR 0.40 ".into()),
                Piece::Cite(vec!["42927985".into()]),
                Piece::Text(". Conversion [10]; more ".into()),
                Piece::Cite(vec!["11111111".into()]),
                Piece::Text(".".into()),
            ]
        );
        assert_eq!(as_citations(t).len(), 1);
        assert_eq!(as_citations(t)[0].3, "10");
        // An unknown PMID stays as written (never dropped).
        let p = pieces("x [99999999 as 3]", "f", &[], |_| false, true);
        assert_eq!(p, vec![Piece::Text("x [99999999 as 3]".into())]);
        assert!(cites_pmid(t, "42927985"));
        assert!(cites_pmid(t, "11111111"));
        assert!(!cites_pmid(t, "99999999"));
    }

    #[test]
    fn marks_follow_their_runs_and_registration_ids_are_suppressed() {
        let t = "Pain fell 37% [12345678] in NTR3593 (CRD420261370145).";
        let at = |s: &str| t.find(s).unwrap();
        let spans = vec![
            Span::new("f", at("37"), at("37") + 2),
            Span::new("f", at("3593"), at("3593") + 4),
            Span::new("f", at("420261370145"), at("420261370145") + 12),
            Span::new("g", 0, 4),
        ];
        let p = pieces(t, "f", &spans, |_| false, true);
        assert_eq!(
            p,
            vec![
                Piece::Text("Pain fell 37".into()),
                Piece::Mark,
                Piece::Text("% ".into()),
                Piece::Cite(vec!["12345678".into()]),
                Piece::Text(" in NTR3593".into()),
                Piece::Suppressed,
                Piece::Text(" (CRD420261370145".into()),
                Piece::Suppressed,
                Piece::Text(").".into()),
            ]
        );
        // Without suppression (the "not verified" marks): drawn.
        let p = pieces(t, "f", &spans, |_| false, false);
        assert_eq!(p.iter().filter(|x| **x == Piece::Mark).count(), 3);
        // Only whole IDs: "ANTR35" and a plain number are not registrations.
        assert!(!is_registration_run("xANTR35", 5, 7));
        assert!(!is_registration_run("n = 3593", 4, 8));
        assert!(is_registration_run("NCT02816658", 0, 11));
        assert!(is_registration_run("(NCT02816658)", 4, 12));
        assert!(is_registration_run("ISRCTN123", 6, 9));
        // A mark inside a citation is drawn after it.
        let t2 = "Seroma [42927985 as 10] fell";
        let s2 = vec![Span::new(
            "f",
            t2.find("10").unwrap(),
            t2.find("10").unwrap() + 2,
        )];
        let p = pieces(t2, "f", &s2, |p| p == "42927985", true);
        assert_eq!(
            p,
            vec![
                Piece::Text("Seroma ".into()),
                Piece::Cite(vec!["42927985".into()]),
                Piece::Mark,
                Piece::Text(" fell".into()),
            ]
        );
        // The text is never changed.
        let joined: String = pieces(t, "f", &spans, |_| false, true)
            .into_iter()
            .map(|x| match x {
                Piece::Text(s) => s,
                Piece::Cite(ids) => format!("[{}]", ids.join("; ")),
                _ => String::new(),
            })
            .collect();
        assert_eq!(joined, t);
    }

    fn answer_with(w: Option<WrittenAnswer>) -> AnswerV3 {
        AnswerV3 {
            version: 3,
            question: "q".into(),
            mode: crate::runs::RunMode::Focused,
            created_at: chrono::Utc::now(),
            answer: w,
            empty_reason: None,
            references: vec![],
            searches: vec![],
            integrity: Default::default(),
            stopped_early: None,
            cost_paise: 0,
            elapsed_s: 0,
            skeptic: None,
            evidence: vec![],
            notes: vec![],
            untraced: vec![],
            notable_code: vec![],
            coverage: None,
            completeness: None,
        }
    }

    #[test]
    fn suppressed_marks_are_counted() {
        let mut w = written(vec![], vec![study("1", "RCT", "a vs b")]);
        w.studies[0].appraisal = "Same cohort (NTR3593); 12 lost".into();
        let t = w.studies[0].appraisal.clone();
        let mut a = answer_with(Some(w));
        a.untraced = vec![
            Span::new(
                "studies.0.appraisal",
                t.find("3593").unwrap(),
                t.find("3593").unwrap() + 4,
            ),
            Span::new(
                "studies.0.appraisal",
                t.find("12").unwrap(),
                t.find("12").unwrap() + 2,
            ),
        ];
        assert_eq!(suppressed_registration_marks(&a), 1);
    }

    #[test]
    fn guidelines_are_listed_once_and_quotes_move_from_evidence_against() {
        use crate::agent::answer_v3::{GuidelineV3, SkepticItem, SkepticLead, SkepticV3};
        let mut w = written(vec![], vec![]);
        w.guidelines = vec![
            GuidelineV3 {
                pmid: Some("1".into()),
                body: "EHS".into(),
                year: Some(2023),
                statement: "Use mesh.".into(),
                pmid_removed: false,
            },
            GuidelineV3 {
                pmid: Some("2".into()),
                body: "EHS".into(),
                year: Some(2023),
                statement: "use  mesh".into(),
                pmid_removed: false,
            },
        ];
        let mut a = answer_with(Some(w));
        let item = |pmid: &str, text: &str, quote: Option<&str>| SkepticItem {
            pmid: pmid.into(),
            text: text.into(),
            quote: quote.map(String::from),
        };
        a.skeptic = Some(SkepticV3 {
            verdict: "holds".into(),
            must_mention: vec![],
            contradicting: vec![
                item(
                    "3",
                    "Guideline says mesh",
                    Some("\u{201c}Use mesh.\u{201d}"),
                ),
                item("4", "A trial found 12 more", Some("Use mesh.")),
                item("5", "Other claim", Some("Something else")),
            ],
            missed_rcts: vec![],
            guidelines: vec![
                item("6", "HerniaSurge: Use mesh", Some("Use mesh.")),
                item(
                    "7",
                    "HerniaSurge: TEP or TAPP",
                    Some("TEP or TAPP are equivalent"),
                ),
            ],
            overlaps: vec![],
            ma_notes: vec![],
            spans: vec![Span::new("contradicting.1.text", 15, 17)],
            leads: vec![
                SkepticLead {
                    pmid: "8".into(),
                    kind: "guideline".into(),
                },
                SkepticLead {
                    pmid: "9".into(),
                    kind: "contradicting study".into(),
                },
            ],
            dropped_unseen: 0,
            dropped_quotes: 0,
            extract_next: vec![],
        });
        let e = guideline_entries(&a, &HashSet::new());
        let sources: Vec<GuidelineSource> = e.iter().map(|x| x.source).collect();
        assert_eq!(
            sources,
            vec![
                GuidelineSource::Writer(0),
                GuidelineSource::Skeptic(1),
                GuidelineSource::Lead(0),
            ],
            "the repeated statements are listed once, grouped by body"
        );
        assert_eq!(e[1].body, "HerniaSurge");
        assert_eq!(e[2].body, LEAD_BODY);
        // The quote equals a guideline statement: shown under Guidelines
        // only; the item with a mark of its own stays.
        assert_eq!(against_moved(&a), vec![true, false, false]);
        // A lead drawn on a trial row is not listed here (as today).
        let on_rows: HashSet<String> = ["8".to_string()].into();
        assert_eq!(guideline_entries(&a, &on_rows).len(), 2);
    }

    #[test]
    fn an_empty_or_stage3_answer_hides_its_blocks() {
        let a = answer_with(None);
        let v = visible(&a);
        assert!(!v.glance && !v.practice && !v.outcomes && !v.compare && !v.forest);
        assert!(!v.no_numbers, "no note without an answer");
        // Stage 3 shaped: no glance, no practice, no numbers.
        let a = answer_with(Some(written(vec![outcome("Pain", vec![])], vec![])));
        let v = visible(&a);
        assert!(!v.glance && !v.practice && !v.out_of_100 && !v.forest && !v.compare);
        assert!(v.outcomes && v.no_numbers);
        // Numbers with a CI show the forest; percentages show Out of 100.
        let w = written(
            vec![outcome(
                "Seroma",
                vec![
                    ci(num("Seroma", 0.4, Some("RR"), None, "1"), 0.08, 2.03),
                    num("Discharge", 95.9, Some("%"), Some("R"), "1"),
                    num("Discharge", 81.5, Some("%"), Some("L"), "1"),
                ],
            )],
            vec![],
        );
        let v = visible(&answer_with(Some(w)));
        assert!(v.forest && v.out_of_100 && v.compare && !v.no_numbers);
        assert_eq!(debug_counts(&answer_with(None)), (0, 0, 0, 0, 0, 0));
    }
}
