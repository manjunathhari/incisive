//! The Ask page (ivory redesign): the question box with PICO chips and
//! "Shape with PICO", the privacy line, starter chips, the Quick/Full radio
//! cards (no price), the latest answers, the Today card and the advisory
//! Question check. Submit, depth memory and Edit & re-run are unchanged.

use leptos::prelude::*;
use leptos_router::hooks::use_navigate;

use super::ivory::{asked_times, cert_view, spine_view, verdict_or_status};
use super::{
    AskQuestion, error_text, get_my_ask_overview, load_mode, outcome_view, save_mode,
    take_rerun_fragment,
};
use crate::agent::answer::{initial_mode, mode_label};
use crate::ask::AskOutcome;
use crate::library::{self as lib, ASK_LATEST, RunGroup, TodayCounts};
use crate::question_check::{self as qc, PICO_FIELDS, Pico, STARTERS};
use crate::runs::RunMode;

fn latest_card(g: RunGroup) -> impl IntoView {
    let c = g.newest().clone();
    let mut meta = lib::meta_line(&c);
    if g.runs.len() > 1 {
        meta.push_str(" \u{b7} ");
        meta.push_str(&asked_times(g.runs.len()));
    }
    view! {
        <a class="iv-lc" href=format!("/runs/{}", c.id)>
            {spine_view(c.certainty)}
            <span class="iv-lc-body">
                <span class="iv-lc-top">
                    <span class="iv-lc-topic">{c.question.clone()}</span>
                    <span class="iv-lc-cert">{cert_view(c.certainty)}</span>
                </span>
                {verdict_or_status(&c, "iv-lc-verdict")}
                <span class="iv-lc-meta">{meta}</span>
            </span>
        </a>
    }
}

// ---------- Ask ----------

/// Quick or Full: name, typical time, one line, the agent steps. No price.
struct Depth {
    mode: RunMode,
    time: &'static str,
    desc: &'static str,
    steps: &'static [&'static str],
    note: &'static str,
}

static DEPTHS: [Depth; 2] = [
    Depth {
        mode: RunMode::Quick,
        time: "3\u{2013}6 min",
        desc: "Abstracts only \u{b7} researcher and writer",
        steps: &["Researcher", "Writer", "Citation check"],
        note: "Fast first look from abstracts",
    },
    Depth {
        mode: RunMode::Focused,
        time: "4\u{2013}9 min",
        desc: "Full text \u{b7} numbers verified \u{b7} skeptic \u{b7} completeness",
        steps: &[
            "Researcher",
            "Extractor \u{d7}3",
            "Skeptic",
            "Writer",
            "Completeness",
        ],
        note: "Deeper check \u{b7} best for comparative questions",
    },
];

fn depth_of(m: RunMode) -> &'static Depth {
    match m {
        RunMode::Quick => &DEPTHS[0],
        RunMode::Focused => &DEPTHS[1],
    }
}

