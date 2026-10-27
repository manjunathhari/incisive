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
settings are in section 10 and the deploy refuses to run until they are set too:

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

**`MODEL_PRICES`.** One JSON object, keyed by model id. Take the numbers from
Anthropic's pricing page (https://www.anthropic.com/pricing); do not guess them.
Every model the app calls (`GATE_MODEL` and `CLAUDE_MODEL`) needs an entry:

```json
{
  "claude-haiku-5-5":  {"input": <input USD per million tokens>, "output": <output USD per million tokens>},
  "claude-sonnet-5-5": {"input": <input USD per million tokens>, "output": <output USD per million tokens>}
}
```

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
| `surgeon` | 30000 | 300000 |
| `fellow` | 15000 | 150000 |

```sql
INSERT INTO incisive.users (email, display_name, role, daily_cap_paise, monthly_cap_paise)
VALUES ('colleague@example.com', 'Dr Colleague', 'surgeon', 30000, 300000);
-- fellow: 'fellow', 15000, 150000
```

Without the `incisive.users` row they can sign in through IAP but the app refuses them.
To suspend someone, `UPDATE incisive.users SET active = false WHERE email = '...';`.
