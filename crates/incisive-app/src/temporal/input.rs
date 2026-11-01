//! The slim input view ([`TemporalInput`]): the parts of a stored answer the
//! temporal table reads.
//!
//! It deserialises from a stored `run_answers.body` (the full AnswerV3, with
//! the writer's part nested under `answer`) and from the slim projection
//! (the same fields at the top level, which is also what [`TemporalInput`]
//! serialises to). Unknown fields are ignored; nulls in text fields read as
//! empty text, so stored data is never trusted to be complete.

use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

/// What the temporal table reads from one stored answer.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(from = "RawInput")]
pub struct TemporalInput {
    pub question: String,
    /// The run's creation time as stored (RFC 3339); its year ends the
    /// latest era.
    pub created_at: Option<String>,
    pub outcomes: Vec<InOutcome>,
    pub studies: Vec<InStudy>,
    pub guidelines: Vec<InGuideline>,
    pub references: Vec<InReference>,
    pub skeptic: Option<InSkeptic>,
}

impl TemporalInput {
    /// Reads a stored answer body or a slim projection.
    pub fn from_json(json: &str) -> Result<TemporalInput, serde_json::Error> {
        serde_json::from_str(json)
    }

    /// The year of `created_at` (its first four characters), if readable.
    pub fn run_year(&self) -> Option<i32> {
        let s = self.created_at.as_deref()?.trim();
        let y = s.get(..4)?;
        if y.chars().all(|c| c.is_ascii_digit()) {
            y.parse().ok()
        } else {
            None
        }
    }
}

/// Both shapes: the AnswerV3 nesting (`answer.outcomes`) and the slim
/// top-level fields. The nested writer's part wins when both are present.
#[derive(Deserialize)]
struct RawInput {
    #[serde(default, deserialize_with = "text")]
    question: String,
    #[serde(default, deserialize_with = "opt_text")]
    created_at: Option<String>,
    #[serde(default)]
    answer: Option<RawAnswer>,
    #[serde(default, deserialize_with = "list")]
    outcomes: Vec<InOutcome>,
    #[serde(default, deserialize_with = "list")]
    studies: Vec<InStudy>,
    #[serde(default, deserialize_with = "list")]
    guidelines: Vec<InGuideline>,
    #[serde(default, deserialize_with = "list")]
    references: Vec<InReference>,
    #[serde(default)]
    skeptic: Option<InSkeptic>,
}

#[derive(Deserialize)]
struct RawAnswer {
    #[serde(default, deserialize_with = "list")]
    outcomes: Vec<InOutcome>,
    #[serde(default, deserialize_with = "list")]
    studies: Vec<InStudy>,
    #[serde(default, deserialize_with = "list")]
    guidelines: Vec<InGuideline>,
}

