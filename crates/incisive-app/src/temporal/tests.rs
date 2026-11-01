//! Tests of the whole build: hand-built inputs that mirror the three
//! prototype tables, the trajectory rules, and the guards. The golden tests
//! on the owner's stored answers are in `tests/temporal.rs`.

use super::input::{InGuideline, InNumber, InOutcome, InOverlap, InReference, InSkeptic, InStudy};
use super::*;

// ---- Builders -------------------------------------------------------------

fn study(pmid: &str, label: &str, design: &str, setting: &str) -> InStudy {
    InStudy {
        pmid: pmid.into(),
        label: label.into(),
        design: design.into(),
        setting: setting.into(),
        ..InStudy::default()
    }
}

fn reference(pmid: &str, year: i32) -> InReference {
    InReference {
        n: None,
        pmid: pmid.into(),
        year: Some(year),
        found_by_skeptic: false,
    }
}

fn ratio(label: &str, v: f64, lo: f64, hi: f64, arm: &str, pmid: &str) -> InNumber {
    InNumber {
        label: label.into(),
        value: Some(v),
        unit: None,
        arm: Some(arm.into()),
        ci_low: Some(lo),
        ci_high: Some(hi),
        p: None,
        pmid: pmid.into(),
    }
}

fn arm_value(label: &str, v: f64, unit: &str, arm: &str, pmid: &str) -> InNumber {
    InNumber {
        label: label.into(),
        value: Some(v),
        unit: Some(unit.into()),
        arm: Some(arm.into()),
        ci_low: None,
        ci_high: None,
        p: None,
        pmid: pmid.into(),
    }
}

fn outcome(name: &str, numbers: Vec<InNumber>) -> InOutcome {
    InOutcome {
        outcome: name.into(),
        numbers,
        ..InOutcome::default()
    }
}

fn input(studies: Vec<InStudy>, refs: Vec<InReference>, outcomes: Vec<InOutcome>) -> TemporalInput {
    TemporalInput {
        question: "Q".into(),
        created_at: Some("2026-10-31T12:00:00Z".into()),
        outcomes,
        studies,
        guidelines: vec![],
        references: refs,
        skeptic: None,
    }
}

fn outcome_named<'a>(t: &'a TemporalTable, name: &str) -> &'a OutcomeResult {
    t.outcomes.iter().find(|o| o.outcome == name).unwrap()
}

fn study_by<'a>(t: &'a TemporalTable, pmid: &str) -> &'a StudyEntry {
    t.studies.iter().find(|s| s.pmid == pmid).unwrap()
}

const RL: &str = "robotic vs laparoscopic";

// ---- Prototype 1: cholecystitis -------------------------------------------

fn cholecystitis() -> TemporalInput {
    let studies = vec![
        study(
            "ma1",
            "Acute cholecystitis MA",
            "Systematic review and meta-analysis of 7 cohorts",
            "multiple centres",
        ),
        study(
            "hcup",
            "HCUP NY/FL",
            "Retrospective cohort",
            "New York and Florida, 2012-2021",
        ),
        study(
            "robocop",
            "ROBOCOP",
            "Retrospective IPTW cohort",
            "US hospitals, 2020-2022",
        ),
        study(
            "medicare",
            "Medicare cholecystitis subgroup",
            "Claims cohort",
            "US Medicare, 2010-2019",
        ),
        study(
            "claims",
            "US commercial claims",
            "Matched claims cohort",
            "US claims, 2016-2021",
        ),
    ];
    let refs = vec![
        reference("ma1", 2026),
        reference("hcup", 2026),
        reference("robocop", 2024),
        reference("medicare", 2023),
        reference("claims", 2025),
    ];
    let outcomes = vec![
        outcome(
            "Conversion",
            vec![
                ratio("RR conversion, MA", 0.61, 0.50, 0.75, RL, "ma1"),
                ratio("OR conversion, HCUP", 1.61, 1.28, 2.03, RL, "hcup"),
                ratio("OR conversion, IPTW", 0.68, 0.49, 0.93, RL, "robocop"),
            ],
        ),
        outcome(
            "Subtotal cholecystectomy",
            vec![
                ratio("OR subtotal, HCUP", 2.02, 1.68, 2.43, RL, "hcup"),
                ratio("OR subtotal, IPTW", 1.64, 1.03, 2.60, RL, "robocop"),
            ],
        ),
        outcome(
            "Bile duct injury",
            vec![
                ratio("RR bile duct injury, MA", 0.97, 0.77, 1.21, RL, "ma1"),
                ratio(
                    "RR bile duct injury, Medicare",
                    2.29,
                    1.79,
                    2.79,
                    RL,
                    "medicare",
                ),
                ratio(
                    "OR bile duct injury, claims",
                    0.93,
                    0.73,
                    1.18,
                    RL,
                    "claims",
                ),
            ],
        ),
    ];
    input(studies, refs, outcomes)
}

