//! The run page's tracker rules (no Leptos, like [`crate::answer_page`]):
//! what the stored step lines say about a run, for the results card, the
//! fill pills and the folded Progress pill.
//!
//! The engine's step lines are read here as written; nothing is stored or
//! fetched for this. A line this file does not know goes to the rolling line
//! and the full list as it is, and never breaks the page. The tests below
//! use step lines copied by hand from real runs and from the engine's format
//! strings, so they catch a change here but not an engine change: a reworded
//! line in the engine falls through to the rolling line unnoticed.

use chrono::{DateTime, Utc};

use crate::runs::RunMode;

/// One stored step line.
#[derive(Debug, Clone, Copy)]
pub struct Step<'a> {
    pub at: DateTime<Utc>,
    pub line: &'a str,
}

/// How the run stands, from its status (the tracker never decides this).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunState {
    Waiting,
    Working,
    /// Ended with a written answer (done or partial).
    Answered,
    /// Ended normally with no answer written (no evidence, the writer cut
    /// off, or a stored body that cannot be read).
    EndedEmpty,
    /// Failed or interrupted.
    Stopped,
}

/// A card row's value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Value {
    Number { n: u32, of: Option<u32> },
    State(String),
    Pending,
    NotReached,
    NotWritten,
}

/// One row of the results card.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub name: &'static str,
    /// Shown only once the row has a value.
    pub sub: Option<String>,
    pub value: Value,
    /// The small bar, 0 to 100.
    pub bar: Option<u8>,
    /// The sub-line stays on the phone's compact finished card.
    pub keep_sub: bool,
}

/// A pill's state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PillState {
    /// White, muted label.
    Idle,
    /// Lavender filling.
    Filling,
    /// Dull sage, with the dot.
    Ready,
    /// The fill stopped: white, never green.
    Stopped,
}

impl PillState {
    /// The pill's `data-state`.
    pub fn as_str(self) -> &'static str {
        match self {
            PillState::Idle => "idle",
            PillState::Filling => "filling",
            PillState::Ready => "ready",
            PillState::Stopped => "stopped",
        }
    }

    /// The pill's name and state, in words for screen readers.
    pub fn aria(self, label: &str) -> String {
        match self {
            PillState::Idle => format!("{label}, not started"),
            PillState::Filling => format!("{label}, being prepared"),
            PillState::Ready => format!("{label}, ready"),
            // The label already says why (today's message).
            PillState::Stopped => label.to_string(),
        }
    }
}

/// The Answer pill's fill: a floor from the steps, and during the writer
/// (which has no steps) a stretch by time. Copied into the page's closures.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Fill {
    floor: f32,
    timed: Option<Timed>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct Timed {
    from: DateTime<Utc>,
    to_pct: f32,
    usual_secs: f32,
}

/// The fill never reaches full until the run has really finished.
const FILL_HOLD: f32 = 98.0;

impl Fill {
    /// The percentage at `now`: only moves forward, and holds short of full.
    pub fn at(&self, now: DateTime<Utc>) -> u8 {
        let pct = match self.timed {
            None => self.floor,
            Some(t) => {
                let secs = (now - t.from).num_milliseconds() as f32 / 1000.0;
                let share = (secs / t.usual_secs).clamp(0.0, 1.0);
                self.floor + (t.to_pct - self.floor) * share
            }
        };
        pct.clamp(0.0, FILL_HOLD) as u8
    }
}

/// What the step lines say.
#[derive(Debug, Clone, PartialEq)]
pub struct Tracker {
    /// At least one line was recognised: the card can be drawn.
    pub recognised: bool,
    pub rows: Vec<Row>,
    /// The plain sentence in the card's header.
    pub sentence: String,
    /// The role label on the Progress pill.
    pub role: &'static str,
    /// The latest step, for the rolling line.
    pub rolling: String,
    /// The step before it, for the roll.
    pub previous: Option<String>,
    pub fill: Fill,
    /// The run ended with a written answer: the tick.
    pub ended_well: bool,
}

/// Stage S1: the words after a writer's role in its first-words step line.
const FIRST_WORDS: &str = "first words after ";
/// The role of the Expert Opinion writer's first-words step line.
const EXPERT_ROLE: &str = "Expert Opinion";

/// A streamed writer's step line: how long until its first words.
pub fn first_words_line(expert: bool, secs: u64) -> String {
    let role = if expert { EXPERT_ROLE } else { "Writer" };
    format!("{role}: {FIRST_WORDS}{secs} s")
}

/// S1 (`QUICK_PARALLEL=on`): the words after "Writer: section <name>" when a
/// part of a Quick answer written in four parts failed twice.
const ONE_PIECE: &str = " failed; writing the answer in one piece";

/// The step line when the four-part writer falls back to the single writer.
pub fn one_piece_line(section: &str) -> String {
    format!("Writer: section {section}{ONE_PIECE}")
}

/// The Expert Opinion writer's first-words line. It is stored after the run has
/// ended, so it says nothing about when the answer was ready.
pub fn is_expert_first_words(line: &str) -> bool {
    line.strip_prefix(EXPERT_ROLE)
        .and_then(|l| l.strip_prefix(": "))
        .is_some_and(|l| l.starts_with(FIRST_WORDS))
}

/// The usual time to the answer, under the clock.
pub fn usual_label(mode: RunMode) -> &'static str {
    match mode {
        RunMode::Quick => "usually about 2 min",
        RunMode::Focused => "usually 5 min",
    }
}

/// Past this many seconds the tagline shows.
fn tagline_after(mode: RunMode) -> i64 {
    match mode {
        RunMode::Quick => 120,
        RunMode::Focused => 300,
    }
}

/// Whether the tagline shows after `secs` of a working run.
pub fn tagline_due(mode: RunMode, secs: i64) -> bool {
    secs > tagline_after(mode)
}

/// The tagline.
pub const TAGLINE: &str = "We're doing deep research to give you a better answer.";
/// The line under a working card.
pub const LEAVE_LINE: &str = "You can leave this page. Your answer will be in the ";

/// The usual Expert Opinion, last 5 Full runs (3 Nov).
pub const EXPERT_USUAL_SECS: f32 = 176.0;
/// Chat's getting-ready fill: no measurement yet, set when Chat comes back.
pub const CHAT_USUAL_SECS: f32 = 40.0;

/// A by-time fill after `secs` of a usual `usual` seconds: holds short of
/// full until the state says ready.
pub fn timed_fill(secs: i64, usual: f32) -> u8 {
    ((secs.max(0) as f32 / usual) * 100.0).clamp(0.0, FILL_HOLD) as u8
}

/// The pill's fill: lavender fading softly into white at `pct`.
pub fn fill_background(pct: u8) -> String {
    format!(
        "background:linear-gradient(to right,#E6ECF8 {}%,#FFFFFF {}%)",
        pct.saturating_sub(14),
        pct
    )
}

/// `m:ss`.
pub fn clock_text(secs: i64) -> String {
    let secs = secs.max(0);
    format!("{}:{:02}", secs / 60, secs % 60)
}

/// The number that opens `s`, if any.
fn lead_number(s: &str) -> Option<u32> {
    let d: String = s.chars().take_while(char::is_ascii_digit).collect();
    d.parse().ok()
}

