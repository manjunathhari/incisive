//! Temporal T4: the Expert Opinion rules (`expert_page`, no Leptos) on the
//! three fixture tables (`tests/fixtures/temporal/full/*.json` through
//! `temporal::build`) and the owner's smoke boards
//! (`tests/fixtures/temporal/boards/*-board.json`): which outcomes go on
//! the linked chart, the scale, the era and ledger labels, the timeline and
//! chart coordinates, and the read-only board mirror.

use incisive_app::agent::temporal_writer::{BoardFlags, ExpertBoardV1};
use incisive_app::expert_page::{
    self as xp, Axis, Board, Flags, GANTT_L, GANTT_R, GANTT_W, LINKED_B, LINKED_H, LINKED_L,
    LINKED_R, LINKED_T, LINKED_W, OptionState,
};
use incisive_app::runs::TemporalView;
use incisive_app::temporal::{Family, StudyKind, TemporalInput, TemporalTable, build};
use serde_json::Value;

const NAMES: [&str; 3] = ["cholecystitis", "hernia", "achalasia"];

fn table(name: &str) -> TemporalTable {
    let p = format!(
        "{}/tests/fixtures/temporal/full/{name}.json",
        env!("CARGO_MANIFEST_DIR")
    );
    build(&TemporalInput::from_json(&std::fs::read_to_string(p).unwrap()).unwrap())
}

fn stored(name: &str) -> Value {
    let p = format!(
        "{}/tests/fixtures/temporal/boards/{name}-board.json",
        env!("CARGO_MANIFEST_DIR")
    );
    serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap()
}

fn board(name: &str) -> Board {
    serde_json::from_value(stored(name)["board"].clone()).unwrap()
}

fn outcomes(
    t: &TemporalTable,
    v: &[(
        &incisive_app::temporal::OutcomeResult,
        &incisive_app::temporal::GroupResult,
    )],
) -> Vec<String> {
    let _ = t;
    v.iter().map(|(o, _)| o.outcome.clone()).collect()
}

fn starts(v: &[String], prefixes: &[&str]) {
    assert_eq!(v.len(), prefixes.len(), "{v:?}");
    for (s, p) in v.iter().zip(prefixes) {
        assert!(s.starts_with(p), "{s} does not start with {p}");
    }
}

/// The board mirror reads exactly what the writer's `ExpertBoardV1` and
/// `BoardFlags` store (field for field, on the three smoke boards).
#[test]
fn the_board_mirror_matches_the_writer_types() {
    for name in NAMES {
        let s = stored(name);
        let writer: ExpertBoardV1 = serde_json::from_value(s["board"].clone()).unwrap();
        let mirror: Board = serde_json::from_value(s["board"].clone()).unwrap();
        assert_eq!(
            serde_json::to_value(&writer).unwrap(),
            serde_json::to_value(&mirror).unwrap(),
            "{name}"
        );
        let wf: BoardFlags = serde_json::from_value(s["flags"].clone()).unwrap();
        let mf: Flags = serde_json::from_value(s["flags"].clone()).unwrap();
        assert_eq!(
            serde_json::to_value(&wf).unwrap(),
            serde_json::to_value(&mf).unwrap(),
            "{name}"
        );
        assert_eq!(wf.total(), mf.total(), "{name}");
        // Every text field the writer's checks read is a text the page draws.
        let texts = mirror.texts();
        for (field, t) in writer.text_fields() {
            assert!(texts.contains(&t), "{name}: {field}");
        }
        assert_eq!(texts.len(), writer.text_fields().len(), "{name}");
    }
}

/// A stored `done` row opens; the option is ready.
#[test]
fn a_done_row_with_a_fixture_board_is_ready() {
    for name in NAMES {
        let t = table(name);
        let s = stored(name);
        let v = TemporalView {
            status: "done".into(),
            reason: None,
            topic: Some("T".into()),
            evidence_year: t.run_year,
            board: Some(s["board"].clone()),
            table: Some(serde_json::to_value(&t).unwrap()),
            flags: Some(s["flags"].clone()),
        };
        assert_eq!(OptionState::of(&v), OptionState::Ready, "{name}");
        let o = xp::open(&v).unwrap();
        assert_eq!(o.table, t);
        assert!(o.flags.is_some());
        // A table that does not read: unavailable, never a broken board.
        let bad = TemporalView {
            table: Some(serde_json::json!({"version": "x"})),
            ..v.clone()
        };
        assert_eq!(OptionState::of(&bad), OptionState::Failed, "{name}");
    }
}

