//! Step 5 helpers: measure, arms and direction, scale and standard error,
//! counts derived from percentages, and outcome polarity.

use std::collections::BTreeMap;
use std::sync::LazyLock;

use regex::Regex;
use serde::{Deserialize, Serialize};

/// 1.96 × 2: the width of a 95% CI in standard errors.
pub const CI_WIDTH_SE: f64 = 3.92;

/// What a number measures.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Measure {
    Or,
    Rr,
    Hr,
    /// Mean (or median) difference, in the number's unit.
    Md,
    /// Risk difference in percentage points.
    Rd,
    /// One arm's percentage (half of a pair).
    Percent,
    /// One arm's count of patients or events (half of a pair).
    Count,
    /// One arm's value in some unit (minutes, points, cost; half of a pair).
    Value,
    /// Not an effect the table can use (an ICER, a cost-utility figure).
    Other,
}

/// The scale a measure is pooled on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Family {
    /// OR / RR / HR: log scale; the null is 1 (0 on the log scale).
    Ratio,
    /// MD / RD: linear; the null is 0.
    Difference,
}

impl Family {
    pub fn as_str(self) -> &'static str {
        match self {
            Family::Ratio => "ratio",
            Family::Difference => "diff",
        }
    }
}

impl Measure {
    pub fn family(self) -> Option<Family> {
        match self {
            Measure::Or | Measure::Rr | Measure::Hr => Some(Family::Ratio),
            Measure::Md | Measure::Rd => Some(Family::Difference),
            _ => None,
        }
    }

    /// One arm's value: two of them make a pair.
    pub fn is_arm_level(self) -> bool {
        matches!(self, Measure::Percent | Measure::Count | Measure::Value)
    }
}

static OTHER_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\bICER\b|cost[- ]utility|cost[- ]effectiveness|per\s+QALY|/\s*QALY")
        .expect("other")
});
static OR_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\ba?OR\b|(?i:\bodds\s+ratio)").expect("or"));
static RR_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\b(?:a?RR|IRR)\b|(?i:\b(?:risk|rate)\s+ratio|\brelative\s+risk)").expect("rr")
});
static HR_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\ba?HR\b|(?i:\bhazard\s+ratio)").expect("hr"));
static DIFF_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\bdifference\b|\b(?:W|S)?MD\b|\bRD\b|\bARR\b|\bdiff\b").expect("diff")
});
static COUNT_UNIT_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^(?:patients?|participants?|people|cases?|events?|n|subjects?)$")
        .expect("count")
});
static PP_UNIT_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^(?:pp|percentage\s+points?|%\s*points?)$").expect("pp"));

fn unit_is_percent(unit: &str) -> bool {
    let u = unit.trim();
    u == "%" || u.eq_ignore_ascii_case("percent") || PP_UNIT_RE.is_match(u)
}

/// What a number measures, from its label prefix, unit and arm text.
pub fn classify(label: &str, unit: Option<&str>, arm: Option<&str>) -> Measure {
    let unit = unit.unwrap_or("").trim();
    let arm = arm.unwrap_or("");
    if OTHER_RE.is_match(label) || OTHER_RE.is_match(unit) {
        return Measure::Other;
    }
    let u = unit.to_ascii_uppercase();
    if u == "OR" || u == "AOR" || OR_RE.is_match(label) {
        return Measure::Or;
    }
    if u == "RR" || u == "IRR" || RR_RE.is_match(label) {
        return Measure::Rr;
    }
    if u == "HR" || HR_RE.is_match(label) {
        return Measure::Hr;
    }
    let two_arms = split_vs(arm).is_some();
    if DIFF_RE.is_match(label) || PP_UNIT_RE.is_match(unit) || (two_arms && !unit.is_empty()) {
        return if unit_is_percent(unit) {
            Measure::Rd
        } else {
            Measure::Md
        };
    }
    if unit_is_percent(unit) {
        Measure::Percent
    } else if COUNT_UNIT_RE.is_match(unit) {
        Measure::Count
    } else {
        Measure::Value
    }
}

