//! The temporal evidence table (T1): when the evidence was generated, and
//! how an outcome's effect has moved across eras.
//!
//! Pure code: no Leptos, no I/O, no LLM, no database. [`build`] takes a
//! [`TemporalInput`] (read from a stored AnswerV3 body or its slim
//! projection) and returns a [`TemporalTable`]:
//!
//! 1. **Study kinds** ([`studies::study_kind`]): meta-analyses and reviews
//!    are markers (never pooled); guidelines are listed; the rest are
//!    primary.
//! 2. **Linked reports** ([`studies::link_reports`]): reports of one trial
//!    are grouped; the earliest publication is the parent, the others are
//!    follow-ups. "Likely share patients" notes are overlap caveats.
//! 3. **Treatment years** ([`studies::stated_years`]), else publication
//!    year minus 2 (`assumed`). Follow-ups inherit their parent's years and
//!    add a durability point at their own publication year.
//! 4. **Eras** ([`studies::era_bounds`]): five-year windows ending in the
//!    run's year.
//! 5. **Effect rows**: measure, direction (turned round to intervention
//!    first), pairs of arm values, counts derived only from known arm
//!    sizes, scale and SE.
//! 6. **Maths** per outcome × measure family ([`maths`]): era pools, Z
//!    against the earlier eras, a Kalman dynamic model per τ, DL random
//!    effects and the predictive range.
//! 7. **Trajectory label** ([`Label`], precedence in [`LABEL_PRECEDENCE`]).
//!
//! Every assumption made is written to [`TemporalTable::notes`].

pub mod effects;
pub mod input;
pub mod maths;
pub mod studies;

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::LazyLock;

use regex::Regex;
use serde::{Deserialize, Serialize};

pub use effects::{Effect, Family, Measure, Orientation, Polarity};
pub use input::TemporalInput;
pub use studies::{Era, Horizon, OverlapCaveat, StudyEntry, StudyKind, YearsFrom};

use effects::{ArmOrder, canon_arm, split_vs};
use maths::{Obs, Z95};

/// The version of [`TemporalTable`]'s shape.
pub const TEMPORAL_VERSION: u32 = 1;

/// |Z| at or above which an era counts as a shift.
pub const Z_SHIFT: f64 = 2.0;

// ---- Output types --------------------------------------------------------

/// The table.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TemporalTable {
    /// Always [`TEMPORAL_VERSION`].
    pub version: u32,
    pub question: String,
    /// The year the latest era ends in.
    pub run_year: Option<i32>,
    /// Oldest first.
    pub eras: Vec<Era>,
    pub studies: Vec<StudyEntry>,
    pub rows: Vec<Row>,
    pub outcomes: Vec<OutcomeResult>,
    pub markers: Vec<MarkerEntry>,
    pub guidelines: Vec<GuidelineEntry>,
    pub overlap_caveats: Vec<OverlapCaveat>,
    /// Every assumption made, in plain words.
    pub notes: Vec<String>,
}

/// What a row does in the table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RowRole {
    /// Enters its era (pooled when it has an SE; a point otherwise).
    Primary,
    /// A follow-up report of a trial: a durability point in the parent's
    /// era, never a second study.
    Durability,
    /// Another row of a study already represented in its group: shown,
    /// not used.
    Alternate,
    /// From a meta-analysis or review: shown as a marker, never pooled.
    Marker,
    /// No usable estimate (one arm only, a count pair with no arm sizes,
    /// an unusable value, or not an effect).
    NoEstimate,
}

/// Two arms' values, intervention first.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Pair {
    pub a: f64,
    pub b: f64,
    pub unit: Option<String>,
}

/// Events and arm sizes derived from a pair and known arm sizes.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Counts {
    pub events_a: f64,
    pub n_a: f64,
    pub events_b: f64,
    pub n_b: f64,
}

/// One effect row: a number, or a pair of arm numbers, of one study for one
/// outcome.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Row {
    /// `r-<outcome>-<number>` (the first number of a pair).
    pub id: String,
    /// `n-<outcome>-<number>`: the source numbers (two for a pair).
    pub number_ids: Vec<String>,
    pub outcome_id: String,
    pub group_id: Option<String>,
    pub study_id: String,
    pub label: String,
    /// What the source numbers measure.
    pub measure: Measure,
    /// The family this row is pooled in (a percent pair with known arm
    /// sizes becomes a derived odds ratio).
    pub family: Option<Family>,
    pub unit_key: String,
    pub polarity: Polarity,
    pub intervention: Option<String>,
    pub comparator: Option<String>,
    pub orientation: Orientation,
    pub flipped: bool,
    /// Natural scale, intervention vs comparator.
    pub value: Option<f64>,
    pub lo: Option<f64>,
    pub hi: Option<f64>,
    /// Pooling scale.
    pub y: Option<f64>,
    pub se: Option<f64>,
    pub pair: Option<Pair>,
    pub counts: Option<Counts>,
    pub derived: bool,
    pub role: RowRole,
    pub era_id: Option<String>,
    pub knowledge_year: Option<i32>,
    pub horizon_months: Option<u32>,
}

/// A meta-analysis or review and its rows (never pooled).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MarkerEntry {
    /// `m-<pmid>`.
    pub id: String,
    pub study_id: String,
    pub label: String,
    pub pub_year: Option<i32>,
    pub row_ids: Vec<String>,
}

/// A guideline (from the answer's guidelines, or a study whose design is
/// a guideline).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GuidelineEntry {
    /// `gl-<i>` or `gl-<pmid>`.
    pub id: String,
    pub pmid: Option<String>,
    pub body: String,
    pub year: Option<i32>,
    pub study_id: Option<String>,
}

/// An estimate on both scales.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Est {
    pub y: f64,
    pub se: f64,
    pub est: f64,
    pub lo: f64,
    pub hi: f64,
}

impl Est {
    fn new(o: Obs, family: Family) -> Est {
        let n = |x| effects::natural(x, family);
        Est {
            y: o.y,
            se: o.se,
            est: n(o.y),
            lo: n(o.y - Z95 * o.se),
            hi: n(o.y + Z95 * o.se),
        }
    }
    /// The 95% CI excludes the null.
    pub fn significant(&self) -> bool {
        self.y - Z95 * self.se > 0.0 || self.y + Z95 * self.se < 0.0
    }
}