#[test]
fn prototype_cholecystitis() {
    let t = build(&cholecystitis());
    assert_eq!(t.version, TEMPORAL_VERSION);
    assert_eq!(t.run_year, Some(2026));
    let eras: Vec<&str> = t.eras.iter().map(|e| e.label.as_str()).collect();
    assert_eq!(eras, ["2011–15", "2016–20", "2021–26"]);
    assert_eq!(
        study_by(&t, "medicare").era_id.as_deref(),
        Some("e-2011-2015")
    );
    assert_eq!(study_by(&t, "hcup").era_id.as_deref(), Some("e-2016-2020"));
    assert_eq!(
        study_by(&t, "robocop").era_id.as_deref(),
        Some("e-2021-2026")
    );
    assert!(!study_by(&t, "hcup").assumed);
    assert_eq!(study_by(&t, "ma1").kind, StudyKind::Marker);
    assert_eq!(t.markers.len(), 1);
    assert_eq!(t.markers[0].row_ids.len(), 2);

    let conv = outcome_named(&t, "Conversion");
    assert_eq!(conv.label, Label::DirectionFlipped);
    assert_eq!(
        conv.marker_row_ids.len(),
        1,
        "the MA is a marker, not pooled"
    );
    let g = &conv.groups[0];
    assert_eq!(g.row_ids.len(), 2);
    let late = g.cells.iter().find(|c| c.era_id == "e-2021-2026").unwrap();
    assert!(late.z.unwrap() < -Z_SHIFT);
    assert_eq!(g.robust, Some(false));
    assert_eq!(g.dynamic.len(), maths::TAUS.len());
    let re = g.random_effects.as_ref().unwrap();
    assert_eq!(re.k, 2);
    assert!(re.i2 > 0.9);
    let pred = g.predictive.as_ref().unwrap();
    assert!(pred.lo < re.est.lo && pred.hi > re.est.hi);

    let sub = outcome_named(&t, "Subtotal cholecystectomy");
    assert_eq!(sub.label, Label::Stable);
    assert!(
        sub.groups[0].robust == Some(true),
        "unfavourable at every τ"
    );

    let bdi = outcome_named(&t, "Bile duct injury");
    assert_eq!(bdi.label, Label::EarlyHarmNotSeenLater);
    assert!(bdi.flags.contains(&Label::NoRecentData));
}

// ---- Prototype 2: hernia --------------------------------------------------

fn hernia() -> TemporalInput {
    let mut direct = study(
        "direct",
        "DIRECT 2025",
        "Secondary analysis of RCT",
        "Denmark",
    );
    direct.appraisal = "Secondary analysis of ROLAIS (same patients, not independent)".into();
    let studies = vec![
        study("rival", "RIVAL 2020", "Multicentre pilot RCT", "6 US sites"),
        study(
            "rival2",
            "RIVAL 2-year",
            "1- and 2-year follow-up of RIVAL",
            "Six sites",
        ),
        study("roger", "ROGER 2025", "Single-centre RCT", "Basel"),
        study(
            "rolais",
            "ROLAIS 2025",
            "Single-centre open-label RCT",
            "Single centre",
        ),
        direct,
        study("voltaire", "VOLTAIRE 2025", "RCT", "Not reported"),
        study(
            "ma",
            "RCT meta-analysis 2026",
            "Meta-analysis of 7 RCTs",
            "RCTs",
        ),
    ];
    let refs = vec![
        reference("rival", 2020),
        reference("rival2", 2023),
        reference("roger", 2025),
        reference("rolais", 2025),
        reference("direct", 2025),
        reference("voltaire", 2025),
        reference("ma", 2026),
    ];
    let md = |v: f64, pmid: &str| {
        arm_value(
            "Operative time difference",
            v,
            "min",
            "robotic minus laparoscopic",
            pmid,
        )
    };
    let outcomes = vec![outcome(
        "Operative time",
        vec![
            md(35.0, "rival"),
            md(16.1, "roger"),
            md(33.2, "voltaire"),
            md(-13.4, "rolais"),
            md(-9.0, "direct"),
            ratio(
                "Pooled MD operative time",
                18.5,
                -8.4,
                45.5,
                "r-TAPP vs l-TAPP",
                "ma",
            ),
        ],
    )];
    input(studies, refs, outcomes)
}

