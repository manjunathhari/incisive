//! Temporal T4: the Expert Opinion screens' rules (no Leptos).
//!
//! The run page's "Answer | Expert opinion" option ([`OptionState`]), and
//! everything the Temporal board page (`/run/:id/temporal`) draws from the
//! stored board and table: the timeline ([`gantt`]), the era figures
//! ([`era_figures`]), the linked progression chart ([`linked_outcomes`],
//! [`linked_panels`], [`Axis`]), the ledger's column labels
//! ([`ledger_headers`]), the predictive range line, the checks
//! ([`Flags::kinds`]) and the citation links ([`cite_pieces`]).
//!
//! The board is read into [`Board`], a read-only mirror of the writer's
//! `ExpertBoardV1` (the writer's module is server only and is not touched;
//! a test keeps the two in step). Nothing here changes stored text: the
//! board is drawn as the writer gave it and the checks only list flags.
//!
//! Charts are drawn by the views from the coordinates computed here, as
//! SVG elements built from data (no `inner_html`).
//!
//! **No cost.** Owner decision 3 keeps cost and country context out of this
//! output: outcomes that measure cost (by name or by a currency unit) are
//! never drawn from the table ([`is_cost_outcome`]). Board text is shown
//! unchanged; the checks list any cost or country words in it.

use std::collections::BTreeSet;
use std::sync::LazyLock;

use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::runs::TemporalView;
use crate::temporal::{Era, EraCell, Family, GroupResult, OutcomeResult, StudyKind, TemporalTable};

// ---------- the board (a read-only mirror of the writer's ExpertBoardV1) ----------

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Glance {
    pub original_quote: String,
    pub expert: String,
    pub today: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct EraNote {
    pub era_id: String,
    pub going_in: String,
    pub showed: String,
    pub knowledge_after: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Round {
    pub round: i64,
    pub speaker: String,
    pub title: String,
    pub points: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Score {
    pub code: String,
    pub name: String,
    #[serde(rename = "for")]
    pub for_: String,
    pub against: String,
    pub status: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LedgerRow {
    pub question: String,
    pub cells: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Ledger {
    pub label_mode: String,
    pub rows: Vec<LedgerRow>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Opinion {
    pub verdict: String,
    pub why: String,
    pub curve_reading: String,
    pub did_not_improve: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Decision {
    pub scenario: String,
    pub choice: String,
    pub reason: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Trigger {
    #[serde(rename = "if")]
    pub if_: String,
    pub then: String,
}

/// The stored board, as the writer gave it (`ExpertBoardV1`'s fields).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Board {
    pub topic: String,
    pub at_a_glance: Glance,
    pub eras: Vec<EraNote>,
    pub linked_note: String,
    pub debate: Vec<Round>,
    pub scoreboard: Vec<Score>,
    pub ledger: Ledger,
    pub opinion: Opinion,
    pub decisions: Vec<Decision>,
    pub triggers: Vec<Trigger>,
    pub settle: Vec<String>,
}

impl Board {
    /// Every text of the board (citations are read from these).
    pub fn texts(&self) -> Vec<&str> {
        let mut v: Vec<&str> = vec![&self.topic];
        let g = &self.at_a_glance;
        v.extend([g.original_quote.as_str(), &g.expert, &g.today]);
        for e in &self.eras {
            v.extend([e.going_in.as_str(), &e.showed, &e.knowledge_after]);
        }
        v.push(&self.linked_note);
        for d in &self.debate {
            v.push(&d.title);
            v.extend(d.points.iter().map(String::as_str));
        }
        for s in &self.scoreboard {
            v.extend([s.name.as_str(), &s.for_, &s.against]);
        }
        for r in &self.ledger.rows {
            v.push(&r.question);
            v.extend(r.cells.iter().map(String::as_str));
        }
        let o = &self.opinion;
        v.extend([
            o.verdict.as_str(),
            &o.why,
            &o.curve_reading,
            &o.did_not_improve,
        ]);
        for d in &self.decisions {
            v.extend([d.scenario.as_str(), &d.choice, &d.reason]);
        }
        for t in &self.triggers {
            v.extend([t.if_.as_str(), &t.then]);
        }
        v.extend(self.settle.iter().map(String::as_str));
        v
    }

    /// The board's note for era `era_id`.
    pub fn era_note(&self, era_id: &str) -> Option<&EraNote> {
        self.eras.iter().find(|e| e.era_id.trim() == era_id)
    }

    /// Era notes whose id is not an era of the table (shown after the
    /// table's eras: flag, never delete).
    pub fn orphan_era_notes<'a>(&'a self, t: &TemporalTable) -> Vec<&'a EraNote> {
        self.eras
            .iter()
            .filter(|n| !t.eras.iter().any(|e| e.id == n.era_id.trim()))
            .collect()
    }
}

// ---------- the checks (a read-only mirror of the writer's BoardFlags) ----------

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Flag {
    pub field: String,
    pub text: String,
}

/// The code checks as stored. Nothing on the board is removed for them.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Flags {
    pub not_traced: Vec<Flag>,
    pub derived: Vec<Flag>,
    pub cost_country: Vec<Flag>,
    pub unknown_references: Vec<Flag>,
    pub unknown_ids: Vec<Flag>,
    pub meta_called_trial: Vec<Flag>,
    pub glance_stronger: Vec<Flag>,
    pub shape: Vec<Flag>,
}

/// One kind of check, as the Checks fold lists it.
pub struct FlagKind<'a> {
    pub label: &'static str,
    pub note: &'static str,
    pub flags: &'a [Flag],
}

impl Flags {
    /// Every kind, in the order the Checks fold lists them.
    pub fn kinds(&self) -> [FlagKind<'_>; 8] {
        [
            FlagKind {
                label: "Not traced",
                note: "Numbers not found in the answer's text or the table.",
                flags: &self.not_traced,
            },
            FlagKind {
                label: "Derived",
                note: "Numbers the writer worked out and marked \u{201c}(derived)\u{201d}: listed for a reader to check, not counted.",
                flags: &self.derived,
            },
            FlagKind {
                label: "At a glance stronger than the opinion",
                note: "Strong words in an At a glance line while the opinion hedges.",
                flags: &self.glance_stronger,
            },
            FlagKind {
                label: "Cost or country",
                note: "Cost or country words (this output carries neither).",
                flags: &self.cost_country,
            },
            FlagKind {
                label: "Unknown table ids",
                note: "Ids the table does not have.",
                flags: &self.unknown_ids,
            },
            FlagKind {
                label: "Unknown references",
                note: "Citations of numbers not in the run's reference list.",
                flags: &self.unknown_references,
            },
            FlagKind {
                label: "A meta-analysis called a trial",
                note: "A meta-analysis or review cited as a trial.",
                flags: &self.meta_called_trial,
            },
            FlagKind {
                label: "Shape",
                note: "Where the board departs from the brief's shape (rounds, speakers, label mode, eras).",
                flags: &self.shape,
            },
        ]
    }

    /// Flags counted as problems (every kind but "derived", as the job
    /// counts them).
    pub fn total(&self) -> usize {
        self.kinds()
            .iter()
            .filter(|k| k.label != "Derived")
            .map(|k| k.flags.len())
            .sum()
    }
}

/// The stored opinion read for drawing: the board, the table and the
/// flags. `None` when the board or the table does not read.
#[derive(Debug, Clone, PartialEq)]
pub struct Opened {
    pub board: Board,
    pub table: TemporalTable,
    pub flags: Option<Flags>,
}

/// Read a `done` row's board, table and flags.
pub fn open(v: &TemporalView) -> Option<Opened> {
    let board: Board = serde_json::from_value(v.board.clone()?).ok()?;
    let table: TemporalTable = serde_json::from_value(v.table.clone()?).ok()?;
    let flags = v
        .flags
        .clone()
        .and_then(|f| serde_json::from_value::<Flags>(f).ok());
    Some(Opened {
        board,
        table,
        flags,
    })
}

// ---------- the run page's option ----------

/// The "Expert opinion" option's state, from the row's status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OptionState {
    /// `pending`: being written; the page polls.
    Pending,
    /// `done` with a board and table that read: selectable.
    Ready,
    /// `skipped`: fewer than two primary studies (or no answer).
    Skipped,
    /// `failed`, an unknown status, or a `done` row that does not read.
    Failed,
    /// `cap_blocked`: the daily limit stopped it.
    CapBlocked,
}

/// How often the run page asks again while the opinion is being written.
pub const POLL_EVERY_SECS: u64 = 15;
/// At most this many polls (10 minutes, the job's stale limit).
pub const POLL_MAX: u32 = 40;

/// The note on the option once the page has stopped asking (the opinion is
/// still being written after [`POLL_MAX`] polls).
pub const POLL_GAVE_UP_NOTE: &str = "Taking longer than usual \u{2014} reload later";

/// What the run page does at a poll tick.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Poll {
    /// Ask again.
    Continue,
    /// Stop: the opinion is no longer being written.
    Stop,
    /// Stop: still being written after [`POLL_MAX`] polls (10 minutes);
    /// the option says to reload later.
    GiveUp,
}

/// The poll decision after `ticks` polls with the option in `state`: ask
/// again only while the opinion is being written and fewer than
/// [`POLL_MAX`] polls (every [`POLL_EVERY_SECS`] s) have been made.
pub fn next_poll(ticks: u32, state: OptionState) -> Poll {
    if !state.polls() {
        Poll::Stop
    } else if ticks >= POLL_MAX {
        Poll::GiveUp
    } else {
        Poll::Continue
    }
}

impl OptionState {
    /// The state for a stored status.
    pub fn from_status(status: &str) -> OptionState {
        match status {
            "pending" => OptionState::Pending,
            "done" => OptionState::Ready,
            "skipped" => OptionState::Skipped,
            "cap_blocked" => OptionState::CapBlocked,
            _ => OptionState::Failed,
        }
    }

    /// The state for a row: a `done` row whose board or table does not read
    /// is unavailable.
    pub fn of(v: &TemporalView) -> OptionState {
        match OptionState::from_status(&v.status) {
            OptionState::Ready if open(v).is_none() => OptionState::Failed,
            s => s,
        }
    }

    /// The option's text.
    pub fn text(self) -> &'static str {
        match self {
            OptionState::Pending => "Expert opinion",
            OptionState::Ready => "Expert opinion",
            OptionState::Skipped => "Expert opinion \u{b7} not enough studies",
            OptionState::Failed => "Expert opinion \u{b7} unavailable",
            OptionState::CapBlocked => "Expert opinion \u{b7} not run (daily limit)",
        }
    }

    /// Only a ready opinion can be opened.
    pub fn selectable(self) -> bool {
        self == OptionState::Ready
    }

    /// The ready dot.
    pub fn dot(self) -> bool {
        self == OptionState::Ready
    }

    /// The page polls while the opinion is being written.
    pub fn polls(self) -> bool {
        self == OptionState::Pending
    }
}

