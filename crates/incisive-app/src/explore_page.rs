//! The Explore page's rules (no Leptos): the List view's groups, rows and
//! five-row folds, the card for a dot, the map's size cut, and the page's
//! fixed words. Built for the browser too.

use std::cmp::Ordering;
use std::collections::{BTreeSet, HashSet};

use crate::graph_view::{Dot, DotKind, MapData};

/// Rows a group shows before "Show all N".
pub const FOLD_AT: usize = 5;

/// The map's frames (width, height): the server lays the dots out once for
/// one of these.
pub const PHONE_FRAME: (f32, f32) = (358.0, 480.0);
pub const DESKTOP_FRAME: (f32, f32) = (860.0, 720.0);

pub const TITLE: &str = "Explore";
pub const EMPTY: &str = "Ask your first question to start your map.";
pub const NOT_FOUND: &str = "Page not found. Go back to the question box.";
pub const NO_MATCH: &str = "Nothing in your runs matches. This doesn't mean no evidence exists.";
/// The note under the map and list, word for word.
pub const NOT_ALL: &str =
    "Every dot comes from your own runs, so this is not all published evidence.";
pub const LINE_MEANS: &str =
    "A line means \"looked at in this run\", not \"this treatment works\".";
pub const SQUARES: &str = "Bigger, darker squares are higher-ranked papers.";
/// Under a map with no room for any paper.
pub const PAPERS_HIDDEN: &str = "Papers are hidden on this map; see them in List";
pub const FIND_LABEL: &str = "Find on your map";
pub const FIND_HINT: &str = "e.g. hernia, bile duct, POEM";

/// The first `FOLD_AT` rows and the fold's button ("Show all N", or "Show
/// fewer" when open); no button for `FOLD_AT` rows or fewer.
pub fn fold<T: Clone>(rows: &[T], open: bool) -> (Vec<T>, Option<String>) {
    if rows.len() <= FOLD_AT {
        return (rows.to_vec(), None);
    }
    if open {
        (rows.to_vec(), Some("Show fewer".to_string()))
    } else {
        (
            rows[..FOLD_AT].to_vec(),
            Some(format!("Show all {}", rows.len())),
        )
    }
}

/// One row of the List view.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub id: String,
    pub title: String,
    pub sub: String,
    pub kind: DotKind,
    /// A paper's rank code (its square's colour).
    pub rank: Option<String>,
}

/// A List view group: its heading and every row (the fold is the page's).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Group {
    pub name: String,
    pub rows: Vec<Row>,
}

/// A paper rank's place (0 = RCT ... 4 = case series, 5 = not ranked), its
/// number and its name.
pub fn rank_info(code: Option<&str>) -> (usize, &'static str, &'static str) {
    match code {
        Some("rct") => (0, "1", "RCT"),
        Some("meta") => (1, "2", "Meta-analysis"),
        Some("prospective") => (2, "3", "Prospective"),
        Some("retrospective") => (3, "4", "Retrospective"),
        Some("case_series") => (4, "5", "Case series"),
        _ => (5, "\u{2013}", "Not ranked"),
    }
}

/// Papers: by rank, then newest year (no year last), then id.
fn paper_order(a: &Dot, b: &Dot) -> Ordering {
    rank_info(a.rank.as_deref())
        .0
        .cmp(&rank_info(b.rank.as_deref()).0)
        .then_with(|| match (a.first_year, b.first_year) {
            (Some(x), Some(y)) => y.cmp(&x),
            (Some(_), None) => Ordering::Less,
            (None, Some(_)) => Ordering::Greater,
            (None, None) => Ordering::Equal,
        })
        .then_with(|| a.id.cmp(&b.id))
}

/// The dots linked to `id`, sorted by id.
fn neighbours<'a>(data: &'a MapData, id: &str) -> Vec<&'a Dot> {
    let near: BTreeSet<&str> = data
        .lines
        .iter()
        .filter_map(|l| {
            if l.a == id {
                Some(l.b.as_str())
            } else if l.b == id {
                Some(l.a.as_str())
            } else {
                None
            }
        })
        .collect();
    data.dots
        .iter()
        .filter(|d| near.contains(d.id.as_str()))
        .collect()
}

fn dot<'a>(data: &'a MapData, id: &str) -> Option<&'a Dot> {
    data.dots.iter().find(|d| d.id == id)
}

/// A question's name: its disease's ("Inguinal hernia question").
fn run_title(data: &MapData, run: &Dot) -> String {
    neighbours(data, &run.id)
        .into_iter()
        .find(|d| d.kind == DotKind::Disease)
        .map_or_else(|| run.label.clone(), |d| format!("{} question", d.label))
}

/// "Full · 31 Oct 2026" as "Full run · 31 Oct 2026".
fn run_sub(run: &Dot) -> String {
    match run.sub.split_once(" \u{b7} ") {
        Some((mode, day)) => format!("{mode} run \u{b7} {day}"),
        None => run.sub.clone(),
    }
}

/// "text · Certainty: X" (just "Certainty: X" with no text); the text as
/// it is with no certainty.
fn with_certainty(text: String, certainty: Option<&str>) -> String {
    match certainty.map(str::trim).filter(|c| !c.is_empty()) {
        None => text,
        Some(c) if text.is_empty() => format!("Certainty: {c}"),
        Some(c) => format!("{text} \u{b7} Certainty: {c}"),
    }
}

fn runs_of(data: &MapData, id: &str) -> usize {
    neighbours(data, id)
        .into_iter()
        .filter(|d| d.kind == DotKind::Run)
        .count()
}

fn paper_sub(d: &Dot) -> String {
    let (_, num, name) = rank_info(d.rank.as_deref());
    let mut parts = vec![format!("Rank {num}"), name.to_string()];
    if d.includes_non_rct {
        parts.push("includes non-RCT studies".into());
    }
    if let Some(y) = d.first_year {
        parts.push(y.to_string());
    }
    if d.retracted {
        parts.push("Retracted".into());
    }
    parts.join(" \u{b7} ")
}

fn row_of(data: &MapData, d: &Dot) -> Row {
    let (title, sub) = match d.kind {
        DotKind::Run => (run_title(data, d), run_sub(d)),
        DotKind::Treatment => {
            let n = runs_of(data, &d.id);
            (
                d.label.clone(),
                format!("{n} {}", if n == 1 { "run" } else { "runs" }),
            )
        }
        DotKind::Outcome if !d.sub.is_empty() => (d.label.clone(), format!("Certainty: {}", d.sub)),
        DotKind::Disease => (
            d.label.clone(),
            with_certainty(d.sub.clone(), d.certainty.as_deref()),
        ),
        DotKind::Paper => (d.label.clone(), paper_sub(d)),
        _ => (d.label.clone(), d.sub.clone()),
    };
    Row {
        id: d.id.clone(),
        title,
        sub,
        kind: d.kind,
        rank: d.rank.clone(),
    }
}

/// The List view's groups, in order, each with rows: diseases, treatments,
/// outcomes, the user's questions, then papers by rank and newest year.
/// A kind with no dots has no group; organs are the page's chips.
pub fn groups(data: &MapData) -> Vec<Group> {
    let order = [
        (DotKind::Disease, "Diseases"),
        (DotKind::Treatment, "Treatments"),
        (DotKind::Outcome, "Outcomes"),
        (DotKind::Run, "Your questions"),
        (DotKind::Paper, "Papers, ranked"),
    ];
    order
        .iter()
        .filter_map(|&(kind, name)| {
            let mut dots: Vec<&Dot> = data.dots.iter().filter(|d| d.kind == kind).collect();
            if dots.is_empty() {
                return None;
            }
            let mut rows: Vec<Row>;
            if kind == DotKind::Paper {
                dots.sort_by(|a, b| paper_order(a, b));
                rows = dots.into_iter().map(|d| row_of(data, d)).collect();
            } else {
                rows = dots.into_iter().map(|d| row_of(data, d)).collect();
                rows.sort_by(|a, b| {
                    a.title
                        .to_lowercase()
                        .cmp(&b.title.to_lowercase())
                        .then_with(|| a.id.cmp(&b.id))
                });
            }
            Some(Group {
                name: name.to_string(),
                rows,
            })
        })
        .collect()
}

