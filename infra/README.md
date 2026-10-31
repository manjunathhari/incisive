# Incisive infrastructure (one-time setup)

Run these once, as the GCP project owner. Region is Mumbai (`asia-south1`).
Replace the `<...>` placeholders. Never commit real secret values.

```bash
export PROJECT_ID=<PROJECT_ID>
export REPO=manjunathhari/incisive
gcloud config set project "$PROJECT_ID"
gcloud services enable run.googleapis.com artifactregistry.googleapis.com \
  secretmanager.googleapis.com iamcredentials.googleapis.com \
  cloudbilling.googleapis.com billingbudgets.googleapis.com \
  iam.googleapis.com sts.googleapis.com cloudresourcemanager.googleapis.com
```

## 1. Artifact Registry

```bash
gcloud artifacts repositories create incisive --repository-format=docker --location=asia-south1
```

## 2. Secrets (values typed on stdin, never written to disk or git)

```bash
gcloud secrets create anthropic-api-key --replication-policy=user-managed --locations=asia-south1
gcloud secrets create database-url --replication-policy=user-managed --locations=asia-south1
gcloud secrets versions add anthropic-api-key --data-file=-   # paste key, then Ctrl-D
gcloud secrets versions add database-url --data-file=-        # paste Supabase URL, then Ctrl-D
```

Append `?sslmode=require` to the Supabase session-pooler connection string before pasting it, so the connection is always TLS-encrypted.

## 3. Runtime service account

```bash
gcloud iam service-accounts create incisive-run --display-name="Incisive runtime"
for s in anthropic-api-key database-url; do
  gcloud secrets add-iam-policy-binding "$s" \
    --member="serviceAccount:incisive-run@$PROJECT_ID.iam.gserviceaccount.com" \
    --role=roles/secretmanager.secretAccessor
done
```

(Project-level alternative: `gcloud projects add-iam-policy-binding "$PROJECT_ID" --member=... --role=roles/secretmanager.secretAccessor`.)

## 4. Deployer service account and Workload Identity Federation

```bash
gcloud iam service-accounts create incisive-deploy --display-name="Incisive GitHub deployer"
DEPLOY_SA=incisive-deploy@$PROJECT_ID.iam.gserviceaccount.com
gcloud projects add-iam-policy-binding "$PROJECT_ID" \
  --member="serviceAccount:$DEPLOY_SA" --role=roles/run.admin
# Least privilege: writer only on the incisive repository, serviceAccountUser only on the runtime SA.
gcloud artifacts repositories add-iam-policy-binding incisive --location=asia-south1 \
  --member="serviceAccount:$DEPLOY_SA" --role=roles/artifactregistry.writer
gcloud iam service-accounts add-iam-policy-binding incisive-run@$PROJECT_ID.iam.gserviceaccount.com \
  --member="serviceAccount:$DEPLOY_SA" --role=roles/iam.serviceAccountUser

gcloud iam workload-identity-pools create github --location=global --display-name="GitHub"
gcloud iam workload-identity-pools providers create-oidc github-oidc \
  --location=global --workload-identity-pool=github \
  --issuer-uri="https://token.actions.githubusercontent.com" \
  --attribute-mapping="google.subject=assertion.sub,attribute.repository=assertion.repository,attribute.repository_owner_id=assertion.repository_owner_id,attribute.ref=assertion.ref" \
  --attribute-condition="assertion.repository=='manjunathhari/incisive' && assertion.repository_owner_id=='<OWNER_ID>' && assertion.ref=='refs/heads/main'"

# Replace <OWNER_ID> above with your GitHub numeric owner ID (`gh api users/manjunathhari -q .id`);
# pinning the owner ID guards against a re-registered username.

PROJECT_NUMBER=$(gcloud projects describe "$PROJECT_ID" --format='value(projectNumber)')
gcloud iam service-accounts add-iam-policy-binding "$DEPLOY_SA" \
  --role=roles/iam.workloadIdentityUser \
  --member="principalSet://iam.googleapis.com/projects/$PROJECT_NUMBER/locations/global/workloadIdentityPools/github/attribute.repository/$REPO"

# Value for the WIF_PROVIDER variable:
gcloud iam workload-identity-pools providers describe github-oidc \
  --location=global --workload-identity-pool=github --format='value(name)'
```

