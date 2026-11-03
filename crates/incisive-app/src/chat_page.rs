//! The chat window's rules (no Leptos), and the types the page and its
//! server functions share. The back end's types are server only, so the
//! browser reads these; the server converts into them in one place.
//!
//! A reply being written is unchecked text: it carries no flags until its turn
//! finishes. A finished reply with no stored checks reads as unchecked, never
//! as clean. The busy marker never reaches here as a flag: the service turns
//! it into the busy line.
//!
//! Nothing here logs a question or a reply, and a question never goes in an
//! address: a refusal after a form post names only a fixed code, and an
//! unknown code shows nothing.

use serde::{Deserialize, Serialize};

use crate::chat::lines;
use crate::deid::FindingKind;

/// How often a pending turn is read.
pub const POLL_TURN_MS: u64 = 700;
/// Seconds between a pre-hydration page's reloads while its turn is pending.
pub const REFRESH_SECS: u32 = 3;
/// Reads of a pending turn that may fail in a row before the page says so and
/// starts to slow down.
pub const CONNECTION_AFTER_FAILURES: u32 = 3;
/// The slowest a failing read goes: this many ticks of [`POLL_TURN_MS`].
const MAX_TICKS_BETWEEN_READS: u32 = 32;
/// The run page's wording for the same trouble.
pub const CONNECTION_RETRYING: &str = "Connection problem \u{2014} retrying. ";
/// The gap between the conversation's end and the question box.
pub const END_GAP_PX: f64 = 12.0;
/// The longest range of citations read as citations (`[1-3]`).
const MAX_CITE_RANGE: u32 = 20;

pub const EMPTY_INTRO: &str =
    "Ask anything about this run. Replies use only its answer, Expert Opinion and papers.";
pub const WAITING_LINE: &str =
    "Reading this run's evidence. Replies usually take 15 to 25 seconds.";
pub const WRITING_LINE: &str = "Still writing. Not checked yet.";
pub const CUT_SHORT_LINE: &str = "This reply was cut short.";
pub const FLAGS_HEAD: &str = "Check this reply";
pub const UNCHECKED_LINE: &str = "This reply could not be checked.";
pub const SUGGEST_LABEL: &str = "Suggest for library";
pub const SUGGESTED_LABEL: &str = "Suggested for library";
pub const OWNER_REVIEWED: &str = "[Owner-reviewed]";
/// The answer and opinion are unaffected when chat is down.
pub const NOT_AFFECTED: &str = "The answer and Expert Opinion are not affected.";

/// Whether chat can be used on a run.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChatState {
    Unavailable,
    NotOffered,
    GettingReady,
    Ready,
}

impl ChatState {
    /// The option's text; `None` when there is no option at all.
    pub fn tab_label(self) -> Option<&'static str> {
        match self {
            ChatState::Ready | ChatState::GettingReady => Some("Chat with me"),
            ChatState::Unavailable => Some("Chat with me \u{b7} Unavailable"),
            ChatState::NotOffered => None,
        }
    }

    pub fn selectable(self) -> bool {
        self == ChatState::Ready
    }

    /// The line under the option.
    pub fn note(self) -> Option<String> {
        (self == ChatState::Unavailable).then(|| format!("{} {NOT_AFFECTED}", lines::UNAVAILABLE))
    }
}

/// What the checks found in one reply (the back end's `Checks`).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ChatChecks {
    pub not_traced: Vec<String>,
    pub unknown_citations: Vec<u32>,
    pub outside_references: Vec<String>,
    pub comparison_mismatch: Vec<String>,
    pub you_should: bool,
    pub uncited: bool,
    pub retracted_source: bool,
    pub unchecked: bool,
}

/// One turn as its own user sees it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ChatTurn {
    pub id: i64,
    pub question: String,
    pub reply: String,
    pub status: String,
    pub mode: String,
    /// Position `i` is citation `[i+1]`: a passage's id, or a Q&A's negated.
    pub sources: Vec<i64>,
    pub checks: Option<ChatChecks>,
    pub safety_line: bool,
    pub personal: bool,
    pub feedback: Option<i16>,
    pub suggested: bool,
}

/// A run's chat.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ChatPageView {
    pub state: ChatState,
    pub starters: Vec<String>,
    pub turns: Vec<ChatTurn>,
}

/// A question that got no turn: its message, and the safety line if it needs
/// one.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Refusal {
    /// Empty after a form post: a question never goes in an address.
    pub question: String,
    pub message: String,
    pub safety_line: bool,
}

/// What asking returned to the page's own code.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AskReply {
    Accepted(ChatTurn),
    Refused(Refusal),
}

/// What a citation opens.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SheetSource {
    pub label: String,
    pub text: String,
}

/// How far a turn has got.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    /// Nothing written yet.
    Waiting,
    /// Unchecked text so far.
    Writing,
    Finished,
}

/// A piece of a reply.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReplyPiece {
    Text(String),
    /// Citation `n` and its stored source id.
    Cite {
        n: u32,
        source: i64,
    },
}

impl ChatTurn {
    pub fn pending(&self) -> bool {
        self.status == "pending"
    }

    pub fn phase(&self) -> Phase {
        if !self.pending() {
            Phase::Finished
        } else if self.reply.trim().is_empty() {
            Phase::Waiting
        } else {
            Phase::Writing
        }
    }

    /// The fixed line of a turn that gave no reply of its own.
    fn is_fixed_line(&self) -> bool {
        matches!(self.status.as_str(), "not_covered" | "unavailable")
    }