/// The group's heading: "Diseases · 3".
pub fn group_heading(g: &Group, shown: usize) -> String {
    format!("{} \u{b7} {shown}", g.name)
}

/// Whether a row matches the find box: every word, in its title or sub.
pub fn row_matches(row: &Row, query: &str) -> bool {
    let hay = format!("{} {}", row.title, row.sub).to_lowercase();
    query
        .to_lowercase()
        .split_whitespace()
        .all(|w| hay.contains(w))
}

/// The groups' rows that match the find box; a group with none goes.
pub fn find_in(groups: &[Group], query: &str) -> Vec<Group> {
    groups
        .iter()
        .filter_map(|g| {
            let rows: Vec<Row> = g
                .rows
                .iter()
                .filter(|r| row_matches(r, query))
                .cloned()
                .collect();
            (!rows.is_empty()).then(|| Group {
                name: g.name.clone(),
                rows,
            })
        })
        .collect()
}

/// The organ chips: concept id and name, by name.
pub fn organs(data: &MapData) -> Vec<(i64, String)> {
    let mut o: Vec<(i64, String)> = data
        .dots
        .iter()
        .filter(|d| d.kind == DotKind::Organ)
        .filter_map(|d| Some((d.concept_id?, d.label.clone())))
        .collect();
    o.sort_by(|a, b| {
        a.1.to_lowercase()
            .cmp(&b.1.to_lowercase())
            .then(a.0.cmp(&b.0))
    });
    o
}

/// A dot's card: what it is, what it links to, and where it opens.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Card {
    pub kicker: String,
    /// A paper's rank chip before the kicker ("1 · RCT"; "Not ranked") and
    /// its rank colour class.
    pub chip: Option<(String, &'static str)>,
    pub title: String,
    pub body: String,
    /// Warnings shown on the card ("Retracted", "includes non-RCT studies").
    pub flags: Vec<String>,
    /// Linked dots other than papers: (dot id, label).
    pub links: Vec<(String, String)>,
    /// The run page ("Open the full answer").
    pub answer: Option<String>,
    /// A paper's study sheet on the run page.
    pub sheet: Option<String>,
    /// A paper on PubMed.
    pub pubmed: Option<String>,
    /// "Ask about this": the Ask page with this concept's id (never its name).
    pub ask: Option<String>,
    /// A paper's questions on the map ("From your question: ..."): (dot
    /// id, name), each name once.
    pub from: Vec<(String, String)>,
}

/// The card before anything is picked.
pub fn default_card() -> Card {
    Card {
        kicker: "Your map".into(),
        title: "Tap a dot".into(),
        body: "Tap any dot to light up what it connects to. Search or pick an organ to narrow the map."
            .into(),
        ..Card::default()
    }
}

/// A number with Indian digit grouping (1,43,717).
pub fn indian_number(n: i64) -> String {
    let digits = n.unsigned_abs().to_string();
    let (head, tail) = if digits.len() > 3 {
        digits.split_at(digits.len() - 3)
    } else {
        ("", digits.as_str())
    };
    let mut groups: Vec<&str> = Vec::new();
    let mut h = head;
    while h.len() > 2 {
        let (a, b) = h.split_at(h.len() - 2);
        groups.push(b);
        h = a;
    }
    if !h.is_empty() {
        groups.push(h);
    }
    groups.reverse();
    groups.push(tail);
    let sign = if n < 0 { "-" } else { "" };
    format!("{sign}{}", groups.join(","))
}

fn run_href(id: uuid::Uuid) -> String {
    format!("/runs/{id}")
}

/// Where "Ask about this" goes: the Ask page with the concept's id. The id
/// only, never the name (the Ask page looks the name up for its owner).
fn ask_href(d: &Dot) -> Option<String> {
    d.concept_id.map(|id| format!("/?topic={id}"))
}

/// The card for dot `id`; `None` when it is not on the map.
pub fn card_for(data: &MapData, id: &str) -> Option<Card> {
    let d = dot(data, id)?;
    let near = neighbours(data, id);
    let links: Vec<(String, String)> = near
        .iter()
        .filter(|n| n.kind != DotKind::Paper)
        .map(|n| {
            let label = if n.kind == DotKind::Run {
                run_title(data, n)
            } else {
                n.label.clone()
            };
            (n.id.clone(), label)
        })
        .collect();
    let first_run = near
        .iter()
        .filter(|n| n.kind == DotKind::Run)
        .find_map(|n| n.run_id);
    let linked = |n: usize| {
        format!(
            "Linked to {n} {} on your map.",
            if n == 1 { "dot" } else { "dots" }
        )
    };
    let mut c = Card {
        title: d.label.clone(),
        links,
        ..Card::default()
    };
    match d.kind {
        DotKind::Paper => {
            let (place, num, name) = rank_info(d.rank.as_deref());
            c.kicker = "Paper".into();
            let chip = if place < 5 {
                format!("{num} \u{b7} {name}")
            } else {
                name.to_string()
            };
            c.chip = Some((chip, rank_class(d.rank.as_deref())));
            let pmid = d
                .pmid
                .as_deref()
                .filter(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()));
            let mut body: Vec<String> = Vec::new();
            if !d.design.trim().is_empty() {
                body.push(d.design.trim().to_string());
            }
            if let Some(y) = d.first_year {
                body.push(y.to_string());
            }
            if let Some(n) = d.n {
                body.push(format!("n={}", indian_number(n)));
            }
            if let Some(p) = pmid {
                body.push(format!("PMID {p}"));
            }
            c.body = body.join(" \u{b7} ");
            if d.retracted {
                c.flags.push("Retracted".into());
            }
            if d.includes_non_rct {
                c.flags.push("includes non-RCT studies".into());
            }
            // A paper's lines go to its questions: named on the card
            // ("From your question"), not offered as buttons.
            let mut seen: BTreeSet<String> = BTreeSet::new();
            c.from = std::mem::take(&mut c.links)
                .into_iter()
                .filter(|(_, label)| seen.insert(label.clone()))
                .collect();
            c.sheet = d.sheet.map(|s| format!("{}#ref-{}", run_href(s.run), s.n));
            c.pubmed = pmid.map(|p| format!("https://pubmed.ncbi.nlm.nih.gov/{p}/"));
        }
        DotKind::Run => {
            // As on the board: the question as asked and its bottom line
            // (the user's own run; the map is never anyone else's).
            c.kicker = with_certainty(run_sub(d), d.certainty.as_deref());
            c.title = d
                .question
                .as_deref()
                .map(str::trim)
                .filter(|q| !q.is_empty())
                .map_or_else(|| run_title(data, d), str::to_string);
            c.body = d
                .bottom_line
                .as_deref()
                .map(str::trim)
                .filter(|b| !b.is_empty())
                .map_or_else(|| linked(near.len()), str::to_string);
            c.answer = d.run_id.map(run_href);
        }
        DotKind::Outcome => {
            c.kicker = if d.sub.is_empty() {
                "Outcome".into()
            } else {
                format!("Outcome \u{b7} Certainty: {}", d.sub)
            };
            c.body =
                "One of up to four outcomes chosen for this run. The figures are in the answer."
                    .into();
            c.answer = first_run.map(run_href);
        }
        DotKind::Treatment => {
            c.kicker = "Treatment".into();
            let n = runs_of(data, id);
            c.body = if n > 1 {
                format!("Looked at in {n} of your runs, so this dot connects those topics.")
            } else {
                "Looked at in one of your runs.".into()
            };
            c.ask = ask_href(d);
        }
        DotKind::Disease | DotKind::Organ => {
            c.kicker = if d.kind == DotKind::Disease {
                "Disease"
            } else {
                "Organ"
            }
            .into();
            c.body = linked(near.len());
            if d.kind == DotKind::Disease {
                c.ask = ask_href(d);
            }
        }
    }
    Some(c)
}

