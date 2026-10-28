//! Gold-set runner: the owner's benchmark questions as real, budgeted runs
//! against a **local** database, with a markdown report (docs/goldset/README.md).
//!
//! ```text
//! goldset --questions docs/goldset/questions.toml --mode focused --out report.md
//! ```
//!
//! Each question goes through the production run path: `ask::start_run`
//! (create the run, reserve its full cost limit under the user's caps and the
//! global monthly cap, audit `run_started`) and then the background worker,
//! which runs `agent::run_pipeline`, charges every Claude call through the
//! run budget, stores the answer and releases what is left. Only HTTP, the
//! sign-in and the medical gate are skipped (the questions are the owner's).
//!
//! The runs belong to one throwaway local user, `goldset@localhost` (role
//! surgeon), whose daily cap is set from `--daily-cap-inr` (default 20 runs at
//! the Focused limit).
//!
//! Safety: before any database connection or network call it refuses
//! `APP_ENV=prod`, any Supabase host, and any other non-local database host
//! unless `--allow-remote-db <host>` names that exact host (never Supabase).
//! There is no override.
//!
//! The report holds each question's id and its first 80 characters only.
//! A failed question is reported and the next one runs; the exit code is
//! non-zero only for setup errors.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use incisive_app::agent::answer::{Answer, Reference};
use incisive_app::agent::{AgentLlm, Literature, clip};
use incisive_app::ask::{AskOutcome, MAX_QUESTION_CHARS, start_run};
use incisive_app::config::Config;
use incisive_app::db::{lazy_pool, migrate};
use incisive_app::deid;
use incisive_app::gate::OfflineClassifier;
use incisive_app::identity::{self, IapVerifier, JwkSource, Role, User};
use incisive_app::lit::http::{AllowlistedClient, OaPolicy, default_user_agent};
use incisive_app::runs::budget::HARD_STOP_AFTER;
use incisive_app::runs::worker::WorkerHandle;
use incisive_app::runs::{RunMode, RunStatus};
use incisive_app::server::AppState;
use incisive_app::spend::{self, CapsConfig};
use serde::Deserialize;
use sqlx::PgPool;
use sqlx::Row as _;
use uuid::Uuid;

/// The throwaway local user every gold-set run belongs to.
pub const GOLDSET_EMAIL: &str = "goldset@localhost";

/// Default daily cap: this many runs at the Focused cost limit.
pub const DEFAULT_CAP_RUNS: i64 = 20;

/// Characters of each question shown in the report (after the id).
pub const TITLE_CHARS: usize = 80;

/// A title hint must be at least this long (normalised) to match as a prefix;
/// a shorter hint must match the whole title.
pub const MIN_PREFIX: usize = 20;

// ---------- questions file ----------

/// A paper a good answer should cite.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Landmark {
    #[serde(default)]
    pub pmid: Option<String>,
    #[serde(default)]
    pub doi: Option<String>,
    /// The start of the title (or the whole title when it is short).
    pub title_hint: String,
}

