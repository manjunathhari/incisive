//! Reading one user's map (`graph::read::load_map`) and placing its dots
//! (`graph_view::layout`). The layout needs no database; the rest use
//! `TEST_DATABASE_URL`.

use std::collections::HashSet;

use incisive_app::db::{lazy_pool, migrate};
use incisive_app::graph::extract::{PickedOutcome, Picks};
use incisive_app::graph::rank::rank_paper;
use incisive_app::graph::read::load_map;
use incisive_app::graph::store::{PaperRow, write_run};
use incisive_app::graph_view::{Dot, DotKind, Line, MapData, MapFilter, layout};
use secrecy::SecretString;
use sqlx::PgPool;
use uuid::Uuid;

/// The database tests share the concept table, so they run one at a time.
static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

// ---------- layout (pure) ----------

fn dot(id: &str, kind: DotKind, r: f32) -> Dot {
    Dot {
        id: id.into(),
        kind,
        label: String::new(),
        sub: String::new(),
        first_year: None,
        r,
        rank: None,
        x: 0.0,
        y: 0.0,
        run_id: None,
        concept_id: None,
        pmid: None,
        retracted: false,
        design: String::new(),
        n: None,
        includes_non_rct: false,
        sheet: None,
        question: None,
        bottom_line: None,
        certainty: None,
    }
}

fn line(a: &str, b: &str, paper: bool) -> Line {
    Line {
        a: a.into(),
        b: b.into(),
        paper,
    }
}

fn sample() -> (Vec<Dot>, Vec<Line>) {
    let dots = vec![
        dot("r:1", DotKind::Run, 9.0),
        dot("g:1", DotKind::Organ, 12.0),
        dot("d:2", DotKind::Disease, 15.0),
        dot("t:3", DotKind::Treatment, 11.0),
        dot("o:4", DotKind::Outcome, 8.0),
        dot("p:100", DotKind::Paper, 8.0),
        dot("p:101", DotKind::Paper, 5.0),
        dot("r:2", DotKind::Run, 9.0),
    ];
    let lines = vec![
        line("g:1", "d:2", false),
        line("d:2", "r:1", false),
        line("d:2", "r:2", false),
        line("r:1", "t:3", false),
        line("r:2", "t:3", false),
        line("r:1", "o:4", false),
        line("r:1", "p:100", true),
        line("r:2", "p:101", true),
        line("r:2", "p:missing", true),
    ];
    (dots, lines)
}

fn assert_inside(dots: &[Dot], w: f32, h: f32) {
    for d in dots {
        assert!(d.x.is_finite() && d.y.is_finite(), "{}: not finite", d.id);
        assert!(
            (0.0..=w).contains(&d.x),
            "{}: x {} outside 0..{w}",
            d.id,
            d.x
        );
        assert!(
            (0.0..=h).contains(&d.y),
            "{}: y {} outside 0..{h}",
            d.id,
            d.y
        );
    }
}

#[test]
fn layout_is_bit_identical_for_the_same_input() {
    let (mut a, lines) = sample();
    let (mut b, _) = sample();
    b.reverse(); // order in does not matter: dots are sorted by id
    layout(&mut a, &lines, 800.0, 600.0);
    layout(&mut b, &lines, 800.0, 600.0);
    assert_eq!(a.len(), b.len());
    for (x, y) in a.iter().zip(&b) {
        assert_eq!(x.id, y.id);
        assert_eq!(x.x.to_bits(), y.x.to_bits(), "{}", x.id);
        assert_eq!(x.y.to_bits(), y.y.to_bits(), "{}", x.id);
    }
    let ids: Vec<&str> = a.iter().map(|d| d.id.as_str()).collect();
    let mut sorted = ids.clone();
    sorted.sort();
    assert_eq!(ids, sorted, "sorted by id");
    assert_inside(&a, 800.0, 600.0);
    // Something actually moved: not every dot on one spot.
    let spots: HashSet<(u32, u32)> = a.iter().map(|d| (d.x.to_bits(), d.y.to_bits())).collect();
    assert_eq!(spots.len(), a.len());
}