#[component]
pub fn AskPage() -> impl IntoView {
    let (question, set_question) = signal(String::new());
    // Full until the browser says otherwise (the server always renders Full).
    let mode = RwSignal::new(RunMode::default());
    let pico = RwSignal::new(Pico::default());
    let pico_open = RwSignal::new(false);
    let action = ServerAction::<AskQuestion>::new();
    let pending = action.pending();
    let navigate = use_navigate();
    let overview = Resource::new(|| (), |_| get_my_ask_overview());
    // Edit & re-run arrives as `/#q=<question>&mode=<mode>`; unchanged from
    // before the redesign (see `take_rerun_fragment`).
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
    let check = Memo::new(move |_| question.with(|q| pico.with(|p| qc::check(q, p))));

    let chips = move || {
        PICO_FIELDS
            .iter()
            .map(|(k, label, _)| {
                let k = *k;
                let value = move || pico.with(|p| p.get(k).trim().to_string());
                view! {
                    <span
                        class="iv-chip"
                        class:on=move || !value().is_empty()
                        title=move || {
                            let v = value();
                            if v.is_empty() { "Not set".to_string() } else { v }
                        }
                    >
                        <b>{k}</b>
                        <span class="iv-chip-label">{*label}</span>
                        <span class="iv-sr">{move || if value().is_empty() { " not set" } else { " set" }}</span>
                    </span>
                }
            })
            .collect_view()
    };

    let panel = move || {
        pico_open.get().then(|| {
            let fields = PICO_FIELDS
                .iter()
                .map(|(k, label, ph)| {
                    let k = *k;
                    let id = format!("pico-{k}");
                    view! {
                        <div class="iv-field">
                            <label for=id.clone()>{format!("{k} \u{b7} {label}")}</label>
                            <input
                                id=id
                                type="text"
                                placeholder=*ph
                                prop:value=move || pico.with(|p| p.get(k).to_string())
                                on:input=move |ev| {
                                    let v = event_target_value(&ev);
                                    pico.update(|p| match k {
                                        "P" => p.p = v,
                                        "I" => p.i = v,
                                        "C" => p.c = v,
                                        _ => p.o = v,
                                    });
                                }
                            />
                        </div>
                    }
                })
                .collect_view();
            view! {
                <div class="iv-pico-panel" id="pico-panel">
                    {fields}
                    <div class="iv-composed">
                        <p aria-live="polite">{move || pico.with(Pico::composed)}</p>
                        <button
                            type="button"
                            class="iv-primary"
                            disabled=move || !pico.with(Pico::can_compose)
                            on:click=move |_| {
                                if pico.with_untracked(Pico::can_compose) {
                                    set_question.set(pico.with_untracked(Pico::composed));
                                }
                            }
                        >
                            "Use this question"
                        </button>
                    </div>
                </div>
            }
        })
    };

    let starters = STARTERS
        .iter()
        .map(|s| {
            view! {
                <button
                    type="button"
                    class="iv-starter"
                    on:click=move |_| {
                        let p = s.to_pico();
                        set_question.set(p.composed());
                        pico.set(p);
                    }
                >
                    {s.short}
                </button>
            }
        })
        .collect_view();

    let depth_card = move |d: &'static Depth| {
        let m = d.mode;
        view! {
            <label class="iv-mode">
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
                <span class="iv-mode-card">
                    <span class="iv-mode-top">
                        <span class="iv-radio" aria-hidden="true"></span>
                        <span class="iv-mode-name">{mode_label(m)}</span>
                        <span class="iv-mode-time">{d.time}</span>
                    </span>
                    <span class="iv-mode-desc">{d.desc}</span>
                </span>
            </label>
        }
    };

    let steps = move || {
        let d = depth_of(mode.get());
        let n = d.steps.len();
        view! {
            <span>{format!("{} run:", mode_label(d.mode))}</span>
            {d
                .steps
                .iter()
                .enumerate()
                .map(|(i, s)| {
                    view! {
                        <span class="iv-step">{*s}</span>
                        {(i + 1 < n).then(|| view! { <span aria-hidden="true">"\u{2192}"</span> })}
                    }
                })
                .collect_view()}
            <span class="iv-steps-note">{d.note}</span>
        }
    };

    let latest = move || {
        overview.get().map(|r| match r {
            Ok(o) => {
                let groups: Vec<RunGroup> = lib::group_runs(o.recent)
                    .into_iter()
                    .take(ASK_LATEST)
                    .collect();
                if groups.is_empty() {
                    view! { <p class="iv-empty">"No answers yet. Your answers will appear here."</p> }
                        .into_any()
                } else {
                    view! {
                        <div class="iv-latest-grid">
                            {groups.into_iter().map(latest_card).collect_view()}
                        </div>
                    }
                    .into_any()
                }
            }
            Err(e) => view! { <p class="iv-empty">{error_text(&e)}</p> }.into_any(),
        })
    };

    let today = move || {
        overview.get().map(|r| match r {
            Ok(o) => today_view(o.today).into_any(),
            // A short error, never zeros that look like real counts.
            Err(_) => today_card(view! {
                <p class="iv-today-line">"Today\u{2019}s counts couldn\u{2019}t load."</p>
            })
            .into_any(),
        })
    };

    let check_view = move || {
        let c = check.get();
        let bar = c
            .items
            .iter()
            .map(|i| {
                let class = if i.warn {
                    "warn"
                } else if i.ok {
                    "ok"
                } else {
                    ""
                };
                view! { <span class=class></span> }
            })
            .collect_view();
        let items = c
            .items
            .into_iter()
            .map(|i| {
                let state = if i.warn {
                    "warn"
                } else if i.ok {
                    "ok"
                } else {
                    ""
                };
                let mark = if i.warn {
                    "!"
                } else if i.ok {
                    "\u{2713}"
                } else {
                    ""
                };
                let sr = if i.warn {
                    "Warning: "
                } else if i.ok {
                    "Done: "
                } else {
                    "Missing: "
                };
                view! {
                    <li>
                        <span class=format!("iv-dot {state}") aria-hidden="true">{mark}</span>
                        <span>
                            <span class="iv-sr">{sr}</span>
                            <span class=format!("iv-check-label {state}")>{i.label}</span>
                            <span class="iv-check-hint">{i.hint}</span>
                        </span>
                    </li>
                }
            })
            .collect_view();
        view! {
            <div class="iv-check-head">
                <h2 class="iv-kicker">"Question check"</h2>
                <span class="iv-check-score">{format!("{} of 6", c.score)}</span>
            </div>
            <div class="iv-check-bar" aria-hidden="true">{bar}</div>
            <ul>{items}</ul>
            <p class="iv-tip" aria-live="polite">{c.tip}</p>
        }
    };

    let qbox = view! {
                    <div class="iv-card iv-qbox">
                        <label class="iv-qlabel" for="question">"Your clinical question"</label>
                        <textarea
                            id="question"
                            name="question"
                            rows="3"
                            maxlength="4000"
                            placeholder="e.g. 52 y/o M, robotic LAR: does ICG angiography reduce anastomotic leak?"
                            aria-describedby="privacy-line"
                            prop:value=question
                            on:input=move |ev| set_question.set(event_target_value(&ev))
                        ></textarea>
                        <div class="iv-pico-row">
                            <span class="iv-pico-k">"PICO"</span>
                            {chips}
                            <button
                                type="button"
                                class="iv-ghost"
                                aria-expanded=move || pico_open.get().to_string()
                                aria-controls=move || pico_open.get().then_some("pico-panel")
                                on:click=move |_| pico_open.update(|o| *o = !*o)
                            >
                                {move || if pico_open.get() { "Hide PICO" } else { "Shape with PICO" }}
                            </button>
                        </div>
                        {panel}
                    </div>
    }
    .into_any();
    let depth = view! {
                    <fieldset class="iv-card iv-depth" aria-describedby="depth-hint">
                        <legend class="iv-sr">"Depth"</legend>
                        <div class="iv-depth-cards">{depth_card(&DEPTHS[0])} {depth_card(&DEPTHS[1])}</div>
                        <button
                            type="submit"
                            class="iv-send"
                            disabled=move || pending.get() || question.with(|q| q.trim().is_empty())
                        >
                            {move || {
                                if pending.get() {
                                    view! { "Checking\u{2026}" }.into_any()
                                } else {
                                    view! { "Read the evidence " <span aria-hidden="true">"\u{2192}"</span> }
                                        .into_any()
                                }
                            }}
                        </button>
                        <p class="iv-steps" id="depth-hint">{steps}</p>
                    </fieldset>
    }
    .into_any();
    let form = view! {
                <form class="iv-form" on:submit=on_submit>
                    {qbox}
                    <p class="iv-privacy" id="privacy-line">
                        <svg width="14" height="14" viewBox="0 0 16 16" aria-hidden="true" focusable="false">
                            <path d="M8 1.5 L13.5 3.5 V7.5 C13.5 11 11 13.4 8 14.5 C5 13.4 2.5 11 2.5 7.5 V3.5 Z" fill="none" stroke="#2C4A85" stroke-width="1.4" stroke-linejoin="round"></path>
                        </svg>
                        <span>
                            "No names, phone numbers, exact dates, addresses or hospital IDs. Age, sex and post-op day are fine."
                        </span>
                    </p>
                    <div class="iv-try">
                        <span class="iv-try-k">"Try"</span>
                        {starters}
                    </div>
                    // The steps line is announced with the group (and so with each choice).
                    {depth}
                </form>
    }
    .into_any();
    let latest_sec = view! {
                <section class="iv-latest" aria-labelledby="latest-title">
                    <div class="iv-latest-head">
                        <h2 class="iv-kicker" id="latest-title">"Latest answers"</h2>
                        <a href="/history">"All in Library \u{2192}"</a>
                    </div>
                    <Suspense fallback=|| view! { <p class="iv-foot">"Loading your latest answers\u{2026}"</p> }>
                        {latest}
                    </Suspense>
                </section>
    }
    .into_any();
    let aside = view! {
            <section class="iv-side" aria-label="Today and question check">
                <Suspense fallback=|| {
                    today_card(view! { <p class="iv-today-line">"Loading today\u{2019}s counts\u{2026}"</p> })
                }>{today}</Suspense>
                <div class="iv-card iv-check">
                    {check_view}
                    <p class="iv-advisory">
                        "Advice only \u{2014} it never blocks Send. Incisive checks for identifiers again when you send."
                    </p>
                </div>
                <p class="iv-foot">
                    "Decision support \u{2014} the clinician decides. Answers are kept for 12 months and only you can see them."
                </p>
            </section>
    }
    .into_any();
    view! {
        <div class="iv-wrap iv-ask">
            <section class="iv-ask-main" aria-labelledby="ask-title">
                <div>
                    <h1 class="iv-h1" id="ask-title">"What should the literature answer?"</h1>
                    <p class="iv-intro">
                        "Agents search PubMed and Europe PMC, verify each number, challenge the conclusion, and write one cited answer."
                    </p>
                </div>
                {form}
                {move || action.value().get().map(outcome_view)}
                {latest_sec}
            </section>
            {aside}
        </div>
    }
}

fn plural(n: u32, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

fn today_view(t: TodayCounts) -> impl IntoView {
    let longest = t.longest_s.map_or_else(
        || "No finished runs yet today".to_string(),
        |s| {
            format!(
                "Longest run {}",
                crate::agent::answer::duration_text(s.max(0) as u64)
            )
        },
    );
    today_card(view! {
        <div class="iv-today-big">
            <b>{plural(t.answers, "answer", "answers")}</b>
            <span>{plural(t.questions, "question", "questions")}</span>
        </div>
        <p class="iv-today-line">{longest}</p>
    })
}

/// The Today card around `body` (counts, a loading line or an error).
fn today_card(body: impl IntoView) -> impl IntoView {
    view! {
        <div class="iv-card iv-today">
            <h2 class="iv-kicker">"Today"</h2>
            {body}
        </div>
    }
}