/// A follow-up's durability point in its parent's era.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DurabilityPoint {
    pub row_id: String,
    pub knowledge_year: Option<i32>,
    pub horizon_months: Option<u32>,
    /// Natural scale.
    pub value: f64,
    pub favours: Option<bool>,
}

/// One era of one group.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EraCell {
    /// `c-<group>-<era>` without the prefixes, e.g. `c-o0-ratio-2016-2020`.
    pub id: String,
    pub era_id: String,
    /// Primary rows in this era.
    pub row_ids: Vec<String>,
    /// The fixed-effect pool of the rows with an SE.
    pub pooled: Option<Est>,
    /// The mean of the rows with no SE (on the pooling scale, shown on the
    /// natural scale); never pooled.
    pub point_mean: Option<f64>,
    /// `pooled.est` if any, else `point_mean`.
    pub estimate: Option<f64>,
    /// (era pool − earlier posterior mean) / sqrt(se² + earlier posterior
    /// variance), against the τ = 0 posterior before this era (the first
    /// pooled era is compared with the N(0, 1²) prior).
    pub z: Option<f64>,
    /// An earlier era has a pool: only then does `z` count as a shift.
    pub after_earlier: bool,
    pub favours: Option<bool>,
    /// The pooled CI excludes the null (`None`: no pool).
    pub significant: Option<bool>,
    /// No primary row.
    pub empty: bool,
    pub durability: Vec<DurabilityPoint>,
}

/// One era of the dynamic model.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DynamicEra {
    pub era_id: String,
    pub mean: f64,
    pub sd: f64,
    pub est: f64,
    pub lo: f64,
    pub hi: f64,
    pub p_favours: f64,
}

/// The dynamic model at one τ.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DynamicFit {
    pub tau: f64,
    pub log_likelihood: f64,
    pub eras: Vec<DynamicEra>,
    /// P(effect favours the intervention) after the latest era.
    pub p_favours: f64,
    /// `Some(true)`: favours with P > 0.8; `Some(false)`: against with
    /// P > 0.8; `None`: undecided.
    pub direction: Option<bool>,
}

/// DerSimonian–Laird across the group's primary rows with an SE.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RandomEffects {
    pub k: usize,
    pub est: Est,
    pub tau: f64,
    pub i2: f64,
    pub p_favours: f64,
}

/// Where the next study is predicted to land.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Predictive {
    pub sd: f64,
    pub est: f64,
    pub lo: f64,
    pub hi: f64,
    pub p_next_favours: f64,
}

/// The trajectory of an outcome. The first rule that holds (in
/// [`LABEL_PRECEDENCE`] order) is the label; every rule that holds is a
/// flag.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Label {
    /// No primary row with an estimate.
    NotAssessable,
    /// The earliest era with a pool is unfavourable and significant; a
    /// later era's pool is null (its CI includes the null).
    EarlyHarmNotSeenLater,
    /// Two consecutive pooled eras sit on opposite sides of the null and
    /// the later one has |Z| ≥ 2.
    DirectionFlipped,
    /// The latest pooled era has |Z| ≥ 2 toward the intervention.
    ShiftingToward,
    /// The latest pooled era has |Z| ≥ 2 against the intervention.
    ShiftingAgainst,
    /// A difference measure whose absolute era estimate shrinks from era
    /// to era (never grows), ending smaller than it began.
    GapNarrowing,
    /// At least two eras with data, at least one Z after an earlier pooled
    /// era, every such |Z| < 2, and every era estimate on one side.
    Stable,
    /// Unfavourable in every era with data and at the latest durability
    /// point, over at least two time points.
    PersistentTradeoff,
    /// The run's era is empty while an earlier era has data.
    NoRecentData,
    /// Only one era has data.
    OneEraOnly,
    /// None of the above.
    Inconclusive,
}

/// The label precedence: the first rule that holds wins.
///
/// Why this order: a significant early harm that a later era does not see
/// is the more specific reading of a crossing of the null, so it comes
/// before `direction_flipped` (which needs the later era to be a shift);
/// a crossing is the most specific shift; a shift outranks the slower
/// readings (narrowing, stability); a persistent trade-off may rest on
/// durability points alone, so it comes before the data-coverage labels;
/// coverage labels come last because they describe the data, not the
/// effect.
pub const LABEL_PRECEDENCE: [Label; 11] = [
    Label::NotAssessable,
    Label::EarlyHarmNotSeenLater,
    Label::DirectionFlipped,
    Label::ShiftingToward,
    Label::ShiftingAgainst,
    Label::GapNarrowing,
    Label::Stable,
    Label::PersistentTradeoff,
    Label::NoRecentData,
    Label::OneEraOnly,
    Label::Inconclusive,
];

/// One outcome × measure family × unit × polarity.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GroupResult {
    /// `g-<outcome>-<family>-<unit>-<polarity>`, e.g. `g-o0-ratio--higher_worse`.
    pub id: String,
    pub family: Family,
    pub unit_key: String,
    pub polarity: Polarity,
    pub row_ids: Vec<String>,
    pub durability_row_ids: Vec<String>,
    pub cells: Vec<EraCell>,
    pub dynamic: Vec<DynamicFit>,
    /// The direction is the same, with P > 0.8, at every τ (`None`: no
    /// rows with an SE).
    pub robust: Option<bool>,
    pub random_effects: Option<RandomEffects>,
    pub predictive: Option<Predictive>,
    pub label: Label,
    pub flags: Vec<Label>,
}

/// One outcome of the answer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OutcomeResult {
    /// `o-<index>`.
    pub id: String,
    pub outcome: String,
    pub groups: Vec<GroupResult>,
    /// The group the label is read from: the most rows with an SE, then
    /// the most primary rows, then ratios before differences.
    pub lead_group_id: Option<String>,
    pub label: Label,
    pub flags: Vec<Label>,
    pub marker_row_ids: Vec<String>,
}

// ---- Build -----------------------------------------------------------------

struct Notes(Vec<String>);

impl Notes {
    fn add(&mut self, s: impl Into<String>) {
        let s = s.into();
        if !self.0.contains(&s) {
            self.0.push(s);
        }
    }
}

