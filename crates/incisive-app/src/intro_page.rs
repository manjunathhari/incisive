//! The introduction's fixed words and page rules (no Leptos; built for the
//! browser too). The job is `runs::intro`, the view is `app::intro`.
//!
//! The text arrives whole; the browser reveals it word by word once
//! ([`Reveal`]), only when it watched the text arrive. All pure, so it is
//! tested without a browser.

/// The prompt, byte for byte (owner approved).
pub const PROMPT: &str = "For the question below, write a simple, brief introduction in plain paragraphs:
- what the question is asking;
- why the question is important;
- a few lines on the disease.

Give no numbers, no studies, and no answer: don't say or hint which approach is better. Write nothing else, no title and no closing line.";

pub const HEADING: &str = "Introduction";
pub const TAG_RUNNING: &str =
    "A quick overview while we research your question and gather published data.";
pub const TAG_FINISHED: &str =
    "A quick overview written while we researched your question and gathered published data.";
/// Added by code, while the run works only, once the text is complete.
pub const CLOSING: &str = "We are doing deep research to give you a well-researched answer.";
/// The folded row's right side after the run.
pub const READ: &str = "Read";

/// The `shown` value that means the whole text.
pub const ALL_WORDS: usize = usize::MAX;

/// The paragraphs of a stored text: split on blank lines, trimmed, empty ones
/// dropped. A single line break stays inside its paragraph.
pub fn paragraphs(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current: Vec<&str> = Vec::new();
    for line in text.lines() {
        if line.trim().is_empty() {
            if !current.is_empty() {
                out.push(current.join("\n"));
                current.clear();
            }
        } else {
            current.push(line.trim());
        }
    }
    if !current.is_empty() {
        out.push(current.join("\n"));
    }
    out
}

/// Words in the text, the reveal's length.
pub fn word_total(paragraphs: &[String]) -> usize {
    paragraphs
        .iter()
        .map(|p| p.split_whitespace().count())
        .sum()
}

/// One paragraph and the number of words before it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paragraph {
    pub text: String,
    pub before: usize,
    pub words: usize,
}

/// The paragraphs with their places in the reveal.
pub fn laid_out(paragraphs: Vec<String>) -> Vec<Paragraph> {
    let mut before = 0;
    paragraphs
        .into_iter()
        .map(|text| {
            let words = text.split_whitespace().count();
            let p = Paragraph {
                text,
                before,
                words,
            };
            before += words;
            p
        })
        .collect()
}

impl Paragraph {
    /// Any of it shown yet, with `shown` words of the whole text out?
    pub fn visible(&self, shown: usize) -> bool {
        shown > self.before
    }

    /// The caret sits here: this paragraph is the one being revealed.
    pub fn caret(&self, shown: usize) -> bool {
        shown > self.before && shown < self.before + self.words
    }

    /// The paragraph's text with `shown` words of the whole text out. Its own
    /// spacing is kept; a word never appears in part.
    pub fn shown_text(&self, shown: usize) -> &str {
        let n = shown.saturating_sub(self.before);
        if n >= self.words {
            return &self.text;
        }
        if n == 0 {
            return "";
        }
        let mut seen = 0;
        let mut in_word = false;
        for (i, c) in self.text.char_indices() {
            if c.is_whitespace() {
                if in_word {
                    seen += 1;
                    if seen == n {
                        return &self.text[..i];
                    }
                }
                in_word = false;
            } else {
                in_word = true;
            }
        }
        &self.text
    }
}

/// What the page does after looking at the latest poll.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RevealStep {
    Nothing,
    /// The text just arrived in front of the reader: reveal it word by word.
    Start,
    /// The run ended in mid-reveal: show it whole (it is the folded row now).
    Finish,
}

/// The reveal's memory across polls and redraws. It starts at most once: only
/// on a page that saw the run working without the text, then saw it arrive.
/// A page loaded with the text already there, a finished run or a reader who
/// prefers reduced motion get the whole text at once.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Reveal {
    waited: bool,
    arrived: bool,
    revealing: bool,
}

impl Reveal {
    pub fn observe(&mut self, active: bool, has_text: bool, reduced_motion: bool) -> RevealStep {
        if !has_text {
            self.waited |= active;
            return RevealStep::Nothing;
        }
        if !active {
            self.arrived = true;
            if self.revealing {
                self.revealing = false;
                return RevealStep::Finish;
            }
            return RevealStep::Nothing;
        }
        if self.arrived {
            return RevealStep::Nothing;
        }
        self.arrived = true;
        if self.waited && !reduced_motion {
            self.revealing = true;
            return RevealStep::Start;
        }
        RevealStep::Nothing
    }

