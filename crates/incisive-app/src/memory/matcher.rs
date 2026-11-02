//! Pure topic matcher: scores how well a new run (a candidate) fits an
//! existing topic. No I/O. Pairs arrive already normalised by
//! `memory::extract::normalise_arm`; this module does not normalise them.

#[derive(Debug, Clone, Copy)]
pub struct Candidate<'a> {
    pub pairs: &'a [(String, String)],
    pub embedding: Option<&'a [f32]>,
    pub fts_rank: f32,
}

#[derive(Debug, Clone)]
pub struct TopicFeatures {
    pub topic_id: i64,
    pub pairs: Vec<(String, String)>,
    pub embedding: Option<Vec<f32>>,
    pub fts_rank: f32,
}

/// NaN counts as 0.0; any other value passes through unchanged.
fn finite(x: f32) -> f32 {
    if x.is_nan() { 0.0 } else { x }
}

/// Canonical form of an unordered pair: the lexically smaller side first.
fn canon(p: &(String, String)) -> (&str, &str) {
    if p.0 <= p.1 {
        (p.0.as_str(), p.1.as_str())
    } else {
        (p.1.as_str(), p.0.as_str())
    }
}

/// Jaccard similarity over sets of unordered pairs: `(x, y)` and `(y, x)`
/// are the same element. Both empty gives 0.0.
pub fn jaccard(a: &[(String, String)], b: &[(String, String)]) -> f32 {
    use std::collections::HashSet;

    let sa: HashSet<(&str, &str)> = a.iter().map(canon).collect();
    let sb: HashSet<(&str, &str)> = b.iter().map(canon).collect();
    if sa.is_empty() && sb.is_empty() {
        return 0.0;
    }
    let inter = sa.intersection(&sb).count();
    let union = sa.len() + sb.len() - inter;
    inter as f32 / union as f32
}

/// Cosine similarity. 0.0 if the lengths differ, either is empty, or a norm
/// is zero. NaN components count as 0.0.
pub fn cosine(a: &[f32], b: &[f32]) -> f32 {
    if a.is_empty() || b.len() != a.len() {
        return 0.0;
    }
    let (mut dot, mut na, mut nb) = (0.0_f32, 0.0_f32, 0.0_f32);
    for (&x, &y) in a.iter().zip(b) {
        let (x, y) = (finite(x), finite(y));
        dot += x * y;
        na += x * x;
        nb += y * y;
    }
    if na == 0.0 || nb == 0.0 {
        return 0.0;
    }
    let r = (dot / (na.sqrt() * nb.sqrt())).clamp(-1.0, 1.0);
    finite(r)
}

/// Weighted score in 0.0..=1.0 (NaN inputs count as 0.0).
///
/// With both embeddings: `0.5*cosine + 0.3*jaccard + 0.2*fts`.
/// Otherwise: `0.6*jaccard + 0.4*fts`.
pub fn score(c: &Candidate, t: &TopicFeatures) -> f32 {
    let jac = jaccard(c.pairs, &t.pairs);
    let fts = finite(c.fts_rank).clamp(0.0, 1.0);
    let raw = match (c.embedding, t.embedding.as_deref()) {
        (Some(ea), Some(eb)) => 0.5 * cosine(ea, eb) + 0.3 * jac + 0.2 * fts,
        _ => 0.6 * jac + 0.4 * fts,
    };
    finite(raw).clamp(0.0, 1.0)
}

