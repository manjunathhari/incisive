//! Pages: signed-in banner, Ask page, run page, history (My questions), Activity page (owner).
//!
//! The server functions here call the same code as the JSON routes
//! (`crate::ask::run_ask` and friends). All server-only imports live inside their
//! bodies, so this module also builds for the browser (`hydrate`).
//!
//! Everything shown is plain text: Leptos escapes it, and no `inner_html` is used.
//! The answer is drawn from its structured fields: a Phase 2
//! `agent::answer::Answer` (`answer_version` 2) with its own view, or a
//! Phase 3 `agent::answer_v3::AnswerV3` (version 3) with [`answer_v3_view`],
//! whose `[PMID]` citations become numbered links to the reference list.

use leptos::prelude::*;
use leptos_meta::{MetaTags, Title, provide_meta_context};
use leptos_router::{
    components::{A, Route, Router, Routes},
    hooks::{use_navigate, use_params_map},
    path,
};

use crate::agent::answer::{
    self, Answer, DECISION_SUPPORT, INSUFFICIENT, PARTIAL_MARK, Reference, check_text, date_ist,
    duration_text, empty_bottom_line, exclusions_text, initial_mode, key_study_text, mode_hint,
    mode_label, render_text, rerun_href, screened_text, show_insufficient, stopped_text,
    unavailable_text,
};
use crate::agent::answer_v3::{
    ADDED_LABEL, AnswerV3, CompletenessV3, EvidenceV3, FOUND_BY_SKEPTIC, NO_GUIDELINE_SOURCE,
    NotableFinding, ReferenceV3, Segment, SkepticItem, Span, WrittenAnswer, arm_line_text,
    arm_lines, marked_runs, not_cited_at_end, split_citations, whole_marked,
};
use crate::ask::{ActivityView, AskOutcome, MeView};
use crate::runs::{RUN_NOT_FOUND_MESSAGE, RunMode, RunStatus, RunSummary, RunView};
use uuid::Uuid;

