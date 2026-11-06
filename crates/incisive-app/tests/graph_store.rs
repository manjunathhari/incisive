//! Writing a run's concepts, papers and links (`graph::store`). The pure
//! paper rows need no database; the rest use `TEST_DATABASE_URL`.

use std::collections::HashMap;

use incisive_app::agent::answer_v3::AnswerV3;
use incisive_app::db::{lazy_pool, migrate};
use incisive_app::graph::extract::{PickedOutcome, Picks};
use incisive_app::graph::rank::Rank;
use incisive_app::graph::store::{
    PaperRow, cleanup_unused, known_names, papers_from, pub_types_for, write_run,
};
use secrecy::SecretString;
use sqlx::PgPool;
use uuid::Uuid;

/// The database tests share one concept table and `cleanup_unused` is global,
/// so they run one at a time.
static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn hernia() -> AnswerV3 {
    let p = format!(
        "{}/tests/fixtures/temporal/full/hernia.json",
        env!("CARGO_MANIFEST_DIR")
    );
    serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap()
}

#[test]
fn papers_collapse_duplicates_and_skip_blank_pmids() {
    let mut a = hernia();
    let first = a.references[0].clone();
    let mut dup = first.clone();
    dup.title = "second copy".into();
    let mut blank = first.clone();
    blank.pmid = "  ".into();
    let total = a.references.len();
    a.references.push(dup);
    a.references.push(blank);
    let rows = papers_from(&a, &HashMap::new());
    assert_eq!(rows.len(), total);
    assert_eq!(rows.iter().filter(|r| r.pmid == first.pmid).count(), 1);
    assert_eq!(
        rows.iter().find(|r| r.pmid == first.pmid).unwrap().title,
        first.title
    );
}

#[test]
fn study_row_gives_design_and_n_and_missing_row_ranks_from_types() {
    let mut a = hernia();
    let study = a.answer.as_ref().unwrap().studies[0].clone();
    // A reference with no study row.
    let mut extra = a.references[0].clone();
    extra.pmid = "99999991".into();
    let mut plain = a.references[0].clone();
    plain.pmid = "99999992".into();
    a.references.push(extra);
    a.references.push(plain);
    let types = HashMap::from([(
        "99999991".to_string(),
        vec!["Randomized Controlled Trial".to_string()],
    )]);
    let rows = papers_from(&a, &types);
    let with_study = rows.iter().find(|r| r.pmid == study.pmid).unwrap();
    assert_eq!(with_study.design, study.design);
    assert_eq!(with_study.n, study.n);
    assert_eq!(with_study.rank.rank, Rank::Rct);
    let from_types = rows.iter().find(|r| r.pmid == "99999991").unwrap();
    assert_eq!(from_types.design, "");
    assert_eq!(from_types.n, None);
    assert_eq!(from_types.rank.rank, Rank::Rct);
    let none = rows.iter().find(|r| r.pmid == "99999992").unwrap();
    assert_eq!(none.rank.rank, Rank::Unclassified);
}

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
    .bind(format!("store-{}@example.test", Uuid::new_v4()))
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn seed_run(pool: &PgPool, user: i64) -> Uuid {
    let run = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO incisive.runs (id, user_id, question, mode, status, limit_paise) \
         VALUES ($1, $2, 'q', 'quick', 'done', 1000)",
    )
    .bind(run)
    .bind(user)
    .execute(pool)
    .await
    .unwrap();
    run
}

/// A label no other test uses.
fn uniq(s: &str, tag: &str) -> String {
    format!("{s} {tag}")
}

fn picks(tag: &str) -> Picks {
    Picks {
        diseases: vec![uniq("Inguinal hernia", tag)],
        treatments: vec![uniq("Mesh repair", tag)],
        organs: vec![],
        outcomes: vec![PickedOutcome {
            row: 2,
            name: uniq("Recurrence", tag),
            certainty: "moderate".into(),
        }],
        dropped: 0,
    }
}

fn paper(pmid: &str, title: &str, retracted: bool) -> PaperRow {
    PaperRow {
        pmid: pmid.into(),
        title: title.into(),
        journal: Some("J".into()),
        year: Some(2024),
        doi: None,
        retracted,
        rank: incisive_app::graph::rank::rank_paper("randomised trial", &[]),
        design: "randomised trial".into(),
        n: Some(50),
    }
}

async fn count(pool: &PgPool, sql: &str, run: Uuid) -> i64 {
    sqlx::query_scalar(sql)
        .bind(run)
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn concept_id(pool: &PgPool, kind: &str, norm: &str) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO incisive.graph_concepts (kind, label, norm_label) VALUES ($1, $2, $2) \
         ON CONFLICT (kind, norm_label) DO UPDATE SET label = EXCLUDED.label RETURNING id",
    )
    .bind(kind)
    .bind(norm)
    .fetch_one(pool)
    .await
    .unwrap()
}

