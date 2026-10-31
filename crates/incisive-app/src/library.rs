//! The ivory redesign's display rules for the Ask page's latest answers and
//! Today card, the Library (today's "My questions") and the shared header,
//! with no Leptos in them (unit-tested here; shared by the server and the
//! browser).
//!
//! Display only: everything here reads stored runs and answers and decides
//! what a card shows. Nothing here changes a stored value, and nothing here
//! shows a price.

use chrono::{DateTime, Datelike, FixedOffset, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::runs::{RunMode, RunStatus};

/// India Standard Time.
pub fn ist() -> FixedOffset {
    FixedOffset::east_opt(5 * 3600 + 30 * 60).expect("IST offset is valid")
}

/// Now, from the server's clock or (in the browser) the JS clock: chrono's
/// own clock is not available in the browser build.
pub fn now_utc() -> DateTime<Utc> {
    #[cfg(all(feature = "hydrate", not(feature = "ssr")))]
    {
        let ms = web_sys::js_sys::Date::now() as i64;
        DateTime::from_timestamp_millis(ms).unwrap_or_default()
    }
    #[cfg(not(all(feature = "hydrate", not(feature = "ssr"))))]
    {
        Utc::now()
    }
}

/// The IST calendar day of `t`.
pub fn ist_day(t: DateTime<Utc>) -> NaiveDate {
    t.with_timezone(&ist()).date_naive()
}

/// The header's date: `Sun 1 Nov` (IST).
pub fn header_date(now: DateTime<Utc>) -> String {
    now.with_timezone(&ist()).format("%a %-d %b").to_string()
}

/// `16:28` (IST).
pub fn time_ist(t: DateTime<Utc>) -> String {
    t.with_timezone(&ist()).format("%H:%M").to_string()
}

/// A Library day heading: `Today · Sun 1 Nov`, `Yesterday · Sat 31 Oct`,
/// else `Fri 30 Oct` (with the year when it is not this year's).
pub fn day_heading(day: NaiveDate, today: NaiveDate) -> String {
    let short = if day.year() == today.year() {
        day.format("%a %-d %b").to_string()
    } else {
        day.format("%a %-d %b %Y").to_string()
    };
    if day == today {
        format!("Today \u{b7} {short}")
    } else if today.pred_opt() == Some(day) {
        format!("Yesterday \u{b7} {short}")
    } else {
        short
    }
}

/// The avatar's initials: the first letter of the first two words of the
/// display name (an email address counts as its part before `@`),
/// upper-cased; `?` when there is none.
pub fn initials(display_name: &str) -> String {
    let name = display_name.split('@').next().unwrap_or("");
    let out: String = name
        .split(|c: char| c.is_whitespace() || c == '.' || c == '_' || c == '-')
        .filter_map(|w| w.chars().find(|c| c.is_alphanumeric()))
        .take(2)
        .flat_map(char::to_uppercase)
        .collect();
    if out.is_empty() { "?".to_string() } else { out }
}

/// What a card counts under its meta line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CountKind {
    References,
    Studies,
}

/// One run as the latest answers and the Library draw it. Built on the
/// server from the run row and its stored answer (read-only); the user's
/// own runs only.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LibraryCard {
    pub id: Uuid,
    pub mode: RunMode,
    pub status: RunStatus,
    pub created_at: DateTime<Utc>,
    /// The question as asked (the user's own text).
    pub question: String,
    /// Seconds from start to finish; `None` while it runs.
    pub duration_s: Option<i64>,
    /// Whether a stored answer with a written part exists.
    pub has_answer: bool,
    /// The stored verdict (citations stripped), when there is an answer.
    pub verdict: Option<String>,
    /// The bottom-line certainty: 1 Very low … 4 High.
    pub certainty: Option<u8>,
    /// The number of references (or studies) the answer cites.
    pub count: Option<(u32, CountKind)>,
}

