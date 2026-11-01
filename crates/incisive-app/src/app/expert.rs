//! Temporal T4: the Expert Opinion screens (UI only; the rules are in
//! [`crate::expert_page`]).
//!
//! - **Run page** ([`Expert`], [`option_bar`], [`expert_view`]): an "Answer
//!   | Expert opinion" option under the title and badges when the run has a
//!   temporal row, and the Expert Opinion view (opinion, why, the three-line
//!   At a glance, the scenario cards, the board link and a disabled "Ask
//!   this evidence"). The answer below is drawn exactly as before, outside
//!   these pieces. `#expert` opens the view, also before hydration (CSS
//!   `:target`); while the opinion is being written the page polls every
//!   15 s for at most 10 minutes and flips to ready without a reload.
//! - **Board page** ([`TemporalPage`], `/run/:id/temporal`): the frozen
//!   Temporal board, steps 1–7, with Rust-drawn SVG (timeline and linked
//!   progression) and the stored checks folded at the bottom.
//!
//! Everything is escaped text, links and SVG elements built from data; no
//! `inner_html`. The styles ([`EXPERT_STYLE`]) ship inside these pieces
//! only, so a run page with no temporal row is unchanged. Nothing here logs
//! question or reply text.

use std::collections::BTreeSet;

use leptos::prelude::*;
use leptos_router::hooks::use_params_map;
use uuid::Uuid;

use super::page::sheet_click;
use super::{error_text, get_my_run_temporal};
use crate::expert_page::{self as xp, Board, Opened, OptionState, Piece};
use crate::runs::{RUN_NOT_FOUND_MESSAGE, TemporalView};
use crate::temporal::TemporalTable;

