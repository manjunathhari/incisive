//! Counts, then queues, the existing finished runs for the graph job
//! (`graph::job::backfill_count` / `backfill_queue`).
//!
//! Run by the owner against the live database after a deploy. `--count` only
//! reads; `--queue` only inserts pending `graph_jobs` rows and never changes
//! `runs`. It does not run migrations: the app owns the schema.
//!
//! ```text
//! DATABASE_URL=postgres://... graph_backfill --count --paise-each 40
//! DATABASE_URL=postgres://... graph_backfill --queue
//! ```
//!
//! Prints `<n> runs, about <paise> paise` or `queued <n> runs` and nothing
//! else; never prints question text.

use incisive_app::db::lazy_pool;
use incisive_app::graph::job;
use secrecy::SecretString;

const USAGE: &str = "usage: graph_backfill --count --paise-each N | graph_backfill --queue";

#[tokio::main]
async fn main() {
    if let Err(code) = run().await {
        eprintln!("graph_backfill: {code}");
        std::process::exit(1);
    }
}

enum Mode {
    Count { paise_each: i64 },
    Queue,
}

fn parse(args: &[String]) -> Result<Mode, &'static str> {
    match args {
        [q] if q == "--queue" => Ok(Mode::Queue),
        [c, p, n] if c == "--count" && p == "--paise-each" => n
            .parse::<i64>()
            .ok()
            .filter(|n| *n >= 0)
            .map(|paise_each| Mode::Count { paise_each })
            .ok_or("--paise-each must be a whole number of paise"),
        _ => Err(USAGE),
    }
}

async fn run() -> Result<(), &'static str> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mode = parse(&args)?;
    let url = std::env::var("DATABASE_URL").map_err(|_| "DATABASE_URL is not set")?;
    let pool = lazy_pool(&SecretString::from(url)).map_err(|_| "invalid DATABASE_URL")?;
    const FAILED: &str = "could not connect to the database or the query failed";
    match mode {
        Mode::Count { paise_each } => {
            let n = job::backfill_count(&pool).await.map_err(|_| FAILED)?;
            println!("{}", job::backfill_line(n, paise_each));
        }
        Mode::Queue => {
            let n = job::backfill_queue(&pool).await.map_err(|_| FAILED)?;
            println!("queued {n} runs");
        }
    }
    Ok(())
}
