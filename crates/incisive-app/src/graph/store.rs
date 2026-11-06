//! Writes one run's concepts, papers and links. Concepts and papers are
//! shared; the links belong to the run and its user. Names are never logged.

use std::collections::{HashMap, HashSet};

use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

use super::extract::{KnownNames, Picks, norm_label};
use super::rank::{Ranked, rank_paper};
use crate::agent::answer_v3::AnswerV3;

/// Most names offered back to the model, per kind.
const KNOWN_CAP: i64 = 100;
/// Guard against a `merged_into` cycle.
pub(super) const MAX_CHAIN: i32 = 20;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaperRow {
    pub pmid: String,
    pub title: String,
    pub journal: Option<String>,
    pub year: Option<i32>,
    pub doi: Option<String>,
    pub retracted: bool,
    pub rank: Ranked,
    pub design: String,
    pub n: Option<i64>,
}

/// One row per reference with a non-blank PMID, first copy kept. Design and n
/// come from the study row; with none, the rank comes from publication types.
pub fn papers_from(a: &AnswerV3, pub_types: &HashMap<String, Vec<String>>) -> Vec<PaperRow> {
    let studies = a
        .answer
        .as_ref()
        .map(|w| w.studies.as_slice())
        .unwrap_or(&[]);
    let mut seen: HashSet<&str> = HashSet::new();
    let mut out = Vec::new();
    for r in &a.references {
        let pmid = r.pmid.trim();
        if pmid.is_empty() || !seen.insert(pmid) {
            continue;
        }
        let study = studies.iter().find(|s| s.pmid.trim() == pmid);
        let (design, n) = study.map(|s| (s.design.clone(), s.n)).unwrap_or_default();
        let types = pub_types.get(pmid).map(Vec::as_slice).unwrap_or(&[]);
        out.push(PaperRow {
            pmid: pmid.to_string(),
            title: r.title.clone(),
            journal: r.journal.clone(),
            year: r.year,
            doi: r.doi.clone(),
            retracted: r.retracted,
            rank: rank_paper(&design, types),
            design,
            n,
        });
    }
    out
}

/// Publication types from the paper cache, for the PMIDs it holds.
pub async fn pub_types_for(
    pool: &PgPool,
    pmids: &[String],
) -> sqlx::Result<HashMap<String, Vec<String>>> {
    if pmids.is_empty() {
        return Ok(HashMap::new());
    }
    let rows: Vec<(String, Vec<String>)> =
        sqlx::query_as("SELECT pmid, pub_types FROM incisive.paper_cache WHERE pmid = ANY($1)")
            .bind(pmids)
            .fetch_all(pool)
            .await?;
    Ok(rows.into_iter().collect())
}

/// The concept a name lands on: upsert by (kind, norm_label), then follow
/// `merged_into` to the end of the chain.
async fn final_concept(
    tx: &mut Transaction<'_, Postgres>,
    kind: &str,
    label: &str,
) -> sqlx::Result<i64> {
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO incisive.graph_concepts (kind, label, norm_label) VALUES ($1, $2, $3) \
         ON CONFLICT (kind, norm_label) DO UPDATE SET norm_label = EXCLUDED.norm_label \
         RETURNING id",
    )
    .bind(kind)
    .bind(label)
    .bind(norm_label(label))
    .fetch_one(&mut **tx)
    .await?;
    sqlx::query_scalar(
        "WITH RECURSIVE ch(id, merged_into, d) AS ( \
           SELECT id, merged_into, 0 FROM incisive.graph_concepts WHERE id = $1 \
           UNION ALL \
           SELECT c.id, c.merged_into, ch.d + 1 FROM incisive.graph_concepts c \
             JOIN ch ON c.id = ch.merged_into WHERE ch.d < $2) \
         SELECT id FROM ch ORDER BY d DESC LIMIT 1",
    )
    .bind(id)
    .bind(MAX_CHAIN)
    .fetch_one(&mut **tx)
    .await
}

async fn link_concept(
    tx: &mut Transaction<'_, Postgres>,
    run_id: Uuid,
    kind: &str,
    label: &str,
    certainty: Option<&str>,
    source_row: Option<i32>,
) -> sqlx::Result<()> {
    let concept = final_concept(tx, kind, label).await?;
    sqlx::query(
        "INSERT INTO incisive.graph_run_concepts (run_id, user_id, concept_id, certainty, source_row) \
         SELECT r.id, r.user_id, $2, $3, $4 FROM incisive.runs r WHERE r.id = $1 \
         ON CONFLICT DO NOTHING",
    )
    .bind(run_id)
    .bind(concept)
    .bind(certainty)
    .bind(source_row)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// One concept link [`write_run`] writes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConceptLink<'a> {
    pub kind: &'static str,
    pub label: &'a str,
    pub certainty: Option<&'a str>,
    pub source_row: Option<i32>,
}

/// The order [`write_run`] writes in: concepts by (kind, normalised label),
/// papers by PMID, whatever order they were picked in, so two runs written
/// at once take the shared rows' locks in the same order and cannot
/// deadlock.
pub fn write_order<'a>(
    picks: &'a Picks,
    papers: &'a [PaperRow],
) -> (Vec<ConceptLink<'a>>, Vec<&'a PaperRow>) {
    let mut concepts = Vec::new();
    for (kind, names) in [
        ("disease", &picks.diseases),
        ("treatment", &picks.treatments),
        ("organ", &picks.organs),
    ] {
        for name in names {
            concepts.push(ConceptLink {
                kind,
                label: name,
                certainty: None,
                source_row: None,
            });
        }
    }
    for o in &picks.outcomes {
        concepts.push(ConceptLink {
            kind: "outcome",
            label: &o.name,
            certainty: Some(&o.certainty),
            source_row: i32::try_from(o.row).ok(),
        });
    }
    concepts.sort_by_cached_key(|c| (c.kind, norm_label(c.label), c.label, c.source_row));
    let mut papers: Vec<&PaperRow> = papers.iter().collect();
    papers.sort_by(|a, b| a.pmid.cmp(&b.pmid));
    (concepts, papers)
}