/// The Expert Opinion screens' styles: the ivory tokens of `ivory.rs`
/// (`--iv-*`), the run-page option's ready colours from its frozen mockup
/// (`--rx-ok*`) and the frozen board's own researcher, skeptic, warning and
/// era colours (`--rt-*`). Scoped to `.rx` and `.rt`; nothing here matches
/// the answer content.
pub const EXPERT_STYLE: &str = r##"
:root { --rx-ok:#2F6B4F; --rx-ok-tint:#E6F1EB;
  --rt-res:#2f5d50; --rt-res-soft:#e3ede8; --rt-skp:#7a4a8c; --rt-skp-soft:#eee4f2; --rt-warn:#a4522c; --rt-warn-soft:#f3e2d8;
  --rt-era1:#ece4cf; --rt-era2:#e2e9e0; --rt-era3:#dde6ef; }
.rx-answer[hidden], .rx-expert[hidden], .rx-hint[hidden] { display:none; }
.rx-seg, .rx-hint, .rx-expert { font-family:var(--iv-font); }
.rx-seg { display:grid; grid-template-columns:1fr 1fr; background:var(--iv-card); border:1px solid var(--iv-border); border-radius:999px; padding:4px; gap:4px; margin:4px 0 10px; max-width:560px; }
.rx .rx-tab { font:inherit; font-size:.86rem; font-weight:700; border:0; border-radius:999px; padding:6px 10px; min-height:44px; background:none; color:var(--iv-muted); cursor:pointer; display:flex; align-items:center; justify-content:center; gap:6px; text-align:center; text-decoration:none; line-height:1.25; }
.rx .rx-tab.on { background:var(--iv-tint); color:var(--iv-accent); }
.rx .rx-tab[disabled] { cursor:default; opacity:1; font-weight:600; }
.rx-dot { flex:none; width:7px; height:7px; border-radius:50%; background:var(--rx-ok); }
.rx-hint { border:1px dashed var(--iv-accent); background:var(--iv-tint); border-radius:14px; padding:10px 12px; font-size:.84rem; color:var(--iv-accent); display:flex; gap:8px; align-items:flex-start; margin:0 0 10px; max-width:560px; }
.rx-expert { display:flex; flex-direction:column; gap:10px; max-width:760px; margin-top:6px; color:var(--iv-body); font-size:15px; line-height:1.5; }
.rx-op { background:var(--iv-tint); border-radius:16px; padding:12px 14px; display:flex; flex-direction:column; gap:6px; }
.rx-who { font-size:.72rem; font-weight:700; letter-spacing:.07em; text-transform:uppercase; color:var(--iv-accent); }
.rx-verdict { margin:0; color:var(--iv-ink); font-weight:600; font-size:.98rem; line-height:1.45; }
.rx-card { background:var(--iv-card); border:1px solid var(--iv-border); border-radius:16px; padding:12px 14px; display:flex; flex-direction:column; gap:8px; }
.rx .rx-h { margin:0; font-size:.82rem; letter-spacing:.06em; text-transform:uppercase; color:var(--iv-muted); font-weight:700; }
.rx-card p { margin:0; font-size:.92rem; }
.rx-three { display:flex; flex-direction:column; gap:8px; font-size:.88rem; }
.rx-three > div { display:grid; grid-template-columns:78px minmax(0,1fr); gap:8px; }
.rx-three .k { font-weight:700; color:var(--iv-muted); font-size:.72rem; text-transform:uppercase; letter-spacing:.05em; padding-top:2px; }
.rx-three .today { color:var(--iv-ink); font-weight:600; }
.rx-scen { display:flex; flex-direction:column; gap:6px; }
.rx-scen > div { background:var(--iv-ground); border-radius:10px; padding:8px 10px; font-size:.86rem; display:flex; flex-direction:column; gap:2px; }
.rx-scen .lab { font-size:.7rem; letter-spacing:.07em; text-transform:uppercase; font-weight:700; color:var(--iv-muted); }
.rx-scen .choice { color:var(--iv-ink); font-weight:700; }
.rx-act { display:flex; flex-direction:column; gap:8px; margin-top:4px; max-width:560px; }
.rx .rx-btn { font:inherit; font-weight:700; font-size:.92rem; border-radius:14px; padding:10px 14px; min-height:46px; display:flex; justify-content:space-between; align-items:center; gap:10px; border:1px solid var(--iv-accent); background:var(--iv-accent); color:#fff; text-align:left; text-decoration:none; }
.rx .rx-btn.ghost { background:var(--iv-card); color:var(--iv-accent); }
.rx .rx-btn[disabled] { cursor:default; opacity:1; }
.rx-btn small { display:block; font-weight:500; font-size:.76rem; opacity:.85; }
.rx-soon, .rt-soon { flex:none; font-size:.66rem; font-weight:700; letter-spacing:.06em; text-transform:uppercase; background:var(--iv-tint); color:var(--iv-accent); border-radius:999px; padding:2px 8px; }
.rx a.rx-cite, .rt a.rt-cite { color:var(--iv-accent); font-weight:600; }
.rx a:focus-visible, .rx button:focus-visible, .rt a:focus-visible, .rt summary:focus-visible { outline:2px solid var(--iv-accent); outline-offset:2px; }
/* Before hydration: #expert opens a ready view (CSS :target) and hides the answer. */
.rx-pre .rx-expert:target:not(:empty)[hidden] { display:flex; }
.rx-pre:has(.rx-expert:target:not(:empty)) .rx-answer, .rx-pre:has(.rx-expert:target:not(:empty)) .rx-hint { display:none; }
.rx-pre:has(.rx-expert:target:not(:empty)) .rx-tab.on { background:none; color:var(--iv-muted); }
.rx-pre:has(.rx-expert:target:not(:empty)) a.rx-tab[href="#expert"] { background:var(--iv-tint); color:var(--iv-accent); }
/* ---- the Temporal board ---- */
.rt { font-family:var(--iv-font); color:var(--iv-ink); font-size:15px; line-height:1.55; display:flex; flex-direction:column; gap:30px; min-width:0; }
.rt p { margin:0; max-width:68ch; }
.rt h1 { font-size:1.5rem; line-height:1.2; margin:0; font-weight:700; text-wrap:balance; }
.rt h2 { font-size:1.15rem; margin:0; font-weight:700; text-wrap:balance; }
.rt h3 { font-size:1rem; margin:0; font-weight:700; }
.rt section, .rt header { display:flex; flex-direction:column; gap:12px; min-width:0; }
.rt-eyebrow { font-size:.72rem; letter-spacing:.08em; text-transform:uppercase; color:var(--iv-muted); font-weight:600; }
.rt-meta { color:var(--iv-muted); font-size:.86rem; }
.rt-step { display:flex; gap:10px; align-items:baseline; flex-wrap:wrap; }
.rt-step b { font-size:.72rem; letter-spacing:.06em; color:var(--iv-accent); text-transform:uppercase; }
.rt-panel { background:var(--iv-card); border:1px solid var(--iv-border); border-radius:12px; padding:16px; display:flex; flex-direction:column; gap:12px; min-width:0; }
.rt-scroll { overflow-x:auto; max-width:100%; }
.rt svg { display:block; width:100%; height:auto; }
.rt ul { margin:0; padding-left:1.1rem; display:flex; flex-direction:column; gap:5px; }
.rt-chip { font-size:.72rem; font-weight:600; padding:2px 8px; border-radius:999px; background:var(--iv-tint); color:var(--iv-accent); white-space:nowrap; }
.rt-chip.warn { background:var(--rt-warn-soft); color:var(--rt-warn); }
.rt-chip.grey { background:var(--iv-ground); color:var(--iv-muted); }
.rt-eras { display:grid; grid-template-columns:repeat(auto-fit,minmax(min(100%,260px),1fr)); gap:12px; }
.rt-era { border-radius:12px; padding:14px; display:flex; flex-direction:column; gap:10px; border:1px solid var(--iv-border); min-width:0; }
.rt-era.e1 { background:var(--rt-era1); } .rt-era.e2 { background:var(--rt-era2); } .rt-era.e3 { background:var(--rt-era3); }
.rt-era h3 { display:flex; justify-content:space-between; gap:8px; align-items:baseline; flex-wrap:wrap; }
.rt-lab { font-size:.7rem; letter-spacing:.07em; text-transform:uppercase; font-weight:700; color:var(--iv-muted); }
.rt-blk { display:flex; flex-direction:column; gap:4px; font-size:.9rem; }
.rt-kv { display:grid; grid-template-columns:minmax(0,1fr) auto; gap:2px 10px; font-size:.84rem; font-variant-numeric:tabular-nums; }
.rt-kv .v { font-weight:700; text-align:right; white-space:nowrap; }
.rt-kv .d { grid-column:1 / -1; color:var(--iv-muted); font-size:.76rem; margin-bottom:4px; }
.rt-empty { font-style:italic; color:var(--iv-muted); font-size:.86rem; }
.rt-learned { background:var(--iv-card); border-radius:8px; padding:10px; font-size:.88rem; border:1px solid var(--iv-border); display:flex; flex-direction:column; gap:4px; }
.rt-legend { display:flex; flex-wrap:wrap; gap:8px 14px; font-size:.78rem; color:var(--iv-muted); }
.rt-legend span { display:inline-flex; align-items:center; gap:6px; }
.rt-sw { width:16px; height:3px; display:inline-block; border-radius:2px; }
.rt-sw.s0 { background:var(--rt-warn); } .rt-sw.s1 { background:var(--iv-accent); } .rt-sw.s2 { background:var(--rt-skp); }
.rt-debate { gap:14px; }
.rt-round { display:flex; flex-direction:column; gap:8px; }
.rt-rh { font-size:.72rem; letter-spacing:.07em; text-transform:uppercase; font-weight:700; color:var(--iv-muted); }
.rt-msg { border-radius:12px; padding:12px 14px; max-width:min(100%,760px); display:flex; flex-direction:column; gap:6px; font-size:.92rem; border:1px solid var(--iv-border); align-self:flex-start; background:var(--iv-ground); }
.rt-msg .who { font-size:.74rem; font-weight:700; letter-spacing:.05em; text-transform:uppercase; color:var(--iv-muted); }
.rt-msg.r { background:var(--rt-res-soft); } .rt-msg.r .who { color:var(--rt-res); }
.rt-msg.s { background:var(--rt-skp-soft); align-self:flex-end; } .rt-msg.s .who { color:var(--rt-skp); }
.rt table { border-collapse:collapse; width:100%; font-size:.86rem; background:transparent; }
.rt th, .rt td { text-align:left; padding:8px; border-bottom:1px solid var(--iv-border); vertical-align:top; }
.rt th { font-weight:600; color:var(--iv-muted); font-size:.72rem; text-transform:uppercase; letter-spacing:.05em; }
.rt-ledger td:first-child { font-weight:600; }
.rt-verdict { border-left:3px solid var(--iv-accent); padding:4px 0 4px 14px; font-size:1.02rem; font-weight:500; }
.rt-scen { display:grid; grid-template-columns:repeat(auto-fit,minmax(min(100%,230px),1fr)); gap:12px; }
.rt-scen .rt-panel { gap:8px; }
.rt-choice { font-size:1.05rem; font-weight:700; }
details.rt-checks { background:var(--iv-card); border:1px dashed var(--iv-line); border-radius:12px; padding:4px 16px; }
details.rt-checks > summary { cursor:pointer; font-weight:600; min-height:44px; display:flex; align-items:center; color:var(--iv-accent); }
details.rt-checks[open] { display:flex; flex-direction:column; gap:12px; padding-bottom:16px; }
.rt-check { display:flex; flex-direction:column; gap:4px; }
.rt-check code { font-size:.78rem; word-break:break-all; }
.rt-check ul { font-size:.86rem; }
/* SVG marks */
.rt-band.e1 { fill:var(--rt-era1); } .rt-band.e2 { fill:var(--rt-era2); } .rt-band.e3 { fill:var(--rt-era3); }
.rt-tx { fill:var(--iv-muted); font-family:var(--iv-font); }
.rt-tx.ink { fill:var(--iv-ink); }
.rt-tx.b { font-weight:700; }
.rt-bar { fill:var(--iv-accent); stroke:var(--iv-accent); stroke-width:1.5; }
.rt-bar.dashed { fill:none; stroke-dasharray:4 3; }
.rt-know { fill:var(--iv-card); stroke:var(--iv-accent); stroke-width:2; }
.rt-link { stroke:var(--iv-accent); stroke-width:1.2; stroke-dasharray:2 3; }
.rt-mk { stroke:var(--rt-skp); stroke-dasharray:3 3; }
.rt-mk.gl { stroke:var(--rt-res); }
.rt-mk-t { fill:var(--rt-skp); font-family:var(--iv-font); }
.rt-mk-t.gl { fill:var(--rt-res); }
.rt-grid { stroke:var(--iv-border); stroke-width:1; }
.rt-grid.null { stroke:var(--iv-muted); stroke-width:1.6; }
.rt-s0 { stroke:var(--rt-warn); fill:var(--rt-warn); } .rt-s1 { stroke:var(--iv-accent); fill:var(--iv-accent); } .rt-s2 { stroke:var(--rt-skp); fill:var(--rt-skp); }
.rt-ln { fill:none; stroke-width:2.2; } .rt-ln.dashed { stroke-dasharray:5 4; }
.rt-wh { stroke-width:1.6; fill:none; }
.rt-pt { stroke-width:2; } .rt-pt.hollow { fill:var(--iv-card); }
.rt-pt.assumed { stroke-dasharray:2 2; }
.rt-split { fill:var(--rt-warn-soft); stroke:var(--rt-warn); stroke-width:1; }
.rt-split-t { fill:var(--rt-warn); font-family:var(--iv-font); font-weight:700; }
@media (max-width:640px) { .rt { gap:24px; } .rt h1 { font-size:1.3rem; } .rt-panel { padding:12px; } }
"##;

/// How a reference number in board text links.
#[derive(Clone, Copy)]
enum Cite {
    /// On the run page: the study sheet on the same page (`#ref-n`).
    Sheet,
    /// On the board page: the run page's study sheet.
    RunPage(Uuid),
}

/// Board text with its known reference numbers as links (every character
/// kept, escaped).
fn cited(text: &str, known: &BTreeSet<u16>, how: Cite) -> AnyView {
    xp::cite_pieces(text, known)
        .into_iter()
        .map(|p| match (p, how) {
            (Piece::Text(t), _) => t.into_any(),
            (Piece::Ref(n), Cite::Sheet) => view! {
                <a class="rx-cite" href=format!("#ref-{n}") rel="external" on:click=sheet_click(n)>{n.to_string()}</a>
            }
            .into_any(),
            (Piece::Ref(n), Cite::RunPage(id)) => view! {
                <a class="rt-cite" href=format!("/runs/{id}#ref-{n}") rel="external">{n.to_string()}</a>
            }
            .into_any(),
        })
        .collect_view()
        .into_any()
}

// ---------- the run page ----------

/// The run page's Expert Opinion state (signals; copied into views).
#[derive(Clone, Copy)]
pub(super) struct Expert {
    run_id: Uuid,
    /// The Expert Opinion view is shown (`#expert`).
    pub expert: RwSignal<bool>,
    /// The temporal row, refreshed while it is being written.
    state: RwSignal<TemporalView>,
    /// The row's board and table, read once per refresh.
    opened: Memo<Option<Opened>>,
    option: Memo<OptionState>,
    /// The browser has taken over (the `:target` fallback is off).
    hydrated: RwSignal<bool>,
    /// The answer's reference numbers (their sheets are on the page).
    answer_refs: StoredValue<BTreeSet<u16>>,
}

/// The current hash (browser only; empty elsewhere).
fn current_hash() -> String {
    #[cfg(feature = "hydrate")]
    {
        web_sys::window()
            .and_then(|w| w.location().hash().ok())
            .unwrap_or_default()
    }
    #[cfg(not(feature = "hydrate"))]
    String::new()
}

/// Set the address's hash without a new history entry or a scroll
/// (browser only): `#expert`, or none.
fn set_hash(expert: bool) {
    #[cfg(feature = "hydrate")]
    {
        let Some(w) = web_sys::window() else { return };
        let loc = w.location();
        let base = format!(
            "{}{}",
            loc.pathname().unwrap_or_default(),
            loc.search().unwrap_or_default()
        );
        let url = if expert {
            format!("{base}#expert")
        } else {
            base
        };
        if let Ok(h) = w.history() {
            let _ = h.replace_state_with_url(&wasm_bindgen::JsValue::NULL, "", Some(&url));
        }
    }
    #[cfg(not(feature = "hydrate"))]
    let _ = expert;
}

impl Expert {
    /// The state for `run_id`'s row `t`. In the browser: reads the hash
    /// (`#expert` opens the view; `#ref-n` opens study sheet `n`, the board
    /// page's citation links), follows later hash changes, and polls while
    /// the opinion is being written.
    pub fn new(run_id: Uuid, t: TemporalView, answer_refs: BTreeSet<u16>) -> Expert {
        let state = RwSignal::new(t);
        let opened = Memo::new(move |_| state.with(xp::open));
        let option = Memo::new(move |_| {
            let s = state.with(|v| OptionState::from_status(&v.status));
            if s == OptionState::Ready && opened.with(Option::is_none) {
                OptionState::Failed
            } else {
                s
            }
        });
        let x = Expert {
            run_id,
            expert: RwSignal::new(false),
            state,
            opened,
            option,
            hydrated: RwSignal::new(false),
            answer_refs: StoredValue::new(answer_refs),
        };
        // Effects run in the browser only: the server renders the Answer
        // view (the hash never reaches it).
        Effect::new(move |_| {
            x.hydrated.set(true);
            x.follow_hash();
            #[cfg(feature = "hydrate")]
            if let Some(n) = current_hash()
                .strip_prefix("#ref-")
                .and_then(|n| n.parse::<u16>().ok())
            {
                super::page::open_sheet(n);
            }
        });
        #[cfg(feature = "hydrate")]
        {
            let listener = window_event_listener(leptos::ev::hashchange, move |_| x.follow_hash());
            on_cleanup(move || listener.remove());
        }
        x.poll();
        x
    }

    /// Show the view the hash names (`#expert` once ready).
    fn follow_hash(self) {
        let want = current_hash() == "#expert" && self.option.get_untracked().selectable();
        if self.expert.get_untracked() != want {
            self.expert.set(want);
        }
    }

    /// While pending: ask again every [`xp::POLL_EVERY_SECS`] seconds, at
    /// most [`xp::POLL_MAX`] times; stop once the row is no longer pending.
    fn poll(self) {
        type Timer = Option<leptos::leptos_dom::helpers::IntervalHandle>;
        let timer: StoredValue<Timer, LocalStorage> = StoredValue::new_local(None);
        let count = StoredValue::new(0u32);
        let stop = move || {
            if let Some(h) = timer.try_update_value(|t| t.take()).flatten() {
                h.clear();
            }
        };
        Effect::new(move |_| {
            if !self.option.get().polls() {
                stop();
                return;
            }
            if timer.with_value(|t| t.is_some()) {
                return;
            }
            let h = leptos::leptos_dom::helpers::set_interval_with_handle(
                move || {
                    let n = count.get_value() + 1;
                    count.set_value(n);
                    if n > xp::POLL_MAX {
                        stop();
                        return;
                    }
                    let id = self.run_id;
                    leptos::task::spawn_local(async move {
                        // A transient error keeps the last state; the next
                        // tick asks again.
                        if let Ok(Some(v)) = get_my_run_temporal(id).await {
                            self.state.set(v);
                        }
                    });
                },
                std::time::Duration::from_secs(xp::POLL_EVERY_SECS),
            );
            timer.set_value(h.ok());
        });
        on_cleanup(stop);
    }

    /// The `class` of the wrapper (`rx-pre` until the browser takes over).
    pub fn wrapper_class(self) -> impl Fn() -> &'static str + Send + Sync + 'static {
        move || {
            if self.hydrated.get() {
                "rx"
            } else {
                "rx rx-pre"
            }
        }
    }

    fn known_refs(self, o: &Opened) -> BTreeSet<u16> {
        let answer = self.answer_refs.get_value();
        xp::known_refs(&o.board, o.flags.as_ref())
            .into_iter()
            .filter(|n| answer.contains(n))
            .collect()
    }
}

/// The "Answer | Expert opinion" option, and the ready hint under it
/// (Answer view only). Before hydration both are links (`#answer`,
/// `#expert`); after it they switch the view in place and keep `#expert`
/// in the address. An opinion that is not ready is a disabled button that
/// says why.
pub(super) fn option_bar(x: Expert) -> AnyView {
    let to_answer = move |ev: leptos::ev::MouseEvent| {
        ev.prevent_default();
        x.expert.set(false);
        set_hash(false);
    };
    let to_expert = move |ev: leptos::ev::MouseEvent| {
        ev.prevent_default();
        x.expert.set(true);
        set_hash(true);
    };
    let expert_tab = move || {
        let s = x.option.get();
        if s.selectable() {
            view! {
                <a
                    class="rx-tab"
                    class:on=move || x.expert.get()
                    role="tab"
                    aria-selected=move || x.expert.get().to_string()
                    aria-controls="expert"
                    href="#expert"
                    rel="external"
                    on:click=to_expert
                >
                    <span class="rx-dot" aria-hidden="true"></span>
                    {s.text()}
                </a>
            }
            .into_any()
        } else {
            view! {
                <button class="rx-tab" type="button" role="tab" aria-selected="false" disabled=true>
                    {s.text()}
                </button>
            }
            .into_any()
        }
    };
    view! {
        <div class="rx-seg" role="tablist" aria-label="Run views">
            <a
                class="rx-tab"
                class:on=move || !x.expert.get()
                role="tab"
                aria-selected=move || (!x.expert.get()).to_string()
                href="#answer"
                rel="external"
                on:click=to_answer
            >
                "Answer"
            </a>
            {expert_tab}
        </div>
        {move || {
            (x.option.get().dot() && !x.expert.get())
                .then(|| view! {
                    <p class="rx-hint" role="status">
                        <span aria-hidden="true">"\u{25cf}"</span>
                        <span>{xp::READY_HINT}</span>
                    </p>
                })
        }}
    }
    .into_any()
}

/// The Expert Opinion view (`#expert`): drawn once the opinion is ready,
/// hidden while the Answer view is shown.
pub(super) fn expert_view(x: Expert) -> AnyView {
    view! {
        <section class="rx-expert" id="expert" aria-label="Expert opinion" hidden=move || !x.expert.get()>
            {move || {
                x.opened.with(|o| o.as_ref().map(|o| expert_body(x.run_id, o, &x.known_refs(o))))
            }}
        </section>
    }
    .into_any()
}

fn expert_body(run_id: Uuid, o: &Opened, known: &BTreeSet<u16>) -> AnyView {
    let b = &o.board;
    let c = |t: &str| cited(t, known, Cite::Sheet);
    let g = &b.at_a_glance;
    let line = |k: &'static str, t: &str, class: &'static str| {
        view! {
            <div>
                <span class="k">{k}</span>
                <span class=class>{c(t)}</span>
            </div>
        }
        .into_any()
    };
    let decisions = (!b.decisions.is_empty()).then(|| {
        let cards = b
            .decisions
            .iter()
            .map(|d| {
                view! {
                    <div>
                        <span class="lab">{d.scenario.clone()}</span>
                        <span class="choice">{c(&d.choice)}</span>
                        <span>{c(&d.reason)}</span>
                    </div>
                }
                .into_any()
            })
            .collect_view();
        view! {
            <div class="rx-card">
                <h2 class="rx-h">"Decision by scenario"</h2>
                <div class="rx-scen">{cards}</div>
            </div>
        }
        .into_any()
    });
    view! {
        <div class="rx-op">
            <span class="rx-who">"Expert opinion \u{b7} evidence over time"</span>
            <p class="rx-verdict">{c(&b.opinion.verdict)}</p>
        </div>
        <div class="rx-card">
            <h2 class="rx-h">"My view on why"</h2>
            <p>{c(&b.opinion.why)}</p>
        </div>
        <div class="rx-card">
            <h2 class="rx-h">"At a glance"</h2>
            <div class="rx-three">
                {line("Original", &g.original_quote, "")}
                {line("Expert", &g.expert, "")}
                {line("Today", &g.today, "today")}
            </div>
        </div>
        {decisions}
        <div class="rx-act">
            <a class="rx-btn" href=format!("/run/{run_id}/temporal")>
                <span>"Open the Temporal board"</span>
                <span aria-hidden="true">"\u{2192}"</span>
            </a>
            <button class="rx-btn ghost" type="button" disabled=true>
                <span>"Ask this evidence" <small>"Reads only this answer and its expert opinion"</small></span>
                <span class="rx-soon">"Later"</span>
            </button>
        </div>
    }
    .into_any()
}

// ---------- the board page ----------

/// `/run/:id/temporal`: the Temporal board of one of the user's runs
/// (the same read path and scoping as the run page: someone else's run is
/// "not found").
#[component]
pub fn TemporalPage() -> impl IntoView {
    let params = use_params_map();
    let run_id = move || {
        params
            .read()
            .get("id")
            .and_then(|s| Uuid::parse_str(&s).ok())
    };
    let data = Resource::new(run_id, |id| async move {
        match id {
            Some(id) => get_my_run_temporal(id).await,
            None => Err(ServerFnError::new(RUN_NOT_FOUND_MESSAGE)),
        }
    });
    let back = move || match run_id() {
        Some(id) => format!("/runs/{id}#expert"),
        None => "/history".to_string(),
    };
    view! {
        <p class="run-back">
            <a href=back rel="external">"\u{2190} Back to the run"</a>
        </p>
        <Transition fallback=|| view! { <p>"Loading\u{2026}"</p> }>
            {move || {
                data.get()
                    .map(|r| match (r, run_id()) {
                        (Ok(Some(v)), Some(id)) => board_page(id, &v),
                        (Ok(None), _) => not_ready("This question has no expert opinion. It is written only for Full answers, after the answer is saved."),
                        (Ok(Some(_)), None) | (Err(_), None) => not_ready(RUN_NOT_FOUND_MESSAGE),
                        (Err(e), _) => view! { <p class="box amber" role="alert">{error_text(&e)}</p> }.into_any(),
                    })
            }}
        </Transition>
    }
}

fn not_ready(text: &str) -> AnyView {
    view! { <p class="box amber" role="status">{text.to_string()}</p> }.into_any()
}

/// What the board page says for a row that is not ready.
fn status_message(s: OptionState) -> &'static str {
    match s {
        OptionState::Pending => {
            "The expert opinion is being written. It usually takes a few minutes; come back to this page shortly."
        }
        OptionState::Skipped => {
            "There was no expert opinion for this question: it needs at least two primary studies."
        }
        OptionState::CapBlocked => {
            "The expert opinion was not written: today's spending limit had been reached."
        }
        OptionState::Failed | OptionState::Ready => {
            "The expert opinion is unavailable for this question."
        }
    }
}

/// The board page for `run_id`'s row `v` (or why it cannot be drawn).
pub(super) fn board_page(run_id: Uuid, v: &TemporalView) -> AnyView {
    let s = OptionState::of(v);
    let Some(o) = s.selectable().then(|| xp::open(v)).flatten() else {
        return not_ready(status_message(s));
    };
    let known = xp::known_refs(&o.board, o.flags.as_ref());
    let c = move |t: &str| cited(t, &known, Cite::RunPage(run_id));
    let topic = if o.board.topic.trim().is_empty() {
        v.topic
            .clone()
            .unwrap_or_else(|| "Temporal evidence board".into())
    } else {
        o.board.topic.clone()
    };
    view! {
        <div class="rt">
            <style>{EXPERT_STYLE}</style>
            <header>
                <div class="rt-eyebrow">"Incisive \u{b7} temporal evidence board"</div>
                <h1>{topic}</h1>
                <p class="rt-meta">{xp::source_line(&o.table)}</p>
            </header>
            {step1(&o.table)}
            {step2(&o.board, &o.table, &c)}
            {step3(&o.board, &o.table, &c)}
            {step4(&o.board, &c)}
            {step5(&o.board, &c)}
            {step6(&o.board, &o.table, &c)}
            {step7(&o.board, &o.table, &c)}
            {checks(o.flags.as_ref())}
        </div>
    }
    .into_any()
}

fn step_head(n: u8, title: &'static str) -> AnyView {
    view! {
        <div class="rt-step">
            <b>{format!("Step {n}")}</b>
            <h2>{title}</h2>
        </div>
    }
    .into_any()
}

// ---- step 1: the timeline ----

fn step1(t: &TemporalTable) -> AnyView {
    let chart = match xp::gantt(t) {
        Some(g) => gantt_svg(&g),
        None => {
            view! { <p class="rt-empty">"No study has treatment or publication years to draw."</p> }
                .into_any()
        }
    };
    view! {
        <section>
            {step_head(1, "When the patients were treated")}
            <div class="rt-panel">
                <div class="rt-scroll">{chart}</div>
                <p class="rt-meta">
                    "Each bar spans the years a study's patients were treated. Dashed bars have assumed years (publication year minus 2). Rings on a bar's row are later reports of the same trial, at the year they were published. Vertical lines mark the years of guidelines and of meta-analyses and reviews."
                </p>
            </div>
        </section>
    }
    .into_any()
}

fn gantt_svg(g: &xp::Gantt) -> AnyView {
    let p = xp::px;
    let bands = g
        .bands
        .iter()
        .map(|b| {
            view! {
                <rect class=format!("rt-band e{}", b.class) x=p(b.x) y=p(g.band_top) width=p(b.w) height=p(g.band_h)></rect>
                <text class="rt-tx b" x=p(b.x + b.w / 2.0) y=p(g.band_top + 14.0) text-anchor="middle" font-size="12">{b.label.clone()}</text>
            }
            .into_any()
        })
        .collect_view();
    let rows = g
        .rows
        .iter()
        .map(|r| {
            let bar = r.bar.map(|(a, b)| {
                view! {
                    <rect
                        class=if r.dashed { "rt-bar dashed" } else { "rt-bar" }
                        x=p(a)
                        y=p(r.y - 7.0)
                        width=p((b - a).max(4.0))
                        height="14"
                        rx="7"
                    ></rect>
                }
                .into_any()
            });
            let end = r.bar.map_or(xp::GANTT_L, |(_, b)| b);
            let points = r
                .points
                .iter()
                .map(|k| {
                    view! {
                        <line class="rt-link" x1=p(end) x2=p(k.x) y1=p(r.y) y2=p(r.y)></line>
                        <circle class="rt-know" cx=p(k.x) cy=p(r.y) r="5"></circle>
                        <text class="rt-tx" x=p(k.x) y=p(r.y - 9.0) text-anchor="middle" font-size="10">{k.label.clone()}</text>
                    }
                    .into_any()
                })
                .collect_view();
            // The note sits after the bar, or before it when the bar ends
            // near the right edge.
            let far = r.points.iter().map(|k| k.x).fold(end, f64::max);
            let (nx, anchor) = if far + 60.0 > g.width {
                (r.bar.map_or(xp::GANTT_L, |(a, _)| a) - 6.0, "end")
            } else {
                (far + 8.0, "start")
            };
            view! {
                <g>
                    <title>{r.title.clone()}</title>
                    <text class="rt-tx ink" x=p(xp::GANTT_L - 10.0) y=p(r.y + 4.0) text-anchor="end" font-size="12">{r.label.clone()}</text>
                    {bar}
                    {points}
                    <text class="rt-tx" x=p(nx) y=p(r.y + 4.0) text-anchor=anchor font-size="10.5">{r.note.clone()}</text>
                </g>
            }
            .into_any()
        })
        .collect_view();
    let ticks = g
        .ticks
        .iter()
        .map(|(x, y)| {
            view! {
                <text class="rt-tx" x=p(*x) y=p(g.axis_y + 14.0) text-anchor="middle" font-size="11">{y.to_string()}</text>
            }
            .into_any()
        })
        .collect_view();
    let markers = g
        .markers
        .iter()
        .map(|m| {
            let gl = if m.guideline { " gl" } else { "" };
            let tx = if m.anchor_end { m.x - 4.0 } else { m.x + 4.0 };
            view! {
                <line class=format!("rt-mk{gl}") x1=p(m.x) x2=p(m.x) y1=p(g.band_top + 18.0) y2=p(m.label_y - 10.0)></line>
                <text
                    class=format!("rt-mk-t{gl}")
                    x=p(tx)
                    y=p(m.label_y)
                    text-anchor=if m.anchor_end { "end" } else { "start" }
                    font-size="10"
                >
                    {m.label.clone()}
                </text>
            }
            .into_any()
        })
        .collect_view();
    let label = format!(
        "Timeline of {} studies, {} to {}",
        g.rows.len(),
        g.from,
        g.to - 1
    );
    view! {
        <svg viewBox=format!("0 0 {} {}", p(g.width), p(g.height)) style="min-width:620px" role="img" aria-label=label>
            {bands}
            {markers}
            {rows}
            {ticks}
        </svg>
    }
    .into_any()
}

// ---- step 2: the eras ----

fn step2(b: &Board, t: &TemporalTable, c: &dyn Fn(&str) -> AnyView) -> AnyView {
    let n = t.eras.len();
    let mut cards: Vec<AnyView> = t
        .eras
        .iter()
        .enumerate()
        .map(|(i, e)| {
            let note = b.era_note(&e.id);
            let figures = xp::era_figures(t, &e.id);
            let shown = if figures.is_empty() {
                let text = if e.empty || e.n_studies == 0 {
                    "No studies in this era."
                } else {
                    "No outcome of this era has an estimate the table can draw."
                };
                view! { <p class="rt-empty">{text}</p> }.into_any()
            } else {
                let kv = figures
                    .into_iter()
                    .map(|f| {
                        view! {
                            <span>{f.label}</span>
                            <span class="v">{f.value}</span>
                            <span class="d">{f.detail}</span>
                        }
                        .into_any()
                    })
                    .collect_view();
                view! { <div class="rt-kv">{kv}</div> }.into_any()
            };
            let blk = |lab: &'static str, text: Option<&String>| {
                text.filter(|t| !t.trim().is_empty()).map(|t| {
                    view! {
                        <div class="rt-blk">
                            <span class="rt-lab">{lab}</span>
                            <p>{c(t)}</p>
                        </div>
                    }
                    .into_any()
                })
            };
            let after = note
                .map(|n| &n.knowledge_after)
                .filter(|t| !t.trim().is_empty())
                .map(|t| {
                    view! {
                        <div class="rt-learned">
                            <span class="rt-lab">"Knowledge after"</span>
                            <p>{c(t)}</p>
                        </div>
                    }
                    .into_any()
                });
            let missing = note.is_none().then(|| {
                view! { <p class="rt-empty">"The board has no note for this era."</p> }.into_any()
            });
            let chip_class = if i + 1 == n { "rt-chip" } else { "rt-chip grey" };
            view! {
                <article class=format!("rt-era e{}", xp::era_class(i, n))>
                    <h3>{e.label.clone()} " " <span class=chip_class>{xp::era_chip(i, e, n)}</span></h3>
                    {blk("Going in", note.map(|n| &n.going_in))}
                    <div class="rt-blk">
                        <span class="rt-lab">"What the era showed"</span>
                        {shown}
                        {note.filter(|n| !n.showed.trim().is_empty()).map(|n| view! { <p>{c(&n.showed)}</p> }.into_any())}
                    </div>
                    {after}
                    {missing}
                </article>
            }
            .into_any()
        })
        .collect();
    // Notes for eras the table does not have: shown, flagged by the checks.
    for o in b.orphan_era_notes(t) {
        cards.push(
            view! {
                <article class="rt-era">
                    <h3>{o.era_id.clone()} " " <span class="rt-chip warn">"Not an era of the table"</span></h3>
                    <div class="rt-blk"><span class="rt-lab">"Going in"</span><p>{c(&o.going_in)}</p></div>
                    <div class="rt-blk"><span class="rt-lab">"What the era showed"</span><p>{c(&o.showed)}</p></div>
                    <div class="rt-learned"><span class="rt-lab">"Knowledge after"</span><p>{c(&o.knowledge_after)}</p></div>
                </article>
            }
            .into_any(),
        );
    }
    view! {
        <section>
            {step_head(2, "What each era showed, and what it taught")}
            <p class="rt-meta">"Figures are the table's era estimates for each outcome (code, from the run's numbers); the text is the expert writer's."</p>
            <div class="rt-eras">{cards}</div>
        </section>
    }
    .into_any()
}

