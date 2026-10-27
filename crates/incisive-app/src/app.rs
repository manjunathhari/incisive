//! Pages: signed-in banner, Ask page, run page, history (My questions), Activity page (owner).
//!
//! The server functions here call the same code as the JSON routes
//! (`crate::ask::run_ask` and friends). All server-only imports live inside their
//! bodies, so this module also builds for the browser (`hydrate`).
//!
//! Everything shown is plain text: Leptos escapes it, and no `inner_html` is used.

use leptos::prelude::*;
use leptos_meta::{MetaTags, Title, provide_meta_context};
use leptos_router::{
    components::{A, Route, Router, Routes},
    hooks::{use_navigate, use_params_map},
    path,
};

use crate::ask::{ActivityView, AskOutcome, MeView};
use crate::runs::{RunMode, RunStatus, RunSummary, RunView};
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
.steps { padding-left:20px; } .steps li { margin:4px 0; }
.runs { list-style:none; padding:0; margin:12px 0; }
.runs li { display:flex; flex-wrap:wrap; gap:6px 12px; align-items:center; justify-content:space-between; padding:10px 12px; background:var(--card); border:1px solid var(--line); border-radius:6px; margin-bottom:8px; }
.runs .meta { color:var(--muted); font-size:.85rem; }
pre.answer { white-space:pre-wrap; word-break:break-word; background:var(--card); border:1px solid var(--line); border-radius:6px; padding:12px; font-size:.85rem; }
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
    crate::runs::api::list_view(&state, &user, None)
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
    let me = Resource::new(|| (), |_| get_me());
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

#[component]
fn AskPage() -> impl IntoView {
    let (question, set_question) = signal(String::new());
    let action = ServerAction::<AskQuestion>::new();
    let pending = action.pending();
    // Once the question is accepted, follow its run (browser only: an effect never runs on the server).
    let navigate = use_navigate();
    Effect::new(move |_| {
        if let Some(Ok(AskOutcome::RunStarted { run_id, .. })) = action.value().get() {
            navigate(&format!("/runs/{run_id}"), Default::default());
        }
    });
    let on_submit = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        action.dispatch(AskQuestion {
            question: question.get_untracked(),
            mode: RunMode::default(),
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
    format!("\u{20b9}{}.{:02}", paise / 100, paise % 100)
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
                None => Err(ServerFnError::new(
                    "That question wasn't found. It may have been deleted.",
                )),
            }
        },
    );

    // Poll while queued or running; stop at a terminal status. Effects run only
    // in the browser, so the server never starts a timer.
    let timer: StoredValue<Option<leptos::leptos_dom::helpers::IntervalHandle>, LocalStorage> =
        StoredValue::new_local(None);
    let stop = move || {
        if let Some(h) = timer.try_update_value(|t| t.take()).flatten() {
            h.clear();
        }
    };
    Effect::new(move |_| match data.get() {
        Some(Ok(v)) if v.summary.status.is_active() => {
            if timer.with_value(|t| t.is_none()) {
                let h = leptos::leptos_dom::helpers::set_interval_with_handle(
                    move || tick.update(|t| *t += 1),
                    POLL_EVERY,
                );
                timer.set_value(h.ok());
            }
        }
        Some(_) => stop(),
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
                        Err(e) => view! { <p class="box amber" role="alert">{error_text(&e)}</p> }.into_any(),
                    })
            }}
        </Transition>
    }
}

fn run_view(v: RunView) -> impl IntoView {
    let status = v.summary.status;
    let ended_without_answer = v.answer.is_none() && !status.is_active();
    // Placeholder until the real answer view (Task 13): the stored JSON, as text.
    let answer = v
        .answer
        .map(|a| serde_json::to_string_pretty(&a).unwrap_or_default());
    view! {
        <h2>{v.summary.title}</h2>
        <p>{status_badge(status)} " " <span class="meta">{rupees(v.summary.cost_paise)}</span></p>
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
            .map(|a| view! {
                <h3>"Answer"</h3>
                <pre class="answer">{a}</pre>
            })}
        {ended_without_answer
            .then(|| view! {
                <p class="box amber" role="status">
                    "This question ended without an answer. You can ask it again from the Ask page."
                </p>
            })}
    }
}

#[component]
fn HistoryPage() -> impl IntoView {
    let refresh = RwSignal::new(0u32);
    let runs = Resource::new(move || refresh.get(), |_| list_my_runs());
    let confirming = RwSignal::new(false);
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
                    <button class="secondary" on:click=move |_| confirming.set(true)>
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