/// Cut the map to at most `max` dots before it is laid out: papers go
/// first, keeping the highest ranked and newest, and `top_papers` says how
/// many stayed. Lines to a dot that went go too. A map within `max` is left
/// as it is.
pub fn cap_for_map(data: &mut MapData, max: usize) {
    if data.dots.len() <= max {
        return;
    }
    let had_papers = data.dots.iter().any(|d| d.kind == DotKind::Paper);
    let (mut papers, mut others): (Vec<Dot>, Vec<Dot>) = std::mem::take(&mut data.dots)
        .into_iter()
        .partition(|d| d.kind == DotKind::Paper);
    if others.len() > max {
        // Too many even without papers: keep the kinds that hold the map
        // together first.
        let place = |k: DotKind| match k {
            DotKind::Disease => 0,
            DotKind::Organ => 1,
            DotKind::Treatment => 2,
            DotKind::Run => 3,
            _ => 4,
        };
        others.sort_by(|a, b| {
            place(a.kind)
                .cmp(&place(b.kind))
                .then_with(|| a.id.cmp(&b.id))
        });
        others.truncate(max);
    }
    let room = max - others.len();
    papers.sort_by(paper_order);
    papers.truncate(room);
    if had_papers && papers.is_empty() {
        // No room left for any paper.
        data.papers_hidden_by_size = true;
    } else if had_papers {
        data.top_papers = Some(u32::try_from(papers.len()).unwrap_or(u32::MAX));
    }
    others.extend(papers);
    others.sort_by(|a, b| a.id.cmp(&b.id));
    data.dots = others;
    let present: HashSet<&str> = data.dots.iter().map(|d| d.id.as_str()).collect();
    data.lines
        .retain(|l| present.contains(l.a.as_str()) && present.contains(l.b.as_str()));
}

/// The line under a cut map.
pub fn top_papers_line(n: u32) -> String {
    if n == 1 {
        "Showing the top 1 paper".to_string()
    } else {
        format!("Showing the top {n} papers")
    }
}

/// The line under a map that was cut to size, if it was.
pub fn map_note(data: &MapData) -> Option<String> {
    if data.papers_hidden_by_size {
        Some(PAPERS_HIDDEN.to_string())
    } else {
        data.top_papers.map(top_papers_line)
    }
}

/// "12 dots · 14 lines" under the map.
pub fn count_line(data: &MapData) -> String {
    count_words(data.dots.len(), data.lines.len())
}

/// "12 dots · 14 lines".
pub fn count_words(d: usize, l: usize) -> String {
    format!(
        "{d} {} \u{b7} {l} {}",
        if d == 1 { "dot" } else { "dots" },
        if l == 1 { "line" } else { "lines" }
    )
}

// ---------- the map ----------

/// Below this window width the map uses [`PHONE_FRAME`].
pub const PHONE_BELOW: f64 = 700.0;

/// The frame for a browser window this wide.
pub fn frame_for(width: f64) -> (f32, f32) {
    if width < PHONE_BELOW {
        PHONE_FRAME
    } else {
        DESKTOP_FRAME
    }
}

/// The frame the server draws first, from the browser's User-Agent (a
/// phone says "Mobi"); the page checks its real width once it runs.
pub fn frame_for_agent(user_agent: Option<&str>) -> (f32, f32) {
    if user_agent.is_some_and(|ua| ua.contains("Mobi")) {
        PHONE_FRAME
    } else {
        DESKTOP_FRAME
    }
}

/// The zoom range (the registry's), one step of + / −, and one wheel
/// notch.
pub const ZOOM_MIN: f32 = 0.5;
pub const ZOOM_MAX: f32 = 10.0;
pub const ZOOM_STEP: f32 = 1.3;
pub const WHEEL_STEP: f32 = 1.15;
/// A pointer that moves this many screen pixels drags; less is a tap.
pub const DRAG_FROM: f32 = 10.0;
/// From this zoom every lit dot is labelled.
pub const LABELS_FROM_ZOOM: f32 = 1.6;
/// Dots this big or bigger (disease, organ, treatment) are always labelled.
pub const LABELLED_FROM_R: f32 = 11.0;
/// A full screen map with at most this many dots that are not papers
/// labels every lit one.
pub const ROOMY_UP_TO: usize = 40;

/// Zoom and pan: k, then tx and ty in frame units.
pub type ViewBox = (f32, f32, f32);

/// How much dots grow with the zoom: k^0.2, kept within 0.8 to 1.5.
pub fn grow(k: f32) -> f32 {
    k.powf(0.2).clamp(0.8, 1.5)
}

/// A dot's radius on screen at a zoom.
pub fn zoom_radius(r: f32, zoom: f32) -> f32 {
    r * grow(zoom)
}

/// A dot's tap target: at least 30 pixels across.
pub fn hit_size(r: f32) -> f32 {
    (r * 2.0 + 8.0).max(30.0)
}

/// Zoom by `f` about point `p` of the frame (it stays where it is), the
/// zoom kept within [`ZOOM_MIN`] and [`ZOOM_MAX`].
pub fn zoom_about((k, tx, ty): ViewBox, (px, py): (f32, f32), f: f32) -> ViewBox {
    let nk = (k * f).clamp(ZOOM_MIN, ZOOM_MAX);
    let m = nk / k;
    (nk, px - (px - tx) * m, py - (py - ty) * m)
}

/// A pinch: from `v0` with the fingers `d0` apart about `mid0`, to `d`
/// apart about `mid`; the point under the fingers stays under them.
pub fn pinch_view(v0: ViewBox, d0: f32, mid0: (f32, f32), d: f32, mid: (f32, f32)) -> ViewBox {
    let (k0, tx0, ty0) = v0;
    let k = (k0 * d / d0.max(1e-3)).clamp(ZOOM_MIN, ZOOM_MAX);
    let (wx, wy) = ((mid0.0 - tx0) / k0, (mid0.1 - ty0) / k0);
    (k, mid.0 - wx * k, mid.1 - wy * k)
}

/// Where a dot laid out at `xy`, dragged by `off` (layout units), is drawn
/// under the view (frame units).
pub fn screen_at((x, y): (f32, f32), (ox, oy): (f32, f32), (k, tx, ty): ViewBox) -> (f32, f32) {
    (tx + (x + ox) * k, ty + (y + oy) * k)
}

/// Whether a pointer that moved `dx`, `dy` screen pixels is dragging.
pub fn is_drag(dx: f32, dy: f32) -> bool {
    dx.hypot(dy) >= DRAG_FROM
}

/// A page's query string with full screen (`map=full`) put in or taken
/// out; nothing else in it changes.
pub fn map_query(search: &str, full: bool) -> String {
    let mut parts: Vec<&str> = search
        .trim_start_matches('?')
        .split('&')
        .filter(|p| !p.is_empty() && *p != "map=full")
        .collect();
    if full {
        parts.push("map=full");
    }
    if parts.is_empty() {
        String::new()
    } else {
        format!("?{}", parts.join("&"))
    }
}

/// Whether a page's query string asks for the full screen map.
pub fn wants_full(search: &str) -> bool {
    search
        .trim_start_matches('?')
        .split('&')
        .any(|p| p == "map=full")
}

/// Where an old layout of `frame` units is drawn inside a `bx` pixel box
/// until the new one arrives: as big as fits, centred, never stretched
/// (left, top, width, height in pixels).
pub fn contain(bx: (f32, f32), frame: (f32, f32)) -> (f32, f32, f32, f32) {
    if frame.0 <= 0.0 || frame.1 <= 0.0 {
        return (0.0, 0.0, bx.0.max(0.0), bx.1.max(0.0));
    }
    let s = (bx.0 / frame.0).min(bx.1 / frame.1).max(0.0);
    let (w, h) = (frame.0 * s, frame.1 * s);
    ((bx.0 - w) / 2.0, (bx.1 - h) / 2.0, w, h)
}

/// Whether a full screen box went from `old` to `new` by more than a tenth
/// in either side, so the map is laid out again (smaller changes, such as
/// a toolbar showing, keep the layout, drawn whole).
pub fn refit_needed(old: (f32, f32), new: (f32, f32)) -> bool {
    let far = |a: f32, b: f32| (a - b).abs() > a.abs().max(1.0) * 0.1;
    far(old.0, new.0) || far(old.1, new.1)
}

/// Tab inside the full screen map: with `len` controls in it and focus on
/// control `at` (`None`: outside it), where focus must go instead of where
/// the browser would take it (`None`: let the browser move it).
pub fn trap_step(len: usize, at: Option<usize>, back: bool) -> Option<usize> {
    if len == 0 {
        return None;
    }
    match at {
        None => Some(if back { len - 1 } else { 0 }),
        Some(0) if back => Some(len - 1),
        Some(i) if !back && i + 1 >= len => Some(0),
        Some(_) => None,
    }
}

