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
(variables, not secrets; none are sensitive). Create all four:

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
