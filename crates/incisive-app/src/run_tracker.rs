//! The run page's tracker rules (no Leptos, like [`crate::answer_page`]):
//! what the stored step lines say about a run, for the results card, the
//! fill pills and the folded Progress pill.
//!
//! The engine's step lines are read here as written; nothing is stored or
//! fetched for this. A line this file does not know goes to the rolling line
//! and the full list as it is, and never breaks the page. The tests below
//! pin the lines against the real runs, so a later engine change fails
//! loudly.

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
    /// Ended with an answer (done or partial).
    Answered,
    /// Ended with no answer (failed, interrupted, or nothing written).
    Stopped,
}

/// A card row's value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Value {
    Number { n: u32, of: Option<u32> },
    State(String),
    Pending,
    NotReached,
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
            | "Completeness review" => return ("Checker", line.to_string()),
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
                self.citations = Some(if body.starts_with("every citation") {
                    "all matched".into()
                } else {
                    body.to_string()
                });
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
                self.completeness = Some(format!("checking {n}"));
                self.raise(write_end + 6.0);
            }
            "Completeness" => {
                self.any = true;
                self.completeness = Some(match body.split_once(" found;") {
                    Some((gaps, _)) => format!("{gaps} reviewed"),
                    None => "reviewed".to_string(),
                });
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
        return Some(format!("all {n} traced"));
    }
    let untraced = lead_number(body)?;
    let rest = body.split_once(" of ")?.1;
    let total = lead_number(rest)?;
    Some(format!(
        "{} of {total} traced",
        total.saturating_sub(untraced)
    ))
}

/// Nothing more will come: a row with no value will never have one.
fn ended(state: RunState) -> bool {
    matches!(state, RunState::Answered | RunState::Stopped)
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

/// Read a run's stored steps.
pub fn read(mode: RunMode, steps: &[Step<'_>], state: RunState) -> Tracker {
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
            name: "Searches run",
            sub: Some("PubMed and Europe PMC".into()),
            value: number_value(seen.searches, None, state),
            bar: None,
        },
        Row {
            name: "Studies selected",
            sub: (!seen.selected_mix.is_empty()).then(|| seen.selected_mix.clone()),
            value: number_value(seen.selected, None, state),
            bar: None,
        },
    ];
    if full {
        rows.push(Row {
            name: "Studies read in full",
            sub: None,
            value: number_value(seen.read, seen.read_of, state),
            bar: match (seen.read, seen.read_of) {
                (Some(n), Some(of)) if of > 0 => Some((n.min(of) * 100 / of) as u8),
                _ => None,
            },
        });
        rows.push(Row {
            name: "Outcomes verified",
            sub: Some("Quotes matched against the paper".into()),
            value: number_value(seen.outcomes, None, state),
            bar: None,
        });
        rows.push(Row {
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
    // An answer the writer never finished was not reached.
    let answer = match (seen.answer, state) {
        (Some("being written"), RunState::Stopped) => None,
        (_, RunState::Answered) => Some("written"),
        (a, _) => a,
    };
    rows.push(Row {
        name: "Answer",
        sub: Some(
            if full {
                "Written from the studies read"
            } else {
                "Written from the studies selected"
            }
            .into(),
        ),
        value: state_value(answer.map(str::to_string), state),
        bar: None,
    });
    rows.push(Row {
        name: "Citations checked",
        sub: None,
        value: state_value(seen.citations.clone(), state),
        bar: None,
    });
    rows.push(Row {
        name: "Numbers traced",
        sub: None,
        value: state_value(seen.numbers.clone(), state),
        bar: None,
    });
    // With the review switched off, a finished run has no such row.
    if full && (seen.completeness.is_some() || state != RunState::Answered) {
        rows.push(Row {
            name: "Completeness review",
            sub: None,
            value: state_value(seen.completeness.clone(), state),
            bar: None,
        });
    }
    let sentence = match state {
        RunState::Waiting => "Waiting to start".to_string(),
        RunState::Answered => "Answer ready".to_string(),
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
        let tr = read(RunMode::Quick, &steps(&QUICK), RunState::Answered);
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
        let tr = read(RunMode::Focused, &steps(&FULL), RunState::Answered);
        assert_eq!(row(&tr, "Searches run").value, number(11));
        assert_eq!(row(&tr, "Studies selected").value, number(12));
        assert_eq!(
            row(&tr, "Studies read in full").value,
            Value::Number {
                n: 15,
                of: Some(15)
            }
        );
        assert_eq!(row(&tr, "Studies read in full").bar, Some(100));
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
        let tr = read(RunMode::Focused, &steps(&FULL[..21]), RunState::Working);
        assert_eq!(
            row(&tr, "Studies read in full").value,
            Value::Number { n: 4, of: Some(12) }
        );
        assert_eq!(row(&tr, "Studies read in full").bar, Some(33));
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
        let at = |n: usize| read(RunMode::Focused, &steps(&FULL[..n]), RunState::Working).sentence;
        assert_eq!(at(1), "Searching PubMed and Europe PMC");
        assert_eq!(at(12), "The researchers are discussing the best answer");
        assert_eq!(at(32), "Reading 3 more studies the skeptic found");
        assert_eq!(at(36), "Writing the answer from 15 studies");
        assert_eq!(at(37), "Checking citations and numbers");
        assert_eq!(at(40), "Checking completeness");
    }

    #[test]
    fn roles_follow_the_lines() {
        let at = |n: usize| read(RunMode::Focused, &steps(&FULL[..n]), RunState::Working).role;
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
        let tr = read(RunMode::Quick, &steps(&lines), RunState::Working);
        assert!(tr.recognised);
        assert_eq!(tr.rolling, "No colon at all");
        assert_eq!(tr.role, "Researcher");
    }

    #[test]
    fn no_recognised_line_draws_no_card() {
        let lines = ["Searching", "Screening 12 abstracts"];
        let tr = read(RunMode::Quick, &steps(&lines), RunState::Answered);
        assert!(!tr.recognised);
    }

    #[test]
    fn nothing_yet_is_waiting() {
        let tr = read(RunMode::Focused, &[], RunState::Waiting);
        assert_eq!(tr.sentence, "Waiting to start");
        assert_eq!(tr.role, "Starting");
        assert_eq!(tr.rolling, "Nothing yet. This page updates by itself.");
        assert_eq!(tr.fill.at(t(0)), 0);
        assert!(tr.rows.iter().all(|r| r.value == Value::Pending));
    }

    #[test]
    fn a_stopped_run_says_what_was_not_reached() {
        let lines = &FULL[..36];
        let tr = read(RunMode::Focused, &steps(lines), RunState::Stopped);
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
        let tr = read(RunMode::Focused, &steps(&lines), RunState::Answered);
        assert!(tr.rows.iter().all(|r| r.name != "Completeness review"));
    }

    #[test]
    fn a_finished_run_never_says_pending() {
        let tr = read(RunMode::Focused, &steps(&FULL[..5]), RunState::Answered);
        assert_eq!(row(&tr, "Answer").value, state("written"));
        assert_eq!(row(&tr, "Citations checked").value, Value::NotReached);
        assert!(tr.rows.iter().all(|r| r.value != Value::Pending));
    }

    #[test]
    fn a_stopped_search_moves_on() {
        let lines = [
            "Researcher: 2 searches so far (turn 1 of 15)",
            "Search stopped at 465 s after 1 round",
        ];
        let tr = read(RunMode::Focused, &steps(&lines), RunState::Working);
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
                let tr = read(mode, &s, RunState::Working);
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
        let tr = read(RunMode::Quick, &s, RunState::Working);
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
}