/// A label drawn on the map, under its dot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LabelBox {
    pub id: String,
    pub text: String,
    /// The selected dot's.
    pub bold: bool,
    /// 13px rather than 11px.
    pub big: bool,
}

/// A label's font size in pixels.
pub fn label_px(l: &LabelBox) -> u8 {
    if l.big { 13 } else { 11 }
}

/// A label longer than this is cut.
pub const LABEL_MAX: usize = 30;

/// A label's text: names over [`LABEL_MAX`] characters keep the first 28,
/// then "…" (the board's cut).
pub fn label_text(label: &str) -> String {
    if label.chars().count() > LABEL_MAX {
        let head: String = label.chars().take(LABEL_MAX - 2).collect();
        format!("{}\u{2026}", head.trim_end())
    } else {
        label.to_string()
    }
}

/// What the label rule needs of a dot on the map.
#[derive(Debug, Clone, PartialEq)]
pub struct Spot {
    pub id: String,
    pub kind: DotKind,
    pub label: String,
    /// Its radius at 1x, in screen pixels.
    pub r: f32,
}

impl Spot {
    pub fn of(d: &Dot) -> Spot {
        Spot {
            id: d.id.clone(),
            kind: d.kind,
            label: d.label.clone(),
            r: d.r,
        }
    }
}

/// The labels the map shows (the registry's rule; overlaps are not
/// hidden). `focus`: the lit dots when a dot is selected or the find box
/// has words (`None`: nothing is dimmed). A paper is labelled only when it
/// is the selected dot; any other dot when it is lit and it is big (radius
/// [`LABELLED_FROM_R`] or more), or the zoom is [`LABELS_FROM_ZOOM`] or
/// more, or something is in focus, or the map is full screen with at most
/// [`ROOMY_UP_TO`] dots that are not papers. 13px for a disease, the
/// selected dot or anything in full screen; 11px otherwise.
pub fn visible_labels(
    spots: &[Spot],
    zoom: f32,
    sel: Option<&str>,
    focus: Option<&HashSet<String>>,
    full: bool,
) -> Vec<LabelBox> {
    let roomy = full && spots.iter().filter(|d| d.kind != DotKind::Paper).count() <= ROOMY_UP_TO;
    spots
        .iter()
        .filter(|d| {
            let picked = Some(d.id.as_str()) == sel;
            let lit = focus.is_none_or(|f| f.contains(&d.id));
            lit && if d.kind == DotKind::Paper {
                picked
            } else {
                d.r >= LABELLED_FROM_R || zoom >= LABELS_FROM_ZOOM || focus.is_some() || roomy
            }
        })
        .map(|d| {
            let picked = Some(d.id.as_str()) == sel;
            LabelBox {
                id: d.id.clone(),
                text: label_text(&d.label),
                bold: picked,
                big: d.kind == DotKind::Disease || picked || full,
            }
        })
        .collect()
}

/// What to do when the map's filter or frame changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MapFetch {
    /// The wanted map is on screen, or already being fetched.
    Nothing,
    /// The wanted map is on screen but another is being fetched: forget
    /// that fetch (its reply is dropped) and stop showing "Loading".
    Drop,
    /// Fetch the wanted map.
    Ask,
}

/// `on_screen`: the wanted map is the one drawn. `in_flight`: `None` when
/// nothing is being fetched, else whether the fetch is for the wanted map.
pub fn map_fetch(on_screen: bool, in_flight: Option<bool>) -> MapFetch {
    match (on_screen, in_flight) {
        (true, Some(_)) => MapFetch::Drop,
        (true, None) | (false, Some(true)) => MapFetch::Nothing,
        (false, _) => MapFetch::Ask,
    }
}

/// Whether a click on a dot picks it: a keyboard or screen-reader press
/// (`detail` 0) always does; a pointer click does unless the map was just
/// dragged.
pub fn is_tap(moved: bool, detail: i32) -> bool {
    detail == 0 || !moved
}

/// The tapped dot and the dots a line joins it to; empty when it is not
/// on the map.
pub fn near(data: &MapData, id: &str) -> HashSet<String> {
    if dot(data, id).is_none() {
        return HashSet::new();
    }
    let mut out: HashSet<String> = neighbours(data, id)
        .into_iter()
        .map(|d| d.id.clone())
        .collect();
    out.insert(id.to_string());
    out
}

/// The dots the time-lapse shows by the end of `year`: a paper from its
/// year, any other dot from its first paper's year (a dot with no paper
/// year waits for the end). `None`: every dot.
pub fn visible_until(data: &MapData, year: Option<i32>) -> HashSet<String> {
    data.dots
        .iter()
        .filter(|d| year.is_none_or(|y| d.first_year.is_some_and(|f| f <= y)))
        .map(|d| d.id.clone())
        .collect()
}

/// The first and last paper years on the map (the time-lapse's range).
pub fn year_span(data: &MapData) -> Option<(i32, i32)> {
    let years = data
        .dots
        .iter()
        .filter(|d| d.kind == DotKind::Paper)
        .filter_map(|d| d.first_year);
    years.fold(None, |acc, y| match acc {
        None => Some((y, y)),
        Some((lo, hi)) => Some((lo.min(y), hi.max(y))),
    })
}

/// The dots the find box matches: every word in the label, a paper's
/// design, or a question's name and text.
pub fn found_on_map(data: &MapData, query: &str) -> HashSet<String> {
    let words: Vec<String> = query
        .to_lowercase()
        .split_whitespace()
        .map(str::to_string)
        .collect();
    if words.is_empty() {
        return HashSet::new();
    }
    data.dots
        .iter()
        .filter(|d| {
            let mut hay = format!("{} {}", d.label, d.design);
            if d.kind == DotKind::Run {
                hay.push(' ');
                hay.push_str(&run_title(data, d));
                if let Some(q) = &d.question {
                    hay.push(' ');
                    hay.push_str(q);
                }
            }
            let hay = hay.to_lowercase();
            words.iter().all(|w| hay.contains(w.as_str()))
        })
        .map(|d| d.id.clone())
        .collect()
}

/// A kind's name on the map and in the legend.
pub fn kind_name(kind: DotKind) -> &'static str {
    match kind {
        DotKind::Organ => "Organ",
        DotKind::Disease => "Disease",
        DotKind::Run => "Your question",
        DotKind::Treatment => "Treatment",
        DotKind::Outcome => "Outcome",
        DotKind::Paper => "Paper",
    }
}

/// A dot's accessible name: its kind (a paper's rank) and its label.
pub fn dot_aria(data: &MapData, d: &Dot) -> String {
    match d.kind {
        DotKind::Paper => format!("{} paper: {}", rank_info(d.rank.as_deref()).2, d.label),
        DotKind::Run => format!("{}: {}", kind_name(d.kind), run_title(data, d)),
        k => format!("{}: {}", kind_name(k), d.label),
    }
}

/// The class naming a paper's rank colour (`other` when not ranked).
pub fn rank_class(code: Option<&str>) -> &'static str {
    match code {
        Some("rct") => "rct",
        Some("meta") => "meta",
        Some("prospective") => "prospective",
        Some("retrospective") => "retrospective",
        Some("case_series") => "case_series",
        _ => "other",
    }
}

/// The papers of a picked question, or of a disease's questions, ranked
/// (the side column's "Papers, ranked"); none for any other dot.
pub fn papers_for(data: &MapData, id: &str) -> Vec<Row> {
    let Some(d) = dot(data, id) else {
        return Vec::new();
    };
    let runs: Vec<&str> = match d.kind {
        DotKind::Run => vec![d.id.as_str()],
        DotKind::Disease => neighbours(data, id)
            .into_iter()
            .filter(|n| n.kind == DotKind::Run)
            .map(|n| n.id.as_str())
            .collect(),
        _ => return Vec::new(),
    };
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    let mut papers: Vec<&Dot> = runs
        .iter()
        .flat_map(|r| neighbours(data, r))
        .filter(|n| n.kind == DotKind::Paper && seen.insert(n.id.as_str()))
        .collect();
    papers.sort_by(|a, b| paper_order(a, b));
    papers.into_iter().map(|p| row_of(data, p)).collect()
}