/// Builds the table. Never panics on stored data: anything unusable is
/// skipped with a note.
pub fn build(input: &TemporalInput) -> TemporalTable {
    let mut notes = Notes(Vec::new());
    let ref_year: HashMap<&str, i32> = input
        .references
        .iter()
        .filter_map(|r| Some((r.pmid.trim(), r.year?)))
        .collect();
    let ref_skeptic: HashSet<&str> = input
        .references
        .iter()
        .filter(|r| r.found_by_skeptic)
        .map(|r| r.pmid.trim())
        .collect();

    // Steps 1–3.
    let mut entries = build_studies(input, &ref_year, &ref_skeptic, &mut notes);
    let caveats = link_and_inherit(input, &mut entries, &mut notes);

    // Step 4.
    let run_year = input.run_year().or_else(|| {
        let y = entries.iter().filter_map(|s| s.pub_year).max();
        if y.is_some() {
            notes
                .add("The run's date is missing; the latest publication year ends the latest era.");
        }
        y
    });
    let eras = assign_eras(&mut entries, run_year, &mut notes);

    // Step 5.
    let order = ArmOrder::from_arms(
        input
            .outcomes
            .iter()
            .flat_map(|o| &o.numbers)
            .filter_map(|n| n.arm.as_deref()),
    );
    if let Some((x, y)) = &order.dominant {
        notes.add(format!(
            "Direction: \"{x}\" is taken as the intervention and \"{y}\" as the comparator (the most common \"X vs Y\" in the run's numbers); effects are shown intervention vs comparator."
        ));
    }
    let arm_sizes: Vec<HashMap<String, f64>> = input.studies.iter().map(arm_sizes).collect();
    let mut rows = build_rows(input, &entries, &arm_sizes, &order, &mut notes);

    // Step 6–7.
    let outcomes = build_outcomes(input, &mut rows, &eras, &mut notes);

    let markers = entries
        .iter()
        .filter(|s| s.kind == StudyKind::Marker)
        .map(|s| MarkerEntry {
            id: format!("m-{}", s.id.trim_start_matches("s-")),
            study_id: s.id.clone(),
            label: s.label.clone(),
            pub_year: s.pub_year,
            row_ids: rows
                .iter()
                .filter(|r| r.study_id == s.id && r.role == RowRole::Marker)
                .map(|r| r.id.clone())
                .collect(),
        })
        .collect();

    let mut guidelines: Vec<GuidelineEntry> = input
        .guidelines
        .iter()
        .enumerate()
        .map(|(i, g)| {
            let pmid = g
                .pmid
                .as_ref()
                .map(|p| p.trim().to_string())
                .filter(|p| !p.is_empty());
            GuidelineEntry {
                id: format!("gl-{i}"),
                study_id: pmid
                    .as_ref()
                    .and_then(|p| entries.iter().find(|s| &s.pmid == p).map(|s| s.id.clone())),
                pmid,
                body: g.body.clone(),
                year: g.year.or_else(|| {
                    g.pmid
                        .as_deref()
                        .and_then(|p| ref_year.get(p.trim()).copied())
                }),
            }
        })
        .collect();
    for s in entries.iter().filter(|s| s.kind == StudyKind::Guideline) {
        if !guidelines
            .iter()
            .any(|g| g.pmid.as_deref() == Some(s.pmid.as_str()))
        {
            guidelines.push(GuidelineEntry {
                id: format!("gl-{}", s.id.trim_start_matches("s-")),
                pmid: (!s.pmid.is_empty()).then(|| s.pmid.clone()),
                body: s.label.clone(),
                year: s.pub_year,
                study_id: Some(s.id.clone()),
            });
        }
    }

    TemporalTable {
        version: TEMPORAL_VERSION,
        question: input.question.clone(),
        run_year,
        eras,
        studies: entries,
        rows,
        outcomes,
        markers,
        guidelines,
        overlap_caveats: caveats,
        notes: notes.0,
    }
}

fn build_studies(
    input: &TemporalInput,
    ref_year: &HashMap<&str, i32>,
    ref_skeptic: &HashSet<&str>,
    notes: &mut Notes,
) -> Vec<StudyEntry> {
    let mut seen: HashSet<String> = HashSet::new();
    let mut out = Vec::new();
    for (i, s) in input.studies.iter().enumerate() {
        let pmid = s.pmid.trim().to_string();
        let mut id = studies::study_id(&pmid, i);
        if !seen.insert(id.clone()) {
            notes.add(format!("Study {pmid} appears twice in the study table; the second is kept under its row number."));
            id = format!("s-row{i}");
            seen.insert(id.clone());
        }
        let kind = studies::study_kind(&s.design);
        let pub_year = ref_year.get(pmid.as_str()).copied();
        let (start, end, from, assumed) = match studies::stated_years(s) {
            Some((a, b, f)) => (Some(a), Some(b), f, false),
            None => match pub_year {
                Some(y) => (
                    Some(y - 2),
                    Some(y - 2),
                    YearsFrom::PublicationMinusTwo,
                    true,
                ),
                None => (None, None, YearsFrom::Unknown, false),
            },
        };
        if kind == StudyKind::Primary {
            match from {
                YearsFrom::PublicationMinusTwo => notes.add(format!(
                    "{}: treatment years not stated; publication year minus 2 used (assumed).",
                    s.label
                )),
                YearsFrom::Unknown => notes.add(format!(
                    "{}: no treatment years and no publication year; it has no era.",
                    s.label
                )),
                _ => {}
            }
        }
        out.push(StudyEntry {
            id,
            pmid: pmid.clone(),
            label: s.label.clone(),
            design: s.design.clone(),
            n: s.n,
            kind,
            pub_year,
            found_by_skeptic: ref_skeptic.contains(pmid.as_str()),
            treatment_start: start,
            treatment_end: end,
            years_from: from,
            assumed,
            midpoint: start.zip(end).map(|(a, b)| f64::from(a + b) / 2.0),
            era_id: None,
            parent_id: None,
            child_ids: Vec::new(),
            link_reason: None,
            knowledge_year: pub_year,
            horizon: studies::study_horizon(s),
        });
    }
    out
}