## 5. Budget alert (Rs 500/month)

```bash
gcloud billing budgets create --billing-account=<BILLING_ACCOUNT_ID> --display-name="incisive" \
  --budget-amount=500INR --threshold-rule=percent=0.5 --threshold-rule=percent=0.9 \
  --threshold-rule=percent=1.0
```

The amount's currency must match the billing account's. If the account bills in USD,
use the USD equivalent of Rs 500 (for example `--budget-amount=6USD`).

## 6. GitHub repository variables

In GitHub: repo `manjunathhari/incisive` > Settings > Secrets and variables > Actions > **Variables**
(variables, not secrets; none are sensitive). Create these four now; the six Phase 1
settings are in section 10 and the Phase 2 settings in section 11, and the deploy
refuses to run until the required ones are set too:

| Variable | Value |
|---|---|
| `PROJECT_ID` | your GCP project ID |
| `WIF_PROVIDER` | `projects/<PROJECT_NUMBER>/locations/global/workloadIdentityPools/github/providers/github-oidc` |
| `DEPLOY_SA` | `incisive-deploy@<PROJECT_ID>.iam.gserviceaccount.com` |
| `CLAUDE_MODEL` | the Claude model ID chosen by the owner |

## 7. Verify after the first deploy

A push to `main` runs CI; when it succeeds, the Deploy workflow builds, pushes
`asia-south1-docker.pkg.dev/<PROJECT_ID>/incisive/incisive-app:<git-sha>` and deploys it.

1. Both workflows are green on the merge commit.
2. `URL=$(gcloud run services describe incisive-app --region=asia-south1 --format='value(status.url)')`
3. `curl -s $URL/health` returns `ok`; `curl -s $URL/ready` returns `{"db":"up"}`.
   (Cloud Run reserves some paths ending in `z`, so use `/health` and `/ready` there; `/healthz` and `/readyz` are for local use.)
   With request-based billing (`--cpu-throttling`), the background migration retry only progresses while requests are being served, so `/ready` can stay 503 a little longer after a paused Supabase wakes.
4. The home page shows "Incisive" and "Cut to the evidence."
5. Cloud Run console shows region `asia-south1`, timeout `3600s`, max instances `3`.
6. Secrets hygiene: search the Cloud Run logs for `sk-ant` and expect no results;
   `gcloud run services describe incisive-app --region=asia-south1` must show
   `ANTHROPIC_API_KEY` and `DATABASE_URL` as Secret Manager references (`secretKeyRef`), not values.

### Build caching

CI and Deploy share one Docker layer cache (GitHub Actions cache, scope `incisive`),
and the Dockerfile compiles dependencies in their own layer (cargo-chef), so a build
with unchanged dependencies takes a few minutes. No extra credentials are involved.
If a build ever looks stale, delete the cache in GitHub: Actions, Caches (left
sidebar), and remove the entries named `incisive`; the next build is a full, slow one.
GitHub keeps 10 GB of caches per repository and evicts the least recently used.

## 8. Google sign-in (Identity-Aware Proxy)

Only listed Google accounts can open the app; everyone else is refused by Google
before a request reaches the container. The deploy workflow uses
`--no-allow-unauthenticated`, so a deploy can never reopen the service.

First-time setup (in a project without an organization, Google requires the
console for this step because it creates the OAuth client):

1. Cloud Run → `incisive-app` → **Security** tab → **Require authentication** →
   tick **Identity-Aware Proxy (IAP)** → **Save** (enable the IAP API if prompted).