/// A short key for a unit ("min", "pp", "usd"), for grouping rows.
pub fn unit_key(measure: Measure, unit: Option<&str>) -> String {
    match measure {
        Measure::Rd | Measure::Percent => "pp".into(),
        Measure::Count => "count".into(),
        Measure::Or | Measure::Rr | Measure::Hr => String::new(),
        _ => {
            let u: String = unit
                .unwrap_or("")
                .to_lowercase()
                .chars()
                .map(|c| if c.is_alphanumeric() { c } else { '-' })
                .collect();
            let u = u
                .split('-')
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join("-");
            match u.as_str() {
                "" => "unit".into(),
                "mins" | "minutes" | "minute" => "min".into(),
                "day" => "days".into(),
                _ => u,
            }
        }
    }
}

// ---- Arms ---------------------------------------------------------------

static PAREN_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\([^)]*\)").expect("paren"));
static VS_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\s+(?:vs\.?|versus|v\.|minus|compared\s+(?:with|to))\s+").expect("vs")
});

/// "X vs Y" (or "X minus Y", "X versus Y") as (X, Y), brackets removed.
pub fn split_vs(arm: &str) -> Option<(String, String)> {
    let a = PAREN_RE.replace_all(arm, " ");
    let m = VS_RE.find(&a)?;
    let x = a[..m.start()].trim();
    let y = a[m.end()..].trim();
    (!x.is_empty() && !y.is_empty()).then(|| (x.to_string(), y.to_string()))
}

static R_ARM_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(?:r-?(?:tapp|tep|ihr|ventral)|ra-|robot)").expect("r arm"));
static L_ARM_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(?:l-?(?:tapp|tep)\b|lap|tep\b|tapp\b)").expect("l arm"));

/// A canonical token for an arm ("rTAPP", "R-TAPP" and "robotic" are all
/// "robotic"; "LHM plus Dor" is "lhm"), for deciding direction.
pub fn canon_arm(arm: &str) -> String {
    let s = PAREN_RE.replace_all(arm, " ").trim().to_lowercase();
    if s.contains("pneumatic dilat") {
        return "pd".into();
    }
    if s.contains("heller") {
        return "lhm".into();
    }
    if R_ARM_RE.is_match(&s) {
        return "robotic".into();
    }
    if L_ARM_RE.is_match(&s) {
        return "laparoscopic".into();
    }
    s.split(|c: char| !c.is_alphanumeric())
        .find(|t| !t.is_empty())
        .unwrap_or("")
        .to_string()
}

/// How a two-arm number was oriented.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Orientation {
    /// Intervention first, as written.
    AsWritten,
    /// Comparator first as written; turned round.
    Flipped,
    /// The run says nothing about which arm is the intervention: kept as
    /// written.
    Unknown,
}

/// Which arm is the intervention, decided from the run's "X vs Y" texts.
#[derive(Debug, Clone, Default)]
pub struct ArmOrder {
    /// (first, second) → how often written that way.
    votes: BTreeMap<(String, String), u32>,
    /// The most common pair (ties: the first seen).
    pub dominant: Option<(String, String)>,
}

impl ArmOrder {
    /// Counts every "X vs Y" arm text of the run.
    pub fn from_arms<'a>(arms: impl IntoIterator<Item = &'a str>) -> ArmOrder {
        let mut votes: BTreeMap<(String, String), u32> = BTreeMap::new();
        let mut seen: Vec<(String, String)> = Vec::new();
        for arm in arms {
            if let Some((x, y)) = split_vs(arm) {
                let (x, y) = (canon_arm(&x), canon_arm(&y));
                if x.is_empty() || y.is_empty() || x == y {
                    continue;
                }
                let k = (x, y);
                if !seen.contains(&k) {
                    seen.push(k.clone());
                }
                *votes.entry(k).or_default() += 1;
            }
        }
        let mut dominant: Option<((String, String), u32)> = None;
        for k in seen {
            let v = votes[&k];
            if dominant.as_ref().is_none_or(|(_, best)| v > *best) {
                dominant = Some((k, v));
            }
        }
        ArmOrder {
            votes,
            dominant: dominant.map(|(k, _)| k),
        }
    }

    fn v(&self, a: &str, b: &str) -> u32 {
        self.votes
            .get(&(a.to_string(), b.to_string()))
            .copied()
            .unwrap_or(0)
    }

    /// Whether arm `a` (written first) and arm `b` are in intervention
    /// order: the majority of the run's texts for this pair, else whether
    /// one of them is the dominant pair's intervention or comparator.
    pub fn orient(&self, a: &str, b: &str) -> Orientation {
        let (a, b) = (canon_arm(a), canon_arm(b));
        if a.is_empty() || b.is_empty() || a == b {
            return Orientation::Unknown;
        }
        let (ab, ba) = (self.v(&a, &b), self.v(&b, &a));
        if ab != ba {
            return if ab > ba {
                Orientation::AsWritten
            } else {
                Orientation::Flipped
            };
        }
        if let Some((x, y)) = &self.dominant {
            if &a == x || &b == y {
                return Orientation::AsWritten;
            }
            if &b == x || &a == y {
                return Orientation::Flipped;
            }
        }
        Orientation::Unknown
    }
}