    /// The reply with its citations of given passages picked out.
    pub fn pieces(&self) -> Vec<ReplyPiece> {
        if self.is_fixed_line() {
            return vec![ReplyPiece::Text(self.reply.clone())];
        }
        reply_pieces(&self.reply, &self.sources)
    }

    /// The flag lines, in plain words. None until the turn has finished, and
    /// none on a fixed line, which nothing was checked on. A finished reply
    /// with no stored checks reads as unchecked.
    pub fn flag_lines(&self) -> Vec<String> {
        if self.pending() || self.is_fixed_line() {
            return Vec::new();
        }
        let Some(c) = &self.checks else {
            return vec![UNCHECKED_LINE.to_string()];
        };
        let mut out: Vec<String> = c
            .not_traced
            .iter()
            .map(|n| format!("\"{n}\" is not in the sources cited."))
            .collect();
        let mut line = |on: bool, text: &str| {
            if on {
                out.push(text.to_string());
            }
        };
        line(
            !c.unknown_citations.is_empty(),
            "A citation doesn't match any source.",
        );
        line(
            !c.outside_references.is_empty(),
            "It names a reference that isn't from this evidence.",
        );
        line(
            !c.comparison_mismatch.is_empty(),
            "It mentions a different comparison from the one asked.",
        );
        line(c.you_should, "It says \"you should\".");
        line(c.uncited, "It gives no source.");
        line(c.retracted_source, "It relies on a retracted paper.");
        line(c.unchecked, UNCHECKED_LINE);
        out
    }

    pub fn cut_short(&self) -> bool {
        self.status == "partial"
    }

    pub fn shows_surgeon(&self) -> bool {
        self.personal && !self.pending()
    }

    /// Thumbs go under a finished reply; a turn that could not answer at all
    /// has nothing to judge.
    pub fn shows_thumbs(&self) -> bool {
        matches!(self.status.as_str(), "done" | "partial" | "not_covered")
    }

    pub fn can_suggest(&self) -> bool {
        self.status == "done"
    }

    pub fn suggest_label(&self) -> &'static str {
        if self.suggested {
            SUGGESTED_LABEL
        } else {
            SUGGEST_LABEL
        }
    }
}

/// One entry of a citation marker's list: `1`, or `1-3`.
fn marker_items(inner: &str) -> Option<Vec<(&str, u32, u32)>> {
    inner
        .split(',')
        .map(|part| {
            let part = part.trim();
            let number = |s: &str| -> Option<u32> {
                let s = s.trim();
                if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
                    return None;
                }
                s.parse().ok()
            };
            match part.split_once(['-', '\u{2013}', '\u{2014}']) {
                Some((a, b)) => Some((part, number(a)?, number(b)?)),
                None => {
                    let n = number(part)?;
                    Some((part, n, n))
                }
            }
        })
        .collect()
}

/// The source behind citation `n`, if it was given.
fn source_of(n: u32, sources: &[i64]) -> Option<i64> {
    let i = usize::try_from(n.checked_sub(1)?).ok()?;
    sources.get(i).copied().filter(|s| *s != 0)
}

/// `reply` as text and citation pieces. Every character outside a citation of
/// a given source is kept, so a marker with nothing behind it stays in the
/// text for the reader and the check flags it.
pub fn reply_pieces(reply: &str, sources: &[i64]) -> Vec<ReplyPiece> {
    fn push_text(out: &mut Vec<ReplyPiece>, t: &str) {
        if t.is_empty() {
            return;
        }
        if let Some(ReplyPiece::Text(last)) = out.last_mut() {
            last.push_str(t);
        } else {
            out.push(ReplyPiece::Text(t.to_string()));
        }
    }
    let mut out = Vec::new();
    let mut rest = reply;
    while let Some(open) = rest.find('[') {
        let Some(len) = rest[open..].find(']') else {
            break;
        };
        let inner = &rest[open + 1..open + len];
        let marker = &rest[open..=open + len];
        let mut cites = Vec::new();
        if let Some(items) = marker_items(inner) {
            for (raw, a, b) in items {
                let wanted = a <= b && b - a <= MAX_CITE_RANGE;
                let found: Option<Vec<ReplyPiece>> = wanted
                    .then(|| {
                        (a..=b)
                            .map(|n| {
                                source_of(n, sources).map(|source| ReplyPiece::Cite { n, source })
                            })
                            .collect()
                    })
                    .flatten();
                match found {
                    Some(c) => cites.extend(c),
                    None => cites.push(ReplyPiece::Text(format!("[{raw}]"))),
                }
            }
        }
        push_text(&mut out, &rest[..open]);
        if cites.iter().any(|c| matches!(c, ReplyPiece::Cite { .. })) {
            for c in cites {
                match c {
                    ReplyPiece::Text(t) => push_text(&mut out, &t),
                    cite => out.push(cite),
                }
            }
        } else {
            push_text(&mut out, marker);
        }
        rest = &rest[open + len + 1..];
    }
    push_text(&mut out, rest);
    out
}

/// Whether the page reads the pending turn now.
pub fn should_poll(pending: bool, hidden: bool) -> bool {
    pending && !hidden
}

