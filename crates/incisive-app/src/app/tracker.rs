//! The run tracker's views (UI only; the rules are in
//! [`crate::run_tracker`]): the results card, the pills' looks, and the
//! folded Progress pill with its rolling step.
//!
//! Everything is drawn from the stored step lines and the run's status.
//! The page is drawn again at each poll, so what must last across a redraw
//! (the clock, "Show all", the clicked pill, the steps already seen) lives
//! in [`TrackerUi`], made once by the run page. Escaped text only; no
//! `inner_html`. Nothing here logs question or reply text.

use chrono::{DateTime, Utc};
use leptos::prelude::*;

use crate::library::now_utc;
use crate::run_tracker::{self as rt, Fill, PillState, Row, Tracker, Value};
use crate::runs::{RunMode, StepLine};

/// The tracker's styles. In the page's head, so a redraw never flashes.
pub const TRACKER_STYLE: &str = r##"
.rx-seg { display:grid; grid-auto-flow:column; grid-auto-columns:minmax(0,1fr); background:var(--iv-card); border:1px solid var(--iv-border); border-radius:999px; padding:4px; gap:4px; margin:4px 0 12px; font-family:var(--iv-font); }
.rx-seg .rx-tab { font:inherit; font-size:14px; font-weight:600; border:0; border-radius:999px; padding:6px 10px; min-height:44px; background:var(--iv-card); color:#6B675F; cursor:pointer; display:flex; flex-wrap:wrap; align-items:center; justify-content:center; gap:7px; text-align:center; text-decoration:none; min-width:0; }
.rx-seg .rx-tab[disabled] { cursor:default; opacity:1; }
.rx-seg .rx-tab[data-state="filling"] { color:#56688F; }
.rx-seg .rx-tab[data-state="ready"] { background:#E8F1EC; color:#557060; }
.rx-seg .rx-tab[data-clicked="true"] { box-shadow:inset 0 0 0 1px #A9B8D6; }
.rx-seg .rx-dot, .rx-seg .rk-dot { flex:none; width:8px; height:8px; border-radius:999px; background:#5E8A72; }
.rk-card { margin:0 0 10px; padding:20px 24px; background:var(--iv-card); border:1px solid var(--iv-border); border-radius:20px; color:var(--iv-ink); }
.rk-head { display:flex; align-items:flex-start; justify-content:space-between; gap:16px; padding-bottom:14px; }
.rk-head > div { min-width:0; }
.run-chrome .rk-card h2 { margin:0; font-size:19px; line-height:1.25; font-weight:700; color:var(--iv-ink); }
.rk-now { display:flex; align-items:center; gap:7px; margin-top:6px; font-size:14px; line-height:1.3; color:var(--iv-body); }
.rk-pulse { flex:none; width:9px; height:9px; border-radius:999px; background:var(--iv-accent); animation:rk-pulse 1.6s ease-in-out infinite; }
@keyframes rk-pulse { 0%,100% { opacity:1; transform:scale(1); } 50% { opacity:.35; transform:scale(.8); } }
.rk-clock-box { flex:none !important; text-align:right; }
.rk-clock { font-size:30px; line-height:1.1; font-weight:700; font-variant-numeric:tabular-nums; }
.rk-usual { font-size:12px; color:var(--iv-muted); }
.rk-grid { display:grid; grid-template-columns:repeat(auto-fit,minmax(220px,1fr)); column-gap:32px; }
.rk-cell { padding:14px 0; border-top:1px solid var(--iv-rule); min-width:0; }
.rk-name { font-size:14px; font-weight:600; color:var(--iv-ink); }
.rk-name.off { color:var(--iv-muted); font-weight:500; }
.rk-sub { font-size:12px; line-height:1.35; color:var(--iv-muted); margin-top:3px; }
.rk-val { font-size:30px; line-height:1.1; font-weight:700; font-variant-numeric:tabular-nums; margin-top:4px; }
.rk-of { font-size:15px; font-weight:500; color:var(--iv-muted); }
.rk-state { font-size:15px; line-height:1.3; font-weight:600; color:var(--iv-accent); margin-top:8px; }
.rk-pend { font-size:15px; line-height:1.3; font-weight:500; color:var(--iv-muted); margin-top:8px; }
.rk-bar { height:6px; margin-top:8px; border-radius:999px; background:var(--iv-off); overflow:hidden; }
.rk-bar > span { display:block; height:100%; border-radius:999px; background:var(--iv-accent); }
.rk-tag { margin:12px 0 0; padding:10px 14px; border-radius:12px; background:var(--iv-tint); color:var(--iv-accent); font-size:14px; line-height:1.4; font-weight:600; }
.rk-leave { margin:14px 0 0; padding-top:12px; border-top:1px solid var(--iv-rule); font-size:13px; line-height:1.4; color:var(--iv-body); }
.rk-leave a { color:var(--iv-accent); }
.rk-fold { display:block; margin:12px 0; padding:0 18px; background:var(--iv-card); border:1px solid var(--iv-border); border-radius:24px; font-family:var(--iv-font); }
.rk-fold > summary { list-style:none; min-height:48px; display:flex; align-items:center; gap:12px; cursor:pointer; }
.rk-fold > summary::-webkit-details-marker { display:none; }
.rk-fold-title { flex:none; font-size:15px; font-weight:600; color:var(--iv-ink); }
.rk-role { flex:none; padding:3px 10px; border-radius:999px; background:#EFF2F9; color:#56688F; font-size:12px; font-weight:600; }
.rk-roll { flex:1; min-width:0; height:22px; overflow:hidden; display:block; }
.rk-roll-in { display:block; }
.rk-roll-in.moved { transform:translateY(-22px); animation:rk-roll .45s ease; }
@keyframes rk-roll { from { transform:translateY(0); } to { transform:translateY(-22px); } }
.rk-roll-in > span { display:block; height:22px; line-height:22px; font-size:13px; color:var(--iv-body); white-space:nowrap; overflow:hidden; text-overflow:ellipsis; }
.rk-more { flex:none; font-size:13px; font-weight:600; color:var(--iv-accent); }
.rk-more::after { content:'Show all'; }
.rk-fold[open] .rk-more::after { content:'Show less'; }
.rk-fold .steps { margin:0 0 14px; font-size:14px; color:var(--iv-body); }
@media (max-width:640px) {
  .rx-seg .rx-tab { font-size:13px; padding:6px 6px; }
  .rk-card { padding:14px 16px 12px; }
  .run-chrome .rk-card h2 { font-size:17px; }
  .rk-now { font-size:13px; margin-top:5px; }
  .rk-head { gap:12px; padding-bottom:12px; }
  .rk-clock { font-size:24px; }
  .rk-grid { grid-template-columns:minmax(0,1fr); }
  .rk-cell { padding:10px 0; display:grid; grid-template-columns:minmax(0,1fr) auto; column-gap:12px; align-items:baseline; }
  .rk-name, .rk-sub { grid-column:1; }
  .rk-sub { margin-top:2px; }
  .rk-val, .rk-state, .rk-pend { grid-column:2; grid-row:1; margin:0; text-align:right; }
  .rk-val { font-size:22px; }
  .rk-of { font-size:14px; }
  .rk-state, .rk-pend { font-size:13px; }
  .rk-bar { grid-column:1 / -1; margin-top:7px; }
  .rk-leave { margin-top:0; padding-top:10px; }
  .rk-tag { margin-top:6px; padding:10px 12px; }
  .rk-card[data-done="true"] { padding:14px 16px 6px; }
  .rk-card[data-done="true"] .rk-grid { grid-template-columns:repeat(2,minmax(0,1fr)); column-gap:16px; }
  .rk-card[data-done="true"] .rk-cell { display:block; padding:9px 0; }
  .rk-card[data-done="true"] .rk-name { font-size:12px; line-height:1.3; font-weight:500; color:var(--iv-muted); }
  .rk-card[data-done="true"] .rk-sub { display:none; }
  .rk-card[data-done="true"] .rk-val { font-size:20px; line-height:1.15; margin-top:2px; text-align:left; }
  .rk-card[data-done="true"] .rk-of { font-size:13px; }
  .rk-card[data-done="true"] .rk-state, .rk-card[data-done="true"] .rk-pend { font-size:14px; line-height:1.25; color:var(--iv-ink); margin-top:3px; text-align:left; }
  .rk-fold { padding:0 16px; border-radius:20px; margin:10px 0; }
  .rk-fold > summary { display:grid; grid-template-columns:auto minmax(0,1fr) auto; align-items:center; gap:6px 8px; padding:10px 0; }
  .rk-fold-title { grid-column:1 / 3; grid-row:1; }
  .rk-more { grid-column:3; grid-row:1; }
  .rk-role { grid-column:1; grid-row:2; padding:3px 9px; }
  .rk-roll { grid-column:2 / 4; grid-row:2; }
}
@media (prefers-reduced-motion:reduce) {
  .rk-pulse, .rk-roll-in.moved { animation:none; }
}
"##;

/// A pill's name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Pill {
    Answer,
    Expert,
    Chat,
}

/// The tracker's state across the page's redraws.
#[derive(Clone, Copy)]
pub(super) struct TrackerUi {
    /// The clock: moved once a second in the browser.
    pub now: RwSignal<DateTime<Utc>>,
    show_all: RwSignal<bool>,
    /// The pill last clicked: the only one with a border.
    pub clicked: RwSignal<Option<Pill>>,
    /// The steps at the last draw: a new one rolls in.
    seen: StoredValue<Option<usize>>,
}

impl TrackerUi {
    /// Made once by the run page. The clock runs in the browser only.
    pub fn new() -> TrackerUi {
        let now = RwSignal::new(now_utc());
        Effect::new(move |_| {
            let h = leptos::leptos_dom::helpers::set_interval_with_handle(
                move || now.set(now_utc()),
                std::time::Duration::from_secs(1),
            );
            if let Ok(h) = h {
                on_cleanup(move || h.clear());
            }
        });
        TrackerUi {
            now,
            show_all: RwSignal::new(false),
            clicked: RwSignal::new(None),
            seen: StoredValue::new(None),
        }
    }

    /// `data-clicked` for `pill`.
    pub fn clicked_attr(self, pill: Pill) -> impl Fn() -> &'static str + Copy + Send + Sync {
        move || {
            if self.clicked.get() == Some(pill) {
                "true"
            } else {
                "false"
            }
        }
    }
}

/// The Answer pill, for the bars.
#[derive(Clone, Copy)]
pub(super) struct AnswerPill {
    pub state: PillState,
    pub fill: Fill,
    /// When the answer was ready (the Expert Opinion starts then).
    pub ready_at: DateTime<Utc>,
}

impl AnswerPill {
    /// The pill's fill, by the clock.
    pub fn style(self, ui: TrackerUi) -> impl Fn() -> String + Copy + Send + Sync {
        move || match self.state {
            PillState::Filling => rt::fill_background(self.fill.at(ui.now.get())),
            _ => String::new(),
        }
    }

    pub fn dot(self) -> Option<AnyView> {
        (self.state == PillState::Ready)
            .then(|| view! { <span class="rk-dot" aria-hidden="true"></span> }.into_any())
    }
}

/// Chat with me where it cannot be opened: visible, white, not clickable.
pub(super) fn chat_pill_off() -> AnyView {
    let label = "Chat with me";
    view! {
        <button
            class="rx-tab"
            type="button"
            role="tab"
            aria-selected="false"
            aria-label=PillState::Idle.aria(label)
            data-state=PillState::Idle.as_str()
            disabled=true
        >
            {label}
        </button>
    }
    .into_any()
}

/// The bar on a page with no expert opinion row: a Quick run, or a Full run
/// still working or with no row. Answer, Expert opinion on Full, and
/// Chat with me (visible, not clickable).
pub(super) fn plain_bar(ui: TrackerUi, mode: RunMode, answer: AnswerPill) -> AnyView {
    let to_answer = move |ev: leptos::ev::MouseEvent| {
        ev.prevent_default();
        ui.clicked.set(Some(Pill::Answer));
    };
    let expert = (mode == RunMode::Focused).then(|| {
        let label = "Expert opinion";
        view! {
            <button
                class="rx-tab"
                type="button"
                role="tab"
                aria-selected="false"
                aria-label=PillState::Idle.aria(label)
                data-state=PillState::Idle.as_str()
                disabled=true
            >
                {label}
            </button>
        }
        .into_any()
    });
    view! {
        <div class="rx-seg" role="tablist" aria-label="Run views">
            <a
                class="rx-tab"
                role="tab"
                aria-selected="true"
                aria-label=answer.state.aria("Answer")
                data-state=answer.state.as_str()
                data-clicked=ui.clicked_attr(Pill::Answer)
                style=answer.style(ui)
                href="#answer"
                rel="external"
                on:click=to_answer
            >
                {answer.dot()}
                "Answer"
            </a>
            {expert}
            {chat_pill_off()}
        </div>
    }
    .into_any()
}

/// One row of the card.
fn row_view(r: Row) -> AnyView {
    let off = matches!(r.value, Value::Pending | Value::NotReached);
    let name_class = if off { "rk-name off" } else { "rk-name" };
    let value = match r.value {
        Value::Number { n, of } => view! {
            <div class="rk-val">
                {n.to_string()}
                {of.map(|of| view! { " " <span class="rk-of">{format!("of {of}")}</span> })}
            </div>
        }
        .into_any(),
        Value::State(s) => view! { <div class="rk-state">{s}</div> }.into_any(),
        Value::Pending => view! { <div class="rk-pend">"pending"</div> }.into_any(),
        Value::NotReached => view! { <div class="rk-pend">"not reached"</div> }.into_any(),
    };
    let bar = r.bar.map(|pct| {
        let width = format!("width:{pct}%");
        view! { <div class="rk-bar"><span style=width></span></div> }
    });
    let sub = r
        .sub
        .filter(|_| !off)
        .map(|s| view! { <div class="rk-sub">{s}</div> });
    view! {
        <div class="rk-cell">
            <div class=name_class>{r.name}</div>
            {value}
            {bar}
            {sub}
        </div>
    }
    .into_any()
}

/// The results card: `working` while the run is queued or running, with the
/// clock moving from `created_at`; afterwards with `total_secs`.
pub(super) fn results_card(
    ui: TrackerUi,
    mode: RunMode,
    tracker: Tracker,
    working: bool,
    waiting: bool,
    created_at: DateTime<Utc>,
    total_secs: i64,
) -> AnyView {
    let secs = move || {
        if waiting {
            0
        } else if working {
            (ui.now.get() - created_at).num_seconds()
        } else {
            total_secs
        }
    };
    let ended_well = !working && tracker.sentence == "Answer ready";
    let title = if working { "Results so far" } else { "Results" };
    // A redraw keeps the pulse in step with the clock's own beat.
    let pulse = format!(
        "animation-delay:-{}ms",
        ui.now.get_untracked().timestamp_millis() % 1600
    );
    let mark = if working {
        view! { <span class="rk-pulse" aria-hidden="true" style=pulse></span> }.into_any()
    } else if ended_well {
        view! {
            <svg width="16" height="16" viewBox="0 0 16 16" aria-hidden="true" focusable="false" style="flex:none">
                <path d="M3 8.5 L6.5 12 L13 4.5" fill="none" stroke="#1B6B3A" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"></path>
            </svg>
        }
        .into_any()
    } else {
        ().into_any()
    };
    let usual = if working {
        rt::usual_label(mode)
    } else {
        "total time"
    };
    let tagline = move || {
        (working && rt::tagline_due(mode, secs()))
            .then(|| view! { <p class="rk-tag">{rt::TAGLINE}</p> })
    };
    let leave = working.then(|| {
        view! {
            <p class="rk-leave">
                {rt::LEAVE_LINE} <a href="/history">"Library"</a> "."
            </p>
        }
    });
    let rows = tracker
        .rows
        .into_iter()
        .map(row_view)
        .collect::<Vec<AnyView>>();
    view! {
        <section class="rk-card" aria-label="Results" data-done=(!working).to_string()>
            <div class="rk-head">
                <div>
                    <h2>{title}</h2>
                    <div class="rk-now">{mark} {tracker.sentence}</div>
                </div>
                <div class="rk-clock-box">
                    <div class="rk-clock">{move || rt::clock_text(secs())}</div>
                    <div class="rk-usual">{usual}</div>
                </div>
            </div>
            <div class="rk-grid">{rows}</div>
            {tagline}
            {leave}
        </section>
    }
    .into_any()
}

/// The Progress pill while the run works: always folded, with the role and
/// the latest step rolling in; "Show all" opens the original list.
pub(super) fn progress_pill(ui: TrackerUi, tracker: &Tracker, steps: Vec<StepLine>) -> AnyView {
    let n = steps.len();
    let moved = ui.seen.get_value().is_some_and(|s| n > s) && tracker.previous.is_some();
    ui.seen.set_value(Some(n));
    let head = format!(
        "Progress \u{b7} {n} {}",
        if n == 1 { "step" } else { "steps" }
    );
    let roll = if moved {
        let before = tracker.previous.clone().unwrap_or_default();
        let now = tracker.rolling.clone();
        view! {
            <span class="rk-roll-in moved">
                <span>{before}</span>
                <span>{now}</span>
            </span>
        }
        .into_any()
    } else {
        let now = tracker.rolling.clone();
        view! { <span class="rk-roll-in"><span>{now}</span></span> }.into_any()
    };
    let toggle = move |ev: leptos::ev::MouseEvent| {
        ev.prevent_default();
        ui.show_all.update(|v| *v = !*v);
    };
    let role = tracker.role;
    view! {
        <details class="rk-fold" prop:open=move || ui.show_all.get()>
            <summary on:click=toggle>
                <span class="rk-fold-title">{head}</span>
                <span class="rk-role">{role}</span>
                <span class="rk-roll" aria-live="polite">{roll}</span>
                <span class="rk-more" aria-hidden="true"></span>
            </summary>
            <ol class="steps">
                {steps.into_iter().map(|s| view! { <li>{s.line}</li> }).collect_view()}
            </ol>
        </details>
    }
    .into_any()
}