// ---- step 3: linked progression ----

fn step3(b: &Board, t: &TemporalTable, c: &dyn Fn(&str) -> AnyView) -> AnyView {
    let panels = xp::linked_panels(t);
    let legend: Vec<AnyView> = xp::linked_outcomes(t)
        .into_iter()
        .enumerate()
        .map(|(i, (o, _))| {
            view! { <span><i class=format!("rt-sw s{i}")></i>{o.outcome.clone()}</span> }.into_any()
        })
        .collect();
    let body = if panels.is_empty() {
        view! { <p class="rt-empty">"No outcome has data in two or more eras, so there is no progression to draw."</p> }
            .into_any()
    } else {
        let charts = panels
            .iter()
            .map(|pn| {
                view! {
                    <div class="rt-scroll">{panel_svg(pn)}</div>
                    <p class="rt-meta">{pn.caption()}</p>
                }
                .into_any()
            })
            .collect_view();
        view! {
            <div class="rt-legend">{legend}</div>
            {charts}
            <p class="rt-meta">"Era estimates with 95% CI where pooled. Hollow points have no CI (points only); dashed rings and lines rest on assumed treatment years; \u{201c}split\u{201d} marks an era whose points sit on both sides of no difference."</p>
        }
        .into_any()
    };
    let note =
        (!b.linked_note.trim().is_empty()).then(|| view! { <p>{c(&b.linked_note)}</p> }.into_any());
    view! {
        <section>
            {step_head(3, "Linked progression")}
            <div class="rt-panel">
                {body}
                {note}
            </div>
        </section>
    }
    .into_any()
}