#[test]
fn prototype_hernia() {
    let t = build(&hernia());
    for s in t.studies.iter().filter(|s| s.kind == StudyKind::Primary) {
        assert!(s.assumed, "{} should have assumed years", s.label);
    }
    assert_eq!(
        study_by(&t, "rival").years_from,
        YearsFrom::PublicationMinusTwo
    );
    assert_eq!(study_by(&t, "rival").treatment_start, Some(2018));
    let rival2 = study_by(&t, "rival2");
    assert_eq!(rival2.parent_id.as_deref(), Some("s-rival"));
    assert_eq!(rival2.years_from, YearsFrom::Parent);
    assert_eq!(rival2.era_id.as_deref(), Some("e-2016-2020"));
    assert_eq!(rival2.horizon.as_ref().unwrap().months, 24);
    assert_eq!(rival2.knowledge_year, Some(2023));
    assert_eq!(
        study_by(&t, "direct").parent_id.as_deref(),
        Some("s-rolais")
    );
    assert_eq!(study_by(&t, "rolais").child_ids, vec!["s-direct"]);
    let eras: Vec<&str> = t.eras.iter().map(|e| e.label.as_str()).collect();
    assert_eq!(eras, ["2016–20", "2021–26"]);
    // DIRECT is not a second study in its era.
    assert_eq!(t.eras[1].n_studies, 3);

    let ot = outcome_named(&t, "Operative time");
    assert_eq!(ot.label, Label::GapNarrowing);
    let g = &ot.groups[0];
    assert_eq!(g.family, Family::Difference);
    let direct_row = t.rows.iter().find(|r| r.study_id == "s-direct").unwrap();
    assert_eq!(direct_row.role, RowRole::Durability);
    assert!(!g.row_ids.contains(&direct_row.id));
    let late = &g.cells[1];
    assert!((late.estimate.unwrap() - (16.1 + 33.2 - 13.4) / 3.0).abs() < 1e-9);
    assert!(late.pooled.is_none(), "no CI: points, never pooled");
    // The meta-analysis row is a marker.
    assert_eq!(ot.marker_row_ids.len(), 1);
}

// ---- Prototype 3: achalasia -----------------------------------------------