// ---- Effect, flip, scale ------------------------------------------------

/// A point estimate with its 95% CI, on the natural scale.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Effect {
    pub value: f64,
    pub lo: Option<f64>,
    pub hi: Option<f64>,
}

/// Turns an effect round (comparator first → intervention first): ratios
/// are inverted (the CI bounds swap and invert); differences are negated
/// (the bounds swap and negate). Flipping twice gives the effect back.
pub fn flip(e: Effect, family: Family) -> Effect {
    match family {
        Family::Ratio => Effect {
            value: 1.0 / e.value,
            lo: e.hi.map(|h| 1.0 / h),
            hi: e.lo.map(|l| 1.0 / l),
        },
        Family::Difference => Effect {
            value: -e.value,
            lo: e.hi.map(|h| -h),
            hi: e.lo.map(|l| -l),
        },
    }
}

/// An effect on its pooling scale.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Scaled {
    /// ln(value) for ratios, the value for differences.
    pub y: f64,
    /// From the CI; `None` when there is no usable CI.
    pub se: Option<f64>,
}

/// An effect on its pooling scale, with a plain reason for anything left
/// out. `Err`: the value itself is unusable (the row is skipped). A bad CI
/// keeps the point and drops the standard error.
pub fn to_scale(e: Effect, family: Family) -> Result<(Scaled, Option<String>), String> {
    let ok = |x: f64| x.is_finite() && (family == Family::Difference || x > 0.0);
    if !ok(e.value) {
        return Err(match family {
            Family::Ratio => format!("ratio {} is not a positive number", e.value),
            Family::Difference => format!("difference {} is not a number", e.value),
        });
    }
    let tr = |x: f64| if family == Family::Ratio { x.ln() } else { x };
    let y = tr(e.value);
    let (lo, hi) = match (e.lo, e.hi) {
        (Some(lo), Some(hi)) => (lo, hi),
        (None, None) => return Ok((Scaled { y, se: None }, None)),
        _ => {
            return Ok((
                Scaled { y, se: None },
                Some("only one CI bound; no standard error".into()),
            ));
        }
    };
    if !ok(lo) || !ok(hi) {
        return Ok((
            Scaled { y, se: None },
            Some(format!(
                "CI {lo} to {hi} is not usable on this scale; no standard error"
            )),
        ));
    }
    if lo >= hi {
        return Ok((
            Scaled { y, se: None },
            Some(format!("CI {lo} to {hi} has no width; no standard error")),
        ));
    }
    let slack = 0.01 * (hi - lo);
    if e.value < lo - slack || e.value > hi + slack {
        return Ok((
            Scaled { y, se: None },
            Some(format!(
                "value {} lies outside its CI {lo} to {hi}; no standard error",
                e.value
            )),
        ));
    }
    let se = (tr(hi) - tr(lo)) / CI_WIDTH_SE;
    Ok((Scaled { y, se: Some(se) }, None))
}

/// The natural-scale value of `y` on `family`'s scale.
pub fn natural(y: f64, family: Family) -> f64 {
    match family {
        Family::Ratio => y.exp(),
        Family::Difference => y,
    }
}

/// Log odds ratio and SE from events/totals in two arms. A zero cell adds
/// 0.5 to every cell (and says so). `None` when the arms are unusable.
pub fn log_or_from_counts(ea: f64, na: f64, eb: f64, nb: f64) -> Option<(f64, f64, bool)> {
    let fine = |e: f64, n: f64| e.is_finite() && n.is_finite() && e >= 0.0 && n > 0.0 && e <= n;
    if !fine(ea, na) || !fine(eb, nb) {
        return None;
    }
    let (mut a, mut b, mut c, mut d) = (ea, na - ea, eb, nb - eb);
    let corrected = [a, b, c, d].contains(&0.0);
    if corrected {
        a += 0.5;
        b += 0.5;
        c += 0.5;
        d += 0.5;
    }
    let y = ((a * d) / (b * c)).ln();
    let se = (1.0 / a + 1.0 / b + 1.0 / c + 1.0 / d).sqrt();
    (y.is_finite() && se.is_finite()).then_some((y, se, corrected))
}

