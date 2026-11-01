//! Golden tests of the temporal table (T1) on the owner's stored answers
//! (slim projection, `tests/fixtures/temporal/`). The expected labels are
//! the prototype tables' readings; where the real rows give a different
//! reading, the test asserts what the rules give and says why.

use incisive_app::temporal::{
    Label, OutcomeResult, RowRole, StudyKind, TemporalInput, TemporalTable, YearsFrom, build,
};

fn table(name: &str) -> TemporalTable {
    let p = format!(
        "{}/tests/fixtures/temporal/{name}.json",
        env!("CARGO_MANIFEST_DIR")
    );
    let json = std::fs::read_to_string(p).unwrap();
    build(&TemporalInput::from_json(&json).unwrap())
}

fn outcome<'a>(t: &'a TemporalTable, starts: &str) -> &'a OutcomeResult {
    t.outcomes
        .iter()
        .find(|o| o.outcome.starts_with(starts))
        .unwrap_or_else(|| panic!("no outcome {starts}"))
}

fn eras(t: &TemporalTable) -> Vec<&str> {
    t.eras.iter().map(|e| e.label.as_str()).collect()
}

#[test]
fn temporal_golden_cholecystitis() {
    let t = table("cholecystitis");
    assert_eq!(t.run_year, Some(2026));
    assert_eq!(eras(&t), ["2011–15", "2016–20", "2021–26"]);
    // Four reviews are markers.
    assert_eq!(
        t.studies
            .iter()
            .filter(|s| s.kind == StudyKind::Marker)
            .count(),
        4
    );
    // The "databases likely share patients" notes are caveats, not links.
    assert_eq!(t.overlap_caveats.len(), 2);
    assert!(t.studies.iter().all(|s| s.parent_id.is_none()));

    let conv = outcome(&t, "Conversion");
    assert_eq!(conv.label, Label::DirectionFlipped);

    let sub = outcome(&t, "Bailout procedures and subtotal");
    assert_eq!(sub.label, Label::Stable);

    let bdi = outcome(&t, "Bile duct injury");
    assert_eq!(bdi.label, Label::EarlyHarmNotSeenLater);
    assert!(bdi.flags.contains(&Label::NoRecentData));

    // Not in the prototype's labels: major complications OR 1.57
    // (1.48–1.67, 2016–21 claims) then a derived OR 0.80 (11/107 vs
    // 27/215, null) from the 2026 single-centre cohort.
    let comp = outcome(&t, "Postoperative complications");
    assert_eq!(comp.label, Label::EarlyHarmNotSeenLater);
    assert!(
        t.rows
            .iter()
            .any(|r| r.derived && r.label == "Early complications")
    );

    // The "laparoscopic vs robotic" OR is turned round.
    let r = t
        .rows
        .iter()
        .find(|r| r.label == "OR prolonged hospitalisation")
        .unwrap();
    assert!(r.flipped && (r.value.unwrap() - 1.0 / 1.079).abs() < 1e-9);
}

#[test]
fn temporal_golden_hernia() {
    let t = table("hernia");
    assert_eq!(eras(&t), ["2016–20", "2021–26"]);
    for s in t.studies.iter().filter(|s| s.kind == StudyKind::Primary) {
        assert!(s.assumed, "{}", s.label);
    }
    let by = |p: &str| t.studies.iter().find(|s| s.pmid == p).unwrap();
    // RIVAL 2-year and DIRECT are reports of RIVAL and ROLAIS.
    assert_eq!(by("35578051").parent_id.as_deref(), Some("s-32186683"));
    assert_eq!(by("35578051").years_from, YearsFrom::Parent);
    assert_eq!(by("35578051").horizon.as_ref().unwrap().months, 24);
    assert_eq!(by("40632168").parent_id.as_deref(), Some("s-40277023"));
    assert_eq!(
        t.overlap_caveats.len(),
        1,
        "only the meta-analyses note is a caveat"
    );

    // Not the prototype's `gap_narrowing`: the 2016–20 era is RIVAL +35 min
    // (75.5 vs 40.5), but the 2021–26 era is ROGER +16.1 (80.3 vs 64.2) and
    // ROLAIS −13.4 (reported difference): points without CIs in opposite
    // directions. The era is split (not averaged), so no narrowing can be
    // claimed; every era value is a point (points_only).
    let ot = outcome(&t, "Operative time");
    assert_eq!(ot.label, Label::Inconclusive);
    assert!(ot.flags.contains(&Label::PointsOnly));
    let lead = ot
        .groups
        .iter()
        .find(|g| Some(&g.id) == ot.lead_group_id.as_ref())
        .unwrap();
    assert!(lead.cells.last().unwrap().split);
    assert_eq!(lead.intervention, "robotic");
    assert_eq!(lead.comparator, "laparoscopic");
    // The pooled MD is a marker row.
    assert_eq!(ot.marker_row_ids.len(), 1);
    assert!(
        t.rows
            .iter()
            .filter(|r| r.outcome_id == ot.id && r.role == RowRole::Primary)
            .all(|r| r.se.is_none()),
        "differences of arm means are points, never pooled"
    );
}

#[test]
fn temporal_golden_achalasia() {
    let t = table("achalasia");
    assert_eq!(eras(&t), ["2006–10", "2011–15", "2016–20", "2021–26"]);
    assert!(t.eras[1].empty);
    let by = |p: &str| t.studies.iter().find(|s| s.pmid == p).unwrap();
    for (child, parent, months) in [
        ("40112837", "s-31800987", 60),
        ("36206786", "s-31287522", 60),
        ("38050085", "s-21561346", 120),
    ] {
        let s = by(child);
        assert_eq!(s.parent_id.as_deref(), Some(parent), "{child}");
        assert_eq!(s.horizon.as_ref().unwrap().months, months, "{child}");
        assert_eq!(
            s.era_id,
            by(parent.trim_start_matches("s-")).era_id,
            "{child} inherits the era"
        );
        assert_eq!(s.knowledge_year, s.pub_year);
    }

    let reflux = outcome(&t, "Gastro-oesophageal reflux");
    assert_eq!(reflux.label, Label::PersistentTradeoff);
    assert!(reflux.flags.contains(&Label::PointsOnly));

    // POEM vs PD success: +38 points at 2 years (Ponds 2019), +35.5 after
    // failed LHM (Dutch 2023): a 7% drop, below the 25% narrowing bar, so
    // no trajectory label (was `gap_narrowing` before the review).
    let pd = outcome(&t, "Place of pneumatic dilation");
    assert_eq!(pd.label, Label::Inconclusive);
    // Each comparison is its own group: POEM−PD never pools with LHM−PD.
    assert!(
        pd.groups
            .iter()
            .all(|g| g.intervention == "poem" && g.comparator == "pd")
    );
}

/// The same answer read twice gives the same table (ids included).
#[test]
fn temporal_golden_is_deterministic() {
    for f in ["cholecystitis", "hernia", "achalasia"] {
        assert_eq!(table(f), table(f), "{f}");
    }
}