fn achalasia() -> TemporalInput {
    let studies = vec![
        study(
            "eat",
            "European Achalasia Trial 2011",
            "Multicentre RCT",
            "European centres",
        ),
        study(
            "eat10",
            "European Achalasia Trial 10-year",
            "10-year follow-up of the European Achalasia Trial",
            "European multicentre",
        ),
        study(
            "werner",
            "Werner 2019",
            "Multicentre non-inferiority RCT",
            "European multicentre",
        ),
        study(
            "werner5",
            "5-year follow-up 2025",
            "5-year follow-up of an RCT",
            "8 centres",
        ),
        study("brazil", "Brazil 2022", "Single-centre RCT", "Brazil"),
        study("nma", "NMA 2021", "Network meta-analysis of 9 RCTs", "RCTs"),
    ];
    let refs = vec![
        reference("eat", 2011),
        reference("eat10", 2024),
        reference("werner", 2019),
        reference("werner5", 2025),
        reference("brazil", 2022),
        reference("nma", 2021),
    ];
    let mut t = input(
        studies,
        refs,
        vec![
            outcome(
                "Reflux",
                vec![
                    arm_value(
                        "Reflux oesophagitis at 24 months",
                        44.0,
                        "%",
                        "POEM",
                        "werner",
                    ),
                    arm_value(
                        "Reflux oesophagitis at 24 months",
                        29.0,
                        "%",
                        "LHM plus Dor",
                        "werner",
                    ),
                    arm_value(
                        "Reflux oesophagitis at 5 years",
                        41.0,
                        "%",
                        "POEM",
                        "werner5",
                    ),
                    arm_value(
                        "Reflux oesophagitis at 5 years",
                        31.0,
                        "%",
                        "LHM plus Dor",
                        "werner5",
                    ),
                    arm_value("GER by GerdQ", 64.6, "%", "POEM", "brazil"),
                    arm_value("GER by GerdQ", 11.1, "%", "LM-PF", "brazil"),
                    ratio("RR oesophagitis", 3.1, 2.17, 4.41, "POEM vs LHM+PF", "nma"),
                ],
            ),
            outcome(
                "Success, PD vs LHM",
                vec![
                    arm_value("Success at 2 years", 86.0, "%", "PD", "eat"),
                    arm_value("Success at 2 years", 90.0, "%", "LHM", "eat"),
                    arm_value("Success at 10 years", 74.0, "%", "PD", "eat10"),
                    arm_value("Success at 10 years", 74.0, "%", "LHM", "eat10"),
                ],
            ),
        ],
    );
    t.skeptic = Some(InSkeptic {
        overlaps: vec![InOverlap {
            pmids: vec!["werner".into(), "werner5".into()],
            note: "These are the same multicentre trial at 2 and 5 years.".into(),
        }],
    });
    t.guidelines = vec![InGuideline {
        pmid: None,
        body: "SAGES 2024 update".into(),
        year: Some(2025),
    }];
    t
}

#[test]
fn prototype_achalasia() {
    let t = build(&achalasia());
    let eras: Vec<&str> = t.eras.iter().map(|e| e.label.as_str()).collect();
    assert_eq!(eras, ["2006–10", "2011–15", "2016–20", "2021–26"]);
    assert!(t.eras[1].empty && t.eras[3].empty);
    assert!(
        t.notes
            .iter()
            .any(|n| n.contains("2011–15") && n.contains("kept, flagged"))
    );
    for (child, parent, months, era) in [
        ("eat10", "s-eat", 120, "e-2006-2010"),
        ("werner5", "s-werner", 60, "e-2016-2020"),
    ] {
        let s = study_by(&t, child);
        assert_eq!(s.parent_id.as_deref(), Some(parent), "{child}");
        assert_eq!(s.horizon.as_ref().unwrap().months, months);
        assert_eq!(s.era_id.as_deref(), Some(era));
        assert!(s.assumed);
    }
    assert_eq!(study_by(&t, "werner5").knowledge_year, Some(2025));
    assert!(t.overlap_caveats.is_empty(), "a same-trial note is a link");
    assert_eq!(t.guidelines.len(), 1);

    let reflux = outcome_named(&t, "Reflux");
    assert_eq!(reflux.label, Label::PersistentTradeoff);
    let g = reflux
        .groups
        .iter()
        .find(|g| Some(&g.id) == reflux.lead_group_id.as_ref())
        .unwrap();
    let cell = g.cells.iter().find(|c| c.era_id == "e-2016-2020").unwrap();
    assert_eq!(cell.durability.len(), 1);
    assert_eq!(cell.durability[0].horizon_months, Some(60));
    assert_eq!(cell.durability[0].favours, Some(false));
    // Pairs, intervention first; no pooling without arm sizes.
    let w = t.rows.iter().find(|r| r.study_id == "s-werner").unwrap();
    assert_eq!(w.pair.as_ref().map(|p| (p.a, p.b)), Some((44.0, 29.0)));
    assert!(!w.derived && w.se.is_none());

    // PD vs LHM: two time points of one trial (2 and 10 years) in one era.
    let pd = outcome_named(&t, "Success, PD vs LHM");
    let g = &pd.groups[0];
    assert_eq!(g.row_ids.len(), 1);
    assert_eq!(g.durability_row_ids.len(), 1);
}

