//! Queues the memory job for every existing Full run whose Expert Opinion
//! finished `done` (`memory::job::backfill`).
//!
//! Run by the owner against the live database after a deploy with MEMORY=on.
//! It only inserts pending `memory_jobs` rows and never changes `runs`. It
//! does not run migrations: the app owns the schema and migrates at startup.
//!
//! ```text
//! DATABASE_URL=postgres://... memory_backfill
//! ```
//!
//! Prints `queued <n> runs` and nothing else; never prints question text.

use incisive_app::db::lazy_pool;
use incisive_app::memory::job;
use secrecy::SecretString;

#[tokio::main]
async fn main() {
    if let Err(code) = run().await {
        eprintln!("memory_backfill: {code}");
        std::process::exit(1);
    }
}

async fn run() -> Result<(), &'static str> {
    let url = std::env::var("DATABASE_URL").map_err(|_| "DATABASE_URL is not set")?;
    let pool = lazy_pool(&SecretString::from(url)).map_err(|_| "invalid DATABASE_URL")?;
    let n = job::backfill(&pool)
        .await
        .map_err(|_| "could not connect to the database or backfill failed")?;
    println!("queued {n} runs");
    Ok(())
}