/// Which outcomes go on the linked chart: up to three, data in two or more
/// eras, in table order, never cost.
#[test]
fn linked_outcomes_on_the_three_fixtures() {
    let t = table("cholecystitis");
    let v = outcomes(&t, &xp::linked_outcomes(&t));
    starts(
        &v,
        &[
            "Conversion",
            "Bailout procedures and subtotal",
            "Bile duct injury",
        ],
    );
    // A fourth with two eras exists (complications); three at most.
    assert!(
        xp::drawn_outcomes(&t)
            .iter()
            .filter(|(_, g)| g.cells.iter().filter(|c| xp::has_data(c)).count() >= 2)
            .count()
            > 3
    );

    // Hernia: operative time (35.0 then a split era) and workload; cost
    // (two eras) is never drawn.
    let t = table("hernia");
    let v = outcomes(&t, &xp::linked_outcomes(&t));
    starts(&v, &["Operative time", "Surgeon workload"]);
    assert!(t.outcomes.iter().any(|o| o.outcome == "Cost"));
    assert!(!v.iter().any(|o| o.contains("Cost")));

    // Achalasia: only pneumatic dilation has two eras with data.
    let t = table("achalasia");
    let v = outcomes(&t, &xp::linked_outcomes(&t));
    starts(&v, &["Place of pneumatic dilation"]);
}

/// No cost outcome is drawn anywhere from the table (owner decision 3).
#[test]
fn cost_outcomes_are_never_drawn() {
    for name in NAMES {
        let t = table(name);
        for (o, _) in xp::drawn_outcomes(&t) {
            assert!(!o.outcome.to_lowercase().contains("cost"), "{name}");
        }
        for e in &t.eras {
            for f in xp::era_figures(&t, &e.id) {
                assert!(!f.label.to_lowercase().contains("cost"), "{name}");
                assert!(!f.value.contains("usd"), "{name}: {}", f.value);
            }
        }
        if let Some(l) = xp::predictive_line(&t) {
            assert!(!l.to_lowercase().contains("cost"), "{name}: {l}");
        }
    }
}

/// The scale: ratios on a log axis with no difference at 1, differences on
/// a linear axis with no difference at 0.
#[test]
fn the_scale_per_fixture() {
    let p = xp::linked_panels(&table("cholecystitis"));
    assert_eq!(p.len(), 1);
    assert_eq!(p[0].family, Family::Ratio);
    assert!(matches!(p[0].axis, Axis::Log { .. }));
    assert_eq!(p[0].axis.null(), 1.0);
    for name in ["hernia", "achalasia"] {
        let p = xp::linked_panels(&table(name));
        assert!(!p.is_empty(), "{name}");
        for pn in &p {
            assert_eq!(pn.family, Family::Difference, "{name}");
            assert!(matches!(pn.axis, Axis::Linear { .. }), "{name}");
            assert_eq!(pn.axis.null(), 0.0);
        }
    }
    // Hernia: minutes and the workload score are separate panels.
    let p = xp::linked_panels(&table("hernia"));
    assert_eq!(p.len(), 2);
    assert_eq!(p[0].unit_key, "min");
    assert_eq!(p[1].unit_key, "score");
}