// ---- Trajectory rules -----------------------------------------------------

/// A cell with an optional pool (y, se) on the pooling scale.
fn cell(i: usize, pool: Option<(f64, f64)>, point: Option<f64>, z: Option<f64>) -> EraCell {
    let fam = Family::Ratio;
    let pooled = pool.map(|(y, se)| Est::new(Obs { y, se }, fam));
    let est = pooled.map(|p| p.est).or(point.map(f64::exp));
    EraCell {
        id: format!("c-{i}"),
        era_id: format!("e-{i}"),
        row_ids: vec![],
        pooled,
        point_mean: point.map(f64::exp),
        estimate: est,
        z,
        after_earlier: z.is_some(),
        favours: None,
        significant: pooled.map(|p| p.significant()),
        empty: est.is_none(),
        durability: vec![],
    }
}

fn flags(cells: &[EraCell]) -> Vec<Label> {
    trajectory_flags(Family::Ratio, Polarity::HigherWorse, cells)
}

#[test]
fn rule_one_era_only_and_not_assessable() {
    let f = flags(&[
        cell(0, None, None, None),
        cell(1, Some((0.3, 0.1)), None, None),
    ]);
    assert_eq!(pick_label(&f), Label::OneEraOnly);
    let f = flags(&[cell(0, None, None, None)]);
    assert_eq!(f, vec![Label::NotAssessable]);
    assert_eq!(flags(&[]), vec![Label::NotAssessable]);
}

#[test]
fn rule_no_recent_data() {
    let f = flags(&[
        cell(0, Some((0.05, 0.3)), None, None),
        cell(1, None, None, None),
    ]);
    assert!(f.contains(&Label::NoRecentData));
    assert_eq!(pick_label(&f), Label::NoRecentData);
}

#[test]
fn rule_stable() {
    let f = flags(&[
        cell(0, Some((0.7, 0.1)), None, None),
        cell(1, Some((0.5, 0.2)), None, Some(-0.8)),
    ]);
    assert_eq!(pick_label(&f), Label::Stable);
    // Same side but a shift: not stable.
    let f = flags(&[
        cell(0, Some((0.7, 0.1)), None, None),
        cell(1, Some((0.1, 0.05)), None, Some(-3.0)),
    ]);
    assert!(!f.contains(&Label::Stable));
    assert_eq!(pick_label(&f), Label::ShiftingToward);
}

#[test]
fn rule_shifting_against() {
    let f = flags(&[
        cell(0, Some((0.05, 0.1)), None, None),
        cell(1, Some((0.6, 0.1)), None, Some(2.5)),
    ]);
    assert_eq!(pick_label(&f), Label::ShiftingAgainst);
    // Higher is better: the same rise is toward.
    let f = trajectory_flags(
        Family::Ratio,
        Polarity::HigherBetter,
        &[
            cell(0, Some((0.05, 0.1)), None, None),
            cell(1, Some((0.6, 0.1)), None, Some(2.5)),
        ],
    );
    assert_eq!(pick_label(&f), Label::ShiftingToward);
}

#[test]
fn rule_direction_flipped() {
    let f = flags(&[
        cell(0, Some((0.48, 0.12)), None, None),
        cell(1, Some((-0.39, 0.16)), None, Some(-4.3)),
    ]);
    assert_eq!(pick_label(&f), Label::DirectionFlipped);
    // Crossing without |Z| ≥ 2 is not a flip.
    let f = flags(&[
        cell(0, Some((0.1, 0.3)), None, None),
        cell(1, Some((-0.1, 0.3)), None, Some(-0.5)),
    ]);
    assert!(!f.contains(&Label::DirectionFlipped));
}

#[test]
fn rule_early_harm_not_seen_later_outranks_flip() {
    let f = flags(&[
        cell(0, Some((0.83, 0.11)), None, None),
        cell(1, Some((-0.07, 0.12)), None, Some(-5.4)),
    ]);
    assert!(f.contains(&Label::DirectionFlipped));
    assert_eq!(pick_label(&f), Label::EarlyHarmNotSeenLater);
    // A later significant benefit is not "null later".
    let f = flags(&[
        cell(0, Some((0.83, 0.11)), None, None),
        cell(1, Some((-0.7, 0.1)), None, Some(-9.0)),
    ]);
    assert!(!f.contains(&Label::EarlyHarmNotSeenLater));
}