2. Let IAP call the service, and allow the owner's account:

```bash
gcloud run services add-iam-policy-binding incisive-app --region=asia-south1 \
  --member=serviceAccount:service-<PROJECT_NUMBER>@gcp-sa-iap.iam.gserviceaccount.com \
  --role=roles/run.invoker
gcloud iap web add-iam-policy-binding --region=asia-south1 \
  --resource-type=cloud-run --service=incisive-app \
  --member=user:<EMAIL> --role=roles/iap.httpsResourceAccessor
```

3. Check: `gcloud run services describe incisive-app --region=asia-south1` shows
   `Iap Enabled: true`; opening the URL in a private window asks for Google sign-in;
   a non-listed account is refused.

To add a colleague later, repeat the second command with their Google email.
After this, the section 7 checks are done in a signed-in browser
(`curl` without a Google identity token gets the sign-in redirect).

## 9. Audit log (`incisive.audit`)

The audit log is a hash chain: each row's hash covers its fields and the previous
row's hash. `audit::verify_chain` recomputes it and reports the first broken `seq`
(an edited row, a deleted row anywhere but the tail, or a gap in `seq`).

This is tamper **detection**, not prevention. Supabase connects the app as the
database owner, so the migration's `REVOKE UPDATE, DELETE ... FROM PUBLIC` does not
restrict the app; it is kept because it is harmless and documents intent. Real
prevention needs a separate restricted database role for the app (INSERT and SELECT
only on `incisive.audit`); that is a later follow-up.

Known limit: deleting the **newest** rows (truncating the tail) leaves a valid chain
and is not detectable by the chain alone. Periodically saving the latest `(seq, hash)`
somewhere the database owner cannot edit (an external checkpoint) would cover it. This
is a follow-up, not built yet.

The log never holds question text, patient identifiers, or model-generated text about
the question. It records only finding kinds with byte offsets, lengths, a SHA-256 of
accepted questions, verdicts, models, tokens and paise; the gate's reason is stored
only for rule decisions (static text), never for model decisions. `detail` may not
contain floating-point numbers (rejected on append). Follow-up: the question hash is
unsalted; an HMAC with a server secret would be stronger.

## 10. Phase 1 deploy settings and colleague onboarding

Six more repository variables (same place as section 6). The Deploy workflow fails
fast, naming any that is empty, and fails if `MODEL_PRICES` is not valid JSON. The app
also refuses to start in prod if `GATE_MODEL` or `CLAUDE_MODEL` has no entry in
`MODEL_PRICES`.

| Variable | What it is | Example shape |
|---|---|---|
| `IAP_AUDIENCE` | JWT audience the app checks on every request | `/projects/<PROJECT_NUMBER>/locations/asia-south1/services/incisive-app` |
| `OWNER_EMAIL` | Your Google account; created as the owner at startup | `you@gmail.com` |
| `GATE_MODEL` | Cheap model for the medical-only gate | `claude-haiku-5-5` |
| `MODEL_PRICES` | USD per million tokens, per model (JSON) | see below |
| `USD_INR` | Rupees per US dollar, used to cost each call | `92` |
| `GLOBAL_MONTHLY_CAP_INR` | Whole-app monthly budget in whole rupees | `<about 90% of your monthly Max API credit in rupees>` |

**`IAP_AUDIENCE`.** With IAP enabled directly on the Cloud Run service (section 8) it is
`/projects/<PROJECT_NUMBER>/locations/<REGION>/services/<SERVICE>`, here
`/projects/<PROJECT_NUMBER>/locations/asia-south1/services/incisive-app`. The project
number is `gcloud projects describe "$PROJECT_ID" --format='value(projectNumber)'`.
To confirm the exact string, open Google Cloud console → Security → Identity-Aware
Proxy, find `incisive-app`, click the three-dot menu → **Get JWT audience code**, and
copy the value.

