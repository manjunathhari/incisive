# Gold-set runner

The gold set is your own list of 20 clinical questions, each with the papers
you would expect a good answer to cite (its "landmarks"). The runner sends each
question through Incisive's real evidence agent and writes a report: did the
answer find the landmarks, did every citation check out, what did each run
cost, and how long did it take. Run it after any change to prompts, models or
limits and compare the report with the last one.

**These runs cost real money.** Each question is a real Claude run charged to
your Anthropic account, exactly like a question asked on the site. A Focused
run is capped at ₹40, so 20 Focused questions cost at most about ₹40 × 20 =
₹800 (Quick: ₹15 × 20 = ₹300). Most runs cost less than the cap.

## 1. Write your questions

Copy the example and edit it:

```bash
cp docs/goldset/questions.example.toml docs/goldset/questions.toml
```

Each question is a `[[q]]` block:

```toml
[[q]]
id = "icg-leak"
question = "In adults having colorectal resection, does indocyanine green fluorescence angiography reduce anastomotic leak?"
landmarks = [
  { pmid = "40042831", doi = "10.1001/jamasurg.2025.0006", title_hint = "Indocyanine Green Fluorescence Imaging in Prevention of Colorectal Anastomotic Leakage" },
  { title_hint = "Intraoperative fluorescence angiography with indocyanine green to prevent anastomotic leak" },
]
```

- `id`: a short label of your choice, different for every question. The report
  identifies questions by this.
- `question`: the question as you would type it into Incisive. Do not put
  patient details in it. A question with anything that looks like a patient
  identifier is refused, as it would be on the site.
- `landmarks`: the papers a good answer should cite. Two to four per question
  is plenty. For each one:
  - `title_hint` (required): the start of the paper's title, at least 20
    characters. Case, punctuation and extra spaces do not matter. A hint
    shorter than 20 characters has to match the whole title.
  - `pmid` and `doi` (optional, but best): copy them from PubMed. With them
    the match is exact. The DOI can be written with or without
    `https://doi.org/`.

A landmark counts as found when a reference in the final answer has the same
PMID, or else the same DOI, or else a title that starts with the hint.

Write 20 questions across the topics you care about, mixing ones with strong
trial evidence and ones where the honest answer is "evidence is thin".

`docs/goldset/questions.toml` is your file; the runner only reads it.

## 2. Run it

The runner needs:

- a **local** PostgreSQL database (it refuses the production database: see
  section 4);
- your Anthropic API key;
- the same `MODEL_PRICES` and `USD_INR` values as the production deploy
  (infra/README.md, section 10), so the costs in the report are real;
- the contact email used for Unpaywall and PubMed (`UNPAYWALL_EMAIL`);
- optionally your NCBI key (`NCBI_API_KEY`), which makes PubMed faster.

The key is typed into a hidden prompt, so it never appears on screen, in your
shell history or in a file.

### On your computer, or in Google Cloud Shell

You need Docker and Rust. Cloud Shell has Docker; if `cargo --version` fails
there or on your computer, install Rust from https://rustup.rs.

```bash
# 1. A throwaway local database (on this machine only).
docker run --rm -d --name goldset-db -e POSTGRES_PASSWORD=goldset \
  -p 127.0.0.1:5432:5432 pgvector/pgvector:pg16

# 2. Settings (paste MODEL_PRICES from your GitHub repository variables).
export DATABASE_URL=postgres://postgres:goldset@127.0.0.1:5432/postgres
export APP_ENV=dev
export UNPAYWALL_EMAIL=you@example.com
export MODEL_PRICES='{"claude-haiku-5-5": {"input": ..., "output": ...}, "claude-sonnet-5-5": {"input": ..., "output": ...}}'
export USD_INR=92

# 3. The key, typed into a hidden prompt (nothing is shown; press Enter).
read -rs ANTHROPIC_API_KEY; export ANTHROPIC_API_KEY
# Optional:
read -rs NCBI_API_KEY; export NCBI_API_KEY

# 4. Run (from the repository folder).
cargo run --release -p incisive-app --bin goldset -- \
  --questions docs/goldset/questions.toml --mode focused --out goldset-report.md

# 5. Afterwards: forget the key and remove the database.
unset ANTHROPIC_API_KEY NCBI_API_KEY
docker stop goldset-db
```

The first build takes several minutes. Then each question takes up to about
5 minutes; the runner prints one line per question as it goes (id, status,
landmarks found, cost, time; never the question text) and rewrites the report
after every question, so stopping it part-way keeps what is done.