/// The parts of a stored answer a card reads (read from the JSON on the
/// server; no other field is loaded).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StoredParts {
    /// `answer.at_a_glance.verdict` (version 3).
    pub glance_verdict: Option<String>,
    /// `answer.bottom_line.text` (version 3), or the first bottom-line
    /// claim (version 2).
    pub bottom_line: Option<String>,
    /// `answer.bottom_line.certainty` as stored ("High" … "Very low").
    pub certainty: Option<String>,
    /// The number of references.
    pub references: Option<u32>,
}

/// The verdict a card shows: the stored At a glance verdict, else the
/// first sentence of the bottom line; citations stripped either way.
/// `None` when neither has text.
pub fn verdict_of(p: &StoredParts) -> Option<String> {
    let glance = p
        .glance_verdict
        .as_deref()
        .map(crate::answer_page::strip_citations)
        .filter(|t| !t.is_empty());
    glance.or_else(|| {
        p.bottom_line
            .as_deref()
            .map(crate::answer_page::first_sentence)
            .filter(|t| !t.is_empty())
    })
}

/// The certainty level of a stored certainty string (1 Very low … 4 High).
pub fn certainty_level(c: &str) -> Option<u8> {
    match c.trim() {
        "High" => Some(4),
        "Moderate" => Some(3),
        "Low" => Some(2),
        "Very low" => Some(1),
        _ => None,
    }
}

/// The name of a certainty level.
pub fn certainty_name(level: u8) -> &'static str {
    match level {
        4 => "High",
        3 => "Moderate",
        2 => "Low",
        1 => "Very low",
        _ => "",
    }
}

/// `●●●○` for a level out of 4.
pub fn pips(level: u8) -> String {
    (1..=4u8)
        .map(|i| if i <= level { '\u{25cf}' } else { '\u{25cb}' })
        .collect()
}

/// The spine's four segments, top first: `true` where filled.
pub fn spine(level: Option<u8>) -> [bool; 4] {
    let l = level.unwrap_or(0);
    [l >= 4, l >= 3, l >= 2, l >= 1]
}

/// Build a card from a run row and what its stored answer holds.
#[allow(clippy::too_many_arguments)]
pub fn card(
    id: Uuid,
    mode: RunMode,
    status: RunStatus,
    created_at: DateTime<Utc>,
    question: String,
    duration_s: Option<i64>,
    stored: Option<StoredParts>,
) -> LibraryCard {
    let verdict = stored.as_ref().and_then(verdict_of);
    let has_answer = verdict.is_some();
    let certainty = stored
        .as_ref()
        .filter(|_| has_answer)
        .and_then(|p| p.certainty.as_deref())
        .and_then(certainty_level);
    let count = stored
        .as_ref()
        .filter(|_| has_answer)
        .and_then(|p| p.references)
        .filter(|n| *n > 0)
        .map(|n| (n, CountKind::References));
    LibraryCard {
        id,
        mode,
        status,
        created_at,
        question,
        duration_s: duration_s.filter(|s| *s >= 0),
        has_answer,
        verdict,
        certainty,
        count,
    }
}

/// What a card without an answer says in place of the verdict.
pub fn no_answer_text(status: RunStatus) -> &'static str {
    match status {
        RunStatus::Queued => "Waiting to start\u{2026}",
        RunStatus::Running => "Working on it\u{2026}",
        RunStatus::Failed => "Didn\u{2019}t finish \u{2014} no answer",
        RunStatus::Interrupted => "Interrupted \u{2014} no answer",
        RunStatus::Done | RunStatus::Partial => "Ended without an answer",
    }
}

/// A short duration for a meta line: `7 min`, or `41 s` under a minute.
pub fn short_duration(s: i64) -> String {
    if s >= 60 {
        format!("{} min", (s + 30) / 60)
    } else {
        format!("{s} s")
    }
}

