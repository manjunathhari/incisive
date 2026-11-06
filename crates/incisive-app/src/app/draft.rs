//! Stage S1: the page while a writer works (the rules are in
//! [`crate::draft_page`]). The sections keep the page's order, those not yet
//! arrived say "Coming", and the unfinished last item is faded.
//!
//! The text comes from a signal, so a poll that only moves the draft on never
//! touches the rest of the page. Escaped text only; no `inner_html`. No marks, charts or labels: none of the
//! checks has run. The styles are drawn only with a draft, so every other page
//! is unchanged.

use chrono::{DateTime, Utc};
use leptos::prelude::*;

use super::tracker::TrackerUi;
use crate::draft_page::{
    BANNER, COMING, DraftText, Item, Kind, Piece, Section, read_answer, read_opinion,
};
use crate::run_tracker as rt;

const DRAFT_STYLE: &str = r##"
.dr { display:flex; flex-direction:column; gap:12px; width:100%; max-width:760px; margin:10px 0 0; font-family:var(--iv-font); color:var(--iv-ink); min-width:0; overflow-wrap:anywhere; }
.dr-banner { margin:0; padding:10px 12px; background:var(--iv-warn-bg); color:var(--iv-warn); border:1px solid #F0E2C2; border-radius:10px; font-size:13px; font-weight:600; }
.dr-card { padding:14px 16px; background:var(--iv-card); border:1px solid var(--iv-border); border-radius:14px; }
.dr-head { display:flex; justify-content:space-between; align-items:baseline; gap:12px; margin:0 0 6px; }
.dr-card h2 { margin:0; font-size:15px; font-weight:700; }
.dr-count { flex:none; font-size:12px; font-weight:500; color:var(--iv-muted); }
.dr-card p { margin:0; font-size:14px; line-height:1.5; }
.dr-card p + p { margin-top:8px; }
.dr-c { color:var(--iv-muted); }
.dr-fade { color:#8B877F; }
.dr-ul { margin:8px 0 0; padding-left:18px; font-size:14px; line-height:1.45; }
.dr-o { margin-top:10px; padding-top:10px; border-top:1px solid var(--iv-rule); }
.dr-on { font-weight:600; margin-bottom:4px !important; }
.dr-coming { display:flex; justify-content:space-between; align-items:center; gap:12px; padding:12px 16px; background:var(--iv-card); border:1px dashed var(--iv-line); border-radius:14px; font-size:14px; font-weight:600; color:var(--iv-body); }
.dr-coming span { flex:none; font-size:12px; font-weight:500; color:var(--iv-muted); }
.dr-fold { display:block; margin:10px 0; padding:0 16px; background:var(--iv-card); border:1px solid var(--iv-border); border-radius:16px; font-family:var(--iv-font); }
.dr-fold > summary { list-style:none; min-height:44px; display:flex; align-items:center; justify-content:space-between; gap:12px; cursor:pointer; font-size:15px; font-weight:600; }
.dr-fold > summary::-webkit-details-marker { display:none; }
.dr-clock { flex:none; font-size:13px; font-weight:600; color:var(--iv-accent); }
.dr-fold > .rk-card { margin:0 -16px; border:0; background:transparent; }
"##;

fn pieces(p: Vec<Piece>) -> Vec<AnyView> {
    p.into_iter()
        .map(|x| match x {
            Piece::Text(t) => t.into_any(),
            Piece::Cite(c) => view! { <span class="dr-c">{c}</span> }.into_any(),
        })
        .collect()
}

fn text_view(t: DraftText) -> AnyView {
    let tail = (!t.tail.is_empty()).then(|| {
        view! { <span class="dr-fade">{pieces(t.tail)}</span> }
    });
    view! { {pieces(t.head)} {tail} }.into_any()
}

fn item_view(i: Item) -> AnyView {
    let class = if i.faded { "dr-fade" } else { "" };
    match i.kind {
        Kind::Para => view! {
            <p class=class>
                {i.label.map(|l| view! { <strong>{l}</strong> " " })}
                {text_view(i.text)}
            </p>
        }
        .into_any(),
        Kind::Bullet => view! {
            <li class=class>
                {i.label.map(|l| view! { {l} ": " })}
                {text_view(i.text)}
            </li>
        }
        .into_any(),
        Kind::Entry => view! {
            <div class="dr-o">
                {i.label.map(|l| view! { <p class=format!("dr-on {class}")>{l}</p> })}
                <p class=class>{text_view(i.text)}</p>
            </div>
        }
        .into_any(),
    }
}

fn section_view(s: Section) -> AnyView {
    if s.items.is_empty() {
        return view! {
            <div class="dr-coming">{s.title}<span>{COMING}</span></div>
        }
        .into_any();
    }
    // Neighbouring list lines share one list.
    let mut body: Vec<AnyView> = Vec::new();
    let mut bullets: Vec<AnyView> = Vec::new();
    for i in s.items {
        if i.kind == Kind::Bullet {
            bullets.push(item_view(i));
            continue;
        }
        if !bullets.is_empty() {
            let list = std::mem::take(&mut bullets);
            body.push(view! { <ul class="dr-ul">{list}</ul> }.into_any());
        }
        body.push(item_view(i));
    }
    if !bullets.is_empty() {
        body.push(view! { <ul class="dr-ul">{bullets}</ul> }.into_any());
    }
    view! {
        <section class="dr-card" aria-label=s.title>
            <div class="dr-head">
                <h2>{s.title}</h2>
                {s.note.map(|n| view! { <span class="dr-count">{n}</span> })}
            </div>
            {body}
        </section>
    }
    .into_any()
}

/// The banner and the sections of `text()`. Each section is drawn from its own
/// memo, so new text redraws only the section it changes: what is already
/// written stays in the page, and can be read and selected.
fn draft_view(
    read: fn(&str) -> Vec<Section>,
    text: impl Fn() -> String + Send + Sync + 'static,
) -> AnyView {
    let sections = Memo::new(move |_| read(&text()));
    let cards = (0..sections.with_untracked(Vec::len))
        .map(|i| {
            let one = Memo::new(move |_| sections.with(|s| s.get(i).cloned()));
            view! { {move || one.get().map(section_view)} }.into_any()
        })
        .collect::<Vec<AnyView>>();
    view! {
        <style>{DRAFT_STYLE}</style>
        <div class="dr">
            <p class="dr-banner" role="status">{BANNER}</p>
            {cards}
        </div>
    }
    .into_any()
}

/// The answer as far as it is written, following `live`.
pub(super) fn answer_view(live: RwSignal<Option<String>>) -> AnyView {
    draft_view(read_answer, move || live.get().unwrap_or_default())
}

/// The opinion as far as it is written, following `live`.
pub(super) fn opinion_view(live: Memo<Option<String>>) -> AnyView {
    draft_view(read_opinion, move || live.get().unwrap_or_default())
}

/// The results card folded to a row with the clock; it opens to the card.
pub(super) fn fold_results(ui: TrackerUi, from: DateTime<Utc>, card: AnyView) -> AnyView {
    let toggle = move |ev: leptos::ev::MouseEvent| {
        ev.prevent_default();
        ui.results_open.update(|v| *v = !*v);
    };
    view! {
        <style>{DRAFT_STYLE}</style>
        <details class="dr-fold" prop:open=move || ui.results_open.get()>
            <summary on:click=toggle>
                <span>"Results so far"</span>
                <span class="dr-clock">{move || rt::clock_text((ui.now.get() - from).num_seconds())}</span>
            </summary>
            {card}
        </details>
    }
    .into_any()
}