// ---------- outcomes and eras ----------

static COST_WORDS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)\b(?:costs?|cost-\w+|price[sd]?|pricing|charges?|expenditure|economic\w*|reimburs\w*|budget\w*|financial|spending)\b",
    )
    .expect("cost words")
});

const CURRENCIES: [&str; 14] = [
    "usd", "eur", "gbp", "inr", "aud", "cad", "jpy", "cny", "chf", "$", "\u{a3}", "\u{20ac}",
    "\u{20b9}", "rs",
];

/// An outcome that measures cost, by its name or its lead group's unit:
/// never drawn from the table (owner decision 3).
pub fn is_cost_outcome(o: &OutcomeResult) -> bool {
    if COST_WORDS.is_match(&o.outcome) {
        return true;
    }
    o.groups.iter().any(|g| {
        let u = g.unit_key.trim().to_ascii_lowercase();
        CURRENCIES
            .iter()
            .any(|c| u == *c || u.contains(c) && c.len() > 2)
    })
}

/// The outcome's lead group (the one its label is read from).
pub fn lead_group(o: &OutcomeResult) -> Option<&GroupResult> {
    let id = o.lead_group_id.as_deref()?;
    o.groups.iter().find(|g| g.id == id)
}

/// The cell holds data: a primary row with an estimate, or points on both
/// sides of the null (`split`).
pub fn has_data(c: &EraCell) -> bool {
    !c.empty && (c.estimate.is_some() || c.split)
}

/// The outcomes drawn: a lead group, not cost, in table order.
pub fn drawn_outcomes(t: &TemporalTable) -> Vec<(&OutcomeResult, &GroupResult)> {
    t.outcomes
        .iter()
        .filter(|o| !is_cost_outcome(o))
        .filter_map(|o| lead_group(o).map(|g| (o, g)))
        .collect()
}

/// At most this many outcomes on the linked chart.
pub const LINKED_MAX: usize = 3;

/// The linked chart's outcomes: up to [`LINKED_MAX`] drawn outcomes whose
/// lead group has data in two or more eras, in table order.
pub fn linked_outcomes(t: &TemporalTable) -> Vec<(&OutcomeResult, &GroupResult)> {
    drawn_outcomes(t)
        .into_iter()
        .filter(|(_, g)| g.cells.iter().filter(|c| has_data(c)).count() >= 2)
        .take(LINKED_MAX)
        .collect()
}

/// The colour slot of era `i` of `n`, 1 to 3 (the frozen board's three era
/// tints): the latest era 3, then 2 and 1 alternating backwards, so
/// neighbouring eras never share a tint.
pub fn era_class(i: usize, n: usize) -> u8 {
    match n.saturating_sub(1).saturating_sub(i) {
        0 => 3,
        b if b % 2 == 1 => 2,
        _ => 1,
    }
}

/// The chip of an era card.
pub fn era_chip(i: usize, e: &Era, n: usize) -> String {
    if i + 1 == n {
        "Latest five years".into()
    } else if e.empty || e.n_studies == 0 {
        "No studies".into()
    } else if e.n_studies == 1 {
        "1 study".into()
    } else {
        format!("{} studies", e.n_studies)
    }
}

/// Whether the ledger's columns are the years results became known.
pub fn known_by(mode: &str) -> bool {
    mode.trim().eq_ignore_ascii_case("known_by")
}

/// The ledger's column labels after "Question": `n` columns, oldest first,
/// the last being now. They map to the table's last `n` eras (extra leading
/// columns, beyond the table's eras, are "Earlier"). Treatment eras read
/// "After 2016–20" … "Now, after 2021–26"; "known by" reads "Known by 2020"
/// … "Known now, 2026". Any other mode reads as treatment eras (the checks
/// flag it).
pub fn ledger_headers(mode: &str, eras: &[Era], n: usize) -> Vec<String> {
    let kb = known_by(mode);
    let extra = n.saturating_sub(eras.len());
    let used = &eras[eras.len().saturating_sub(n)..];
    let mut v: Vec<String> = (0..extra).map(|_| "Earlier".to_string()).collect();
    for (i, e) in used.iter().enumerate() {
        let last = i + 1 == used.len();
        v.push(match (kb, last) {
            (false, false) => format!("After {}", e.label),
            (false, true) => format!("Now, after {}", e.label),
            (true, false) => format!("Known by {}", e.end),
            (true, true) => format!("Known now, {}", e.end),
        });
    }
    if used.is_empty()
        && let Some(l) = v.last_mut()
    {
        *l = "Now".into();
    }
    v
}