const STYLE: &str = r#"
:root { --fg:#1d232b; --muted:#5b6672; --bg:#f7f8fa; --card:#fff; --line:#d9dee4;
  --red:#a4161a; --red-bg:#fdecec; --amber:#8a5300; --amber-bg:#fff6e0;
  --green:#1b6b3a; --green-bg:#e9f7ee; --accent:#1f4e79; }
* { box-sizing:border-box; }
body { margin:0; font:16px/1.5 system-ui,-apple-system,"Segoe UI",sans-serif; color:var(--fg); background:var(--bg); }
header { background:var(--card); border-bottom:1px solid var(--line); padding:12px 16px; }
.top { max-width:860px; margin:0 auto; display:flex; flex-wrap:wrap; gap:8px 24px; align-items:baseline; justify-content:space-between; }
h1 { margin:0; font-size:1.4rem; color:var(--accent); }
.tagline { margin:0; color:var(--muted); font-size:.95rem; }
.banner { margin:0; font-size:.9rem; color:var(--muted); }
nav a { margin-right:16px; color:var(--accent); }
main { max-width:860px; margin:24px auto; padding:0 16px; }
label { display:block; font-weight:600; margin-bottom:6px; }
textarea { width:100%; padding:10px; font:inherit; border:1px solid var(--line); border-radius:6px; resize:vertical; }
.hint { color:var(--muted); font-size:.85rem; margin:6px 0 12px; }
button { font:inherit; padding:8px 20px; border:0; border-radius:6px; background:var(--accent); color:#fff; cursor:pointer; }
button[disabled] { opacity:.6; cursor:default; }
.box { margin-top:20px; padding:12px 16px; border-radius:6px; border-left:4px solid; }
.box p { margin:0 0 6px; } .box ul { margin:6px 0; }
.red { background:var(--red-bg); border-color:var(--red); color:var(--red); }
.amber { background:var(--amber-bg); border-color:var(--amber); color:var(--amber); }
.green { background:var(--green-bg); border-color:var(--green); color:var(--green); }
.chain { display:inline-block; padding:4px 10px; border-radius:12px; font-size:.9rem; }
.scroll { overflow-x:auto; }
table { border-collapse:collapse; width:100%; font-size:.85rem; background:var(--card); }
th, td { text-align:left; padding:6px 8px; border-bottom:1px solid var(--line); vertical-align:top; }
td code { font-size:.8rem; word-break:break-all; }
button.secondary { background:#fff; color:var(--accent); border:1px solid var(--accent); margin-left:8px; }
button.danger { background:var(--red); }
button.small { padding:4px 12px; font-size:.85rem; }
.badge { display:inline-block; padding:2px 10px; border-radius:12px; font-size:.85rem; background:var(--line); color:var(--fg); }
.badge.queued, .badge.running { background:var(--amber-bg); color:var(--amber); }
.badge.done { background:var(--green-bg); color:var(--green); }
.badge.partial { background:var(--amber-bg); color:var(--amber); }
.badge.failed, .badge.interrupted { background:var(--red-bg); color:var(--red); }
.run-question { overflow-wrap:anywhere; font-size:1.2rem; }
.steps { padding-left:20px; } .steps li { margin:4px 0; }
.runs { list-style:none; padding:0; margin:12px 0; }
.runs li { display:flex; flex-wrap:wrap; gap:6px 12px; align-items:center; justify-content:space-between; padding:10px 12px; background:var(--card); border:1px solid var(--line); border-radius:6px; margin-bottom:8px; }
.runs .meta { color:var(--muted); font-size:.85rem; }
.answer { background:var(--card); border:1px solid var(--line); border-radius:6px; padding:12px 16px; margin-top:16px; }
.answer h3 { margin:18px 0 6px; font-size:1.05rem; }
.answer .decision { font-weight:600; color:var(--accent); margin:0 0 4px; }
.answer dl { display:grid; grid-template-columns:max-content 1fr; gap:2px 12px; margin:0; }
.answer dt { color:var(--muted); } .answer dd { margin:0; }
.answer code { word-break:break-word; font-size:.85rem; }
.answer ul { padding-left:20px; margin:4px 0; } .answer li { margin:4px 0; }
.answer .refs-list { list-style:none; padding-left:0; }
.conf { color:var(--muted); font-size:.9rem; }
.partial-mark { color:var(--amber); font-weight:600; }
.answer a { color:var(--accent); word-break:break-word; }
.badge.mode { background:#e6edf5; color:var(--accent); }
fieldset.depth { border:0; padding:0; margin:0 0 14px; min-width:0; }
fieldset.depth legend { font-weight:600; padding:0; margin-bottom:6px; }
.seg { display:inline-flex; flex-wrap:wrap; border:1px solid var(--accent); border-radius:6px; overflow:hidden; background:var(--card); }
.seg label { display:block; position:relative; margin:0; font-weight:400; }
.seg input { position:absolute; opacity:0; width:1px; height:1px; margin:0; }
.seg span { display:block; padding:6px 14px; cursor:pointer; color:var(--accent); font-size:.95rem; }
.seg label + label span { border-left:1px solid var(--accent); }
.seg input:checked + span { background:var(--accent); color:#fff; }
.seg input:focus-visible + span { outline:2px solid var(--fg); outline-offset:-4px; }
.answer table.wide { min-width:720px; }
.answer .cite a { text-decoration:none; }
.answer .retracted { color:var(--red); font-weight:600; }
.answer details summary { cursor:pointer; font-weight:600; margin:18px 0 6px; }
.answer .etype { display:inline-block; margin-top:2px; padding:0 6px; border-radius:8px; font-size:.72rem; background:#e6edf5; color:var(--accent); }
.answer .glance { margin-top:12px; } .answer .glance ul { margin:4px 0 6px; }
.answer ul.arms { list-style:none; padding-left:0; margin:6px 0 0; font-size:.8rem; color:var(--muted); }
.answer textarea.copy-area { position:absolute; left:-9999px; width:1px; height:1px; opacity:0; }
.answer textarea.copy-area.shown { position:static; width:100%; height:12rem; opacity:1; margin-top:8px; font-size:.8rem; }
.answer sup.flag { font-size:.68rem; color:var(--amber); margin-left:2px; white-space:nowrap; cursor:help; }
.answer .flag-label { font-size:.75rem; color:var(--amber); font-weight:600; }
.answer ul.unverified { padding-left:16px; margin:4px 0 0; font-size:.8rem; }
.answer .flag-label.added { color:var(--accent); }
.answer p.added { margin:6px 0 0; }
.answer tr.added td { background:#f4f8fc; }
"#;

pub fn shell(options: LeptosOptions) -> impl IntoView {
    view! {
        <!DOCTYPE html>
        <html lang="en">
            <head>
                <meta charset="utf-8" />
                <meta name="viewport" content="width=device-width, initial-scale=1" />
                <style>{STYLE}</style>
                <AutoReload options=options.clone() />
                <HydrationScripts options />
                <MetaTags />
            </head>
            <body>
                <App />
            </body>
        </html>
    }
}

// ---------- server functions (same pipeline as the JSON routes) ----------

/// Ask a question. Same pipeline as `POST /api/ask`; `mode` defaults to Focused.
#[server(prefix = "/api/fn", endpoint = "ask")]
pub async fn ask_question(
    question: String,
    #[server(default)] mode: RunMode,
) -> Result<AskOutcome, ServerFnError> {
    use crate::identity::CurrentUser;
    let (state, user) = signed_in().await?;
    let user = match user {
        Ok(CurrentUser(u)) => u,
        Err(rejection) => {
            crate::ask::audit_identity_rejection(&state, &rejection).await;
            set_status(rejection.status());
            return Err(ServerFnError::new(rejection.message()));
        }
    };
    crate::ask::run_ask(&state, &user, &question, mode)
        .await
        .map_err(|f| {
            set_status(axum::http::StatusCode::INTERNAL_SERVER_ERROR);
            ServerFnError::new(f.message())
        })
}

/// Who is signed in, and today's spend. Same as `GET /api/me`.
#[server(prefix = "/api/fn", endpoint = "me")]
pub async fn get_me() -> Result<MeView, ServerFnError> {
    use crate::identity::CurrentUser;
    let (state, user) = signed_in().await?;
    let CurrentUser(user) = user.map_err(|r| {
        set_status(r.status());
        ServerFnError::new(r.message())
    })?;
    crate::ask::me_view(&state, &user).await.map_err(|f| {
        set_status(axum::http::StatusCode::INTERNAL_SERVER_ERROR);
        ServerFnError::new(f.message())
    })
}

/// The audit log (owner only). Same as `GET /api/activity`.
#[server(prefix = "/api/fn", endpoint = "activity")]
pub async fn get_activity() -> Result<ActivityView, ServerFnError> {
    use crate::ask::ActivityError;
    use crate::identity::CurrentUser;
    let (state, user) = signed_in().await?;
    // Owner check here as well as on the JSON route.
    let CurrentUser(user) = user.map_err(|r| {
        set_status(r.status());
        ServerFnError::new(r.message())
    })?;
    crate::ask::activity_view(&state, &user, None)
        .await
        .map_err(|e| {
            set_status(match e {
                ActivityError::NotOwner => axum::http::StatusCode::FORBIDDEN,
                ActivityError::Failed(_) => axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            });
            ServerFnError::new(e.message())
        })
}

/// The signed-in user's runs, newest first. Same as `GET /api/runs`.
#[server(prefix = "/api/fn", endpoint = "my_runs")]
pub async fn list_my_runs() -> Result<Vec<RunSummary>, ServerFnError> {
    use crate::identity::CurrentUser;
    let (state, user) = signed_in().await?;
    let CurrentUser(user) = user.map_err(|r| {
        set_status(r.status());
        ServerFnError::new(r.message())
    })?;
    // My questions shows the most recent 100 (the API's maximum).
    crate::runs::api::list_view(&state, &user, Some(crate::runs::api::MAX_LIMIT))
        .await
        .map_err(run_api_error)
}

/// One of the user's runs. Same as `GET /api/runs/{id}`; someone else's run is "not found".
#[server(prefix = "/api/fn", endpoint = "my_run")]
pub async fn get_my_run(run_id: Uuid) -> Result<RunView, ServerFnError> {
    use crate::identity::CurrentUser;
    let (state, user) = signed_in().await?;
    let CurrentUser(user) = user.map_err(|r| {
        set_status(r.status());
        ServerFnError::new(r.message())
    })?;
    crate::runs::api::get_view(&state, &user, run_id)
        .await
        .map_err(run_api_error)
}

/// Delete one of the user's runs (cancelling it if active). Same as `DELETE /api/runs/{id}`.
#[server(prefix = "/api/fn", endpoint = "delete_my_run")]
pub async fn delete_my_run(run_id: Uuid) -> Result<(), ServerFnError> {
    use crate::identity::CurrentUser;
    let (state, user) = signed_in().await?;
    let CurrentUser(user) = user.map_err(|r| {
        set_status(r.status());
        ServerFnError::new(r.message())
    })?;
    crate::runs::api::delete_one(&state, &user, run_id)
        .await
        .map_err(run_api_error)
}

/// Delete all of the user's runs; returns how many. Same as `DELETE /api/runs`.
#[server(prefix = "/api/fn", endpoint = "delete_my_runs")]
pub async fn delete_my_runs() -> Result<u64, ServerFnError> {
    use crate::identity::CurrentUser;
    let (state, user) = signed_in().await?;
    let CurrentUser(user) = user.map_err(|r| {
        set_status(r.status());
        ServerFnError::new(r.message())
    })?;
    crate::runs::api::delete_all(&state, &user)
        .await
        .map_err(run_api_error)
}

#[cfg(feature = "ssr")]
fn run_api_error(e: crate::runs::api::RunApiError) -> ServerFnError {
    use crate::runs::api::RunApiError;
    set_status(match e {
        RunApiError::NotFound => axum::http::StatusCode::NOT_FOUND,
        RunApiError::Failed => axum::http::StatusCode::INTERNAL_SERVER_ERROR,
    });
    ServerFnError::new(e.message())
}

#[cfg(feature = "ssr")]
type SignedIn = (
    crate::server::AppState,
    Result<crate::identity::CurrentUser, crate::identity::IdentityRejection>,
);

/// The app state and the identity check result for this request.
#[cfg(feature = "ssr")]
async fn signed_in() -> Result<SignedIn, ServerFnError> {
    let state = use_context::<crate::server::AppState>()
        .ok_or_else(|| ServerFnError::new("Incisive isn't ready yet. Please try again."))?;
    let user = leptos_axum::extract_with_state::<
        Result<crate::identity::CurrentUser, crate::identity::IdentityRejection>,
        _,
    >(&state)
    .await
    .map_err(|_| ServerFnError::new("Incisive couldn't read this request. Please try again."))?;
    Ok((state, user))
}

/// Server-function URL prefix (see the `#[server]` attributes above).
#[cfg(feature = "ssr")]
const FN_PREFIX: &str = "/api/fn/";

/// Set the HTTP status, but only when this is a direct server-function call
/// (`/api/fn/...`). The same functions also run while a page renders on the
/// server, and there the page's own status must stay as it is.
#[cfg(feature = "ssr")]
fn set_status(code: axum::http::StatusCode) {
    let is_fn_call = use_context::<axum::http::request::Parts>()
        .is_some_and(|p| p.uri.path().starts_with(FN_PREFIX));
    if !is_fn_call {
        return;
    }
    if let Some(opts) = use_context::<leptos_axum::ResponseOptions>() {
        opts.set_status(code);
    }
}

/// The plain message inside a server-function error.
fn error_text(e: &ServerFnError) -> String {
    match e {
        ServerFnError::ServerError(m) => m.clone(),
        _ => "Incisive couldn't reach the server. Check your connection and try again.".to_string(),
    }
}

// ---------- pages ----------

#[component]
pub fn App() -> impl IntoView {
    provide_meta_context();
    let me: MeResource = Resource::new(|| (), |_| get_me());
    // The Ask page reads the depth limits from it (one `/api/fn/me` per load).
    provide_context(me);
    view! {
        <Title text="Incisive" />
        <Router>
            <header>
                <div class="top">
                    <div>
                        <h1>"Incisive"</h1>
                        <p class="tagline">"Cut to the evidence."</p>
                    </div>
                    <Suspense fallback=|| view! { <p class="banner">"Checking sign-in…"</p> }>
                        {move || {
                            me.get()
                                .map(|r| match r {
                                    Ok(m) => {
                                        let owner = m.role == "owner";
                                        view! {
                                            <div>
                                                <p class="banner">"Signed in as " {m.display_name}</p>
                                                <nav>
                                                    <A href="/">"Ask"</A>
                                                    <A href="/history">"My questions"</A>
                                                    {owner.then(|| view! { <A href="/activity">"Activity"</A> })}
                                                </nav>
                                            </div>
                                        }
                                            .into_any()
                                    }
                                    Err(e) => view! { <p class="banner">{error_text(&e)}</p> }.into_any(),
                                })
                        }}
                    </Suspense>
                </div>
            </header>
            <main>
                <Routes fallback=|| view! { <p>"Page not found. Go back to the question box."</p> }>
                    <Route path=path!("") view=AskPage />
                    <Route path=path!("runs/:id") view=RunPage />
                    <Route path=path!("history") view=HistoryPage />
                    <Route path=path!("activity") view=ActivityPage />
                </Routes>
            </main>
        </Router>
    }
}

/// Who is signed in, and the depth limits (provided by [`App`]).
type MeResource = Resource<Result<MeView, ServerFnError>>;

#[component]
fn AskPage() -> impl IntoView {
    let (question, set_question) = signal(String::new());
    // Focused until the browser says otherwise (the server always renders Focused).
    let mode = RwSignal::new(RunMode::default());
    let action = ServerAction::<AskQuestion>::new();
    let pending = action.pending();
    let navigate = use_navigate();
    // Edit & re-run arrives as `/#q=<question>&mode=<mode>`. The fragment never
    // reaches the server; after hydration (effects run in the browser only)
    // it fills the box and is removed from the address bar. Its mode wins for
    // this page load (and is not remembered); otherwise the browser's saved
    // choice applies. The question box is only ever filled here, client-side.
    Effect::new(move |_| {
        let rerun = take_rerun_fragment();
        let rerun_mode = rerun.as_ref().map(|(_, m)| *m);
        if let Some((q, _)) = rerun {
            set_question.set(q);
        }
        mode.set(initial_mode(rerun_mode, load_mode().as_deref()));
    });
    // Once the question is accepted, follow its run.
    Effect::new(move |_| {
        if let Some(Ok(AskOutcome::RunStarted { run_id, .. })) = action.value().get() {
            navigate(&format!("/runs/{run_id}"), Default::default());
        }
    });
    let on_submit = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        action.dispatch(AskQuestion {
            question: question.get_untracked(),
            mode: mode.get_untracked(),
        });
    };
    view! {
        <form on:submit=on_submit>
            <label for="question">"Your clinical question"</label>
            <textarea
                id="question"
                name="question"
                rows="6"
                maxlength="4000"
                placeholder="e.g. 52 y/o M, robotic LAR: does ICG angiography reduce anastomotic leak?"
                prop:value=question
                on:input=move |ev| set_question.set(event_target_value(&ev))
            ></textarea>
            <p class="hint">
                "Leave out names, phone numbers, exact dates, addresses and hospital IDs. Ages, sex and post-op day are fine."
            </p>
            <DepthToggle mode />
            <button
                type="submit"
                disabled=move || pending.get() || question.with(|q| q.trim().is_empty())
            >
                {move || if pending.get() { "Checking…" } else { "Send" }}
            </button>
        </form>
        {move || action.value().get().map(outcome_view)}
    }
}

/// Quick or Full (stored as `focused`): two radio buttons drawn as a segmented control, each
/// with its cost limit from config. The choice is remembered in this browser.
#[component]
fn DepthToggle(mode: RwSignal<RunMode>) -> impl IntoView {
    let me = use_context::<MeResource>();
    let option = move |m: RunMode| {
        let hint = move || match me.and_then(|r| r.get()) {
            Some(Ok(v)) => mode_hint(
                m,
                match m {
                    RunMode::Quick => v.quick_limit_paise,
                    RunMode::Focused => v.focused_limit_paise,
                },
            ),
            _ => mode_label(m).to_string(),
        };
        view! {
            <label>
                <input
                    type="radio"
                    name="mode"
                    value=m.as_str()
                    // `checked` is what the server renders; `prop:checked` follows the signal.
                    checked=m == RunMode::default()
                    prop:checked=move || mode.get() == m
                    on:change=move |_| {
                        mode.set(m);
                        save_mode(m);
                    }
                />
                <span>
                    <Suspense fallback=move || mode_label(m)>{hint}</Suspense>
                </span>
            </label>
        }
    };
    view! {
        // The hint is announced with the group (and so with each choice).
        <fieldset class="depth" aria-describedby="depth-hint">
            <legend>"Depth"</legend>
            <div class="seg">{option(RunMode::Quick)} {option(RunMode::Focused)}</div>
            <p class="hint" id="depth-hint">
                "Quick reads abstracts only. Full reads more papers, with free full text where available, and checks the evidence more deeply. The amount is each run's cost limit."
            </p>
        </fieldset>
    }
}

/// The depth this browser last chose (`None` when storage is absent, blocked
/// or throws; browser only).
fn load_mode() -> Option<String> {
    #[cfg(feature = "hydrate")]
    {
        // Every step returns a Result or Option: a storage exception is caught
        // by web-sys and ends here as `None`.
        web_sys::window()?
            .local_storage()
            .ok()??
            .get_item(answer::MODE_STORAGE_KEY)
            .ok()?
    }
    #[cfg(not(feature = "hydrate"))]
    None
}

/// Remember the depth choice in this browser; any storage failure is ignored.
fn save_mode(m: RunMode) {
    #[cfg(feature = "hydrate")]
    {
        if let Some(s) = web_sys::window().and_then(|w| w.local_storage().ok().flatten()) {
            let _ = s.set_item(answer::MODE_STORAGE_KEY, m.as_str());
        }
    }
    #[cfg(not(feature = "hydrate"))]
    let _ = m;
}

fn outcome_view(r: Result<AskOutcome, ServerFnError>) -> AnyView {
    match r {
        Ok(AskOutcome::BlockedIdentifiers { findings, message }) => {
            let mut labels: Vec<String> = Vec::new();
            for f in findings {
                if !labels.contains(&f.label) {
                    labels.push(f.label);
                }
            }
            view! {
                <div class="box red" role="alert">
                    <p>{message}</p>
                    <ul>{labels.into_iter().map(|l| view! { <li>{l}</li> }).collect_view()}</ul>
                </div>
            }
            .into_any()
        }
        Ok(
            AskOutcome::InvalidInput { message }
            | AskOutcome::NotMedical { message }
            | AskOutcome::Unclear { message }
            | AskOutcome::OverCap { message }
            | AskOutcome::GateUnavailable { message }
            | AskOutcome::RunInProgress { message },
        ) => view! {
            <div class="box amber" role="status">
                <p>{message}</p>
            </div>
        }
        .into_any(),
        Ok(AskOutcome::RunStarted { message, alert, .. }) => view! {
            <div class="box green" role="status">
                <p>{message}</p>
                {alert.map(|a| view! { <p>{a}</p> })}
            </div>
        }
        .into_any(),
        Err(e) => view! {
            <div class="box amber" role="alert">
                <p>{error_text(&e)}</p>
            </div>
        }
        .into_any(),
    }
}

fn status_label(s: RunStatus) -> &'static str {
    match s {
        RunStatus::Queued => "Waiting to start",
        RunStatus::Running => "Working",
        RunStatus::Done => "Done",
        RunStatus::Partial => "Partly done",
        RunStatus::Failed => "Didn't finish",
        RunStatus::Interrupted => "Interrupted",
    }
}

fn status_badge(s: RunStatus) -> impl IntoView {
    view! { <span class=format!("badge {}", s.as_str())>{status_label(s)}</span> }
}

fn rupees(paise: i64) -> String {
    answer::rupees(paise)
}

/// How often the run page asks for news while the run is queued or running.
const POLL_EVERY: std::time::Duration = std::time::Duration::from_secs(2);

#[component]
fn RunPage() -> impl IntoView {
    let params = use_params_map();
    let run_id = move || {
        params
            .read()
            .get("id")
            .and_then(|s| Uuid::parse_str(&s).ok())
    };
    let tick = RwSignal::new(0u32);
    let data = Resource::new(
        move || (run_id(), tick.get()),
        |(id, _)| async move {
            match id {
                Some(id) => get_my_run(id).await,
                None => Err(ServerFnError::new(RUN_NOT_FOUND_MESSAGE)),
            }
        },
    );

    // Poll while queued or running. Stop only at a terminal status or when
    // the run is not found (deleted); any other error is transient: keep
    // polling and keep showing the last good view. Effects run only in the
    // browser, so the server never starts a timer.
    let timer: StoredValue<Option<leptos::leptos_dom::helpers::IntervalHandle>, LocalStorage> =
        StoredValue::new_local(None);
    let last_good: RwSignal<Option<RunView>> = RwSignal::new(None);
    let stop = move || {
        if let Some(h) = timer.try_update_value(|t| t.take()).flatten() {
            h.clear();
        }
    };
    let start = move || {
        if timer.with_value(|t| t.is_none()) {
            let h = leptos::leptos_dom::helpers::set_interval_with_handle(
                move || tick.update(|t| *t += 1),
                POLL_EVERY,
            );
            timer.set_value(h.ok());
        }
    };
    Effect::new(move |_| match data.get() {
        Some(Ok(v)) => {
            let active = v.summary.status.is_active();
            last_good.set(Some(v));
            if active { start() } else { stop() }
        }
        Some(Err(e)) if is_not_found(&e) => stop(),
        Some(Err(_)) => start(),
        None => {}
    });
    on_cleanup(stop);

    view! {
        <p>
            <A href="/history">"\u{2190} My questions"</A>
        </p>
        <Transition fallback=|| view! { <p>"Loading…"</p> }>
            {move || {
                data.get()
                    .map(|r| match r {
                        Ok(v) => run_view(v).into_any(),
                        Err(e) if is_not_found(&e) => {
                            view! { <p class="box amber" role="alert">{error_text(&e)}</p> }.into_any()
                        }
                        Err(e) => match last_good.get() {
                            Some(v) => view! {
                                <p class="hint" role="status">
                                    "Connection problem \u{2014} retrying. " {error_text(&e)}
                                </p>
                                {run_view(v)}
                            }
                                .into_any(),
                            None => view! {
                                <p class="box amber" role="alert">
                                    {error_text(&e)} " Retrying\u{2026}"
                                </p>
                            }
                                .into_any(),
                        },
                    })
            }}
        </Transition>
    }
}

/// The run is gone (or someone else's): polling stops.
fn is_not_found(e: &ServerFnError) -> bool {
    error_text(e) == RUN_NOT_FOUND_MESSAGE
}

fn run_view(v: RunView) -> impl IntoView {
    let status = v.summary.status;
    let ended_without_answer = v.answer.is_none() && !status.is_active();
    // The stored body is drawn from its fields, never shown raw: Phase 3
    // answers (version 3) with their own view, everything else as Phase 2.
    let answer = v.answer.map(|a| {
        if v.answer_version == crate::agent::answer_v3::ANSWER_VERSION {
            serde_json::from_value::<AnswerV3>(a).ok().map(Body::V3)
        } else {
            serde_json::from_value::<Answer>(a).ok().map(Body::V2)
        }
    });
    // The header is the full question once the answer holds it (§4), else
    // the stored 80-character title. Plain text: Leptos escapes it.
    let heading = answer
        .as_ref()
        .and_then(Option::as_ref)
        .map_or(v.summary.title, |a| match a {
            Body::V2(a) => a.question.clone(),
            Body::V3(a) => a.question.clone(),
        });
    view! {
        <h2 class="run-question">{heading}</h2>
        <p>
            {status_badge(status)} " " <span class="badge mode">{mode_label(v.summary.mode)}</span> " "
            <span class="meta">{rupees(v.summary.cost_paise)}</span>
        </p>
        <h3>"Progress"</h3>
        {if v.steps.is_empty() {
            view! { <p class="hint">"Nothing yet. This page updates by itself."</p> }.into_any()
        } else {
            view! {
                <ol class="steps">
                    {v.steps.into_iter().map(|s| view! { <li>{s.line}</li> }).collect_view()}
                </ol>
            }
                .into_any()
        }}
        {answer
            .map(|a| match a {
                Some(Body::V2(a)) => answer_view(a).into_any(),
                Some(Body::V3(a)) => answer_v3_view(a).into_any(),
                None => view! {
                    <p class="box amber" role="status">
                        "This answer can't be shown. Please ask the question again."
                    </p>
                }
                    .into_any(),
            })}
        {ended_without_answer
            .then(|| view! {
                <p class="box amber" role="status">
                    "This question ended without an answer. You can ask it again from the Ask page."
                </p>
            })}
    }
}

/// A stored answer, by format. Short-lived (one per page render), so the
/// size difference between the variants does not matter.
#[allow(clippy::large_enum_variant)]
enum Body {
    V2(Answer),
    V3(AnswerV3),
}

/// `https://pubmed.ncbi.nlm.nih.gov/<pmid>/` for an all-digit PMID.
fn pubmed_link(pmid: &str) -> Option<String> {
    (!pmid.is_empty() && pmid.chars().all(|c| c.is_ascii_digit()))
        .then(|| format!("https://pubmed.ncbi.nlm.nih.gov/{pmid}/"))
}

/// `https://doi.org/<doi>`, percent-encoded except for `/`.
fn doi_link(doi: &str) -> Option<String> {
    let doi = doi.trim();
    (!doi.is_empty()).then(|| {
        let path: Vec<String> = doi.split('/').map(answer::percent_encode).collect();
        format!("https://doi.org/{}", path.join("/"))
    })
}

fn reference_view(r: Reference) -> impl IntoView {
    let mut head = r.title.trim_end_matches('.').to_string();
    head.push('.');
    match (&r.journal, r.year) {
        (Some(j), Some(y)) => head.push_str(&format!(" {j}, {y}.")),
        (Some(j), None) => head.push_str(&format!(" {j}.")),
        (None, Some(y)) => head.push_str(&format!(" {y}.")),
        (None, None) => {}
    }
    let pmid = r
        .pmid
        .clone()
        .and_then(|p| pubmed_link(&p).map(|href| (p, href)));
    let doi = r
        .doi
        .clone()
        .and_then(|d| doi_link(&d).map(|href| (d, href)));
    view! {
        <li>
            {format!("[{}] ", r.ref_id)}
            {head}
            {pmid.map(|(p, href)| view! { " " <a href=href rel="noopener noreferrer" target="_blank">{format!("PMID {p}")}</a> })}
            {doi.map(|(d, href)| view! { " " <a href=href rel="noopener noreferrer" target="_blank">{format!("DOI {d}")}</a> })}
            " " <span class="conf">{format!("({})", r.study_type)}</span>
        </li>
    }
}

/// The Edit & re-run fragment of the current address, if any, which is then
/// cleared (browser only; `None` elsewhere).
fn take_rerun_fragment() -> Option<(String, RunMode)> {
    #[cfg(feature = "hydrate")]
    {
        let w = web_sys::window()?;
        let found = answer::parse_rerun_fragment(&w.location().hash().ok()?)?;
        // Keep the question out of reloads, history and shared links.
        if let Ok(h) = w.history() {
            let _ = h.replace_state_with_url(&wasm_bindgen::JsValue::NULL, "", Some("/"));
        }
        Some(found)
    }
    #[cfg(not(feature = "hydrate"))]
    None
}

/// Copy the plain-text answer (browser only).
fn copy_text(text: &str) {
    #[cfg(feature = "hydrate")]
    {
        // The promise is not awaited: a refused copy changes nothing.
        let _ = web_sys::window().map(|w| w.navigator().clipboard().write_text(text));
    }
    #[cfg(not(feature = "hydrate"))]
    let _ = text;
}

/// The answer, in the §4 order, as escaped text and links only.
fn answer_view(a: Answer) -> impl IntoView {
    let text = render_text(&a);
    let rerun = rerun_href(&a.question, a.mode);
    let empty = empty_bottom_line(&a);
    let insufficient = show_insufficient(&a);
    let pico = a.pico.clone();
    let check = check_text(&a.check);
    let t = a.trail.clone();
    let screened = screened_text(&t);
    let exclusions = exclusions_text(&t);
    let stopped = t.stopped_early.as_deref().map(stopped_text);
    let unavailable = unavailable_text(&t);
    let cost = format!(
        "{} \u{b7} {}",
        rupees(a.cost_paise),
        duration_text(a.elapsed_s)
    );
    let copied = RwSignal::new(false);
    view! {
        <section class="answer" aria-label="Answer">
            <p class="decision">{DECISION_SUPPORT}</p>
            <p class="meta">{format!("{} \u{b7} {}", mode_label(a.mode), date_ist(a.created_at))}</p>

            <h3>"How I read your question"</h3>
            <dl>
                <dt>"Population"</dt><dd>{pico.population}</dd>
                <dt>"Intervention"</dt><dd>{pico.intervention}</dd>
                <dt>"Comparison"</dt><dd>{pico.comparison.unwrap_or_else(|| "none stated".into())}</dd>
                {(!pico.outcomes.is_empty()).then(|| view! { <dt>"Outcomes"</dt><dd>{pico.outcomes.join("; ")}</dd> })}
                {(!pico.limits.is_empty()).then(|| view! { <dt>"Limits"</dt><dd>{pico.limits.join("; ")}</dd> })}
            </dl>
            {(!a.final_query.is_empty()).then(|| view! { <p>"PubMed query: " <code>{a.final_query.clone()}</code></p> })}
            // A full page load (`rel="external"`) so the fragment survives to the
            // home page; the browser does not send it to the server.
            <p><a href=rerun rel="external">"Edit & re-run"</a></p>

            <h3>"Bottom line"</h3>
            {insufficient.then(|| view! { <p class="box amber">{INSUFFICIENT}</p> })}
            {match empty {
                Some(msg) => view! { <p>{msg}</p> }.into_any(),
                None => view! {
                    <ul>
                        {a.bottom_line
                            .into_iter()
                            .map(|c| {
                                view! {
                                    <li>
                                        {c.partial.then(|| view! { <span class="partial-mark" title="partly supported">{PARTIAL_MARK}</span> " " })}
                                        {c.text}
                                        " " {answer::refs_text(&c.refs)}
                                        " " <span class="conf">
                                            {if c.partial {
                                                format!("({}, partly supported)", c.confidence)
                                            } else {
                                                format!("({})", c.confidence)
                                            }}
                                        </span>
                                    </li>
                                }
                            })
                            .collect_view()}
                    </ul>
                }
                    .into_any(),
            }}

            {(!a.key_studies.is_empty()).then(|| view! {
                <h3>"Key studies"</h3>
                <ul>
                    {a.key_studies
                        .into_iter()
                        .map(|k| view! { <li>{format!("[{}] {}", k.ref_id, key_study_text(&k))}</li> })
                        .collect_view()}
                </ul>
            })}

            <h3>"References"</h3>
            {if a.references.is_empty() {
                view! { <p>"None verified."</p> }.into_any()
            } else {
                view! {
                    <ul class="refs-list">
                        {a.references.into_iter().map(reference_view).collect_view()}
                    </ul>
                }
                    .into_any()
            }}

            <h3>"Check"</h3>
            <p>{check}</p>

            <h3>"Search trail"</h3>
            <ul>
                <li>{format!("PubMed: {} hits \u{b7} Europe PMC: {} hits", t.pubmed_count, t.epmc_count)}</li>
                <li>{screened}</li>
                {exclusions.map(|x| view! { <li>{x}</li> })}
                {t.refinements
                    .into_iter()
                    .map(|r| view! { <li>"Refined: " {r.reason} " " <code>{r.query}</code></li> })
                    .collect_view()}
                {stopped.map(|x| view! { <li>{x}</li> })}
                {unavailable.map(|x| view! { <li>{x}</li> })}
            </ul>

            <h3>"Cost and time"</h3>
            <p>{cost}</p>

            <button
                class="secondary small"
                on:click=move |_| {
                    copy_text(&text);
                    copied.set(true);
                }
            >
                {move || if copied.get() { "Copied" } else { "Copy as text" }}
            </button>
        </section>
    }
}

/// Answer text with its `[PMID]` markers as numbered links to the
/// references (`#ref-N`). Text stays plain (escaped); a PMID with no
/// reference (none should be left after the citation check) is not shown.
fn cited(text: &str, refs: &std::collections::HashMap<String, u16>) -> AnyView {
    split_citations(text, |p| refs.contains_key(p))
        .into_iter()
        .map(|seg| match seg {
            Segment::Text(t) => t.into_any(),
            Segment::Cite(ids) => {
                let mut nums: Vec<u16> = ids.iter().filter_map(|p| refs.get(p).copied()).collect();
                nums.sort_unstable();
                nums.dedup();
                if nums.is_empty() {
                    return ().into_any();
                }
                let last = nums.len() - 1;
                view! {
                    <span class="cite">
                        "["
                        {nums
                            .into_iter()
                            .enumerate()
                            .map(|(i, n)| {
                                view! {
                                    <a href=format!("#ref-{n}") rel="external">{n.to_string()}</a>
                                    {(i < last).then_some(", ")}
                                }
                            })
                            .collect_view()}
                        "]"
                    </span>
                }
                .into_any()
            }
        })
        .collect_view()
        .into_any()
}

fn reference_v3_view(r: ReferenceV3) -> impl IntoView {
    let mut head = r.title.trim_end_matches('.').to_string();
    head.push('.');
    match (&r.journal, r.year) {
        (Some(j), Some(y)) => head.push_str(&format!(" {j}, {y}.")),
        (Some(j), None) => head.push_str(&format!(" {j}.")),
        (None, Some(y)) => head.push_str(&format!(" {y}.")),
        (None, None) => {}
    }
    let pmid = pubmed_link(&r.pmid).map(|href| (r.pmid.clone(), href));
    let doi = r
        .doi
        .clone()
        .and_then(|d| doi_link(&d).map(|href| (d, href)));
    view! {
        <li id=format!("ref-{}", r.n)>
            {format!("[{}] ", r.n)}
            {r.retracted.then(|| view! { <span class="retracted">"RETRACTED \u{2014} do not rely on this paper. "</span> })}
            {head}
            {pmid.map(|(p, href)| view! { " " <a href=href rel="noopener noreferrer" target="_blank">{format!("PMID {p}")}</a> })}
            {doi.map(|(d, href)| view! { " " <a href=href rel="noopener noreferrer" target="_blank">{format!("DOI {d}")}</a> })}
        </li>
    }
}

/// Which mark a flagged number gets (Stage 4 items 5 and 7).
#[derive(Clone, Copy)]
enum Mark {
    /// The writer's number, not traced to what it was given.
    NotTraced,
    /// An extractor or skeptic number its source does not have.
    NotVerified,
}

fn mark_view(m: Mark) -> AnyView {
    let (label, title) = match m {
        Mark::NotTraced => (
            "not traced",
            "This number was not found in a verified quote, a structured number or an abstract given to the writer.",
        ),
        Mark::NotVerified => (
            "not verified",
            "This number was not found in the source text.",
        ),
    };
    view! { <sup class="flag" title=title>{label}</sup> }.into_any()
}

/// Text with numbered citations ([`cited`]) and a small mark after each run
/// `spans` marks on `field` (the text itself is never changed).
#[derive(Clone, Copy)]
struct Marks<'a> {
    refs: &'a std::collections::HashMap<String, u16>,
    spans: &'a [Span],
    mark: Mark,
}

impl Marks<'_> {
    fn t(&self, text: &str, field: &str) -> AnyView {
        let runs = marked_runs(text, field, self.spans);
        if runs.iter().all(|(_, m)| !m) {
            return cited(text, self.refs);
        }
        let mark = self.mark;
        runs.into_iter()
            .map(|(t, m)| view! { {cited(&t, self.refs)} {m.then(|| mark_view(mark))} })
            .collect_view()
            .into_any()
    }

    /// The mark alone, when `field` is marked as a whole (a structured
    /// number).
    fn whole(&self, field: &str) -> Option<AnyView> {
        whole_marked(field, self.spans).then(|| mark_view(self.mark))
    }
}

/// The written part of a Phase 3 answer: bottom line, outcome table,
/// trials table, conflicts, guidelines, applicability, what would change it.
/// Numbers not traced to the writer's sources carry a "not traced" mark.
// The page's parts, each drawn from its own stored field.
#[allow(clippy::too_many_arguments)]
fn written_v3_view(
    w: WrittenAnswer,
    refs: &std::collections::HashMap<String, u16>,
    untraced: &[Span],
    evidence: &std::collections::HashMap<String, EvidenceV3>,
    skeptic_guidelines: Vec<(usize, SkepticItem)>,
    skeptic_spans: &[Span],
    guideline_leads: Vec<crate::agent::answer_v3::SkepticLead>,
    completeness: Option<&CompletenessV3>,
    row_leads: &std::collections::HashMap<String, Vec<String>>,
) -> impl IntoView {
    let full = !evidence.is_empty();
    // Stage 4b: the completeness review's additions, drawn in place, each
    // labelled; its numbers not traced to the writer's sources are marked.
    let comp = completeness.cloned().unwrap_or_default();
    let cm = Marks {
        refs,
        spans: &comp.untraced,
        mark: Mark::NotTraced,
    };
    let added = || view! { " " <span class="flag-label added">{ADDED_LABEL}</span> };
    let unsourced =
        |f: bool| f.then(|| view! { " " <span class="flag-label">{NO_GUIDELINE_SOURCE}</span> });
    let mk = Marks {
        refs,
        spans: untraced,
        mark: Mark::NotTraced,
    };
    let c = w.bottom_line.certainty;
    // Guidelines (Stage 4 item 1; R-S4a): the skeptic's verified guidelines
    // and its unverified guideline leads. The writer no longer writes them;
    // answers stored with writer guidelines still show them first.
    let mut guidelines: Vec<AnyView> = w
        .guidelines
        .iter()
        .enumerate()
        .map(|(i, g)| {
            let n = g.pmid.as_ref().and_then(|p| refs.get(p).copied());
            // Stage 4 review 2: no PMID (none given, or removed by the
            // citation check): flagged, never presented as sourced.
            let unsourced = g.pmid.is_none();
            view! {
                <li>
                    {mk.t(&g.body, &format!("guidelines.{i}.body"))}
                    {g.year.map(|y| format!(" ({y})"))}
                    ": " {mk.t(&g.statement, &format!("guidelines.{i}.statement"))}
                    {n.map(ref_link)}
                    {unsourced.then(|| view! { " " <span class="flag-label">{NO_GUIDELINE_SOURCE}</span> })}
                </li>
            }
            .into_any()
        })
        .collect();
    // The skeptic's bodies keep their "not verified" marks (review 3).
    let sk = Marks {
        refs,
        spans: skeptic_spans,
        mark: Mark::NotVerified,
    };
    for (i, g) in skeptic_guidelines {
        if w.guidelines
            .iter()
            .any(|x| x.pmid.as_deref() == Some(g.pmid.as_str()))
        {
            continue;
        }
        // A reference when there is one, otherwise PubMed (as the leads).
        let link = match refs.get(&g.pmid) {
            Some(n) => ref_link(*n),
            None => pubmed_link(&g.pmid)
                .map(|href| {
                    view! { " " <a href=href rel="noopener noreferrer" target="_blank">{format!("PMID {}", g.pmid)}</a> }
                        .into_any()
                })
                .unwrap_or_else(|| ().into_any()),
        };
        let unsourced = g.pmid.trim().is_empty();
        guidelines.push(
            view! {
                <li>
                    {sk.t(&g.text, &format!("guidelines.{i}.text"))}
                    {g.quote.map(|q| view! { " \u{2014} " {cited(&format!("\u{201c}{q}\u{201d}"), refs)} })}
                    {link}
                    {unsourced.then(|| view! { " " <span class="flag-label">{NO_GUIDELINE_SOURCE}</span> })}
                </li>
            }
            .into_any(),
        );
    }
    for l in guideline_leads {
        let link = match refs.get(&l.pmid) {
            Some(n) => ref_link(*n),
            None => pubmed_link(&l.pmid)
                .map(|href| {
                    view! { " " <a href=href rel="noopener noreferrer" target="_blank">{format!("PMID {}", l.pmid)}</a> }
                        .into_any()
                })
                .unwrap_or_else(|| ().into_any()),
        };
        guidelines
            .push(view! { <li><span class="flag-label">{l.text()}</span> {link}</li> }.into_any());
    }
    // At a glance (Stage 4 item 8): about five lines, at the top.
    let glance = w.at_a_glance.as_ref().map(|g| {
        view! {
            <section class="box glance" aria-label="At a glance">
                <p><strong>"At a glance: "</strong> {mk.t(&g.verdict, "at_a_glance.verdict")}</p>
                {(!g.key_numbers.is_empty()).then(|| view! {
                    <ul>
                        {g.key_numbers
                            .iter()
                            .enumerate()
                            .map(|(i, k)| view! {
                                <li>
                                    {(!k.label.is_empty()).then(|| view! {
                                        {mk.t(&k.label, &format!("at_a_glance.key_numbers.{i}.label"))} ": "
                                    })}
                                    {mk.t(&k.value_text, &format!("at_a_glance.key_numbers.{i}.value_text"))}
                                    {refs.get(&k.pmid).copied().map(ref_link)}
                                </li>
                            })
                            .collect_view()}
                    </ul>
                })}
                {(!g.main_caveat.is_empty()).then(|| view! {
                    <p><strong>"Main caveat: "</strong> {mk.t(&g.main_caveat, "at_a_glance.main_caveat")}</p>
                })}
            </section>
        }
    });
    // What this means for clinical practice (Stage 4 item 9).
    let practice = w.clinical_practice.as_ref().map(|p| {
        // Labelled parts as a list; text under no label as written
        // (flat schema: parsed from labelled lines, never dropped).
        let parts: Vec<AnyView> = [
            ("Default choice", &p.default_choice, "default_choice"),
            ("Who may benefit", &p.who_may_benefit, "who_may_benefit"),
            ("Counselling", &p.counselling, "counselling"),
            ("Cost per case", &p.cost_per_case, "cost_per_case"),
        ]
        .into_iter()
        .filter(|(_, t, _)| !t.is_empty())
        .map(|(label, t, f)| {
            view! { <dt>{label}</dt><dd>{mk.t(t, &format!("clinical_practice.{f}"))}</dd> }.into_any()
        })
        .collect();
        view! {
            <h3>"What this means for clinical practice"</h3>
            {(!parts.is_empty()).then(|| view! { <dl>{parts}</dl> })}
            {(!p.text.is_empty()).then(|| view! { <p>{mk.t(&p.text, "clinical_practice.text")}</p> })}
        }
    });
    // Each outcome's structured numbers, by label and study, the
    // intervention arm first (Stage 4 item 10).
    let comparison = |p: &str| {
        w.studies
            .iter()
            .find(|s| s.pmid == p)
            .map(|s| s.comparison.as_str())
    };
    let arms: Vec<Vec<AnyView>> = w
        .outcomes
        .iter()
        .enumerate()
        .map(|(i, o)| {
            arm_lines(o, comparison)
                .into_iter()
                .map(|l| {
                    let flagged = l
                        .numbers
                        .iter()
                        .any(|j| whole_marked(&format!("outcomes.{i}.numbers.{j}"), untraced));
                    view! {
                        <li>
                            {arm_line_text(o, &l)}
                            {flagged.then(|| mark_view(Mark::NotTraced))}
                            {refs.get(&l.pmid).copied().map(ref_link)}
                        </li>
                    }
                    .into_any()
                })
                .collect()
        })
        .collect();
    let mut arms = arms.into_iter();
    view! {
        {glance}
        <h3>"Bottom line"</h3>
        <p>{mk.t(&w.bottom_line.text, "bottom_line.text")}</p>
        <p>
            <span class=c.class()>{format!("Certainty: {}", c.as_str())}</span>
            " " <span class="conf">{mk.t(&w.bottom_line.certainty_reason, "bottom_line.certainty_reason")}</span>
        </p>
        {practice}

        <h3>"Outcome by outcome"</h3>
        {if w.outcomes.is_empty() && comp.added_outcomes.is_empty() {
            view! { <p>"No outcome could be assessed from the evidence found."</p> }.into_any()
        } else {
            view! {
                <div class="scroll">
                    <table class="wide">
                        <thead>
                            <tr>
                                <th>"Outcome"</th>
                                <th>"RCT evidence"</th>
                                <th>"Effect"</th>
                                <th>"Certainty"</th>
                                <th>"Why"</th>
                            </tr>
                        </thead>
                        <tbody>
                            {w.outcomes
                                .iter()
                                .enumerate()
                                .map(|(i, o)| view! {
                                    <tr>
                                        <td>
                                            {mk.t(&o.outcome, &format!("outcomes.{i}.outcome"))}
                                            {o.evidence_type.label().map(|l| view! { <br /><span class="etype">{l}</span> })}
                                        </td>
                                        <td>{mk.t(&o.rct_evidence, &format!("outcomes.{i}.rct_evidence"))}</td>
                                        <td>
                                            {mk.t(&o.effect_text, &format!("outcomes.{i}.effect_text"))}
                                            {arms.next().filter(|a| !a.is_empty()).map(|a| view! { <ul class="arms">{a}</ul> })}
                                            {comp
                                                .additions_to(i)
                                                .map(|(k, a)| view! {
                                                    <p class="added">
                                                        {cm.t(&a.text, &format!("outcome_additions.{k}.text"))}
                                                        {ref_links(&not_cited_at_end(&a.text, &a.pmids), refs)}
                                                        {added()}
                                                        {unsourced(a.unsourced)}
                                                    </p>
                                                })
                                                .collect_view()}
                                        </td>
                                        <td><span class=o.certainty.class()>{o.certainty.as_str()}</span></td>
                                        <td>{mk.t(&o.why, &format!("outcomes.{i}.why"))}</td>
                                    </tr>
                                })
                                .collect_view()}
                            {comp
                                .added_outcomes
                                .iter()
                                .enumerate()
                                .map(|(k, x)| view! {
                                    <tr class="added">
                                        <td>
                                            {cm.t(&x.outcome, &format!("added_outcomes.{k}.outcome"))}
                                            <br />
                                            <span class="flag-label added">{ADDED_LABEL}</span>
                                        </td>
                                        <td>{ref_links(&x.pmids, refs)}</td>
                                        <td>
                                            {cm.t(&x.effect_text, &format!("added_outcomes.{k}.effect_text"))}
                                            {unsourced(x.unsourced)}
                                        </td>
                                        <td>
                                            {match x.certainty {
                                                Some(c) => view! { <span class=c.class()>{c.as_str()}</span> }.into_any(),
                                                None => view! { <span class="conf">"not graded"</span> }.into_any(),
                                            }}
                                        </td>
                                        <td>{cm.t(&x.why, &format!("added_outcomes.{k}.why"))}</td>
                                    </tr>
                                })
                                .collect_view()}
                        </tbody>
                    </table>
                </div>
            }
                .into_any()
        }}

        <h3>"The trials"</h3>
        {if w.studies.is_empty() {
            view! { <p>"None."</p> }.into_any()
        } else {
            view! {
                <div class="scroll">
                    <table class="wide">
                        <thead>
                            <tr>
                                <th>"Trial"</th>
                                <th>"Design / n / setting"</th>
                                <th>"Comparison"</th>
                                <th>"Key results"</th>
                                <th>"Appraisal"</th>
                                {full.then(|| view! { <th>"Evidence checked"</th> })}
                            </tr>
                        </thead>
                        <tbody>
                            {w.studies
                                .iter()
                                .enumerate()
                                .map(|(i, s)| {
                                    let n = refs.get(&s.pmid).copied();
                                    let e = evidence.get(s.pmid.trim());
                                    let checked = full.then(|| evidence_cell(e));
                                    let funding = e.and_then(|e| funding_view(e, refs));
                                    view! {
                                        <tr>
                                            <td>
                                                {mk.t(&s.label, &format!("studies.{i}.label"))}
                                                {n.map(ref_link)}
                                                {e.filter(|e| e.found_by_skeptic).map(|_| view! {
                                                    <br /><span class="flag-label added">{FOUND_BY_SKEPTIC}</span>
                                                })}
                                                {row_leads
                                                    .get(s.pmid.trim())
                                                    .into_iter()
                                                    .flatten()
                                                    .map(|t| view! { <br /><span class="flag-label">{format!("Skeptic: {t}")}</span> })
                                                    .collect_view()}
                                            </td>
                                            <td>
                                                {mk.t(&s.design, &format!("studies.{i}.design"))}
                                                {s.n.map(|n| format!(" \u{b7} n = {n}"))}
                                                {mk.whole(&format!("studies.{i}.n"))}
                                                " \u{b7} " {mk.t(&s.setting, &format!("studies.{i}.setting"))}
                                            </td>
                                            <td>{mk.t(&s.comparison, &format!("studies.{i}.comparison"))}</td>
                                            <td>{mk.t(&s.key_results, &format!("studies.{i}.key_results"))}</td>
                                            <td>
                                                {mk.t(&s.appraisal, &format!("studies.{i}.appraisal"))}
                                                {funding}
                                            </td>
                                            {checked.map(|c| view! { <td>{c}</td> })}
                                        </tr>
                                    }
                                })
                                .collect_view()}
                        </tbody>
                    </table>
                </div>
            }
                .into_any()
        }}

        <h3>"Conflicting evidence"</h3>
        {if w.conflicts.is_empty() && comp.added_conflicts.is_empty() {
            view! { <p>"None identified."</p> }.into_any()
        } else {
            view! {
                <ul>
                    {w.conflicts
                        .iter()
                        .enumerate()
                        .map(|(i, x)| view! { <li>{mk.t(&x.text, &format!("conflicts.{i}.text"))}</li> })
                        .collect_view()}
                    {comp
                        .added_conflicts
                        .iter()
                        .enumerate()
                        .map(|(k, x)| view! {
                            <li>
                                {cm.t(&x.text, &format!("added_conflicts.{k}.text"))}
                                {ref_links(&not_cited_at_end(&x.text, &x.pmids), refs)}
                                {added()}
                                {unsourced(x.unsourced)}
                            </li>
                        })
                        .collect_view()}
                </ul>
            }
                .into_any()
        }}

        {(!guidelines.is_empty()).then(|| view! {
            <h3>"Guidelines and consensus statements"</h3>
            <ul>{guidelines}</ul>
        })}

        <h3>"Applicability"</h3>
        <dl>
            <dt>"Case mix"</dt><dd>{mk.t(&w.applicability.case_mix, "applicability.case_mix")}</dd>
            <dt>"India and cost"</dt><dd>{mk.t(&w.applicability.india_cost_context, "applicability.india_cost_context")}</dd>
            <dt>"Training"</dt><dd>{mk.t(&w.applicability.training, "applicability.training")}</dd>
        </dl>

        <h3>"What would change this answer"</h3>
        <ul>
            {w.would_change
                .iter()
                .enumerate()
                .map(|(i, x)| view! { <li>{mk.t(x, &format!("would_change.{i}"))}</li> })
                .collect_view()}
        </ul>
    }
}

/// The reference links of `pmids` that have references, in order.
fn ref_links(pmids: &[String], refs: &std::collections::HashMap<String, u16>) -> AnyView {
    let mut n: Vec<u16> = pmids
        .iter()
        .filter_map(|p| refs.get(p.trim()).copied())
        .collect();
    n.sort_unstable();
    n.dedup();
    n.into_iter().map(ref_link).collect_view().into_any()
}

/// Stage 4b: the completeness review's checks line, its notes and,
/// collapsed, the gaps it judged not relevant (each with why).
fn completeness_view(
    c: Option<CompletenessV3>,
    refs: &std::collections::HashMap<String, u16>,
) -> Option<AnyView> {
    let c = c?;
    let cm = Marks {
        refs,
        spans: &c.untraced,
        mark: Mark::NotTraced,
    };
    let notes: Vec<AnyView> = c
        .notes
        .iter()
        .enumerate()
        .map(|(k, n)| {
            view! {
                <li>
                    {cm.t(n, &format!("notes.{k}"))}
                    " " <span class="flag-label added">"note from the completeness review"</span>
                </li>
            }
            .into_any()
        })
        .collect();
    let not_added: Vec<AnyView> = c
        .not_added
        .iter()
        .map(|n| view! { <li>{n.gap.clone()} " \u{2014} " {n.reason.clone()}</li> }.into_any())
        .collect();
    let n_not = not_added.len();
    Some(
        view! {
            <section class="checks" aria-label="Completeness review">
                <p class="hint">{c.line()}</p>
                {(!notes.is_empty()).then(|| view! { <ul>{notes}</ul> })}
                {(n_not > 0).then(|| view! {
                    <details>
                        <summary>{format!("Judged not relevant ({n_not})")}</summary>
                        <ul>{not_added}</ul>
                    </details>
                })}
            </section>
        }
        .into_any(),
    )
}

/// " [n]" linking reference `n`.
fn ref_link(n: u16) -> AnyView {
    view! { " " <span class="cite">"[" <a href=format!("#ref-{n}") rel="external">{n.to_string()}</a> "]"</span> }
        .into_any()
}

/// The trials table's "Evidence checked" cell: what was read, the verified
/// quotes, and (Stage 4 item 7) what was kept but not verified, labelled.
fn evidence_cell(e: Option<&EvidenceV3>) -> AnyView {
    let Some(e) = e else {
        return "not extracted".into_any();
    };
    let unverified: Vec<String> = e
        .unverified_details
        .iter()
        .map(|u| format!("{}: {}", u.outcome, u.effect_text))
        .collect();
    let numbers = (e.numbers_unverified > 0).then(|| {
        format!(
            " \u{b7} {} not verified",
            if e.numbers_unverified == 1 {
                "1 number".to_string()
            } else {
                format!("{} numbers", e.numbers_unverified)
            }
        )
    });
    let registration = e
        .trial_registration_unverified
        .as_ref()
        .map(|r| view! { " \u{b7} registration " {r.clone()} " " <span class="flag-label">"not in the source"</span> });
    view! {
        {evidence_line(e)}
        {numbers}
        {registration}
        {(!unverified.is_empty()).then(|| view! {
            <ul class="unverified">
                {unverified
                    .into_iter()
                    .map(|u| view! { <li>{u} " " <span class="flag-label">"not verified against the paper's wording"</span></li> })
                    .collect_view()}
            </ul>
        })}
    }
    .into_any()
}

/// What the extractor read for a study and how many quotes it verified.
fn evidence_line(e: &EvidenceV3) -> String {
    if e.text == "not extracted" {
        return "not extracted (abstract)".to_string();
    }
    format!(
        "{} \u{b7} {} verified {}",
        e.text,
        e.verified_quotes,
        if e.verified_quotes == 1 {
            "quote"
        } else {
            "quotes"
        }
    )
}

/// "Funding: …" from the extracted row (Stage 4 item 3), when it was
/// looked for; a number the source does not have is marked.
fn funding_view(e: &EvidenceV3, refs: &std::collections::HashMap<String, u16>) -> Option<AnyView> {
    if !e.funding_checked {
        return None;
    }
    let mk = Marks {
        refs,
        spans: &e.spans,
        mark: Mark::NotVerified,
    };
    Some(match &e.funding {
        Some(f) => {
            view! { <br /><span class="conf">"Funding: " {mk.t(f, "funding")}</span> }.into_any()
        }
        None => {
            let w = if e.text == "full text" {
                "text"
            } else {
                "abstract"
            };
            view! { <br /><span class="conf">{format!("Funding: not stated in the {w}")}</span> }
                .into_any()
        }
    })
}

/// The skeptic's review (Full): verdict, caveats, overlaps, missed trials,
/// meta-analysis pool notes, contradictions and guidelines, as escaped text
/// with numbered citations; a number no text its tools returned has carries
/// a "not verified" mark (Stage 4 item 7).
fn skeptic_v3_view(
    k: crate::agent::answer_v3::SkepticV3,
    refs: &std::collections::HashMap<String, u16>,
    on_rows: &std::collections::HashSet<String>,
) -> AnyView {
    let mk = Marks {
        refs,
        spans: &k.spans,
        mark: Mark::NotVerified,
    };
    // Each line: the skeptic's text (its path for marks, citations linked;
    // a verified quote after it) and the studies it names: a numbered
    // reference when there is one, otherwise a PubMed link (re-review 3: a
    // study whose record could not be fetched is not dropped). Only PMIDs
    // of 1 to 9 digits are linked.
    type Line = (String, String, Option<String>, Vec<String>);
    let item = |path: String, i: &SkepticItem| -> Line {
        (path, i.text.clone(), i.quote.clone(), vec![i.pmid.clone()])
    };
    let caveats: Vec<Line> = k
        .must_mention
        .iter()
        .enumerate()
        .map(|(i, m)| (format!("must_mention.{i}"), m.clone(), None, vec![]))
        .collect();
    let overlaps: Vec<Line> = k
        .overlaps
        .iter()
        .enumerate()
        .map(|(i, o)| {
            (
                format!("overlaps.{i}.note"),
                o.note.clone(),
                None,
                o.pmids.clone(),
            )
        })
        .collect();
    let picked: Vec<Line> = k
        .extract_next
        .iter()
        .enumerate()
        .map(|(i, x)| item(format!("extract_next.{i}.text"), x))
        .collect();
    let missed: Vec<Line> = k
        .missed_rcts
        .iter()
        .enumerate()
        .map(|(i, x)| item(format!("missed_rcts.{i}.text"), x))
        .collect();
    let pooled: Vec<Line> = k
        .ma_notes
        .iter()
        .enumerate()
        .map(|(i, x)| item(format!("ma_notes.{i}.text"), x))
        .collect();
    // Each with its verified quote (review 2).
    let against: Vec<Line> = k
        .contradicting
        .iter()
        .enumerate()
        .map(|(i, x)| item(format!("contradicting.{i}.text"), x))
        .chain(
            k.guidelines
                .iter()
                .enumerate()
                .map(|(i, x)| item(format!("guidelines.{i}.text"), x)),
        )
        .collect();
    // Unverified leads (review 6): PMID and kind only, no quote or claim.
    // Stage 4b item 5: a lead on a study with a trial row is drawn there.
    let leads: Vec<Line> = k
        .leads
        .iter()
        .filter(|l| !on_rows.contains(&l.pmid))
        .map(|l| (String::new(), l.text(), None, vec![l.pmid.clone()]))
        .collect();
    let study = |p: String| -> AnyView {
        if let Some(n) = refs.get(&p) {
            ref_link(*n)
        } else if (1..=9).contains(&p.len()) && p.bytes().all(|b| b.is_ascii_digit()) {
            let href = format!("https://pubmed.ncbi.nlm.nih.gov/{p}/");
            view! { " " <a href=href rel="noopener noreferrer" target="_blank">{format!("PMID {p}")}</a> }
                .into_any()
        } else {
            ().into_any()
        }
    };
    let list = |title: &'static str, xs: Vec<Line>| {
        (!xs.is_empty()).then(|| {
            view! {
                <p><strong>{title}</strong></p>
                <ul>
                    {xs
                        .into_iter()
                        .map(|(path, t, q, ps)| view! {
                            <li>
                                {mk.t(&t, &path)}
                                {q.map(|q| view! { " \u{2014} " {cited(&format!("\u{201c}{q}\u{201d}"), refs)} })}
                                {ps.into_iter().map(study).collect_view()}
                            </li>
                        })
                        .collect_view()}
                </ul>
            }
        })
    };
    view! {
        <section class="box" aria-label="Challenged by the skeptic">
            <h3>"Challenged by the skeptic"</h3>
            <p>{format!("Verdict: the conclusion {}.", k.verdict)}</p>
            {list("Caveats", caveats)}
            {list("Not independent", overlaps)}
            {list("Picked for extraction (second wave)", picked)}
            {list("Trials the search missed", missed)}
            {list("What the meta-analyses pooled", pooled)}
            {list("Evidence against, and guidelines", against)}
            {list("Unverified leads", leads)}
        </section>
    }
    .into_any()
}

/// A Phase 3 answer (AnswerV3), in the spec §2 order, as escaped text and
/// links only. The polished layout is Stage 4.
fn answer_v3_view(a: AnswerV3) -> impl IntoView {
    let refs = a.ref_numbers();
    let rerun = rerun_href(&a.question, a.mode);
    let cost = format!(
        "{} \u{b7} {}",
        rupees(a.cost_paise),
        duration_text(a.elapsed_s)
    );
    let notes = a.trail_notes();
    let untraced = a.untraced.clone();
    // Copy as text (Stage 4 item 10): the whole answer as plain text.
    let text = a.plain_text();
    let copy = RwSignal::new(CopyState::Ready);
    // Other notable findings (Stage 4 item 12): code's, then the writer's.
    let notable_writer: Vec<NotableFinding> = a
        .answer
        .as_ref()
        .map(|w| w.notable_findings.clone())
        .unwrap_or_default();
    // Stage 4b item 5: the skeptic's items are in its box only.
    let notable_code: Vec<NotableFinding> = a
        .notable_code
        .iter()
        .filter(|n| !n.repeats_skeptic())
        .cloned()
        .collect();
    let notable = notable_view(&notable_code, &notable_writer, &refs, &untraced);
    let n_searches = a.searches.len();
    // Full: per study, what the extractor read and checked.
    let evidence: std::collections::HashMap<String, EvidenceV3> = a
        .evidence
        .iter()
        .map(|e| (e.pmid.clone(), e.clone()))
        .collect();
    // Stage 4b item 5: leads on studies with a trial row are drawn on it.
    let on_rows: std::collections::HashSet<String> = a
        .skeptic
        .iter()
        .flat_map(|k| &k.leads)
        .filter(|l| a.lead_on_row(&l.pmid))
        .map(|l| l.pmid.clone())
        .collect();
    let mut row_leads: std::collections::HashMap<String, Vec<String>> =
        std::collections::HashMap::new();
    for p in &on_rows {
        row_leads.insert(
            p.clone(),
            a.leads_on_row(p).into_iter().map(|l| l.text()).collect(),
        );
    }
    let guideline_leads: Vec<crate::agent::answer_v3::SkepticLead> = a
        .skeptic
        .as_ref()
        .map(|k| {
            k.leads
                .iter()
                .filter(|l| l.kind == "guideline" && !on_rows.contains(&l.pmid))
                .cloned()
                .collect()
        })
        .unwrap_or_default();
    let skeptic_guidelines: Vec<(usize, SkepticItem)> = a
        .skeptic
        .as_ref()
        .map(|k| k.guidelines.iter().cloned().enumerate().collect())
        .unwrap_or_default();
    let skeptic_spans: Vec<Span> = a
        .skeptic
        .as_ref()
        .map(|k| k.spans.clone())
        .unwrap_or_default();
    let completeness = a.completeness.clone();
    let written = a.answer.map(|w| {
        written_v3_view(
            w,
            &refs,
            &untraced,
            &evidence,
            skeptic_guidelines,
            &skeptic_spans,
            guideline_leads,
            completeness.as_ref(),
            &row_leads,
        )
    });
    let checks = completeness_view(completeness.clone(), &refs);
    let skeptic = a.skeptic.map(|k| skeptic_v3_view(k, &refs, &on_rows));
    let run_notes = a.notes.clone();
    let empty = a.empty_reason.clone();
    view! {
        <section class="answer" aria-label="Answer">
            <p class="decision">{DECISION_SUPPORT}</p>
            <p class="meta">{format!("{} \u{b7} {}", mode_label(a.mode), date_ist(a.created_at))}</p>
            <p><a href=rerun rel="external">"Edit & re-run"</a></p>

            {match written {
                Some(w) => w.into_any(),
                None => view! {
                    <h3>"Bottom line"</h3>
                    <p>{empty.unwrap_or_else(|| answer::NO_EVIDENCE.to_string())}</p>
                }
                    .into_any(),
            }}

            {skeptic}
            {notable}
            {checks}
            {(!run_notes.is_empty())
                .then(|| view! {
                    <ul class="hint">{run_notes.into_iter().map(|n| view! { <li>{n}</li> }).collect_view()}</ul>
                })}

            <h3>"References"</h3>
            {if a.references.is_empty() {
                view! { <p>"None."</p> }.into_any()
            } else {
                view! {
                    <ul class="refs-list">
                        {a.references.into_iter().map(reference_v3_view).collect_view()}
                    </ul>
                }
                    .into_any()
            }}

            <details>
                <summary>{format!("Search trail ({n_searches} searches)")}</summary>
                <ul>
                    {a.searches
                        .into_iter()
                        .map(|s| view! { <li>{format!("{}: ", s.source)} <code>{s.query}</code> {format!(" \u{2014} {} hits", s.hits)}</li> })
                        .collect_view()}
                    {notes.into_iter().map(|n| view! { <li>{n}</li> }).collect_view()}
                </ul>
            </details>

            <h3>"Cost and time"</h3>
            <p>{cost}</p>

            <button
                class="secondary small"
                on:click=move |_| copy_answer(&text, copy)
            >
                {move || match copy.get() {
                    CopyState::Ready => "Copy as text",
                    CopyState::Copied => "Copied",
                    CopyState::Manual => "Select the text below and copy it",
                }}
            </button>
            // The fallback (shown only when the clipboard refused). It is
            // empty in the markup: Leptos does not escape a textarea's
            // children, so the text is set as its value in the click
            // handler, never written as HTML.
            <textarea
                id=COPY_AREA
                class=move || if copy.get() == CopyState::Manual { "copy-area shown" } else { "copy-area" }
                readonly=true
                aria-hidden=move || (copy.get() != CopyState::Manual).to_string()
                tabindex=move || if copy.get() == CopyState::Manual { "0" } else { "-1" }
            ></textarea>
        </section>
    }
}

/// A stored Phase 3 answer as the run page draws it (server only; for
/// tests and the Stage 4 replay).
#[cfg(feature = "ssr")]
pub fn render_answer_v3(a: AnswerV3) -> String {
    use leptos::tachys::view::RenderHtml;
    Owner::new().with(|| answer_v3_view(a).to_html())
}

/// The id of the hidden textarea holding the plain-text answer.
const COPY_AREA: &str = "answer-plain-text";

/// What the Copy button has done (set only in the browser).
#[derive(Clone, Copy, PartialEq, Eq)]
#[cfg_attr(not(feature = "hydrate"), allow(dead_code))]
enum CopyState {
    Ready,
    Copied,
    /// The clipboard refused: the textarea is shown, its text selected.
    Manual,
}

/// Copy `text` (Stage 4 item 10). In the click handler: the text is set as
/// the hidden textarea's value (a property, never HTML), then
/// `navigator.clipboard.writeText` is called; if the Clipboard API is missing or refuses, the selected
/// textarea is copied with `execCommand("copy")`, and if that fails too the
/// textarea is shown with its text selected so it can be copied by hand
/// (phones). Browser only.
fn copy_answer(text: &str, state: RwSignal<CopyState>) {
    #[cfg(feature = "hydrate")]
    {
        use wasm_bindgen::JsCast;
        use wasm_bindgen::prelude::Closure;
        use web_sys::js_sys::Reflect;
        let area = || {
            web_sys::window()
                .and_then(|w| w.document())
                .and_then(|d| d.get_element_by_id(COPY_AREA))
                .and_then(|e| e.dyn_into::<web_sys::HtmlTextAreaElement>().ok())
        };
        if let Some(a) = area() {
            a.set_value(text);
        }
        let select = move || -> bool {
            let Some(doc) = web_sys::window().and_then(|w| w.document()) else {
                return false;
            };
            let Some(area) = area() else {
                return false;
            };
            // iOS will not select inside a readonly textarea (review 8).
            area.set_read_only(false);
            area.set_tab_index(0);
            let _ = area.focus();
            area.select();
            let _ = area.set_selection_range(0, area.value().len() as u32);
            doc.dyn_into::<web_sys::HtmlDocument>()
                .ok()
                .and_then(|d| d.exec_command("copy").ok())
                .unwrap_or(false)
        };
        let Some(w) = web_sys::window() else {
            return;
        };
        let nav = w.navigator();
        let has_clipboard = Reflect::get(&nav, &"clipboard".into())
            .map(|c| !c.is_undefined() && !c.is_null())
            .unwrap_or(false);
        if !has_clipboard {
            state.set(if select() {
                CopyState::Copied
            } else {
                CopyState::Manual
            });
            return;
        }
        let ok = Closure::once(move |_: wasm_bindgen::JsValue| state.set(CopyState::Copied));
        let failed = Closure::once(move |_: wasm_bindgen::JsValue| {
            state.set(if select() {
                CopyState::Copied
            } else {
                CopyState::Manual
            });
        });
        let _ = nav.clipboard().write_text(text).then2(&ok, &failed);
        ok.forget();
        failed.forget();
    }
    #[cfg(not(feature = "hydrate"))]
    {
        let _ = (text, state);
    }
}

/// "Other notable findings from this run" (Stage 4 item 12): the items code
/// built from the team's checks, then the writer's, each with its studies.
fn notable_view(
    code: &[NotableFinding],
    writer: &[NotableFinding],
    refs: &std::collections::HashMap<String, u16>,
    untraced: &[Span],
) -> Option<AnyView> {
    if code.is_empty() && writer.is_empty() {
        return None;
    }
    let studies = |pmids: &[String]| -> AnyView {
        let mut n: Vec<u16> = pmids
            .iter()
            .filter_map(|p| refs.get(p.trim()).copied())
            .collect();
        n.sort_unstable();
        n.dedup();
        n.into_iter().map(ref_link).collect_view().into_any()
    };
    let wk = Marks {
        refs,
        spans: untraced,
        mark: Mark::NotTraced,
    };
    Some(
        view! {
            <h3>"Other notable findings from this run"</h3>
            <ul class="notable">
                {code
                    .iter()
                    .map(|n| {
                        let mk = Marks {
                            refs,
                            spans: &n.spans,
                            mark: Mark::NotVerified,
                        };
                        view! { <li>{mk.t(&n.text, "text")} {studies(&not_cited_at_end(&n.text, &n.pmids))}</li> }
                    })
                    .collect_view()}
                {writer
                    .iter()
                    .enumerate()
                    .map(|(i, n)| view! {
                        <li>
                            {wk.t(&n.text, &format!("notable_findings.{i}.text"))}
                            {studies(&not_cited_at_end(&n.text, &n.pmids))}
                        </li>
                    })
                    .collect_view()}
            </ul>
        }
        .into_any(),
    )
}

#[component]
fn HistoryPage() -> impl IntoView {
    let refresh = RwSignal::new(0u32);
    let runs = Resource::new(move || refresh.get(), |_| list_my_runs());
    let confirming = RwSignal::new(false);
    // A failed delete's message is cleared by the next action.
    let delete_one = Action::new(move |id: &Uuid| {
        let id = *id;
        async move {
            let r = delete_my_run(id).await;
            refresh.update(|n| *n += 1);
            r
        }
    });
    let delete_all = Action::new(move |_: &()| async move {
        let r = delete_my_runs().await;
        confirming.set(false);
        refresh.update(|n| *n += 1);
        r.map(|_| ())
    });
    let clear_errors = move || {
        delete_one.value().set(None);
        delete_all.value().set(None);
    };
    let failure = move || {
        let e = match (delete_one.value().get(), delete_all.value().get()) {
            (Some(Err(e)), _) | (_, Some(Err(e))) => Some(e),
            _ => None,
        };
        e.map(|e| view! { <p class="box amber" role="alert">{error_text(&e)}</p> })
    };
    view! {
        <h2>"My questions"</h2>
        <p class="hint">"Only you can see these. They are kept for 12 months."</p>
        {failure}
        <Suspense fallback=|| view! { <p>"Loading your questions…"</p> }>
            {move || {
                runs.get()
                    .map(|r| match r {
                        Ok(list) if list.is_empty() => {
                            view! { <p>"No questions yet. Ask one on the Ask page."</p> }.into_any()
                        }
                        Ok(list) => {
                            view! {
                                <ul class="runs">
                                    {list
                                        .into_iter()
                                        .map(|r| {
                                            let id = r.id;
                                            view! {
                                                <li>
                                                    <span>
                                                        <A href=format!("/runs/{id}")>{r.title}</A>
                                                        <br />
                                                        <span class="meta">
                                                            {r.created_at.format("%Y-%m-%d %H:%M UTC").to_string()}
                                                            " \u{b7} " {rupees(r.cost_paise)}
                                                        </span>
                                                    </span>
                                                    <span>
                                                        {status_badge(r.status)} " "
                                                        <button
                                                            class="secondary small"
                                                            on:click=move |_| {
                                                                clear_errors();
                                                                delete_one.dispatch(id);
                                                            }
                                                        >
                                                            "Delete"
                                                        </button>
                                                    </span>
                                                </li>
                                            }
                                        })
                                        .collect_view()}
                                </ul>
                            }
                                .into_any()
                        }
                        Err(e) => view! { <p class="box amber" role="alert">{error_text(&e)}</p> }.into_any(),
                    })
            }}
        </Suspense>
        {move || {
            if confirming.get() {
                view! {
                    <div class="box amber" role="alertdialog">
                        <p>"Delete all? This cannot be undone."</p>
                        <button
                            class="danger"
                            disabled=move || delete_all.pending().get()
                            on:click=move |_| {
                                clear_errors();
                                delete_all.dispatch(());
                            }
                        >
                            "Yes, delete all"
                        </button>
                        <button class="secondary" on:click=move |_| confirming.set(false)>
                            "Cancel"
                        </button>
                    </div>
                }
                    .into_any()
            } else {
                view! {
                    <button
                        class="secondary"
                        on:click=move |_| {
                            clear_errors();
                            confirming.set(true);
                        }
                    >
                        "Delete all my history"
                    </button>
                }
                    .into_any()
            }
        }}
    }
}

#[component]
fn ActivityPage() -> impl IntoView {
    let data = Resource::new(|| (), |_| get_activity());
    view! {
        <h2>"Activity"</h2>
        <Suspense fallback=|| view! { <p>"Loading the activity log…"</p> }>
            {move || {
                data.get()
                    .map(|r| match r {
                        Ok(v) => {
                            let class = if v.chain.status == "valid" {
                                "chain green"
                            } else {
                                "chain red"
                            };
                            view! {
                                <p class=class>{v.chain.label}</p>
                                <div class="scroll">
                                    <table>
                                        <thead>
                                            <tr>
                                                <th>"#"</th>
                                                <th>"Time (IST)"</th>
                                                <th>"User"</th>
                                                <th>"Event"</th>
                                                <th>"Detail"</th>
                                            </tr>
                                        </thead>
                                        <tbody>
                                            {v
                                                .entries
                                                .into_iter()
                                                .map(|e| {
                                                    view! {
                                                        <tr>
                                                            <td>{e.seq}</td>
                                                            <td>{e.time_ist}</td>
                                                            <td>{e.user_email.unwrap_or_else(|| "—".to_string())}</td>
                                                            <td>{e.event}</td>
                                                            <td>
                                                                <code>{e.detail}</code>
                                                            </td>
                                                        </tr>
                                                    }
                                                })
                                                .collect_view()}
                                        </tbody>
                                    </table>
                                </div>
                            }
                                .into_any()
                        }
                        Err(e) => view! { <p class="box amber">{error_text(&e)}</p> }.into_any(),
                    })
            }}
        </Suspense>
    }
}
