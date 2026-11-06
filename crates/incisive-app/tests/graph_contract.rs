//! What the graph reads from a stored answer, and that the graph module
//! stays independent of the pages.

use incisive_app::agent::answer_v3::AnswerV3;
use serde_json::Value;

const FIXTURES: [&str; 3] = ["hernia", "cholecystitis", "achalasia"];

fn load(name: &str) -> (Value, AnswerV3) {
    let p = format!(
        "{}/tests/fixtures/temporal/full/{name}.json",
        env!("CARGO_MANIFEST_DIR")
    );
    let text = std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{p}: {e}"));
    let raw: Value = serde_json::from_str(&text).unwrap();
    let a: AnswerV3 = serde_json::from_str(&text)
        .unwrap_or_else(|e| panic!("{name}: no longer reads as AnswerV3: {e}"));
    (raw, a)
}

fn need(v: &Value, path: &str, ctx: &str) {
    assert!(
        v.get(path).is_some(),
        "{ctx}: field `{path}` is missing; the graph reads it, so a rename must be handled in src/graph"
    );
}

#[test]
fn stored_answers_carry_every_field_the_graph_reads() {
    for name in FIXTURES {
        let (raw, a) = load(name);
        need(&raw, "question", name);
        let ans = raw
            .get("answer")
            .unwrap_or_else(|| panic!("{name}: no answer"));
        assert!(a.answer.is_some(), "{name}: answer is empty");
        let bl = ans
            .get("bottom_line")
            .unwrap_or_else(|| panic!("{name}: bottom_line"));
        need(bl, "text", name);
        let outs = ans["outcomes"]
            .as_array()
            .unwrap_or_else(|| panic!("{name}: outcomes"));
        assert!(!outs.is_empty(), "{name}: no outcomes");
        for o in outs {
            need(o, "outcome", name);
            need(o, "certainty", name);
        }
        let studies = ans["studies"]
            .as_array()
            .unwrap_or_else(|| panic!("{name}: studies"));
        assert!(!studies.is_empty(), "{name}: no studies");
        for s in studies {
            for f in ["pmid", "design", "n"] {
                need(s, f, &format!("{name} studies[]"));
            }
        }
        let refs = raw["references"]
            .as_array()
            .unwrap_or_else(|| panic!("{name}: references"));
        assert!(!refs.is_empty(), "{name}: no references");
        for r in refs {
            for f in ["pmid", "title", "journal", "year", "doi", "retracted"] {
                need(r, f, &format!("{name} references[]"));
            }
        }
    }
}

#[test]
fn graph_sources_do_not_reach_into_the_pages() {
    const FORBIDDEN: [&str; 6] = [
        "crate::app",
        "answer_page",
        "expert_page",
        "library",
        "leptos",
        "chat_page",
    ];
    let dir = format!("{}/src/graph", env!("CARGO_MANIFEST_DIR"));
    let mut seen = 0;
    for e in std::fs::read_dir(&dir).unwrap() {
        let path = e.unwrap().path();
        if path.extension().is_none_or(|x| x != "rs") {
            continue;
        }
        seen += 1;
        let text = std::fs::read_to_string(&path).unwrap();
        for w in FORBIDDEN {
            assert!(!text.contains(w), "{} contains `{w}`", path.display());
        }
    }
    assert!(seen >= 4, "scan found only {seen} files");
}