#[test]
fn layout_uses_the_frame_with_padding() {
    let (mut a, lines) = sample();
    layout(&mut a, &lines, 800.0, 600.0);
    let min_x = a.iter().map(|d| d.x).fold(f32::INFINITY, f32::min);
    let max_x = a.iter().map(|d| d.x).fold(f32::NEG_INFINITY, f32::max);
    let min_y = a.iter().map(|d| d.y).fold(f32::INFINITY, f32::min);
    let max_y = a.iter().map(|d| d.y).fold(f32::NEG_INFINITY, f32::max);
    assert!((min_x - 34.0).abs() < 0.01 && (max_x - 766.0).abs() < 0.01);
    assert!((min_y - 34.0).abs() < 0.01 && (max_y - 566.0).abs() < 0.01);
}

#[test]
fn layout_handles_zero_one_and_two_dots() {
    let mut none: Vec<Dot> = vec![];
    layout(&mut none, &[], 300.0, 200.0);

    let mut one = vec![dot("d:1", DotKind::Disease, 15.0)];
    layout(&mut one, &[], 300.0, 200.0);
    assert_eq!((one[0].x, one[0].y), (150.0, 100.0), "single dot at centre");

    let mut two = vec![
        dot("d:1", DotKind::Disease, 15.0),
        dot("r:1", DotKind::Run, 9.0),
    ];
    layout(&mut two, &[line("d:1", "r:1", false)], 300.0, 200.0);
    assert_inside(&two, 300.0, 200.0);
    assert_ne!((two[0].x, two[0].y), (two[1].x, two[1].y));
}

#[test]
fn layout_separates_two_dots_with_the_same_start() {
    // The same id twice: the same start rule, the same radius.
    let mut two = vec![
        dot("p:1", DotKind::Paper, 5.0),
        dot("p:1", DotKind::Paper, 5.0),
    ];
    layout(&mut two, &[], 300.0, 200.0);
    assert_inside(&two, 300.0, 200.0);
    // A tiny frame still gives values inside it.
    let mut tiny = vec![
        dot("p:1", DotKind::Paper, 5.0),
        dot("p:2", DotKind::Paper, 5.0),
        dot("p:3", DotKind::Paper, 5.0),
    ];
    layout(&mut tiny, &[], 20.0, 10.0);
    assert_inside(&tiny, 20.0, 10.0);
}

// ---------- load_map (database) ----------

async fn connect() -> Option<PgPool> {
    let Ok(url) = std::env::var("TEST_DATABASE_URL") else {
        eprintln!("SKIP: TEST_DATABASE_URL not set");
        return None;
    };
    let pool = lazy_pool(&SecretString::from(url)).expect("lazy pool");
    migrate(&pool).await.expect("migrate");
    Some(pool)
}

async fn seed_user(pool: &PgPool) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO incisive.users (email, display_name, role, daily_cap_paise) \
         VALUES ($1, 'Graph', 'surgeon', 100000) RETURNING id",
    )
    .bind(format!("read-{}@example.test", Uuid::new_v4()))
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn seed_run(pool: &PgPool, user: i64, mode: &str) -> Uuid {
    let run = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO incisive.runs (id, user_id, question, mode, status, limit_paise, created_at) \
         VALUES ($1, $2, 'q', $3, 'done', 1000, '2026-11-06 06:00:00+00')",
    )
    .bind(run)
    .bind(user)
    .bind(mode)
    .execute(pool)
    .await
    .unwrap();
    run
}

struct Pick<'a> {
    organ: &'a str,
    disease: &'a str,
    treatments: &'a [&'a str],
    outcome: (&'a str, &'a str),
}

fn picks(tag: &str, p: &Pick) -> Picks {
    Picks {
        diseases: vec![format!("{} {tag}", p.disease)],
        treatments: p.treatments.iter().map(|t| format!("{t} {tag}")).collect(),
        organs: vec![format!("{} {tag}", p.organ)],
        outcomes: vec![PickedOutcome {
            row: 0,
            name: format!("{} {tag}", p.outcome.0),
            certainty: p.outcome.1.into(),
        }],
        dropped: 0,
    }
}