    /// The last word is out.
    pub fn complete(&mut self) {
        self.revealing = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_words_are_byte_exact() {
        assert_eq!(
            PROMPT,
            "For the question below, write a simple, brief introduction in plain paragraphs:\n\
             - what the question is asking;\n\
             - why the question is important;\n\
             - a few lines on the disease.\n\
             \n\
             Give no numbers, no studies, and no answer: don't say or hint which approach is better. Write nothing else, no title and no closing line."
        );
        assert_eq!(HEADING, "Introduction");
        assert_eq!(
            TAG_RUNNING,
            "A quick overview while we research your question and gather published data."
        );
        assert_eq!(
            TAG_FINISHED,
            "A quick overview written while we researched your question and gathered published data."
        );
        assert_eq!(
            CLOSING,
            "We are doing deep research to give you a well-researched answer."
        );
        assert_eq!(READ, "Read");
    }

    #[test]
    fn paragraphs_split_on_blank_lines() {
        assert_eq!(
            paragraphs("  One.\n\nTwo\nlines.\r\n \r\n\r\nThree.  \n"),
            ["One.", "Two\nlines.", "Three."]
        );
        assert!(paragraphs(" \n\n ").is_empty());
    }

    #[test]
    fn reveal_goes_word_by_word_across_paragraphs() {
        let p = laid_out(paragraphs("Aa  bb cc.\n\nDd ee."));
        assert_eq!(word_total(&["Aa  bb cc.".into(), "Dd ee.".into()]), 5);
        assert!(!p[0].visible(0));
        assert_eq!(p[0].shown_text(0), "");
        assert_eq!(p[0].shown_text(1), "Aa");
        assert_eq!(p[0].shown_text(2), "Aa  bb");
        assert!(p[0].caret(2));
        assert_eq!(p[0].shown_text(3), "Aa  bb cc.");
        assert!(!p[0].caret(3));
        assert!(!p[1].visible(3));
        assert!(p[1].visible(4));
        assert_eq!(p[1].shown_text(4), "Dd");
        assert!(p[1].caret(4));
        assert_eq!(p[1].shown_text(5), "Dd ee.");
        assert!(!p[1].caret(5));
        // The whole text: what the server draws.
        assert_eq!(p[0].shown_text(ALL_WORDS), "Aa  bb cc.");
        assert_eq!(p[1].shown_text(ALL_WORDS), "Dd ee.");
        assert!(!p[1].caret(ALL_WORDS));
    }

    #[test]
    fn reveal_handles_other_alphabets() {
        let p = laid_out(paragraphs("जठर कैंसर है"));
        assert_eq!(p[0].shown_text(2), "जठर कैंसर");
    }

    #[test]
    fn reveal_starts_once_only_when_the_text_arrives_in_view() {
        let mut r = Reveal::default();
        assert_eq!(r.observe(true, false, false), RevealStep::Nothing);
        assert_eq!(r.observe(true, true, false), RevealStep::Start);
        // Later polls and redraws never start it again, during or after.
        assert_eq!(r.observe(true, true, false), RevealStep::Nothing);
        r.complete();
        assert_eq!(r.observe(true, true, false), RevealStep::Nothing);
        assert_eq!(r.observe(false, true, false), RevealStep::Nothing);
    }

    #[test]
    fn no_reveal_on_a_page_loaded_with_the_text() {
        let mut r = Reveal::default();
        assert_eq!(r.observe(true, true, false), RevealStep::Nothing);
        assert_eq!(r.observe(true, true, false), RevealStep::Nothing);
    }

    #[test]
    fn no_reveal_on_a_finished_run() {
        let mut r = Reveal::default();
        assert_eq!(r.observe(false, true, false), RevealStep::Nothing);
        // A page that watched the run end first, and the text come after.
        let mut r = Reveal::default();
        assert_eq!(r.observe(true, false, false), RevealStep::Nothing);
        assert_eq!(r.observe(false, false, false), RevealStep::Nothing);
        assert_eq!(r.observe(false, true, false), RevealStep::Nothing);
    }

    #[test]
    fn no_reveal_with_reduced_motion() {
        let mut r = Reveal::default();
        assert_eq!(r.observe(true, false, true), RevealStep::Nothing);
        assert_eq!(r.observe(true, true, true), RevealStep::Nothing);
    }

    #[test]
    fn run_ending_in_mid_reveal_finishes_it() {
        let mut r = Reveal::default();
        assert_eq!(r.observe(true, false, false), RevealStep::Nothing);
        assert_eq!(r.observe(true, true, false), RevealStep::Start);
        assert_eq!(r.observe(false, true, false), RevealStep::Finish);
        assert_eq!(r.observe(false, true, false), RevealStep::Nothing);
    }
}