/// One gold-set question (`[[q]]`).
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Question {
    pub id: String,
    pub question: String,
    #[serde(default)]
    pub landmarks: Vec<Landmark>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct QuestionFile {
    #[serde(default)]
    q: Vec<Question>,
}

/// Parse and check a questions file: at least one question, unique non-blank
/// ids, non-blank questions and title hints. The error is a reason only.
pub fn parse_questions(text: &str) -> Result<Vec<Question>, String> {
    let file: QuestionFile = toml::from_str(text).map_err(|e| e.to_string())?;
    if file.q.is_empty() {
        return Err("no questions: add [[q]] entries (see questions.example.toml)".into());
    }
    let mut seen = HashSet::new();
    for (i, q) in file.q.iter().enumerate() {
        let n = i + 1;
        let id = q.id.trim();
        if id.is_empty() {
            return Err(format!("question {n}: id is blank"));
        }
        if !seen.insert(id.to_string()) {
            return Err(format!("question {n}: duplicate id `{id}`"));
        }
        if q.question.trim().is_empty() {
            return Err(format!("question `{id}`: question is blank"));
        }
        for (j, lm) in q.landmarks.iter().enumerate() {
            if lm.title_hint.trim().is_empty() {
                return Err(format!(
                    "question `{id}`, landmark {}: title_hint is blank",
                    j + 1
                ));
            }
        }
    }
    Ok(file.q)
}

// ---------- landmark matching ----------

/// Lower-case, `doi:` / resolver prefix removed.
fn normalise_doi(doi: &str) -> String {
    let d = doi.trim().to_lowercase();
    let d = [
        "https://doi.org/",
        "http://doi.org/",
        "https://dx.doi.org/",
        "http://dx.doi.org/",
        "doi:",
    ]
    .iter()
    .fold(d, |d, p| d.strip_prefix(p).map(str::to_string).unwrap_or(d));
    d.trim().to_string()
}

/// Lower-case, punctuation stripped (apostrophes removed, other punctuation
/// read as a space, so "ICG-based" and "ICG based" agree), whitespace collapsed.
pub fn normalise_title(s: &str) -> String {
    let spaced: String = s
        .chars()
        .filter(|c| !matches!(c, '\'' | '\u{2019}' | '\u{2018}'))
        .map(|c| if c.is_alphanumeric() { c } else { ' ' })
        .flat_map(char::to_lowercase)
        .collect();
    spaced.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn title_matches(hint: &str, title: &str) -> bool {
    let (hint, title) = (normalise_title(hint), normalise_title(title));
    if hint.is_empty() {
        return false;
    }
    if hint.chars().count() >= MIN_PREFIX {
        title.starts_with(&hint)
    } else {
        title == hint
    }
}

/// Whether a reference of the final answer is this landmark: same PMID, else
/// same DOI, else a title starting with the hint (see [`MIN_PREFIX`]).
pub fn landmark_hit(lm: &Landmark, refs: &[Reference]) -> bool {
    let pmid = lm.pmid.as_deref().map(str::trim).filter(|p| !p.is_empty());
    let doi = lm
        .doi
        .as_deref()
        .map(normalise_doi)
        .filter(|d| !d.is_empty());
    refs.iter().any(|r| {
        pmid.is_some_and(|p| r.pmid.as_deref().map(str::trim) == Some(p))
            || doi
                .as_deref()
                .is_some_and(|d| r.doi.as_deref().map(normalise_doi).as_deref() == Some(d))
            || title_matches(&lm.title_hint, &r.title)
    })
}

/// How a missed landmark is named in the report.
fn landmark_label(lm: &Landmark) -> String {
    if let Some(p) = lm.pmid.as_deref().map(str::trim).filter(|p| !p.is_empty()) {
        format!("PMID {p}")
    } else if let Some(d) = lm
        .doi
        .as_deref()
        .map(normalise_doi)
        .filter(|d| !d.is_empty())
    {
        format!("DOI {d}")
    } else {
        format!("\"{}\"", clip(&lm.title_hint, 40))
    }
}

// ---------- report ----------

/// One question's result.
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub id: String,
    /// The question's first [`TITLE_CHARS`] characters.
    pub title: String,
    /// done, partial, failed (or interrupted).
    pub status: String,
    /// The error kind (failed), or why the answer is partial or thin.
    pub detail: Option<String>,
    pub landmarks_hit: usize,
    pub landmarks_total: usize,
    pub missed: Vec<String>,
    /// References in the final answer.
    pub references: usize,
    /// Drafted references that Check verified, of all drafted references.
    pub refs_verified: u32,
    pub refs_drafted: u32,
    /// Every final reference resolves: each is one Check verified and has a
    /// PMID or DOI. Must be true for every row with an answer.
    pub final_refs_resolve: bool,
    pub claims_drafted: u32,
    pub claims_removed: u32,
    /// `runs.cost_paise`, read after the worker finished.
    pub cost_paise: i64,
    /// From start to the run's final status.
    pub wall: Duration,
}

impl Row {
    fn failed(q: &Question, kind: &str, cost_paise: i64, wall: Duration) -> Row {
        Row {
            id: q.id.trim().to_string(),
            title: clip(&q.question, TITLE_CHARS),
            status: RunStatus::Failed.as_str().to_string(),
            detail: Some(kind.to_string()),
            landmarks_hit: 0,
            landmarks_total: q.landmarks.len(),
            missed: q.landmarks.iter().map(landmark_label).collect(),
            references: 0,
            refs_verified: 0,
            refs_drafted: 0,
            final_refs_resolve: true,
            claims_drafted: 0,
            claims_removed: 0,
            cost_paise,
            wall,
        }
    }
}

/// The report's totals.
#[derive(Debug, Clone, PartialEq)]
pub struct Totals {
    pub questions: usize,
    pub landmark_hits: usize,
    pub landmark_total: usize,
    pub mean_cost_paise: i64,
    pub max_cost_paise: i64,
    pub mean_wall: Duration,
    pub max_wall: Duration,
    /// Runs over the cost limit or the time limit (must be 0).
    pub over_limits: usize,
    /// Rows whose final references do not all resolve (must be 0).
    pub unresolved: usize,
}

pub fn totals(rows: &[Row], limit_paise: i64, time_limit: Duration) -> Totals {
    let n = rows.len();
    let cost_sum: i64 = rows.iter().map(|r| r.cost_paise).sum();
    let wall_sum: Duration = rows.iter().map(|r| r.wall).sum();
    let (mean_cost_paise, mean_wall) = match i64::try_from(n) {
        Ok(k) if k > 0 => (
            (cost_sum + k / 2).div_euclid(k),
            wall_sum / u32::try_from(n).unwrap_or(u32::MAX),
        ),
        _ => (0, Duration::ZERO),
    };
    Totals {
        questions: n,
        landmark_hits: rows.iter().map(|r| r.landmarks_hit).sum(),
        landmark_total: rows.iter().map(|r| r.landmarks_total).sum(),
        mean_cost_paise,
        max_cost_paise: rows.iter().map(|r| r.cost_paise).max().unwrap_or(0),
        mean_wall,
        max_wall: rows.iter().map(|r| r.wall).max().unwrap_or_default(),
        over_limits: rows
            .iter()
            .filter(|r| r.cost_paise > limit_paise || r.wall > time_limit)
            .count(),
        unresolved: rows.iter().filter(|r| !r.final_refs_resolve).count(),
    }
}

fn rupees(paise: i64) -> String {
    let sign = if paise < 0 { "-" } else { "" };
    let p = paise.unsigned_abs();
    format!("{sign}\u{20b9}{}.{:02}", p / 100, p % 100)
}

fn duration(d: Duration) -> String {
    let s = d.as_secs();
    if s < 60 {
        format!("{s}s")
    } else {
        format!("{}m {:02}s", s / 60, s % 60)
    }
}

fn percent(num: usize, den: usize) -> String {
    if den == 0 {
        "\u{2014}".to_string()
    } else {
        format!("{:.0}%", num as f64 * 100.0 / den as f64)
    }
}

/// A markdown table cell: pipes escaped, no line breaks.
fn cell(s: &str) -> String {
    s.replace('|', "\\|")
        .replace(['\r', '\n'], " ")
        .trim()
        .to_string()
}

/// The markdown report: one row per question, then the totals.
pub fn render_report(
    rows: &[Row],
    totals: &Totals,
    mode: &str,
    limit_paise: i64,
    time_limit: Duration,
) -> String {
    let mut out = String::new();
    out.push_str("# Gold-set report\n\n");
    out.push_str(&format!(
        "Mode: **{mode}** (limit {} and {} per run). Real budgeted runs as `{GOLDSET_EMAIL}`; cost is `runs.cost_paise`.\n\n",
        rupees(limit_paise),
        duration(time_limit),
    ));
    out.push_str("| ID | Question (first 80 chars) | Status | Landmarks | Missed | Refs | Cite resolve | Final refs | Claims drafted | Claims removed | Cost | Time |\n");
    out.push_str("|---|---|---|---|---|---|---|---|---|---|---|---|\n");
    for r in rows {
        let status = match &r.detail {
            Some(d) => format!("{} ({d})", r.status),
            None => r.status.clone(),
        };
        let missed = if r.missed.is_empty() {
            "\u{2014}".to_string()
        } else {
            r.missed.join("; ")
        };
        let resolve = if r.refs_drafted == 0 {
            "\u{2014}".to_string()
        } else {
            format!(
                "{}/{} ({})",
                r.refs_verified,
                r.refs_drafted,
                percent(r.refs_verified as usize, r.refs_drafted as usize)
            )
        };
        let final_refs = if r.final_refs_resolve {
            "100%"
        } else {
            "**FLAG**"
        };
        let title: String = r.title.chars().take(TITLE_CHARS).collect();
        out.push_str(&format!(
            "| {} | {} | {} | {}/{} | {} | {} | {} | {} | {} | {} | {} | {} |\n",
            cell(&r.id),
            cell(&title),
            cell(&status),
            r.landmarks_hit,
            r.landmarks_total,
            cell(&missed),
            r.references,
            resolve,
            final_refs,
            r.claims_drafted,
            r.claims_removed,
            rupees(r.cost_paise),
            duration(r.wall),
        ));
    }
    let flag = |n: usize| if n == 0 { "" } else { " **FLAG**" };
    out.push_str("\n## Totals\n\n");
    out.push_str(&format!("- Questions: {}\n", totals.questions));
    out.push_str(&format!(
        "- Landmark recall: {}/{} ({})\n",
        totals.landmark_hits,
        totals.landmark_total,
        percent(totals.landmark_hits, totals.landmark_total)
    ));
    out.push_str(&format!(
        "- Cost: mean {}, max {}\n",
        rupees(totals.mean_cost_paise),
        rupees(totals.max_cost_paise)
    ));
    out.push_str(&format!(
        "- Time: mean {}, max {}\n",
        duration(totals.mean_wall),
        duration(totals.max_wall)
    ));
    out.push_str(&format!(
        "- Runs over {} or {} (must be 0): {}{}\n",
        rupees(limit_paise),
        duration(time_limit),
        totals.over_limits,
        flag(totals.over_limits)
    ));
    out.push_str(&format!(
        "- Answers with a final reference that does not resolve (must be 0): {}{}\n",
        totals.unresolved,
        flag(totals.unresolved)
    ));
    out.push_str(
        "\nCite resolve: drafted references that Check verified against PubMed / Europe PMC (removed ones never reach the answer). Final refs: every reference in the final answer was verified and has a PMID or DOI.\n",
    );
    out
}

// ---------- safety ----------

fn is_supabase(host: &str) -> bool {
    let h = host.trim_end_matches('.');
    h == "supabase.co"
        || h == "supabase.com"
        || h.ends_with(".supabase.co")
        || h.ends_with(".supabase.com")
}

fn check_host(raw: &str, allow_remote: Option<&str>) -> Result<(), String> {
    let host = raw.trim().trim_matches(['[', ']']).to_ascii_lowercase();
    // No host, or a Unix socket directory: this machine.
    if host.is_empty() || host.starts_with('/') {
        return Ok(());
    }
    if is_supabase(&host) {
        return Err(format!(
            "refusing to run: DATABASE_URL points at Supabase ({host}), which holds production data. \
             The gold-set runner never runs against Supabase; use a local database."
        ));
    }
    if matches!(host.as_str(), "localhost" | "127.0.0.1" | "::1") {
        return Ok(());
    }
    let allowed = allow_remote
        .map(|a| a.trim().trim_matches(['[', ']']).to_ascii_lowercase())
        .is_some_and(|a| a == host);
    if allowed {
        return Ok(());
    }
    Err(format!(
        "refusing to run: DATABASE_URL host {host} is not local (localhost, 127.0.0.1 or ::1). \
         If it is a disposable database, pass --allow-remote-db {host}."
    ))
}

/// Refuse a production environment or database. Runs before any connection.
/// `pghost` is `PGHOST`, which applies when the URL names no host. Messages
/// name the host only, never the URL (it may carry a password).
pub fn check_target(
    app_env: Option<&str>,
    database_url: &str,
    pghost: Option<&str>,
    allow_remote: Option<&str>,
) -> Result<(), String> {
    if app_env.is_some_and(|e| e.trim().eq_ignore_ascii_case("prod")) {
        return Err(
            "refusing to run: APP_ENV=prod. The gold-set runner runs only against a local development database."
                .into(),
        );
    }
    let not_url = || "refusing to run: DATABASE_URL is not a postgres:// URL".to_string();
    let url = url::Url::parse(database_url.trim()).map_err(|_| not_url())?;
    if !matches!(url.scheme(), "postgres" | "postgresql") {
        return Err(not_url());
    }
    let mut hosts: Vec<String> = url
        .query_pairs()
        .filter(|(k, _)| k == "host" || k == "hostaddr")
        .map(|(_, v)| v.into_owned())
        .collect();
    if let Some(h) = url.host_str().filter(|h| !h.is_empty()) {
        hosts.push(h.to_string());
    }
    if hosts.is_empty()
        && let Some(h) = pghost.filter(|h| !h.trim().is_empty())
    {
        hosts.push(h.to_string());
    }
    for raw in &hosts {
        for h in raw.split(',') {
            check_host(h, allow_remote)?;
        }
    }
    Ok(())
}

// ---------- running ----------

/// Create (or reuse) the gold-set user: role surgeon, active, with
/// `daily_cap_paise` and a monthly cap of at least that much.
pub async fn ensure_goldset_user(pool: &PgPool, daily_cap_paise: i64) -> sqlx::Result<User> {
    let (_, monthly) = spend::default_caps(Role::Surgeon);
    let monthly = monthly.map(|m| m.max(daily_cap_paise));
    sqlx::query(
        "INSERT INTO incisive.users (email, display_name, role, daily_cap_paise, monthly_cap_paise) \
         VALUES ($1, 'Gold set', 'surgeon', $2, $3) \
         ON CONFLICT (email) DO UPDATE SET role = 'surgeon', active = TRUE, \
           daily_cap_paise = EXCLUDED.daily_cap_paise, monthly_cap_paise = EXCLUDED.monthly_cap_paise",
    )
    .bind(GOLDSET_EMAIL)
    .bind(daily_cap_paise)
    .bind(monthly)
    .execute(pool)
    .await?;
    identity::find_user(pool, GOLDSET_EMAIL)
        .await?
        .ok_or(sqlx::Error::RowNotFound)
}

const TERMINAL: [&str; 4] = ["done", "partial", "failed", "interrupted"];

async fn run_state(pool: &PgPool, run_id: Uuid) -> sqlx::Result<Option<(String, Option<String>)>> {
    let row = sqlx::query("SELECT status, error_kind FROM incisive.runs WHERE id = $1")
        .bind(run_id)
        .fetch_optional(pool)
        .await?;
    row.map(|r| Ok((r.try_get("status")?, r.try_get("error_kind")?)))
        .transpose()
}

async fn run_cost(pool: &PgPool, run_id: Uuid) -> i64 {
    sqlx::query_scalar("SELECT cost_paise FROM incisive.runs WHERE id = $1")
        .bind(run_id)
        .fetch_optional(pool)
        .await
        .ok()
        .flatten()
        .unwrap_or(0)
}

async fn run_answer(pool: &PgPool, run_id: Uuid) -> Option<Answer> {
    let body: Option<serde_json::Value> =
        sqlx::query_scalar("SELECT body FROM incisive.run_answers WHERE run_id = $1")
            .bind(run_id)
            .fetch_optional(pool)
            .await
            .ok()
            .flatten();
    body.and_then(|b| serde_json::from_value(b).ok())
}

/// Why an answer is partial or thin (shown next to the status).
fn answer_notes(a: &Answer) -> Option<String> {
    let mut notes = Vec::new();
    if let Some(s) = &a.trail.stopped_early {
        notes.push(format!("stopped early: {s}"));
    }
    if a.trail.write_refused {
        notes.push("write refused".to_string());
    }
    if a.no_verified_claims {
        notes.push("no verified claims".to_string());
    }
    if !a.trail.unavailable.is_empty() {
        notes.push(format!("unavailable: {}", a.trail.unavailable.join(", ")));
    }
    if a.insufficient {
        notes.push("insufficient evidence".to_string());
    }
    (!notes.is_empty()).then(|| notes.join("; "))
}

/// Run one question as a real run of `user` and report it. Never fails: a
/// question that cannot run is a `failed` row with the reason.
pub async fn run_question(
    state: &AppState,
    user: &User,
    q: &Question,
    mode: RunMode,
    poll: Duration,
) -> Row {
    let started = Instant::now();
    // The same local checks `run_ask` makes before anything else.
    if q.question.chars().count() > MAX_QUESTION_CHARS {
        return Row::failed(q, "too_long", 0, started.elapsed());
    }
    if !deid::scan(&q.question).is_empty() {
        return Row::failed(q, "identifiers", 0, started.elapsed());
    }

    let request_id = Uuid::new_v4();
    let run_id = match start_run(state, request_id, user, q.question.trim(), mode, None).await {
        Ok(AskOutcome::RunStarted { run_id, .. }) => run_id,
        Ok(AskOutcome::OverCap { .. }) => {
            return Row::failed(q, "over_cap", 0, started.elapsed());
        }
        Ok(AskOutcome::RunInProgress { .. }) => {
            return Row::failed(q, "run_in_progress", 0, started.elapsed());
        }
        Ok(_) => return Row::failed(q, "not_started", 0, started.elapsed()),
        Err(_) => return Row::failed(q, "record", 0, started.elapsed()),
    };

    // Wait for the final status, then for the worker to finish (release,
    // audit, late charges), so the cost read below is final.
    let deadline = HARD_STOP_AFTER + Duration::from_secs(60);
    let mut wall: Option<Duration> = None;
    let (status, kind) = loop {
        match run_state(&state.pool, run_id).await {
            Ok(Some((status, kind))) if TERMINAL.contains(&status.as_str()) => {
                wall.get_or_insert_with(|| started.elapsed());
                if !state.worker.is_running(run_id) {
                    break (status, kind);
                }
            }
            Ok(Some(_)) => {}
            Ok(None) => {
                let cost = 0;
                return Row::failed(q, "run_missing", cost, started.elapsed());
            }
            // Transient: keep polling until the deadline.
            Err(_) => {}
        }
        if started.elapsed() > deadline {
            state.worker.cancel(run_id);
            let grace = Instant::now();
            while state.worker.is_running(run_id) && grace.elapsed() < Duration::from_secs(30) {
                tokio::time::sleep(poll).await;
            }
            break ("failed".to_string(), Some("runner_timeout".to_string()));
        }
        tokio::time::sleep(poll).await;
    };
    let wall = wall.unwrap_or_else(|| started.elapsed());
    let cost = run_cost(&state.pool, run_id).await;

    let answer = if matches!(status.as_str(), "done" | "partial") {
        run_answer(&state.pool, run_id).await
    } else {
        None
    };
    let Some(a) = answer else {
        let kind = kind.unwrap_or_else(|| {
            if status == "failed" || status == "interrupted" {
                status.clone()
            } else {
                "answer_unreadable".to_string()
            }
        });
        let mut row = Row::failed(q, &kind, cost, wall);
        if status == "interrupted" {
            row.status = status;
        }
        return row;
    };

    let (hit, missed): (Vec<&Landmark>, Vec<&Landmark>) = q
        .landmarks
        .iter()
        .partition(|lm| landmark_hit(lm, &a.references));
    let final_refs_resolve = u32::try_from(a.references.len())
        .is_ok_and(|n| n <= a.check.refs_verified)
        && a.references.iter().all(|r| {
            r.pmid.as_deref().is_some_and(|p| !p.trim().is_empty())
                || r.doi.as_deref().is_some_and(|d| !d.trim().is_empty())
        });
    Row {
        id: q.id.trim().to_string(),
        title: clip(&q.question, TITLE_CHARS),
        status,
        detail: answer_notes(&a),
        landmarks_hit: hit.len(),
        landmarks_total: q.landmarks.len(),
        missed: missed.into_iter().map(landmark_label).collect(),
        references: a.references.len(),
        refs_verified: a.check.refs_verified,
        refs_drafted: a.check.refs_total,
        final_refs_resolve,
        claims_drafted: a.check.checked,
        claims_removed: a.check.removed,
        cost_paise: cost,
        wall,
    }
}

// ---------- command line ----------

const USAGE: &str = "\
usage: goldset --questions FILE [--mode quick|focused] [--out FILE]
               [--daily-cap-inr RUPEES] [--limit N] [--allow-remote-db HOST]

Runs each question in FILE as a real, budgeted run against the local database
in DATABASE_URL and writes a markdown report (default goldset-report.md).
Needs ANTHROPIC_API_KEY, UNPAYWALL_EMAIL and MODEL_PRICES (NCBI_API_KEY
optional). See docs/goldset/README.md. These runs cost real money.";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Args {
    pub questions: PathBuf,
    pub mode: RunMode,
    pub out: PathBuf,
    pub daily_cap_inr: Option<i64>,
    pub limit: Option<usize>,
    pub allow_remote_db: Option<String>,
}