**`MODEL_PRICES`.** One JSON object, keyed by model id, in USD per million
tokens. Take the numbers from Anthropic's pricing page; do not guess them. Every
model the app calls needs an entry: `GATE_MODEL`, `CLAUDE_MODEL` and every agent
step and role in `AGENT_MODELS`, including the defaults (sections 11 and 12), so
Opus 5.5 must be priced for the Phase 3 researcher and writer. The official prices
on platform.claude.com as of 29 Oct 2026:

```json
{
  "claude-haiku-5-5":  {"input": 0.10, "output": 0.50},
  "claude-sonnet-5-5": {"input": 2,    "output": 10},
  "claude-opus-5-5":   {"input": 4,    "output": 20}
}
```

Haiku 5.5 costs more for prompts above 100k tokens; the figure above is the rate
up to 100k. Incisive never sends Haiku a prompt that large (the gate and the
screening batches are a few thousand tokens), so that is the rate to use. If the
pricing page changes, update this variable; the app reads it at startup.

The deploy passes this through an env-vars file, so the commas and quotes in the JSON
are safe. Paste it as a single value in the GitHub variable.

**`USD_INR`.** Use the current rate rounded up (for example, if it is 91.3, enter
`92`), so the app slightly overestimates rupee spend rather than under.

**`GLOBAL_MONTHLY_CAP_INR`.** About 90% of your monthly Max API credit, converted to
whole rupees, so the cap trips before the credit runs out.

**`GATE_MODEL`.** `claude-haiku-5-5` unless you have a reason to change it. If you
change it, add the new model to `MODEL_PRICES`.

**Local runs** need `APP_ENV=dev` (this relaxes the prod-only variables and trusts the
`x-dev-user-email` header). The container image defaults to `APP_ENV=prod`.

### Adding a colleague

1. Add them as a test user on the OAuth consent screen (Google Auth Platform →
   Audience → Test users) if the app is still in testing mode.
2. Grant them access through IAP (the **IAP-secured Web App User** role):

```bash
gcloud iap web add-iam-policy-binding --region=asia-south1 \
  --resource-type=cloud-run --service=incisive-app \
  --member=user:<EMAIL> --role=roles/iap.httpsResourceAccessor
```

3. Add them to `incisive.users`. The email must be lower case (a CHECK constraint
   enforces it). Caps are in paise and match the app's defaults
   (`spend::default_caps`):

| Role | `daily_cap_paise` | `monthly_cap_paise` |
|---|---|---|
| `surgeon` | 80000 | 300000 |
| `fellow` | 40000 | 150000 |

```sql
INSERT INTO incisive.users (email, display_name, role, daily_cap_paise, monthly_cap_paise)
VALUES ('colleague@example.com', 'Dr Colleague', 'surgeon', 80000, 300000);
-- fellow: 'fellow', 40000, 150000
```

