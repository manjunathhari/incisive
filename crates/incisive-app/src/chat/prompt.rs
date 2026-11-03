//! The grounded prompt. Gemma is never asked for a chat reply without
//! passages (rule C8): `grounded` takes `&GroundedPassages`, which cannot be
//! built from an empty list. Nothing here logs question, history or passages.

use serde_json::json;

use super::ChatMode;
use super::gemma::GemmaRequest;
use super::lines::NOT_COVERED;
use super::retrieve::Hit;

/// Reply length cap.
pub const NUM_PREDICT: u32 = 300;
/// How long the model stays loaded after a call.
pub const KEEP_ALIVE: &str = "30m";
/// Earlier turns sent as context.
pub const HISTORY_TURNS: usize = 3;

/// One or more retrieved hits. The private field means the only way to get
/// one is `new`, which refuses an empty list.
#[derive(Debug, Clone)]
pub struct GroundedPassages(Vec<Hit>);

impl GroundedPassages {
    pub fn new(hits: Vec<Hit>) -> Option<GroundedPassages> {
        if hits.is_empty() {
            None
        } else {
            Some(GroundedPassages(hits))
        }
    }

    pub fn hits(&self) -> &[Hit] {
        &self.0
    }
}

/// The rules Gemma is given. Plain mode is the clinical rules plus a reading
/// level; the grounding rules are never loosened.
pub fn system_rules(mode: ChatMode) -> String {
    let mut s = format!(
        "You answer questions about one surgical evidence summary. \
Answer only from the numbered passages you are given; use nothing else you know. \
Cite the passages you use like [1] or [2], placed right after the claim. \
Stay within the comparison asked: do not bring in other treatments or comparisons the question does not name. \
If the passages do not answer the question, reply exactly: {NOT_COVERED} \
Be brief: at most a few sentences. \
Describe what the evidence shows and never say \"you should\" or tell the reader what to do. \
Do not name authors, years or journals, and do not add references of your own; the numbers in square brackets are the only citations. \
Do not invent numbers: use a number only if it appears in a passage."
    );
    if mode == ChatMode::Plain {
        s.push_str(
            " The reader is a patient or family member, not a clinician. \
Write in plain, everyday words and short sentences; explain any medical term you must use in a few words. \
Do not give advice about the reader's own care.",
        );
    }
    s
}

/// The request for one grounded reply: rules, the last three earlier turns,
/// then the numbered passages and the question. The model is added by the
/// client.
pub fn grounded(
    p: &GroundedPassages,
    question: &str,
    history: &[(String, String)],
    mode: ChatMode,
) -> GemmaRequest {
    let mut messages = vec![json!({"role": "system", "content": system_rules(mode)})];
    let from = history.len().saturating_sub(HISTORY_TURNS);
    for (q, a) in &history[from..] {
        messages.push(json!({"role": "user", "content": q}));
        messages.push(json!({"role": "assistant", "content": a}));
    }
    let numbered = p
        .hits()
        .iter()
        .enumerate()
        .map(|(i, h)| format!("[{}] {}", i + 1, h.text))
        .collect::<Vec<_>>()
        .join("\n");
    messages.push(json!({
        "role": "user",
        "content": format!("Passages:\n{numbered}\n\nQuestion: {question}"),
    }));
    GemmaRequest::new(json!({
        "messages": messages,
        "stream": true,
        "keep_alive": KEEP_ALIVE,
        "options": {"num_predict": NUM_PREDICT},
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chat::passages::PassageKind;

    fn hit(text: &str) -> Hit {
        Hit {
            passage_id: Some(1),
            qa_id: None,
            kind: PassageKind::Evidence,
            text: text.to_string(),
            ref_ns: vec![],
            score: 0.9,
        }
    }

    fn passages(texts: &[&str]) -> GroundedPassages {
        GroundedPassages::new(texts.iter().map(|t| hit(t)).collect()).unwrap()
    }

    fn msgs(r: &GemmaRequest) -> Vec<(String, String)> {
        r.body()["messages"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| {
                (
                    m["role"].as_str().unwrap().to_string(),
                    m["content"].as_str().unwrap().to_string(),
                )
            })
            .collect()
    }

    #[test]
    fn empty_passages_cannot_build_a_request() {
        assert!(GroundedPassages::new(vec![]).is_none());
        assert!(GroundedPassages::new(vec![hit("x")]).is_some());
    }

    #[test]
    fn system_rules_are_present() {
        let sys = system_rules(ChatMode::Clinical);
        for needle in [
            "only from the numbered passages",
            "like [1]",
            "comparison asked",
            "Not covered in this evidence",
            "never say \"you should\"",
            "authors, years or journals",
        ] {
            assert!(sys.contains(needle), "missing: {needle}");
        }
        assert!(!sys.contains("plain, everyday"));
        let r = grounded(&passages(&["a"]), "q?", &[], ChatMode::Clinical);
        assert_eq!(msgs(&r)[0], ("system".to_string(), sys));
    }

    #[test]
    fn plain_mode_adds_plain_rules() {
        let c = system_rules(ChatMode::Clinical);
        let p = system_rules(ChatMode::Plain);
        assert!(p.starts_with(&c));
        assert!(p.contains("plain, everyday"));
        assert!(p.contains("patient"));
    }

    #[test]
    fn passages_numbered_from_one() {
        let r = grounded(
            &passages(&["first text", "second text"]),
            "what about X?",
            &[],
            ChatMode::Clinical,
        );
        let m = msgs(&r);
        assert_eq!(m.len(), 2);
        let (role, last) = &m[1];
        assert_eq!(role, "user");
        assert!(last.contains("[1] first text\n[2] second text"));
        assert!(last.ends_with("Question: what about X?"));
        assert!(!last.contains("[0]"));
    }

    #[test]
    fn history_keeps_last_three_turns() {
        let h: Vec<(String, String)> = (1..=5)
            .map(|i| (format!("q{i}"), format!("a{i}")))
            .collect();
        let r = grounded(&passages(&["a"]), "now", &h, ChatMode::Clinical);
        let m = msgs(&r);
        // system + 3 * (user, assistant) + final user
        assert_eq!(m.len(), 8);
        assert_eq!(m[1], ("user".to_string(), "q3".to_string()));
        assert_eq!(m[2], ("assistant".to_string(), "a3".to_string()));
        assert_eq!(m[5], ("user".to_string(), "q5".to_string()));
        assert_eq!(m[6], ("assistant".to_string(), "a5".to_string()));
        assert!(!m.iter().any(|(_, c)| c == "q2"));
        let short = grounded(&passages(&["a"]), "now", &h[..1], ChatMode::Clinical);
        assert_eq!(msgs(&short).len(), 4);
    }

    #[test]
    fn body_sets_num_predict_and_keep_alive() {
        let r = grounded(&passages(&["a"]), "q", &[], ChatMode::Clinical);
        let b = r.body();
        assert_eq!(b["options"]["num_predict"], 300);
        assert_eq!(b["keep_alive"], "30m");
        assert_eq!(b["stream"], true);
        assert!(b.get("model").is_none(), "the client adds the model");
    }
}