#[tokio::test]
async fn write_run_is_idempotent_and_takes_user_id_from_the_run() {
    let _g = SERIAL.lock().await;
    let Some(pool) = connect().await else { return };
    let (a, b) = (seed_user(&pool).await, seed_user(&pool).await);
    let (ra, rb) = (seed_run(&pool, a).await, seed_run(&pool, b).await);
    let tag = Uuid::new_v4().to_string();
    let papers = vec![paper("88000001", "T1", false)];
    for _ in 0..2 {
        write_run(&pool, ra, &picks(&tag), &papers).await.unwrap();
    }
    write_run(&pool, rb, &picks(&tag), &papers).await.unwrap();
    assert_eq!(
        count(
            &pool,
            "SELECT count(*) FROM incisive.graph_run_concepts WHERE run_id=$1",
            ra
        )
        .await,
        3
    );
    assert_eq!(
        count(
            &pool,
            "SELECT count(*) FROM incisive.graph_run_papers WHERE run_id=$1",
            ra
        )
        .await,
        1
    );
    for (run, user) in [(ra, a), (rb, b)] {
        let wrong: i64 = sqlx::query_scalar(
            "SELECT (SELECT count(*) FROM incisive.graph_run_concepts WHERE run_id=$1 AND user_id<>$2) \
             + (SELECT count(*) FROM incisive.graph_run_papers WHERE run_id=$1 AND user_id<>$2)",
        )
        .bind(run)
        .bind(user)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(wrong, 0);
    }
    // Same normalised label across runs: one concept.
    let shared: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM incisive.graph_concepts WHERE kind='disease' AND norm_label=$1",
    )
    .bind(format!("inguinal hernia {tag}"))
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(shared, 1);
    // Outcome keeps certainty and source row.
    let (cert, row): (Option<String>, Option<i32>) = sqlx::query_as(
        "SELECT rc.certainty, rc.source_row FROM incisive.graph_run_concepts rc \
         JOIN incisive.graph_concepts c ON c.id = rc.concept_id \
         WHERE rc.run_id=$1 AND c.kind='outcome'",
    )
    .bind(ra)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!((cert.as_deref(), row), (Some("moderate"), Some(2)));
}

#[tokio::test]
async fn a_rewrite_replaces_links_and_updates_title_and_retracted() {
    let _g = SERIAL.lock().await;
    let Some(pool) = connect().await else { return };
    let u = seed_user(&pool).await;
    let run = seed_run(&pool, u).await;
    let tag = Uuid::new_v4().to_string();
    let pmid = format!("88{}", &tag[..6]);
    write_run(&pool, run, &picks(&tag), &[paper(&pmid, "Old", false)])
        .await
        .unwrap();
    let mut p2 = picks(&tag);
    p2.treatments.clear();
    write_run(&pool, run, &p2, &[paper(&pmid, "New", true)])
        .await
        .unwrap();
    assert_eq!(
        count(
            &pool,
            "SELECT count(*) FROM incisive.graph_run_concepts WHERE run_id=$1",
            run
        )
        .await,
        2
    );
    let (title, retracted): (String, bool) =
        sqlx::query_as("SELECT title, retracted FROM incisive.graph_papers WHERE pmid=$1")
            .bind(&pmid)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!((title.as_str(), retracted), ("New", true));
}

#[tokio::test]
async fn merged_labels_link_to_the_final_concept_and_collisions_do_not_error() {
    let _g = SERIAL.lock().await;
    let Some(pool) = connect().await else { return };
    let u = seed_user(&pool).await;
    let run = seed_run(&pool, u).await;
    let tag = Uuid::new_v4().to_string();
    let (a, b, c) = (uniq("alpha", &tag), uniq("beta", &tag), uniq("gamma", &tag));
    let (ia, ib, ic) = (
        concept_id(&pool, "disease", &a).await,
        concept_id(&pool, "disease", &b).await,
        concept_id(&pool, "disease", &c).await,
    );
    // alpha -> beta -> gamma (a chain).
    sqlx::query("UPDATE incisive.graph_concepts SET merged_into=$2 WHERE id=$1")
        .bind(ia)
        .bind(ib)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("UPDATE incisive.graph_concepts SET merged_into=$2 WHERE id=$1")
        .bind(ib)
        .bind(ic)
        .execute(&pool)
        .await
        .unwrap();
    // Picks alpha and beta: both collapse onto gamma.
    let p = Picks {
        diseases: vec![a.clone(), b.clone()],
        ..Picks::default()
    };
    write_run(&pool, run, &p, &[]).await.unwrap();
    let linked: Vec<i64> =
        sqlx::query_scalar("SELECT concept_id FROM incisive.graph_run_concepts WHERE run_id=$1")
            .bind(run)
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(linked, vec![ic]);
}