The daily caps were raised on 30 Oct 2026 (owner ruling R-S3b: fellow ₹400, surgeon
₹800, so a Full run's ₹300 reservation fits). Caps are stored on each user's row, so
users added earlier keep their old caps until updated:

```sql
UPDATE incisive.users SET daily_cap_paise = 80000 WHERE role = 'surgeon' AND daily_cap_paise = 30000;
UPDATE incisive.users SET daily_cap_paise = 40000 WHERE role = 'fellow' AND daily_cap_paise = 15000;
```

Without the `incisive.users` row they can sign in through IAP but the app refuses them.
To suspend someone, `UPDATE incisive.users SET active = false WHERE email = '...';`.


**Prompt caching (Phase 3 Stage 3).** The evidence team's tool turns use Anthropic
prompt caching. Each model's entry may also set `cache_write` and `cache_read`
(USD per million tokens); when they are absent the app uses 1.25 × and 0.1 × the
`input` price, the published rates (Opus 5.5: 5 and 0.40; Sonnet 5.5: 2.50 and 0.20).
Existing `MODEL_PRICES` values keep working unchanged. Holds never assume a cache
hit; charges use the real cache counts, which the spend ledger records (migration
0011, two nullable columns).

## 11. Phase 2 settings: evidence agent, NCBI key, CPU always on

Phase 2 adds five repository variables (same place as section 6) and one optional
secret. Only `UNPAYWALL_EMAIL` is required; the rest have defaults. The Deploy
workflow fails fast if `UNPAYWALL_EMAIL` is missing or is not an email address, if
`AGENT_MODELS` or `RUN_LIMITS` is set but is not a JSON object using only the key
names below (a typo such as `limt_paise` is caught here), or if `HAS_NCBI_KEY` or
`CPU_ALWAYS` is set to anything other than `true` or `false`.

| Variable | Required? | What it is | Default |
|---|---|---|---|
| `UNPAYWALL_EMAIL` | yes | Contact address sent to Unpaywall (which requires one) and to NCBI | none; the app will not start in prod without it |
| `AGENT_MODELS` | no | Model per agent step (JSON); the Phase 3 roles are in section 12 | `screen` = `claude-haiku-5-5`; `frame`, `search`, `extract`, `write`, `check` = `claude-sonnet-5-5` |
| `RUN_LIMITS` | no | Limits for Quick and Focused (JSON) | see the table below |
| `HAS_NCBI_KEY` | no | `true` once the `ncbi-api-key` secret exists (see below) | `false` |
| `CPU_ALWAYS` | no | `true` keeps the CPU on between requests (see below) | `false` |

**`AGENT_MODELS`.** Any of the keys `frame`, `search`, `screen`, `extract`, `write`,
`check`, and the Phase 3 roles `researcher`, `writer`, `extractor`, `skeptic`
(section 12); keys you leave out keep their default. Every model named here must also
have an entry in `MODEL_PRICES` (section 10), or the app refuses to start. The
Deploy workflow checks this before building, using the defaults above for any key
you leave out, so a missing price fails the deploy early.

```json
{"screen": "claude-haiku-5-5", "extract": "claude-sonnet-5-5"}
```

**`RUN_LIMITS`.** Either or both of `quick` and `focused`, each with only the
fields you want to change. Money is in paise (100 paise = 1 rupee). The defaults:

| Field | Quick | Focused | Meaning |
|---|---|---|---|
| `refinements` | 1 | 3 | times the search may be refined |
| `screen_n` | 20 | 100 | records screened |
| `use_n` | 5 | 10 | papers read |
| `full_text` | `false` | `true` | fetch free full text (otherwise abstracts only) |
| `limit_paise` | 1500 (₹15) | 4000 (₹40) | cost limit, reserved in full when the run starts |
| `time_limit_s` | 300 | 300 | time limit in seconds: the run starts writing up at 60% of it (180 s by default) and is failed at it plus 60 s (360 s by default); must be above 0 |

```json
{"quick": {"limit_paise": 2000}, "focused": {"screen_n": 80}}
```

**Phase 3 (evidence team) fields.** These sit in the same `quick` / `focused`
objects; `focused` holds Phase 3's **Full** mode. The Phase 2 pipeline that runs
today ignores them; they take effect when Phase 3 Stage 2 switches the run over.
The global, daily and monthly caps apply as before.

| Field | Quick | Focused (Full) | Meaning |
|---|---|---|---|
| `agent_limit_paise` | 10000 (₹100) | 30000 (₹300) | Phase 3 cost limit |
| `agent_time_limit_s` | 360 | 600 | Phase 3 time limit in seconds (hard stop at it plus 60 s) |
| `researcher_turns` | 8 | 15 | researcher tool-use turns at most (1 to 50) |
| `skeptic_turns` | 0 | 8 | skeptic tool-use turns at most (0 = no skeptic; up to 50) |
| `researcher_max_tokens` | 4000 | 4000 | output tokens per researcher turn (1 to 64000) |
| `skeptic_max_tokens` | 4000 | 4000 | output tokens per skeptic turn |
| `extractor_max_tokens` | 4000 | 4000 | output tokens per extraction call |
| `writer_max_tokens` | 20000 | 20000 | output tokens of the writer's call (its cost is reserved before research starts) |
| `writer_time_allowance_s` | 150 | 180 | seconds kept for the writer: research wraps up at `agent_time_limit_s` minus this; must be below `agent_time_limit_s` |
| `review_time_allowance_s` | 0 | 180 | seconds kept for the Full review (extractor ∥ skeptic): research wraps up this much earlier again; with `writer_time_allowance_s` it must be below `agent_time_limit_s` |
| `extractor_concurrency` | 3 | 3 | extractor calls in flight at once (1 to 8) |

Each call's full `max_tokens` is held against the cost limit before it is made,
so raising these makes each turn harder to fit, not more expensive in itself.

The question box shows each cost limit next to Quick and Focused, read from these
settings, so the figures there change when you change `RUN_LIMITS`.

**`UNPAYWALL_EMAIL`.** A real address you read. Unpaywall and NCBI use it only to
contact you if the app misbehaves on their service.

### NCBI API key (optional)

Without a key PubMed allows 3 requests a second; with one, 10. The app works
either way; a key just makes searches faster.

The app keeps to that rate **per instance**: each Cloud Run instance paces its own
PubMed requests. With `--max-instances=3`, three instances running questions at
the same moment could together send up to three times NCBI's limit. That needs
concurrent load across instances, which is unlikely at current use, and NCBI's
"too many requests" replies are retried with backoff. The key raises the limit
for every instance.

1. Sign in (or create a free account) at https://account.ncbi.nlm.nih.gov/.
2. Open **Account Settings** (your name, top right → Account settings).
3. Under **API Key Management**, click **Create an API Key** and copy it.
4. Store it in Secret Manager (paste the key, then Ctrl-D, so it never touches a
   file or your shell history) and let the runtime account read it:

```bash
gcloud secrets create ncbi-api-key --replication-policy=user-managed \
  --locations=asia-south1 --data-file=-   # paste key, then Ctrl-D
gcloud secrets add-iam-policy-binding ncbi-api-key \
  --member="serviceAccount:incisive-run@$PROJECT_ID.iam.gserviceaccount.com" \
  --role=roles/secretmanager.secretAccessor
```

5. Set the repository variable `HAS_NCBI_KEY` to `true`. The next deploy adds
   `NCBI_API_KEY` as a Secret Manager reference. Set it only after step 4 is done:
   a deploy that names a missing secret, or one the runtime account cannot read,
   fails.

To remove the key later, set `HAS_NCBI_KEY` to `false` and deploy; the deploy
replaces the secret list, so the key is no longer attached. The app never logs the
key (only whether one is set).

### CPU always on (optional; costs more)

**Why it matters.** A question runs in the background for up to about 5 minutes
after the Send request has already returned. With the default (`--cpu-throttling`,
request-based billing) Cloud Run gives the instance CPU only while it is serving a
request. The run page's polling does not reliably keep a run going: each poll gives
the run only a short burst of CPU, and every Claude call needs a dozen or so
database round trips. Expect Phase 2 under throttling to be slow even with the page
open, so Focused runs may stop early (partial, "time limit") or fail at the hard
stop. Once the page is closed or left idle, a run stalls; after 90 seconds without
a heartbeat it ends as `interrupted`. A stalled worker can also hold the spending
lock mid-transaction and delay other users' questions.

**`CPU_ALWAYS=true` is recommended for Phase 2.** It deploys with
`--no-cpu-throttling` (instance-based billing): the CPU stays on for the life of
the instance, so runs finish whether or not anyone is watching. The deploy default
stays `false` until you have confirmed the cost below.

**What it costs.** With instance-based billing you pay for every second an
instance exists, busy or idle, not just while it serves requests. With
`--min-instances=0` there is no instance (and no charge) when nobody is using the
app; but after the last request Google may keep the instance alive, and billed,
for up to about 15 minutes before shutting it down.

Work out the cost per instance-hour for this service (1 vCPU, 512 MiB = 0.5 GiB).
Read the current asia-south1 rates for instance-based billing from
https://cloud.google.com/run/pricing (pick the region on that page; do not reuse
figures from elsewhere, they change):