/// `Ok(None)` for `--help`.
pub fn parse_args(args: impl IntoIterator<Item = String>) -> Result<Option<Args>, String> {
    let mut it = args.into_iter();
    let (mut questions, mut mode, mut out) = (None, RunMode::Focused, None);
    let (mut daily_cap_inr, mut limit, mut allow_remote_db) = (None, None, None);
    while let Some(flag) = it.next() {
        if flag == "-h" || flag == "--help" {
            return Ok(None);
        }
        let mut value = || it.next().ok_or_else(|| format!("{flag} needs a value"));
        match flag.as_str() {
            "--questions" => questions = Some(PathBuf::from(value()?)),
            "--out" => out = Some(PathBuf::from(value()?)),
            "--mode" => {
                mode = match value()?.as_str() {
                    "quick" => RunMode::Quick,
                    "focused" => RunMode::Focused,
                    other => return Err(format!("--mode must be quick or focused, not `{other}`")),
                }
            }
            "--daily-cap-inr" => {
                let v = value()?;
                daily_cap_inr = Some(
                    v.parse::<i64>()
                        .ok()
                        .filter(|r| *r > 0 && r.checked_mul(100).is_some())
                        .ok_or_else(|| {
                            format!("--daily-cap-inr must be whole rupees, not `{v}`")
                        })?,
                );
            }
            "--limit" => {
                let v = value()?;
                limit = Some(
                    v.parse::<usize>()
                        .ok()
                        .filter(|n| *n > 0)
                        .ok_or_else(|| format!("--limit must be a positive number, not `{v}`"))?,
                );
            }
            "--allow-remote-db" => allow_remote_db = Some(value()?),
            other => return Err(format!("unknown argument `{other}`")),
        }
    }
    Ok(Some(Args {
        questions: questions.ok_or("--questions is required")?,
        mode,
        out: out.unwrap_or_else(|| PathBuf::from("goldset-report.md")),
        daily_cap_inr,
        limit,
        allow_remote_db,
    }))
}