fn panel_svg(pn: &xp::Panel) -> AnyView {
    let p = xp::px;
    let left = xp::LINKED_L;
    let right = pn.width - xp::LINKED_R;
    let grid = pn
        .ticks
        .iter()
        .map(|(y, l)| {
            view! {
                <line class="rt-grid" x1=p(left) x2=p(right) y1=p(*y) y2=p(*y)></line>
                <text class="rt-tx" x=p(left - 8.0) y=p(y + 4.0) text-anchor="end" font-size="11">{l.clone()}</text>
            }
            .into_any()
        })
        .collect_view();
    let cols = pn
        .cols
        .iter()
        .map(|(x, l)| {
            view! {
                <text class="rt-tx b" x=p(*x) y=p(pn.height - 12.0) text-anchor="middle" font-size="12">{l.clone()}</text>
            }
            .into_any()
        })
        .collect_view();
    let series = pn
        .series
        .iter()
        .map(|s| {
            let k = s.slot;
            let segs = s
                .segs
                .iter()
                .map(|g| {
                    let class = if g.dashed {
                        format!("rt-ln dashed rt-s{k}")
                    } else {
                        format!("rt-ln rt-s{k}")
                    };
                    view! { <line class=class x1=p(g.x1) y1=p(g.y1) x2=p(g.x2) y2=p(g.y2)></line> }.into_any()
                })
                .collect_view();
            let pts = s
                .points
                .iter()
                .map(|pt| match pt.y {
                    Some(y) => {
                        let whisker = pt.whisker.map(|(a, b)| {
                            view! { <line class=format!("rt-wh rt-s{k}") x1=p(pt.x) x2=p(pt.x) y1=p(a) y2=p(b)></line> }
                                .into_any()
                        });
                        let mut class = format!("rt-pt rt-s{k}");
                        if pt.hollow {
                            class.push_str(" hollow");
                        }
                        if pt.assumed {
                            class.push_str(" assumed");
                        }
                        view! {
                            {whisker}
                            <circle class=class cx=p(pt.x) cy=p(y) r="5"></circle>
                            <text class=format!("rt-tx b rt-s{k}") x=p(pt.x + 8.0) y=p(y + 4.0) font-size="11" stroke="none">{pt.label.clone()}</text>
                        }
                        .into_any()
                    }
                    None => view! {
                        <rect class="rt-split" x=p(pt.x - 16.0) y=p(pn.null_y - 9.0) width="32" height="18" rx="9"></rect>
                        <text class="rt-split-t" x=p(pt.x) y=p(pn.null_y + 4.0) text-anchor="middle" font-size="10">"split"</text>
                    }
                    .into_any(),
                })
                .collect_view();
            view! { <g>{segs}{pts}</g> }.into_any()
        })
        .collect_view();
    let names: Vec<&str> = pn.series.iter().map(|s| s.outcome.as_str()).collect();
    let label = format!(
        "Era estimates, {}: {}",
        if pn.axis.is_log() {
            "ratio on a log scale"
        } else {
            "difference on a linear scale"
        },
        names.join(", ")
    );
    view! {
        <svg viewBox=format!("0 0 {} {}", p(pn.width), p(pn.height)) style="min-width:560px" role="img" aria-label=label>
            {grid}
            <line class="rt-grid null" x1=p(left) x2=p(right) y1=p(pn.null_y) y2=p(pn.null_y)></line>
            <text class="rt-tx" x=p(left + 6.0) y=p(pn.null_y - 6.0) font-size="10">"no difference"</text>
            {cols}
            {series}
        </svg>
    }
    .into_any()
}