/// The linked chart's coordinates on cholecystitis: the CIs as whiskers,
/// the null line at y(1), the points inside the plot, a lower ratio drawn
/// lower, and series offset around each era's centre.
#[test]
fn linked_chart_coordinates_cholecystitis() {
    let t = table("cholecystitis");
    let pn = &xp::linked_panels(&t)[0];
    let top = LINKED_T;
    let bottom = LINKED_H - LINKED_B;
    let y = |v: f64| top + (1.0 - pn.axis.frac(v)) * (bottom - top);
    assert!((pn.null_y - y(1.0)).abs() < 1e-9);
    // Era centres: three equal columns between the margins.
    let col = (LINKED_W - LINKED_L - LINKED_R) / 3.0;
    for (i, (x, _)) in pn.cols.iter().enumerate() {
        assert!((x - (LINKED_L + (i as f64 + 0.5) * col)).abs() < 1e-9);
    }
    assert_eq!(pn.series.len(), 3);
    let conv = &pn.series[0];
    assert!(conv.outcome.starts_with("Conversion"));
    // Two points (2016–20 OR 1.61, 2021–26 OR 0.68), offset to the left.
    assert_eq!(conv.points.len(), 2);
    let (a, b) = (&conv.points[0], &conv.points[1]);
    assert!((a.x - (pn.cols[1].0 - xp::LINKED_OFFSET)).abs() < 1e-9);
    assert!((b.x - (pn.cols[2].0 - xp::LINKED_OFFSET)).abs() < 1e-9);
    assert_eq!(a.label, "1.61");
    assert_eq!(b.label, "0.68");
    assert!(a.y.unwrap() < pn.null_y && b.y.unwrap() > pn.null_y);
    // Whiskers: the stored CIs (1.28–2.03 and 0.49–0.94), top above
    // bottom, around the point.
    let lead = xp::linked_outcomes(&t)[0].1;
    let ci = |era: &str| {
        let c = lead.cells.iter().find(|c| c.era_id == era).unwrap();
        let p = c.pooled.unwrap();
        (p.lo, p.hi)
    };
    let (lo, hi) = ci("e-2016-2020");
    assert!((format!("{lo:.2}"), format!("{hi:.2}")) == ("1.28".into(), "2.03".into()));
    let (wt, wb) = a.whisker.unwrap();
    assert!((wt - y(hi)).abs() < 1e-9 && (wb - y(lo)).abs() < 1e-9);
    assert!(wt < a.y.unwrap() && a.y.unwrap() < wb);
    let (lo, hi) = ci("e-2021-2026");
    let (wt, wb) = b.whisker.unwrap();
    assert!((wt - y(hi)).abs() < 1e-9 && (wb - y(lo)).abs() < 1e-9);
    assert!(!a.hollow && !b.hollow && !a.split);
    // Pooled CIs and stated years: solid line.
    assert_eq!(conv.segs.len(), 1);
    assert!(!conv.segs[0].dashed);
    // Every coordinate inside the plot.
    for s in &pn.series {
        for p in &s.points {
            let py = p.y.unwrap();
            assert!(py >= top - 1e-9 && py <= bottom + 1e-9, "{py}");
            if let Some((a, b)) = p.whisker {
                assert!(a >= top - 1e-9 && b <= bottom + 1e-9);
            }
        }
    }
    // Log scale: equal ratios are equal distances (2 and 0.5 around 1).
    assert!(((y(2.0) - y(1.0)) + (y(0.5) - y(1.0))).abs() < 1e-9);
}

/// Hernia operative time: points only (hollow), assumed years (dashed),
/// and a split latest era drawn as a tag with no line to it.
#[test]
fn linked_chart_points_only_split_and_assumed_hernia() {
    let t = table("hernia");
    let pn = &xp::linked_panels(&t)[0];
    let s = &pn.series[0];
    assert!(s.outcome.starts_with("Operative time"));
    assert_eq!(s.points.len(), 2);
    let (a, b) = (&s.points[0], &s.points[1]);
    assert_eq!(a.label, "+35.0");
    assert!(a.hollow && a.whisker.is_none() && a.assumed);
    assert!(b.split && b.y.is_none() && b.label == "split");
    // Only one drawn point: no segment.
    assert!(s.segs.is_empty());
    // Linear: 0 sits inside the axis, 35 above it.
    assert!(a.y.unwrap() < pn.null_y);
    let Axis::Linear { lo, hi } = pn.axis else {
        panic!()
    };
    assert!(lo <= 0.0 && hi >= 35.0);
    // Workload: 12.6 then −15.6, crossing 0, dashed (assumed years).
    let w = &xp::linked_panels(&t)[1].series[0];
    assert_eq!(w.segs.len(), 1);
    assert!(w.segs[0].dashed);
    assert!(w.points[0].y.unwrap() < xp::linked_panels(&t)[1].null_y);
    assert!(w.points[1].y.unwrap() > xp::linked_panels(&t)[1].null_y);
    // The slot (colour) follows the chart's outcome order across panels.
    assert_eq!(s.slot, 0);
    assert_eq!(w.slot, 1);
}