/// Events from a percentage of a known arm size (rounded).
pub fn events_from_percent(pct: f64, n: f64) -> Option<f64> {
    (pct.is_finite() && (0.0..=100.0).contains(&pct) && n.is_finite() && n > 0.0)
        .then(|| (pct / 100.0 * n).round())
}

// ---- Polarity -----------------------------------------------------------

/// Whether a higher value of the outcome is worse or better for patients.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Polarity {
    HigherWorse,
    HigherBetter,
}

impl Polarity {
    pub fn as_str(self) -> &'static str {
        match self {
            Polarity::HigherWorse => "higher_worse",
            Polarity::HigherBetter => "higher_better",
        }
    }

    /// Whether `y` (intervention vs comparator, on the pooling scale: 0 is
    /// the null) favours the intervention. `None` at exactly the null.
    pub fn favours(self, y: f64) -> Option<bool> {
        if y == 0.0 || !y.is_finite() {
            return None;
        }
        Some(match self {
            Polarity::HigherWorse => y < 0.0,
            Polarity::HigherBetter => y > 0.0,
        })
    }
}

static BENEFIT_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\bsuccess|\brelief\b|\bremission\b|quality\s+of\s+life|\bqol\b|eq-5d|sf-36|\breturn(?:ed)?\s+to\b|\bresum|\bdischarge|\bsurvival\b|\bsatisfaction\b|\befficacy\b|\bcure\b|\bpatency\b")
        .expect("benefit")
});
/// Harms named alongside a benefit word ("PPI use in remission") that
/// still decide the direction.
static STRONG_HARM_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\breflux|o?esophagitis|\bacid\b|\bger\b|\bgerd|\bppi\b|\bproton\s+pump|\bcomplication|\badverse|\bmortality|\bdeath|\binjur")
        .expect("strong harm")
});
static HARM_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\bfail|complication|\binjur|\bconversion|\bbailout|\bsubtotal|\breflux|o?esophagitis|\bacid\b|\bger\b|\bgerd|\bppi\b|\bpain\b|\btime\b|\bcost|\bstay\b|hospitali|\bmortality|\bdeath|\brecurren|\badverse|\bperforation|\btear\b|\btransfusion|\breadmission|\bleak|\binfection|\bworkload|\btlx\b|\bfrustration|\beckardt|\bharm")
        .expect("harm")
});

/// The polarity of a number: from its label (benefit words first: "success
/// after failed LHM" is a success), else the outcome's name; higher is
/// worse by default (most surgical outcomes are harms). The table first
/// tries the same study's other numbers in the outcome before the outcome
/// name (see `build`).
pub fn polarity(label: &str, outcome: &str) -> Polarity {
    stated_polarity(label)
        .or_else(|| stated_polarity(outcome))
        .unwrap_or(Polarity::HigherWorse)
}