/// Ticks of [`POLL_TURN_MS`] to the next read after `failures` failed reads in
/// a row: every tick at first, then doubling up to about 22 s.
pub fn ticks_between_reads(failures: u32) -> u32 {
    match failures.checked_sub(CONNECTION_AFTER_FAILURES) {
        None => 1,
        Some(over) => 2u32
            .checked_pow(over + 1)
            .map_or(MAX_TICKS_BETWEEN_READS, |t| t.min(MAX_TICKS_BETWEEN_READS)),
    }
}

/// Whether the page reads the chat's state. Getting ready, or not offered yet
/// while the opinion is being written or since it finished on this page: its
/// record may still make the chat ready.
pub fn reads_state(state: ChatState, writing: bool, was_writing: bool) -> bool {
    match state {
        ChatState::GettingReady => true,
        ChatState::NotOffered => writing || was_writing,
        ChatState::Ready | ChatState::Unavailable => false,
    }
}

/// The reload of a page that arrived by a form post before hydration. Once
/// hydrated the page reads its turn itself, so the reload tag is never drawn
/// again, whatever is asked later.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PageReload {
    wait: bool,
}

impl PageReload {
    /// From the address's `chat_wait`.
    pub fn from_address(wait: bool) -> PageReload {
        PageReload { wait }
    }

    /// The browser has taken over.
    pub fn hydrated(self) -> PageReload {
        PageReload { wait: false }
    }

    /// Whether the reload tag is drawn.
    pub fn tag(self, pending: bool) -> bool {
        self.wait && pending
    }
}

/// The style a new question starts in: the last one asked, else Clinical.
pub fn default_mode(turns: &[ChatTurn]) -> &str {
    turns.last().map_or("clinical", |t| t.mode.as_str())
}

/// The headings a stored opinion passage opens with, and how its label reads.
const OPINION_HEADINGS: &[(&str, &str)] = &[
    ("Expert verdict: ", "Expert verdict"),
    ("My view on why: ", "My view on why"),
    ("At a glance, the original view: ", "At a glance"),
    ("At a glance, the expert view: ", "At a glance"),
    ("At a glance, today: ", "At a glance"),
];

/// The citation sheet's label and text for a stored passage. `paper` names
/// the paper a paper's passage comes from.
pub fn passage_sheet(kind: &str, text: &str, paper: Option<&str>) -> SheetSource {
    let sheet = |label: &str| SheetSource {
        label: label.to_string(),
        text: text.to_string(),
    };
    let answer = |part: &str| sheet(&format!("Answer \u{b7} {part}"));
    match kind {
        "bottom_line" => answer("Bottom line"),
        "outcome" => answer("Outcomes"),
        "evidence" => answer("Evidence"),
        "conflict" => answer("Conflicts"),
        "applicability" => answer("Applicability"),
        "what_would_change" => answer("What would change this"),
        "guideline" => answer("Guidelines"),
        "clinical_practice" => answer("Clinical practice"),
        "notable" => answer("Notable findings"),
        "abstract" | "quote" | "reference" => sheet(paper.unwrap_or("Paper")),
        "owner_qa" => sheet(OWNER_REVIEWED),
        "opinion" => OPINION_HEADINGS
            .iter()
            .find_map(|(head, label)| {
                text.strip_prefix(head).map(|t| SheetSource {
                    label: format!("Expert Opinion \u{b7} {label}"),
                    text: t.to_string(),
                })
            })
            .unwrap_or_else(|| sheet("Expert Opinion")),
        _ => sheet("This run's evidence"),
    }
}

/// The citation sheet for an owner-reviewed Q&A.
pub fn qa_sheet(text: &str) -> SheetSource {
    SheetSource {
        label: OWNER_REVIEWED.to_string(),
        text: text.to_string(),
    }
}

/// "Journal year" of the first of a passage's references (as the store gives
/// them), for its sheet's label.
pub fn paper_name(references: &serde_json::Value) -> Option<String> {
    let first = references.as_array()?.first()?;
    let journal = first.get("journal").and_then(|j| j.as_str()).map(str::trim);
    let year = first.get("year").and_then(|y| {
        y.as_i64()
            .map(|n| n.to_string())
            .or_else(|| y.as_str().map(|s| s.trim().to_string()))
    });
    match (journal.filter(|j| !j.is_empty()), year) {
        (Some(j), Some(y)) => Some(format!("{j} {y}")),
        (Some(j), None) => Some(j.to_string()),
        (None, _) => None,
    }
}

/// The fixed code of a finding kind that has its own hint.
fn hint_code(kind: FindingKind) -> Option<&'static str> {
    match kind {
        FindingKind::UnusualCharacters => Some("unusual_characters"),
        _ => None,
    }
}

/// The distinct hint codes of what the identifier scan found.
pub fn hint_codes(kinds: impl IntoIterator<Item = FindingKind>) -> Vec<&'static str> {
    let mut out = Vec::new();
    for code in kinds.into_iter().filter_map(hint_code) {
        if !out.contains(&code) {
            out.push(code);
        }
    }
    out
}

