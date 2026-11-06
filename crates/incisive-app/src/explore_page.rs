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

/// Where this browser keeps its Map/List choice.
pub const VIEW_STORAGE_KEY: &str = "incisive.explore.view";

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
            let (_, num, name) = rank_info(d.rank.as_deref());
            c.kicker = format!("Paper \u{b7} Rank {num} \u{b7} {name}");
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
            c.body = body.join(" \u{b7} ");
            if d.retracted {
                c.flags.push("Retracted".into());
            }
            if d.includes_non_rct {
                c.flags.push("includes non-RCT studies".into());
            }
            c.sheet = d.sheet.map(|s| format!("{}#ref-{}", run_href(s.run), s.n));
            c.pubmed = d
                .pmid
                .as_deref()
                .filter(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
                .map(|p| format!("https://pubmed.ncbi.nlm.nih.gov/{p}/"));
        }
        DotKind::Run => {
            c.kicker = run_sub(d);
            c.title = run_title(data, d);
            c.body = linked(near.len());
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
        }
        DotKind::Disease | DotKind::Organ => {
            c.kicker = if d.kind == DotKind::Disease {
                "Disease"
            } else {
                "Organ"
            }
            .into();
            c.body = linked(near.len());
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
    if had_papers {
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

/// "12 dots · 14 lines" under the map.
pub fn count_line(data: &MapData) -> String {
    let (d, l) = (data.dots.len(), data.lines.len());
    format!(
        "{d} {} \u{b7} {l} {}",
        if d == 1 { "dot" } else { "dots" },
        if l == 1 { "line" } else { "lines" }
    )
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
        }
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
        assert_eq!(c.kicker, "Paper \u{b7} Rank 2 \u{b7} Meta-analysis");
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
        // A small map is left alone.
        let mut small = sample();
        cap_for_map(&mut small, 400);
        assert_eq!(small, sample());
    }
}