/// The legend's entries ("Show on the map"): the key it toggles in
/// `MapFilter::hidden`, its name, its swatch class, and whether it is a
/// paper rank (a square). "Not ranked" hides every unranked code.
pub const LEGEND: [(&str, &str, &str, bool); 11] = [
    ("organ", "Organ", "organ", false),
    ("disease", "Disease", "disease", false),
    ("run", "Your question", "run", false),
    ("treatment", "Treatment", "treatment", false),
    ("outcome", "Outcome", "outcome", false),
    ("rct", "1 RCT", "rct", true),
    ("meta", "2 Meta-analysis", "meta", true),
    ("prospective", "3 Prospective", "prospective", true),
    ("retrospective", "4 Retrospective", "retrospective", true),
    ("case_series", "5 Case series", "case_series", true),
    ("other", "Not ranked", "other", true),
];

/// The rank codes "Not ranked" stands for.
pub const UNRANKED: [&str; 3] = ["guideline", "model", "unclassified"];

/// The `hidden` codes a legend key stands for.
pub fn hidden_codes(key: &str) -> Vec<String> {
    if key == "other" {
        UNRANKED.iter().map(|s| s.to_string()).collect()
    } else {
        vec![key.to_string()]
    }
}

/// The paper year choices: (value, label, from, to).
pub const YEARS: [(&str, &str, Option<i32>, Option<i32>); 4] = [
    ("all", "Any year", None, None),
    ("new", "2024\u{2013}2026", Some(2024), None),
    ("mid", "2019\u{2013}2023", Some(2019), Some(2023)),
    ("old", "Before 2019", None, Some(2018)),
];

/// The run certainty choices: (value, label).
pub const CERTAINTIES: [(&str, &str); 4] = [
    ("all", "Any"),
    ("Moderate", "Moderate"),
    ("Low", "Low"),
    ("Very low", "Very low"),
];