fn env(var: &str) -> Option<String> {
    std::env::var(var)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

/// The app state the worker needs, built as `main.rs` builds it. The gate and
/// the IAP verifier are never used here.
fn build_state(pool: PgPool, config: &Config) -> Result<AppState, String> {
    let key = config
        .anthropic_api_key
        .clone()
        .ok_or("missing ANTHROPIC_API_KEY")?;
    let client = Arc::new(claude_client::Client::new(key));
    let llm = Arc::new(AgentLlm {
        client,
        models: config.agent_models.clone(),
    });
    let oa = OaPolicy::default();
    let lit = Arc::new(Literature::new(
        AllowlistedClient::new(default_user_agent(), oa.clone()),
        oa,
        config.ncbi_api_key.clone(),
        Some(config.unpaywall_email.clone()),
    ));
    let no_keys = serde_json::from_str(r#"{"keys":[]}"#).map_err(|e| e.to_string())?;
    Ok(AppState {
        pool,
        verifier: Arc::new(IapVerifier::new(String::new(), JwkSource::Static(no_keys))),
        gate: Arc::new(OfflineClassifier),
        prices: config.model_prices.clone(),
        usd_inr: config.usd_inr,
        caps: CapsConfig {
            global_monthly_paise: config.global_monthly_cap_paise,
        },
        gate_model: config.gate_model.clone(),
        dev_mode: false,
        runs: config.runs,
        worker: WorkerHandle::default(),
        llm: Some(llm),
        lit,
    })
}

async fn run() -> Result<(), String> {
    let Some(args) = parse_args(std::env::args().skip(1))? else {
        println!("{USAGE}");
        return Ok(());
    };

    // Safety first: nothing below may run against production.
    let database_url = env("DATABASE_URL").ok_or("missing DATABASE_URL")?;
    check_target(
        env("APP_ENV").as_deref(),
        &database_url,
        env("PGHOST").as_deref(),
        args.allow_remote_db.as_deref(),
    )?;
    for var in ["ANTHROPIC_API_KEY", "UNPAYWALL_EMAIL", "MODEL_PRICES"] {
        if env(var).is_none() {
            return Err(format!("missing {var} (see docs/goldset/README.md)"));
        }
    }
    // The app's own configuration, with the server-only settings defaulted.
    let config = Config::from_lookup(|k| match k {
        "APP_ENV" => env(k).or_else(|| Some("dev".into())),
        "CLAUDE_MODEL" => env(k).or_else(|| Some("claude-sonnet-5-5".into())),
        "OWNER_EMAIL" => env(k).or_else(|| Some(GOLDSET_EMAIL.into())),
        _ => env(k),
    })
    .map_err(|e| e.to_string())?;
    for (step, model) in config.agent_models.entries() {
        if config.model_prices.get(model).is_none() {
            return Err(format!(
                "MODEL_PRICES has no entry for AGENT_MODELS {step} `{model}`"
            ));
        }
    }

    let text = std::fs::read_to_string(&args.questions)
        .map_err(|e| format!("cannot read {}: {e}", args.questions.display()))?;
    let mut questions =
        parse_questions(&text).map_err(|e| format!("{}: {e}", args.questions.display()))?;
    if let Some(n) = args.limit {
        questions.truncate(n);
    }

    let settings = config.runs.for_mode(args.mode);
    let daily_cap_paise = args
        .daily_cap_inr
        .map(|r| r * 100)
        .unwrap_or(DEFAULT_CAP_RUNS * config.runs.focused.limit_paise);

    // Connection errors may echo URL parts: report the kind only.
    let pool = lazy_pool(&config.database_url).map_err(|_| "invalid DATABASE_URL".to_string())?;
    migrate(&pool)
        .await
        .map_err(|_| "could not connect to or migrate the database in DATABASE_URL".to_string())?;
    let user = ensure_goldset_user(&pool, daily_cap_paise)
        .await
        .map_err(|_| format!("could not create the {GOLDSET_EMAIL} user"))?;
    let state = build_state(pool, &config)?;

    let time_limit = Duration::from_secs(u64::from(settings.time_limit_s));
    let mode = args.mode.as_str();
    let write = |rows: &[Row]| {
        let t = totals(rows, settings.limit_paise, time_limit);
        let report = render_report(rows, &t, mode, settings.limit_paise, time_limit);
        std::fs::write(&args.out, report)
            .map_err(|e| format!("cannot write {}: {e}", args.out.display()))
    };
    eprintln!(
        "goldset: {} {mode} question(s); limit {} each; daily cap {}",
        questions.len(),
        rupees(settings.limit_paise),
        rupees(daily_cap_paise)
    );
    let mut rows = Vec::new();
    for (i, q) in questions.iter().enumerate() {
        eprintln!("[{}/{}] {} ...", i + 1, questions.len(), q.id.trim());
        let row = run_question(&state, &user, q, args.mode, Duration::from_secs(1)).await;
        eprintln!(
            "    {}{} | landmarks {}/{} | {} | {}",
            row.status,
            row.detail
                .as_deref()
                .map(|d| format!(" ({d})"))
                .unwrap_or_default(),
            row.landmarks_hit,
            row.landmarks_total,
            rupees(row.cost_paise),
            duration(row.wall)
        );
        rows.push(row);
        // After every question, so an interrupted session keeps its results.
        write(&rows)?;
    }
    write(&rows)?;
    eprintln!("goldset: report written to {}", args.out.display());
    Ok(())
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
        )
        .init();
    if let Err(e) = run().await {
        eprintln!("goldset: {e}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reference(pmid: Option<&str>, doi: Option<&str>, title: &str) -> Reference {
        Reference {
            ref_id: 1,
            title: title.to_string(),
            journal: None,
            year: None,
            pmid: pmid.map(str::to_string),
            doi: doi.map(str::to_string),
            study_type: "RCT".to_string(),
        }
    }

    fn row(id: &str, hit: usize, total: usize, cost: i64, secs: u64) -> Row {
        Row {
            id: id.to_string(),
            title: String::new(),
            status: "done".to_string(),
            detail: None,
            landmarks_hit: hit,
            landmarks_total: total,
            missed: Vec::new(),
            references: 0,
            refs_verified: 0,
            refs_drafted: 0,
            final_refs_resolve: true,
            claims_drafted: 0,
            claims_removed: 0,
            cost_paise: cost,
            wall: Duration::from_secs(secs),
        }
    }

    #[test]
    fn toml_parses() {
        let text = r#"
[[q]]
id = "icg-leak"
question = "Does ICG angiography reduce anastomotic leak after colorectal resection?"
landmarks = [
  { pmid = "40042831", title_hint = "Indocyanine Green Fluorescence Imaging in Prevention" },
  { doi = "10.1016/S2468-1253(25)00373-5", title_hint = "Indocyanine green fluorescence angiography for anastomotic" },
  { title_hint = "Intraoperative fluorescence angiography with indocyanine green" },
]

[[q]]
id = "hernia-mesh"
question = "Laparoscopic versus open mesh repair for inguinal hernia?"
"#;
        let qs = parse_questions(text).unwrap();
        assert_eq!(qs.len(), 2);
        assert_eq!(qs[0].id, "icg-leak");
        assert_eq!(qs[0].landmarks.len(), 3);
        assert_eq!(qs[0].landmarks[0].pmid.as_deref(), Some("40042831"));
        assert_eq!(
            qs[0].landmarks[1].doi.as_deref(),
            Some("10.1016/S2468-1253(25)00373-5")
        );
        assert!(qs[0].landmarks[2].pmid.is_none() && qs[0].landmarks[2].doi.is_none());
        assert!(qs[1].landmarks.is_empty());

        // The shipped example parses.
        let example = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../docs/goldset/questions.example.toml"
        ))
        .unwrap();
        let ex = parse_questions(&example).unwrap();
        assert_eq!(ex.len(), 2);
        assert!(ex.iter().all(|q| !q.landmarks.is_empty()));

        // Bad files are refused with a reason.
        for (bad, why) in [
            ("[[q]]\nid = \"a\"\n", "question"),
            ("[[q]]\nid = \"a\"\nquestion = \"q\"\nextra = 1\n", "extra"),
            (
                "[[q]]\nid = \"a\"\nquestion = \"q\"\n[[q]]\nid = \"a\"\nquestion = \"r\"\n",
                "duplicate",
            ),
            ("[[q]]\nid = \" \"\nquestion = \"q\"\n", "id"),
            ("[[q]]\nid = \"a\"\nquestion = \"  \"\n", "question"),
            (
                "[[q]]\nid = \"a\"\nquestion = \"q\"\nlandmarks = [{ pmid = \"1\" }]\n",
                "title_hint",
            ),
            ("", "no questions"),
        ] {
            let err = parse_questions(bad).unwrap_err();
            assert!(err.contains(why), "{bad:?}: {err}");
        }
    }

    #[test]
    fn landmark_matching_by_pmid_doi_title() {
        let refs = [
            reference(
                Some("40042831"),
                Some("10.1001/jamasurg.2025.0006"),
                "Indocyanine Green Fluorescence Imaging in Prevention of Colorectal Anastomotic Leakage: A Randomized Clinical Trial.",
            ),
            reference(
                None,
                Some("10.1016/S2468-1253(25)00373-5"),
                "Indocyanine green fluorescence angiography for anastomotic perfusion assessment in colorectal surgery: a systematic review",
            ),
            reference(None, None, "Short title."),
        ];
        let lm = |pmid: Option<&str>, doi: Option<&str>, hint: &str| Landmark {
            pmid: pmid.map(str::to_string),
            doi: doi.map(str::to_string),
            title_hint: hint.to_string(),
        };
        let unrelated = "completely unrelated title of some length";

        // PMID, exact (whitespace trimmed).
        assert!(landmark_hit(
            &lm(Some(" 40042831 "), None, unrelated),
            &refs
        ));
        assert!(!landmark_hit(&lm(Some("4004283"), None, unrelated), &refs));
        // DOI, case-insensitive, with or without the resolver prefix.
        assert!(landmark_hit(
            &lm(
                None,
                Some("https://doi.org/10.1016/s2468-1253(25)00373-5"),
                unrelated
            ),
            &refs
        ));
        assert!(landmark_hit(
            &lm(None, Some("10.1001/JAMASURG.2025.0006"), unrelated),
            &refs
        ));
        assert!(!landmark_hit(
            &lm(None, Some("10.1001/jamasurg.2025.0007"), unrelated),
            &refs
        ));
        // A wrong PMID still matches by DOI, then by title.
        assert!(landmark_hit(
            &lm(Some("1"), Some("10.1001/jamasurg.2025.0006"), unrelated),
            &refs
        ));
        // Title hint: normalised (case, punctuation, whitespace) prefix of >= 20 chars.
        assert!(landmark_hit(
            &lm(
                None,
                None,
                "indocyanine  green fluorescence ANGIOGRAPHY for anastomotic"
            ),
            &refs
        ));
        assert!(landmark_hit(
            &lm(
                None,
                None,
                "Indocyanine Green Fluorescence Imaging in Prevention of Colorectal"
            ),
            &refs
        ));
        // Not a prefix (a middle fragment).
        assert!(!landmark_hit(
            &lm(
                None,
                None,
                "Fluorescence Imaging in Prevention of Colorectal"
            ),
            &refs
        ));
        // A hint shorter than 20 characters must match the whole title.
        assert!(!landmark_hit(&lm(None, None, "Indocyanine green"), &refs));
        assert!(landmark_hit(&lm(None, None, "short  TITLE"), &refs));
        assert!(!landmark_hit(&lm(None, None, "short"), &refs));
        // Nothing to match against.
        assert!(!landmark_hit(&lm(Some("40042831"), None, unrelated), &[]));
    }

    #[test]
    fn report_totals() {
        let mut rows = vec![
            row("a", 2, 3, 1_200, 90),
            row("b", 1, 1, 3_800, 200),
            row("c", 0, 2, 0, 10),
        ];
        let t = totals(&rows, 4_000, Duration::from_secs(300));
        assert_eq!(t.questions, 3);
        assert_eq!((t.landmark_hits, t.landmark_total), (3, 6));
        assert_eq!(t.mean_cost_paise, 1_667); // 5000 / 3, rounded
        assert_eq!(t.max_cost_paise, 3_800);
        assert_eq!(t.mean_wall, Duration::from_secs(100));
        assert_eq!(t.max_wall, Duration::from_secs(200));
        assert_eq!(t.over_limits, 0);

        // At the limit is fine; over the cost or the time limit counts once.
        rows.push(row("d", 0, 0, 4_000, 300));
        rows.push(row("e", 0, 0, 4_001, 10));
        rows.push(row("f", 0, 0, 10, 301));
        rows.push(row("g", 0, 0, 5_000, 400));
        let t = totals(&rows, 4_000, Duration::from_secs(300));
        assert_eq!(t.over_limits, 3);
        assert_eq!(t.max_cost_paise, 5_000);

        let empty = totals(&[], 4_000, Duration::from_secs(300));
        assert_eq!(empty.questions, 0);
        assert_eq!(empty.mean_cost_paise, 0);

        // The rendered report carries the totals and one row per question,
        // and no question text past 80 characters.
        let mut long = row("long", 1, 2, 1_234, 75);
        long.title = format!("{}TAIL-NOT-SHOWN", "x".repeat(80));
        long.missed = vec!["pmid 123".to_string()];
        let report = render_report(
            &[long],
            &totals(&[], 4_000, Duration::from_secs(300)),
            "focused",
            4_000,
            Duration::from_secs(300),
        );
        assert!(report.contains("| long |"), "{report}");
        assert!(report.contains("1/2"), "{report}");
        assert!(report.contains("pmid 123"), "{report}");
        assert!(report.contains("\u{20b9}12.34"), "{report}");
        assert!(!report.contains("TAIL-NOT-SHOWN"), "{report}");
        assert!(report.contains("Landmark recall"), "{report}");
    }

    #[test]
    fn refuses_prod_database() {
        let local = "postgres://postgres@127.0.0.1:55434/postgres";
        // APP_ENV=prod is refused whatever the database.
        let err = check_target(Some("prod"), local, None, None).unwrap_err();
        assert!(err.contains("APP_ENV=prod"), "{err}");
        assert!(check_target(Some(" PROD "), local, None, None).is_err());

        // Local databases are allowed.
        for url in [
            local,
            "postgres://u:pw@localhost/db",
            "postgresql://u@[::1]:5432/db",
            "postgres:///db",
        ] {
            assert_eq!(check_target(Some("dev"), url, None, None), Ok(()), "{url}");
        }
        assert_eq!(check_target(None, local, None, None), Ok(()));

        // Supabase: refused, even when named in --allow-remote-db.
        for (url, host) in [
            (
                "postgres://postgres:hunter2pw@db.abcdefgh.supabase.co:5432/postgres",
                "db.abcdefgh.supabase.co",
            ),
            (
                "postgres://postgres.x:hunter2pw@aws-0-ap-south-1.pooler.supabase.com:6543/postgres",
                "aws-0-ap-south-1.pooler.supabase.com",
            ),
            ("postgres://u@x.SUPABASE.COM/db", "x.supabase.com"),
        ] {
            let err = check_target(Some("dev"), url, None, None).unwrap_err();
            assert!(err.contains("Supabase"), "{err}");
            assert!(!err.contains("hunter2pw"), "password leaked: {err}");
            assert!(check_target(Some("dev"), url, None, Some(host)).is_err());
        }

        // Any other remote host: refused unless named exactly.
        let remote = "postgres://u:hunter2pw@db.example.org:5432/db";
        let err = check_target(Some("dev"), remote, None, None).unwrap_err();
        assert!(err.contains("db.example.org"), "{err}");
        assert!(err.contains("--allow-remote-db"), "{err}");
        assert!(!err.contains("hunter2pw"), "password leaked: {err}");
        assert_eq!(
            check_target(Some("dev"), remote, None, Some("db.example.org")),
            Ok(())
        );
        assert!(check_target(Some("dev"), remote, None, Some("example.org")).is_err());
        assert!(check_target(Some("dev"), "postgres://u@10.0.0.5/db", None, None).is_err());

        // A host given as a query parameter, or through PGHOST when the URL
        // has none, is the host that counts.
        assert!(
            check_target(
                Some("dev"),
                "postgres://u@localhost/db?host=db.x.supabase.co",
                None,
                None
            )
            .is_err()
        );
        assert!(check_target(Some("dev"), "postgres:///db", Some("db.example.org"), None).is_err());
        assert_eq!(
            check_target(
                Some("dev"),
                "postgres:///db?host=/var/run/postgresql",
                None,
                None
            ),
            Ok(())
        );
        // Not a URL at all.
        assert!(check_target(Some("dev"), "not a url", None, None).is_err());
    }
}