// ---- step 4: the debate ----

fn step4(b: &Board, c: &dyn Fn(&str) -> AnyView) -> AnyView {
    let rounds = b
        .debate
        .iter()
        .map(|r| {
            let class = match xp::side(&r.speaker) {
                xp::Side::Researcher => "rt-msg r",
                xp::Side::Skeptic => "rt-msg s",
                xp::Side::Other => "rt-msg",
            };
            let points = if r.points.len() == 1 {
                view! { <p>{c(&r.points[0])}</p> }.into_any()
            } else {
                let items = r
                    .points
                    .iter()
                    .map(|pt| view! { <li>{c(pt)}</li> }.into_any())
                    .collect_view();
                view! { <ul>{items}</ul> }.into_any()
            };
            view! {
                <div class="rt-round">
                    <div class="rt-rh">{xp::round_heading(r)}</div>
                    <div class=class>
                        <span class="who">{xp::speaker_label(&r.speaker)}</span>
                        {points}
                    </div>
                </div>
            }
            .into_any()
        })
        .collect_view();
    view! {
        <section>
            {step_head(4, "Latest-period researcher and skeptic")}
            <p class="rt-meta">"The researcher names competing explanations before choosing one; the skeptic challenges the conclusion whichever way it leans."</p>
            <div class="rt-panel rt-debate">{rounds}</div>
        </section>
    }
    .into_any()
}