/// Highest-scoring topic; ties go to the lowest `topic_id`. `None` if empty.
pub fn best(c: &Candidate, topics: &[TopicFeatures]) -> Option<(i64, f32)> {
    let mut best: Option<(i64, f32)> = None;
    for t in topics {
        let s = score(c, t);
        best = match best {
            None => Some((t.topic_id, s)),
            Some((id, bs)) if s > bs || (s == bs && t.topic_id < id) => Some((t.topic_id, s)),
            keep => keep,
        };
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(x: &str, y: &str) -> (String, String) {
        (x.to_string(), y.to_string())
    }

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-6
    }

    fn topic(
        id: i64,
        pairs: Vec<(String, String)>,
        emb: Option<Vec<f32>>,
        fts: f32,
    ) -> TopicFeatures {
        TopicFeatures {
            topic_id: id,
            pairs,
            embedding: emb,
            fts_rank: fts,
        }
    }

    #[test]
    fn jaccard_order_insensitive() {
        let a = vec![p("robotic", "laparoscopic")];
        let b = vec![p("laparoscopic", "robotic")];
        assert!(close(jaccard(&a, &b), 1.0));
    }

    #[test]
    fn jaccard_disjoint_zero() {
        let a = vec![p("robotic", "laparoscopic")];
        let b = vec![p("open", "laparoscopic")];
        assert!(close(jaccard(&a, &b), 0.0));
    }

    #[test]
    fn jaccard_partial_overlap() {
        // {A,B} vs {B,C}: intersection 1, union 3.
        let a = vec![p("a", "x"), p("b", "x")];
        let b = vec![p("x", "b"), p("c", "x")];
        assert!(close(jaccard(&a, &b), 1.0 / 3.0));
    }

    #[test]
    fn jaccard_both_empty_zero() {
        assert!(close(jaccard(&[], &[]), 0.0));
    }

    #[test]
    fn cosine_mismatched_len_zero() {
        assert!(close(cosine(&[1.0, 0.0], &[1.0, 0.0, 0.0]), 0.0));
    }

    #[test]
    fn cosine_empty_and_zero_norm_are_zero() {
        assert!(close(cosine(&[], &[]), 0.0));
        assert!(close(cosine(&[0.0, 0.0], &[1.0, 2.0]), 0.0));
    }

    #[test]
    fn cosine_identical_is_one() {
        assert!(close(cosine(&[1.0, 2.0, 3.0], &[1.0, 2.0, 3.0]), 1.0));
    }

    #[test]
    fn score_without_embeddings_uses_fallback_weights() {
        let pairs = vec![p("robotic", "laparoscopic")];
        let c = Candidate {
            pairs: &pairs,
            embedding: None,
            fts_rank: 0.5,
        };
        let t = topic(1, pairs.clone(), None, 0.0);
        // 0.6 * jaccard(1) + 0.4 * fts(0.5) = 0.8
        assert!(close(score(&c, &t), 0.8));
    }

    #[test]
    fn score_with_embeddings_uses_primary_weights() {
        let pairs = vec![p("robotic", "laparoscopic")];
        let emb = [1.0_f32, 0.0];
        let c = Candidate {
            pairs: &pairs,
            embedding: Some(&emb),
            fts_rank: 1.0,
        };
        let t = topic(1, vec![p("open", "x")], Some(vec![1.0, 0.0]), 0.0);
        // 0.5 * cosine(1) + 0.3 * jaccard(0) + 0.2 * fts(1) = 0.7
        assert!(close(score(&c, &t), 0.7));
    }

    #[test]
    fn score_clamped_and_nan_safe() {
        let pairs = vec![p("a", "b")];
        let c = Candidate {
            pairs: &pairs,
            embedding: None,
            fts_rank: f32::NAN,
        };
        let t = topic(1, pairs.clone(), None, 0.0);
        // NaN fts treated as 0.0, leaving 0.6 * jaccard(1) = 0.6.
        assert!(close(score(&c, &t), 0.6));

        let c2 = Candidate {
            pairs: &pairs,
            embedding: None,
            fts_rank: 50.0,
        };
        let s = score(&c2, &t);
        assert!((0.0..=1.0).contains(&s));
        assert!(close(s, 1.0));
    }

    #[test]
    fn best_breaks_ties_by_lowest_id() {
        let pairs = vec![p("a", "b")];
        let c = Candidate {
            pairs: &pairs,
            embedding: None,
            fts_rank: 0.0,
        };
        let topics = vec![
            topic(9, pairs.clone(), None, 0.0),
            topic(3, pairs.clone(), None, 0.0),
            topic(7, pairs.clone(), None, 0.0),
        ];
        let (id, s) = best(&c, &topics).expect("some");
        assert_eq!(id, 3);
        assert!(close(s, 0.6));
    }

    #[test]
    fn best_picks_highest_score() {
        let pairs = vec![p("a", "b")];
        let c = Candidate {
            pairs: &pairs,
            embedding: None,
            fts_rank: 0.0,
        };
        let topics = vec![
            topic(1, vec![p("x", "y")], None, 0.0),
            topic(2, pairs.clone(), None, 0.0),
        ];
        assert_eq!(best(&c, &topics).map(|(id, _)| id), Some(2));
    }

    #[test]
    fn best_empty_none() {
        let c = Candidate {
            pairs: &[],
            embedding: None,
            fts_rank: 0.0,
        };
        assert!(best(&c, &[]).is_none());
    }
}