/// The line under the ledger saying what its columns are.
pub fn ledger_note(mode: &str) -> &'static str {
    if known_by(mode) {
        "Columns are the years results became known."
    } else {
        "Columns are the eras patients were treated in."
    }
}

// ---------- numbers ----------

/// A unit as written after a number (" min", " pp"); empty for none.
pub fn unit_suffix(unit_key: &str) -> String {
    let u = unit_key.trim();
    if u.is_empty() {
        String::new()
    } else {
        format!(" {u}")
    }
}

/// A plain number: two decimals under 1 in size, one under 100, none above.
pub fn fmt_plain(v: f64) -> String {
    let a = v.abs();
    let s = if a >= 100.0 {
        format!("{a:.0}")
    } else if a >= 1.0 {
        format!("{a:.1}")
    } else {
        format!("{a:.2}")
    };
    if v < 0.0 && s.chars().any(|c| c.is_ascii_digit() && c != '0') {
        format!("\u{2212}{s}")
    } else {
        s
    }
}

/// A difference with its sign and unit ("+35.0 min", "−18.5 pp").
pub fn fmt_diff(v: f64, unit_key: &str) -> String {
    let p = fmt_plain(v);
    let sign = if v > 0.0 && p.chars().any(|c| c.is_ascii_digit() && c != '0') {
        "+"
    } else {
        ""
    };
    format!("{sign}{p}{}", unit_suffix(unit_key))
}

/// A ratio to two decimals.
pub fn fmt_ratio(v: f64) -> String {
    format!("{v:.2}")
}

/// A value in its family.
pub fn fmt_value(family: Family, v: f64, unit_key: &str) -> String {
    match family {
        Family::Ratio => fmt_ratio(v),
        Family::Difference => fmt_diff(v, unit_key),
    }
}

/// A range in its family ("1.28–2.03", "−0.34 to 0.90 points").
pub fn fmt_range(family: Family, lo: f64, hi: f64, unit_key: &str) -> String {
    match family {
        Family::Ratio => format!("{}\u{2013}{}", fmt_ratio(lo), fmt_ratio(hi)),
        Family::Difference => format!(
            "{} to {}{}",
            fmt_plain(lo),
            fmt_plain(hi),
            unit_suffix(unit_key)
        ),
    }
}

/// One figure of an era card: the outcome, its value and what it rests on.
#[derive(Debug, Clone, PartialEq)]
pub struct Figure {
    pub label: String,
    pub value: String,
    pub detail: String,
}

/// The era's figures: each drawn outcome's lead-group cell with data.
pub fn era_figures(t: &TemporalTable, era_id: &str) -> Vec<Figure> {
    let mut v = Vec::new();
    for (o, g) in drawn_outcomes(t) {
        let Some(c) = g.cells.iter().find(|c| c.era_id == era_id) else {
            continue;
        };
        if !has_data(c) {
            continue;
        }
        let value = match c.estimate {
            Some(e) => fmt_value(g.family, e, &g.unit_key),
            None => "split".into(),
        };
        let mut detail = match (&c.pooled, c.split) {
            (Some(p), _) => format!("95% CI {}", fmt_range(g.family, p.lo, p.hi, &g.unit_key)),
            (None, true) => "points on both sides of no difference".into(),
            (None, false) => "points only, no CI".into(),
        };
        if let Some(z) = c.z {
            detail.push_str(&format!(" \u{b7} Z {}", fmt_z(z)));
        }
        if !c.durability.is_empty() {
            let n = c.durability.len();
            detail.push_str(&format!(
                " \u{b7} {n} follow-up {}",
                if n == 1 { "point" } else { "points" }
            ));
        }
        v.push(Figure {
            label: o.outcome.clone(),
            value,
            detail,
        });
    }
    v
}

/// Z to one decimal with a true minus sign.
pub fn fmt_z(z: f64) -> String {
    let s = format!("{:.1}", z.abs());
    if z < 0.0 && s != "0.0" {
        format!("\u{2212}{s}")
    } else {
        s
    }
}

/// The table's predictive range for the next study, per drawn outcome
/// that has one: "Predicted range for the next independent study: …".
pub fn predictive_line(t: &TemporalTable) -> Option<String> {
    let parts: Vec<String> = drawn_outcomes(t)
        .into_iter()
        .filter_map(|(o, g)| {
            let p = g.predictive.as_ref()?;
            Some(format!(
                "{} {}",
                o.outcome,
                fmt_range(g.family, p.lo, p.hi, &g.unit_key)
            ))
        })
        .collect();
    (!parts.is_empty()).then(|| {
        format!(
            "Predicted range for the next independent study (code, from the table): {}.",
            parts.join("; ")
        )
    })
}

/// The board page's source line.
pub fn source_line(t: &TemporalTable) -> String {
    let primary = t
        .studies
        .iter()
        .filter(|s| s.kind == StudyKind::Primary && s.parent_id.is_none())
        .count();
    let follow = t
        .studies
        .iter()
        .filter(|s| s.kind == StudyKind::Primary && s.parent_id.is_some())
        .count();
    let markers = t.markers.len();
    let guidelines = t.guidelines.len();
    let plural =
        |n: usize, one: &str, many: &str| format!("{n} {}", if n == 1 { one } else { many });
    let mut parts = vec![plural(primary, "primary study", "primary studies")];
    if follow > 0 {
        parts.push(plural(follow, "follow-up report", "follow-up reports"));
    }
    parts.push(plural(
        markers,
        "meta-analysis or review",
        "meta-analyses and reviews",
    ));
    parts.push(plural(guidelines, "guideline", "guidelines"));
    let year = t
        .run_year
        .map(|y| format!(", evidence to {y}"))
        .unwrap_or_default();
    let arms = t
        .outcomes
        .iter()
        .filter_map(lead_group)
        .find(|g| g.intervention != "?" && g.comparator != "?")
        .map(|g| format!(" Effects are {} vs {}.", g.intervention, g.comparator))
        .unwrap_or_default();
    format!(
        "Source: the stored Full run ({}){year}. Eras follow the years patients were treated. Numbers are the run's own.{arms}",
        parts.join(", ")
    )
}

// ---------- citations ----------

/// `[3]`, `[1, 4]`, `[2–5]` (as the writer's checks read them).
static CITE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\[\s*\d{1,4}(?:\s*[,;\u{2013}-]\s*\d{1,4})*\s*\]").expect("cite pattern")
});
static DIGITS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\d{1,4}").expect("digits"));

/// A piece of board text: plain text, or a reference number that links.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Piece {
    Text(String),
    Ref(u16),
}

/// `text` cut into plain text and the reference numbers in its citations
/// that are in `known`. Every character is kept, in order: only the digits
/// of a known number become a link.
pub fn cite_pieces(text: &str, known: &BTreeSet<u16>) -> Vec<Piece> {
    let mut out: Vec<Piece> = Vec::new();
    let mut push = |p: Piece| match (out.last_mut(), p) {
        (Some(Piece::Text(a)), Piece::Text(b)) => a.push_str(&b),
        (_, p) => out.push(p),
    };
    let mut at = 0;
    for m in CITE.find_iter(text) {
        push(Piece::Text(text[at..m.start()].to_string()));
        let c = m.as_str();
        let mut k = 0;
        for d in DIGITS.find_iter(c) {
            push(Piece::Text(c[k..d.start()].to_string()));
            match d.as_str().parse::<u16>() {
                Ok(n) if known.contains(&n) => push(Piece::Ref(n)),
                _ => push(Piece::Text(d.as_str().to_string())),
            }
            k = d.end();
        }
        push(Piece::Text(c[k..].to_string()));
        at = m.end();
    }
    push(Piece::Text(text[at..].to_string()));
    out.retain(|p| !matches!(p, Piece::Text(t) if t.is_empty()));
    out
}