fn paper(pmid: &str, year: i32, design: &str) -> PaperRow {
    PaperRow {
        pmid: pmid.into(),
        title: format!("Title {pmid}"),
        journal: None,
        year: Some(year),
        doi: None,
        retracted: false,
        rank: rank_paper(design, &[]),
        design: design.into(),
        n: None,
    }
}

/// PMIDs no other test uses.
fn pmid(tag: &str, i: u32) -> String {
    format!("{}{i:04}", &tag[..8])
}

fn ids(m: &MapData) -> HashSet<String> {
    m.dots.iter().map(|d| d.id.clone()).collect()
}

fn of_kind(m: &MapData, k: DotKind) -> Vec<&Dot> {
    m.dots.iter().filter(|d| d.kind == k).collect()
}

fn find<'a>(m: &'a MapData, id: &str) -> &'a Dot {
    m.dots
        .iter()
        .find(|d| d.id == id)
        .unwrap_or_else(|| panic!("no dot {id}"))
}

fn has_line(m: &MapData, a: &str, b: &str) -> bool {
    m.lines.iter().any(|l| l.a == a && l.b == b)
}

async fn concept(pool: &PgPool, kind: &str, label: &str) -> i64 {
    sqlx::query_scalar(
        "SELECT id FROM incisive.graph_concepts WHERE kind = $1 AND norm_label = lower($2)",
    )
    .bind(kind)
    .bind(label)
    .fetch_one(pool)
    .await
    .unwrap()
}

const HERNIA: Pick = Pick {
    organ: "Abdominal wall",
    disease: "Inguinal hernia",
    treatments: &["Mesh repair"],
    outcome: ("Recurrence", "Moderate"),
};