/// Step 2 and the inheritance of step 3. Only primary studies are linked:
/// a review is never a report of one trial.
fn link_and_inherit(
    input: &TemporalInput,
    entries: &mut [StudyEntry],
    notes: &mut Notes,
) -> Vec<OverlapCaveat> {
    let primary: Vec<usize> = (0..entries.len())
        .filter(|&i| entries[i].kind == StudyKind::Primary)
        .collect();
    let sub: Vec<_> = primary.iter().map(|&i| input.studies[i].clone()).collect();
    let years: Vec<Option<i32>> = primary.iter().map(|&i| entries[i].pub_year).collect();
    let (links, mut caveats) = studies::link_reports(&sub, &years, input);
    // Caveats name every study of the table, not only primaries.
    for c in &mut caveats {
        c.study_ids = c
            .pmids
            .iter()
            .filter_map(|p| entries.iter().find(|s| !s.pmid.is_empty() && &s.pmid == p))
            .map(|s| s.id.clone())
            .collect();
    }
    for (c, p, why) in links {
        let (c, p) = (primary[c], primary[p]);
        let pid = entries[p].id.clone();
        let cid = entries[c].id.clone();
        entries[p].child_ids.push(cid);
        entries[p].link_reason.get_or_insert_with(|| why.clone());
        let (ps, pe, pa) = (
            entries[p].treatment_start,
            entries[p].treatment_end,
            entries[p].assumed,
        );
        let pl = entries[p].label.clone();
        let ch = &mut entries[c];
        ch.parent_id = Some(pid);
        ch.link_reason = Some(why);
        ch.treatment_start = ps;
        ch.treatment_end = pe;
        ch.assumed = pa;
        ch.years_from = YearsFrom::Parent;
        ch.midpoint = ps.zip(pe).map(|(a, b)| f64::from(a + b) / 2.0);
        notes.add(format!(
            "{} is a later report of the same trial as {pl}: it takes that trial's treatment years and adds a durability point at its own publication year; it is not counted as a second study.",
            ch.label
        ));
    }
    caveats
}

fn assign_eras(entries: &mut [StudyEntry], run_year: Option<i32>, notes: &mut Notes) -> Vec<Era> {
    let Some(run_year) = run_year else {
        notes.add("No run date and no publication years: no eras could be built.");
        return Vec::new();
    };
    let earliest = entries
        .iter()
        .filter(|s| s.kind == StudyKind::Primary)
        .filter_map(|s| s.midpoint)
        .filter(|m| m.is_finite())
        .map(|m| m.floor() as i32)
        .min()
        .unwrap_or(run_year)
        .min(run_year);
    let bounds = studies::era_bounds(run_year, earliest);
    let mut counts = vec![0usize; bounds.len()];
    for s in entries.iter_mut().filter(|s| s.kind == StudyKind::Primary) {
        if let Some(e) = s.midpoint.and_then(|m| studies::era_of(&bounds, m)) {
            s.era_id = Some(studies::era_id(bounds[e].0, bounds[e].1));
            if s.parent_id.is_none() {
                counts[e] += 1;
            }
        } else if s.midpoint.is_some() {
            notes.add(format!(
                "{}: its treatment years fall before every era.",
                s.label
            ));
        }
    }
    let eras: Vec<Era> = bounds
        .iter()
        .zip(counts)
        .map(|(&(a, b), n)| Era {
            id: studies::era_id(a, b),
            start: a,
            end: b,
            label: studies::era_label(a, b),
            n_studies: n,
            empty: n == 0,
        })
        .collect();
    let empty: Vec<&str> = eras
        .iter()
        .filter(|e| e.empty)
        .map(|e| e.label.as_str())
        .collect();
    if !empty.is_empty() {
        notes.add(format!(
            "Eras with no primary study (kept, flagged): {}.",
            empty.join(", ")
        ));
    }
    eras
}

static ARM_N_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(\d{1,3}(?:,\d{3})+|\d+)\s+([a-z][\w+-]*)\s+(?:vs\.?|versus|and)\s+(\d{1,3}(?:,\d{3})+|\d+)\s+([a-z][\w+-]*)")
        .expect("arm n regex")
});

/// Arm sizes a study states ("107 robotic vs 215 laparoscopic"), by
/// canonical arm. Only both arms together count.
pub fn arm_sizes(s: &input::InStudy) -> HashMap<String, f64> {
    let mut out = HashMap::new();
    for text in [&s.appraisal, &s.key_results, &s.design, &s.setting] {
        for c in ARM_N_RE.captures_iter(text) {
            let num = |m: &str| m.replace(',', "").parse::<f64>().ok();
            let (Some(na), Some(nb)) = (num(&c[1]), num(&c[3])) else {
                continue;
            };
            let (a, b) = (canon_arm(&c[2]), canon_arm(&c[4]));
            if a.is_empty() || b.is_empty() || a == b || na <= 0.0 || nb <= 0.0 {
                continue;
            }
            out.entry(a).or_insert(na);
            out.entry(b).or_insert(nb);
        }
        if !out.is_empty() {
            break;
        }
    }
    out
}

/// One number of an outcome with its index.
struct Num<'a> {
    k: usize,
    n: &'a input::InNumber,
    measure: Measure,
}

