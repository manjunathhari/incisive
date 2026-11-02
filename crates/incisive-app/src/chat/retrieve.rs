//! Retrieval: the few passages Gemma may read for one question. One run's
//! record rows plus the topic's approved Q&As, blended from meaning (cosine
//! similarity, when a vector is given) and words (the share of the question's
//! content words found in the text). Gemma sees nothing else.
//!
//! Nothing here logs or returns question or passage text in an error.

use sqlx::{PgPool, Row};

use super::passages::PassageKind;
use crate::memory::store::{fts_words, vec_literal};

/// Hits kept.
pub const TOP_N: usize = 3;
/// A question with fewer content words than this is joined to the previous one.
pub const SHORT_QUESTION_WORDS: usize = 6;
/// Weight of meaning in the blend (words get the rest).
const W_COS: f32 = 0.7;
const W_WORDS: f32 = 0.3;

/// One retrieved passage or approved Q&A. Exactly one of `passage_id` and
/// `qa_id` is set.
#[derive(Debug, Clone, PartialEq)]
pub struct Hit {
    pub passage_id: Option<i64>,
    pub qa_id: Option<i64>,
    pub kind: PassageKind,
    pub text: String,
    pub ref_ns: Vec<i32>,
    pub score: f32,
}

/// With a vector: `0.7 * cos + 0.3 * share`. Without one (the question or the
/// row has none): the word share alone.
pub fn score(cos: Option<f32>, word_share: f32) -> f32 {
    match cos {
        Some(c) => W_COS * c + W_WORDS * word_share,
        None => word_share,
    }
}

/// The text to search with. A short follow-up ("how sure is that?") has too
/// few content words to find anything, so it is joined to the previous
/// question; a longer one stands alone.
pub fn query_text(question: &str, previous: Option<&str>) -> String {
    match previous {
        Some(p) if fts_words(question).len() < SHORT_QUESTION_WORDS => {
            format!("{} {}", p.trim(), question.trim())
        }
        _ => question.trim().to_string(),
    }
}