// ---- step 5: the scoreboard ----

fn step5(b: &Board, c: &dyn Fn(&str) -> AnyView) -> AnyView {
    let rows = b
        .scoreboard
        .iter()
        .map(|s| {
            view! {
                <tr>
                    <td><b>{s.code.clone()}</b> " " {c(&s.name)}</td>
                    <td>{c(&s.for_)}</td>
                    <td>{c(&s.against)}</td>
                    <td><span class=xp::status_class(&s.status)>{s.status.clone()}</span></td>
                </tr>
            }
            .into_any()
        })
        .collect_view();
    view! {
        <section>
            {step_head(5, "Hypothesis scoreboard after the debate")}
            <div class="rt-panel rt-scroll">
                <table style="min-width:600px">
                    <thead><tr><th>"Explanation"</th><th>"For"</th><th>"Against"</th><th>"Status"</th></tr></thead>
                    <tbody>{rows}</tbody>
                </table>
            </div>
        </section>
    }
    .into_any()
}

// ---- step 6: the ledger ----

fn step6(b: &Board, t: &TemporalTable, c: &dyn Fn(&str) -> AnyView) -> AnyView {
    let mode = &b.ledger.label_mode;
    let ncols = b
        .ledger
        .rows
        .iter()
        .map(|r| r.cells.len())
        .max()
        .unwrap_or(0);
    let heads = xp::ledger_headers(mode, &t.eras, ncols)
        .into_iter()
        .map(|h| view! { <th>{h}</th> }.into_any())
        .collect_view();
    let rows = b
        .ledger
        .rows
        .iter()
        .map(|r| {
            let cells = (0..ncols)
                .map(|i| {
                    let t = r.cells.get(i).map(String::as_str).unwrap_or("");
                    view! { <td>{c(t)}</td> }.into_any()
                })
                .collect_view();
            view! { <tr><td>{c(&r.question)}</td>{cells}</tr> }.into_any()
        })
        .collect_view();
    view! {
        <section>
            {step_head(6, "Temporal knowledge ledger")}
            <div class="rt-panel rt-scroll">
                <table class="rt-ledger" style="min-width:640px">
                    <thead><tr><th>"Question"</th>{heads}</tr></thead>
                    <tbody>{rows}</tbody>
                </table>
                <p class="rt-meta">{xp::ledger_note(mode)}</p>
            </div>
        </section>
    }
    .into_any()
}