fn build_rows(
    input: &TemporalInput,
    entries: &[StudyEntry],
    arm_sizes: &[HashMap<String, f64>],
    order: &ArmOrder,
    notes: &mut Notes,
) -> Vec<Row> {
    let mut rows = Vec::new();
    for (oi, o) in input.outcomes.iter().enumerate() {
        let mut used = vec![false; o.numbers.len()];
        let nums: Vec<Num> = o
            .numbers
            .iter()
            .enumerate()
            .map(|(k, n)| Num {
                k,
                n,
                measure: effects::classify(&n.label, n.unit.as_deref(), n.arm.as_deref()),
            })
            .collect();
        for a in &nums {
            if used[a.k] {
                continue;
            }
            used[a.k] = true;
            let pmid = a.n.pmid.trim();
            let Some(si) = entries
                .iter()
                .position(|s| !pmid.is_empty() && s.pmid == pmid)
            else {
                notes.add(format!(
                    "{}: \"{}\" cites {} which is not in the study table; not used.",
                    o.outcome,
                    a.n.label,
                    if pmid.is_empty() { "no study" } else { pmid }
                ));
                continue;
            };
            let st = &entries[si];
            let polarity = number_polarity(&nums, a, &o.outcome, notes);
            let mut row = Row {
                id: format!("r-{oi}-{}", a.k),
                number_ids: vec![format!("n-{oi}-{}", a.k)],
                outcome_id: format!("o-{oi}"),
                group_id: None,
                study_id: st.id.clone(),
                label: a.n.label.clone(),
                measure: a.measure,
                family: a.measure.family(),
                unit_key: effects::unit_key(a.measure, a.n.unit.as_deref()),
                polarity,
                intervention: None,
                comparator: None,
                orientation: Orientation::Unknown,
                flipped: false,
                value: None,
                lo: None,
                hi: None,
                y: None,
                se: None,
                pair: None,
                counts: None,
                derived: false,
                role: RowRole::NoEstimate,
                era_id: st.era_id.clone(),
                knowledge_year: st.knowledge_year,
                horizon_months: st.horizon.as_ref().map(|h| h.months),
            };
            let Some(value) = a.n.value.filter(|v| v.is_finite()) else {
                notes.add(format!(
                    "{}: \"{}\" has no usable value; skipped.",
                    o.outcome, a.n.label
                ));
                rows.push(finish_role(row, st));
                continue;
            };
            if let Some(fam) = a.measure.family() {
                one_number_row(&mut row, a.n, value, fam, order, &o.outcome, notes);
            } else if a.measure.is_arm_level() {
                // Find its partner: same study, same label, another arm.
                let partner = nums.iter().find(|b| {
                    !used[b.k]
                        && b.n.pmid.trim() == pmid
                        && b.measure == a.measure
                        && b.n.label.trim().eq_ignore_ascii_case(a.n.label.trim())
                        && b.n.value.is_some_and(f64::is_finite)
                        && canon_arm(b.n.arm.as_deref().unwrap_or(""))
                            != canon_arm(a.n.arm.as_deref().unwrap_or(""))
                });
                match partner {
                    Some(b) => {
                        used[b.k] = true;
                        row.number_ids.push(format!("n-{oi}-{}", b.k));
                        pair_row(&mut row, a.n, b.n, &arm_sizes[si], order, &o.outcome, notes);
                    }
                    None => notes.add(format!(
                        "{}: \"{}\" ({}) gives one arm only; shown, not used.",
                        o.outcome, a.n.label, st.label
                    )),
                }
            } else {
                notes.add(format!(
                    "{}: \"{}\" is not an effect the table can use; shown, not used.",
                    o.outcome, a.n.label
                ));
            }
            rows.push(finish_role(row, st));
        }
    }
    rows
}

/// A number's polarity: its label; else the nearest number of the same
/// study in this outcome whose label names one ("Adjusted difference at 5
/// years" after "Success at 5 years"); else the outcome's name; else
/// higher is worse (with a note).
fn number_polarity(nums: &[Num], a: &Num, outcome: &str, notes: &mut Notes) -> Polarity {
    if let Some(p) = effects::stated_polarity(&a.n.label) {
        return p;
    }
    let pmid = a.n.pmid.trim();
    let sibling = nums
        .iter()
        .filter(|b| b.k != a.k && b.n.pmid.trim() == pmid)
        .filter_map(|b| {
            Some((
                b.k.abs_diff(a.k),
                b.k,
                effects::stated_polarity(&b.n.label)?,
            ))
        })
        .min_by_key(|&(d, k, _)| (d, k));
    if let Some((_, k, p)) = sibling {
        notes.add(format!(
            "{outcome}: \"{}\" takes its direction of benefit from \"{}\" (same study).",
            a.n.label, nums[k].n.label
        ));
        return p;
    }
    effects::stated_polarity(outcome).unwrap_or_else(|| {
        notes.add(format!(
            "{outcome}: \"{}\": no direction of benefit is named; a higher value is taken as worse.",
            a.n.label
        ));
        Polarity::HigherWorse
    })
}

/// The role from the study kind and whether the row has an estimate.
fn finish_role(mut row: Row, st: &StudyEntry) -> Row {
    let has_estimate = row.y.is_some() && row.family.is_some();
    row.role = match st.kind {
        StudyKind::Marker | StudyKind::Guideline => RowRole::Marker,
        StudyKind::Primary if !has_estimate => RowRole::NoEstimate,
        StudyKind::Primary if st.parent_id.is_some() => RowRole::Durability,
        StudyKind::Primary => RowRole::Primary,
    };
    row
}

/// Orients a two-arm text, recording the arms; returns whether to flip.
fn orient_arms(row: &mut Row, arm: Option<&str>, order: &ArmOrder) -> bool {
    let Some((x, y)) = arm.and_then(split_vs) else {
        return false;
    };
    let o = order.orient(&x, &y);
    row.orientation = o;
    let flip = o == Orientation::Flipped;
    let (i, c) = if flip { (y, x) } else { (x, y) };
    row.intervention = Some(i);
    row.comparator = Some(c);
    flip
}

fn one_number_row(
    row: &mut Row,
    n: &input::InNumber,
    value: f64,
    fam: Family,
    order: &ArmOrder,
    outcome: &str,
    notes: &mut Notes,
) {
    let flip = orient_arms(row, n.arm.as_deref(), order);
    if row.orientation == Orientation::Unknown {
        notes.add(format!(
            "{outcome}: \"{}\" does not say which arm is the intervention; taken as written.",
            n.label
        ));
    }
    let mut e = Effect {
        value,
        lo: n.ci_low,
        hi: n.ci_high,
    };
    if flip {
        e = effects::flip(e, fam);
        row.flipped = true;
        notes.add(format!(
            "{outcome}: \"{}\" was written comparator first; turned round (ratios inverted, differences negated).",
            n.label
        ));
    }
    set_effect(row, e, fam, outcome, notes);
}

fn set_effect(row: &mut Row, e: Effect, fam: Family, outcome: &str, notes: &mut Notes) {
    match effects::to_scale(e, fam) {
        Ok((s, why)) => {
            row.value = Some(e.value);
            row.lo = e.lo;
            row.hi = e.hi;
            row.y = Some(s.y);
            row.se = s.se;
            if let Some(why) = why {
                notes.add(format!("{outcome}: \"{}\": {why}.", row.label));
            }
        }
        Err(why) => notes.add(format!("{outcome}: \"{}\": {why}; skipped.", row.label)),
    }
}