/// Top hits (at most three) scoring at least `min_score` (and above zero).
///
/// * `record_id`: only this record's passages are read.
/// * `topic_id`: the record's topic; its approved Q&As count, and so do those
///   of every topic it was merged into (`merged_into` chain, cycle-safe).
/// * `asked`: from [`super::comparison::focus`]. A comparison key: a passage
///   tagged with comparisons, none of them this one, is out. A single arm (no
///   `|`): a passage tagged with comparisons, none of them holding this arm,
///   is out. Untagged passages stay; `None` excludes nothing.
/// * `vector`: the question's embedding; a row without an embedding is scored
///   on words alone.
///
/// Ties break passages before Q&As, then lower id first.
pub async fn top(
    pool: &PgPool,
    record_id: i64,
    topic_id: i64,
    asked: Option<&str>,
    query: &str,
    vector: Option<&[f32]>,
    min_score: f32,
) -> sqlx::Result<Vec<Hit>> {
    let words = fts_words(query);
    let vec_text = vector.map(vec_literal);

    // Word share = matching words / words the English parser keeps (stop
    // words like "the" produce an empty query and count for nothing).
    // Matching is any-word, not all: the whole share is the score.
    let share = "COALESCE((SELECT count(*) FILTER (WHERE FTS @@ plainto_tsquery('english', w))::float8 \
                 / NULLIF(count(*) FILTER (WHERE numnode(plainto_tsquery('english', w)) > 0), 0) \
                 FROM unnest($2::text[]) AS w), 0)";
    let passages_sql = format!(
        "SELECT p.id, p.kind, p.text, p.ref_ns, \
                1 - (p.embedding <=> $3::vector) AS cos, {} AS share \
         FROM incisive.chat_passages p \
         WHERE p.record_id = $1 \
           AND ($4::text IS NULL OR cardinality(p.comparisons) = 0 OR $4 = ANY(p.comparisons) \
                OR EXISTS (SELECT 1 FROM unnest(p.comparisons) AS c \
                           WHERE $4 = ANY(string_to_array(c, '|'))))",
        share.replace("FTS", "p.fts")
    );
    let qas_sql = format!(
        "WITH RECURSIVE chain(id, depth) AS ( \
             SELECT id, 0 FROM incisive.topics WHERE id = $1 \
             UNION ALL \
             SELECT t.merged_into, c.depth + 1 FROM incisive.topics t \
             JOIN chain c ON t.id = c.id \
             WHERE t.merged_into IS NOT NULL AND c.depth < 16) \
         SELECT q.id, q.owner_text AS text, \
                1 - (q.embedding <=> $3::vector) AS cos, {} AS share \
         FROM incisive.topic_qas q \
         WHERE q.status = 'approved' AND q.owner_text IS NOT NULL \
           AND q.topic_id IN (SELECT id FROM chain)",
        share.replace("FTS", "q.fts")
    );

    let mut hits: Vec<Hit> = Vec::new();

    let rows = sqlx::query(&passages_sql)
        .bind(record_id)
        .bind(&words)
        .bind(vec_text.as_deref())
        .bind(asked)
        .fetch_all(pool)
        .await?;
    for r in rows {
        let kind: String = r.try_get("kind")?;
        let Some(kind) = kind_of(&kind) else { continue };
        let cos: Option<f64> = r.try_get("cos")?;
        let share: f64 = r.try_get("share")?;
        hits.push(Hit {
            passage_id: Some(r.try_get("id")?),
            qa_id: None,
            kind,
            text: r.try_get("text")?,
            ref_ns: r.try_get("ref_ns")?,
            score: score(cos.map(|c| c as f32), share as f32),
        });
    }

    let rows = sqlx::query(&qas_sql)
        .bind(topic_id)
        .bind(&words)
        .bind(vec_text.as_deref())
        .fetch_all(pool)
        .await?;
    for r in rows {
        let cos: Option<f64> = r.try_get("cos")?;
        let share: f64 = r.try_get("share")?;
        hits.push(Hit {
            passage_id: None,
            qa_id: Some(r.try_get("id")?),
            kind: PassageKind::OwnerQa,
            text: r.try_get("text")?,
            ref_ns: Vec::new(),
            score: score(cos.map(|c| c as f32), share as f32),
        });
    }

    hits.retain(|h| h.score >= min_score && h.score > 0.0);
    hits.sort_by(|a, b| {
        b.score
            .total_cmp(&a.score)
            .then(a.qa_id.is_some().cmp(&b.qa_id.is_some()))
            .then(a.passage_id.cmp(&b.passage_id))
            .then(a.qa_id.cmp(&b.qa_id))
    });
    hits.truncate(TOP_N);
    Ok(hits)
}

fn kind_of(s: &str) -> Option<PassageKind> {
    use PassageKind::*;
    [
        BottomLine,
        Outcome,
        Evidence,
        Conflict,
        Applicability,
        WhatWouldChange,
        Guideline,
        ClinicalPractice,
        Notable,
        Opinion,
        Abstract,
        Quote,
        Reference,
        OwnerQa,
        Belief,
    ]
    .into_iter()
    .find(|k| k.as_str() == s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn score_blends_meaning_and_words() {
        assert!((score(Some(0.5), 1.0) - (0.7 * 0.5 + 0.3)).abs() < 1e-6);
        assert!((score(Some(1.0), 0.0) - 0.7).abs() < 1e-6);
        assert_eq!(score(None, 0.4), 0.4);
    }

    #[test]
    fn short_follow_up_includes_previous_question() {
        let q = query_text("how sure is that?", Some("Is robotic hernia repair safer?"));
        assert!(q.contains("robotic hernia"));
        assert!(q.contains("how sure"));
    }

    #[test]
    fn long_question_stands_alone() {
        let q = query_text(
            "what are the recurrence rates after robotic repair here",
            Some("earlier question"),
        );
        assert!(!q.contains("earlier"));
        assert_eq!(query_text("how sure is that?", None), "how sure is that?");
    }

    #[test]
    fn six_content_words_is_not_short() {
        // exactly 6 words: stands alone; 5 words: joined
        assert!(!query_text("one two three four five six", Some("prev")).contains("prev"));
        assert!(query_text("one two three four five", Some("prev")).contains("prev"));
    }
}
