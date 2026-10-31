//! The Library (ivory redesign; today's "My questions" page at
//! `/history`): the user's runs grouped by question and IST day, searched
//! and filtered by certainty in the browser. No delete (owner decision 4).

use leptos::prelude::*;

use super::ivory::{cert_view, spine_view, verdict_or_status};
use super::{CopyState, copy_answer, error_text, get_my_run, list_my_library};
use crate::agent::answer::{mode_label, rerun_href};
use crate::library::{self as lib, CertaintyFilter, FILTERS, RunGroup};

// ---------- Library ----------

fn filter_label(f: CertaintyFilter) -> AnyView {
    match f {
        CertaintyFilter::All => view! { "All" }.into_any(),
        CertaintyFilter::Level(l) => view! {
            <span class="iv-pips" aria-hidden="true">{lib::pips(l)}</span>
            " "
            {lib::certainty_name(l)}
        }
        .into_any(),
    }
}

/// The Library: the user's runs (as History had them: the newest 100),
/// grouped by question, by IST day, searchable and filtered by certainty
/// in the browser. No delete (owner decision 4).
#[component]
pub fn LibraryPage() -> impl IntoView {
    let runs = Resource::new(|| (), |_| list_my_library());
    let query = RwSignal::new(String::new());
    let filter = RwSignal::new(CertaintyFilter::All);
    let pills = FILTERS
        .iter()
        .map(|f| {
            let f = *f;
            view! {
                <button
                    type="button"
                    class="iv-filter"
                    aria-pressed=move || (filter.get() == f).to_string()
                    on:click=move |_| filter.set(f)
                >
                    {filter_label(f)}
                </button>
            }
        })
        .collect_view();
    let list = move || {
        runs.get().map(|r| match r {
            Ok(cards) if cards.is_empty() => {
                view! { <p class="iv-empty">"No questions yet. Ask one on the Ask page."</p> }
                    .into_any()
            }
            Ok(cards) => {
                let today = lib::ist_day(lib::now_utc());
                let groups = lib::group_runs(cards);
                let shown: Vec<RunGroup> = {
                    let f = filter.get();
                    let q = query.get();
                    groups
                        .into_iter()
                        .filter(|g| lib::group_matches(g, f, &q))
                        .collect()
                };
                if shown.is_empty() {
                    return view! {
                        <p class="iv-empty">
                            "Nothing matches. " <a href="/">"Ask it as a new question"</a> "."
                        </p>
                    }
                    .into_any();
                }
                lib::by_day(shown)
                    .into_iter()
                    .map(|(day, gs)| {
                        let sum = lib::day_summary(&gs);
                        view! {
                            <section class="iv-dayblock">
                                <div class="iv-day">
                                    <h2 class="iv-kicker">{lib::day_heading(day, today)}</h2>
                                    <span class="iv-day-sum">{sum}</span>
                                </div>
                                <div class="iv-grid">{gs.into_iter().map(library_card).collect_view()}</div>
                            </section>
                        }
                    })
                    .collect_view()
                    .into_any()
            }
            Err(e) => view! { <p class="box amber" role="alert">{error_text(&e)}</p> }.into_any(),
        })
    };
    view! {
        <div class="iv-wrap iv-lib">
            <div class="iv-lib-head">
                <div>
                    <h1 class="iv-h1">"Library"</h1>
                    <p class="iv-intro">
                        "Every answer leads with its verdict. Only you can see these; they are kept for 12 months."
                    </p>
                </div>
                <a class="iv-new" href="/">
                    "New question " <span aria-hidden="true">"\u{2192}"</span>
                </a>
            </div>
            <div class="iv-card iv-tools">
                <label for="find" class="iv-sr">"Search your answers"</label>
                <div class="iv-search">
                    <svg width="16" height="16" viewBox="0 0 16 16" aria-hidden="true" focusable="false">
                        <circle cx="7" cy="7" r="4.6" fill="none" stroke="#6B675F" stroke-width="1.5"></circle>
                        <path d="M10.5 10.5 L14 14" stroke="#6B675F" stroke-width="1.5" stroke-linecap="round"></path>
                    </svg>
                    <input
                        id="find"
                        type="search"
                        placeholder="Search questions and verdicts"
                        prop:value=query
                        on:input=move |ev| query.set(event_target_value(&ev))
                    />
                </div>
                <div class="iv-filters" role="group" aria-label="Certainty">{pills}</div>
            </div>
            <Suspense fallback=|| view! { <p class="iv-foot">"Loading your questions\u{2026}"</p> }>
                {list}
            </Suspense>
            <p class="iv-foot">"Times in IST. Decision support \u{2014} the clinician decides."</p>
        </div>
    }
}