fn pair_row(
    row: &mut Row,
    a: &input::InNumber,
    b: &input::InNumber,
    sizes: &HashMap<String, f64>,
    order: &ArmOrder,
    outcome: &str,
    notes: &mut Notes,
) {
    let (arm_a, arm_b) = (
        a.arm.clone().unwrap_or_default(),
        b.arm.clone().unwrap_or_default(),
    );
    let o = order.orient(&arm_a, &arm_b);
    row.orientation = o;
    let flip = o == Orientation::Flipped;
    let (first, second) = if flip { (b, a) } else { (a, b) };
    row.flipped = flip;
    row.intervention = first.arm.clone();
    row.comparator = second.arm.clone();
    let (va, vb) = (
        first.value.unwrap_or(f64::NAN),
        second.value.unwrap_or(f64::NAN),
    );
    row.pair = Some(Pair {
        a: va,
        b: vb,
        unit: a.unit.clone(),
    });
    if o == Orientation::Unknown {
        notes.add(format!(
            "{outcome}: \"{}\" ({arm_a} / {arm_b}): which arm is the intervention is not known; taken in the order written.",
            a.label
        ));
    }
    let na = sizes.get(&canon_arm(row.intervention.as_deref().unwrap_or("")));
    let nb = sizes.get(&canon_arm(row.comparator.as_deref().unwrap_or("")));
    let counts = match row.measure {
        Measure::Percent => na.zip(nb).and_then(|(&na, &nb)| {
            Some((
                effects::events_from_percent(va, na)?,
                na,
                effects::events_from_percent(vb, nb)?,
                nb,
            ))
        }),
        Measure::Count => na.zip(nb).map(|(&na, &nb)| (va, na, vb, nb)),
        _ => None,
    };
    if let Some((ea, na, eb, nb)) = counts
        && let Some((y, se, corrected)) = effects::log_or_from_counts(ea, na, eb, nb)
    {
        row.family = Some(Family::Ratio);
        row.unit_key = String::new();
        row.derived = true;
        row.counts = Some(Counts {
            events_a: ea,
            n_a: na,
            events_b: eb,
            n_b: nb,
        });
        row.y = Some(y);
        row.se = Some(se);
        row.value = Some(y.exp());
        row.lo = Some((y - Z95 * se).exp());
        row.hi = Some((y + Z95 * se).exp());
        notes.add(format!(
            "{outcome}: \"{}\": counts derived from the stated arm sizes ({ea}/{na} vs {eb}/{nb}) and turned into an odds ratio{}.",
            a.label,
            if corrected { " (0.5 added to each cell for a zero count)" } else { "" }
        ));
        return;
    }
    match row.measure {
        Measure::Percent | Measure::Value if va.is_finite() && vb.is_finite() => {
            row.family = Some(Family::Difference);
            row.value = Some(va - vb);
            row.y = Some(va - vb);
            notes.add(format!(
                "{outcome}: \"{}\": the two arms are kept as a pair; their difference ({:+}) is a point without a CI and is never pooled{}.",
                a.label,
                round2(va - vb),
                if row.measure == Measure::Percent { " (arm sizes unknown)" } else { "" }
            ));
        }
        _ => notes.add(format!(
            "{outcome}: \"{}\": counts in two arms with no arm sizes; kept as a pair, no estimate.",
            a.label
        )),
    }
}

fn round2(x: f64) -> f64 {
    (x * 100.0).round() / 100.0
}

static ENDPOINT_DROP: &[&str] = &[
    "or",
    "aor",
    "rr",
    "irr",
    "hr",
    "md",
    "smd",
    "rd",
    "adjusted",
    "adj",
    "pooled",
    "iptw",
    "mean",
    "median",
    "difference",
    "diff",
    "rate",
    "overall",
    "in",
    "of",
    "the",
];
static AT_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)\bat\b.*$").expect("at"));

/// The endpoint a label names ("OR subtotal cholecystectomy, HCUP" →
/// "subtotal cholecystectomy"; "Reflux oesophagitis at 5 years" → "reflux
/// oesophagitis"): used to pick one row per study in a group.
pub fn endpoint_key(label: &str) -> String {
    let stem = label.split([',', '(']).next().unwrap_or("").to_lowercase();
    let stem = AT_RE.replace(&stem, "");
    stem.split(|c: char| !c.is_alphanumeric())
        .filter(|w| {
            !w.is_empty() && !ENDPOINT_DROP.contains(w) && !w.chars().all(|c| c.is_ascii_digit())
        })
        .collect::<Vec<_>>()
        .join(" ")
}

type GroupKey = (Family, String, Polarity);

fn group_id(oi: usize, k: &GroupKey) -> String {
    format!("g-o{oi}-{}-{}-{}", k.0.as_str(), k.1, k.2.as_str())
}