#[tokio::test]
async fn cleanup_removes_only_unused_names() {
    let _g = SERIAL.lock().await;
    let Some(pool) = connect().await else { return };
    let u = seed_user(&pool).await;
    let tag = Uuid::new_v4().to_string();
    let (keep_run, drop_run) = (seed_run(&pool, u).await, seed_run(&pool, u).await);
    write_run(
        &pool,
        keep_run,
        &Picks {
            diseases: vec![uniq("kept", &tag)],
            ..Picks::default()
        },
        &[],
    )
    .await
    .unwrap();
    write_run(
        &pool,
        drop_run,
        &Picks {
            diseases: vec![uniq("gone", &tag)],
            ..Picks::default()
        },
        &[],
    )
    .await
    .unwrap();
    // A name with no links that another concept merges into.
    let target = concept_id(&pool, "disease", &uniq("target", &tag)).await;
    let source = concept_id(&pool, "disease", &uniq("source", &tag)).await;
    sqlx::query("UPDATE incisive.graph_concepts SET merged_into=$2 WHERE id=$1")
        .bind(source)
        .bind(target)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM incisive.runs WHERE id=$1")
        .bind(drop_run)
        .execute(&pool)
        .await
        .unwrap();
    let removed = cleanup_unused(&pool).await.unwrap();
    assert!(removed >= 2, "removed {removed}");
    let exists = |n: String| {
        let pool = pool.clone();
        async move {
            sqlx::query_scalar::<_, bool>(
                "SELECT EXISTS (SELECT 1 FROM incisive.graph_concepts WHERE norm_label=$1)",
            )
            .bind(n)
            .fetch_one(&pool)
            .await
            .unwrap()
        }
    };
    assert!(exists(format!("kept {tag}")).await);
    assert!(!exists(format!("gone {tag}")).await);
    assert!(exists(format!("target {tag}")).await);
    assert!(!exists(format!("source {tag}")).await);
}

#[tokio::test]
async fn known_names_are_the_users_own_most_used_first() {
    let _g = SERIAL.lock().await;
    let Some(pool) = connect().await else { return };
    let (a, b) = (seed_user(&pool).await, seed_user(&pool).await);
    let tag = Uuid::new_v4().to_string();
    for _ in 0..2 {
        let r = seed_run(&pool, a).await;
        write_run(
            &pool,
            r,
            &Picks {
                diseases: vec![uniq("common", &tag)],
                ..Picks::default()
            },
            &[],
        )
        .await
        .unwrap();
    }
    let r = seed_run(&pool, a).await;
    write_run(
        &pool,
        r,
        &Picks {
            diseases: vec![uniq("rare", &tag)],
            ..Picks::default()
        },
        &[],
    )
    .await
    .unwrap();
    let r = seed_run(&pool, b).await;
    write_run(
        &pool,
        r,
        &Picks {
            diseases: vec![uniq("theirs", &tag)],
            ..Picks::default()
        },
        &[],
    )
    .await
    .unwrap();
    let names = known_names(&pool, a).await.unwrap();
    let mine: Vec<&String> = names
        .diseases
        .iter()
        .filter(|n| n.ends_with(&tag))
        .collect();
    assert_eq!(mine.len(), 2);
    assert!(mine[0].starts_with("common"));
    assert!(!names.diseases.iter().any(|n| n.starts_with("theirs")));
}

#[tokio::test]
async fn pub_types_come_from_the_paper_cache() {
    let _g = SERIAL.lock().await;
    let Some(pool) = connect().await else { return };
    let pmid = format!("77{}", &Uuid::new_v4().simple().to_string()[..6]);
    sqlx::query(
        "INSERT INTO incisive.paper_cache (pmid, title, authors, pub_types, fetched_at) \
         VALUES ($1, 't', 'a', ARRAY['Meta-Analysis','Review'], now())",
    )
    .bind(&pmid)
    .execute(&pool)
    .await
    .unwrap();
    let got = pub_types_for(&pool, &[pmid.clone(), "nope".into()])
        .await
        .unwrap();
    assert_eq!(got.len(), 1);
    assert_eq!(
        got[&pmid],
        vec!["Meta-Analysis".to_string(), "Review".to_string()]
    );
    assert!(pub_types_for(&pool, &[]).await.unwrap().is_empty());
}
