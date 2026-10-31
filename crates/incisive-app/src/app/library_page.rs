//! The Library (ivory redesign; today's "My questions" page at
//! `/history`): the user's runs grouped by question and IST day, searched
//! and filtered by certainty in the browser. Delete (one run, or all) uses
//! the existing server functions, each behind an inline confirm.

use leptos::prelude::*;

use super::ivory::{cert_view, spine_view, verdict_or_status};
use super::{
    CopyState, copy_answer, delete_my_run, delete_my_runs, error_text, get_my_run, list_my_library,
};
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
/// in the browser. Groups are built once per load; search and filter only
/// change which keyed cards show, so open panels and Copy state persist.
#[component]
pub fn LibraryPage() -> impl IntoView {
    let refresh = RwSignal::new(0u32);
    let runs = Resource::new(move || refresh.get(), |_| list_my_library());
    let query = RwSignal::new(String::new());
    let filter = RwSignal::new(CertaintyFilter::All);
    // Deleting (History's behaviour, unchanged): one run, or all after a
    // confirm; the list reloads after either, and a failure is shown.
    let delete_one = Action::new(move |id: &uuid::Uuid| {
        let id = *id;
        async move {
            let r = delete_my_run(id).await;
            refresh.update(|n| *n += 1);
            r
        }
    });
    let confirming = RwSignal::new(false);
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
    let on_delete = Callback::new(move |id: uuid::Uuid| {
        clear_errors();
        delete_one.dispatch(id);
    });
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
                let groups = StoredValue::new(lib::group_runs(cards));
                let shown = Memo::new(move |_| {
                    let f = filter.get();
                    query.with(|q| {
                        groups.with_value(|gs| {
                            gs.iter()
                                .filter(|g| lib::group_matches(g, f, q))
                                .cloned()
                                .collect::<Vec<RunGroup>>()
                        })
                    })
                });
                let days = Memo::new(move |_| {
                    shown.with(|gs| {
                        let mut d: Vec<chrono::NaiveDate> = Vec::new();
                        for g in gs {
                            let day = lib::ist_day(g.newest().created_at);
                            if d.last() != Some(&day) {
                                d.push(day);
                            }
                        }
                        d
                    })
                });
                let on_day = move |day: chrono::NaiveDate| {
                    shown.with(|gs| {
                        gs.iter()
                            .filter(|g| lib::ist_day(g.newest().created_at) == day)
                            .cloned()
                            .collect::<Vec<RunGroup>>()
                    })
                };
                view! {
                    {move || {
                        shown
                            .with(Vec::is_empty)
                            .then(|| view! {
                                <p class="iv-empty">
                                    "Nothing matches. " <a href="/">"Ask it as a new question"</a> "."
                                </p>
                            })
                    }}
                    <For each=move || days.get() key=|d| *d let:day>
                        <section class="iv-dayblock">
                            <div class="iv-day">
                                <h2 class="iv-kicker">{lib::day_heading(day, today)}</h2>
                                <span class="iv-day-sum">{move || lib::day_summary(&on_day(day))}</span>
                            </div>
                            <div class="iv-grid">
                                <For
                                    each=move || on_day(day)
                                    key=|g| (g.newest().id, g.runs.len())
                                    children=move |g| library_card(g, on_delete)
                                />
                            </div>
                        </section>
                    </For>
                }
                .into_any()
            }
            Err(e) => view! { <p class="box amber" role="alert">{error_text(&e)}</p> }.into_any(),
        })
    };
    let delete_all_view = move || {
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
                    class="secondary iv-delete-all"
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
            {failure}
            <Transition fallback=|| view! { <p class="iv-foot">"Loading your questions\u{2026}"</p> }>
                {list}
            </Transition>
            <p class="iv-foot">"Times in IST. Decision support \u{2014} the clinician decides."</p>
            <div class="iv-danger">{delete_all_view}</div>
        </div>
    }
}

fn library_card(g: RunGroup, on_delete: Callback<uuid::Uuid>) -> impl IntoView {
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
                    {delete_control(c.id, on_delete)}
                </div>
            </div>
        </article>
    }
}

/// Delete this card's (newest) run: a bin icon, then the inline confirm
/// "Delete?" with Keep and Delete (the same on a phone; no swipe).
fn delete_control(id: uuid::Uuid, on_delete: Callback<uuid::Uuid>) -> impl IntoView {
    let asking = RwSignal::new(false);
    view! {
        <span class="iv-del">
            {move || {
                if asking.get() {
                    view! {
                        <span class="iv-del-q" role="alert">"Delete?"</span>
                        <button type="button" class="iv-keep" on:click=move |_| asking.set(false)>
                            "Keep"
                        </button>
                        <button
                            type="button"
                            class="iv-del-yes"
                            on:click=move |_| {
                                asking.set(false);
                                on_delete.run(id);
                            }
                        >
                            "Delete"
                        </button>
                    }
                        .into_any()
                } else {
                    view! {
                        <button
                            type="button"
                            class="iv-del-icon"
                            aria-label="Delete this answer"
                            on:click=move |_| asking.set(true)
                        >
                            <svg width="16" height="16" viewBox="0 0 16 16" aria-hidden="true" focusable="false">
                                <path d="M3 4.5 H13 M6.5 4.5 V3 H9.5 V4.5 M4.5 4.5 L5.2 13 H10.8 L11.5 4.5" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linejoin="round" stroke-linecap="round"></path>
                            </svg>
                        </button>
                    }
                        .into_any()
                }
            }}
        </span>
    }
}

/// Copy as text from the Library: the run is fetched through the existing
/// run route (`get_my_run`) and its existing Copy-as-text is copied, with
/// the run page's fallbacks (`copy_answer`).
fn copy_button(id: uuid::Uuid) -> impl IntoView {
    let state = RwSignal::new(CopyState::Ready);
    let busy = RwSignal::new(false);
    let failed = RwSignal::new(false);
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
                failed.set(false);
                leptos::task::spawn_local(async move {
                    match get_my_run(id).await.ok().and_then(super::copy_text_of) {
                        // The run page's copy, with its manual-copy fallback.
                        Some(text) => copy_answer(&text, state, &area),
                        None => failed.set(true),
                    }
                    busy.set(false);
                });
            }
        >
            {move || match state.get() {
                CopyState::Ready if failed.get() => "Couldn\u{2019}t copy",
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