fn library_card(g: RunGroup) -> impl IntoView {
    let c = g.newest().clone();
    let href = format!("/runs/{}", c.id);
    let thread = (g.runs.len() > 1).then(|| {
        let n = g.runs.len();
        let rows = g
            .runs
            .iter()
            .enumerate()
            .map(|(i, r)| {
                let day = lib::ist_day(r.created_at);
                let time = if day == lib::ist_day(c.created_at) {
                    lib::time_ist(r.created_at)
                } else {
                    format!("{} {}", day.format("%-d %b"), lib::time_ist(r.created_at))
                };
                let note = if i == 0 {
                    format!("Latest \u{b7} {}", super::status_label(r.status))
                } else {
                    super::status_label(r.status).to_string()
                };
                view! {
                    <li>
                        <span class="iv-thread-time">{time}</span>
                        <span class="iv-thread-note">{note}</span>
                        <a href=format!("/runs/{}", r.id)>"Open"</a>
                    </li>
                }
            })
            .collect_view();
        view! {
            <details class="iv-thread">
                <summary>
                    <span class="iv-dots" aria-hidden="true">
                        {(0..n.min(6)).map(|_| view! { <span></span> }).collect_view()}
                    </span>
                    <span class="iv-thread-n">{format!("Asked {n} times")}</span>
                    <span class="iv-thread-sub">"newest shown"</span>
                    <span class="iv-thread-toggle" aria-hidden="true"></span>
                </summary>
                <ol>{rows}</ol>
            </details>
        }
    });
    let copy = c.has_answer.then(|| copy_button(c.id));
    view! {
        <article class="iv-libcard">
            {spine_view(c.certainty)}
            <div class="iv-lib-body">
                <div class="iv-lib-top">
                    <span class="iv-modepill">{mode_label(c.mode)}</span>
                    <span>{lib::meta_line(&c)}</span>
                    <span class="iv-lib-cert">{cert_view(c.certainty)}</span>
                </div>
                <a class="iv-lib-main" href=href.clone()>
                    <span class="iv-lib-q">{c.question.clone()}</span>
                    {verdict_or_status(&c, "iv-lib-verdict")}
                </a>
                {thread}
                <div class="iv-actions">
                    <a class="iv-read" href=href>"Read answer"</a>
                    <a href=rerun_href(&c.question, c.mode) rel="external">"Edit & re-run"</a>
                    {copy}
                </div>
            </div>
        </article>
    }
}

/// Copy as text from the Library: the run is fetched through the existing
/// run route (`get_my_run`) and its existing Copy-as-text is copied, with
/// the run page's fallbacks (`copy_answer`).
fn copy_button(id: uuid::Uuid) -> impl IntoView {
    let state = RwSignal::new(CopyState::Ready);
    let busy = RwSignal::new(false);
    let area = format!("copy-{id}");
    let area_click = area.clone();
    view! {
        <button
            type="button"
            class="iv-copy"
            disabled=move || busy.get()
            on:click=move |_| {
                let area = area_click.clone();
                busy.set(true);
                leptos::task::spawn_local(async move {
                    if let Ok(v) = get_my_run(id).await
                        && let Some(text) = super::copy_text_of(v)
                    {
                        copy_answer(&text, state, &area);
                    }
                    busy.set(false);
                });
            }
        >
            {move || match state.get() {
                CopyState::Ready => if busy.get() { "Copying\u{2026}" } else { "Copy" },
                CopyState::Copied => "Copied",
                CopyState::Manual => "Select the text below and copy it",
            }}
        </button>
        <textarea
            id=area
            class=move || if state.get() == CopyState::Manual { "copy-area shown" } else { "copy-area" }
            readonly=true
            aria-hidden=move || (state.get() != CopyState::Manual).to_string()
            tabindex=move || if state.get() == CopyState::Manual { "0" } else { "-1" }
        ></textarea>
    }
}