#[test]
fn rule_gap_narrowing_needs_a_difference() {
    let mk = |v: f64, i: usize| {
        let mut c = cell(i, None, None, None);
        c.estimate = Some(v);
        c.point_mean = Some(v);
        c.empty = false;
        c
    };
    let cells = [mk(35.0, 0), mk(12.0, 1)];
    let f = trajectory_flags(Family::Difference, Polarity::HigherWorse, &cells);
    assert_eq!(pick_label(&f), Label::GapNarrowing);
    assert!(f.contains(&Label::PersistentTradeoff));
    // Widening is not narrowing.
    let f = trajectory_flags(
        Family::Difference,
        Polarity::HigherWorse,
        &[mk(12.0, 0), mk(35.0, 1)],
    );
    assert!(!f.contains(&Label::GapNarrowing));
    // Ratios never narrow.
    let f = trajectory_flags(
        Family::Ratio,
        Polarity::HigherWorse,
        &[mk(3.0, 0), mk(1.5, 1)],
    );
    assert!(!f.contains(&Label::GapNarrowing));
}

#[test]
fn rule_persistent_tradeoff_needs_the_latest_durability_point() {
    let mut c = cell(0, None, Some(0.4), None);
    c.durability.push(DurabilityPoint {
        row_id: "r".into(),
        knowledge_year: Some(2025),
        horizon_months: Some(60),
        value: 1.2,
        favours: Some(false),
    });
    let f = flags(std::slice::from_ref(&c));
    assert_eq!(pick_label(&f), Label::PersistentTradeoff);
    c.durability[0].favours = Some(true);
    let f = flags(std::slice::from_ref(&c));
    assert!(!f.contains(&Label::PersistentTradeoff));
    // One point only: not a trade-off over time.
    let f = flags(&[cell(0, None, Some(0.4), None)]);
    assert!(!f.contains(&Label::PersistentTradeoff));
}

#[test]
fn precedence_is_total_and_ordered() {
    let all: HashSet<Label> = LABEL_PRECEDENCE.iter().copied().collect();
    assert_eq!(all.len(), LABEL_PRECEDENCE.len());
    assert_eq!(
        pick_label(&[Label::OneEraOnly, Label::PersistentTradeoff]),
        Label::PersistentTradeoff
    );
    assert_eq!(
        pick_label(&[Label::Stable, Label::GapNarrowing]),
        Label::GapNarrowing
    );
    assert_eq!(pick_label(&[]), Label::Inconclusive);
}

// ---- Direction, pairs and counts ------------------------------------------

#[test]
fn flipped_rows_are_turned_round_once() {
    let mut i = cholecystitis();
    i.outcomes.push(outcome(
        "Prolonged stay",
        vec![ratio(
            "OR prolonged hospitalisation",
            1.079,
            0.682,
            1.707,
            "laparoscopic vs robotic",
            "claims",
        )],
    ));
    let t = build(&i);
    let r = t
        .rows
        .iter()
        .find(|r| r.label == "OR prolonged hospitalisation")
        .unwrap();
    assert!(r.flipped);
    assert_eq!(r.orientation, Orientation::Flipped);
    assert_eq!(r.intervention.as_deref(), Some("robotic"));
    assert!((r.value.unwrap() - 1.0 / 1.079).abs() < 1e-12);
    assert!((r.lo.unwrap() - 1.0 / 1.707).abs() < 1e-12);
    // Building again from the same input gives the same row: no double flip.
    let t2 = build(&i);
    assert_eq!(t, t2);
    assert!(t.notes.iter().any(|n| n.contains("comparator first")));
}