/// The reference numbers the board cites that are in the run's reference
/// list: every cited number but those the checks flagged as unknown. No
/// flags stored: none (the citations stay plain text).
pub fn known_refs(board: &Board, flags: Option<&Flags>) -> BTreeSet<u16> {
    let Some(flags) = flags else {
        return BTreeSet::new();
    };
    let unknown: BTreeSet<u16> = flags
        .unknown_references
        .iter()
        .filter_map(|f| f.text.trim_matches(['[', ']', ' ']).parse().ok())
        .collect();
    board
        .texts()
        .into_iter()
        .flat_map(|t| CITE.find_iter(t).collect::<Vec<_>>())
        .flat_map(|m| {
            DIGITS
                .find_iter(m.as_str())
                .filter_map(|d| d.as_str().parse::<u16>().ok())
                .collect::<Vec<_>>()
        })
        .filter(|n| !unknown.contains(n))
        .collect()
}

// ---------- the debate and the scoreboard ----------

/// Which side a debate round is drawn on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    /// Left.
    Researcher,
    /// Right.
    Skeptic,
    /// Left, labelled as stored.
    Other,
}

pub fn side(speaker: &str) -> Side {
    let s = speaker.trim().to_ascii_lowercase();
    if s.contains("skeptic") || s.contains("sceptic") {
        Side::Skeptic
    } else if s.contains("researcher") {
        Side::Researcher
    } else {
        Side::Other
    }
}

/// The speaker's label.
pub fn speaker_label(speaker: &str) -> String {
    match side(speaker) {
        Side::Researcher => "Latest-period researcher".into(),
        Side::Skeptic => "Skeptic".into(),
        Side::Other => speaker.trim().to_string(),
    }
}

/// The class of a scoreboard status chip: leading (accent), unresolved
/// (warn), anything else grey.
pub fn status_class(status: &str) -> &'static str {
    let s = status.trim().to_ascii_lowercase();
    if s.starts_with("leading") {
        "rt-chip"
    } else if s.starts_with("unresolved") {
        "rt-chip warn"
    } else {
        "rt-chip grey"
    }
}

// ---------- the round table's chat ----------

/// The bold opening of a point: up to its first colon, when that is near the
/// start and holds no sentence end or citation ("What held:").
static LEAD: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?s)^([^:.\[\]]{3,48}:)(\s.*)$").expect("lead pattern"));
/// An explanation's code in the text: H1 … H9.
static HYPOTHESIS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\bH[1-9]\b").expect("hypothesis pattern"));

/// A run of a chat message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChatSpan {
    Text(String),
    /// The bold opening.
    Lead(String),
    /// An explanation's code (H1), marked in the text.
    Code(String),
    Ref(u16),
}

/// "H2 blockers", "H1 receptor": the drug, not an explanation's code.
fn is_drug_word(after: &str) -> bool {
    let a = after.trim_start_matches([' ', '-']).to_ascii_lowercase();
    ["block", "receptor", "antagonist", "antihistamine"]
        .iter()
        .any(|w| a.starts_with(w))
}

fn split_codes(text: &str, bold: bool, out: &mut Vec<ChatSpan>) {
    let plain = |t: &str| {
        if bold {
            ChatSpan::Lead(t.to_string())
        } else {
            ChatSpan::Text(t.to_string())
        }
    };
    let mut at = 0;
    for m in HYPOTHESIS.find_iter(text) {
        if is_drug_word(&text[m.end()..]) {
            continue;
        }
        if m.start() > at {
            out.push(plain(&text[at..m.start()]));
        }
        out.push(ChatSpan::Code(m.as_str().to_string()));
        at = m.end();
    }
    if at < text.len() {
        out.push(plain(&text[at..]));
    }
}

/// A debate point cut into runs for its chat message: the bold opening, the
/// explanation codes and the reference links. Every character is kept, in
/// order, as [`cite_pieces`] keeps them.
pub fn chat_spans(text: &str, known: &BTreeSet<u16>) -> Vec<ChatSpan> {
    let mut out = Vec::new();
    let rest = match LEAD.captures(text) {
        Some(c) => {
            split_codes(c.get(1).map_or("", |m| m.as_str()), true, &mut out);
            c.get(2).map_or("", |m| m.as_str())
        }
        None => text,
    };
    for p in cite_pieces(rest, known) {
        match p {
            Piece::Text(t) => split_codes(&t, false, &mut out),
            Piece::Ref(n) => out.push(ChatSpan::Ref(n)),
        }
    }
    out
}

/// A point about the patient's question gets its own band in the chat.
pub fn is_patient_point(text: &str) -> bool {
    text.trim_start()
        .get(..16)
        .is_some_and(|s| s.eq_ignore_ascii_case("patient question"))
}

/// The tab's title for a round: its stored title.
pub fn round_tab(r: &Round) -> (String, String) {
    (format!("Round {}", r.round), r.title.trim().to_string())
}

// ---------- the era cards' "More" ----------

/// A box this much longer than the usual length (px) is not worth cutting.
pub const MORE_SLACK: i32 = 24;

/// From the "What the era showed" boxes' full heights: the usual length (the
/// middle one) and which boxes run past it and get "More".
pub fn more_cut(heights: &[i32]) -> (i32, Vec<bool>) {
    let mut sorted = heights.to_vec();
    sorted.sort_unstable();
    let usual = sorted.get(sorted.len() / 2).copied().unwrap_or(0);
    (
        usual,
        heights.iter().map(|h| *h > usual + MORE_SLACK).collect(),
    )
}

// ---------- SVG geometry ----------

/// Rounded to 0.1 for the SVG attributes (`0` for a value that is not a
/// finite number, so no `NaN` or `inf` reaches the markup).
pub fn px(v: f64) -> String {
    if !v.is_finite() {
        return "0".into();
    }
    let r = (v * 10.0).round() / 10.0;
    if r == 0.0 {
        "0".into()
    } else if r == r.trunc() {
        format!("{r:.0}")
    } else {
        format!("{r:.1}")
    }
}

/// At most this many year ticks on the timeline.
pub const MAX_YEAR_TICKS: i32 = 12;

/// The timeline's year step for a span of `span` years: the smallest of
/// 1, 2, 5, 10, 20, 25, 50, 100, 200, 250, 500, 1000 that gives at most [`MAX_YEAR_TICKS`]
/// ticks.
pub fn year_step(span: i32) -> i32 {
    let span = span.max(1);
    [1, 2, 5, 10, 20, 25, 50, 100, 200, 250, 500, 1000]
        .into_iter()
        .find(|step| (span + step - 1) / step <= MAX_YEAR_TICKS)
        .unwrap_or((span + MAX_YEAR_TICKS - 1) / MAX_YEAR_TICKS)
}

/// `s` cut to `max` characters with an ellipsis.
pub fn clip(s: &str, max: usize) -> String {
    let s = s.trim();
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut t: String = s.chars().take(max.saturating_sub(1)).collect();
        t = t.trim_end().to_string();
        t.push('\u{2026}');
        t
    }
}

/// The timeline's width (the SVG's viewBox).
pub const GANTT_W: f64 = 900.0;
/// The label column.
pub const GANTT_L: f64 = 210.0;
pub const GANTT_R: f64 = 20.0;
/// The top of the first row band.
pub const GANTT_T: f64 = 34.0;
pub const GANTT_ROW: f64 = 28.0;
/// Row labels are cut to this many characters (the full label is the
/// row's `<title>`).
pub const GANTT_LABEL_MAX: usize = 30;