/// The timeline on each fixture: one row per independent primary study,
/// bars over the treatment years inside the plot, assumed years dashed,
/// follow-ups on their parent's row, era bands tiling the eras, and marker
/// lines for the meta-analysis years.
#[test]
fn gantt_coordinates_on_the_three_fixtures() {
    for name in NAMES {
        let t = table(name);
        let g = xp::gantt(&t).unwrap();
        let primaries: Vec<_> = t
            .studies
            .iter()
            .filter(|s| s.kind == StudyKind::Primary && s.parent_id.is_none())
            .collect();
        assert_eq!(g.rows.len(), primaries.len(), "{name}");
        let span = f64::from(g.to - g.from);
        let unit = (GANTT_W - GANTT_L - GANTT_R) / span;
        for (i, r) in g.rows.iter().enumerate() {
            assert!((r.y - (xp::GANTT_T + 10.0 + i as f64 * xp::GANTT_ROW)).abs() < 1e-9);
            let s = primaries.iter().find(|s| s.label == r.title).unwrap();
            assert_eq!(r.dashed, s.assumed, "{name}: {}", s.label);
            let (a, b) = r.bar.unwrap();
            let start = s.treatment_start.unwrap();
            let end = s.treatment_end.unwrap();
            assert!((a - g.x(f64::from(start))).abs() < 1e-9);
            // A bar spans whole years: start of the first to the end of the last.
            assert!((b - a - f64::from(end - start + 1) * unit).abs() < 1e-6);
            assert!(
                a >= GANTT_L - 1e-9 && b <= GANTT_W - GANTT_R + 1e-9,
                "{name}"
            );
            assert!(r.label.chars().count() <= xp::GANTT_LABEL_MAX);
            // Follow-up reports are points on the parent's row.
            let kids: Vec<_> = t
                .studies
                .iter()
                .filter(|c| c.parent_id.as_deref() == Some(s.id.as_str()))
                .collect();
            assert_eq!(r.points.len(), kids.len(), "{name}: {}", s.label);
            for (k, c) in r.points.iter().zip(&kids) {
                let ky = c.knowledge_year.unwrap();
                assert!((k.x - g.x(f64::from(ky) + 0.5)).abs() < 1e-9);
            }
        }
        // Rows sorted by treatment start.
        let order: Vec<i32> = g
            .rows
            .iter()
            .map(|r| {
                primaries
                    .iter()
                    .find(|s| s.label == r.title)
                    .unwrap()
                    .treatment_start
                    .unwrap()
            })
            .collect();
        assert!(order.windows(2).all(|w| w[0] <= w[1]), "{name}: {order:?}");
        // Era bands: one per era, contiguous, the latest with tint 3.
        assert_eq!(g.bands.len(), t.eras.len());
        for w in g.bands.windows(2) {
            assert!((w[0].x + w[0].w - w[1].x).abs() < 1e-9, "{name}");
        }
        assert_eq!(g.bands.last().unwrap().class, 3);
        // Marker lines: one per meta-analysis year (no guidelines here).
        let mut years: Vec<i32> = t.markers.iter().filter_map(|m| m.pub_year).collect();
        years.sort_unstable();
        years.dedup();
        assert_eq!(g.markers.len(), years.len(), "{name}");
        assert!(g.markers.iter().all(|m| !m.guideline));
        for (m, y) in g.markers.iter().zip(&years) {
            assert!((m.x - g.x(f64::from(*y) + 0.5)).abs() < 1e-9);
        }
        assert!(g.height > g.axis_y);
    }
    // Hernia: RIVAL's 2-year report on RIVAL's row; achalasia: three
    // follow-ups (5-, 5- and 10-year).
    let g = xp::gantt(&table("hernia")).unwrap();
    let rival = g
        .rows
        .iter()
        .find(|r| r.title.starts_with("RIVAL 2020"))
        .unwrap();
    assert_eq!(rival.points.len(), 1);
    assert_eq!(rival.points[0].label, "2-year");
    let g = xp::gantt(&table("achalasia")).unwrap();
    let pts: usize = g.rows.iter().map(|r| r.points.len()).sum();
    assert_eq!(pts, 3);
    assert!(
        g.rows
            .iter()
            .any(|r| r.points.iter().any(|p| p.label == "10-year"))
    );
    // Cholecystitis: stated years, solid bars; two single-year cohorts assumed.
    let g = xp::gantt(&table("cholecystitis")).unwrap();
    assert_eq!(g.rows.iter().filter(|r| r.dashed).count(), 2);
}

