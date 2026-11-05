//! Stage S1: the reader on every prefix of real replies: the stored fixture
//! answers and boards, as the writers' replies carry them. No database.

use incisive_app::draft_page::{read_answer, read_opinion, read_prefix};
use serde_json::{Value, json};

const NAMES: [&str; 3] = ["hernia", "achalasia", "cholecystitis"];

fn read_json(path: &str) -> Value {
    let p = format!(
        "{}/tests/fixtures/temporal/{path}",
        env!("CARGO_MANIFEST_DIR")
    );
    serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap()
}

/// The opinion writer's reply: the board, keys in the schema's (sorted) order.
fn board_reply(name: &str) -> String {
    read_json(&format!("boards/{name}-board.json"))["board"].to_string()
}

/// The answer writer's reply: the stored answer with the three flat texts the
/// live schema sends, keys in the schema's (sorted) order.
fn answer_reply(name: &str) -> String {
    let stored = read_json(&format!("full/{name}.json"));
    let mut a = stored["answer"].clone();
    let g = a["at_a_glance"].clone();
    let mut glance = vec![format!("Verdict: {}", g["verdict"].as_str().unwrap_or(""))];
    for k in g["key_numbers"].as_array().into_iter().flatten() {
        glance.push(format!(
            "- {}: {}",
            k["label"].as_str().unwrap_or(""),
            k["value_text"].as_str().unwrap_or("")
        ));
    }
    glance.push(format!(
        "Main caveat: {}",
        g["main_caveat"].as_str().unwrap_or("")
    ));
    a["at_a_glance"] = json!(glance.join("\n"));
    let p = a["clinical_practice"].clone();
    a["clinical_practice"] = json!(format!(
        "Default choice: {}\nCounselling: {}",
        p["default_choice"].as_str().unwrap_or(""),
        p["counselling"].as_str().unwrap_or("")
    ));
    let notable: Vec<String> = a["notable_findings"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|n| format!("- {}", n["text"].as_str().unwrap_or("")))
        .collect();
    a["notable_findings"] = json!(notable.join("\n"));
    a.to_string()
}

fn replies() -> Vec<(String, String)> {
    let mut out = Vec::new();
    for n in NAMES {
        out.push((format!("answer {n}"), answer_reply(n)));
        out.push((format!("opinion {n}"), board_reply(n)));
    }
    out
}

/// Everything a value holds: `Some` if `a` is a part of `b`: strings may be
/// cut short and containers may be missing their end, nothing else may differ.
fn is_part_of(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::String(x), Value::String(y)) => y.starts_with(x.as_str()),
        (Value::Object(x), Value::Object(y)) => x
            .iter()
            .all(|(k, v)| y.get(k).is_some_and(|w| is_part_of(v, w))),
        (Value::Array(x), Value::Array(y)) => {
            x.len() <= y.len() && x.iter().zip(y).all(|(v, w)| is_part_of(v, w))
        }
        (Value::Null, _) => true,
        _ => a == b,
    }
}

/// Every prefix, at every character: no panic, content only grows, and each
/// reading is a part of the whole. The prefixes are read through the plain
/// reader; the page's sections are read on the same prefixes further down.
#[test]
fn every_prefix_of_every_fixture_reply_reads_and_only_grows() {
    for (name, reply) in replies() {
        let whole: Value = serde_json::from_str(&reply).unwrap();
        let mut last = read_prefix("");
        for (i, _) in reply.char_indices() {
            let now = read_prefix(&reply[..i]);
            assert!(is_part_of(&now.value, &whole), "{name} at {i}");
            assert!(is_part_of(&last.value, &now.value), "{name} grows at {i}");
            assert!(now.open.is_some(), "{name} at {i}: not whole yet");
            last = now;
        }
    }
}

/// The whole text reads exactly as the final parse does.
#[test]
fn the_full_text_equals_the_final() {
    for (name, reply) in replies() {
        let p = read_prefix(&reply);
        assert_eq!(p.open, None, "{name}");
        assert_eq!(
            p.value,
            serde_json::from_str::<Value>(&reply).unwrap(),
            "{name}"
        );
    }
}

fn shape(s: &[incisive_app::draft_page::Section]) -> Vec<usize> {
    s.iter().map(|x| x.items.len()).collect()
}

/// The page's sections on the way there: no panic, sections never vanish, and
/// at the end nothing is faded or left coming that the reply holds.
#[test]
fn the_sections_read_at_every_step_and_finish_whole() {
    for n in NAMES {
        let reply = answer_reply(n);
        let mut last = vec![0; 9];
        for (i, _) in reply.char_indices().step_by(37) {
            let now = shape(&read_answer(&reply[..i]));
            assert_eq!(now.len(), last.len());
            for (a, b) in last.iter().zip(&now) {
                assert!(a <= b, "{n} at {i}: {last:?} -> {now:?}");
            }
            last = now;
        }
        let done = read_answer(&reply);
        for s in &done {
            assert!(s.note.is_none(), "{n}: {}", s.title);
            for it in &s.items {
                assert!(!it.faded && it.text.tail.is_empty(), "{n}: {}", s.title);
            }
        }
        for t in [
            "At a glance",
            "Bottom line",
            "What this means for clinical practice",
            "Outcome by outcome",
            "The trials",
        ] {
            assert!(
                done.iter().any(|s| s.title == t && !s.items.is_empty()),
                "{n}: {t} empty"
            );
        }
        let reply = board_reply(n);
        let mut last = vec![0; 4];
        for (i, _) in reply.char_indices().step_by(37) {
            let now = shape(&read_opinion(&reply[..i]));
            for (a, b) in last.iter().zip(&now) {
                assert!(a <= b, "{n} at {i}: {last:?} -> {now:?}");
            }
            last = now;
        }
        let done = read_opinion(&reply);
        assert!(done.iter().all(|s| !s.items.is_empty()), "{n}");
    }
}