If a run goes past its time limit and the database still shows it as
working, the runner cancels it. Its row then reads `failed (runner_timeout)`.
In the database the run itself is marked `failed (cancelled)`, and the
activity (audit) log records it as `deleted`, because cancelling uses the same
path as a user deleting a running question. The charges stand and the unused
reservation is released. If a cancelled run has not finished 15 minutes
later, the runner stops the whole set with a message. The report written so
far stays on disk.

Options:

| Option | What it does |
|---|---|
| `--questions FILE` | The questions file (required). |
| `--mode focused` or `--mode quick` | Run depth; Focused by default. |
| `--out FILE` | Where the report goes (default `goldset-report.md`). |
| `--limit N` | Only the first N questions (for a short trial run). |
| `--daily-cap-inr N` | The gold-set user's daily cap in rupees (default 20 × the Focused limit = ₹800). |
| `--allow-remote-db HOST` | Allow a disposable database on another machine, named exactly. Supabase is always refused. |

Optional settings read the same way as the app's: `RUN_LIMITS` and
`AGENT_MODELS` change limits and models, and `GLOBAL_MONTHLY_CAP_INR` sets the
monthly cap (₹10,000 if unset). These caps count only the runs recorded in the
local database, not what the live site has spent.

### Who the runs belong to

The runner creates (or reuses) one local user, `goldset@localhost`, with the
surgeon role. Every run goes through the same budget path as the site: the
full cost limit is reserved before the run starts, each Claude call is charged
at its real cost, and what is left is released at the end. The daily cap set
by `--daily-cap-inr` and the monthly cap still apply; a question that would go
over a cap is reported as `failed (over_cap)` and not run.

## 3. Reading the report

One row per question:

| Column | Meaning |
|---|---|
| ID | Your `id`. |
| Question | The first 80 characters of the question (nothing more is written). |
| Status | `done`, `partial` or `failed`. Partial and failed rows add the reason in brackets: for example `partial (stopped early: cost limit)`, `partial (write refused)`, `failed (llm)`, `failed (over_cap)`. "insufficient evidence" means the answer said the evidence was too thin to conclude. |
| Landmarks | Landmarks found in the final answer's references, as found/total. |
| Missed | Which landmarks were not found (by PMID, DOI or title hint). |
| Refs | References in the final answer. |
| Cite resolve | References the draft cited that the Check step verified in PubMed or Europe PMC, as verified/drafted (%). References that fail are removed before the answer is shown, so a lower number means Check caught something, not that a bad citation reached you. |
| Refs re-checked | After the run, the runner looks every reference in the final answer up again by itself, separately from Incisive's own Check step: in PubMed by PMID, or in Europe PMC by DOI when there is no PMID (free lookups, no Claude calls). A reference passes when the record is found, is not retracted (by PubMed's retraction type or a retraction notice), has the same title (ignoring case and punctuation) and, when both sides have a DOI, the same DOI. Shown as passed/total (%). This must be 100%; anything else is shown as **FLAG** with the reason for each failing reference (for example `ref 2 (PMID 123): title differs`) and should be reported. "—" means there was no answer to check. |
| Claims drafted | Claims in the first draft of the answer. |
| Claims removed | Claims the Check step removed (unsupported, or left with no verified reference). |
| Cost | What the run cost in rupees (the run's recorded cost). |
| Time | From starting the run to its final status. |

Totals at the end:

- **Landmark recall**: all landmarks found, out of all landmarks.
- **Cost**: mean and maximum per run.
- **Time**: mean and maximum per run.
- **Runs over the cost limit** (₹40 Focused, ₹15 Quick): must be 0.
- **Runs over 5 minutes**: must be 0.
- **Answers with a final reference that did not re-resolve**: must be 0.

## 4. Safety

The runner refuses to start, with a message saying why, when:

- `APP_ENV` is `prod`;
- `DATABASE_URL` points at Supabase (any `*.supabase.co` or `*.supabase.com`
  host, including the pooler), the production database. This is refused even
  with `--allow-remote-db`;
- `DATABASE_URL` points at any machine other than this one (`localhost`,
  `127.0.0.1` or `::1`), unless `--allow-remote-db` names that exact host.
  The same applies to the `PGHOST` and `PGHOSTADDR` variables whenever they
  are set, and to the host the database library itself would connect to.

These checks run before it connects to anything, and there is no switch to
turn them off. The runner never uses the live site, its sign-in or its data.