impl From<RawInput> for TemporalInput {
    fn from(r: RawInput) -> TemporalInput {
        let (outcomes, studies, guidelines) = match r.answer {
            Some(a) => (a.outcomes, a.studies, a.guidelines),
            None => (r.outcomes, r.studies, r.guidelines),
        };
        TemporalInput {
            question: r.question,
            created_at: r.created_at,
            outcomes,
            studies,
            guidelines,
            references: r.references,
            skeptic: r.skeptic,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct InOutcome {
    #[serde(default, deserialize_with = "text")]
    pub outcome: String,
    #[serde(default, deserialize_with = "text")]
    pub evidence_type: String,
    #[serde(default, deserialize_with = "text")]
    pub certainty: String,
    #[serde(default, deserialize_with = "text_list")]
    pub pmids: Vec<String>,
    #[serde(default, deserialize_with = "list")]
    pub numbers: Vec<InNumber>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct InNumber {
    #[serde(default, deserialize_with = "text")]
    pub label: String,
    #[serde(default)]
    pub value: Option<f64>,
    #[serde(default, deserialize_with = "opt_text")]
    pub unit: Option<String>,
    #[serde(default, deserialize_with = "opt_text")]
    pub arm: Option<String>,
    #[serde(default)]
    pub ci_low: Option<f64>,
    #[serde(default)]
    pub ci_high: Option<f64>,
    #[serde(default, deserialize_with = "opt_text")]
    pub p: Option<String>,
    #[serde(default, deserialize_with = "text")]
    pub pmid: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct InStudy {
    #[serde(default, deserialize_with = "text")]
    pub pmid: String,
    #[serde(default, deserialize_with = "text")]
    pub label: String,
    #[serde(default, deserialize_with = "text")]
    pub design: String,
    #[serde(default)]
    pub n: Option<i64>,
    #[serde(default, deserialize_with = "text")]
    pub setting: String,
    #[serde(default, deserialize_with = "text")]
    pub comparison: String,
    #[serde(default, deserialize_with = "text")]
    pub key_results: String,
    #[serde(default, deserialize_with = "text")]
    pub appraisal: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct InGuideline {
    #[serde(default, deserialize_with = "opt_text")]
    pub pmid: Option<String>,
    #[serde(default, deserialize_with = "text")]
    pub body: String,
    #[serde(default)]
    pub year: Option<i32>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct InReference {
    #[serde(default)]
    pub n: Option<u16>,
    #[serde(default, deserialize_with = "text")]
    pub pmid: String,
    #[serde(default)]
    pub year: Option<i32>,
    #[serde(default)]
    pub found_by_skeptic: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct InSkeptic {
    #[serde(default, deserialize_with = "list")]
    pub overlaps: Vec<InOverlap>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct InOverlap {
    #[serde(default, deserialize_with = "text_list")]
    pub pmids: Vec<String>,
    #[serde(default, deserialize_with = "text")]
    pub note: String,
}

/// A JSON scalar as text: strings as written, numbers in their shortest
/// form, null and anything else as `None`.
fn scalar_text(v: Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

fn text<'de, D: Deserializer<'de>>(d: D) -> Result<String, D::Error> {
    Ok(scalar_text(Value::deserialize(d)?).unwrap_or_default())
}

fn opt_text<'de, D: Deserializer<'de>>(d: D) -> Result<Option<String>, D::Error> {
    Ok(scalar_text(Value::deserialize(d)?))
}

fn text_list<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<String>, D::Error> {
    Ok(match Value::deserialize(d)? {
        Value::Array(xs) => xs.into_iter().filter_map(scalar_text).collect(),
        other => scalar_text(other).into_iter().collect(),
    })
}

/// A list that may be null.
fn list<'de, D: Deserializer<'de>, T: Deserialize<'de>>(d: D) -> Result<Vec<T>, D::Error> {
    Ok(Option::<Vec<T>>::deserialize(d)?.unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_full_answer_shape() {
        let j = r#"{
            "version": 3, "question": "Q", "mode": "full",
            "created_at": "2026-10-31T10:00:00Z",
            "answer": {
                "bottom_line": {"text": "x"},
                "outcomes": [{"outcome": "Conversion", "numbers": [
                    {"label": "OR conversion", "value": 1.61, "unit": null,
                     "arm": "robotic vs laparoscopic", "ci_low": 1.28,
                     "ci_high": 2.03, "p": "<0.001", "pmid": "1"}],
                    "pmids": ["1"], "certainty": "Low", "evidence_type": "observational"}],
                "studies": [{"pmid": "1", "label": "A", "design": "cohort", "n": null,
                    "setting": "2012-2021", "comparison": "", "key_results": "",
                    "appraisal": ""}],
                "guidelines": [{"pmid": null, "body": "SAGES", "year": 2024, "statement": "s"}]
            },
            "references": [{"n": 1, "pmid": "1", "title": "t", "year": 2025,
                "found_by_skeptic": false, "retracted": false}],
            "skeptic": {"verdict": "holds", "overlaps": [{"pmids": ["1", "2"], "note": "n"}]}
        }"#;
        let i = TemporalInput::from_json(j).unwrap();
        assert_eq!(i.question, "Q");
        assert_eq!(i.run_year(), Some(2026));
        assert_eq!(i.outcomes.len(), 1);
        assert_eq!(i.outcomes[0].numbers[0].value, Some(1.61));
        assert_eq!(i.studies[0].setting, "2012-2021");
        assert_eq!(i.guidelines[0].year, Some(2024));
        assert_eq!(i.references[0].year, Some(2025));
        assert_eq!(i.skeptic.unwrap().overlaps[0].pmids, vec!["1", "2"]);
    }

    #[test]
    fn reads_the_slim_shape_and_round_trips() {
        let j = r#"{"question": "Q", "created_at": "2025-01-01",
            "outcomes": [{"outcome": "O", "numbers": []}],
            "studies": [{"pmid": 123, "label": "L", "design": null}],
            "references": [{"pmid": "123", "year": 2020}]}"#;
        let i = TemporalInput::from_json(j).unwrap();
        assert_eq!(i.studies[0].pmid, "123");
        assert_eq!(i.studies[0].design, "");
        let back = serde_json::to_string(&i).unwrap();
        assert_eq!(TemporalInput::from_json(&back).unwrap(), i);
    }

    #[test]
    fn an_empty_answer_reads_as_empty() {
        let j = r#"{"question": "Q", "answer": null, "references": null}"#;
        let i = TemporalInput::from_json(j).unwrap();
        assert!(i.outcomes.is_empty() && i.references.is_empty());
        assert_eq!(i.run_year(), None);
    }
}