#[test]
fn percent_pairs_derive_counts_only_with_both_arm_sizes() {
    let mut s = study(
        "ac",
        "Acute care cohort",
        "Retrospective cohort",
        "Acute care service",
    );
    s.appraisal = "Small (107 robotic vs 215 laparoscopic) cohort".into();
    let refs = vec![reference("ac", 2026)];
    let nums = vec![
        arm_value("Early complications", 10.3, "%", "robotic", "ac"),
        arm_value("Early complications", 12.6, "%", "laparoscopic", "ac"),
    ];
    let t = build(&input(
        vec![s.clone()],
        refs.clone(),
        vec![outcome("Complications", nums.clone())],
    ));
    let r = &t.rows[0];
    assert!(r.derived);
    let c = r.counts.unwrap();
    assert_eq!(
        (c.events_a, c.n_a, c.events_b, c.n_b),
        (11.0, 107.0, 27.0, 215.0)
    );
    assert_eq!(r.family, Some(Family::Ratio));
    assert!(r.se.is_some());
    assert_eq!(r.number_ids.len(), 2);
    // No arm sizes: a pair, a point, no SE, nothing derived.
    s.appraisal = "Small cohort".into();
    let t = build(&input(vec![s], refs, vec![outcome("Complications", nums)]));
    let r = &t.rows[0];
    assert!(!r.derived && r.counts.is_none() && r.se.is_none());
    assert_eq!(r.pair.as_ref().map(|p| (p.a, p.b)), Some((10.3, 12.6)));
    assert_eq!(r.family, Some(Family::Difference));
}

// ---- Maths at table level --------------------------------------------------

#[test]
fn pooling_is_order_invariant_at_tau_zero() {
    let a = cholecystitis();
    let mut b = a.clone();
    b.studies.reverse();
    b.references.reverse();
    for o in &mut b.outcomes {
        o.numbers.reverse();
    }
    let (ta, tb) = (build(&a), build(&b));
    for name in ["Conversion", "Subtotal cholecystectomy", "Bile duct injury"] {
        let (ga, gb) = (
            &outcome_named(&ta, name).groups[0],
            &outcome_named(&tb, name).groups[0],
        );
        let (da, db) = (&ga.dynamic[0], &gb.dynamic[0]);
        assert_eq!(da.tau, 0.0);
        let (ea, eb) = (da.eras.last().unwrap(), db.eras.last().unwrap());
        assert!(
            (ea.mean - eb.mean).abs() < 1e-12 && (ea.sd - eb.sd).abs() < 1e-12,
            "{name}"
        );
        let (ra, rb) = (
            ga.random_effects.as_ref().unwrap(),
            gb.random_effects.as_ref().unwrap(),
        );
        assert!((ra.est.y - rb.est.y).abs() < 1e-12);
        assert_eq!(ga.label, gb.label);
    }
}

#[test]
fn single_study_outcome() {
    let i = input(
        vec![study("x", "Only trial", "RCT", "2022-2024")],
        vec![reference("x", 2023)],
        vec![outcome(
            "Leak",
            vec![ratio("OR leak", 0.8, 0.5, 1.28, RL, "x")],
        )],
    );
    let t = build(&i);
    let o = &t.outcomes[0];
    assert_eq!(o.label, Label::OneEraOnly);
    let g = &o.groups[0];
    let re = g.random_effects.as_ref().unwrap();
    assert_eq!((re.k, re.tau, re.i2), (1, 0.0, 0.0));
    assert!((re.est.est - 0.8).abs() < 1e-9);
    assert_eq!(g.dynamic.len(), 4);
    assert_eq!(t.eras.len(), 1);
    // An older single study: both coverage rules hold; no recent data wins.
    let mut old = i.clone();
    old.studies[0].setting = "2016-2018".into();
    let t = build(&old);
    assert_eq!(
        t.outcomes[0].flags,
        vec![Label::NoRecentData, Label::OneEraOnly]
    );
    assert_eq!(t.outcomes[0].label, Label::NoRecentData);
}