/// An era band behind the bars.
#[derive(Debug, Clone, PartialEq)]
pub struct Band {
    pub x: f64,
    pub w: f64,
    pub label: String,
    pub class: u8,
}

/// A follow-up's knowledge point on its parent's row.
#[derive(Debug, Clone, PartialEq)]
pub struct KnowPoint {
    pub x: f64,
    pub label: String,
}

/// One study row.
#[derive(Debug, Clone, PartialEq)]
pub struct GanttRow {
    pub label: String,
    pub title: String,
    /// The row's centre line.
    pub y: f64,
    /// The bar's ends (`None`: no treatment years).
    pub bar: Option<(f64, f64)>,
    /// Assumed years: a dashed, hollow bar.
    pub dashed: bool,
    pub note: String,
    pub points: Vec<KnowPoint>,
}

/// A guideline or meta-analysis marker line.
#[derive(Debug, Clone, PartialEq)]
pub struct MarkerLine {
    pub x: f64,
    pub label: String,
    /// Its label's baseline, in a strip under the year axis (one row per
    /// line, so labels overlap neither each other nor the bars).
    pub label_y: f64,
    /// The label sits to the line's left.
    pub anchor_end: bool,
    pub guideline: bool,
}

/// The timeline, ready to draw.
#[derive(Debug, Clone, PartialEq)]
pub struct Gantt {
    pub width: f64,
    pub height: f64,
    /// The first year and the year after the last.
    pub from: i32,
    pub to: i32,
    pub band_top: f64,
    pub band_h: f64,
    pub bands: Vec<Band>,
    pub rows: Vec<GanttRow>,
    /// Year ticks: x and the year.
    pub ticks: Vec<(f64, i32)>,
    pub axis_y: f64,
    pub markers: Vec<MarkerLine>,
}

impl Gantt {
    /// x of the start of `year` (`year + 1` is its end).
    pub fn x(&self, year: f64) -> f64 {
        x_of(year, self.from, self.to)
    }
}

fn x_of(year: f64, from: i32, to: i32) -> f64 {
    let span = f64::from((to - from).max(1));
    GANTT_L + (year - f64::from(from)) / span * (GANTT_W - GANTT_L - GANTT_R)
}

/// The timeline of the table's studies: one row per independent primary
/// study (sorted by treatment years), its bar over the treatment years
/// (dashed when assumed); each follow-up report's knowledge year as a point
/// on its parent's row; the era bands behind; a line per guideline year and
/// per meta-analysis year. `None` when no study has a year.
pub fn gantt(t: &TemporalTable) -> Option<Gantt> {
    let primaries: Vec<_> = t
        .studies
        .iter()
        .filter(|s| s.kind == StudyKind::Primary && s.parent_id.is_none())
        .collect();
    let mut years: Vec<i32> = Vec::new();
    for s in &t.studies {
        years.extend(s.treatment_start);
        years.extend(s.treatment_end);
        if s.parent_id.is_some() {
            years.extend(s.knowledge_year);
        }
    }
    years.extend(t.eras.iter().map(|e| e.start));
    years.extend(t.eras.iter().map(|e| e.end));
    years.extend(t.markers.iter().filter_map(|m| m.pub_year));
    years.extend(t.guidelines.iter().filter_map(|g| g.year));
    years.extend(t.run_year);
    let from = *years.iter().min()?;
    let to = *years.iter().max()? + 1;
    let x = |y: f64| x_of(y, from, to);

    let mut sorted = primaries.clone();
    sorted.sort_by_key(|s| {
        (
            s.treatment_start.unwrap_or(i32::MAX),
            s.treatment_end.unwrap_or(i32::MAX),
        )
    });
    let rows: Vec<GanttRow> = sorted
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let y = GANTT_T + 10.0 + i as f64 * GANTT_ROW;
            let bar = match (s.treatment_start, s.treatment_end) {
                (Some(a), Some(b)) => Some((x(f64::from(a)), x(f64::from(b.max(a)) + 1.0))),
                (Some(a), None) | (None, Some(a)) => Some((x(f64::from(a)), x(f64::from(a) + 1.0))),
                (None, None) => None,
            };
            let points = t
                .studies
                .iter()
                .filter(|c| c.parent_id.as_deref() == Some(s.id.as_str()))
                .filter_map(|c| {
                    let k = c.knowledge_year?;
                    Some(KnowPoint {
                        x: x(f64::from(k) + 0.5),
                        label: c
                            .horizon
                            .as_ref()
                            .map(|h| h.text.clone())
                            .unwrap_or_else(|| format!("follow-up {k}")),
                    })
                })
                .collect();
            let note = match (s.n, bar) {
                (_, None) => "years unknown".into(),
                (Some(n), _) => format!("n {n}"),
                (None, _) => String::new(),
            };
            GanttRow {
                label: clip(&s.label, GANTT_LABEL_MAX),
                title: s.label.clone(),
                y,
                bar,
                dashed: s.assumed,
                note,
                points,
            }
        })
        .collect();
    let n = t.eras.len();
    let band_top = GANTT_T - 24.0;
    let band_h = rows.len() as f64 * GANTT_ROW + 30.0;
    let bands = t
        .eras
        .iter()
        .enumerate()
        .map(|(i, e)| Band {
            x: x(f64::from(e.start)),
            w: x(f64::from(e.end) + 1.0) - x(f64::from(e.start)),
            label: e.label.clone(),
            class: era_class(i, n),
        })
        .collect();
    let axis_y = GANTT_T + rows.len() as f64 * GANTT_ROW + 12.0;
    let step = year_step(to - from);
    let ticks = (from..to)
        .filter(|y| y.rem_euclid(step) == 0)
        .map(|y| (x(f64::from(y)), y))
        .collect();

    // Marker lines: one per guideline year, one per meta-analysis year.
    let mut lines: Vec<(i32, bool, String)> = Vec::new();
    let mut gl_years: Vec<i32> = t.guidelines.iter().filter_map(|g| g.year).collect();
    gl_years.sort_unstable();
    gl_years.dedup();
    for y in gl_years {
        let k = t.guidelines.iter().filter(|g| g.year == Some(y)).count();
        let label = if k == 1 {
            let g = t.guidelines.iter().find(|g| g.year == Some(y)).unwrap();
            clip(&format!("{y}: guideline, {}", g.body), 40)
        } else {
            format!("{y}: {k} guidelines")
        };
        lines.push((y, true, label));
    }
    let mut ma_years: Vec<i32> = t.markers.iter().filter_map(|m| m.pub_year).collect();
    ma_years.sort_unstable();
    ma_years.dedup();
    for y in ma_years {
        let k = t.markers.iter().filter(|m| m.pub_year == Some(y)).count();
        let label = if k == 1 {
            format!("{y}: 1 meta-analysis or review")
        } else {
            format!("{y}: {k} meta-analyses and reviews")
        };
        lines.push((y, false, label));
    }
    lines.sort_by_key(|l| (l.0, !l.1));
    let mid = GANTT_L + (GANTT_W - GANTT_L - GANTT_R) * 0.6;
    let markers: Vec<MarkerLine> = lines
        .into_iter()
        .enumerate()
        .map(|(i, (y, guideline, label))| {
            let lx = x(f64::from(y) + 0.5);
            MarkerLine {
                x: lx,
                label,
                label_y: axis_y + 32.0 + 13.0 * i as f64,
                anchor_end: lx > mid,
                guideline,
            }
        })
        .collect();
    let strip = 13.0 * markers.len() as f64;
    Some(Gantt {
        width: GANTT_W,
        height: axis_y + 26.0 + strip + if markers.is_empty() { 0.0 } else { 8.0 },
        from,
        to,
        band_top,
        band_h,
        bands,
        rows,
        ticks,
        axis_y,
        markers,
    })
}

