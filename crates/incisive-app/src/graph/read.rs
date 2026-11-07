//! Reads one user's map. Every query is filtered by the user passed in, on
//! the link rows and on the run they hang from; concepts and papers are only
//! reached through those links. Names are never logged.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use sqlx::PgPool;
use uuid::Uuid;

use super::layout::{concept_radius, paper_radius};
use super::store::MAX_CHAIN;
use crate::graph_view::{Dot, DotKind, Line, MapData, MapFilter, SheetRef};

/// Above this many dots, papers are left out unless the user chose what to
/// hide.
pub const MAX_DOTS: usize = 400;

/// Rank codes, strongest first (the `rank` SQL check's values).
const RANK_ORDER: [&str; 8] = [
    "rct",
    "meta",
    "prospective",
    "retrospective",
    "case_series",
    "guideline",
    "model",
    "unclassified",
];

/// The bottom-line certainties an answer can state.
const CERTAINTIES: [&str; 4] = ["High", "Moderate", "Low", "Very low"];

fn rank_order(code: &str) -> usize {
    RANK_ORDER
        .iter()
        .position(|r| *r == code)
        .unwrap_or(RANK_ORDER.len())
}

struct RunRow {
    id: Uuid,
    mode: String,
    day: String,
    /// The user's own question; never logged.
    question: String,
    bottom_line: Option<String>,
    certainty: Option<String>,
}

struct ConceptLink {
    run: Uuid,
    id: i64,
    kind: DotKind,
    label: String,
    certainty: Option<String>,
    /// An outcome's row in its run's stored answer, 1-based (as the
    /// extract step numbers the rows).
    source_row: Option<i32>,
}

#[derive(sqlx::FromRow)]
struct PaperLink {
    run: Uuid,
    pmid: String,
    title: String,
    year: Option<i32>,
    retracted: bool,
    rank: String,
    rank_label: String,
    includes_non_rct: bool,
    design: String,
    n: Option<i64>,
}

fn concept_kind(kind: &str) -> Option<DotKind> {
    match kind {
        "organ" => Some(DotKind::Organ),
        "disease" => Some(DotKind::Disease),
        "treatment" => Some(DotKind::Treatment),
        "outcome" => Some(DotKind::Outcome),
        _ => None,
    }
}

fn concept_dot_id(kind: DotKind, id: i64) -> String {
    let p = match kind {
        DotKind::Organ => "g",
        DotKind::Disease => "d",
        DotKind::Treatment => "t",
        _ => "o",
    };
    format!("{p}:{id}")
}

fn run_dot_id(id: Uuid) -> String {
    format!("r:{id}")
}

fn paper_dot_id(pmid: &str) -> String {
    format!("p:{pmid}")
}

/// The user's runs that have any link, newest first, each with its
/// question, and its stored bottom line and certainty when it has an
/// answer (version 3; version 2 has no certainty).
async fn user_runs(pool: &PgPool, user_id: i64) -> sqlx::Result<Vec<RunRow>> {
    type Row = (Uuid, String, String, String, Option<String>, Option<String>);
    let rows: Vec<Row> = sqlx::query_as(
        "SELECT r.id, r.mode, \
                to_char(r.created_at AT TIME ZONE 'Asia/Kolkata', 'FMDD Mon YYYY'), \
                r.question, \
                CASE WHEN a.answer_version = 3 \
                     THEN a.body #>> '{answer,bottom_line,text}' \
                     ELSE a.body #>> '{bottom_line,0,text}' END, \
                CASE WHEN a.answer_version = 3 \
                     THEN a.body #>> '{answer,bottom_line,certainty}' END \
           FROM incisive.runs r \
           LEFT JOIN incisive.run_answers a ON a.run_id = r.id \
          WHERE r.user_id = $1 \
            AND (EXISTS (SELECT 1 FROM incisive.graph_run_concepts rc \
                          WHERE rc.run_id = r.id AND rc.user_id = $1) \
              OR EXISTS (SELECT 1 FROM incisive.graph_run_papers rp \
                          WHERE rp.run_id = r.id AND rp.user_id = $1)) \
          ORDER BY r.created_at DESC, r.id",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(id, mode, day, question, bottom_line, certainty)| RunRow {
            id,
            mode,
            day,
            question,
            // As stored, citations and all; the page strips them.
            bottom_line: bottom_line.filter(|b| !b.trim().is_empty()),
            certainty: certainty
                .map(|c| c.trim().to_string())
                .filter(|c| CERTAINTIES.contains(&c.as_str())),
        })
        .collect())
}