/// Era cards: the board's notes by era id, the table's figures, and an
/// empty era said plainly.
#[test]
fn era_mapping_on_the_three_fixtures() {
    for name in NAMES {
        let t = table(name);
        let b = board(name);
        assert!(b.orphan_era_notes(&t).is_empty(), "{name}");
        for e in &t.eras {
            assert!(b.era_note(&e.id).is_some(), "{name}: {}", e.id);
        }
    }
    // Achalasia 2011–15 has no studies and no figures.
    let t = table("achalasia");
    let e = t.eras.iter().find(|e| e.id == "e-2011-2015").unwrap();
    assert!(e.empty);
    assert!(xp::era_figures(&t, &e.id).is_empty());
    assert_eq!(xp::era_chip(1, e, t.eras.len()), "No studies");
    // Cholecystitis 2016–20: conversion OR 1.61 with its CI.
    let t = table("cholecystitis");
    let f = xp::era_figures(&t, "e-2016-2020");
    let conv = f
        .iter()
        .find(|f| f.label.starts_with("Conversion"))
        .unwrap();
    assert_eq!(conv.value, "1.61");
    assert!(
        conv.detail.starts_with("95% CI 1.28\u{2013}2.03"),
        "{}",
        conv.detail
    );
    // The latest era's conversion carries its Z.
    let f = xp::era_figures(&t, "e-2021-2026");
    let conv = f
        .iter()
        .find(|f| f.label.starts_with("Conversion"))
        .unwrap();
    assert!(
        conv.detail.contains(" \u{b7} Z \u{2212}"),
        "{}",
        conv.detail
    );
    // Hernia's split era says so.
    let t = table("hernia");
    let f = xp::era_figures(&t, "e-2021-2026");
    let op = f
        .iter()
        .find(|f| f.label.starts_with("Operative time"))
        .unwrap();
    assert_eq!(op.value, "split");
    // An orphan note is kept and listed.
    let mut b = board("hernia");
    b.eras[0].era_id = "e-1990-1994".into();
    assert_eq!(b.orphan_era_notes(&t).len(), 1);
}

/// The ledger's labels: hernia by treatment era, achalasia "known by".
#[test]
fn ledger_labels_on_the_fixtures() {
    let b = board("hernia");
    let t = table("hernia");
    assert_eq!(b.ledger.label_mode, "treatment");
    assert_eq!(
        xp::ledger_headers(&b.ledger.label_mode, &t.eras, 2),
        ["After 2016\u{2013}20", "Now, after 2021\u{2013}26"]
    );
    let b = board("achalasia");
    let t = table("achalasia");
    assert_eq!(b.ledger.label_mode, "known_by");
    assert_eq!(
        xp::ledger_headers(&b.ledger.label_mode, &t.eras, 4),
        [
            "Known by 2010",
            "Known by 2015",
            "Known by 2020",
            "Known now, 2026"
        ]
    );
}

/// The predictive range line: the table's lead outcomes with a range.
#[test]
fn predictive_line_on_cholecystitis() {
    let l = xp::predictive_line(&table("cholecystitis")).unwrap();
    assert!(l.contains("0.24\u{2013}4.57"), "{l}");
    assert!(l.contains("1.40\u{2013}2.76"), "{l}");
    assert!(l.contains("0.31\u{2013}6.82"), "{l}");
}

/// Known references on the boards: every cited number in the run's
/// reference list, none the checks flagged unknown.
#[test]
fn known_references_on_the_boards() {
    for name in NAMES {
        let s = stored(name);
        let b = board(name);
        let f: Flags = serde_json::from_value(s["flags"].clone()).unwrap();
        let known = xp::known_refs(&b, Some(&f));
        assert!(!known.is_empty(), "{name}");
        let p = format!(
            "{}/tests/fixtures/temporal/full/{name}.json",
            env!("CARGO_MANIFEST_DIR")
        );
        let body: Value = serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap();
        let refs: Vec<u64> = body["references"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["n"].as_u64().unwrap())
            .collect();
        for n in &known {
            assert!(refs.contains(&u64::from(*n)), "{name}: [{n}]");
        }
    }
}