/// A value axis.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Axis {
    /// Ratios: log scale, no difference at 1.
    Log { lo: f64, hi: f64 },
    /// Differences: linear, no difference at 0.
    Linear { lo: f64, hi: f64 },
}

const LOG_TICKS: [f64; 19] = [
    0.05, 0.1, 0.2, 0.25, 0.33, 0.5, 0.67, 0.75, 1.0, 1.25, 1.5, 2.0, 3.0, 4.0, 5.0, 10.0, 20.0,
    50.0, 100.0,
];

impl Axis {
    /// The axis for `family` covering `values` and the null, padded.
    pub fn for_values(family: Family, values: &[f64]) -> Axis {
        match family {
            Family::Ratio => {
                let logs: Vec<f64> = values
                    .iter()
                    .filter(|v| v.is_finite() && **v > 0.0)
                    .map(|v| v.ln())
                    .chain([0.0])
                    .collect();
                let lo = logs.iter().copied().fold(f64::INFINITY, f64::min);
                let hi = logs.iter().copied().fold(f64::NEG_INFINITY, f64::max);
                let pad = ((hi - lo) * 0.08).max(0.15);
                Axis::Log {
                    lo: (lo - pad).exp(),
                    hi: (hi + pad).exp(),
                }
            }
            Family::Difference => {
                let vs: Vec<f64> = values
                    .iter()
                    .copied()
                    .filter(|v| v.is_finite())
                    .chain([0.0])
                    .collect();
                let lo = vs.iter().copied().fold(f64::INFINITY, f64::min);
                let hi = vs.iter().copied().fold(f64::NEG_INFINITY, f64::max);
                let pad = ((hi - lo) * 0.1).max(if hi == lo { 1.0 } else { 0.0 });
                Axis::Linear {
                    lo: lo - pad,
                    hi: hi + pad,
                }
            }
        }
    }

    pub fn is_log(&self) -> bool {
        matches!(self, Axis::Log { .. })
    }

    /// No difference: 1 for ratios, 0 for differences.
    pub fn null(&self) -> f64 {
        if self.is_log() { 1.0 } else { 0.0 }
    }

    /// Where `v` sits, 0 at the bottom and 1 at the top.
    pub fn frac(&self, v: f64) -> f64 {
        match *self {
            Axis::Log { lo, hi } => (v.max(1e-9).ln() - lo.ln()) / (hi.ln() - lo.ln()),
            Axis::Linear { lo, hi } => (v - lo) / (hi - lo),
        }
    }

    /// The tick values (the null always among them).
    pub fn ticks(&self) -> Vec<f64> {
        match *self {
            Axis::Log { lo, hi } => {
                let mut t: Vec<f64> = LOG_TICKS
                    .iter()
                    .copied()
                    .filter(|v| *v >= lo && *v <= hi)
                    .collect();
                while t.len() > 7 {
                    // Thin out, keeping 1.
                    t = t
                        .iter()
                        .enumerate()
                        .filter(|(i, v)| **v == 1.0 || i % 2 == 0)
                        .map(|(_, v)| *v)
                        .collect();
                }
                t
            }
            Axis::Linear { lo, hi } => {
                let step = nice_step((hi - lo) / 5.0);
                let mut v = (lo / step).ceil() * step;
                let mut t = Vec::new();
                while v <= hi + step * 1e-9 && t.len() < 12 {
                    t.push(if v.abs() < step * 1e-9 { 0.0 } else { v });
                    v += step;
                }
                t
            }
        }
    }

    /// A tick's label.
    pub fn tick_label(&self, v: f64) -> String {
        if self.is_log() {
            let s = format!("{v:.2}");
            s.trim_end_matches('0').trim_end_matches('.').to_string()
        } else if (v - v.round()).abs() < 1e-9 {
            let r = v.round();
            if r < 0.0 {
                format!("\u{2212}{}", -r)
            } else {
                format!("{}", r.abs())
            }
        } else {
            fmt_plain(v)
        }
    }
}

/// 1, 2 or 5 × a power of ten, at least `raw`.
pub fn nice_step(raw: f64) -> f64 {
    if !(raw.is_finite() && raw > 0.0) {
        return 1.0;
    }
    let p = 10f64.powf(raw.log10().floor());
    let m = raw / p;
    let k = if m <= 1.0 {
        1.0
    } else if m <= 2.0 {
        2.0
    } else if m <= 5.0 {
        5.0
    } else {
        10.0
    };
    k * p
}

/// The linked chart's width (viewBox) and margins.
pub const LINKED_W: f64 = 900.0;
pub const LINKED_H: f64 = 280.0;
pub const LINKED_L: f64 = 64.0;
pub const LINKED_R: f64 = 24.0;
pub const LINKED_T: f64 = 20.0;
pub const LINKED_B: f64 = 40.0;
/// The horizontal step between series at one era.
pub const LINKED_OFFSET: f64 = 14.0;

/// One era point of a series.
#[derive(Debug, Clone, PartialEq)]
pub struct Pt {
    pub x: f64,
    /// The estimate's y (`None`: a split era, drawn as a tag).
    pub y: Option<f64>,
    /// The CI whisker's ends (top, bottom).
    pub whisker: Option<(f64, f64)>,
    pub label: String,
    /// No CI (points only): a hollow point.
    pub hollow: bool,
    /// The era's rows rest on assumed treatment years: a dashed ring.
    pub assumed: bool,
    pub split: bool,
}

/// A segment joining two consecutive points; dashed when either is assumed.
#[derive(Debug, Clone, PartialEq)]
pub struct Seg {
    pub x1: f64,
    pub y1: f64,
    pub x2: f64,
    pub y2: f64,
    pub dashed: bool,
}

/// One outcome on a panel.
#[derive(Debug, Clone, PartialEq)]
pub struct Series {
    pub outcome: String,
    /// The colour slot, 0 to 2, in the chart's outcome order.
    pub slot: usize,
    pub points: Vec<Pt>,
    pub segs: Vec<Seg>,
}

/// One chart panel: outcomes of one family and unit on one axis.
#[derive(Debug, Clone, PartialEq)]
pub struct Panel {
    pub family: Family,
    pub unit_key: String,
    pub axis: Axis,
    pub width: f64,
    pub height: f64,
    /// Ticks: y and label.
    pub ticks: Vec<(f64, String)>,
    pub null_y: f64,
    /// Era columns: x and label.
    pub cols: Vec<(f64, String)>,
    pub series: Vec<Series>,
}

impl Panel {
    /// The panel's caption ("Ratio, log scale: 1 is no difference").
    pub fn caption(&self) -> String {
        match self.family {
            Family::Ratio => "Ratio, log scale; 1 is no difference.".into(),
            Family::Difference => {
                let u = self.unit_key.trim();
                if u.is_empty() {
                    "Difference, linear scale; 0 is no difference.".into()
                } else {
                    format!("Difference in {u}, linear scale; 0 is no difference.")
                }
            }
        }
    }
}

/// The era cell's rows rest on a study with assumed treatment years.
pub fn cell_assumed(t: &TemporalTable, c: &EraCell) -> bool {
    c.row_ids.iter().any(|rid| {
        t.rows
            .iter()
            .find(|r| &r.id == rid)
            .and_then(|r| t.studies.iter().find(|s| s.id == r.study_id))
            .is_some_and(|s| s.assumed)
    })
}