#[test]
fn empty_eras_are_kept_and_flagged() {
    let i = input(
        vec![
            study("a", "Old trial", "RCT", "2001-2003"),
            study("b", "New trial", "RCT", "2022-2024"),
        ],
        vec![reference("a", 2005), reference("b", 2025)],
        vec![outcome(
            "Leak",
            vec![
                ratio("OR leak", 0.8, 0.5, 1.28, RL, "a"),
                ratio("OR leak", 0.9, 0.6, 1.35, RL, "b"),
            ],
        )],
    );
    let t = build(&i);
    assert_eq!(t.eras.first().unwrap().label, "2001–05");
    assert_eq!(t.eras.iter().filter(|e| e.empty).count(), t.eras.len() - 2);
    let g = &t.outcomes[0].groups[0];
    assert_eq!(g.cells.len(), t.eras.len());
    assert!(
        g.cells
            .iter()
            .filter(|c| c.empty)
            .all(|c| c.estimate.is_none() && c.z.is_none())
    );
    // The dynamic model walks every era (drift is added through empty ones).
    assert_eq!(g.dynamic[2].eras.len(), t.eras.len());
    assert_eq!(t.outcomes[0].label, Label::Stable);
}

#[test]
fn bad_numbers_are_skipped_with_a_note_never_a_panic() {
    let mut nums = vec![
        ratio("OR a", 0.0, 0.5, 1.0, RL, "x"),
        ratio("OR b", -1.2, 0.5, 1.0, RL, "x"),
        ratio("OR c", 1.2, -0.5, 2.0, RL, "x"),
        ratio("OR d", 1.2, 2.0, 1.0, RL, "x"),
        ratio("OR e", 1.2, 0.0, 2.0, RL, "x"),
        ratio("OR f", 1.2, 1.2, 1.2, RL, "x"),
        ratio("OR g", 1.2, 0.9, 1.6, RL, "missing"),
    ];
    nums.push(InNumber {
        value: Some(f64::NAN),
        ..ratio("OR h", 1.0, 0.5, 2.0, RL, "x")
    });
    nums.push(InNumber {
        value: None,
        ..ratio("OR i", 1.0, 0.5, 2.0, RL, "x")
    });
    let i = input(
        vec![study("x", "Trial", "RCT", "2019-2021")],
        vec![reference("x", 2022)],
        vec![outcome("Leak", nums)],
    );
    let t = build(&i);
    assert!(
        t.rows.iter().all(|r| r.se.is_none()),
        "no row has a usable SE"
    );
    for l in [
        "OR a", "OR b", "OR c", "OR d", "OR e", "OR f", "OR g", "OR h", "OR i",
    ] {
        assert!(t.notes.iter().any(|n| n.contains(l)), "no note for {l}");
    }
    assert!(
        t.outcomes[0]
            .groups
            .iter()
            .all(|g| g.random_effects.is_none())
    );
    // NaN never reaches the output (serialises cleanly).
    serde_json::to_string(&t).unwrap();
}

#[test]
fn nothing_in_nothing_out() {
    let t = build(&TemporalInput::default());
    assert!(t.eras.is_empty() && t.rows.is_empty() && t.outcomes.is_empty());
    assert_eq!(t.run_year, None);
    // No run date: the latest publication year ends the latest era.
    let mut i = cholecystitis();
    i.created_at = None;
    let t = build(&i);
    assert_eq!(t.run_year, Some(2026));
    assert!(t.notes.iter().any(|n| n.contains("run's date is missing")));
}

#[test]
fn ids_are_stable_and_unique() {
    let t = build(&achalasia());
    let mut ids: Vec<&str> = Vec::new();
    ids.extend(t.studies.iter().map(|s| s.id.as_str()));
    ids.extend(t.eras.iter().map(|e| e.id.as_str()));
    ids.extend(t.rows.iter().map(|r| r.id.as_str()));
    ids.extend(t.outcomes.iter().map(|o| o.id.as_str()));
    for o in &t.outcomes {
        for g in &o.groups {
            ids.push(&g.id);
            ids.extend(g.cells.iter().map(|c| c.id.as_str()));
        }
    }
    ids.extend(t.markers.iter().map(|m| m.id.as_str()));
    ids.extend(t.guidelines.iter().map(|g| g.id.as_str()));
    let set: HashSet<&str> = ids.iter().copied().collect();
    assert_eq!(set.len(), ids.len());
    assert_eq!(build(&achalasia()), t);
    // Round trip.
    let j = serde_json::to_string(&t).unwrap();
    let back: TemporalTable = serde_json::from_str(&j).unwrap();
    assert_eq!(back, t);
}