/// The Expert Opinion styles never reach the answer content: every rule's
/// selectors are scoped to the `rx` / `rt` classes (or `:root` tokens),
/// and the colours come from the ivory tokens or the frozen mockups' own.
#[test]
fn expert_styles_are_scoped_away_from_the_answer() {
    let style = incisive_app::app::EXPERT_STYLE;
    for sel in [
        ".ap-main",
        ".ap ",
        ".ap{",
        ".ap-original",
        ".answer",
        "\nbody",
        "main ",
    ] {
        assert!(!style.contains(sel), "{sel}");
    }
    let selectors = regex::Regex::new(r"(?m)^([^{}@/\n][^{}]*)\{").unwrap();
    let mut n = 0;
    for c in selectors.captures_iter(style) {
        for sel in c[1].split(',') {
            let sel = sel.trim();
            if sel.is_empty() || sel.starts_with(":root") {
                continue;
            }
            n += 1;
            let scoped = sel.split_whitespace().next().is_some_and(|first| {
                ["rx", "rt"].iter().any(|p| {
                    first.starts_with(&format!(".{p}"))
                        || first.starts_with(&format!("details.{p}"))
                        || first.starts_with(&format!("a.{p}"))
                })
            });
            assert!(scoped, "unscoped selector: {sel}");
            // The run page's wrapper (`.rx`, `.rx-pre`) holds the answer
            // too: a rule under it must name one of the option's or the
            // view's own `rx-*` elements, or the page chrome above the
            // answer (`.run-chrome`, the results card and Progress).
            let mut parts = sel.split_whitespace();
            let first = parts.next().unwrap();
            let base = first.split([':', '[']).next().unwrap();
            if base == ".rx" || base == ".rx-pre" {
                let next = parts.next().unwrap_or("");
                assert!(
                    next.starts_with(".rx-") || next.contains(".rx-") || next == ".run-chrome",
                    "a rule under the run page wrapper reaches the answer: {sel}"
                );
            }
        }
    }
    assert!(n > 50, "{n}");
    // No new colour tokens beyond the ivory ones and the mockups' own.
    let tokens = regex::Regex::new(r"--([a-z0-9-]+):").unwrap();
    for t in tokens.captures_iter(style) {
        let t = &t[1];
        assert!(t.starts_with("rx-ok") || t.starts_with("rt-"), "{t}");
    }
    assert!(style.contains("var(--iv-accent)") && style.contains("var(--iv-font)"));
}

/// The scope check rejects a bare wrapper rule (the review's case).
#[test]
fn a_bare_wrapper_rule_would_fail_the_scope_check() {
    let bad = [".rx a:focus-visible", ".rx button", ".rx-pre p", ".rx > a"];
    let good = [
        ".rx .rx-tab",
        ".rx-pre .rx-expert:target",
        ".rx-pre:has(.rx-expert) a.rx-tab",
    ];
    let reaches = |sel: &str| {
        let mut parts = sel.split_whitespace();
        let base = parts.next().unwrap().split([':', '[']).next().unwrap();
        (base == ".rx" || base == ".rx-pre") && !parts.next().unwrap_or("").contains(".rx-")
    };
    for s in bad {
        assert!(reaches(s), "{s}");
    }
    for s in good {
        assert!(!reaches(s), "{s}");
    }
}

/// Long unbroken words (DOIs, URLs, ids) wrap inside the Expert Opinion
/// view and the board instead of widening the page.
#[test]
fn long_words_wrap_in_the_view_and_the_board() {
    let style = incisive_app::app::EXPERT_STYLE;
    for class in ["\n.rx-expert {", "\n.rt {"] {
        let i = style.find(class).unwrap_or_else(|| panic!("{class}"));
        let rule = &style[i..i + style[i..].find('}').unwrap()];
        assert!(rule.contains("overflow-wrap:anywhere"), "{class}");
    }
}