/// The linked chart: [`linked_outcomes`] grouped by family and unit (one
/// panel each, in first-appearance order), the table's eras as columns.
pub fn linked_panels(t: &TemporalTable) -> Vec<Panel> {
    let chosen = linked_outcomes(t);
    let mut keys: Vec<(Family, String)> = Vec::new();
    for (_, g) in &chosen {
        let k = (g.family, g.unit_key.clone());
        if !keys.contains(&k) {
            keys.push(k);
        }
    }
    let n = t.eras.len().max(1);
    let cx = |i: usize| LINKED_L + (i as f64 + 0.5) * (LINKED_W - LINKED_L - LINKED_R) / n as f64;
    let cols: Vec<(f64, String)> = t
        .eras
        .iter()
        .enumerate()
        .map(|(i, e)| (cx(i), e.label.clone()))
        .collect();
    keys.into_iter()
        .map(|(family, unit)| {
            let members: Vec<(usize, &OutcomeResult, &GroupResult)> = chosen
                .iter()
                .enumerate()
                .filter(|(_, (_, g))| g.family == family && g.unit_key == unit)
                .map(|(slot, (o, g))| (slot, *o, *g))
                .collect();
            let mut values = Vec::new();
            for (_, _, g) in &members {
                for c in g.cells.iter().filter(|c| has_data(c)) {
                    values.extend(c.estimate);
                    if let Some(p) = &c.pooled {
                        values.extend([p.lo, p.hi]);
                    }
                }
            }
            let axis = Axis::for_values(family, &values);
            let y = |v: f64| LINKED_T + (1.0 - axis.frac(v)) * (LINKED_H - LINKED_T - LINKED_B);
            let m = members.len();
            let series = members
                .iter()
                .enumerate()
                .map(|(k, (slot, o, g))| {
                    let off = (k as f64 - (m as f64 - 1.0) / 2.0) * LINKED_OFFSET;
                    let points: Vec<Pt> = t
                        .eras
                        .iter()
                        .enumerate()
                        .filter_map(|(i, e)| {
                            let c = g.cells.iter().find(|c| c.era_id == e.id)?;
                            if !has_data(c) {
                                return None;
                            }
                            Some(Pt {
                                x: cx(i) + off,
                                y: c.estimate.map(y),
                                whisker: c.pooled.as_ref().map(|p| (y(p.hi), y(p.lo))),
                                label: c
                                    .estimate
                                    .map(|v| fmt_value(family, v, ""))
                                    .unwrap_or_else(|| "split".into()),
                                hollow: c.pooled.is_none(),
                                assumed: cell_assumed(t, c),
                                split: c.estimate.is_none() && c.split,
                            })
                        })
                        .collect();
                    let drawn: Vec<&Pt> = points.iter().filter(|p| p.y.is_some()).collect();
                    let segs = drawn
                        .windows(2)
                        .map(|w| Seg {
                            x1: w[0].x,
                            y1: w[0].y.unwrap_or_default(),
                            x2: w[1].x,
                            y2: w[1].y.unwrap_or_default(),
                            dashed: w[0].assumed || w[1].assumed,
                        })
                        .collect();
                    Series {
                        outcome: o.outcome.clone(),
                        slot: *slot,
                        points,
                        segs,
                    }
                })
                .collect();
            let ticks = axis
                .ticks()
                .into_iter()
                .map(|v| (y(v), axis.tick_label(v)))
                .collect();
            Panel {
                family,
                unit_key: unit,
                axis,
                width: LINKED_W,
                height: LINKED_H,
                ticks,
                null_y: y(axis.null()),
                cols: cols.clone(),
                series,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tv(status: &str) -> TemporalView {
        TemporalView {
            status: status.into(),
            reason: None,
            topic: None,
            evidence_year: None,
            board: None,
            table: None,
            flags: None,
        }
    }

    #[test]
    fn option_state_per_status() {
        use OptionState::*;
        let cases = [
            ("pending", Pending, "Expert opinion", false, true),
            ("done", Ready, "Expert opinion", true, false),
            (
                "skipped",
                Skipped,
                "Expert opinion \u{b7} not enough studies",
                false,
                false,
            ),
            (
                "failed",
                Failed,
                "Expert opinion \u{b7} unavailable",
                false,
                false,
            ),
            (
                "cap_blocked",
                CapBlocked,
                "Expert opinion \u{b7} not run (daily limit)",
                false,
                false,
            ),
            (
                "something else",
                Failed,
                "Expert opinion \u{b7} unavailable",
                false,
                false,
            ),
        ];
        for (s, state, text, selectable, polls) in cases {
            let o = OptionState::from_status(s);
            assert_eq!(o, state, "{s}");
            assert_eq!(o.text(), text, "{s}");
            assert_eq!(o.selectable(), selectable, "{s}");
            assert_eq!(o.dot(), selectable, "{s}");
            assert_eq!(o.polls(), polls, "{s}");
        }
        // A done row whose board does not read is unavailable.
        assert_eq!(OptionState::of(&tv("done")), Failed);
        assert_eq!(OptionState::of(&tv("pending")), Pending);
        assert_eq!(POLL_EVERY_SECS * u64::from(POLL_MAX), 600);
    }

    #[test]
    fn citations_keep_every_character() {
        let known: BTreeSet<u16> = [1, 3].into_iter().collect();
        let p = cite_pieces(
            "Pain equal [1, 2] and [3]; see [2\u{2013}3]. Not [x].",
            &known,
        );
        assert_eq!(
            p,
            vec![
                Piece::Text("Pain equal [".into()),
                Piece::Ref(1),
                Piece::Text(", 2] and [".into()),
                Piece::Ref(3),
                Piece::Text("]; see [2\u{2013}".into()),
                Piece::Ref(3),
                Piece::Text("]. Not [x].".into()),
            ]
        );
        let back: String = p
            .iter()
            .map(|x| match x {
                Piece::Text(t) => t.clone(),
                Piece::Ref(n) => n.to_string(),
            })
            .collect();
        assert_eq!(
            back,
            "Pain equal [1, 2] and [3]; see [2\u{2013}3]. Not [x]."
        );
        assert_eq!(cite_pieces("", &known), vec![]);
        // No known references: plain text, unchanged.
        assert_eq!(
            cite_pieces("a [1] b", &BTreeSet::new()),
            vec![Piece::Text("a [1] b".into())]
        );
    }

    #[test]
    fn known_refs_leave_out_the_flagged_ones() {
        let b = Board {
            opinion: Opinion {
                verdict: "x [1, 2] y [16]".into(),
                ..Opinion::default()
            },
            ..Board::default()
        };
        let f = Flags {
            unknown_references: vec![Flag {
                field: "opinion.verdict".into(),
                text: "[16]".into(),
            }],
            ..Flags::default()
        };
        assert_eq!(known_refs(&b, Some(&f)), [1, 2].into_iter().collect());
        assert!(known_refs(&b, None).is_empty());
    }

    fn era(start: i32, end: i32, label: &str) -> Era {
        Era {
            id: format!("e-{start}-{end}"),
            start,
            end,
            label: label.into(),
            n_studies: 1,
            empty: false,
        }
    }

    #[test]
    fn ledger_headers_follow_the_label_mode() {
        let eras = [
            era(2011, 2015, "2011\u{2013}15"),
            era(2016, 2020, "2016\u{2013}20"),
            era(2021, 2026, "2021\u{2013}26"),
        ];
        assert_eq!(
            ledger_headers("treatment", &eras, 3),
            [
                "After 2011\u{2013}15",
                "After 2016\u{2013}20",
                "Now, after 2021\u{2013}26"
            ]
        );
        assert_eq!(
            ledger_headers(" Known_By ", &eras, 3),
            ["Known by 2015", "Known by 2020", "Known now, 2026"]
        );
        // Fewer columns: the latest eras.
        assert_eq!(
            ledger_headers("known_by", &eras, 2),
            ["Known by 2020", "Known now, 2026"]
        );
        // More columns than eras: "Earlier" first.
        assert_eq!(
            ledger_headers("treatment", &eras[..1], 2),
            ["Earlier", "Now, after 2011\u{2013}15"]
        );
        // An unknown mode reads as treatment eras.
        assert_eq!(
            ledger_headers("era", &eras, 1),
            ["Now, after 2021\u{2013}26"]
        );
        assert_eq!(ledger_headers("treatment", &[], 2), ["Earlier", "Now"]);
        assert!(ledger_headers("treatment", &eras, 0).is_empty());
        assert_eq!(
            ledger_note("known_by"),
            "Columns are the years results became known."
        );
    }

    #[test]
    fn era_classes_end_on_the_latest_tint() {
        assert_eq!(
            (0..4).map(|i| era_class(i, 4)).collect::<Vec<_>>(),
            [2, 1, 2, 3]
        );
        assert_eq!(
            (0..3).map(|i| era_class(i, 3)).collect::<Vec<_>>(),
            [1, 2, 3]
        );
        assert_eq!((0..2).map(|i| era_class(i, 2)).collect::<Vec<_>>(), [2, 3]);
        let e = Era {
            empty: true,
            n_studies: 0,
            ..era(2011, 2015, "2011\u{2013}15")
        };
        assert_eq!(era_chip(0, &e, 4), "No studies");
        assert_eq!(era_chip(3, &e, 4), "Latest five years");
    }

    #[test]
    fn the_scale_is_log_for_ratios_and_linear_for_differences() {
        let a = Axis::for_values(Family::Ratio, &[0.68, 0.49, 0.94, 2.29]);
        assert!(a.is_log());
        assert_eq!(a.null(), 1.0);
        let Axis::Log { lo, hi } = a else { panic!() };
        assert!(lo < 0.49 && hi > 2.29);
        // Equal steps in log space.
        assert!((a.frac(2.0) - a.frac(1.0) - (a.frac(1.0) - a.frac(0.5))).abs() < 1e-9);
        assert!(a.ticks().contains(&1.0));
        // Only values above 1: the axis still holds the null.
        let b = Axis::for_values(Family::Ratio, &[1.64, 2.02]);
        assert!(b.frac(1.0) > 0.0);

        let d = Axis::for_values(Family::Difference, &[12.6, -15.6]);
        assert!(!d.is_log());
        assert_eq!(d.null(), 0.0);
        assert!((d.frac(0.0) - 0.5).abs() < 0.1);
        let t = d.ticks();
        assert!(t.contains(&0.0), "{t:?}");
        assert!(t.windows(2).all(|w| w[1] > w[0]));
        assert_eq!(d.tick_label(0.0), "0");
        assert_eq!(d.tick_label(-10.0), "\u{2212}10");
        assert_eq!(d.tick_label(2.5), "2.5");
        assert_eq!(a.tick_label(0.5), "0.5");
        // Positive only: 0 is on the axis.
        let e = Axis::for_values(Family::Difference, &[35.0]);
        assert!(e.frac(0.0) >= 0.0 && e.frac(35.0) <= 1.0);
        assert_eq!(nice_step(7.3), 10.0);
        assert_eq!(nice_step(1.4), 2.0);
        assert_eq!(nice_step(0.03), 0.05);
    }

    #[test]
    fn numbers_are_written_plainly() {
        assert_eq!(fmt_diff(35.0, "min"), "+35.0 min");
        assert_eq!(fmt_diff(-18.5, "pp"), "\u{2212}18.5 pp");
        assert_eq!(fmt_diff(-0.01, "index"), "\u{2212}0.01 index");
        assert_eq!(fmt_diff(-123.05, "min"), "\u{2212}123 min");
        assert_eq!(fmt_diff(0.0, ""), "0.00");
        assert_eq!(fmt_ratio(0.6789), "0.68");
        assert_eq!(fmt_range(Family::Ratio, 1.28, 2.03, ""), "1.28\u{2013}2.03");
        assert_eq!(
            fmt_range(Family::Difference, -0.34, 0.9, "points"),
            "\u{2212}0.34 to 0.90 points"
        );
        assert_eq!(fmt_z(-4.27), "\u{2212}4.3");
        assert_eq!(px(12.345), "12.3");
        assert_eq!(px(12.0), "12");
        assert_eq!(clip("abcdef", 4), "abc\u{2026}");
    }

    #[test]
    fn debate_sides_and_status_chips() {
        assert_eq!(side("researcher"), Side::Researcher);
        assert_eq!(side("Skeptic"), Side::Skeptic);
        assert_eq!(side("writer"), Side::Other);
        assert_eq!(speaker_label("researcher"), "Latest-period researcher");
        assert_eq!(speaker_label(" writer "), "writer");
        assert_eq!(status_class("leading"), "rt-chip");
        assert_eq!(status_class("Unresolved"), "rt-chip warn");
        assert_eq!(status_class("weakened"), "rt-chip grey");
        let r = Round {
            round: 2,
            title: " Tested ".into(),
            ..Round::default()
        };
        assert_eq!(round_tab(&r), ("Round 2".into(), "Tested".into()));
    }

    #[test]
    fn poll_ticks_stop_when_ready_or_after_ten_minutes() {
        use OptionState::*;
        assert_eq!(next_poll(0, Pending), Poll::Continue);
        assert_eq!(next_poll(POLL_MAX - 1, Pending), Poll::Continue);
        assert_eq!(next_poll(POLL_MAX, Pending), Poll::GiveUp);
        assert_eq!(next_poll(POLL_MAX + 5, Pending), Poll::GiveUp);
        for s in [Ready, Skipped, Failed, CapBlocked] {
            assert_eq!(next_poll(0, s), Poll::Stop, "{s:?}");
            assert_eq!(next_poll(POLL_MAX, s), Poll::Stop, "{s:?}");
        }
        // Every tick from the first to the last asks: 40 polls, 15 s apart.
        let asks = (0..1000)
            .take_while(|t| next_poll(*t, Pending) == Poll::Continue)
            .count();
        assert_eq!(asks as u64 * POLL_EVERY_SECS, 600);
        assert!(POLL_GAVE_UP_NOTE.contains("reload later"));
    }

    #[test]
    fn px_writes_zero_for_values_that_are_not_finite() {
        assert_eq!(px(f64::NAN), "0");
        assert_eq!(px(f64::INFINITY), "0");
        assert_eq!(px(f64::NEG_INFINITY), "0");
        assert_eq!(px(12.34), "12.3");
        assert_eq!(px(12.0), "12");
        assert_eq!(px(-0.04), "0");
    }

    #[test]
    fn year_step_keeps_the_timeline_to_twelve_ticks() {
        assert_eq!(year_step(0), 1);
        assert_eq!(year_step(1), 1);
        assert_eq!(year_step(12), 1);
        assert_eq!(year_step(13), 2);
        assert_eq!(year_step(24), 2);
        assert_eq!(year_step(25), 5);
        assert_eq!(year_step(60), 5);
        assert_eq!(year_step(61), 10);
        assert_eq!(year_step(130), 20);
        assert_eq!(year_step(290), 25);
        assert_eq!(year_step(5000), 500);
        assert_eq!(year_step(100_000), 8334);
        for span in 1..3000 {
            let step = year_step(span);
            // Ticks over any `span` consecutive years.
            for from in [-7, 0, 1993, 2001] {
                let n = (from..from + span)
                    .filter(|y: &i32| y.rem_euclid(step) == 0)
                    .count();
                assert!(n <= 12, "{span}: step {step}, {n} ticks");
            }
        }
    }
}