/// `t` and `T` in "… (turn t of T)".
fn turn_of(s: &str) -> Option<(u32, u32)> {
    let rest = &s[s.find("(turn ")? + 6..];
    let t = lead_number(rest)?;
    let rest = rest.split_once(" of ")?.1;
    Some((t, lead_number(rest)?))
}

fn capitalised(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().chain(c).collect(),
        None => String::new(),
    }
}

/// The role a line belongs to and the text to roll (its role's prefix off,
/// unless the prefix says which check it is).
fn role_of(line: &str, last: &'static str) -> (&'static str, String) {
    if line == "Answer ready" {
        return ("Done", line.to_string());
    }
    if let Some((head, body)) = line.split_once(": ") {
        let role = match head {
            "Researcher" => Some("Researcher"),
            "Skeptic" => Some("Skeptic"),
            "Extractor" => Some("Extractor"),
            "Writer" => Some("Writer"),
            "Extractor and skeptic" => Some("Extractor + Skeptic"),
            "Citation check"
            | "Number check"
            | "Coverage"
            | "Completeness"
            | "Completeness review"
            | "Completeness review skipped" => return ("Checker", line.to_string()),
            _ => None,
        };
        if let Some(role) = role {
            return (role, capitalised(body));
        }
    }
    let role = if line.starts_with("Search stopped") || line.starts_with("Research stopped") {
        "Researcher"
    } else if line.starts_with("Extraction ") {
        "Extractor"
    } else if line.starts_with("The writer") {
        "Writer"
    } else {
        last
    };
    (role, line.to_string())
}

/// What the lines build up to.
#[derive(Default)]
struct Seen {
    any: bool,
    searches: Option<u32>,
    selected: Option<u32>,
    selected_mix: String,
    read: Option<u32>,
    read_of: Option<u32>,
    outcomes: Option<u32>,
    first_read: u32,
    first_of: u32,
    first_outcomes: u32,
    second_wave: bool,
    /// The wave timed out. Its line writer outlives the cancellation, so a
    /// progress line stored after the note must not count.
    wave_abandoned: bool,
    wave_read: u32,
    wave_of: u32,
    skeptic: Option<String>,
    skeptic_sub: Option<String>,
    answer: Option<&'static str>,
    citations: Option<String>,
    numbers: Option<String>,
    completeness: Option<String>,
    sentence: Option<String>,
    floor: f32,
    timed: Option<Timed>,
}

impl Seen {
    fn not_read(&mut self, n: u32) {
        self.read = self.read.map(|r| r.saturating_sub(n));
    }

    fn raise(&mut self, pct: f32) {
        if pct > self.floor {
            self.floor = pct;
        }
    }

    /// The fill's bands, by mode: (searching ends, first wave ends, second
    /// wave ends, writing ends, usual writer seconds).
    fn bands(mode: RunMode) -> (f32, f32, f32, f32, f32) {
        match mode {
            RunMode::Quick => (45.0, 45.0, 45.0, 90.0, 67.0),
            RunMode::Focused => (35.0, 58.0, 65.0, 90.0, 102.0),
        }
    }

    fn read_line(&mut self, mode: RunMode, s: Step<'_>) {
        let (search_end, first_end, second_end, write_end, usual) = Seen::bands(mode);
        let line = s.line;
        if line.starts_with("Search stopped") || line.starts_with("Research stopped") {
            self.raise(search_end);
        }
        // A study counted by a progress line that was not read after all.
        if let Some(rest) = line.strip_prefix("Extraction failed for ") {
            self.not_read(lead_number(rest).unwrap_or(0));
        }
        if self.second_wave && line.contains(" picked by the skeptic ") {
            if line.contains("not extracted (time)") {
                // The wave timed out: none of it counts.
                self.read = Some(self.first_read);
                self.read_of = Some(self.first_of);
                self.outcomes = Some(self.first_outcomes);
                self.second_wave = false;
                self.wave_abandoned = true;
            } else if line.contains("not extracted (extraction failed)") {
                self.not_read(lead_number(line).unwrap_or(0));
            }
        }
        let Some((head, body)) = line.split_once(": ") else {
            return;
        };
        match head {
            "Researcher" => {
                self.any = true;
                if let Some(rest) = body.strip_prefix("selected ") {
                    self.selected = lead_number(rest);
                    if let Some((_, mix)) = rest.split_once(" (") {
                        self.selected_mix = mix.trim_end_matches(')').to_string();
                    }
                    self.raise(search_end);
                } else if body.starts_with("searching ") {
                    self.sentence = Some("Searching PubMed and Europe PMC".into());
                } else if body.contains(" so far") {
                    self.searches = lead_number(body).or(self.searches);
                    if let Some((t, of)) = turn_of(body) {
                        self.raise(search_end * t as f32 / of.max(1) as f32);
                    }
                }
            }
            "Extractor and skeptic" => {
                self.any = true;
                self.sentence = Some("The researchers are discussing the best answer".into());
                self.skeptic = Some("starting".into());
                if let Some(rest) = body.strip_prefix("extracting ") {
                    self.first_of = lead_number(rest).unwrap_or(0);
                    self.read_of = Some(self.first_of);
                    self.read = Some(0);
                }
                self.raise(search_end);
            }
            "Skeptic" => {
                self.any = true;
                if let Some(rest) = body.strip_prefix("verdict ") {
                    // "holds with caveats, 6 caveats" reads "holds, 6 caveats".
                    let verdict = rest.replace(" with caveats,", ",");
                    self.skeptic = Some(format!("verdict {verdict}"));
                    self.skeptic_sub = Some("Conclusion challenged".into());
                } else if body.starts_with("the review could not be completed") {
                    self.skeptic = Some("could not be completed".into());
                    self.skeptic_sub = Some("Answer written without it".into());
                } else if let Some(n) = lead_number(body).filter(|_| body.contains(" so far")) {
                    let noun = if n == 1 { "search" } else { "searches" };
                    self.skeptic = Some(format!("{n} {noun}"));
                }
            }
            "Extractor" => {
                self.any = true;
                if let Some(rest) = body.strip_prefix("second wave, extracting ") {
                    let n = lead_number(rest).unwrap_or(0);
                    self.second_wave = true;
                    self.wave_of = n;
                    self.first_read = self.read.unwrap_or(0);
                    self.first_outcomes = self.outcomes.unwrap_or(0);
                    self.read = Some(self.first_read);
                    self.read_of = Some(self.first_of + n);
                    let noun = if n == 1 { "study" } else { "studies" };
                    self.sentence = Some(format!("Reading {n} more {noun} the skeptic found"));
                    self.skeptic_sub = Some(format!("Found {n} more {noun}"));
                    self.raise(first_end);
                } else if self.wave_abandoned {
                    // A late line from the abandoned wave.
                } else if let Some(n) = lead_number(body) {
                    let Some((_, rest)) = body.split_once('/') else {
                        return;
                    };
                    let of = lead_number(rest).unwrap_or(0);
                    let verified = rest
                        .split_once(" (")
                        .and_then(|(_, r)| lead_number(r))
                        .unwrap_or(0);
                    if self.second_wave {
                        self.wave_read = n;
                        self.read = Some(self.first_read + n);
                        self.outcomes = Some(self.first_outcomes + verified);
                        let share = n as f32 / self.wave_of.max(1) as f32;
                        self.raise(first_end + (second_end - first_end) * share);
                    } else {
                        if self.first_of == 0 {
                            self.first_of = of;
                            self.read_of = Some(of);
                        }
                        self.read = Some(n);
                        self.outcomes = Some(verified);
                        let share = n as f32 / of.max(1) as f32;
                        self.raise(search_end + (first_end - search_end) * share);
                    }
                }
            }
            // Stage S1: a note on the writer already counted; the fill goes on.
            "Writer" if body.starts_with(FIRST_WORDS) => {}
            // S1: a part failed; the answer is written again in one piece, so
            // the writer's timed fill starts again from this line.
            "Writer" if body.starts_with("section ") && body.ends_with(ONE_PIECE) => {
                self.any = true;
                self.answer = Some("being written");
                self.sentence = Some("Writing the answer in one piece".into());
                self.raise(second_end);
                // Never back: the fill goes on from where it had got to.
                let so_far = Fill {
                    floor: self.floor,
                    timed: self.timed,
                }
                .at(s.at);
                self.raise(f32::from(so_far));
                self.timed = Some(Timed {
                    from: s.at,
                    to_pct: write_end,
                    usual_secs: usual,
                });
            }
            "Writer" => {
                self.any = true;
                self.answer = Some("being written");
                let about = body.split(" (").next().unwrap_or(body);
                self.sentence = Some(capitalised(about));
                self.raise(second_end);
                self.timed = Some(Timed {
                    from: s.at,
                    to_pct: write_end,
                    usual_secs: usual,
                });
            }
            "Citation check" => {
                self.any = true;
                self.answer = Some("written");
                self.citations = Some(citation_check(body));
                self.sentence = Some("Checking citations and numbers".into());
                self.timed = None;
                self.raise(write_end + 2.0);
            }
            "Number check" => {
                self.any = true;
                self.numbers = number_check(body);
                self.raise(write_end + 4.0);
            }
            "Coverage" => self.raise(write_end + 5.0),
            "Completeness review" => {
                self.any = true;
                self.sentence = Some("Checking completeness".into());
                let n = body
                    .strip_prefix("checking the answer against ")
                    .unwrap_or(body);
                self.completeness = Some(if lead_number(n) == Some(0) {
                    "checking".to_string()
                } else {
                    format!("checking {n}")
                });
                self.raise(write_end + 6.0);
            }
            "Completeness" => {
                self.any = true;
                self.completeness = Some(match body.split_once(" found;") {
                    Some((gaps, _)) if lead_number(gaps) != Some(0) => format!("{gaps} reviewed"),
                    Some(_) => "no gaps".to_string(),
                    None => "reviewed".to_string(),
                });
                self.raise(FILL_HOLD);
            }
            "Completeness review skipped" => {
                self.any = true;
                self.completeness = Some("skipped".to_string());
                self.raise(FILL_HOLD);
            }
            _ => {}
        }
    }
}