/// The refusal named in the address after a form post; `None` for a code that
/// is not one. The address names codes only (`hints`: comma-separated hint
/// codes; unknown ones are ignored), so what shows is fixed text.
pub fn refusal_from_code(code: &str, safety: bool, hints: &str) -> Option<Refusal> {
    let identifiers;
    let message = match code {
        "not_found" => lines::NOT_FOUND_MESSAGE,
        "bad_request" => lines::BAD_REQUEST_MESSAGE,
        "empty" => lines::EMPTY_MESSAGE,
        "too_long" => lines::TOO_LONG_MESSAGE,
        "identifiers" => {
            identifiers = crate::ask::blocked_message(
                hints
                    .split(',')
                    .filter(|h| *h == "unusual_characters")
                    .map(|_| FindingKind::UnusualCharacters),
            );
            identifiers.as_str()
        }
        "in_flight" => lines::IN_FLIGHT_MESSAGE,
        "not_offered" => lines::NOT_OFFERED_MESSAGE,
        "daily_limit" => lines::DAILY_LIMIT_MESSAGE,
        "busy" => lines::BUSY,
        "unavailable" => lines::UNAVAILABLE,
        "failed" => lines::FAILED_MESSAGE,
        "not_suggestable" => lines::NOT_SUGGESTABLE_MESSAGE,
        _ => return None,
    };
    Some(Refusal {
        question: String::new(),
        message: message.to_string(),
        safety_line: safety,
    })
}

/// Where a form post lands once its question is accepted.
pub fn accepted_url(run: impl std::fmt::Display) -> String {
    format!("/runs/{run}?chat_wait=1#chat")
}

/// Where a form post lands once a thumb or a suggestion is stored.
pub fn chat_url(run: impl std::fmt::Display) -> String {
    format!("/runs/{run}#chat")
}

/// Where a refused form post lands. `code` and `hints` are fixed codes; the
/// question never goes in the address.
pub fn refused_url(
    run: impl std::fmt::Display,
    code: &str,
    safety: bool,
    hints: &[&str],
) -> String {
    let safety = if safety { "&chat_safety=1" } else { "" };
    let hints = if hints.is_empty() {
        String::new()
    } else {
        format!("&chat_hints={}", hints.join(","))
    };
    format!("/runs/{run}?chat_refused={code}{safety}{hints}#chat")
}

/// The pre-hydration reload's `content` value.
pub fn refresh_content(run: impl std::fmt::Display) -> String {
    format!("{REFRESH_SECS};url={}", accepted_url(run))
}

#[cfg(feature = "ssr")]
mod from_service {
    use super::*;
    use crate::chat::checks::Checks;
    use crate::chat::service::{self, ChatView, TurnSource, TurnView};

    impl From<service::ChatState> for ChatState {
        fn from(s: service::ChatState) -> ChatState {
            match s {
                service::ChatState::Unavailable => ChatState::Unavailable,
                service::ChatState::NotOffered => ChatState::NotOffered,
                service::ChatState::GettingReady => ChatState::GettingReady,
                service::ChatState::Ready => ChatState::Ready,
            }
        }
    }

    impl From<Checks> for ChatChecks {
        fn from(c: Checks) -> ChatChecks {
            ChatChecks {
                not_traced: c.not_traced,
                unknown_citations: c.unknown_citations,
                outside_references: c.outside_references,
                comparison_mismatch: c.comparison_mismatch,
                you_should: c.you_should,
                uncited: c.uncited,
                retracted_source: c.retracted_source,
                unchecked: c.unchecked,
            }
        }
    }

    impl From<TurnView> for ChatTurn {
        fn from(t: TurnView) -> ChatTurn {
            ChatTurn {
                id: t.id,
                question: t.question,
                reply: t.reply,
                status: t.status,
                mode: t.mode,
                sources: t
                    .sources
                    .into_iter()
                    .map(|s| match s {
                        TurnSource::Passage(id) => id,
                        TurnSource::Qa(id) => -id,
                    })
                    .collect(),
                checks: t.checks.map(ChatChecks::from),
                safety_line: t.safety_line,
                personal: t.personal,
                feedback: t.feedback,
                suggested: t.suggested,
            }
        }
    }

    impl From<ChatView> for ChatPageView {
        fn from(v: ChatView) -> ChatPageView {
            ChatPageView {
                state: v.state.into(),
                starters: v.starters,
                turns: v.turns.into_iter().map(ChatTurn::from).collect(),
            }
        }
    }
}

impl ChatTurn {
    /// A question just accepted, before its first read.
    pub fn just_asked(id: i64, question: &str, mode: &str, safety_line: bool) -> ChatTurn {
        ChatTurn {
            id,
            question: question.trim().to_string(),
            reply: String::new(),
            status: "pending".to_string(),
            mode: mode.to_string(),
            sources: Vec::new(),
            checks: None,
            safety_line,
            personal: false,
            feedback: None,
            suggested: false,
        }
    }
}

/// A CSS length as the browser reports it, in pixels; anything else is none.
pub fn px_of(value: &str) -> f64 {
    value
        .trim()
        .strip_suffix("px")
        .and_then(|n| n.parse::<f64>().ok())
        .filter(|n| n.is_finite())
        .unwrap_or(0.0)
}

/// How far above the conversation's end the page must stop so the end clears
/// the sticky question box: the box as tall as it is now, whatever stands
/// under it (the phone's nav) and a small gap.
pub fn end_scroll_margin(composer_height: f64, stuck_bottom: f64) -> f64 {
    let part = |n: f64| if n.is_finite() { n.max(0.0) } else { 0.0 };
    part(composer_height) + part(stuck_bottom) + END_GAP_PX
}

/// Whether the question box still holds `question`: if not, the reader has
/// typed something else, which must not be cleared.
pub fn box_holds(in_box: &str, question: &str) -> bool {
    in_box.trim() == question.trim()
}