/// The user's concept links, each resolved to the concept it was merged
/// into.
async fn concept_links(pool: &PgPool, user_id: i64) -> sqlx::Result<Vec<ConceptLink>> {
    type Row = (Uuid, i64, String, String, Option<String>, Option<i32>);
    let rows: Vec<Row> = sqlx::query_as(
        "WITH RECURSIVE walk(start, id, merged_into, d) AS ( \
           SELECT c.id, c.id, c.merged_into, 0 FROM incisive.graph_concepts c \
            WHERE c.id IN (SELECT concept_id FROM incisive.graph_run_concepts WHERE user_id = $1) \
           UNION ALL \
           SELECT w.start, c.id, c.merged_into, w.d + 1 FROM walk w \
             JOIN incisive.graph_concepts c ON c.id = w.merged_into WHERE w.d < $2), \
         fin AS (SELECT DISTINCT ON (start) start, id FROM walk ORDER BY start, d DESC) \
         SELECT rc.run_id, c.id, c.kind, c.label, rc.certainty, rc.source_row \
           FROM incisive.graph_run_concepts rc \
           JOIN incisive.runs r ON r.id = rc.run_id AND r.user_id = $1 \
           JOIN fin ON fin.start = rc.concept_id \
           JOIN incisive.graph_concepts c ON c.id = fin.id \
          WHERE rc.user_id = $1",
    )
    .bind(user_id)
    .bind(MAX_CHAIN)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .filter_map(|(run, id, kind, label, certainty, source_row)| {
            Some(ConceptLink {
                run,
                id,
                kind: concept_kind(&kind)?,
                label,
                certainty,
                source_row,
            })
        })
        .collect())
}