/// "all 413 numbers traced …" reads "all 413 traced"; "6 of 398 numbers not
/// traced …" reads "392 of 398 traced".
fn number_check(body: &str) -> Option<String> {
    if let Some(rest) = body.strip_prefix("all ") {
        let n = lead_number(rest)?;
        return Some(if n == 0 {
            "no numbers to trace".to_string()
        } else {
            format!("all {n} traced")
        });
    }
    let untraced = lead_number(body)?;
    let rest = body.split_once(" of ")?.1;
    let total = lead_number(rest)?;
    Some(format!(
        "{} of {total} traced",
        total.saturating_sub(untraced)
    ))
}

/// "every citation …" reads "all matched"; "removed 1 unknown citation,
/// dropped 2 numbers and 0 trial rows" reads "1 removed, 2 dropped".
fn citation_check(body: &str) -> String {
    if body.starts_with("every citation") {
        return "all matched".to_string();
    }
    let Some(rest) = body.strip_prefix("removed ") else {
        return "changes marked".to_string();
    };
    let removed = lead_number(rest).unwrap_or(0);
    let dropped: u32 = rest
        .split_once(", dropped ")
        .map(|(_, d)| {
            d.split(" and ")
                .filter_map(lead_number)
                .fold(0u32, u32::saturating_add)
        })
        .unwrap_or(0);
    match (removed, dropped) {
        (r, 0) => format!("{r} removed"),
        (0, d) => format!("{d} dropped"),
        (r, d) => format!("{r} removed, {d} dropped"),
    }
}

/// Nothing more will come: a row with no value will never have one.
fn ended(state: RunState) -> bool {
    matches!(
        state,
        RunState::Answered | RunState::EndedEmpty | RunState::Stopped
    )
}

/// A state row's value, by how the run stands.
fn state_value(s: Option<String>, state: RunState) -> Value {
    match s {
        Some(s) => Value::State(s),
        None if ended(state) => Value::NotReached,
        None => Value::Pending,
    }
}

fn number_value(n: Option<u32>, of: Option<u32>, state: RunState) -> Value {
    match n {
        Some(n) => Value::Number { n, of },
        None if ended(state) => Value::NotReached,
        None => Value::Pending,
    }
}

/// What the stored answer says about one study.
#[derive(Debug, Clone, Copy)]
pub struct StudyRead<'a> {
    pub text: &'a str,
    pub verified_quotes: u32,
}