// ---- step 7: the expert opinion ----

fn step7(b: &Board, t: &TemporalTable, c: &dyn Fn(&str) -> AnyView) -> AnyView {
    let o = &b.opinion;
    let item = |lab: &'static str, text: &str| {
        (!text.trim().is_empty()).then(|| view! { <li><b>{lab}</b> " " {c(text)}</li> }.into_any())
    };
    let scen = b
        .decisions
        .iter()
        .map(|d| {
            view! {
                <div class="rt-panel">
                    <span class="rt-lab">{d.scenario.clone()}</span>
                    <span class="rt-choice">{c(&d.choice)}</span>
                    <p class="rt-meta">{c(&d.reason)}</p>
                </div>
            }
            .into_any()
        })
        .collect_view();
    let triggers = b
        .triggers
        .iter()
        .map(|tr| view! { <tr><td>{c(&tr.if_)}</td><td>{c(&tr.then)}</td></tr> }.into_any())
        .collect_view();
    let predictive =
        xp::predictive_line(t).map(|l| view! { <p class="rt-meta">{l}</p> }.into_any());
    let settle = b
        .settle
        .iter()
        .map(|s| view! { <li>{c(s)}</li> }.into_any())
        .collect_view();
    view! {
        <section>
            {step_head(7, "Expert opinion")}
            <div class="rt-panel">
                <p class="rt-verdict">{c(&o.verdict)}</p>
                <ul>
                    {item("Why.", &o.why)}
                    {item("The learning curve.", &o.curve_reading)}
                    {item("What did not improve.", &o.did_not_improve)}
                </ul>
            </div>
            <h3>"Expert decision: what should we conclude today?"</h3>
            <div class="rt-scen">{scen}</div>
            <div class="rt-panel rt-scroll">
                <h3>"What the next five years could show, and what it would change"</h3>
                <table style="min-width:600px">
                    <thead><tr><th>"If the next independent study shows"</th><th>"Then"</th></tr></thead>
                    <tbody>{triggers}</tbody>
                </table>
                {predictive}
            </div>
            <div class="rt-panel">
                <h3>"What would settle it"</h3>
                <ul>{settle}</ul>
            </div>
        </section>
    }
    .into_any()
}