/// Merge a fresh read of the chat's turns into what the page holds, in the
/// order they were asked. Returns whether anything changed.
pub fn merge_turns(held: &mut Vec<ChatTurn>, read: Vec<ChatTurn>) -> bool {
    let mut changed = false;
    for new in read {
        match held.iter_mut().find(|t| t.id == new.id) {
            Some(t) if *t == new => {}
            Some(t) => {
                *t = new;
                changed = true;
            }
            None => {
                held.push(new);
                changed = true;
            }
        }
    }
    if changed {
        held.sort_by_key(|t| t.id);
    }
    changed
}

/// Whether a fresh read holds a turn the page did not know of for `question`:
/// the server stored it although the answer to the ask never arrived.
pub fn stored_unseen(held: &[ChatTurn], read: &[ChatTurn], question: &str) -> bool {
    let question = question.trim();
    read.iter()
        .any(|t| t.question == question && !held.iter().any(|h| h.id == t.id))
}

/// One entry of the conversation.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Entry<'a> {
    Turn(&'a ChatTurn),
    Refusal(&'a Refusal),
}

/// The turns in order, each refusal after the number of turns there were when
/// it was given.
pub fn conversation<'a>(turns: &'a [ChatTurn], refusals: &'a [(usize, Refusal)]) -> Vec<Entry<'a>> {
    let mut out = Vec::with_capacity(turns.len() + refusals.len());
    let before = |at: usize, out: &mut Vec<Entry<'a>>| {
        out.extend(
            refusals
                .iter()
                .filter(|(after, _)| *after == at)
                .map(|(_, r)| Entry::Refusal(r)),
        );
    };
    for (i, t) in turns.iter().enumerate() {
        before(i, &mut out);
        out.push(Entry::Turn(t));
    }
    // Anything at or past the end, so a refusal is never lost.
    out.extend(
        refusals
            .iter()
            .filter(|(after, _)| *after >= turns.len())
            .map(|(_, r)| Entry::Refusal(r)),
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn turn(status: &str, reply: &str, checks: Option<ChatChecks>) -> ChatTurn {
        ChatTurn {
            id: 1,
            question: "Q".into(),
            reply: reply.into(),
            status: status.into(),
            mode: "clinical".into(),
            sources: vec![11, -22],
            checks,
            safety_line: false,
            personal: false,
            feedback: None,
            suggested: false,
        }
    }

    fn text(s: &str) -> ReplyPiece {
        ReplyPiece::Text(s.to_string())
    }

    #[test]
    fn tab_labels_and_notes_are_the_mocks() {
        assert_eq!(ChatState::Ready.tab_label(), Some("Chat with me"));
        assert_eq!(ChatState::GettingReady.tab_label(), Some("Chat with me"));
        assert_eq!(
            ChatState::Unavailable.tab_label(),
            Some("Chat with me \u{b7} Unavailable")
        );
        assert_eq!(ChatState::NotOffered.tab_label(), None);
        assert!(ChatState::Ready.selectable());
        assert!(!ChatState::GettingReady.selectable());
        assert!(!ChatState::Unavailable.selectable());
        assert_eq!(
            ChatState::Unavailable.note().as_deref(),
            Some("Chat is unavailable right now. The answer and Expert Opinion are not affected.")
        );
        assert_eq!(ChatState::Ready.note(), None);
        assert_eq!(ChatState::GettingReady.note(), None);
    }

    #[test]
    fn phases() {
        assert_eq!(turn("pending", "", None).phase(), Phase::Waiting);
        assert_eq!(turn("pending", "  ", None).phase(), Phase::Waiting);
        assert_eq!(turn("pending", "so far", None).phase(), Phase::Writing);
        for s in ["done", "partial", "not_covered", "unavailable"] {
            assert_eq!(turn(s, "x", None).phase(), Phase::Finished, "{s}");
        }
        assert!(turn("pending", "", None).pending());
        assert!(!turn("done", "", None).pending());
    }

    #[test]
    fn flag_labels_are_plain_words() {
        let all = ChatChecks {
            not_traced: vec!["3 days".into(), "7%".into()],
            unknown_citations: vec![4, 5],
            outside_references: vec!["Smith 2020".into()],
            comparison_mismatch: vec!["open vs lap".into()],
            you_should: true,
            uncited: true,
            retracted_source: true,
            unchecked: true,
        };
        assert_eq!(
            turn("done", "r", Some(all)).flag_lines(),
            vec![
                "\"3 days\" is not in the sources cited.",
                "\"7%\" is not in the sources cited.",
                "A citation doesn't match any source.",
                "It names a reference that isn't from this evidence.",
                "It mentions a different comparison from the one asked.",
                "It says \"you should\".",
                "It gives no source.",
                "It relies on a retracted paper.",
                "This reply could not be checked.",
            ]
        );
    }

    #[test]
    fn a_clean_reply_has_no_flags() {
        let t = turn("done", "r", Some(ChatChecks::default()));
        assert!(t.flag_lines().is_empty());
    }

    #[test]
    fn a_pending_reply_has_no_flags_whatever_is_stored() {
        let c = ChatChecks {
            uncited: true,
            ..ChatChecks::default()
        };
        assert!(turn("pending", "so far", Some(c)).flag_lines().is_empty());
        assert!(turn("pending", "so far", None).flag_lines().is_empty());
    }

    #[test]
    fn a_finished_reply_with_no_checks_is_unchecked_not_clean() {
        for s in ["done", "partial"] {
            assert_eq!(
                turn(s, "text", None).flag_lines(),
                vec![UNCHECKED_LINE],
                "{s}"
            );
        }
    }

    #[test]
    fn a_turn_with_no_reply_of_its_own_has_no_flags() {
        // Not covered, unavailable and busy show a fixed line nothing was
        // checked on: not a flag, and never the busy marker.
        for s in ["not_covered", "unavailable"] {
            let t = turn(s, lines::BUSY, None);
            assert!(t.flag_lines().is_empty(), "{s}");
        }
    }

    #[test]
    fn cut_short_only_on_partial() {
        assert!(turn("partial", "r", None).cut_short());
        for s in ["pending", "done", "not_covered", "unavailable"] {
            assert!(!turn(s, "r", None).cut_short(), "{s}");
        }
        assert_eq!(CUT_SHORT_LINE, "This reply was cut short.");
    }

    #[test]
    fn actions_by_status() {
        // (status, thumbs, suggest)
        for (s, thumbs, suggest) in [
            ("pending", false, false),
            ("done", true, true),
            ("partial", true, false),
            ("not_covered", true, false),
            ("unavailable", false, false),
        ] {
            let t = turn(s, "r", None);
            assert_eq!(t.shows_thumbs(), thumbs, "{s} thumbs");
            assert_eq!(t.can_suggest(), suggest, "{s} suggest");
        }
        let mut t = turn("done", "r", None);
        assert_eq!(t.suggest_label(), "Suggest for library");
        t.suggested = true;
        assert_eq!(t.suggest_label(), "Suggested for library");
    }

    #[test]
    fn surgeon_line_waits_for_the_finish() {
        let mut t = turn("pending", "so far", None);
        t.personal = true;
        assert!(!t.shows_surgeon());
        t.status = "done".into();
        assert!(t.shows_surgeon());
        t.personal = false;
        assert!(!t.shows_surgeon());
    }

    #[test]
    fn citations_render_only_for_given_passages() {
        assert_eq!(
            reply_pieces("A [1] and B [2], not C [3].", &[11, -22]),
            vec![
                text("A "),
                ReplyPiece::Cite { n: 1, source: 11 },
                text(" and B "),
                ReplyPiece::Cite { n: 2, source: -22 },
                text(", not C [3]."),
            ]
        );
        // No sources: every marker stays as text.
        assert_eq!(reply_pieces("A [1].", &[]), vec![text("A [1].")]);
        // Zero is not a citation.
        assert_eq!(reply_pieces("A [0].", &[11]), vec![text("A [0].")]);
    }

    #[test]
    fn citation_lists_and_ranges() {
        let c = |n, source| ReplyPiece::Cite { n, source };
        assert_eq!(reply_pieces("[1, 2]", &[11, 22]), vec![c(1, 11), c(2, 22)]);
        assert_eq!(
            reply_pieces("[1-3]", &[11, 22, 33]),
            vec![c(1, 11), c(2, 22), c(3, 33)]
        );
        assert_eq!(
            reply_pieces("[1\u{2013}2]", &[11, 22]),
            vec![c(1, 11), c(2, 22)]
        );
        // A list with one unknown keeps the unknown as text.
        assert_eq!(reply_pieces("[1, 9]", &[11]), vec![c(1, 11), text("[9]")]);
        // A range past the sources, or backwards, is all text.
        assert_eq!(reply_pieces("[1-3]", &[11]), vec![text("[1-3]")]);
        assert_eq!(reply_pieces("[3-1]", &[11, 22, 33]), vec![text("[3-1]")]);
    }

    #[test]
    fn non_markers_keep_every_character() {
        for s in [
            "[",
            "]",
            "[]",
            "[a]",
            "[1",
            "1]",
            "[1,]",
            "[,1]",
            "[99999999999]",
            "x [ y ] z",
            "\u{e9}[1\u{e9}]",
            "[1-]",
            "[-1]",
        ] {
            assert_eq!(reply_pieces(s, &[11]), vec![text(s)], "{s}");
        }
        assert_eq!(reply_pieces("", &[11]), Vec::<ReplyPiece>::new());
    }

    #[test]
    fn a_fixed_line_has_no_citations() {
        // A turn that gave no reply of its own may hold sources; its line
        // is drawn as it is.
        for s in ["not_covered", "unavailable"] {
            let t = turn(s, "See [1].", None);
            assert_eq!(t.pieces(), vec![text("See [1].")], "{s}");
        }
        assert_eq!(
            turn("pending", "See [1]", None).pieces(),
            vec![text("See "), ReplyPiece::Cite { n: 1, source: 11 }]
        );
    }

    #[test]
    fn polling_runs_only_while_pending_and_visible() {
        assert!(should_poll(true, false));
        assert!(!should_poll(true, true));
        assert!(!should_poll(false, false));
        assert!(!should_poll(false, true));
        assert_eq!(POLL_TURN_MS, 700);
    }

    #[test]
    fn mode_starts_as_last_asked() {
        assert_eq!(default_mode(&[]), "clinical");
        let mut t = turn("done", "r", None);
        t.mode = "plain".into();
        assert_eq!(default_mode(&[t]), "plain");
    }

    #[test]
    fn sheet_labels() {
        let s = passage_sheet("bottom_line", "Very low certainty.", None);
        assert_eq!(s.label, "Answer \u{b7} Bottom line");
        assert_eq!(s.text, "Very low certainty.");
        assert_eq!(
            passage_sheet("conflict", "t", None).label,
            "Answer \u{b7} Conflicts"
        );
        // An opinion's own heading moves to the label.
        let s = passage_sheet("opinion", "My view on why: Experience.", None);
        assert_eq!(s.label, "Expert Opinion \u{b7} My view on why");
        assert_eq!(s.text, "Experience.");
        let s = passage_sheet("opinion", "At a glance, today: Now.", None);
        assert_eq!(s.label, "Expert Opinion \u{b7} At a glance");
        assert_eq!(s.text, "Now.");
        let s = passage_sheet("opinion", "Era 1. Going in: x", None);
        assert_eq!(s.label, "Expert Opinion");
        assert_eq!(s.text, "Era 1. Going in: x");
        // A paper's passage names the paper.
        let s = passage_sheet("abstract", "t", Some("Hernia 2024"));
        assert_eq!(s.label, "Hernia 2024");
        assert_eq!(passage_sheet("abstract", "t", None).label, "Paper");
        // Whatever comes in, a label says only what is fixed here.
        assert_eq!(
            passage_sheet("something_new", "t", None).label,
            "This run's evidence"
        );
        assert_eq!(qa_sheet("t").label, "[Owner-reviewed]");
        assert_eq!(
            passage_sheet("owner_qa", "t", None).label,
            "[Owner-reviewed]"
        );
    }

    #[test]
    fn a_papers_name_comes_from_its_first_reference() {
        let r = serde_json::json!([{"journal": "Hernia", "year": 2024}, {"journal": "BJS"}]);
        assert_eq!(paper_name(&r).as_deref(), Some("Hernia 2024"));
        let r = serde_json::json!([{"journal": "Hernia", "year": "2024"}]);
        assert_eq!(paper_name(&r).as_deref(), Some("Hernia 2024"));
        let r = serde_json::json!([{"journal": "Hernia"}]);
        assert_eq!(paper_name(&r).as_deref(), Some("Hernia"));
        assert_eq!(paper_name(&serde_json::json!([{"year": 2024}])), None);
        assert_eq!(paper_name(&serde_json::json!([])), None);
        assert_eq!(paper_name(&serde_json::json!(null)), None);
    }

    #[test]
    fn refusal_codes_name_fixed_text_only() {
        let r = refusal_from_code("busy", true, "").unwrap();
        assert_eq!(r.message, lines::BUSY);
        assert!(r.safety_line);
        assert!(r.question.is_empty());
        for (code, msg) in [
            ("not_found", lines::NOT_FOUND_MESSAGE),
            ("bad_request", lines::BAD_REQUEST_MESSAGE),
            ("empty", lines::EMPTY_MESSAGE),
            ("too_long", lines::TOO_LONG_MESSAGE),
            ("daily_limit", lines::DAILY_LIMIT_MESSAGE),
            ("not_offered", lines::NOT_OFFERED_MESSAGE),
            ("in_flight", lines::IN_FLIGHT_MESSAGE),
            ("not_suggestable", lines::NOT_SUGGESTABLE_MESSAGE),
            ("failed", lines::FAILED_MESSAGE),
            ("unavailable", lines::UNAVAILABLE),
            ("identifiers", crate::ask::BLOCKED_MESSAGE),
        ] {
            let r = refusal_from_code(code, false, "").unwrap();
            assert_eq!(r.message, msg, "{code}");
            assert!(!r.safety_line);
        }
        assert_eq!(refusal_from_code("<b>hi", true, ""), None);
        assert_eq!(refusal_from_code("", true, ""), None);
    }

    #[test]
    fn addresses() {
        assert_eq!(accepted_url("R"), "/runs/R?chat_wait=1#chat");
        assert_eq!(chat_url("R"), "/runs/R#chat");
        assert_eq!(
            refused_url("R", "busy", true, &[]),
            "/runs/R?chat_refused=busy&chat_safety=1#chat"
        );
        assert_eq!(
            refused_url("R", "empty", false, &[]),
            "/runs/R?chat_refused=empty#chat"
        );
        assert_eq!(refresh_content("R"), "3;url=/runs/R?chat_wait=1#chat");
    }

    #[test]
    fn hints_travel_as_fixed_codes() {
        use FindingKind::*;
        // Every kind with a hint of its own has a code, and the code brings
        // that hint back.
        for kind in [
            Aadhaar,
            Pan,
            Phone,
            Email,
            CalendarDate,
            HospitalId,
            PersonName,
            Address,
            UnusualCharacters,
        ] {
            assert_eq!(
                hint_code(kind).is_some(),
                crate::deid::hint(kind).is_some(),
                "{kind:?}"
            );
            let codes = hint_codes([kind, kind]);
            assert!(codes.len() <= 1, "{kind:?}");
            let r = refusal_from_code("identifiers", false, &codes.join(",")).unwrap();
            assert_eq!(r.message, crate::ask::blocked_message([kind]), "{kind:?}");
        }
        assert_eq!(
            refused_url("R", "identifiers", true, &["unusual_characters"]),
            "/runs/R?chat_refused=identifiers&chat_safety=1&chat_hints=unusual_characters#chat"
        );
        // A code that is not one adds nothing, and only identifiers read them.
        let r = refusal_from_code("identifiers", false, "<b>hi,phone").unwrap();
        assert_eq!(r.message, crate::ask::BLOCKED_MESSAGE);
        let r = refusal_from_code("busy", false, "unusual_characters").unwrap();
        assert_eq!(r.message, lines::BUSY);
    }

    #[test]
    fn the_reload_tag_is_never_drawn_again_after_hydration() {
        let arrived = PageReload::from_address(true);
        assert!(arrived.tag(true), "before hydration, while pending");
        assert!(!arrived.tag(false), "finished");
        let hydrated = arrived.hydrated();
        // The next question asked in place is pending too: no tag.
        assert!(!hydrated.tag(true));
        assert!(!hydrated.hydrated().tag(true));
        assert!(!PageReload::from_address(false).tag(true));
    }

    #[test]
    fn failing_reads_slow_down_and_recover() {
        assert_eq!(ticks_between_reads(0), 1);
        assert_eq!(ticks_between_reads(2), 1);
        assert_eq!(ticks_between_reads(3), 2);
        assert_eq!(ticks_between_reads(4), 4);
        assert_eq!(ticks_between_reads(7), 32);
        assert_eq!(ticks_between_reads(8), 32);
        assert_eq!(ticks_between_reads(u32::MAX), 32);
        assert_eq!(
            CONNECTION_RETRYING,
            "Connection problem \u{2014} retrying. "
        );
    }

    #[test]
    fn state_is_read_until_the_chat_can_settle() {
        use ChatState::*;
        assert!(reads_state(GettingReady, false, false));
        assert!(reads_state(NotOffered, true, false), "being written");
        // Finished on this page, record still to come: the tab must appear.
        assert!(reads_state(NotOffered, false, true));
        assert!(!reads_state(NotOffered, false, false));
        assert!(!reads_state(Ready, true, true));
        assert!(!reads_state(Unavailable, true, true));
    }

    fn asked(id: i64, status: &str, question: &str) -> ChatTurn {
        let mut t = turn(status, "", None);
        t.id = id;
        t.question = question.into();
        t
    }

    #[test]
    fn a_fresh_read_merges_in_asking_order() {
        let mut held = vec![asked(1, "done", "a"), asked(3, "pending", "c")];
        let read = vec![
            asked(1, "done", "a"),
            asked(2, "done", "b"),
            asked(3, "done", "c"),
        ];
        assert!(merge_turns(&mut held, read.clone()));
        assert_eq!(held, read);
        // The same read again changes nothing, so nothing is redrawn.
        assert!(!merge_turns(&mut held, read.clone()));
        // A turn only the page knows of stays.
        let mut held = vec![asked(5, "pending", "new")];
        assert!(merge_turns(&mut held, vec![asked(4, "done", "old")]));
        assert_eq!(
            held,
            vec![asked(4, "done", "old"), asked(5, "pending", "new")]
        );
    }

    #[test]
    fn a_lost_answer_is_found_by_its_question() {
        let held = vec![asked(1, "done", "a")];
        let read = vec![asked(1, "done", "a"), asked(2, "pending", "b")];
        assert!(stored_unseen(&held, &read, "b"));
        assert!(stored_unseen(&held, &read, "  b \n"), "stored trimmed");
        // Another question's turn is in flight: this one was not stored.
        assert!(!stored_unseen(&held, &read, "c"));
        // A turn the page already had is not news, even with the same text.
        assert!(!stored_unseen(&held, &read, "a"));
    }

    #[test]
    fn the_end_stops_clear_of_the_box_as_tall_as_it_is() {
        // The empty box on a phone, above its 78 px nav.
        assert_eq!(end_scroll_margin(156.0, 78.0), 246.0);
        // A draft grows it, and the margin with it.
        assert_eq!(end_scroll_margin(250.0, 78.0), 340.0);
        // On a desktop nothing stands under it.
        assert_eq!(end_scroll_margin(139.0, 0.0), 151.0);
        // Never less than the gap, whatever the browser reports.
        assert_eq!(end_scroll_margin(f64::NAN, -5.0), END_GAP_PX);
    }

    #[test]
    fn lengths_read_from_the_browser() {
        assert_eq!(px_of("78px"), 78.0);
        assert_eq!(px_of(" 112.5px "), 112.5);
        assert_eq!(px_of("0px"), 0.0);
        for other in [
            "auto",
            "",
            "px",
            "calc(78px + 1px)",
            "NaNpx",
            "infpx",
            "5em",
        ] {
            assert_eq!(px_of(other), 0.0, "{other}");
        }
    }

    #[test]
    fn the_box_is_cleared_only_if_it_still_holds_the_question() {
        assert!(box_holds("  what next?\n", "what next?"));
        assert!(box_holds("what next?", "  what next? "));
        assert!(!box_holds("what next? and also", "what next?"));
        assert!(!box_holds("", "what next?"));
    }

    #[test]
    fn refusals_stay_where_they_were_given() {
        let a = turn("done", "a", None);
        let mut b = a.clone();
        b.id = 2;
        let r = Refusal {
            question: "q".into(),
            message: "m".into(),
            safety_line: false,
        };
        let turns = [a, b];
        let refusals = [(0, r.clone()), (1, r.clone()), (2, r.clone())];
        assert_eq!(
            conversation(&turns, &refusals),
            vec![
                Entry::Refusal(&refusals[0].1),
                Entry::Turn(&turns[0]),
                Entry::Refusal(&refusals[1].1),
                Entry::Turn(&turns[1]),
                Entry::Refusal(&refusals[2].1),
            ]
        );
    }

    #[test]
    fn checks_read_a_stored_row_with_missing_fields() {
        let c: ChatChecks = serde_json::from_str(r#"{"uncited":true}"#).unwrap();
        assert!(c.uncited && c.not_traced.is_empty());
    }
}