impl Tracker {
    /// A finished run: the stored evidence is the final authority for
    /// "Studies read" (the total, and how each was read, as stored) and for
    /// "Outcomes verified" (the sum over the stored studies). The stored
    /// "not extracted" is a mix (failed, stopped, or no text at all), so it
    /// is never called an abstract. With no evidence the last counts stay,
    /// with no sub-line.
    pub fn read_from_evidence(&mut self, studies: &[StudyRead<'_>]) {
        if studies.is_empty() {
            return;
        }
        let count = |f: &dyn Fn(&str) -> bool| {
            u32::try_from(studies.iter().filter(|s| f(s.text)).count()).unwrap_or(u32::MAX)
        };
        let full = count(&|t| t == "full text");
        let abstract_only = count(&|t| t == "abstract only");
        let total = u32::try_from(studies.len()).unwrap_or(u32::MAX);
        let other = total - full - abstract_only;
        let sub = [
            (full, "full text"),
            (abstract_only, "abstract only"),
            (other, "not extracted"),
        ]
        .iter()
        .filter(|(n, _)| *n > 0)
        .map(|(n, label)| format!("{n} {label}"))
        .collect::<Vec<_>>()
        .join(", ");
        if let Some(r) = self.rows.iter_mut().find(|r| r.name == "Studies read") {
            r.value = Value::Number { n: total, of: None };
            r.bar = None;
            r.sub = Some(sub);
            r.keep_sub = true;
        }
        if let Some(r) = self.rows.iter_mut().find(|r| r.name == "Outcomes verified") {
            let n = studies
                .iter()
                .fold(0u32, |sum, s| sum.saturating_add(s.verified_quotes));
            r.value = Value::Number { n, of: None };
        }
    }
}

/// Read a run's stored steps. `empty_reason`: why a run that ended empty has
/// no answer, as stored.
pub fn read(
    mode: RunMode,
    steps: &[Step<'_>],
    state: RunState,
    empty_reason: Option<&str>,
) -> Tracker {
    let mut seen = Seen::default();
    let mut role = "Starting";
    let mut rolling = String::new();
    let mut previous = None;
    for s in steps {
        seen.read_line(mode, *s);
        let (r, text) = role_of(s.line, role);
        role = r;
        previous = Some(std::mem::replace(&mut rolling, text));
    }
    let previous = previous.filter(|p: &String| !p.is_empty());
    if steps.is_empty() {
        rolling = "Nothing yet. This page updates by itself.".into();
    }
    let full = mode == RunMode::Focused;
    let mut rows = vec![
        Row {
            keep_sub: false,
            name: "Searches run",
            sub: Some("PubMed and Europe PMC".into()),
            value: number_value(seen.searches, None, state),
            bar: None,
        },
        Row {
            keep_sub: false,
            name: "Studies selected",
            sub: (!seen.selected_mix.is_empty()).then(|| seen.selected_mix.clone()),
            value: number_value(seen.selected, None, state),
            bar: None,
        },
    ];
    if full {
        rows.push(Row {
            keep_sub: false,
            name: "Studies read",
            sub: None,
            value: number_value(seen.read, seen.read_of, state),
            bar: match (seen.read, seen.read_of) {
                (Some(n), Some(of)) if of > 0 => Some((n.min(of) * 100 / of) as u8),
                _ => None,
            },
        });
        rows.push(Row {
            keep_sub: false,
            name: "Outcomes verified",
            sub: Some("Quotes matched against the source".into()),
            value: number_value(seen.outcomes, None, state),
            bar: None,
        });
        rows.push(Row {
            keep_sub: false,
            name: "Skeptic's challenge",
            sub: Some(
                seen.skeptic_sub
                    .clone()
                    .unwrap_or_else(|| "Looking for what the team missed".into()),
            ),
            value: state_value(seen.skeptic.clone(), state),
            bar: None,
        });
    }
    // Written only if a written answer was stored.
    let answer_value = match state {
        RunState::Answered => Value::State("written".to_string()),
        RunState::EndedEmpty => Value::NotWritten,
        RunState::Stopped => Value::NotReached,
        RunState::Waiting | RunState::Working => {
            state_value(seen.answer.map(str::to_string), state)
        }
    };
    rows.push(Row {
        keep_sub: false,
        name: "Answer",
        sub: Some(
            if full {
                "Written from the studies read"
            } else {
                "Written from the studies selected"
            }
            .into(),
        ),
        value: answer_value,
        bar: None,
    });
    rows.push(Row {
        keep_sub: false,
        name: "Citations checked",
        sub: None,
        value: state_value(seen.citations.clone(), state),
        bar: None,
    });
    rows.push(Row {
        keep_sub: false,
        name: "Numbers traced",
        sub: None,
        value: state_value(seen.numbers.clone(), state),
        bar: None,
    });
    // With the review switched off, a finished run has no such row.
    if full && (seen.completeness.is_some() || !matches!(state, RunState::Answered)) {
        rows.push(Row {
            keep_sub: false,
            name: "Completeness review",
            sub: None,
            value: state_value(seen.completeness.clone(), state),
            bar: None,
        });
    }
    let sentence = match state {
        RunState::Waiting => "Waiting to start".to_string(),
        RunState::Answered => "Answer ready".to_string(),
        RunState::EndedEmpty => empty_reason
            .filter(|r| !r.is_empty())
            .unwrap_or("No answer was written")
            .to_string(),
        RunState::Stopped => "The run ended before the answer was ready".to_string(),
        RunState::Working => seen.sentence.clone().unwrap_or_else(|| {
            if seen.any {
                "Searching PubMed and Europe PMC"
            } else {
                "Starting"
            }
            .to_string()
        }),
    };
    Tracker {
        recognised: seen.any,
        rows,
        sentence,
        role,
        rolling,
        previous,
        fill: Fill {
            floor: seen.floor,
            timed: seen.timed,
        },
        ended_well: state == RunState::Answered,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn t(secs: i64) -> DateTime<Utc> {
        Utc.timestamp_opt(1_800_000_000 + secs, 0).unwrap()
    }

    const QUICK: [&str; 10] = [
        "Researcher: searching PubMed and Europe PMC",
        "Researcher: 2 searches so far (turn 1 of 8)",
        "Researcher: 5 searches so far (turn 2 of 8)",
        "Researcher: 6 searches so far (turn 3 of 8)",
        "Researcher: selected 10 studies (8 RCTs, 2 meta-analyses)",
        "Search stopped at the Quick limit after 3 rounds",
        "Writer: writing the answer from 10 studies (10 with an abstract)",
        "Citation check: every citation is one of the selected studies",
        "Number check: 6 of 398 numbers not traced to a verified quote, structured number or abstract (marked \"not traced\"; nothing removed)",
        "Answer ready",
    ];

    const FULL: [&str; 42] = [
        "Researcher: searching PubMed and Europe PMC",
        "Researcher: 2 searches so far (turn 1 of 15)",
        "Researcher: 5 searches so far (turn 2 of 15)",
        "Researcher: 6 searches so far (turn 3 of 15)",
        "Researcher: 7 searches so far (turn 4 of 15)",
        "Researcher: 8 searches so far (turn 5 of 15)",
        "Researcher: 10 searches so far (turn 6 of 15)",
        "Researcher: 11 searches so far (turn 7 of 15)",
        "Researcher: 11 searches so far (turn 8 of 15)",
        "Researcher: 11 searches so far (turn 9 of 15)",
        "Researcher: selected 12 studies (7 RCTs, 4 meta-analyses)",
        "Extractor and skeptic: extracting 12 studies and challenging the conclusion",
        "Skeptic: 4 searches so far (turn 1 of 8)",
        "Skeptic: 5 searches so far (turn 2 of 8)",
        "Skeptic: 7 searches so far (turn 3 of 8)",
        "Skeptic: 7 searches so far (turn 4 of 8)",
        "Skeptic: 7 searches so far (turn 5 of 8)",
        "Extractor: 1/12 studies (3 verified outcomes, 0 not verified, 0 numbers not verified; all kept, labelled)",
        "Extractor: 2/12 studies (7 verified outcomes, 0 not verified, 0 numbers not verified; all kept, labelled)",
        "Extractor: 3/12 studies (13 verified outcomes, 0 not verified, 3 numbers not verified; all kept, labelled)",
        "Extractor: 4/12 studies (17 verified outcomes, 0 not verified, 4 numbers not verified; all kept, labelled)",
        "Extractor: 5/12 studies (21 verified outcomes, 0 not verified, 4 numbers not verified; all kept, labelled)",
        "Skeptic: verdict holds with caveats, 6 caveats",
        "Skeptic: 2 studies whose quote was not found kept as unverified leads (PMID only)",
        "Extractor: 6/12 studies (26 verified outcomes, 0 not verified, 4 numbers not verified; all kept, labelled)",
        "Extractor: 7/12 studies (28 verified outcomes, 0 not verified, 4 numbers not verified; all kept, labelled)",
        "Extractor: 8/12 studies (32 verified outcomes, 0 not verified, 6 numbers not verified; all kept, labelled)",
        "Extractor: 9/12 studies (34 verified outcomes, 0 not verified, 6 numbers not verified; all kept, labelled)",
        "Extractor: 10/12 studies (36 verified outcomes, 0 not verified, 6 numbers not verified; all kept, labelled)",
        "Extractor: 11/12 studies (40 verified outcomes, 0 not verified, 6 numbers not verified; all kept, labelled)",
        "Extractor: 12/12 studies (42 verified outcomes, 0 not verified, 7 numbers not verified; all kept, labelled)",
        "Extractor: second wave, extracting 3 studies found by the skeptic",
        "Extractor: 1/3 studies (0 verified outcomes, 0 not verified, 0 numbers not verified; all kept, labelled)",
        "Extractor: 2/3 studies (4 verified outcomes, 0 not verified, 3 numbers not verified; all kept, labelled)",
        "Extractor: 3/3 studies (8 verified outcomes, 0 not verified, 3 numbers not verified; all kept, labelled)",
        "Writer: writing the answer from 15 studies (15 with an abstract)",
        "Citation check: every citation is one of the selected studies",
        "Number check: all 413 numbers traced to a verified quote, structured number or abstract",
        "Coverage: 1 verified number and 5 outcomes not in the draft",
        "Completeness review: checking the answer against 6 gaps",
        "Completeness: 6 gaps found; 0 added, 5 judged not relevant (listed) and 3 notes",
        "Answer ready",
    ];

    fn steps<'a>(lines: &[&'a str]) -> Vec<Step<'a>> {
        lines
            .iter()
            .enumerate()
            .map(|(i, line)| Step {
                at: t(i as i64 * 5),
                line,
            })
            .collect()
    }