// ---- the checks ----

fn checks(flags: Option<&xp::Flags>) -> AnyView {
    let Some(f) = flags else {
        return view! {
            <details class="rt-checks">
                <summary>"Checks \u{b7} none stored"</summary>
                <p class="rt-meta">"No code checks were stored with this board."</p>
            </details>
        }
        .into_any();
    };
    let total = f.total();
    let kinds = f
        .kinds()
        .into_iter()
        .map(|k| {
            let list = if k.flags.is_empty() {
                view! { <p class="rt-meta">"None."</p> }.into_any()
            } else {
                let items = k
                    .flags
                    .iter()
                    .map(|fl| {
                        view! { <li><code>{fl.field.clone()}</code> ": " {fl.text.clone()}</li> }
                            .into_any()
                    })
                    .collect_view();
                view! { <ul>{items}</ul> }.into_any()
            };
            view! {
                <div class="rt-check">
                    <h3>{format!("{} ({})", k.label, k.flags.len())}</h3>
                    <p class="rt-meta">{k.note}</p>
                    {list}
                </div>
            }
            .into_any()
        })
        .collect_view();
    let head = format!(
        "Checks \u{b7} {total} {} \u{b7} {} derived",
        if total == 1 { "flag" } else { "flags" },
        f.derived.len()
    );
    view! {
        <details class="rt-checks">
            <summary>{head}</summary>
            <p class="rt-meta">"Code checks on the expert writer's board, as stored. Flag, never delete: the board above is shown exactly as written."</p>
            {kinds}
        </details>
    }
    .into_any()
}