async fn paper_links(pool: &PgPool, user_id: i64) -> sqlx::Result<Vec<PaperLink>> {
    sqlx::query_as(
        "SELECT rp.run_id AS run, p.pmid, p.title, p.year, p.retracted, rp.rank, rp.rank_label, \
                rp.includes_non_rct, rp.design, rp.n \
           FROM incisive.graph_run_papers rp \
           JOIN incisive.runs r ON r.id = rp.run_id AND r.user_id = $1 \
           JOIN incisive.graph_papers p ON p.pmid = rp.pmid \
          WHERE rp.user_id = $1",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await
}

/// The PMIDs each outcome row of a run's stored answer cites that are also
/// that run's own linked papers, by (run, 1-based row). The user's own runs
/// only; an answer with no `answer.outcomes` array (an old shape), a row
/// past the end, or a row without a `pmids` array gives nothing.
async fn outcome_papers(
    pool: &PgPool,
    user_id: i64,
) -> sqlx::Result<HashMap<(Uuid, i32), Vec<String>>> {
    let rows: Vec<(Uuid, i32, String)> = sqlx::query_as(
        "SELECT DISTINCT rc.run_id, rc.source_row, rp.pmid \
           FROM incisive.graph_run_concepts rc \
           JOIN incisive.runs r ON r.id = rc.run_id AND r.user_id = $1 \
           JOIN incisive.run_answers a ON a.run_id = rc.run_id \
           CROSS JOIN LATERAL ( \
                SELECT CASE WHEN jsonb_typeof(a.body #> '{answer,outcomes}') = 'array' \
                            THEN a.body #> '{answer,outcomes}' -> (rc.source_row - 1) -> 'pmids' \
                       END AS pmids) cited \
           CROSS JOIN LATERAL jsonb_array_elements_text( \
                CASE WHEN jsonb_typeof(cited.pmids) = 'array' \
                     THEN cited.pmids ELSE '[]'::jsonb END) pm(pmid) \
           JOIN incisive.graph_run_papers rp \
             ON rp.run_id = rc.run_id AND rp.user_id = $1 AND rp.pmid = pm.pmid \
          WHERE rc.user_id = $1 AND rc.source_row >= 1",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await?;
    let mut out: HashMap<(Uuid, i32), Vec<String>> = HashMap::new();
    for (run, row, pmid) in rows {
        out.entry((run, row)).or_default().push(pmid);
    }
    Ok(out)
}

/// Each linked paper's reference number in its run's stored answer, by run
/// and PMID (the user's own runs only).
async fn sheet_refs(pool: &PgPool, user_id: i64) -> sqlx::Result<HashMap<(Uuid, String), u16>> {
    let rows: Vec<(Uuid, String, i32)> = sqlx::query_as(
        "SELECT rp.run_id, rp.pmid, (ref->>'n')::int \
           FROM incisive.graph_run_papers rp \
           JOIN incisive.runs r ON r.id = rp.run_id AND r.user_id = $1 \
           JOIN incisive.run_answers a ON a.run_id = rp.run_id \
           CROSS JOIN LATERAL jsonb_array_elements( \
                CASE WHEN jsonb_typeof(a.body->'references') = 'array' \
                     THEN a.body->'references' ELSE '[]'::jsonb END) ref \
          WHERE rp.user_id = $1 AND ref->>'pmid' = rp.pmid \
            AND jsonb_typeof(ref->'n') = 'number' \
            AND (ref->>'n') ~ '^[0-9]{1,5}$'",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .filter_map(|(run, pmid, n)| Some(((run, pmid), u16::try_from(n).ok()?)))
        .collect())
}

fn blank_dot(id: String, kind: DotKind) -> Dot {
    Dot {
        id,
        kind,
        label: String::new(),
        sub: String::new(),
        first_year: None,
        r: 0.0,
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

fn in_years(year: Option<i32>, f: &MapFilter) -> bool {
    if f.year_from.is_none() && f.year_to.is_none() {
        return true;
    }
    year.is_some_and(|y| f.year_from.is_none_or(|lo| y >= lo) && f.year_to.is_none_or(|hi| y <= hi))
}

/// The name of concept `concept_id`, only when it is on `user_id`'s own map:
/// some concept of theirs (`graph_run_concepts` for this user) is it, or
/// merged into it, as the map's dots are. `None` for anyone else's concept
/// and for an id that does not exist.
pub async fn topic_label(
    pool: &PgPool,
    user_id: i64,
    concept_id: i64,
) -> sqlx::Result<Option<String>> {
    sqlx::query_scalar(
        "WITH RECURSIVE walk(start, id, merged_into, d) AS ( \
           SELECT c.id, c.id, c.merged_into, 0 FROM incisive.graph_concepts c \
            WHERE c.id IN (SELECT concept_id FROM incisive.graph_run_concepts WHERE user_id = $1) \
           UNION ALL \
           SELECT w.start, c.id, c.merged_into, w.d + 1 FROM walk w \
             JOIN incisive.graph_concepts c ON c.id = w.merged_into WHERE w.d < $3), \
         fin AS (SELECT DISTINCT ON (start) start, id FROM walk ORDER BY start, d DESC) \
         SELECT c.label FROM incisive.graph_concepts c \
          WHERE c.id = $2 AND c.id IN (SELECT id FROM fin)",
    )
    .bind(user_id)
    .bind(concept_id)
    .bind(MAX_CHAIN)
    .fetch_optional(pool)
    .await
}

/// One user's map under a filter. Dots come sorted by id with `x`/`y` at 0;
/// the page places them with [`crate::graph_view::layout`]. Above
/// [`MAX_DOTS`] the papers are left out unless the user chose what to hide.
pub async fn load_map(pool: &PgPool, user_id: i64, f: &MapFilter) -> sqlx::Result<MapData> {
    let mut m = load_map_all(pool, user_id, f).await?;
    if f.hidden.is_empty() && m.dots.len() > MAX_DOTS {
        m.dots.retain(|d| d.kind != DotKind::Paper);
        let present: HashSet<&str> = m.dots.iter().map(|d| d.id.as_str()).collect();
        m.lines
            .retain(|l| present.contains(l.a.as_str()) && present.contains(l.b.as_str()));
        m.papers_hidden_by_size = true;
    }
    Ok(m)
}

/// [`load_map`] with every dot the filter keeps, however many (the Explore
/// list, and the map before it is cut to size).
pub async fn load_map_all(pool: &PgPool, user_id: i64, f: &MapFilter) -> sqlx::Result<MapData> {
    let runs = user_runs(pool, user_id).await?;
    let concepts = concept_links(pool, user_id).await?;
    let papers = paper_links(pool, user_id).await?;
    let sheets = sheet_refs(pool, user_id).await?;
    let cited = outcome_papers(pool, user_id).await?;

    // Runs the organ and certainty filters keep.
    let keep: HashSet<Uuid> = runs
        .iter()
        .map(|r| r.id)
        .filter(|&run| {
            let organ_ok = f.organ.is_none_or(|organ| {
                concepts
                    .iter()
                    .any(|c| c.run == run && c.kind == DotKind::Organ && c.id == organ)
            });
            // The run's own (bottom-line) certainty; a run with no stored
            // certainty goes by its outcomes' certainty.
            let own = runs
                .iter()
                .find(|r| r.id == run)
                .and_then(|r| r.certainty.as_deref());
            let certainty_ok = f.certainty.as_deref().is_none_or(|want| match own {
                Some(got) => got.eq_ignore_ascii_case(want),
                None => concepts.iter().any(|c| {
                    c.run == run
                        && c.kind == DotKind::Outcome
                        && c.certainty
                            .as_deref()
                            .is_some_and(|got| got.eq_ignore_ascii_case(want))
                }),
            });
            organ_ok && certainty_ok
        })
        .collect();
    let runs: Vec<&RunRow> = runs.iter().filter(|r| keep.contains(&r.id)).collect();
    let concepts: Vec<&ConceptLink> = concepts.iter().filter(|c| keep.contains(&c.run)).collect();
    let papers: Vec<&PaperLink> = papers.iter().filter(|p| keep.contains(&p.run)).collect();
    // Newest run first, for the certainty an outcome shows.
    let newness: HashMap<Uuid, usize> = runs.iter().enumerate().map(|(i, r)| (r.id, i)).collect();

    // Earliest paper year per run, before the year range.
    let mut run_year: HashMap<Uuid, i32> = HashMap::new();
    for p in &papers {
        if let Some(y) = p.year {
            run_year
                .entry(p.run)
                .and_modify(|e| *e = (*e).min(y))
                .or_insert(y);
        }
    }
    let earliest =
        |runs: &BTreeSet<Uuid>| runs.iter().filter_map(|r| run_year.get(r)).min().copied();

    let mut dots: Vec<Dot> = Vec::new();
    let mut lines: BTreeSet<Line> = BTreeSet::new();
    let line = |a: &str, b: &str, paper: bool| Line {
        a: a.to_string(),
        b: b.to_string(),
        paper,
        out: false,
    };

    for r in &runs {
        let mut d = blank_dot(run_dot_id(r.id), DotKind::Run);
        d.label = "Your question".into();
        let mode = if r.mode == "quick" { "Quick" } else { "Full" };
        d.sub = format!("{mode} · {}", r.day);
        d.first_year = run_year.get(&r.id).copied();
        d.r = concept_radius(DotKind::Run, 1);
        d.run_id = Some(r.id);
        d.question = Some(r.question.clone());
        d.bottom_line = r.bottom_line.clone();
        d.certainty = r.certainty.clone();
        dots.push(d);
    }

    // Concepts, one dot each, with the runs that link them.
    struct Seen<'a> {
        kind: DotKind,
        label: &'a str,
        runs: BTreeSet<Uuid>,
        certainty: Option<(usize, &'a str)>,
    }
    let mut seen: BTreeMap<i64, Seen> = BTreeMap::new();
    let mut organs_of_run: HashMap<Uuid, BTreeSet<i64>> = HashMap::new();
    let mut diseases_of_run: HashMap<Uuid, BTreeSet<i64>> = HashMap::new();
    for c in &concepts {
        let s = seen.entry(c.id).or_insert(Seen {
            kind: c.kind,
            label: &c.label,
            runs: BTreeSet::new(),
            certainty: None,
        });
        s.runs.insert(c.run);
        if let Some(cert) = c.certainty.as_deref() {
            let at = newness.get(&c.run).copied().unwrap_or(usize::MAX);
            if s.certainty.is_none_or(|(old, _)| at < old) {
                s.certainty = Some((at, cert));
            }
        }
        let run = run_dot_id(c.run);
        let me = concept_dot_id(c.kind, c.id);
        match c.kind {
            DotKind::Organ => {
                organs_of_run.entry(c.run).or_default().insert(c.id);
            }
            DotKind::Disease => {
                diseases_of_run.entry(c.run).or_default().insert(c.id);
                lines.insert(line(&me, &run, false));
            }
            _ => {
                lines.insert(line(&run, &me, false));
            }
        }
    }
    for (run, organs) in &organs_of_run {
        let diseases = diseases_of_run.get(run);
        for g in organs {
            let g = concept_dot_id(DotKind::Organ, *g);
            match diseases {
                Some(ds) => {
                    for d in ds {
                        lines.insert(line(&g, &concept_dot_id(DotKind::Disease, *d), false));
                    }
                }
                // No disease: the organ hangs from the run itself.
                None => {
                    lines.insert(line(&g, &run_dot_id(*run), false));
                }
            }
        }
    }
    for (&id, s) in &seen {
        let mut d = blank_dot(concept_dot_id(s.kind, id), s.kind);
        d.label = s.label.to_string();
        d.sub = match s.kind {
            DotKind::Disease => {
                let organs: BTreeSet<&str> = s
                    .runs
                    .iter()
                    .filter_map(|r| organs_of_run.get(r))
                    .flatten()
                    .filter_map(|g| seen.get(g).map(|o| o.label))
                    .collect();
                organs.into_iter().collect::<Vec<_>>().join(", ")
            }
            DotKind::Outcome => s.certainty.map(|(_, c)| c.to_string()).unwrap_or_default(),
            _ => String::new(),
        };
        d.first_year = earliest(&s.runs);
        if s.kind == DotKind::Disease {
            // The newest of its runs that has a certainty.
            d.certainty = runs
                .iter()
                .filter(|r| s.runs.contains(&r.id))
                .find_map(|r| r.certainty.clone());
        }
        d.r = concept_radius(s.kind, s.runs.len());
        d.concept_id = Some(id);
        dots.push(d);
    }

    // Papers inside the year range, one dot each, at their strongest rank.
    let mut by_pmid: BTreeMap<&str, Vec<&PaperLink>> = BTreeMap::new();
    for p in papers.iter().filter(|p| in_years(p.year, f)) {
        by_pmid.entry(&p.pmid).or_default().push(p);
    }
    for (pmid, links) in &by_pmid {
        let best = links
            .iter()
            .min_by_key(|p| (rank_order(&p.rank), &p.rank_label))
            .expect("at least one link");
        let mut d = blank_dot(paper_dot_id(pmid), DotKind::Paper);
        d.label = best.title.clone();
        d.sub = match best.year {
            Some(y) => format!("{} · {y}", best.rank_label),
            None => best.rank_label.clone(),
        };
        d.first_year = best.year;
        d.r = paper_radius(&best.rank);
        d.rank = Some(best.rank.clone());
        d.pmid = Some(pmid.to_string());
        d.retracted = best.retracted;
        d.design = best.design.clone();
        d.n = best.n;
        d.includes_non_rct = best.includes_non_rct;
        // The newest run that cites it.
        d.sheet = links
            .iter()
            .filter_map(|p| {
                let n = sheets.get(&(p.run, p.pmid.clone()))?;
                Some((
                    newness.get(&p.run).copied().unwrap_or(usize::MAX),
                    p.run,
                    *n,
                ))
            })
            .min()
            .map(|(_, run, n)| SheetRef { run, n });
        dots.push(d);
        for p in links {
            lines.insert(line(&run_dot_id(p.run), &paper_dot_id(pmid), true));
        }
    }

    // Each outcome to the papers its own answer row cites, on the map
    // (one line per pair, however many runs share the outcome).
    for c in concepts.iter().filter(|c| c.kind == DotKind::Outcome) {
        let Some(pmids) = c.source_row.and_then(|row| cited.get(&(c.run, row))) else {
            continue;
        };
        for pmid in pmids {
            if by_pmid.contains_key(pmid.as_str()) {
                lines.insert(Line {
                    a: concept_dot_id(DotKind::Outcome, c.id),
                    b: paper_dot_id(pmid),
                    paper: true,
                    out: true,
                });
            }
        }
    }

    // What the user hid: kinds, and paper ranks.
    let hidden: HashSet<&str> = f.hidden.iter().map(String::as_str).collect();
    dots.retain(|d| {
        !hidden.contains(d.kind.as_str()) && d.rank.as_deref().is_none_or(|r| !hidden.contains(r))
    });
    dots.sort_by(|a, b| a.id.cmp(&b.id));
    let present: HashSet<&str> = dots.iter().map(|d| d.id.as_str()).collect();
    let lines: Vec<Line> = lines
        .into_iter()
        .filter(|l| present.contains(l.a.as_str()) && present.contains(l.b.as_str()))
        .collect();
    Ok(MapData {
        dots,
        lines,
        papers_hidden_by_size: false,
        runs: u32::try_from(runs.len()).unwrap_or(u32::MAX),
        top_papers: None,
    })
}