/// Store a run's picks and papers in one transaction, replacing any links an
/// earlier attempt wrote, in [`write_order`]. The user comes from the run,
/// never from the caller. A run that is gone is `Err(RowNotFound)` and
/// nothing is written; the run's row is key-share locked until commit, so it
/// cannot be deleted mid-write.
pub async fn write_run(
    pool: &PgPool,
    run_id: Uuid,
    picks: &Picks,
    papers: &[PaperRow],
) -> sqlx::Result<()> {
    let mut tx = pool.begin().await?;
    let exists: Option<i32> =
        sqlx::query_scalar("SELECT 1 FROM incisive.runs WHERE id = $1 FOR KEY SHARE")
            .bind(run_id)
            .fetch_optional(&mut *tx)
            .await?;
    if exists.is_none() {
        return Err(sqlx::Error::RowNotFound);
    }
    sqlx::query("DELETE FROM incisive.graph_run_concepts WHERE run_id = $1")
        .bind(run_id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM incisive.graph_run_papers WHERE run_id = $1")
        .bind(run_id)
        .execute(&mut *tx)
        .await?;

    let (concepts, papers) = write_order(picks, papers);
    for c in &concepts {
        link_concept(&mut tx, run_id, c.kind, c.label, c.certainty, c.source_row).await?;
    }

    for p in papers {
        sqlx::query(
            "INSERT INTO incisive.graph_papers (pmid, title, journal, year, doi, retracted) \
             VALUES ($1, $2, $3, $4, $5, $6) \
             ON CONFLICT (pmid) DO UPDATE SET title = EXCLUDED.title, \
               retracted = EXCLUDED.retracted, updated_at = now()",
        )
        .bind(&p.pmid)
        .bind(&p.title)
        .bind(&p.journal)
        .bind(p.year)
        .bind(&p.doi)
        .bind(p.retracted)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "INSERT INTO incisive.graph_run_papers \
               (run_id, user_id, pmid, rank, rank_label, includes_non_rct, design, n) \
             SELECT r.id, r.user_id, $2, $3, $4, $5, $6, $7 FROM incisive.runs r WHERE r.id = $1 \
             ON CONFLICT DO NOTHING",
        )
        .bind(run_id)
        .bind(&p.pmid)
        .bind(p.rank.rank.as_str())
        .bind(p.rank.label)
        .bind(p.rank.includes_non_rct)
        .bind(&p.design)
        .bind(p.n)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await
}

/// The user's own names, most used first, at most 100 per kind, each
/// resolved to the concept it was merged into.
pub async fn known_names(pool: &PgPool, user_id: i64) -> sqlx::Result<KnownNames> {
    let rows: Vec<(String, String)> = sqlx::query_as(
        "WITH RECURSIVE walk(start, id, merged_into, d) AS ( \
           SELECT c.id, c.id, c.merged_into, 0 FROM incisive.graph_concepts c \
            WHERE c.id IN (SELECT concept_id FROM incisive.graph_run_concepts WHERE user_id = $1) \
           UNION ALL \
           SELECT w.start, c.id, c.merged_into, w.d + 1 FROM walk w \
             JOIN incisive.graph_concepts c ON c.id = w.merged_into WHERE w.d < $2), \
         fin AS (SELECT DISTINCT ON (start) start, id FROM walk ORDER BY start, d DESC), \
         used AS ( \
           SELECT c.kind, c.label, count(*) AS n, \
                  row_number() OVER (PARTITION BY c.kind ORDER BY count(*) DESC, c.label) AS rn \
             FROM incisive.graph_run_concepts rc \
             JOIN fin ON fin.start = rc.concept_id \
             JOIN incisive.graph_concepts c ON c.id = fin.id \
            WHERE rc.user_id = $1 AND c.kind IN ('disease','treatment','outcome') \
            GROUP BY c.id, c.kind, c.label) \
         SELECT kind, label FROM used WHERE rn <= $3 ORDER BY kind, rn",
    )
    .bind(user_id)
    .bind(MAX_CHAIN)
    .bind(KNOWN_CAP)
    .fetch_all(pool)
    .await?;
    let mut out = KnownNames::default();
    for (kind, label) in rows {
        match kind.as_str() {
            "disease" => out.diseases.push(label),
            "treatment" => out.treatments.push(label),
            _ => out.outcomes.push(label),
        }
    }
    Ok(out)
}

/// Delete concepts with no links that no other concept merges into.
pub async fn cleanup_unused(pool: &PgPool) -> sqlx::Result<u64> {
    let r = sqlx::query(
        "DELETE FROM incisive.graph_concepts c \
          WHERE NOT EXISTS (SELECT 1 FROM incisive.graph_run_concepts rc WHERE rc.concept_id = c.id) \
            AND NOT EXISTS (SELECT 1 FROM incisive.graph_concepts m WHERE m.merged_into = c.id)",
    )
    .execute(pool)
    .await?;
    Ok(r.rows_affected())
}