/// The card's meta line: IST time, duration, reference count. No price.
pub fn meta_line(c: &LibraryCard) -> String {
    let mut parts = vec![time_ist(c.created_at)];
    if let Some(d) = c.duration_s {
        parts.push(short_duration(d));
    }
    if let Some((n, kind)) = c.count {
        let word = match (kind, n == 1) {
            (CountKind::References, true) => "reference",
            (CountKind::References, false) => "references",
            (CountKind::Studies, true) => "study",
            (CountKind::Studies, false) => "studies",
        };
        parts.push(format!("{n} {word}"));
    }
    parts.join(" \u{b7} ")
}

/// A question for grouping: trimmed, case-folded, whitespace squashed.
pub fn normalise_question(q: &str) -> String {
    q.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// Identical questions (normalised): the newest run first, then the
/// earlier ones, newest first.
#[derive(Debug, Clone, PartialEq)]
pub struct RunGroup {
    pub runs: Vec<LibraryCard>,
}

impl RunGroup {
    /// The newest run (the one the card shows).
    pub fn newest(&self) -> &LibraryCard {
        &self.runs[0]
    }
}

/// Group runs (newest first) by normalised question, in the order of each
/// group's newest run.
pub fn group_runs(cards: Vec<LibraryCard>) -> Vec<RunGroup> {
    let mut sorted = cards;
    sorted.sort_by(|a, b| b.created_at.cmp(&a.created_at).then(a.id.cmp(&b.id)));
    let mut keys: Vec<String> = Vec::new();
    let mut groups: Vec<RunGroup> = Vec::new();
    for c in sorted {
        let k = normalise_question(&c.question);
        match keys.iter().position(|x| *x == k) {
            Some(i) => groups[i].runs.push(c),
            None => {
                keys.push(k);
                groups.push(RunGroup { runs: vec![c] });
            }
        }
    }
    groups
}

/// The Library's certainty filter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CertaintyFilter {
    All,
    Level(u8),
}

/// The filter pills, in order.
pub const FILTERS: [CertaintyFilter; 5] = [
    CertaintyFilter::All,
    CertaintyFilter::Level(4),
    CertaintyFilter::Level(3),
    CertaintyFilter::Level(2),
    CertaintyFilter::Level(1),
];

/// Whether a group passes the certainty filter and the search (over the
/// newest run's question and verdict, case-insensitive).
pub fn group_matches(g: &RunGroup, filter: CertaintyFilter, query: &str) -> bool {
    let c = g.newest();
    if let CertaintyFilter::Level(l) = filter
        && c.certainty != Some(l)
    {
        return false;
    }
    let q = normalise_question(query);
    if q.is_empty() {
        return true;
    }
    let hay = normalise_question(&format!(
        "{} {}",
        c.question,
        c.verdict.as_deref().unwrap_or("")
    ));
    hay.contains(&q)
}

/// Groups split by the IST day of their newest run, in order.
pub fn by_day(groups: Vec<RunGroup>) -> Vec<(NaiveDate, Vec<RunGroup>)> {
    let mut out: Vec<(NaiveDate, Vec<RunGroup>)> = Vec::new();
    for g in groups {
        let d = ist_day(g.newest().created_at);
        match out.last_mut() {
            Some((day, list)) if *day == d => list.push(g),
            _ => out.push((d, vec![g])),
        }
    }
    out
}

/// `6 questions · 9 runs`.
pub fn day_summary(groups: &[RunGroup]) -> String {
    let q = groups.len();
    let r: usize = groups.iter().map(|g| g.runs.len()).sum();
    format!(
        "{q} {} \u{b7} {r} {}",
        if q == 1 { "question" } else { "questions" },
        if r == 1 { "run" } else { "runs" }
    )
}

/// The Ask page's Today card. No spend.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TodayCounts {
    /// Today's runs with an answer.
    pub answers: u32,
    /// Distinct (normalised) questions asked today.
    pub questions: u32,
    /// The longest finished run today, in seconds.
    pub longest_s: Option<i64>,
}