    fn row<'a>(tr: &'a Tracker, name: &str) -> &'a Row {
        tr.rows.iter().find(|r| r.name == name).unwrap()
    }

    fn number(n: u32) -> Value {
        Value::Number { n, of: None }
    }

    fn state(s: &str) -> Value {
        Value::State(s.into())
    }

    #[test]
    fn a_finished_quick_run_fills_every_row() {
        let tr = read(RunMode::Quick, &steps(&QUICK), RunState::Answered, None);
        assert!(tr.recognised);
        let names: Vec<_> = tr.rows.iter().map(|r| r.name).collect();
        assert_eq!(
            names,
            [
                "Searches run",
                "Studies selected",
                "Answer",
                "Citations checked",
                "Numbers traced"
            ]
        );
        assert_eq!(row(&tr, "Searches run").value, number(6));
        assert_eq!(row(&tr, "Studies selected").value, number(10));
        assert_eq!(
            row(&tr, "Studies selected").sub.as_deref(),
            Some("8 RCTs, 2 meta-analyses")
        );
        assert_eq!(row(&tr, "Answer").value, state("written"));
        assert_eq!(row(&tr, "Citations checked").value, state("all matched"));
        assert_eq!(row(&tr, "Numbers traced").value, state("392 of 398 traced"));
        assert_eq!(tr.sentence, "Answer ready");
        assert_eq!(tr.role, "Done");
    }

    #[test]
    fn a_finished_full_run_adds_both_waves() {
        let tr = read(RunMode::Focused, &steps(&FULL), RunState::Answered, None);
        assert_eq!(row(&tr, "Searches run").value, number(11));
        assert_eq!(row(&tr, "Studies selected").value, number(12));
        assert_eq!(
            row(&tr, "Studies read").value,
            Value::Number {
                n: 15,
                of: Some(15)
            }
        );
        assert_eq!(row(&tr, "Studies read").bar, Some(100));
        assert_eq!(row(&tr, "Outcomes verified").value, number(50));
        assert_eq!(
            row(&tr, "Skeptic's challenge").value,
            state("verdict holds, 6 caveats")
        );
        assert_eq!(
            row(&tr, "Skeptic's challenge").sub.as_deref(),
            Some("Found 3 more studies")
        );
        assert_eq!(row(&tr, "Numbers traced").value, state("all 413 traced"));
        assert_eq!(
            row(&tr, "Completeness review").value,
            state("6 gaps reviewed")
        );
    }

    #[test]
    fn the_mockup_moment_at_step_21_of_a_full_run() {
        let tr = read(
            RunMode::Focused,
            &steps(&FULL[..21]),
            RunState::Working,
            None,
        );
        assert_eq!(
            row(&tr, "Studies read").value,
            Value::Number { n: 4, of: Some(12) }
        );
        assert_eq!(row(&tr, "Studies read").bar, Some(33));
        assert_eq!(row(&tr, "Outcomes verified").value, number(17));
        assert_eq!(row(&tr, "Skeptic's challenge").value, state("7 searches"));
        assert_eq!(row(&tr, "Answer").value, Value::Pending);
        assert_eq!(row(&tr, "Completeness review").value, Value::Pending);
        assert_eq!(
            tr.sentence,
            "The researchers are discussing the best answer"
        );
        assert_eq!(tr.role, "Extractor");
        assert!(tr.rolling.starts_with("4/12 studies (17 verified outcomes"));
        assert!(tr.previous.as_deref().unwrap().starts_with("3/12 studies"));
    }

    #[test]
    fn each_full_sentence_appears_at_its_stage() {
        let at = |n: usize| {
            read(
                RunMode::Focused,
                &steps(&FULL[..n]),
                RunState::Working,
                None,
            )
            .sentence
        };
        assert_eq!(at(1), "Searching PubMed and Europe PMC");
        assert_eq!(at(12), "The researchers are discussing the best answer");
        assert_eq!(at(32), "Reading 3 more studies the skeptic found");
        assert_eq!(at(36), "Writing the answer from 15 studies");
        assert_eq!(at(37), "Checking citations and numbers");
        assert_eq!(at(40), "Checking completeness");
    }

    #[test]
    fn roles_follow_the_lines() {
        let at = |n: usize| {
            read(
                RunMode::Focused,
                &steps(&FULL[..n]),
                RunState::Working,
                None,
            )
            .role
        };
        assert_eq!(at(0), "Starting");
        assert_eq!(at(1), "Researcher");
        assert_eq!(at(12), "Extractor + Skeptic");
        assert_eq!(at(13), "Skeptic");
        assert_eq!(at(18), "Extractor");
        assert_eq!(at(36), "Writer");
        assert_eq!(at(37), "Checker");
        assert_eq!(at(42), "Done");
    }

    #[test]
    fn unknown_lines_roll_as_written_and_keep_the_role() {
        let lines = [
            "Researcher: searching PubMed and Europe PMC",
            "Something new the engine says: with a colon",
            "No colon at all",
        ];
        let tr = read(RunMode::Quick, &steps(&lines), RunState::Working, None);
        assert!(tr.recognised);
        assert_eq!(tr.rolling, "No colon at all");
        assert_eq!(tr.role, "Researcher");
    }

    #[test]
    fn no_recognised_line_draws_no_card() {
        let lines = ["Searching", "Screening 12 abstracts"];
        let tr = read(RunMode::Quick, &steps(&lines), RunState::Answered, None);
        assert!(!tr.recognised);
    }

    #[test]
    fn nothing_yet_is_waiting() {
        let tr = read(RunMode::Focused, &[], RunState::Waiting, None);
        assert_eq!(tr.sentence, "Waiting to start");
        assert_eq!(tr.role, "Starting");
        assert_eq!(tr.rolling, "Nothing yet. This page updates by itself.");
        assert_eq!(tr.fill.at(t(0)), 0);
        assert!(tr.rows.iter().all(|r| r.value == Value::Pending));
    }

    #[test]
    fn a_stopped_run_says_what_was_not_reached() {
        let lines = &FULL[..36];
        let tr = read(RunMode::Focused, &steps(lines), RunState::Stopped, None);
        assert_eq!(row(&tr, "Searches run").value, number(11));
        assert_eq!(row(&tr, "Answer").value, Value::NotReached);
        assert_eq!(row(&tr, "Citations checked").value, Value::NotReached);
        assert_eq!(row(&tr, "Completeness review").value, Value::NotReached);
    }

    #[test]
    fn a_finished_run_without_the_review_has_no_such_row() {
        let lines: Vec<&str> = FULL
            .iter()
            .copied()
            .filter(|l| !l.starts_with("Completeness"))
            .collect();
        let tr = read(RunMode::Focused, &steps(&lines), RunState::Answered, None);
        assert!(tr.rows.iter().all(|r| r.name != "Completeness review"));
    }

    #[test]
    fn a_finished_run_never_says_pending() {
        let tr = read(
            RunMode::Focused,
            &steps(&FULL[..5]),
            RunState::Answered,
            None,
        );
        assert_eq!(row(&tr, "Answer").value, state("written"));
        assert!(tr.ended_well);
        assert_eq!(row(&tr, "Citations checked").value, Value::NotReached);
        assert!(tr.rows.iter().all(|r| r.value != Value::Pending));
    }

    #[test]
    fn a_run_that_ended_empty_says_so_and_gets_no_tick() {
        // The engine writes "Answer ready" here too.
        let lines = [
            "Researcher: selected 12 studies (7 RCTs, 4 meta-analyses)",
            "Writer: writing the answer from 12 studies (12 with an abstract)",
            "The writer's answer was cut off by its output limit",
            "Answer ready",
        ];
        let why = "The answer was longer than the writer's output limit.";
        let tr = read(
            RunMode::Focused,
            &steps(&lines),
            RunState::EndedEmpty,
            Some(why),
        );
        assert_eq!(row(&tr, "Answer").value, Value::NotWritten);
        assert_eq!(tr.sentence, why);
        assert!(!tr.ended_well);
        let tr = read(RunMode::Quick, &steps(&lines), RunState::EndedEmpty, None);
        assert_eq!(tr.sentence, "No answer was written");
    }

    #[test]
    fn an_interrupted_run_never_says_written() {
        // Interrupted after the checks: the answer was never stored.
        let tr = read(
            RunMode::Focused,
            &steps(&FULL[..40]),
            RunState::Stopped,
            None,
        );
        assert_eq!(row(&tr, "Answer").value, Value::NotReached);
        assert_eq!(tr.sentence, "The run ended before the answer was ready");
        assert!(!tr.ended_well);
    }

    #[test]
    fn a_skeptic_failure_says_so() {
        let lines = [
            "Extractor and skeptic: extracting 12 studies and challenging the conclusion",
            "Skeptic: the review could not be completed; the answer is written without it",
        ];
        let tr = read(RunMode::Focused, &steps(&lines), RunState::Working, None);
        let r = row(&tr, "Skeptic's challenge");
        assert_eq!(r.value, state("could not be completed"));
        assert_eq!(r.sub.as_deref(), Some("Answer written without it"));
        assert_eq!(tr.role, "Skeptic");
    }

    #[test]
    fn a_verdict_that_does_not_hold_reads_as_written() {
        let lines = ["Skeptic: verdict does not hold, 2 caveats"];
        let tr = read(RunMode::Focused, &steps(&lines), RunState::Working, None);
        assert_eq!(
            row(&tr, "Skeptic's challenge").value,
            state("verdict does not hold, 2 caveats")
        );
    }

    #[test]
    fn a_skipped_completeness_says_so() {
        let lines = ["Completeness review skipped: time; the answer is as the writer wrote it"];
        let tr = read(RunMode::Focused, &steps(&lines), RunState::Working, None);
        assert_eq!(row(&tr, "Completeness review").value, state("skipped"));
        assert_eq!(tr.role, "Checker");
    }

    #[test]
    fn citation_removals_read_short() {
        assert_eq!(
            citation_check("removed 1 unknown citation, dropped 0 numbers and 0 trial rows"),
            "1 removed"
        );
        assert_eq!(
            citation_check("removed 2 unknown citations, dropped 3 numbers and 1 trial row"),
            "2 removed, 4 dropped"
        );
        assert_eq!(
            citation_check("removed 0 unknown citations, dropped 1 number and 0 trial rows"),
            "1 dropped"
        );
    }

    #[test]
    fn zero_counts_read_sensibly() {
        assert_eq!(
            number_check("all 0 numbers traced to a verified quote").as_deref(),
            Some("no numbers to trace")
        );
        let lines = ["Completeness: 0 gaps found; 0 added"];
        let tr = read(RunMode::Focused, &steps(&lines), RunState::Working, None);
        assert_eq!(row(&tr, "Completeness review").value, state("no gaps"));
    }

    const WAVE: [&str; 3] = [
        "Extractor: 12/12 studies (42 verified outcomes, 0 not verified, 7 numbers not verified; all kept, labelled)",
        "Extractor: second wave, extracting 3 studies found by the skeptic",
        "Extractor: 2/3 studies (4 verified outcomes, 0 not verified, 3 numbers not verified; all kept, labelled)",
    ];

    fn read_after_wave(note: &str) -> Tracker {
        let mut lines =
            vec!["Extractor and skeptic: extracting 12 studies and challenging the conclusion"];
        lines.extend(WAVE);
        lines.push(note);
        read(RunMode::Focused, &steps(&lines), RunState::Working, None)
    }

    #[test]
    fn a_wave_that_timed_out_does_not_count() {
        let tr = read_after_wave(
            "3 studies picked by the skeptic were not extracted (time); the writer was given their abstract",
        );
        assert_eq!(
            row(&tr, "Studies read").value,
            Value::Number {
                n: 12,
                of: Some(12)
            }
        );
        assert_eq!(row(&tr, "Outcomes verified").value, number(42));
    }

    #[test]
    fn a_failed_study_in_the_wave_is_not_read() {
        let tr = read_after_wave(
            "1 study picked by the skeptic was not extracted (extraction failed); the writer was given its abstract",
        );
        assert_eq!(
            row(&tr, "Studies read").value,
            Value::Number {
                n: 13,
                of: Some(15)
            }
        );
    }

    fn fixture_studies(json: &str) -> Vec<(String, u32)> {
        let v: serde_json::Value = serde_json::from_str(json).unwrap();
        v["evidence"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| {
                (
                    e["text"].as_str().unwrap().to_string(),
                    e["verified_quotes"].as_u64().unwrap() as u32,
                )
            })
            .collect()
    }

    fn finish(tr: &mut Tracker, studies: &[(&str, u32)]) {
        let s: Vec<StudyRead<'_>> = studies
            .iter()
            .map(|(text, verified_quotes)| StudyRead {
                text,
                verified_quotes: *verified_quotes,
            })
            .collect();
        tr.read_from_evidence(&s);
    }

    #[test]
    fn a_finished_run_counts_how_each_study_was_read() {
        let cases = [
            (
                include_str!("../tests/fixtures/temporal/full/hernia.json"),
                15,
                "5 full text, 10 abstract only",
            ),
            (
                include_str!("../tests/fixtures/temporal/full/achalasia.json"),
                15,
                "3 full text, 12 abstract only",
            ),
            (
                include_str!("../tests/fixtures/temporal/full/cholecystitis.json"),
                13,
                "1 full text, 12 abstract only",
            ),
        ];
        for (json, total, sub) in cases {
            let owned = fixture_studies(json);
            let studies: Vec<(&str, u32)> = owned.iter().map(|(t, n)| (t.as_str(), *n)).collect();
            let mut tr = read(RunMode::Focused, &steps(&FULL), RunState::Answered, None);
            finish(&mut tr, &studies);
            let r = row(&tr, "Studies read");
            assert_eq!(r.value, number(total));
            assert_eq!(r.bar, None);
            assert_eq!(r.sub.as_deref(), Some(sub));
            assert!(r.keep_sub);
            let sum: u32 = owned.iter().map(|(_, n)| n).sum();
            assert_eq!(row(&tr, "Outcomes verified").value, number(sum));
        }
    }

    #[test]
    fn stored_labels_are_followed_and_only_full_text_is_full_text() {
        let mut tr = read(RunMode::Focused, &steps(&FULL), RunState::Answered, None);
        finish(
            &mut tr,
            &[
                ("full text", 1),
                ("abstract only", 1),
                ("not extracted", 0),
                ("something new", 0),
                ("Full Text", 0),
            ],
        );
        assert_eq!(
            row(&tr, "Studies read").sub.as_deref(),
            Some("1 full text, 1 abstract only, 3 not extracted")
        );
        // A clause only when non-zero.
        let mut tr = read(RunMode::Focused, &steps(&FULL), RunState::Answered, None);
        finish(&mut tr, &[("full text", 1), ("full text", 1)]);
        assert_eq!(row(&tr, "Studies read").sub.as_deref(), Some("2 full text"));
    }

    #[test]
    fn a_late_line_after_a_wave_timeout_does_not_count() {
        let lines = [
            "Extractor and skeptic: extracting 12 studies and challenging the conclusion",
            "Extractor: 12/12 studies (42 verified outcomes, 0 not verified, 7 numbers not verified; all kept, labelled)",
            "Extractor: second wave, extracting 3 studies found by the skeptic",
            "Extractor: 2/3 studies (4 verified outcomes, 0 not verified, 3 numbers not verified; all kept, labelled)",
            "3 studies picked by the skeptic were not extracted (time); the writer was given their abstract",
            // The line writer outlives the cancellation.
            "Extractor: 3/3 studies (8 verified outcomes, 0 not verified, 3 numbers not verified; all kept, labelled)",
        ];
        let tr = read(RunMode::Focused, &steps(&lines), RunState::Working, None);
        assert_eq!(
            row(&tr, "Studies read").value,
            Value::Number {
                n: 12,
                of: Some(12)
            }
        );
        assert_eq!(row(&tr, "Outcomes verified").value, number(42));
        // Finished: the stored evidence has the final word on both rows.
        let mut studies = vec![("full text", 4); 10];
        studies.extend([("abstract only", 1), ("abstract only", 1)]);
        studies.extend([("not extracted", 0); 3]);
        let mut tr = read(RunMode::Focused, &steps(&lines), RunState::Answered, None);
        finish(&mut tr, &studies);
        assert_eq!(row(&tr, "Studies read").value, number(15));
        assert_eq!(
            row(&tr, "Studies read").sub.as_deref(),
            Some("10 full text, 2 abstract only, 3 not extracted")
        );
        assert_eq!(row(&tr, "Outcomes verified").value, number(42));
    }

    #[test]
    fn no_evidence_keeps_the_last_count_with_no_sub_line() {
        let mut tr = read(RunMode::Focused, &steps(&FULL), RunState::Answered, None);
        tr.read_from_evidence(&[]);
        let r = row(&tr, "Studies read");
        assert_eq!(
            r.value,
            Value::Number {
                n: 15,
                of: Some(15)
            }
        );
        assert_eq!(r.sub, None);
        assert!(!r.keep_sub);
    }

    #[test]
    fn a_failed_extraction_is_not_read() {
        let lines = [
            "Extractor and skeptic: extracting 12 studies and challenging the conclusion",
            "Extractor: 4/12 studies (17 verified outcomes, 0 not verified, 4 numbers not verified; all kept, labelled)",
            "Extraction failed for 1 study; the abstract is used instead",
        ];
        let tr = read(RunMode::Focused, &steps(&lines), RunState::Working, None);
        assert_eq!(
            row(&tr, "Studies read").value,
            Value::Number { n: 3, of: Some(12) }
        );
        assert_eq!(tr.role, "Extractor");
    }

    #[test]
    fn a_stopped_search_moves_on() {
        let lines = [
            "Researcher: 2 searches so far (turn 1 of 15)",
            "Search stopped at 465 s after 1 round",
        ];
        let tr = read(RunMode::Focused, &steps(&lines), RunState::Working, None);
        assert_eq!(tr.fill.at(t(0)), 35);
        assert_eq!(tr.role, "Researcher");
    }

    #[test]
    fn the_answer_fill_only_moves_forward_and_holds_short_of_full() {
        for (mode, lines) in [(RunMode::Quick, &QUICK[..]), (RunMode::Focused, &FULL[..])] {
            let mut last = 0;
            for n in 0..=lines.len() {
                let s = steps(&lines[..n]);
                // Well after the last line: the writer's stretch is at its end.
                let early = s.last().map_or(t(0), |x| x.at);
                let late = early + chrono::Duration::seconds(1000);
                let tr = read(mode, &s, RunState::Working, None);
                let (a, b) = (tr.fill.at(early), tr.fill.at(late));
                assert!(a >= last, "{mode:?} step {n}: {a} after {last}");
                assert!(b >= a, "{mode:?} step {n}");
                assert!(b <= 98, "{mode:?} step {n}: {b}");
                last = a;
            }
            assert!(last >= 90, "{mode:?}: {last}");
        }
    }

    #[test]
    fn the_writer_fills_by_time() {
        let s = steps(&QUICK[..7]);
        let tr = read(RunMode::Quick, &s, RunState::Working, None);
        let from = s.last().unwrap().at;
        assert_eq!(tr.fill.at(from), 45);
        let mid = tr.fill.at(from + chrono::Duration::seconds(33));
        assert!((60..75).contains(&mid), "{mid}");
        assert_eq!(tr.fill.at(from + chrono::Duration::seconds(600)), 90);
    }

    #[test]
    fn by_time_fills_hold_short_of_full() {
        assert_eq!(timed_fill(0, EXPERT_USUAL_SECS), 0);
        assert_eq!(timed_fill(88, EXPERT_USUAL_SECS), 50);
        assert_eq!(timed_fill(100_000, EXPERT_USUAL_SECS), 98);
        assert_eq!(timed_fill(-5, CHAT_USUAL_SECS), 0);
    }

    #[test]
    fn the_fill_fades_softly_into_white() {
        assert_eq!(
            fill_background(52),
            "background:linear-gradient(to right,#E6ECF8 38%,#FFFFFF 52%)"
        );
        assert_eq!(
            fill_background(5),
            "background:linear-gradient(to right,#E6ECF8 0%,#FFFFFF 5%)"
        );
    }

    #[test]
    fn the_clock_and_the_tagline() {
        assert_eq!(clock_text(0), "0:00");
        assert_eq!(clock_text(285), "4:45");
        assert_eq!(clock_text(-3), "0:00");
        assert!(!tagline_due(RunMode::Quick, 120));
        assert!(tagline_due(RunMode::Quick, 121));
        assert!(!tagline_due(RunMode::Focused, 300));
        assert!(tagline_due(RunMode::Focused, 301));
        assert_eq!(usual_label(RunMode::Quick), "usually about 2 min");
        assert_eq!(usual_label(RunMode::Focused), "usually 5 min");
    }

    #[test]
    fn pills_say_their_state_in_words_and_only_ready_is_ready() {
        assert_eq!(PillState::Ready.aria("Answer"), "Answer, ready");
        assert_eq!(
            PillState::Stopped.aria("Expert opinion \u{b7} unavailable"),
            "Expert opinion \u{b7} unavailable"
        );
        assert_eq!(
            PillState::Filling.aria("Expert opinion"),
            "Expert opinion, being prepared"
        );
        assert_eq!(PillState::Idle.as_str(), "idle");
        assert_eq!(PillState::Filling.as_str(), "filling");
    }

    // ---------- Stage S1: the first-words step lines ----------

    /// The line a streamed writer adds says nothing new: the card, the sentence
    /// and the fill read as they did, and the fill never steps back.
    #[test]
    fn the_writers_first_words_line_changes_nothing_but_the_rolling_line() {
        let t0 = chrono::DateTime::parse_from_rfc3339("2026-11-05T10:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let at = |s: i64| t0 + chrono::Duration::seconds(s);
        let line = "Writer: writing the answer from 15 studies (15 with an abstract)";
        let first = first_words_line(false, 26);
        assert_eq!(first, "Writer: first words after 26 s");
        let base = [
            Step {
                at: at(0),
                line: "Researcher: selected 15 studies (15 RCTs)",
            },
            Step { at: at(100), line },
        ];
        let with: Vec<Step<'_>> = base
            .iter()
            .copied()
            .chain([Step {
                at: at(126),
                line: &first,
            }])
            .collect();
        let a = read(RunMode::Focused, &base, RunState::Working, None);
        let b = read(RunMode::Focused, &with, RunState::Working, None);
        assert_eq!(b.rows, a.rows);
        assert_eq!(b.sentence, a.sentence);
        assert_eq!(b.fill, a.fill, "the fill goes on from the writer's start");
        assert_eq!(b.role, "Writer");
        assert_eq!(b.rolling, "First words after 26 s");
        let now = at(130);
        assert_eq!(b.fill.at(now), a.fill.at(now));
    }

    /// S1: the four-part writer's fallback line keeps the card on the writer,
    /// says it plainly and restarts the writer's timed fill from itself.
    #[test]
    fn the_one_piece_fallback_line_restarts_the_writers_fill() {
        let t0 = chrono::DateTime::parse_from_rfc3339("2026-11-08T10:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let at = |s: i64| t0 + chrono::Duration::seconds(s);
        let fallback = one_piece_line("outcomes");
        assert_eq!(
            fallback,
            "Writer: section outcomes failed; writing the answer in one piece"
        );
        let base = [
            Step {
                at: at(0),
                line: "Researcher: selected 5 studies (5 RCTs)",
            },
            Step {
                at: at(60),
                line: "Writer: writing the answer from 5 studies (5 with an abstract)",
            },
        ];
        let with: Vec<Step<'_>> = base
            .iter()
            .copied()
            .chain([Step {
                at: at(100),
                line: &fallback,
            }])
            .collect();
        let a = read(RunMode::Quick, &base, RunState::Working, None);
        let b = read(RunMode::Quick, &with, RunState::Working, None);
        assert_eq!(
            b.rows, a.rows,
            "the card still says the answer is being written"
        );
        assert_eq!(b.role, "Writer");
        assert_eq!(b.sentence, "Writing the answer in one piece");
        // The fill goes on from where it was (never back), then moves on.
        assert_eq!(b.fill.at(at(100)), a.fill.at(at(100)));
        assert!(b.fill.at(at(130)) > b.fill.at(at(100)));
        // Its first-words line afterwards changes nothing.
        let first = first_words_line(false, 20);
        let more: Vec<Step<'_>> = with
            .iter()
            .copied()
            .chain([Step {
                at: at(120),
                line: &first,
            }])
            .collect();
        let c = read(RunMode::Quick, &more, RunState::Working, None);
        assert_eq!(c.sentence, b.sentence);
        assert_eq!(c.fill, b.fill);
    }

    /// The Expert Opinion's line lands after the run has ended: the finished
    /// card reads the same.
    #[test]
    fn the_expert_first_words_line_leaves_the_finished_card_alone() {
        let t0 = chrono::DateTime::parse_from_rfc3339("2026-11-05T10:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let first = first_words_line(true, 41);
        assert_eq!(first, "Expert Opinion: first words after 41 s");
        assert!(is_expert_first_words(&first));
        assert!(!is_expert_first_words("Writer: first words after 41 s"));
        assert!(!is_expert_first_words("Expert Opinion: something else"));
        let base = [
            Step {
                at: t0,
                line: "Writer: writing the answer from 15 studies (15 with an abstract)",
            },
            Step {
                at: t0 + chrono::Duration::seconds(100),
                line: "Citation check: every citation is one of the selected studies",
            },
            Step {
                at: t0 + chrono::Duration::seconds(101),
                line: "Answer ready",
            },
        ];
        let with: Vec<Step<'_>> = base
            .iter()
            .copied()
            .chain([Step {
                at: t0 + chrono::Duration::seconds(141),
                line: &first,
            }])
            .collect();
        let a = read(RunMode::Focused, &base, RunState::Answered, None);
        let b = read(RunMode::Focused, &with, RunState::Answered, None);
        assert_eq!(b.rows, a.rows);
        assert_eq!(b.sentence, a.sentence);
        assert_eq!(b.fill.at(t0), a.fill.at(t0));
        assert!(b.ended_well);
    }
}