/// The time-lapse button's words.
pub fn play_label(playing: bool, until: Option<i32>, last: Option<i32>) -> &'static str {
    if playing {
        "Pause"
    } else if until.is_some() && until >= last {
        "Play again"
    } else {
        "Time-lapse by paper year"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph_view::{Dot, Line, SheetRef};
    use uuid::Uuid;

    fn dot(id: &str, kind: DotKind, label: &str, sub: &str) -> Dot {
        Dot {
            id: id.into(),
            kind,
            label: label.into(),
            sub: sub.into(),
            first_year: None,
            r: 5.0,
            rank: None,
            x: 0.0,
            y: 0.0,
            run_id: None,
            concept_id: None,
            pmid: None,
            retracted: false,
            design: String::new(),
            n: None,
            includes_non_rct: false,
            sheet: None,
            question: None,
            bottom_line: None,
            certainty: None,
        }
    }

    #[test]
    fn only_disease_and_treatment_cards_offer_ask_about_this() {
        let mut m = MapData::default();
        for (id, kind, cid) in [
            ("d:7", DotKind::Disease, 7),
            ("t:8", DotKind::Treatment, 8),
            ("g:9", DotKind::Organ, 9),
            ("o:4", DotKind::Outcome, 4),
        ] {
            let mut d = dot(id, kind, "Some name", "");
            d.concept_id = Some(cid);
            m.dots.push(d);
        }
        let ask = |id: &str| card_for(&m, id).unwrap().ask;
        assert_eq!(ask("d:7").as_deref(), Some("/?topic=7"));
        assert_eq!(ask("t:8").as_deref(), Some("/?topic=8"));
        assert_eq!(ask("g:9"), None);
        assert_eq!(ask("o:4"), None);
    }

    fn paper(pmid: &str, rank: &str, year: Option<i32>) -> Dot {
        let mut d = dot(
            &format!("p:{pmid}"),
            DotKind::Paper,
            &format!("Paper {pmid}"),
            "",
        );
        d.rank = Some(rank.into());
        d.first_year = year;
        d.pmid = Some(pmid.into());
        d
    }

    fn line(a: &str, b: &str, paper: bool) -> Line {
        Line {
            a: a.into(),
            b: b.into(),
            paper,
            out: false,
        }
    }

    const RUN: &str = "r:00000000-0000-0000-0000-000000000001";

    fn sample() -> MapData {
        let mut run = dot(
            RUN,
            DotKind::Run,
            "Your question",
            "Full \u{b7} 31 Oct 2026",
        );
        run.run_id = Some(Uuid::from_u128(1));
        let mut meta = paper("200", "meta", Some(2025));
        meta.design = "Meta-analysis of randomised and non-randomised studies".into();
        meta.n = Some(143717);
        meta.includes_non_rct = true;
        meta.sheet = Some(SheetRef {
            run: Uuid::from_u128(1),
            n: 3,
        });
        let mut retracted = paper("300", "retrospective", Some(2020));
        retracted.retracted = true;
        MapData {
            dots: vec![
                dot("g:1", DotKind::Organ, "Abdominal wall", ""),
                dot("d:2", DotKind::Disease, "Inguinal hernia", "Abdominal wall"),
                dot("t:3", DotKind::Treatment, "Robotic surgery", ""),
                dot("o:4", DotKind::Outcome, "Recurrence", "Low"),
                run,
                paper("100", "unclassified", Some(2026)),
                paper("101", "rct", Some(2019)),
                paper("102", "rct", Some(2023)),
                paper("103", "case_series", Some(2024)),
                paper("104", "prospective", None),
                meta,
                retracted,
            ],
            lines: vec![
                line("g:1", "d:2", false),
                line("d:2", RUN, false),
                line(RUN, "t:3", false),
                line(RUN, "o:4", false),
                line(RUN, "p:101", true),
                line(RUN, "p:200", true),
            ],
            papers_hidden_by_size: false,
            runs: 1,
            top_papers: None,
        }
    }

    #[test]
    fn fold_shows_five_then_a_button() {
        let six: Vec<u32> = (1..=6).collect();
        let (shown, button) = fold(&six, false);
        assert_eq!(shown, vec![1, 2, 3, 4, 5]);
        assert_eq!(button.as_deref(), Some("Show all 6"));
        let (shown, button) = fold(&six, true);
        assert_eq!(shown.len(), 6);
        assert_eq!(button.as_deref(), Some("Show fewer"));
        let five: Vec<u32> = (1..=5).collect();
        assert_eq!(fold(&five, false), (five.clone(), None));
        assert_eq!(fold(&five, true), (five, None));
        assert_eq!(FOLD_AT, 5);
    }

    #[test]
    fn groups_come_in_order_and_papers_by_rank_then_newest() {
        let g = groups(&sample());
        let names: Vec<&str> = g.iter().map(|g| g.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "Diseases",
                "Treatments",
                "Outcomes",
                "Your questions",
                "Papers, ranked"
            ]
        );
        let papers: Vec<&str> = g[4].rows.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(
            papers,
            [
                "p:102", "p:101", "p:200", "p:104", "p:300", "p:103", "p:100"
            ]
        );
        assert_eq!(g[4].rows[0].sub, "Rank 1 \u{b7} RCT \u{b7} 2023");
        assert_eq!(
            g[4].rows[6].sub,
            "Rank \u{2013} \u{b7} Not ranked \u{b7} 2026"
        );
        assert_eq!(g[3].rows[0].title, "Inguinal hernia question");
        assert_eq!(g[3].rows[0].sub, "Full run \u{b7} 31 Oct 2026");
        assert_eq!(g[1].rows[0].sub, "1 run");
        assert_eq!(g[2].rows[0].sub, "Certainty: Low");
        // Organs are the chips, not a group; an empty kind has no group.
        let mut only = sample();
        only.dots.retain(|d| d.kind != DotKind::Treatment);
        assert!(groups(&only).iter().all(|g| g.name != "Treatments"));
    }

    #[test]
    fn a_papers_card_names_retraction_non_rct_studies_and_its_sheet() {
        let m = sample();
        let c = card_for(&m, "p:300").unwrap();
        assert!(c.flags.iter().any(|f| f == "Retracted"), "{c:?}");
        let c = card_for(&m, "p:200").unwrap();
        assert_eq!(c.kicker, "Paper");
        assert!(c.flags.iter().any(|f| f == "includes non-RCT studies"));
        assert!(!c.flags.iter().any(|f| f == "Retracted"));
        assert!(c.body.contains("n=1,43,717"), "{}", c.body);
        assert_eq!(
            c.sheet.as_deref(),
            Some("/runs/00000000-0000-0000-0000-000000000001#ref-3")
        );
        assert_eq!(
            c.pubmed.as_deref(),
            Some("https://pubmed.ncbi.nlm.nih.gov/200/")
        );
        // A question and an outcome open the run.
        let run = card_for(&m, RUN).unwrap();
        assert_eq!(
            run.answer.as_deref(),
            Some("/runs/00000000-0000-0000-0000-000000000001")
        );
        assert_eq!(card_for(&m, "o:4").unwrap().answer, run.answer);
        assert!(card_for(&m, "x:9").is_none());
    }

    #[test]
    fn no_room_for_papers_hides_them_rather_than_showing_the_top_zero() {
        let mut m = sample();
        for i in 0..10 {
            m.dots
                .push(dot(&format!("t:9{i}"), DotKind::Treatment, "T", ""));
        }
        // 15 dots that are not papers, a cap of 12.
        cap_for_map(&mut m, 12);
        assert_eq!(m.dots.len(), 12);
        assert!(m.dots.iter().all(|d| d.kind != DotKind::Paper));
        assert_eq!(m.top_papers, None);
        assert!(m.papers_hidden_by_size);
        assert_eq!(
            map_note(&m).as_deref(),
            Some("Papers are hidden on this map; see them in List")
        );
        assert_eq!(map_note(&sample()), None);
    }

    #[test]
    fn numbers_use_indian_grouping() {
        assert_eq!(indian_number(7), "7");
        assert_eq!(indian_number(999), "999");
        assert_eq!(indian_number(1000), "1,000");
        assert_eq!(indian_number(143717), "1,43,717");
        assert_eq!(indian_number(4040555), "40,40,555");
    }

    #[test]
    fn the_find_box_needs_every_word() {
        let g = groups(&sample());
        let found = find_in(&g, "INGUINAL question");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].name, "Your questions");
        assert!(find_in(&g, "nothing here").is_empty());
        assert_eq!(find_in(&g, "  ").len(), g.len());
    }

    #[test]
    fn a_big_map_keeps_the_top_papers() {
        let mut m = sample();
        for i in 0..500 {
            let rank = if i % 2 == 0 { "rct" } else { "retrospective" };
            m.dots
                .push(paper(&format!("9{i:04}"), rank, Some(2000 + i % 20)));
            m.lines.push(line(RUN, &format!("p:9{i:04}"), true));
        }
        cap_for_map(&mut m, 400);
        assert_eq!(m.dots.len(), 400);
        let papers: Vec<_> = m.dots.iter().filter(|d| d.kind == DotKind::Paper).collect();
        assert_eq!(m.top_papers, Some(papers.len() as u32));
        assert_eq!(papers.len(), 395);
        // Every RCT (2 + 250), the meta-analysis and the prospective study,
        // then the newest retrospective ones.
        let of = |r: &str| {
            papers
                .iter()
                .filter(|d| d.rank.as_deref() == Some(r))
                .count()
        };
        assert_eq!((of("rct"), of("meta"), of("prospective")), (252, 1, 1));
        assert_eq!(of("retrospective"), 141);
        let oldest_kept = papers
            .iter()
            .filter(|d| d.rank.as_deref() == Some("retrospective"))
            .filter_map(|d| d.first_year)
            .min()
            .unwrap();
        assert!(
            m.dots
                .iter()
                .all(|d| d.kind != DotKind::Paper || d.rank.as_deref() != Some("case_series"))
        );
        assert_eq!(oldest_kept, 2009);
        let present: std::collections::HashSet<&str> =
            m.dots.iter().map(|d| d.id.as_str()).collect();
        assert!(
            m.lines
                .iter()
                .all(|l| present.contains(l.a.as_str()) && present.contains(l.b.as_str()))
        );
        assert_eq!(top_papers_line(395), "Showing the top 395 papers");
        assert_eq!(map_note(&m).as_deref(), Some("Showing the top 395 papers"));
        // A small map is left alone.
        let mut small = sample();
        cap_for_map(&mut small, 400);
        assert_eq!(small, sample());
    }

    // ---------- the map ----------

    fn ids(labels: &[LabelBox]) -> Vec<&str> {
        let mut v: Vec<&str> = labels.iter().map(|l| l.id.as_str()).collect();
        v.sort_unstable();
        v
    }

    fn spots(dots: &[Dot]) -> Vec<Spot> {
        dots.iter().map(Spot::of).collect()
    }

    fn outcome() -> Dot {
        let mut d = dot("o:1", DotKind::Outcome, "Recurrence", "");
        d.r = 8.0;
        d
    }

    fn big(id: &str, kind: DotKind, r: f32) -> Dot {
        let mut d = dot(id, kind, "Some name", "");
        d.r = r;
        d
    }

    #[test]
    fn a_papers_label_shows_only_when_it_is_selected() {
        let mut p = paper("1", "rct", Some(2020));
        p.r = 8.0;
        let s = spots(&[p]);
        let focus: HashSet<String> = ["p:1".to_string()].into();
        // Not at any zoom, not in a roomy full screen, not when found.
        assert!(visible_labels(&s, 10.0, None, None, true).is_empty());
        assert!(visible_labels(&s, 1.0, None, Some(&focus), false).is_empty());
        let got = visible_labels(&s, 1.0, Some("p:1"), Some(&focus), false);
        assert_eq!(ids(&got), ["p:1"]);
        assert!(got[0].bold && got[0].big, "{got:?}");
    }

    #[test]
    fn big_kinds_are_labelled_and_small_ones_from_one_point_six() {
        let dots = [
            big("d:1", DotKind::Disease, 15.0),
            big("g:1", DotKind::Organ, 12.0),
            big("t:1", DotKind::Treatment, 11.0),
            big(RUN, DotKind::Run, 9.0),
            outcome(),
        ];
        let s = spots(&dots);
        assert_eq!(
            ids(&visible_labels(&s, 1.0, None, None, false)),
            ["d:1", "g:1", "t:1"]
        );
        assert_eq!(
            ids(&visible_labels(&s, 1.5, None, None, false)),
            ["d:1", "g:1", "t:1"]
        );
        assert_eq!(
            ids(&visible_labels(&s, 1.6, None, None, false)),
            ["d:1", "g:1", "o:1", RUN, "t:1"]
        );
        // Overlapping labels are not hidden (the registry's rule).
        let mut close = dots.clone();
        for d in &mut close {
            d.x = 100.0;
            d.y = 100.0;
        }
        assert_eq!(
            visible_labels(&spots(&close), 2.0, None, None, false).len(),
            5
        );
    }

    #[test]
    fn a_selection_or_a_search_labels_every_lit_dot_and_never_a_dimmed_one() {
        let dots = [
            big("d:1", DotKind::Disease, 15.0),
            outcome(),
            big(RUN, DotKind::Run, 9.0),
        ];
        let s = spots(&dots);
        let lit: HashSet<String> = [RUN.to_string(), "o:1".to_string()].into();
        // Picked: the run and its outcome; the dimmed disease has none.
        assert_eq!(
            ids(&visible_labels(&s, 1.0, Some(RUN), Some(&lit), false)),
            ["o:1", RUN]
        );
        // A search with the same matches.
        assert_eq!(
            ids(&visible_labels(&s, 1.0, None, Some(&lit), false)),
            ["o:1", RUN]
        );
        // Dimmed at any zoom and in full screen.
        let only: HashSet<String> = ["o:1".to_string()].into();
        assert_eq!(
            ids(&visible_labels(&s, 10.0, None, Some(&only), true)),
            ["o:1"]
        );
    }

    #[test]
    fn a_roomy_full_screen_labels_every_dot_up_to_forty() {
        let roomy = |n: usize| -> Vec<Dot> {
            let mut v = vec![outcome()];
            for i in 1..n {
                v.push(big(&format!("t:{i}"), DotKind::Treatment, 11.0));
            }
            // Papers do not count.
            for i in 0..30 {
                v.push(paper(&format!("{i}"), "rct", Some(2020)));
            }
            v
        };
        let forty = spots(&roomy(40));
        assert!(ids(&visible_labels(&forty, 1.0, None, None, true)).contains(&"o:1"));
        assert!(!ids(&visible_labels(&forty, 1.0, None, None, false)).contains(&"o:1"));
        let forty_one = spots(&roomy(41));
        assert!(!ids(&visible_labels(&forty_one, 1.0, None, None, true)).contains(&"o:1"));
    }

    #[test]
    fn label_size_and_weight() {
        let dots = [
            big("d:1", DotKind::Disease, 15.0),
            big("t:1", DotKind::Treatment, 11.0),
        ];
        let s = spots(&dots);
        let got = visible_labels(&s, 1.0, None, None, false);
        let of = |id: &str, got: &[LabelBox]| got.iter().find(|l| l.id == id).cloned().unwrap();
        // 13px for a disease, 11px otherwise; bold only when selected.
        assert!(of("d:1", &got).big && !of("d:1", &got).bold);
        assert!(!of("t:1", &got).big);
        assert_eq!(label_px(&of("d:1", &got)), 13);
        assert_eq!(label_px(&of("t:1", &got)), 11);
        // Everything is 13px in full screen.
        let full = visible_labels(&s, 1.0, None, None, true);
        assert!(full.iter().all(|l| l.big));
        let lit: HashSet<String> = ["t:1".to_string(), "d:1".to_string()].into();
        let picked = visible_labels(&s, 1.0, Some("t:1"), Some(&lit), false);
        assert!(of("t:1", &picked).big && of("t:1", &picked).bold);
    }

    #[test]
    fn long_names_are_cut_at_thirty_characters() {
        let long = "Laparoscopic Heller myotomy with Dor fundoplication";
        let thirty = "Robotic transabdominal repair!";
        assert_eq!(thirty.chars().count(), 30);
        let mut a = big("d:1", DotKind::Disease, 15.0);
        a.label = long.into();
        let mut b = big("d:2", DotKind::Disease, 15.0);
        b.label = thirty.into();
        let got = visible_labels(&spots(&[a, b]), 1.0, None, None, false);
        let text = |id: &str| got.iter().find(|l| l.id == id).unwrap().text.clone();
        // The board's cut: 28 characters, then "…" (a trailing space dropped).
        assert_eq!(text("d:1"), "Laparoscopic Heller myotomy\u{2026}");
        assert!(text("d:1").chars().count() <= 30);
        assert_eq!(text("d:2"), thirty);
    }

    fn close(a: (f32, f32, f32), b: (f32, f32, f32)) -> bool {
        (a.0 - b.0).abs() < 1e-3 && (a.1 - b.1).abs() < 1e-3 && (a.2 - b.2).abs() < 1e-3
    }

    #[test]
    fn the_registrys_zoom_rules() {
        assert_eq!((ZOOM_MIN, ZOOM_MAX), (0.5, 10.0));
        assert_eq!(ZOOM_STEP, 1.3);
        assert_eq!(WHEEL_STEP, 1.15);
        // About a point: the point stays where it is.
        let v = zoom_about((1.0, 0.0, 0.0), (100.0, 50.0), 2.0);
        assert!(close(v, (2.0, -100.0, -50.0)), "{v:?}");
        let (k, tx, ty) = v;
        assert!(((100.0 - tx) / k - 100.0).abs() < 1e-3 && ((50.0 - ty) / k - 50.0).abs() < 1e-3);
        // Clamped to 0.5..10, still about the point.
        let v = zoom_about((8.0, 0.0, 0.0), (0.0, 0.0), 2.0);
        assert!(close(v, (10.0, 0.0, 0.0)), "{v:?}");
        let v = zoom_about((0.6, 10.0, 10.0), (10.0, 10.0), 0.5);
        assert!(close(v, (0.5, 10.0, 10.0)), "{v:?}");
        // A pinch: the point under the fingers stays under them.
        let v = pinch_view((1.0, 0.0, 0.0), 100.0, (50.0, 50.0), 200.0, (60.0, 50.0));
        assert!(close(v, (2.0, -40.0, -50.0)), "{v:?}");
        let v = pinch_view((5.0, 0.0, 0.0), 10.0, (0.0, 0.0), 1000.0, (0.0, 0.0));
        assert_eq!(v.0, 10.0);
        // A dot's place on screen, with its dragged offset.
        assert_eq!(
            screen_at((10.0, 20.0), (2.0, -4.0), (2.0, 5.0, 6.0)),
            (29.0, 38.0)
        );
    }

    #[test]
    fn dots_grow_a_little_with_the_zoom() {
        assert_eq!(zoom_radius(10.0, 1.0), 10.0);
        assert!((zoom_radius(10.0, 2.0) - 10.0 * 2f32.powf(0.2)).abs() < 1e-4);
        assert_eq!(zoom_radius(10.0, 10.0), 15.0);
        assert!((zoom_radius(10.0, 0.5) - 10.0 * 0.5f32.powf(0.2)).abs() < 1e-4);
        assert_eq!(zoom_radius(10.0, 0.1), 8.0);
        // A tap target is at least 30px.
        assert_eq!(hit_size(4.5), 30.0);
        assert_eq!(hit_size(15.0), 38.0);
    }

    #[test]
    fn ten_pixels_make_a_drag() {
        assert!(!is_drag(6.0, 7.9));
        assert!(is_drag(6.0, 8.0));
        assert!(is_drag(-10.0, 0.0));
        assert!(!is_drag(0.0, 9.99));
    }

    #[test]
    fn full_screen_is_in_the_url_as_map_full_only() {
        assert_eq!(map_query("", true), "?map=full");
        assert_eq!(map_query("?a=1", true), "?a=1&map=full");
        assert_eq!(map_query("?map=full", true), "?map=full");
        assert_eq!(map_query("?a=1&map=full", false), "?a=1");
        assert_eq!(map_query("?map=full&a=1", false), "?a=1");
        assert_eq!(map_query("?map=full", false), "");
        assert_eq!(map_query("", false), "");
        assert!(wants_full("?map=full"));
        assert!(wants_full("?a=1&map=full"));
        assert!(!wants_full("?map=fullish"));
        assert!(!wants_full(""));
    }

    #[test]
    fn a_papers_card_has_its_rank_chip_pmid_and_questions() {
        let m = sample();
        let c = card_for(&m, "p:200").unwrap();
        assert_eq!(c.kicker, "Paper");
        assert_eq!(c.chip, Some(("2 \u{b7} Meta-analysis".to_string(), "meta")));
        assert_eq!(
            c.body,
            "Meta-analysis of randomised and non-randomised studies \u{b7} 2025 \u{b7} n=1,43,717 \u{b7} PMID 200"
        );
        assert_eq!(
            c.from,
            [(RUN.to_string(), "Inguinal hernia question".to_string())]
        );
        let c = card_for(&m, "p:101").unwrap();
        assert_eq!(c.chip, Some(("1 \u{b7} RCT".to_string(), "rct")));
        assert_eq!(c.body, "2019 \u{b7} PMID 101");
        // Not ranked: its label alone; no question linked: none named.
        let c = card_for(&m, "p:100").unwrap();
        assert_eq!(c.chip, Some(("Not ranked".to_string(), "other")));
        assert!(c.from.is_empty());
        // Only papers have a chip.
        assert_eq!(card_for(&m, "d:2").unwrap().chip, None);
    }

    #[test]
    fn a_papers_questions_are_named_once() {
        // A second run on the same disease cites the same paper: both are
        // "Inguinal hernia question", named once (the first run's).
        let mut m = sample();
        let two = "r:00000000-0000-0000-0000-000000000002";
        let mut run = dot(two, DotKind::Run, "Your question", "Full \u{b7} 1 Nov 2026");
        run.run_id = Some(Uuid::from_u128(2));
        m.dots.push(run);
        m.lines.push(line("d:2", two, false));
        m.lines.push(line(two, "p:200", true));
        let c = card_for(&m, "p:200").unwrap();
        assert_eq!(
            c.from,
            [(RUN.to_string(), "Inguinal hernia question".to_string())]
        );
        assert!(c.links.is_empty());
    }

    #[test]
    fn tab_stays_inside_the_full_screen_map() {
        // Forward off the last control goes to the first, back off the
        // first to the last; elsewhere the browser moves focus.
        assert_eq!(trap_step(5, Some(4), false), Some(0));
        assert_eq!(trap_step(5, Some(0), true), Some(4));
        assert_eq!(trap_step(5, Some(2), false), None);
        assert_eq!(trap_step(5, Some(2), true), None);
        // Focus outside the dialog comes in at its first (or last) control.
        assert_eq!(trap_step(5, None, false), Some(0));
        assert_eq!(trap_step(5, None, true), Some(4));
        assert_eq!(trap_step(0, None, false), None);
    }

    #[test]
    fn an_old_layout_is_drawn_whole_in_a_new_box() {
        // Same shape: the whole box.
        assert_eq!(
            contain((358.0, 480.0), (358.0, 480.0)),
            (0.0, 0.0, 358.0, 480.0)
        );
        assert_eq!(
            contain((716.0, 960.0), (358.0, 480.0)),
            (0.0, 0.0, 716.0, 960.0)
        );
        // A taller box: full width, centred (no stretch).
        let (l, t, w, h) = contain((358.0, 700.0), (358.0, 480.0));
        assert_eq!((l, w, h), (0.0, 358.0, 480.0));
        assert_eq!(t, 110.0);
        // A wider box: full height, centred.
        let (l, t, w, h) = contain((1000.0, 480.0), (358.0, 480.0));
        assert_eq!((t, w, h), (0.0, 358.0, 480.0));
        assert_eq!(l, 321.0);
        // Nothing measured yet: the box as it is.
        assert_eq!(contain((0.0, 0.0), (358.0, 480.0)), (0.0, 0.0, 0.0, 0.0));
    }

    #[test]
    fn full_screen_refits_only_for_a_real_change() {
        assert!(!refit_needed((390.0, 700.0), (390.0, 700.0)));
        assert!(!refit_needed((390.0, 700.0), (395.0, 690.0)));
        assert!(refit_needed((390.0, 700.0), (700.0, 390.0)));
        assert!(refit_needed((390.0, 700.0), (390.0, 600.0)));
    }

    /// The page's fetch bookkeeping, as `explore.rs` keeps it.
    struct Fetches {
        shown: char,
        asking: Option<char>,
        busy: bool,
    }

    impl Fetches {
        fn want(&mut self, k: char) {
            let in_flight = self.asking.map(|a| a == k);
            match map_fetch(self.shown == k, in_flight) {
                MapFetch::Nothing => {}
                MapFetch::Drop => {
                    self.asking = None;
                    self.busy = false;
                }
                MapFetch::Ask => {
                    self.asking = Some(k);
                    self.busy = true;
                }
            }
        }

        /// A reply for `k` arrives: drawn only if it is still the one asked.
        fn reply(&mut self, k: char) -> bool {
            if self.asking != Some(k) {
                return false;
            }
            self.asking = None;
            self.busy = false;
            self.shown = k;
            true
        }
    }

    #[test]
    fn a_filter_changed_and_changed_back_keeps_its_map() {
        let mut f = Fetches {
            shown: 'A',
            asking: None,
            busy: false,
        };
        f.want('A');
        assert_eq!((f.asking, f.busy), (None, false), "already on screen");
        f.want('B');
        assert_eq!((f.asking, f.busy), (Some('B'), true));
        f.want('B');
        assert_eq!(f.asking, Some('B'), "not asked twice");
        // Back to A while B loads: B is forgotten.
        f.want('A');
        assert_eq!((f.asking, f.busy), (None, false));
        assert!(!f.reply('B'), "B's late reply is not drawn under A");
        assert_eq!(f.shown, 'A');
        // A then C: C is drawn.
        f.want('C');
        assert!(f.reply('C'));
        assert_eq!((f.shown, f.busy), ('C', false));
        // A newer request replaces an older one.
        f.want('D');
        f.want('E');
        assert!(!f.reply('D'));
        assert!(f.reply('E'));
    }

    #[test]
    fn a_keyboard_press_always_picks_a_dot() {
        assert!(is_tap(false, 1));
        assert!(!is_tap(true, 1), "the end of a drag is not a tap");
        assert!(is_tap(true, 0), "Enter or Space after a drag still picks");
        assert!(is_tap(false, 0));
    }

    #[test]
    fn near_is_the_tapped_dot_and_its_neighbours() {
        let m = sample();
        let got = near(&m, RUN);
        let want: HashSet<String> = [RUN, "d:2", "t:3", "o:4", "p:101", "p:200"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(got, want);
        let got = near(&m, "g:1");
        assert_eq!(got, ["g:1", "d:2"].iter().map(|s| s.to_string()).collect());
        assert!(near(&m, "x:9").is_empty());
    }

    #[test]
    fn the_time_lapse_hides_later_papers_and_concepts() {
        let mut m = sample();
        for d in &mut m.dots {
            match d.id.as_str() {
                "d:2" | "g:1" => d.first_year = Some(2019),
                "t:3" => d.first_year = Some(2023),
                _ => {}
            }
        }
        let all: HashSet<String> = m.dots.iter().map(|d| d.id.clone()).collect();
        assert_eq!(visible_until(&m, None), all);
        let shown = visible_until(&m, Some(2020));
        for id in ["d:2", "g:1", "p:101"] {
            assert!(shown.contains(id), "{id} by 2020");
        }
        // A paper after the year, a concept whose first paper is later, and
        // one with no paper year.
        for id in ["p:102", "p:200", "t:3", "o:4", "p:104"] {
            assert!(!shown.contains(id), "{id} by 2020");
        }
        assert!(visible_until(&m, Some(2026)).contains("p:100"));
    }

    #[test]
    fn the_frame_follows_the_window_width() {
        assert_eq!(frame_for(390.0), PHONE_FRAME);
        assert_eq!(frame_for(699.0), PHONE_FRAME);
        assert_eq!(frame_for(700.0), DESKTOP_FRAME);
        assert_eq!(frame_for(1280.0), DESKTOP_FRAME);
        assert_eq!(
            frame_for_agent(Some("Mozilla/5.0 (iPhone) Mobile/15E148")),
            PHONE_FRAME
        );
        assert_eq!(
            frame_for_agent(Some("Mozilla/5.0 (X11; Linux x86_64)")),
            DESKTOP_FRAME
        );
        assert_eq!(frame_for_agent(None), DESKTOP_FRAME);
    }

    #[test]
    fn a_dot_is_named_by_its_kind_and_label() {
        let m = sample();
        let name = |id: &str| dot_aria(&m, m.dots.iter().find(|d| d.id == id).unwrap());
        assert_eq!(name("d:2"), "Disease: Inguinal hernia");
        assert_eq!(name("g:1"), "Organ: Abdominal wall");
        assert_eq!(name("p:101"), "RCT paper: Paper 101");
        assert_eq!(name("p:100"), "Not ranked paper: Paper 100");
        assert_eq!(name(RUN), "Your question: Inguinal hernia question");
    }

    #[test]
    fn a_question_card_shows_the_question_and_its_bottom_line() {
        let mut m = sample();
        for d in &mut m.dots {
            if d.id == RUN {
                d.question = Some("Is robotic repair better?".into());
                d.bottom_line = Some("No consistent benefit.".into());
                d.certainty = Some("Low".into());
            }
            if d.id == "d:2" {
                d.certainty = Some("Low".into());
            }
        }
        let c = card_for(&m, RUN).unwrap();
        assert_eq!(
            c.kicker,
            "Full run \u{b7} 31 Oct 2026 \u{b7} Certainty: Low"
        );
        assert_eq!(c.title, "Is robotic repair better?");
        assert_eq!(c.body, "No consistent benefit.");
        assert!(c.answer.is_some());
        let g = groups(&m);
        assert_eq!(g[0].rows[0].sub, "Abdominal wall \u{b7} Certainty: Low");
        // Without a stored answer the card still names the question.
        let c = card_for(&sample(), RUN).unwrap();
        assert_eq!(c.title, "Inguinal hernia question");
        assert_eq!(c.kicker, "Full run \u{b7} 31 Oct 2026");
    }

    #[test]
    fn a_question_or_disease_lists_its_papers_by_rank() {
        let m = sample();
        let rows = papers_for(&m, RUN);
        let got: Vec<&str> = rows.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(got, ["p:101", "p:200"]);
        assert_eq!(
            papers_for(&m, "d:2")
                .iter()
                .map(|r| r.id.as_str())
                .collect::<Vec<_>>(),
            ["p:101", "p:200"]
        );
        assert!(papers_for(&m, "t:3").is_empty());
    }
}