/// Today's counts from today's runs.
pub fn today_counts(today: &[LibraryCard]) -> TodayCounts {
    let mut qs: Vec<String> = today
        .iter()
        .map(|c| normalise_question(&c.question))
        .collect();
    qs.sort();
    qs.dedup();
    TodayCounts {
        answers: today.iter().filter(|c| c.has_answer).count() as u32,
        questions: qs.len() as u32,
        longest_s: today.iter().filter_map(|c| c.duration_s).max(),
    }
}

/// What the Ask page loads: recent runs for the latest answers, and
/// today's counts.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AskOverview {
    pub recent: Vec<LibraryCard>,
    pub today: TodayCounts,
}

/// How many recent runs the Ask page loads (grouped, then 4 shown).
pub const ASK_RECENT: i64 = 24;
/// How many latest answers the Ask page shows.
pub const ASK_LATEST: usize = 4;

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn at(h: u32, m: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 11, 1, h, m, 0).unwrap()
    }

    fn c(q: &str, t: DateTime<Utc>, level: Option<u8>, verdict: Option<&str>) -> LibraryCard {
        LibraryCard {
            id: Uuid::new_v4(),
            mode: RunMode::Focused,
            status: RunStatus::Done,
            created_at: t,
            question: q.to_string(),
            duration_s: Some(300),
            has_answer: verdict.is_some(),
            verdict: verdict.map(str::to_string),
            certainty: level,
            count: None,
        }
    }

    #[test]
    fn questions_group_by_normalised_text_newest_first() {
        let a = c("Robotic vs lap hernia?", at(10, 0), Some(2), Some("v1"));
        let b = c(
            "  robotic   VS lap\nhernia? ",
            at(11, 0),
            Some(2),
            Some("v2"),
        );
        let other = c("POEM vs Heller?", at(10, 30), Some(3), Some("v3"));
        let groups = group_runs(vec![a.clone(), other.clone(), b.clone()]);
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].runs.len(), 2);
        assert_eq!(groups[0].newest().id, b.id);
        assert_eq!(groups[0].runs[1].id, a.id);
        assert_eq!(groups[1].newest().id, other.id);
        // Different words stay apart.
        assert_ne!(
            normalise_question("hernia repair"),
            normalise_question("hernia  repairs")
        );
        assert_eq!(normalise_question("  A\tB  c "), "a b c");
    }

    #[test]
    fn verdict_prefers_at_a_glance_then_first_sentence_of_the_bottom_line() {
        let p = StoredParts {
            glance_verdict: Some("No consistent benefit [12345678].".into()),
            bottom_line: Some("Longer and costlier [1]. More text.".into()),
            certainty: Some("Low".into()),
            references: Some(15),
        };
        assert_eq!(verdict_of(&p).as_deref(), Some("No consistent benefit."));
        let p2 = StoredParts {
            glance_verdict: Some("  ".into()),
            ..p.clone()
        };
        assert_eq!(verdict_of(&p2).as_deref(), Some("Longer and costlier."));
        let p3 = StoredParts {
            glance_verdict: None,
            ..p.clone()
        };
        assert_eq!(verdict_of(&p3).as_deref(), Some("Longer and costlier."));
        assert_eq!(verdict_of(&StoredParts::default()), None);
    }

    #[test]
    fn a_run_with_no_answer_shows_its_status_not_a_verdict() {
        let id = Uuid::new_v4();
        let running = card(
            id,
            RunMode::Quick,
            RunStatus::Running,
            at(9, 0),
            "q".into(),
            None,
            None,
        );
        assert!(!running.has_answer);
        assert_eq!(running.verdict, None);
        assert_eq!(running.certainty, None);
        assert_eq!(no_answer_text(running.status), "Working on it\u{2026}");
        // An empty stored answer (no written part) counts as no answer: no
        // certainty or count is shown either.
        let empty = card(
            id,
            RunMode::Focused,
            RunStatus::Done,
            at(9, 0),
            "q".into(),
            Some(30),
            Some(StoredParts {
                certainty: Some("High".into()),
                references: Some(3),
                ..Default::default()
            }),
        );
        assert!(!empty.has_answer);
        assert_eq!(empty.certainty, None);
        assert_eq!(empty.count, None);
        assert_eq!(no_answer_text(RunStatus::Done), "Ended without an answer");
        assert_eq!(
            no_answer_text(RunStatus::Failed),
            "Didn\u{2019}t finish \u{2014} no answer"
        );
    }

    #[test]
    fn certainty_filter_and_search() {
        let g = group_runs(vec![
            c("Hernia repair?", at(9, 0), Some(2), Some("No benefit.")),
            c(
                "POEM?",
                at(8, 0),
                Some(3),
                Some("Same relief, more reflux."),
            ),
            c("Running one", at(7, 0), None, None),
        ]);
        let pick = |f: CertaintyFilter, q: &str| -> Vec<String> {
            g.iter()
                .filter(|x| group_matches(x, f, q))
                .map(|x| x.newest().question.clone())
                .collect()
        };
        assert_eq!(pick(CertaintyFilter::All, "").len(), 3);
        assert_eq!(pick(CertaintyFilter::Level(2), ""), vec!["Hernia repair?"]);
        assert_eq!(pick(CertaintyFilter::Level(3), ""), vec!["POEM?"]);
        assert!(pick(CertaintyFilter::Level(4), "").is_empty());
        // Search covers the verdict too, case-insensitively.
        assert_eq!(pick(CertaintyFilter::All, "REFLUX"), vec!["POEM?"]);
        assert_eq!(pick(CertaintyFilter::Level(2), "reflux").len(), 0);
        assert_eq!(certainty_level("Very low"), Some(1));
        assert_eq!(certainty_level("High"), Some(4));
        assert_eq!(certainty_level("odd"), None);
        assert_eq!(pips(3), "\u{25cf}\u{25cf}\u{25cf}\u{25cb}");
        assert_eq!(spine(Some(2)), [false, false, true, true]);
        assert_eq!(spine(None), [false; 4]);
    }

    #[test]
    fn meta_line_has_time_duration_and_count_but_no_price() {
        let mut x = c("q", at(10, 58), Some(3), Some("v"));
        x.duration_s = Some(7 * 60 + 10);
        x.count = Some((17, CountKind::References));
        // 10:58 UTC is 16:28 IST.
        assert_eq!(meta_line(&x), "16:28 \u{b7} 7 min \u{b7} 17 references");
        x.duration_s = None;
        x.count = None;
        assert_eq!(meta_line(&x), "16:28");
        assert!(!meta_line(&x).contains('\u{20b9}'));
        assert_eq!(short_duration(41), "41 s");
    }

    #[test]
    fn today_counts_days_initials() {
        let mut a = c("Q one", at(9, 0), None, Some("v"));
        a.duration_s = Some(553);
        let b = c("q  ONE", at(9, 5), None, None);
        let d = c("Q two", at(9, 9), None, Some("v"));
        let t = today_counts(&[a, b, d]);
        assert_eq!(
            t,
            TodayCounts {
                answers: 2,
                questions: 2,
                longest_s: Some(553)
            }
        );
        let today = NaiveDate::from_ymd_opt(2026, 11, 1).unwrap();
        assert_eq!(day_heading(today, today), "Today \u{b7} Sun 1 Nov");
        assert_eq!(
            day_heading(today.pred_opt().unwrap(), today),
            "Yesterday \u{b7} Sat 31 Oct"
        );
        assert_eq!(
            day_heading(NaiveDate::from_ymd_opt(2025, 12, 30).unwrap(), today),
            "Tue 30 Dec 2025"
        );
        // 20:00 UTC on 31 Oct is already 1 Nov in IST.
        let late = Utc.with_ymd_and_hms(2026, 10, 31, 20, 0, 0).unwrap();
        assert_eq!(ist_day(late), today);
        assert_eq!(header_date(late), "Sun 1 Nov");
        assert_eq!(initials("Manjunath Haridas"), "MH");
        assert_eq!(initials("dr.manj@example.com"), "DM");
        assert_eq!(initials("owner"), "O");
        assert_eq!(initials(""), "?");
    }
}