```
per instance-hour (USD) = (vCPU-second rate × 1 + GiB-second rate × 0.5) × 3600
per instance-hour (INR) = that × USD_INR
```

A single question keeps an instance alive for roughly its run time plus the idle
period, up to about 20 minutes, so about a third of an instance-hour per isolated
question; questions asked close together share the same instance. Multiply your
expected busy hours per month by the per-instance-hour figure, and compare it with
the Rs 500 budget alert (section 5). Cloud Run's monthly free tier may cover some
or all of it; the pricing page says how much.

When you are happy with the figure, set the repository variable `CPU_ALWAYS` to
`true`; the next deploy switches billing. Set it back to `false` to return to
request-based billing.

### Rolling back Phase 2 (prefer rolling forward)

If a Phase 2 release misbehaves, fix it and deploy again (roll forward). Going back
to a Phase 1 image (or shifting traffic to a Phase 1 revision) keeps the app
serving, but:

- **Readiness stays down.** The Phase 1 binary does not know migrations 0005 to
  0009 and its migration step refuses a database that has them, retrying forever:
  `/ready` (Cloud Run; `/readyz` locally) answers 503 and the logs show `migration attempt failed` about every
  30 seconds. Requests are still served (Cloud Run's default startup probe only
  checks the port). Do **not** delete rows from `_sqlx_migrations` to quiet this:
  rolling forward would then try to create the Phase 2 tables again and fail.
