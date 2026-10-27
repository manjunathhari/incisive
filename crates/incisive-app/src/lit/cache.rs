//! Postgres cache of public bibliographic data (`incisive.paper_cache`).
//! No user linkage and no full texts. Entries are served for 30 days and
//! pruned after 90.
//!
//! The table has no source column, so a cached paper always comes back with
//! `source: PubMed`. `pmcid` was added by migration 0007.

use sqlx::{PgPool, Row};

use super::{Paper, PaperSource};

/// Entries older than this are not served.
pub const FRESH_DAYS: i32 = 30;
/// Entries older than this are deleted by [`prune`].
pub const PRUNE_DAYS: i32 = 90;

/// Fresh (<= 30 days) cached papers for the given PMIDs. Unknown and stale
/// ids are simply absent.
pub async fn get_many(pool: &PgPool, pmids: &[String]) -> sqlx::Result<Vec<Paper>> {
    if pmids.is_empty() {
        return Ok(Vec::new());
    }
    let rows = sqlx::query(
        "SELECT pmid, pmcid, doi, title, authors, journal, year, pub_types, abstract, oa_url, retracted \
         FROM incisive.paper_cache \
         WHERE pmid = ANY($1) AND fetched_at > now() - make_interval(days => $2)",
    )
    .bind(pmids)
    .bind(FRESH_DAYS)
    .fetch_all(pool)
    .await?;
    rows.into_iter()
        .map(|r| {
            let authors: String = r.try_get("authors")?;
            Ok(Paper {
                pmid: Some(r.try_get("pmid")?),
                pmcid: r.try_get("pmcid")?,
                doi: r.try_get("doi")?,
                title: r.try_get("title")?,
                // Stored as a JSON array; a damaged value degrades to no authors.
                authors: serde_json::from_str(&authors).unwrap_or_default(),
                journal: r.try_get("journal")?,
                year: r.try_get("year")?,
                pub_types: r.try_get("pub_types")?,
                abstract_text: r.try_get("abstract")?,
                oa_url: r.try_get("oa_url")?,
                retracted: r.try_get("retracted")?,
                source: PaperSource::PubMed,
            })
        })
        .collect()
}

/// Upserts papers and stamps them as fetched now. A refresh never erases data:
/// a missing `doi`, `pmcid`, `abstract` or `oa_url` keeps the cached value, and
/// `retracted` is sticky once set. Papers without a PMID have
/// no cache key and are skipped.
pub async fn put_many(pool: &PgPool, papers: &[Paper]) -> sqlx::Result<()> {
    let mut tx = pool.begin().await?;
    for p in papers {
        let Some(pmid) = p.pmid.as_deref().map(str::trim).filter(|s| !s.is_empty()) else {
            continue;
        };
        let authors = serde_json::to_string(&p.authors).unwrap_or_else(|_| "[]".into());
        sqlx::query(
            "INSERT INTO incisive.paper_cache \
               (pmid, doi, title, authors, journal, year, pub_types, abstract, oa_url, retracted, pmcid, fetched_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, now()) \
             ON CONFLICT (pmid) DO UPDATE SET \
               doi = COALESCE(EXCLUDED.doi, incisive.paper_cache.doi), \
               pmcid = COALESCE(EXCLUDED.pmcid, incisive.paper_cache.pmcid), title = EXCLUDED.title, authors = EXCLUDED.authors, \
               journal = EXCLUDED.journal, year = EXCLUDED.year, pub_types = EXCLUDED.pub_types, \
               abstract = COALESCE(EXCLUDED.abstract, incisive.paper_cache.abstract), \
               oa_url = COALESCE(EXCLUDED.oa_url, incisive.paper_cache.oa_url), \
               retracted = EXCLUDED.retracted OR incisive.paper_cache.retracted, \
               fetched_at = now()",
        )
        .bind(pmid)
        .bind(&p.doi)
        .bind(&p.title)
        .bind(authors)
        .bind(&p.journal)
        .bind(p.year)
        .bind(&p.pub_types)
        .bind(&p.abstract_text)
        .bind(&p.oa_url)
        .bind(p.retracted)
        .bind(&p.pmcid)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await
}

/// Deletes entries fetched more than 90 days ago. Returns the count.
pub async fn prune(pool: &PgPool) -> sqlx::Result<u64> {
    let r = sqlx::query(
        "DELETE FROM incisive.paper_cache WHERE fetched_at < now() - make_interval(days => $1)",
    )
    .bind(PRUNE_DAYS)
    .execute(pool)
    .await?;
    Ok(r.rows_affected())
}
