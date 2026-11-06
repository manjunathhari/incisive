//! Counts, then queues, the existing finished runs for the graph job
//! (`graph::job::backfill_count` / `backfill_queue`).
//!
//! Run by the owner against the live database after a deploy. `--count` only
//! reads; `--queue --expect N` only inserts pending `graph_jobs` rows and
//! never changes `runs`, and only when exactly N runs are still eligible (the
//! N `--count` printed and the owner approved); otherwise it queues nothing
//! and exits non-zero. `--queue` without `--expect` is refused. It does not
//! run migrations: the app owns the schema.
//!
//! ```text
//! DATABASE_URL=postgres://... graph_backfill --count --paise-each 40
//! DATABASE_URL=postgres://... graph_backfill --queue --expect 12
//! ```
//!
//! Prints `<n> runs, about <paise> paise` or `queued <n> runs` and nothing
//! else; never prints question text.

use incisive_app::db::lazy_pool;
use incisive_app::graph::job::{self, BackfillQueued};
use secrecy::SecretString;

const USAGE: &str =
    "usage: graph_backfill --count --paise-each N | graph_backfill --queue --expect N";

#[tokio::main]
async fn main() {
    if let Err(code) = run().await {
        eprintln!("graph_backfill: {code}");
        std::process::exit(1);
    }
}

enum Mode {
    Count { paise_each: i64 },
    Queue { expect: i64 },
}

fn whole(n: &str) -> Option<i64> {
    n.parse::<i64>().ok().filter(|n| *n >= 0)
}

fn parse(args: &[String]) -> Result<Mode, String> {
    match args {
        [q] if q == "--queue" => Err(format!(
            "--queue needs --expect N (the runs --count printed); {USAGE}"
        )),
        [q, e, n] if q == "--queue" && e == "--expect" => whole(n)
            .map(|expect| Mode::Queue { expect })
            .ok_or_else(|| "--expect must be a whole number of runs".to_string()),
        [c, p, n] if c == "--count" && p == "--paise-each" => whole(n)
            .map(|paise_each| Mode::Count { paise_each })
            .ok_or_else(|| "--paise-each must be a whole number of paise".to_string()),
        _ => Err(USAGE.to_string()),
    }
}

async fn run() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mode = parse(&args)?;
    let url = std::env::var("DATABASE_URL").map_err(|_| "DATABASE_URL is not set")?;
    let pool = lazy_pool(&SecretString::from(url)).map_err(|_| "invalid DATABASE_URL")?;
    let failed = |_| "could not connect to the database or the query failed".to_string();
    match mode {
        Mode::Count { paise_each } => {
            let n = job::backfill_count(&pool).await.map_err(failed)?;
            println!("{}", job::backfill_line(n, paise_each));
        }
        Mode::Queue { expect } => match job::backfill_queue(&pool, expect).await.map_err(failed)? {
            BackfillQueued::Queued(n) => println!("queued {n} runs"),
            BackfillQueued::Changed { found } => return Err(job::changed_line(expect, found)),
        },
    }
    Ok(())
}