- **Activity page and chain check are wrong.** Phase 1 cannot read the Phase 2
  audit events (`run_started`, `run_finished`, `run_deleted`): the owner's Activity
  page shows an error and the chain check reports **Broken**. This is a false
  alarm, not tampering; both are correct again after rolling forward.
- **Questions running at the time of the rollback stay reserved.** Phase 1 has no
  run worker or recovery, so each one's full cost limit keeps counting against its
  user's daily and monthly caps and the global cap. Release them in the Supabase
  SQL editor (or `psql`):

  ```sql
  DELETE FROM incisive.spend WHERE kind = 'reservation' AND purpose = 'run';
  ```

  Charges already made stand. After rolling forward, recovery marks those runs
  `interrupted` within a minute.

## 12. Phase 3 settings: evidence team (Quick)

Phase 3 Stage 2 answers **Quick** questions with the evidence team: a researcher
(Opus 5.5, searching PubMed and Europe PMC in several turns, abstracts only) and a
writer (Opus 5.5, one structured answer). **Focused** questions still use the
Phase 2 pipeline until Stage 3. One new optional repository variable and four new
`AGENT_MODELS` keys:

| Variable | Required? | What it is | Default |
|---|---|---|---|
| `QUICK_ENGINE` | no | `team` (the evidence team) or `classic` (the Phase 2 Quick pipeline) | `team` |

**`QUICK_ENGINE`** is the rollback switch: set it to `classic` and deploy to send
Quick questions back to the Phase 2 pipeline, with no code change; set it to
`team` (or delete the variable) to return. The Deploy workflow refuses any other
value, and so does the app at startup. A question is reserved and run by the same
instance right after it is sent, so it always runs on the engine it was reserved
for; runs that are in flight during a deploy are interrupted as before.

Under `team`, a Quick run reserves `agent_limit_paise` (default ₹100, owner ruling of
29 Oct 2026) and runs under `agent_time_limit_s` (default 360 s, hard stop at 420 s)
from `RUN_LIMITS` (section 11); research wraps up `writer_time_allowance_s` (150 s)
before the time limit so the writer has time, and the writer is not started with less
than 150 s left before the hard stop (the run then ends partial, `writer_time`, with its
references). The question box shows the limit next to Quick. Under `classic` it reserves
`limit_paise` (₹15) and uses `time_limit_s`, as before.