/// The polarity a text names, if any: strong harms (reflux, PPI use,
/// complications, death) first, then benefit words ("success after failed
/// LHM" is a success), then other harms.
pub fn stated_polarity(text: &str) -> Option<Polarity> {
    if STRONG_HARM_RE.is_match(text) {
        Some(Polarity::HigherWorse)
    } else if BENEFIT_RE.is_match(text) {
        Some(Polarity::HigherBetter)
    } else if HARM_RE.is_match(text) {
        Some(Polarity::HigherWorse)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn measures() {
        assert_eq!(
            classify(
                "OR conversion, HCUP",
                Some("OR"),
                Some("robotic vs laparoscopic")
            ),
            Measure::Or
        );
        assert_eq!(classify("Adjusted OR bailout", None, None), Measure::Or);
        assert_eq!(classify("Chronic pain RR", Some("RR"), None), Measure::Rr);
        assert_eq!(classify("RR of treatment failure", None, None), Measure::Rr);
        assert_eq!(classify("HR death", None, None), Measure::Hr);
        assert_eq!(
            classify("Pooled MD operative time", Some("min"), None),
            Measure::Md
        );
        assert_eq!(
            classify(
                "Operating time difference (ROLAIS)",
                Some("min"),
                Some("R-TAPP vs L-TAPP")
            ),
            Measure::Md
        );
        assert_eq!(
            classify(
                "Difference in 2-year success",
                Some("percentage points"),
                Some("POEM minus LHM")
            ),
            Measure::Rd
        );
        assert_eq!(
            classify(
                "Adjusted difference at 5 years",
                Some("%"),
                Some("POEM minus PD")
            ),
            Measure::Rd
        );
        assert_eq!(
            classify(
                "VAS mean difference at 3 h",
                Some("points"),
                Some("robotic minus laparoscopic")
            ),
            Measure::Md
        );
        assert_eq!(
            classify("Bailout rate", Some("%"), Some("robotic")),
            Measure::Percent
        );
        assert_eq!(
            classify(
                "Postoperative complications",
                Some("patients"),
                Some("rTAPP")
            ),
            Measure::Count
        );
        assert_eq!(
            classify("Median operative time", Some("min"), Some("robotic")),
            Measure::Value
        );
        assert_eq!(
            classify("ICER", Some("EUR/QALY"), Some("r-TAPP vs l-TAPP")),
            Measure::Other
        );
        // "or" in plain words is not an odds ratio.
        assert_eq!(
            classify("pain or discomfort", Some("%"), Some("robotic")),
            Measure::Percent
        );
    }

    #[test]
    fn arms_and_orientation() {
        assert_eq!(
            split_vs("robotic vs laparoscopic (mostly observational)"),
            Some(("robotic".into(), "laparoscopic".into()))
        );
        assert_eq!(
            split_vs("POEM minus LHM"),
            Some(("POEM".into(), "LHM".into()))
        );
        assert_eq!(split_vs("robotic"), None);
        assert_eq!(canon_arm("rTAPP"), "robotic");
        assert_eq!(canon_arm("R-TAPP"), "robotic");
        assert_eq!(canon_arm("L-TAPP"), "laparoscopic");
        assert_eq!(canon_arm("TEP"), "laparoscopic");
        assert_eq!(canon_arm("LHM plus Dor"), "lhm");
        assert_eq!(canon_arm("LHM+PF"), "lhm");
        assert_eq!(canon_arm("Pneumatic dilation"), "pd");
        let o = ArmOrder::from_arms([
            "robotic vs laparoscopic",
            "robotic vs laparoscopic",
            "r-TAPP vs l-TAPP",
            "laparoscopic vs robotic",
            "POEM vs PD",
        ]);
        assert_eq!(o.dominant, Some(("robotic".into(), "laparoscopic".into())));
        assert_eq!(o.orient("laparoscopic", "robotic"), Orientation::Flipped);
        assert_eq!(o.orient("rTAPP", "TEP"), Orientation::AsWritten);
        assert_eq!(o.orient("PD", "POEM"), Orientation::Flipped);
        // Neither arm known: as written, flagged.
        assert_eq!(o.orient("x", "y"), Orientation::Unknown);
        // One arm is the dominant intervention.
        assert_eq!(o.orient("open", "robotic"), Orientation::Flipped);
    }

    #[test]
    fn flip_is_idempotent() {
        let r = Effect {
            value: 1.079,
            lo: Some(0.682),
            hi: Some(1.707),
        };
        let f = flip(r, Family::Ratio);
        assert!((f.value - 1.0 / 1.079).abs() < 1e-12);
        assert!((f.lo.unwrap() - 1.0 / 1.707).abs() < 1e-12);
        assert!((f.hi.unwrap() - 1.0 / 0.682).abs() < 1e-12);
        let back = flip(f, Family::Ratio);
        assert!((back.value - r.value).abs() < 1e-12);
        assert!((back.lo.unwrap() - 0.682).abs() < 1e-12);
        assert!((back.hi.unwrap() - 1.707).abs() < 1e-12);
        let d = Effect {
            value: -13.4,
            lo: Some(-20.0),
            hi: None,
        };
        let fd = flip(d, Family::Difference);
        assert_eq!(
            fd,
            Effect {
                value: 13.4,
                lo: None,
                hi: Some(20.0)
            }
        );
        assert_eq!(flip(fd, Family::Difference), d);
        // The SE is the same either way round.
        let (a, _) = to_scale(r, Family::Ratio).unwrap();
        let (b, _) = to_scale(f, Family::Ratio).unwrap();
        assert!((a.y + b.y).abs() < 1e-12);
        assert!((a.se.unwrap() - b.se.unwrap()).abs() < 1e-12);
    }

    #[test]
    fn scale_and_guards() {
        let (s, n) = to_scale(
            Effect {
                value: 1.61,
                lo: Some(1.28),
                hi: Some(2.03),
            },
            Family::Ratio,
        )
        .unwrap();
        assert!((s.y - 1.61f64.ln()).abs() < 1e-12);
        assert!((s.se.unwrap() - (2.03f64.ln() - 1.28f64.ln()) / 3.92).abs() < 1e-12);
        assert!(n.is_none());
        let (s, _) = to_scale(
            Effect {
                value: 0.28,
                lo: Some(-0.34),
                hi: Some(0.9),
            },
            Family::Difference,
        )
        .unwrap();
        assert!((s.se.unwrap() - 1.24 / 3.92).abs() < 1e-12);
        // Bad values are skipped, never panic.
        assert!(
            to_scale(
                Effect {
                    value: 0.0,
                    lo: None,
                    hi: None
                },
                Family::Ratio
            )
            .is_err()
        );
        assert!(
            to_scale(
                Effect {
                    value: -1.0,
                    lo: None,
                    hi: None
                },
                Family::Ratio
            )
            .is_err()
        );
        assert!(
            to_scale(
                Effect {
                    value: f64::NAN,
                    lo: None,
                    hi: None
                },
                Family::Difference
            )
            .is_err()
        );
        // Bad CIs keep the point, drop the SE, and say why.
        for (lo, hi) in [
            (0.0, 2.0),
            (-1.0, 2.0),
            (f64::NAN, 2.0),
            (2.0, 2.0),
            (2.0, 1.0),
            (3.0, 4.0),
        ] {
            let (s, n) = to_scale(
                Effect {
                    value: 1.5,
                    lo: Some(lo),
                    hi: Some(hi),
                },
                Family::Ratio,
            )
            .unwrap();
            assert!(s.se.is_none() && n.is_some(), "{lo} {hi}");
        }
        let (s, n) = to_scale(
            Effect {
                value: 1.5,
                lo: Some(1.0),
                hi: None,
            },
            Family::Ratio,
        )
        .unwrap();
        assert!(s.se.is_none() && n.is_some());
    }

    #[test]
    fn counts() {
        let (y, se, c) = log_or_from_counts(11.0, 107.0, 27.0, 215.0).unwrap();
        let expect = ((11.0 * 188.0) / (96.0 * 27.0f64)).ln();
        assert!((y - expect).abs() < 1e-12 && se > 0.0 && !c);
        let (_, _, c) = log_or_from_counts(0.0, 50.0, 1.0, 50.0).unwrap();
        assert!(c);
        assert!(log_or_from_counts(5.0, 0.0, 1.0, 10.0).is_none());
        assert!(log_or_from_counts(11.0, 10.0, 1.0, 10.0).is_none());
        assert_eq!(events_from_percent(10.3, 107.0), Some(11.0));
        assert_eq!(events_from_percent(120.0, 10.0), None);
    }

    #[test]
    fn polarities() {
        assert_eq!(
            polarity("Clinical success at 2 years", "Symptom relief"),
            Polarity::HigherBetter
        );
        assert_eq!(
            polarity("Success after failed LHM, 1 year", "Place of PD"),
            Polarity::HigherBetter
        );
        assert_eq!(
            polarity("Reflux oesophagitis at 24 months", "x"),
            Polarity::HigherWorse
        );
        assert_eq!(
            polarity("Difference in mean Eckardt score", "Symptom relief"),
            Polarity::HigherWorse
        );
        assert_eq!(
            polarity("Rescue analgesia", "Postoperative and chronic pain"),
            Polarity::HigherWorse
        );
        assert_eq!(
            polarity(
                "Resumed normal activity at 14 days",
                "Return to normal activity"
            ),
            Polarity::HigherBetter
        );
        assert_eq!(
            polarity("PPI use in remission", "Place of PD"),
            Polarity::HigherWorse
        );
        assert_eq!(
            polarity("Something", "Something else"),
            Polarity::HigherWorse
        );
        assert_eq!(Polarity::HigherWorse.favours(-0.1), Some(true));
        assert_eq!(Polarity::HigherBetter.favours(-0.1), Some(false));
        assert_eq!(Polarity::HigherBetter.favours(0.0), None);
    }
}
