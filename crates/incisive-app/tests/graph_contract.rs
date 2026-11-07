//! What the graph reads from a stored answer, and that the graph module
//! stays independent of the pages.

use incisive_app::agent::answer_v3::AnswerV3;
use serde_json::Value;

const FIXTURES: [&str; 3] = ["hernia", "cholecystitis", "achalasia"];

fn load(name: &str) -> AnswerV3 {
    let p = format!(
        "{}/tests/fixtures/temporal/full/{name}.json",
        env!("CARGO_MANIFEST_DIR")
    );
    let text = std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{p}: {e}"));
    serde_json::from_str(&text)
        .unwrap_or_else(|e| panic!("{name}: no longer reads as AnswerV3: {e}"))
}

/// Reads every field the graph needs through the typed answer, so a Rust-side
/// rename stops compiling here, naming the field.
fn read_fields(name: &str, a: &AnswerV3) {
    assert!(!a.question.is_empty(), "{name}: question");
    let w = a
        .answer
        .as_ref()
        .unwrap_or_else(|| panic!("{name}: answer is empty"));
    assert!(
        !w.bottom_line.text.is_empty(),
        "{name}: answer.bottom_line.text"
    );
    assert!(!w.outcomes.is_empty(), "{name}: no outcomes");
    for o in &w.outcomes {
        assert!(!o.outcome.is_empty(), "{name}: outcomes[].outcome");
        assert!(
            !o.certainty.as_str().is_empty(),
            "{name}: outcomes[].certainty"
        );
        // The map's outcome -> paper lines.
        let _: &Vec<String> = &o.pmids;
    }
    assert!(!w.studies.is_empty(), "{name}: no studies");
    for s in &w.studies {
        let _: (&String, &String, &Option<i64>) = (&s.pmid, &s.design, &s.n);
        assert!(!s.pmid.is_empty(), "{name}: studies[].pmid");
    }
    assert!(!a.references.is_empty(), "{name}: no references");
    for r in &a.references {
        let _: (
            &String,
            &String,
            &Option<String>,
            &Option<i32>,
            &Option<String>,
            &bool,
        ) = (&r.pmid, &r.title, &r.journal, &r.year, &r.doi, &r.retracted);
        assert!(
            !r.pmid.is_empty() && !r.title.is_empty(),
            "{name}: references[]"
        );
    }
}

/// The serialised form carries the same names (the stored JSON is what is
/// read back), so a `serde(rename)` that changes a key fails with its name.
fn keys_survive_a_round_trip(name: &str, a: &AnswerV3) {
    let v = serde_json::to_value(a).unwrap();
    let need = |o: &Value, k: &str, ctx: &str| {
        assert!(
            o.get(k).is_some(),
            "{name}: `{ctx}.{k}` is missing from the stored form"
        )
    };
    need(&v, "question", "");
    let ans = &v["answer"];
    need(&ans["bottom_line"], "text", "answer.bottom_line");
    for o in ans["outcomes"].as_array().unwrap() {
        need(o, "outcome", "outcomes[]");
        need(o, "certainty", "outcomes[]");
        need(o, "pmids", "outcomes[]");
    }
    for s in ans["studies"].as_array().unwrap() {
        for k in ["pmid", "design", "n"] {
            need(s, k, "studies[]");
        }
    }
    for r in v["references"].as_array().unwrap() {
        for k in ["pmid", "title", "journal", "year", "doi", "retracted"] {
            need(r, k, "references[]");
        }
    }
}

#[test]
fn stored_answers_carry_every_field_the_graph_reads() {
    for name in FIXTURES {
        let a = load(name);
        read_fields(name, &a);
        keys_survive_a_round_trip(name, &a);
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
    fn walk(dir: &std::path::Path, files: &mut Vec<std::path::PathBuf>) {
        for e in std::fs::read_dir(dir).unwrap() {
            let path = e.unwrap().path();
            if path.is_dir() {
                walk(&path, files);
            } else if path.extension().is_some_and(|x| x == "rs") {
                files.push(path);
            }
        }
    }
    let dir = format!("{}/src/graph", env!("CARGO_MANIFEST_DIR"));
    let mut files = Vec::new();
    walk(std::path::Path::new(&dir), &mut files);
    assert!(files.len() >= 4, "scan found only {} files", files.len());
    for path in &files {
        let text = std::fs::read_to_string(path).unwrap();
        for w in FORBIDDEN {
            assert!(!text.contains(w), "{} contains `{w}`", path.display());
        }
    }
}