**New `AGENT_MODELS` keys.** Each must be priced in `MODEL_PRICES` (section 10),
default or set; the Deploy workflow checks the defaults too.

| Key | Role | Default |
|---|---|---|
| `researcher` | finds and selects the studies (tool loop) | `claude-opus-5-5` |
| `writer` | writes the structured answer | `claude-opus-5-5` |
| `extractor` | extracts and appraises each study (Stage 3) | `claude-sonnet-5-5` |
| `skeptic` | looks for contradicting evidence (Stage 3) | `claude-sonnet-5-5` |
| `completeness` | the add-only completeness review after the Full writer (Stage 4b); `claude-opus-5-5` switches it to Opus | `claude-sonnet-5-5` |

**Full (Stage 3).** **Full** (shown as Full; the stored mode and the `RUN_LIMITS`
key stay `focused`) runs the whole team: the researcher (Opus, abstracts and full
text, up to `researcher_turns` 15), then the extractor and appraiser (Sonnet, one call
per study, `extractor_concurrency` 3 at once) beside the skeptic (Sonnet, up to
`skeptic_turns` 8), then the writer. A second optional variable switches it:

| Variable | Required? | What it is | Default |
|---|---|---|---|
| `FULL_ENGINE` | no | `team` (the evidence team) or `classic` (the Phase 2 Focused pipeline) | `team` |

It is checked like `QUICK_ENGINE`. Under `team` a Full run reserves
`agent_limit_paise` (₹300, owner ruling of 30 Oct 2026) and runs under
`agent_time_limit_s` (600 s, hard stop at 660 s): research wraps up
`writer_time_allowance_s + review_time_allowance_s` (180 + 180 s) before the time limit (at 240 s), the extractor and skeptic `writer_time_allowance_s`
before it (at 420 s), and the writer is not started with less than 180 s left before the hard stop (at 480 s; the run then ends partial,
`writer_time`, with its references). The live Full writer took 109 s for about 13,500 output tokens, so about 160 s at its 20,000-token ceiling. Under `classic` it reserves `limit_paise` (₹40) and uses `time_limit_s`.

**Completeness review (Stage 4b, Full only).** After the Full writer, code compares what the team
verified with the answer (the coverage check), and one add-only call (the `completeness` model)
adds what matters, each addition labelled "added on completeness review"; the writer's answer is
stored unchanged. It never fails a run: short of time (under 90 s before the hard stop), money, an
API error or an unusable reply, it is skipped with "Completeness review skipped: …". A third optional
variable switches it off:

| Variable | Required? | What it is | Default |
|---|---|---|---|
| `COMPLETENESS` | no | `on` or `off` (the rollback switch: Full answers then ship as the writer wrote them, as before Stage 4b) | `on` |

It is checked like `FULL_ENGINE` (only the ends trimmed); the app refuses any other value at startup.

**Rolling back.** Prefer `QUICK_ENGINE=classic` / `FULL_ENGINE=classic` to rolling back the image: it needs
no code change and keeps Phase 3 answers readable. Phase 3 adds migration 0010 (an
`answer_version` column on `run_answers`) and Stage 3 migration 0011 (two cache-token
columns on `spend`). From Stage 3 on, an image ignores applied migrations it does not
know (`ignore_missing`), so rolling back to an image built from Stage 3 or later
starts normally against the newer schema; images built before Stage 3 still refuse
it, as described for Phase 2 in section 11. This works only because every migration
is additive (new tables, nullable or defaulted columns, indexes); keep it so. Phase 3
answers are stored with `answer_version = 3` and are shown by the Phase 3 answer
view; Phase 2 answers keep `answer_version = 2` and their own view.
