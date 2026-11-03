//! Pure extraction of the memory record from a stored Full run: the topic
//! label, intervention-comparator pairs, ledger studies and per-era claims.
//! No I/O.

use serde_json::Value;

use crate::agent::temporal_writer::ExpertBoardV1;
use crate::temporal::TemporalTable;

#[derive(Debug, Clone, PartialEq)]
pub struct LedgerStudy {
    pub pmid: String,
    pub pub_year: Option<i32>,
    pub treatment_start: Option<i32>,
    pub treatment_end: Option<i32>,
    pub years_assumed: bool,
    pub era_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LedgerClaim {
    /// `GroupResult.id`: one outcome x arms x family x unit x polarity.
    pub group_id: String,
    pub outcome: String,
    pub intervention: String,
    pub comparator: String,
    pub era_id: String,
    pub era_start: i32,
    pub era_end: i32,
    pub estimate: Option<f64>,
    pub lo: Option<f64>,
    pub hi: Option<f64>,
    pub effect_type: String,
    pub unit_key: String,
    /// Serde string of the group's polarity.
    pub polarity: String,
    pub n_studies: i32,
    pub label: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Extracted {
    /// Board topic, trimmed, clipped to 200 characters.
    pub topic_label: String,
    /// Normalised (intervention, comparator), sorted, deduped.
    pub pairs: Vec<(String, String)>,
    pub studies: Vec<LedgerStudy>,
    pub claims: Vec<LedgerClaim>,
    /// Outcome names and the first at-a-glance line, space-joined.
    pub fts_extra: String,
    /// Topic label, pairs and outcome names for the embedding.
    pub embed_text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExtractError {
    NotV3,
    NoAnswer,
    BadTable,
    BadBoard,
    NoTopic,
}

impl ExtractError {
    pub fn code(&self) -> &'static str {
        match self {
            ExtractError::NotV3 => "not_v3",
            ExtractError::NoAnswer => "no_answer",
            ExtractError::BadTable => "bad_table",
            ExtractError::BadBoard => "bad_board",
            ExtractError::NoTopic => "no_topic",
        }
    }
}

/// Lowercase, drop any parenthesised segment, collapse whitespace, trim.
pub fn normalise_arm(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut depth = 0usize;
    for ch in s.chars() {
        match ch {
            '(' => depth += 1,
            ')' => depth = depth.saturating_sub(1),
            c if depth == 0 => out.push(c),
            _ => {}
        }
    }
    out.to_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// The serde form of a unit enum as text: the string itself when it
/// serialises as a JSON string (both `Family` and `Label` do), else the
/// compact JSON text.
fn serde_text<T: serde::Serialize>(v: &T) -> String {
    match serde_json::to_value(v) {
        Ok(Value::String(s)) => s,
        Ok(other) => other.to_string(),
        Err(_) => String::new(),
    }
}

/// The first non-empty line of the board's at-a-glance (the expert line,
/// else today's, else the original quote).
fn first_glance_line(b: &ExpertBoardV1) -> String {
    let g = &b.at_a_glance;
    [&g.expert, &g.today, &g.original_quote]
        .into_iter()
        .find_map(|t| t.lines().map(str::trim).find(|l| !l.is_empty()))
        .unwrap_or("")
        .to_string()
}

/// The Expert Opinion board: the `ExpertBoardV1` itself, or wrapped as
/// `{"board": {...}}`.
pub fn parse_board(board_json: &Value) -> Result<ExpertBoardV1, ExtractError> {
    let board_val = match board_json.get("board") {
        Some(b) if b.is_object() => b,
        _ => board_json,
    };
    serde_json::from_value(board_val.clone()).map_err(|_| ExtractError::BadBoard)
}

pub fn extract(
    body: &Value,
    table_json: &Value,
    board_json: &Value,
) -> Result<Extracted, ExtractError> {
    if body.get("version").and_then(Value::as_i64) != Some(3) {
        return Err(ExtractError::NotV3);
    }
    if !body.get("answer").is_some_and(Value::is_object) {
        return Err(ExtractError::NoAnswer);
    }
    let table: TemporalTable =
        serde_json::from_value(table_json.clone()).map_err(|_| ExtractError::BadTable)?;
    let board = parse_board(board_json)?;
    let topic = board.topic.trim();
    if topic.is_empty() {
        return Err(ExtractError::NoTopic);
    }
    let topic_label: String = topic.chars().take(200).collect();

    let studies: Vec<LedgerStudy> = table
        .studies
        .iter()
        .filter(|s| !s.pmid.is_empty())
        .map(|s| LedgerStudy {
            pmid: s.pmid.clone(),
            pub_year: s.pub_year,
            treatment_start: s.treatment_start,
            treatment_end: s.treatment_end,
            years_assumed: s.assumed,
            era_id: s.era_id.clone(),
        })
        .collect();

    let mut pairs: Vec<(String, String)> = Vec::new();
    let mut claims: Vec<LedgerClaim> = Vec::new();
    let mut outcome_names: Vec<String> = Vec::new();
    for o in &table.outcomes {
        if !o.outcome.trim().is_empty() && !outcome_names.contains(&o.outcome) {
            outcome_names.push(o.outcome.clone());
        }
        for g in &o.groups {
            let (i, c) = (normalise_arm(&g.intervention), normalise_arm(&g.comparator));
            if !i.is_empty() && !c.is_empty() {
                pairs.push((i, c));
            }
            for cell in g.cells.iter().filter(|c| !c.empty) {
                let Some(era) = table.eras.iter().find(|e| e.id == cell.era_id) else {
                    continue;
                };
                let (estimate, lo, hi) = match &cell.pooled {
                    Some(p) => (Some(p.est), Some(p.lo), Some(p.hi)),
                    None => (cell.estimate, None, None),
                };
                let mut sids: Vec<&str> = table
                    .rows
                    .iter()
                    .filter(|r| cell.row_ids.contains(&r.id))
                    .map(|r| r.study_id.as_str())
                    .collect();
                sids.sort_unstable();
                sids.dedup();
                claims.push(LedgerClaim {
                    group_id: g.id.clone(),
                    outcome: o.outcome.clone(),
                    intervention: g.intervention.clone(),
                    comparator: g.comparator.clone(),
                    era_id: cell.era_id.clone(),
                    era_start: era.start,
                    era_end: era.end,
                    estimate,
                    lo,
                    hi,
                    effect_type: serde_text(&g.family),
                    unit_key: g.unit_key.clone(),
                    polarity: serde_text(&g.polarity),
                    n_studies: sids.len() as i32,
                    label: serde_text(&g.label),
                });
            }
        }
    }
    pairs.sort();
    pairs.dedup();

    let glance = first_glance_line(&board);
    let mut fts = outcome_names.clone();
    // Both arms of every pair, so a run's own topic words (e.g. `lhm`, `pd`)
    // are in the record's full-text and a full-text-only match can find them.
    let arm_words: Vec<&str> = pairs
        .iter()
        .flat_map(|(i, c)| [i.as_str(), c.as_str()])
        .collect();
    if !arm_words.is_empty() {
        fts.push(arm_words.join(" "));
    }
    if !glance.is_empty() {
        fts.push(glance);
    }
    let pair_text = pairs
        .iter()
        .map(|(i, c)| format!("{i} vs {c}"))
        .collect::<Vec<_>>()
        .join("; ");
    let embed_text = format!("{topic_label}\n{pair_text}\n{}", outcome_names.join("; "));

    Ok(Extracted {
        topic_label,
        pairs,
        studies,
        claims,
        fts_extra: fts.join(" "),
        embed_text,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::temporal::{TemporalInput, build};
    use serde_json::json;

    fn dir() -> String {
        format!("{}/tests/fixtures/temporal", env!("CARGO_MANIFEST_DIR"))
    }
    fn read(p: String) -> Value {
        serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap()
    }
    fn table(f: &str) -> TemporalTable {
        let s = std::fs::read_to_string(format!("{}/{f}.json", dir())).unwrap();
        build(&TemporalInput::from_json(&s).unwrap())
    }
    fn board(f: &str) -> Value {
        read(format!("{}/boards/{f}-board.json", dir()))["board"].clone()
    }
    fn body(f: &str) -> Value {
        read(format!("{}/full/{f}.json", dir()))
    }
    fn run(f: &str) -> (TemporalTable, Result<Extracted, ExtractError>) {
        let t = table(f);
        let r = extract(&body(f), &serde_json::to_value(&t).unwrap(), &board(f));
        (t, r)
    }

    #[test]
    fn extract_three_fixtures() {
        for f in ["cholecystitis", "hernia", "achalasia"] {
            let (t, r) = run(f);
            let x = r.unwrap_or_else(|e| panic!("{f}: {e:?}"));
            let n = t.studies.iter().filter(|s| !s.pmid.is_empty()).count();
            assert_eq!(x.studies.len(), n, "{f}");
            for c in &x.claims {
                assert!(t.eras.iter().any(|e| e.id == c.era_id), "{f} {}", c.era_id);
            }
            assert!(!x.pairs.is_empty(), "{f}");
            let mut s = x.pairs.clone();
            s.sort();
            s.dedup();
            assert_eq!(s, x.pairs, "{f}");
            assert!(!x.topic_label.is_empty() && x.topic_label.chars().count() <= 200);
        }
    }

    /// Golden values written from the `build` output of the hernia fixture
    /// (all claims with an estimate; only the pain one is pooled).
    #[test]
    fn claims_match_table() {
        let (_, r) = run("hernia");
        let x = r.unwrap();
        let mut got: Vec<(String, String, f64)> = x
            .claims
            .iter()
            .filter_map(|c| {
                c.estimate
                    .map(|e| (c.outcome.clone(), c.era_id.clone(), (e * 1e4).round() / 1e4))
            })
            .collect();
        got.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let golden: Vec<(String, String, f64)> = [
            ("Cost", "e-2016-2020", 1837.0),
            ("Cost", "e-2021-2026", 595.0),
            ("Operative time", "e-2016-2020", 35.0),
            ("Postoperative and chronic pain", "e-2021-2026", -1.0),
            ("Postoperative and chronic pain", "e-2021-2026", 0.28),
            ("Postoperative and chronic pain", "e-2021-2026", 2.7),
            ("Postoperative complications", "e-2021-2026", -18.5),
            ("Quality of life", "e-2021-2026", -0.01),
            ("Same-day discharge and length of stay", "e-2021-2026", 14.4),
            ("Surgeon workload and ergonomics", "e-2016-2020", 12.6),
            ("Surgeon workload and ergonomics", "e-2021-2026", -15.6),
        ]
        .into_iter()
        .map(|(a, b, c)| (a.to_string(), b.to_string(), c))
        .collect();
        assert_eq!(got, golden);
        let pooled: Vec<_> = x.claims.iter().filter(|c| c.lo.is_some()).collect();
        assert_eq!(pooled.len(), 1);
        assert_eq!(pooled[0].outcome, "Postoperative and chronic pain");
        assert!(pooled[0].lo.unwrap() < pooled[0].hi.unwrap());
        assert!(pooled[0].n_studies >= 1);
    }

    #[test]
    fn fts_extra_carries_pair_words() {
        let (_, r) = run("achalasia");
        let x = r.unwrap();
        for (i, c) in &x.pairs {
            for w in i.split_whitespace().chain(c.split_whitespace()) {
                assert!(x.fts_extra.split_whitespace().any(|t| t == w), "{w}");
            }
        }
        assert!(x.fts_extra.contains("lhm") && x.fts_extra.contains("pd"));
    }

    #[test]
    fn claims_pin_group_identity() {
        use std::collections::HashSet;
        for f in ["cholecystitis", "hernia", "achalasia"] {
            let (t, r) = run(f);
            let x = r.unwrap();
            // Every claim is a distinct (group, era); none is dropped.
            let keys: HashSet<(&str, &str)> = x
                .claims
                .iter()
                .map(|c| (c.group_id.as_str(), c.era_id.as_str()))
                .collect();
            assert_eq!(keys.len(), x.claims.len(), "{f}");
            let cells: usize = t
                .outcomes
                .iter()
                .flat_map(|o| &o.groups)
                .map(|g| {
                    g.cells
                        .iter()
                        .filter(|c| !c.empty && t.eras.iter().any(|e| e.id == c.era_id))
                        .count()
                })
                .sum();
            assert_eq!(x.claims.len(), cells, "{f}");
            assert!(x.claims.iter().all(|c| !c.group_id.is_empty()), "{f}");
        }
        let (_, r) = run("hernia");
        let x = r.unwrap();
        let mut pain: Vec<(String, String, String, f64)> = x
            .claims
            .iter()
            .filter(|c| c.outcome == "Postoperative and chronic pain" && c.era_id == "e-2021-2026")
            .map(|c| {
                (
                    c.group_id.clone(),
                    c.effect_type.clone(),
                    c.unit_key.clone(),
                    (c.estimate.unwrap() * 1e4).round() / 1e4,
                )
            })
            .collect();
        pain.sort_by(|a, b| a.3.partial_cmp(&b.3).unwrap());
        let want: Vec<(&str, &str, &str, f64)> = vec![
            (
                "g-o0-robotic-laparoscopic-diff-nrs-points-higher_worse",
                "difference",
                "nrs-points",
                -1.0,
            ),
            (
                "g-o0-robotic-laparoscopic-diff-points-higher_worse",
                "difference",
                "points",
                0.28,
            ),
            (
                "g-o0-robotic-laparoscopic-diff-pp-higher_worse",
                "difference",
                "pp",
                2.7,
            ),
        ];
        let got: Vec<(&str, &str, &str, f64)> = pain
            .iter()
            .map(|p| (p.0.as_str(), p.1.as_str(), p.2.as_str(), p.3))
            .collect();
        assert_eq!(got, want);
        assert!(
            x.claims
                .iter()
                .all(|c| c.polarity == "higher_worse" || !c.polarity.is_empty())
        );
    }

    #[test]
    fn parse_board_unwraps_and_rejects() {
        let b = board("hernia");
        let bare = parse_board(&b).unwrap();
        assert!(!bare.topic.is_empty());
        assert_eq!(parse_board(&json!({ "board": b })).unwrap(), bare);
        assert_eq!(
            parse_board(&json!({"topic": 5})).unwrap_err(),
            ExtractError::BadBoard
        );
    }

    #[test]
    fn normalise_arm_cases() {
        assert_eq!(normalise_arm("Robotic  (RAS)"), "robotic");
        assert_eq!(normalise_arm(" Laparoscopic "), "laparoscopic");
    }

    #[test]
    fn rejects_v2_body() {
        let t = serde_json::to_value(table("hernia")).unwrap();
        let b = board("hernia");
        let v2 = json!({"version": 2, "answer": {}});
        assert_eq!(extract(&v2, &t, &b).unwrap_err(), ExtractError::NotV3);
        let v3 = body("hernia");
        assert_eq!(
            extract(&v3, &json!({}), &b).unwrap_err(),
            ExtractError::BadTable
        );
        assert_eq!(
            extract(&v3, &t, &json!({})).unwrap_err(),
            ExtractError::NoTopic
        );
        assert_eq!(
            extract(&json!({"version": 3}), &t, &b).unwrap_err(),
            ExtractError::NoAnswer
        );
    }
}