fn build_outcomes(
    input: &TemporalInput,
    rows: &mut [Row],
    eras: &[Era],
    notes: &mut Notes,
) -> Vec<OutcomeResult> {
    let mut out = Vec::new();
    for (oi, o) in input.outcomes.iter().enumerate() {
        let oid = format!("o-{oi}");
        // Group the rows with an estimate.
        let mut groups: BTreeMap<GroupKey, Vec<usize>> = BTreeMap::new();
        for (ri, r) in rows.iter().enumerate() {
            if r.outcome_id != oid || !matches!(r.role, RowRole::Primary | RowRole::Durability) {
                continue;
            }
            if let Some(f) = r.family {
                groups
                    .entry((f, r.unit_key.clone(), r.polarity))
                    .or_default()
                    .push(ri);
            }
        }
        // One row per study per group, by endpoint vote.
        for (key, idx) in &groups {
            let gid = group_id(oi, key);
            let mut votes: HashMap<String, HashSet<&str>> = HashMap::new();
            for &ri in idx {
                votes
                    .entry(endpoint_key(&rows[ri].label))
                    .or_default()
                    .insert(rows[ri].study_id.as_str());
            }
            let vote: HashMap<String, usize> =
                votes.into_iter().map(|(k, v)| (k, v.len())).collect();
            let mut by_study: BTreeMap<String, Vec<usize>> = BTreeMap::new();
            for &ri in idx {
                by_study
                    .entry(rows[ri].study_id.clone())
                    .or_default()
                    .push(ri);
            }
            for (_, rs) in by_study {
                let best = *rs
                    .iter()
                    .max_by_key(|&&ri| {
                        let r = &rows[ri];
                        (
                            r.se.is_some(),
                            vote.get(&endpoint_key(&r.label)).copied().unwrap_or(0),
                            std::cmp::Reverse(ri),
                        )
                    })
                    .expect("non-empty");
                for &ri in &rs {
                    rows[ri].group_id = Some(gid.clone());
                    if ri != best {
                        notes.add(format!(
                            "{}: \"{}\" is another result of a study already in this comparison (\"{}\"); shown, not used.",
                            o.outcome, rows[ri].label, rows[best].label
                        ));
                        rows[ri].role = RowRole::Alternate;
                    }
                }
            }
        }
        let results: Vec<GroupResult> = groups
            .keys()
            .map(|k| group_result(oi, k, rows, eras))
            .collect();
        let lead = results
            .iter()
            .max_by_key(|g| {
                let se_rows = g
                    .row_ids
                    .iter()
                    .filter(|id| rows.iter().any(|r| &r.id == *id && r.se.is_some()))
                    .count();
                (se_rows, g.row_ids.len(), g.family == Family::Ratio)
            })
            .filter(|g| !g.row_ids.is_empty());
        let (label, flags, lead_id) = match lead {
            Some(g) => (g.label, g.flags.clone(), Some(g.id.clone())),
            None => (Label::NotAssessable, vec![Label::NotAssessable], None),
        };
        out.push(OutcomeResult {
            id: oid.clone(),
            outcome: o.outcome.clone(),
            groups: results,
            lead_group_id: lead_id,
            label,
            flags,
            marker_row_ids: rows
                .iter()
                .filter(|r| r.outcome_id == oid && r.role == RowRole::Marker)
                .map(|r| r.id.clone())
                .collect(),
        });
    }
    out
}

fn favours_p(pol: Polarity, mean: f64, sd: f64) -> f64 {
    let below = maths::norm_cdf(-mean / sd);
    match pol {
        Polarity::HigherWorse => below,
        Polarity::HigherBetter => 1.0 - below,
    }
}

fn group_result(oi: usize, key: &GroupKey, rows: &[Row], eras: &[Era]) -> GroupResult {
    let (fam, pol) = (key.0, key.2);
    let gid = group_id(oi, key);
    let mine: Vec<&Row> = rows
        .iter()
        .filter(|r| r.group_id.as_deref() == Some(gid.as_str()))
        .collect();
    let primary: Vec<&Row> = mine
        .iter()
        .copied()
        .filter(|r| r.role == RowRole::Primary)
        .collect();
    let durable: Vec<&Row> = mine
        .iter()
        .copied()
        .filter(|r| r.role == RowRole::Durability)
        .collect();

    // Era cells. Z is against the τ = 0 filtered posterior before the era
    // (the N(0, 1²) prior updated by every earlier era's pool).
    let mut cells = Vec::new();
    let mut prev = maths::Posterior {
        mean: maths::PRIOR_MEAN,
        var: maths::PRIOR_SD * maths::PRIOR_SD,
    };
    let mut seen_pool = false;
    for e in eras {
        let in_era: Vec<&Row> = primary
            .iter()
            .copied()
            .filter(|r| r.era_id.as_deref() == Some(e.id.as_str()))
            .collect();
        let obs: Vec<Obs> = in_era
            .iter()
            .filter_map(|r| Some(Obs { y: r.y?, se: r.se? }))
            .filter(|o| o.usable())
            .collect();
        let pooled = maths::fixed_pool(&obs);
        let points: Vec<f64> = in_era
            .iter()
            .filter(|r| r.se.is_none())
            .filter_map(|r| r.y)
            .filter(|y| y.is_finite())
            .collect();
        let point_y =
            (!points.is_empty()).then(|| points.iter().sum::<f64>() / points.len() as f64);
        let z = pooled.and_then(|p| maths::z_vs_prior(p, prev));
        let after_earlier = pooled.is_some() && seen_pool;
        if let Some(p) = pooled {
            let (post, _) = maths::kalman_step(prev, p);
            prev = post;
            seen_pool = true;
        }
        let est_y = pooled.map(|p| p.y).or(point_y);
        let durability = durable
            .iter()
            .filter(|r| r.era_id.as_deref() == Some(e.id.as_str()))
            .filter_map(|r| {
                Some(DurabilityPoint {
                    row_id: r.id.clone(),
                    knowledge_year: r.knowledge_year,
                    horizon_months: r.horizon_months,
                    value: effects::natural(r.y?, fam),
                    favours: pol.favours(r.y?),
                })
            })
            .collect();
        cells.push(EraCell {
            id: format!(
                "c-o{oi}-{}-{}-{}-{}-{}",
                fam.as_str(),
                key.1,
                pol.as_str(),
                e.start,
                e.end
            ),
            era_id: e.id.clone(),
            row_ids: in_era.iter().map(|r| r.id.clone()).collect(),
            pooled: pooled.map(|p| Est::new(p, fam)),
            point_mean: point_y.map(|y| effects::natural(y, fam)),
            estimate: est_y.map(|y| effects::natural(y, fam)),
            z,
            after_earlier,
            favours: est_y.and_then(|y| pol.favours(y)),
            significant: pooled.map(|p| Est::new(p, fam).significant()),
            empty: in_era.is_empty(),
            durability,
        });
    }

    // Dynamic model over the era pools.
    let steps: Vec<Option<Obs>> = cells
        .iter()
        .map(|c| c.pooled.map(|p| Obs { y: p.y, se: p.se }))
        .collect();
    let has_pool = steps.iter().any(Option::is_some);
    let dynamic: Vec<DynamicFit> = if has_pool {
        maths::TAUS
            .iter()
            .map(|&tau| {
                let (post, ll) = maths::kalman(&steps, tau);
                let eras_out: Vec<DynamicEra> = post
                    .iter()
                    .zip(eras)
                    .map(|(p, e)| DynamicEra {
                        era_id: e.id.clone(),
                        mean: p.mean,
                        sd: p.sd(),
                        est: effects::natural(p.mean, fam),
                        lo: effects::natural(p.mean - Z95 * p.sd(), fam),
                        hi: effects::natural(p.mean + Z95 * p.sd(), fam),
                        p_favours: favours_p(pol, p.mean, p.sd()),
                    })
                    .collect();
                let p_fav = eras_out.last().map_or(0.5, |e| e.p_favours);
                DynamicFit {
                    tau,
                    log_likelihood: ll,
                    eras: eras_out,
                    p_favours: p_fav,
                    direction: if p_fav > maths::ROBUST_P {
                        Some(true)
                    } else if p_fav < 1.0 - maths::ROBUST_P {
                        Some(false)
                    } else {
                        None
                    },
                }
            })
            .collect()
    } else {
        Vec::new()
    };
    let robust = has_pool.then(|| {
        let d0 = dynamic.first().and_then(|d| d.direction);
        d0.is_some() && dynamic.iter().all(|d| d.direction == d0)
    });

    // Random effects and the predictive range over all primary rows with an SE.
    let all: Vec<Obs> = primary
        .iter()
        .filter_map(|r| Some(Obs { y: r.y?, se: r.se? }))
        .collect();
    let re = maths::dersimonian_laird(&all);
    let random_effects = re.map(|re| RandomEffects {
        k: re.k,
        est: Est::new(Obs { y: re.y, se: re.se }, fam),
        tau: re.tau2.sqrt(),
        i2: re.i2,
        p_favours: favours_p(pol, re.y, re.se),
    });
    let predictive = re.map(|re| {
        let sd = maths::predictive_sd(&re);
        Predictive {
            sd,
            est: effects::natural(re.y, fam),
            lo: effects::natural(re.y - Z95 * sd, fam),
            hi: effects::natural(re.y + Z95 * sd, fam),
            p_next_favours: favours_p(pol, re.y, sd),
        }
    });

    let flags = trajectory_flags(fam, pol, &cells);
    let label = pick_label(&flags);
    GroupResult {
        id: gid,
        family: fam,
        unit_key: key.1.clone(),
        polarity: pol,
        row_ids: primary.iter().map(|r| r.id.clone()).collect(),
        durability_row_ids: durable.iter().map(|r| r.id.clone()).collect(),
        cells,
        dynamic,
        robust,
        random_effects,
        predictive,
        label,
        flags,
    }
}