/// A's map carries none of B's runs, concepts or paper links, whatever the
/// filter, and B's organ id gives A nothing.
#[tokio::test]
async fn a_user_never_reaches_another_users_dots() {
    let _g = SERIAL.lock().await;
    let Some(pool) = connect().await else { return };
    let (a, b) = (seed_user(&pool).await, seed_user(&pool).await);
    let tag = Uuid::new_v4().simple().to_string();
    let tag_b = format!("{tag}b");
    let ra = seed_run(&pool, a, "quick").await;
    let rb = seed_run(&pool, b, "quick").await;
    write_run(
        &pool,
        ra,
        &picks(&tag, &HERNIA),
        &[paper(&pmid(&tag, 1), 2020, "randomised trial")],
    )
    .await
    .unwrap();
    let b_pick = Pick {
        organ: "Stomach",
        disease: "Gastric cancer",
        treatments: &["Gastrectomy", "Mesh repair"],
        outcome: ("Survival", "Low"),
    };
    // B shares one treatment name with A (the shared concept table) and adds
    // its own concepts and a paper.
    let mut pb = picks(&tag_b, &b_pick);
    pb.treatments.push(format!("Mesh repair {tag}"));
    write_run(
        &pool,
        rb,
        &pb,
        &[paper(&pmid(&tag, 2), 2021, "retrospective cohort")],
    )
    .await
    .unwrap();

    let b_organ = concept(&pool, "organ", &format!("Stomach {tag_b}")).await;
    let b_only: HashSet<String> = [
        format!("r:{rb}"),
        format!("g:{b_organ}"),
        format!(
            "d:{}",
            concept(&pool, "disease", &format!("Gastric cancer {tag_b}")).await
        ),
        format!(
            "t:{}",
            concept(&pool, "treatment", &format!("Gastrectomy {tag_b}")).await
        ),
        format!(
            "o:{}",
            concept(&pool, "outcome", &format!("Survival {tag_b}")).await
        ),
        format!("p:{}", pmid(&tag, 2)),
    ]
    .into();

    let filters = [
        MapFilter::default(),
        MapFilter {
            organ: Some(b_organ),
            ..Default::default()
        },
        MapFilter {
            certainty: Some("Low".into()),
            ..Default::default()
        },
        MapFilter {
            year_from: Some(2021),
            year_to: Some(2021),
            ..Default::default()
        },
        MapFilter {
            hidden: vec!["rct".into()],
            ..Default::default()
        },
    ];
    for f in &filters {
        let m = load_map(&pool, a, f).await.unwrap();
        let got = ids(&m);
        assert!(got.is_disjoint(&b_only), "{f:?}: B's dots leaked");
        for l in &m.lines {
            assert!(
                !b_only.contains(&l.a) && !b_only.contains(&l.b),
                "{f:?}: B's line leaked"
            );
        }
        let mesh = concept(&pool, "treatment", &format!("Mesh repair {tag}")).await;
        assert!(
            !has_line(&m, &format!("r:{rb}"), &format!("t:{mesh}")),
            "{f:?}: B's link to a shared concept leaked"
        );
    }
    let by_b_organ = load_map(
        &pool,
        a,
        &MapFilter {
            organ: Some(b_organ),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert!(by_b_organ.dots.is_empty() && by_b_organ.lines.is_empty());
    assert_eq!(by_b_organ.runs, 0);

    // A sees its own map, and B its own.
    let ma = load_map(&pool, a, &MapFilter::default()).await.unwrap();
    assert_eq!(ma.runs, 1);
    assert_eq!(ma.dots.len(), 6);
    let mb = load_map(&pool, b, &MapFilter::default()).await.unwrap();
    assert!(b_only.is_subset(&ids(&mb)));
    assert!(!ids(&mb).contains(&format!("r:{ra}")));
    assert!(!ids(&mb).contains(&format!("p:{}", pmid(&tag, 1))));
}

/// Dot ids, labels, subs, sizes and lines for one ordinary map.
#[tokio::test]
async fn dots_and_lines_follow_the_rules() {
    let _g = SERIAL.lock().await;
    let Some(pool) = connect().await else { return };
    let u = seed_user(&pool).await;
    let tag = Uuid::new_v4().simple().to_string();
    let r1 = seed_run(&pool, u, "quick").await;
    let r2 = seed_run(&pool, u, "focused").await;
    let p1 = pmid(&tag, 1);
    let p2 = pmid(&tag, 2);
    write_run(
        &pool,
        r1,
        &picks(&tag, &HERNIA),
        &[
            paper(&p1, 2019, "randomised trial"),
            paper(&p2, 2015, "case series"),
        ],
    )
    .await
    .unwrap();
    let second = Pick {
        organ: "Abdominal wall",
        disease: "Inguinal hernia",
        treatments: &["Mesh repair", "Tissue repair"],
        outcome: ("Chronic pain", "Low"),
    };
    write_run(
        &pool,
        r2,
        &picks(&tag, &second),
        &[paper(&p2, 2015, "case series")],
    )
    .await
    .unwrap();

    let m = load_map(&pool, u, &MapFilter::default()).await.unwrap();
    assert_eq!(m.runs, 2);
    assert!(!m.papers_hidden_by_size);
    let organ = concept(&pool, "organ", &format!("Abdominal wall {tag}")).await;
    let disease = concept(&pool, "disease", &format!("Inguinal hernia {tag}")).await;
    let mesh = concept(&pool, "treatment", &format!("Mesh repair {tag}")).await;
    let tissue = concept(&pool, "treatment", &format!("Tissue repair {tag}")).await;
    let pain = concept(&pool, "outcome", &format!("Chronic pain {tag}")).await;
    let (g, d, t, tt, o) = (
        format!("g:{organ}"),
        format!("d:{disease}"),
        format!("t:{mesh}"),
        format!("t:{tissue}"),
        format!("o:{pain}"),
    );
    let (run1, run2) = (format!("r:{r1}"), format!("r:{r2}"));
    // organ, disease, 2 runs, 2 treatments, 2 outcomes, 2 papers
    assert_eq!(m.dots.len(), 10, "{:?}", ids(&m));

    let dd = find(&m, &d);
    assert_eq!(dd.kind, DotKind::Disease);
    assert_eq!(dd.label, format!("Inguinal hernia {tag}"));
    assert_eq!(dd.sub, format!("Abdominal wall {tag}"));
    assert_eq!(dd.r, 15.0);
    assert_eq!(dd.concept_id, Some(disease));
    assert_eq!(dd.first_year, Some(2015));
    assert_eq!(find(&m, &g).r, 12.0);

    // A treatment in two runs: one dot, two lines, bigger.
    let td = find(&m, &t);
    assert_eq!(td.r, 14.0);
    assert!(has_line(&m, &run1, &t) && has_line(&m, &run2, &t));
    assert_eq!(m.lines.iter().filter(|l| l.b == t).count(), 2);
    assert_eq!(find(&m, &tt).r, 11.0);

    let rd = find(&m, &run1);
    assert_eq!(rd.kind, DotKind::Run);
    assert_eq!(rd.label, "Your question");
    assert_eq!(rd.sub, "Quick · 6 Nov 2026");
    assert_eq!(rd.run_id, Some(r1));
    assert_eq!(rd.r, 9.0);
    assert_eq!(rd.first_year, Some(2015));
    assert_eq!(find(&m, &run2).sub, "Full · 6 Nov 2026");

    let od = find(&m, &o);
    assert_eq!((od.sub.as_str(), od.r), ("Low", 8.0));

    let pd = find(&m, &format!("p:{p1}"));
    assert_eq!(pd.kind, DotKind::Paper);
    assert_eq!(pd.label, format!("Title {p1}"));
    assert_eq!(pd.sub, "RCT · 2019");
    assert_eq!(pd.rank.as_deref(), Some("rct"));
    assert_eq!(pd.first_year, Some(2019));
    assert_eq!(pd.pmid.as_deref(), Some(p1.as_str()));
    assert_eq!(pd.r, 8.0);
    assert_eq!(find(&m, &format!("p:{p2}")).r, 4.5);

    assert!(has_line(&m, &g, &d));
    assert_eq!(m.lines.iter().filter(|l| l.a == g && l.b == d).count(), 1);
    assert!(has_line(&m, &d, &run1) && has_line(&m, &d, &run2));
    assert!(has_line(&m, &run2, &o) && !has_line(&m, &run1, &o));
    let pl = m
        .lines
        .iter()
        .find(|l| l.a == run2 && l.b == format!("p:{p2}"))
        .unwrap();
    assert!(pl.paper);
    assert!(!m.lines.iter().find(|l| l.a == g).unwrap().paper);
    for l in &m.lines {
        assert!(ids(&m).contains(&l.a) && ids(&m).contains(&l.b), "{l:?}");
    }
}

#[tokio::test]
async fn each_filter_narrows() {
    let _g = SERIAL.lock().await;
    let Some(pool) = connect().await else { return };
    let u = seed_user(&pool).await;
    let tag = Uuid::new_v4().simple().to_string();
    let r1 = seed_run(&pool, u, "quick").await;
    let r2 = seed_run(&pool, u, "quick").await;
    let (p1, p2, p3) = (pmid(&tag, 1), pmid(&tag, 2), pmid(&tag, 3));
    write_run(
        &pool,
        r1,
        &picks(&tag, &HERNIA),
        &[
            paper(&p1, 2010, "randomised trial"),
            paper(&p2, 2020, "retrospective cohort"),
        ],
    )
    .await
    .unwrap();
    let gastric = Pick {
        organ: "Stomach",
        disease: "Gastric cancer",
        treatments: &["Gastrectomy"],
        outcome: ("Survival", "Low"),
    };
    write_run(
        &pool,
        r2,
        &picks(&tag, &gastric),
        &[paper(&p3, 2024, "randomised trial")],
    )
    .await
    .unwrap();
    let stomach = concept(&pool, "organ", &format!("Stomach {tag}")).await;
    let run1 = format!("r:{r1}");
    let run2 = format!("r:{r2}");

    let all = load_map(&pool, u, &MapFilter::default()).await.unwrap();
    assert_eq!(all.runs, 2);
    assert_eq!(all.dots.len(), 13);

    // Organ: only the stomach run and what hangs off it.
    let m = load_map(
        &pool,
        u,
        &MapFilter {
            organ: Some(stomach),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(m.runs, 1);
    assert!(ids(&m).contains(&run2) && !ids(&m).contains(&run1));
    assert_eq!(m.dots.len(), 6);

    // Year range: papers only.
    let m = load_map(
        &pool,
        u,
        &MapFilter {
            year_from: Some(2015),
            year_to: Some(2022),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let papers: HashSet<String> = of_kind(&m, DotKind::Paper)
        .iter()
        .map(|d| d.id.clone())
        .collect();
    assert_eq!(papers, HashSet::from([format!("p:{p2}")]));
    assert_eq!(m.runs, 2);
    assert_eq!(m.dots.len(), 11);
    let m = load_map(
        &pool,
        u,
        &MapFilter {
            year_from: Some(2021),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(of_kind(&m, DotKind::Paper).len(), 1);
    let m = load_map(
        &pool,
        u,
        &MapFilter {
            year_to: Some(2010),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(find(&m, &format!("p:{p1}")).kind, DotKind::Paper);
    assert_eq!(of_kind(&m, DotKind::Paper).len(), 1);

    // Certainty: runs with an outcome of that certainty (any case).
    let m = load_map(
        &pool,
        u,
        &MapFilter {
            certainty: Some("moderate".into()),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(m.runs, 1);
    assert!(ids(&m).contains(&run1) && !ids(&m).contains(&run2));
    let m = load_map(
        &pool,
        u,
        &MapFilter {
            certainty: Some("High".into()),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert!(m.dots.is_empty());
    assert_eq!(m.runs, 0);

    // Hidden kinds and ranks; lines to hidden dots go too.
    let m = load_map(
        &pool,
        u,
        &MapFilter {
            hidden: vec!["treatment".into(), "outcome".into()],
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert!(of_kind(&m, DotKind::Treatment).is_empty());
    assert!(of_kind(&m, DotKind::Outcome).is_empty());
    assert_eq!(m.dots.len(), 9);
    assert_eq!(m.runs, 2);
    let m = load_map(
        &pool,
        u,
        &MapFilter {
            hidden: vec!["rct".into()],
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let papers: Vec<&str> = of_kind(&m, DotKind::Paper)
        .iter()
        .map(|d| d.id.as_str())
        .collect();
    assert_eq!(papers, [format!("p:{p2}")]);
    for l in &m.lines {
        assert!(ids(&m).contains(&l.a) && ids(&m).contains(&l.b), "{l:?}");
    }
}

#[tokio::test]
async fn a_merged_concept_shows_once_under_its_final_name() {
    let _g = SERIAL.lock().await;
    let Some(pool) = connect().await else { return };
    let u = seed_user(&pool).await;
    let tag = Uuid::new_v4().simple().to_string();
    let r1 = seed_run(&pool, u, "quick").await;
    let r2 = seed_run(&pool, u, "quick").await;
    let old = Pick {
        disease: "Groin hernia",
        ..HERNIA
    };
    write_run(&pool, r1, &picks(&tag, &old), &[]).await.unwrap();
    write_run(&pool, r2, &picks(&tag, &HERNIA), &[])
        .await
        .unwrap();
    let old_id = concept(&pool, "disease", &format!("Groin hernia {tag}")).await;
    let new_id = concept(&pool, "disease", &format!("Inguinal hernia {tag}")).await;
    sqlx::query("UPDATE incisive.graph_concepts SET merged_into = $2 WHERE id = $1")
        .bind(old_id)
        .bind(new_id)
        .execute(&pool)
        .await
        .unwrap();

    let m = load_map(&pool, u, &MapFilter::default()).await.unwrap();
    let diseases = of_kind(&m, DotKind::Disease);
    assert_eq!(diseases.len(), 1);
    assert_eq!(diseases[0].id, format!("d:{new_id}"));
    assert_eq!(diseases[0].label, format!("Inguinal hernia {tag}"));
    assert!(has_line(&m, &format!("d:{new_id}"), &format!("r:{r1}")));
    assert!(has_line(&m, &format!("d:{new_id}"), &format!("r:{r2}")));
    assert!(!ids(&m).contains(&format!("d:{old_id}")));
}

#[tokio::test]
async fn a_deleted_run_disappears() {
    let _g = SERIAL.lock().await;
    let Some(pool) = connect().await else { return };
    let u = seed_user(&pool).await;
    let tag = Uuid::new_v4().simple().to_string();
    let r1 = seed_run(&pool, u, "quick").await;
    let r2 = seed_run(&pool, u, "quick").await;
    let gastric = Pick {
        organ: "Stomach",
        disease: "Gastric cancer",
        treatments: &["Gastrectomy"],
        outcome: ("Survival", "Low"),
    };
    write_run(
        &pool,
        r1,
        &picks(&tag, &HERNIA),
        &[paper(&pmid(&tag, 1), 2020, "randomised trial")],
    )
    .await
    .unwrap();
    write_run(&pool, r2, &picks(&tag, &gastric), &[])
        .await
        .unwrap();
    sqlx::query("DELETE FROM incisive.runs WHERE id = $1")
        .bind(r1)
        .execute(&pool)
        .await
        .unwrap();
    let m = load_map(&pool, u, &MapFilter::default()).await.unwrap();
    assert_eq!(m.runs, 1);
    assert_eq!(m.dots.len(), 5, "{:?}", ids(&m));
    assert!(!ids(&m).contains(&format!("r:{r1}")));
    assert!(!ids(&m).contains(&format!("p:{}", pmid(&tag, 1))));
    assert!(
        of_kind(&m, DotKind::Disease)[0]
            .label
            .starts_with("Gastric cancer")
    );
}

#[tokio::test]
async fn a_big_map_leaves_papers_out_unless_the_user_chose_what_to_hide() {
    let _g = SERIAL.lock().await;
    let Some(pool) = connect().await else { return };
    let u = seed_user(&pool).await;
    let tag = Uuid::new_v4().simple().to_string();
    let r = seed_run(&pool, u, "quick").await;
    let papers: Vec<PaperRow> = (0..400)
        .map(|i| paper(&pmid(&tag, i), 2020, "randomised trial"))
        .collect();
    write_run(&pool, r, &picks(&tag, &HERNIA), &papers)
        .await
        .unwrap();

    // 5 + 400 dots.
    let m = load_map(&pool, u, &MapFilter::default()).await.unwrap();
    assert!(m.papers_hidden_by_size);
    assert!(of_kind(&m, DotKind::Paper).is_empty());
    assert_eq!(m.dots.len(), 5);
    assert!(m.lines.iter().all(|l| !l.paper));
    assert_eq!(m.runs, 1);

    let m = load_map(
        &pool,
        u,
        &MapFilter {
            hidden: vec!["outcome".into()],
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert!(!m.papers_hidden_by_size);
    assert_eq!(of_kind(&m, DotKind::Paper).len(), 400);
    assert_eq!(m.dots.len(), 404);

    // The size is counted after the filters: a year range that leaves no
    // papers is not "hidden by size".
    let m = load_map(
        &pool,
        u,
        &MapFilter {
            year_from: Some(2021),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert!(!m.papers_hidden_by_size);
    assert_eq!(m.dots.len(), 5);
}

/// A run with no disease still ties its organ to it, so no dot floats free.
#[tokio::test]
async fn an_organ_without_a_disease_hangs_from_the_run() {
    let _g = SERIAL.lock().await;
    let Some(pool) = connect().await else { return };
    let u = seed_user(&pool).await;
    let tag = Uuid::new_v4().simple().to_string();
    let r = seed_run(&pool, u, "quick").await;
    let mut p = picks(&tag, &HERNIA);
    p.diseases.clear();
    write_run(&pool, r, &p, &[]).await.unwrap();
    let organ = concept(&pool, "organ", &format!("Abdominal wall {tag}")).await;
    let m = load_map(&pool, u, &MapFilter::default()).await.unwrap();
    assert!(has_line(&m, &format!("g:{organ}"), &format!("r:{r}")));
    assert_eq!(m.dots.len(), 4);
}