/// The first label of [`LABEL_PRECEDENCE`] among `flags`.
pub fn pick_label(flags: &[Label]) -> Label {
    LABEL_PRECEDENCE
        .iter()
        .copied()
        .find(|l| flags.contains(l))
        .unwrap_or(Label::Inconclusive)
}

/// A cell's Z when it counts as a shift (an earlier era has a pool).
fn shift_z(c: &EraCell) -> Option<f64> {
    c.z.filter(|_| c.after_earlier)
}

/// Step 7: every trajectory rule that holds for a group's era cells
/// (oldest first), in [`LABEL_PRECEDENCE`] order. Pure; deterministic.
pub fn trajectory_flags(fam: Family, pol: Polarity, cells: &[EraCell]) -> Vec<Label> {
    let unfav = |y: f64| pol.favours(y) == Some(false);
    let to_y = |v: f64| match fam {
        Family::Ratio => v.ln(),
        Family::Difference => v,
    };
    let data: Vec<&EraCell> = cells.iter().filter(|c| c.estimate.is_some()).collect();
    let pooled: Vec<&EraCell> = cells.iter().filter(|c| c.pooled.is_some()).collect();
    let mut f = Vec::new();
    if data.is_empty() {
        return vec![Label::NotAssessable];
    }
    // Early harm not seen later.
    if let Some(first) = pooled.first().and_then(|c| c.pooled)
        && first.significant()
        && unfav(first.y)
        && pooled
            .iter()
            .skip(1)
            .any(|c| c.pooled.is_some_and(|p| !p.significant()))
    {
        f.push(Label::EarlyHarmNotSeenLater);
    }
    // Direction flipped.
    if pooled.windows(2).any(|w| {
        let (a, b) = (w[0].pooled.expect("pooled"), w[1].pooled.expect("pooled"));
        a.y * b.y < 0.0 && shift_z(w[1]).is_some_and(|z| z.abs() >= Z_SHIFT)
    }) {
        f.push(Label::DirectionFlipped);
    }
    // Shifting, in the latest pooled era.
    if let Some(z) = pooled
        .last()
        .and_then(|c| shift_z(c))
        .filter(|z| z.abs() >= Z_SHIFT)
    {
        let toward = pol.favours(z) == Some(true);
        f.push(if toward {
            Label::ShiftingToward
        } else {
            Label::ShiftingAgainst
        });
    }
    // Gap narrowing.
    if fam == Family::Difference && data.len() >= 2 {
        let abs: Vec<f64> = data
            .iter()
            .filter_map(|c| c.estimate)
            .map(f64::abs)
            .collect();
        if abs.windows(2).all(|w| w[1] <= w[0]) && abs[abs.len() - 1] < abs[0] {
            f.push(Label::GapNarrowing);
        }
    }
    // Stable.
    let zs: Vec<f64> = pooled.iter().filter_map(|c| shift_z(c)).collect();
    let ys: Vec<f64> = data.iter().filter_map(|c| c.estimate).map(to_y).collect();
    let one_side = ys.iter().all(|&y| y > 0.0) || ys.iter().all(|&y| y < 0.0);
    if data.len() >= 2 && !zs.is_empty() && zs.iter().all(|z| z.abs() < Z_SHIFT) && one_side {
        f.push(Label::Stable);
    }
    // Persistent trade-off.
    let latest_dur = cells.iter().flat_map(|c| &c.durability).max_by_key(|d| {
        (
            d.knowledge_year.unwrap_or(i32::MIN),
            d.horizon_months.unwrap_or(0),
        )
    });
    let n_points = data.len() + cells.iter().map(|c| c.durability.len()).sum::<usize>();
    if n_points >= 2
        && ys.iter().all(|&y| unfav(y))
        && latest_dur.is_none_or(|d| d.favours == Some(false))
    {
        f.push(Label::PersistentTradeoff);
    }
    // Coverage.
    if cells.last().is_some_and(|c| c.estimate.is_none()) {
        f.push(Label::NoRecentData);
    }
    if data.len() == 1 {
        f.push(Label::OneEraOnly);
    }
    if f.is_empty() {
        f.push(Label::Inconclusive);
    }
    f
}

#[cfg(test)]
mod tests;
