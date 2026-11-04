//! The introduction's views (the rules are in [`crate::intro_page`]): the card
//! while the run works, the folded row after it.
//!
//! The page is drawn again at each poll, so the reveal lives in [`IntroUi`],
//! made once by the run page. Escaped text only; no `inner_html`. Nothing is
//! drawn without a finished text, and nothing here logs it.

use leptos::prelude::*;
use uuid::Uuid;

use crate::intro_page::{self as ip, RevealStep};
use crate::runs::IntroView;

/// The introduction's styles. Drawn only with the introduction, so a page
/// without one is unchanged.
const INTRO_STYLE: &str = r##"
.in-tag { margin-top:3px; font-size:12px; line-height:1.35; color:var(--iv-muted); }
.in-text { display:flex; flex-direction:column; gap:10px; margin-top:10px; }
.in-text p { margin:0; font-size:14.5px; line-height:1.5; color:var(--iv-ink); text-wrap:pretty; }
.in-text p[hidden], .in-caret[hidden], .in-close[hidden] { display:none; }
.in-caret { display:inline-block; width:2px; height:15px; margin-left:2px; vertical-align:-2px; background:var(--iv-accent); animation:in-blink 1s steps(2) infinite; }
@keyframes in-blink { 0%,100% { opacity:1; } 50% { opacity:0; } }
.in-close { margin:10px 0 0; padding-top:10px; border-top:1px solid var(--iv-rule); font-size:14px; line-height:1.4; font-weight:600; color:var(--iv-accent); }
.in-fold { display:block; margin:10px 0; padding:0 16px; background:var(--iv-card); border:1px solid var(--iv-border); border-radius:16px; font-family:var(--iv-font); }
.in-fold > summary { list-style:none; min-height:44px; display:flex; align-items:center; justify-content:space-between; gap:12px; cursor:pointer; }
.in-fold > summary::-webkit-details-marker { display:none; }
.in-fold-title { font-size:15px; font-weight:600; color:var(--iv-ink); }
.in-fold-read { flex:none; font-size:13px; font-weight:600; color:var(--iv-accent); }
.in-fold-body { padding-bottom:14px; }
.in-fold-body .in-tag { margin-top:0; }
@media (max-width:640px) {
  .in-card { padding-bottom:14px; }
}
@media (prefers-reduced-motion:reduce) {
  .in-caret { animation:none; }
}
"##;

/// The reveal moves on this often; [`ip::words_at`] says how far.
const TICK_MS: u32 = 100;

/// How many times, one per poll interval, the page asks for a text still being
/// written when the run ended (the job's limit is about 30 s).
const WAIT_ATTEMPTS: u32 = 20;

/// The introduction's state across the page's redraws.
#[derive(Clone, Copy)]
pub(super) struct IntroUi {
    /// Words out; [`ip::ALL_WORDS`] (also what the server draws) is all of them.
    shown: RwSignal<usize>,
    reveal: StoredValue<ip::Reveal>,
    timer: StoredValue<Option<leptos::leptos_dom::helpers::IntervalHandle>, LocalStorage>,
    /// Time the reveal has run.
    elapsed_ms: StoredValue<u32>,
    /// The folded row, open.
    open: RwSignal<bool>,
}

impl IntroUi {
    /// Made once by the run page.
    pub fn new() -> IntroUi {
        let ui = IntroUi {
            shown: RwSignal::new(ip::ALL_WORDS),
            reveal: StoredValue::new(ip::Reveal::default()),
            timer: StoredValue::new_local(None),
            elapsed_ms: StoredValue::new(0),
            open: RwSignal::new(false),
        };
        on_cleanup(move || ui.stop());
        ui
    }

    fn stop(self) {
        if let Some(h) = self.timer.try_update_value(|t| t.take()).flatten() {
            h.clear();
        }
    }

    /// The latest poll (browser only, from the page's effect).
    pub fn observe(self, active: bool, intro: Option<&IntroView>) {
        let text = match intro {
            Some(IntroView::Done { text }) => Some(text.as_str()),
            _ => None,
        };
        let step = self
            .reveal
            .try_update_value(|r| r.observe(active, text.is_some(), reduced_motion()))
            .unwrap_or(RevealStep::Nothing);
        match (step, text) {
            (RevealStep::Start, Some(text)) => self.start(ip::reveal_parts(&ip::paragraphs(text))),
            (RevealStep::Finish, _) => {
                self.stop();
                self.shown.set(ip::ALL_WORDS);
            }
            _ => {}
        }
    }

    fn start(self, (first, last): (usize, usize)) {
        let total = first + last;
        self.shown.set(0);
        self.elapsed_ms.set_value(0);
        let h = leptos::leptos_dom::helpers::set_interval_with_handle(
            move || {
                let elapsed = self
                    .elapsed_ms
                    .try_update_value(|e| {
                        *e = e.saturating_add(TICK_MS);
                        *e
                    })
                    .unwrap_or(u32::MAX);
                let next = ip::words_at(elapsed, first, last);
                if next >= total {
                    self.stop();
                    self.reveal.try_update_value(ip::Reveal::complete);
                    let _ = self.shown.try_set(ip::ALL_WORDS);
                } else {
                    let _ = self.shown.try_set(next);
                }
            },
            std::time::Duration::from_millis(u64::from(TICK_MS)),
        );
        match h {
            Ok(h) => self.timer.set_value(Some(h)),
            // No timer: the text at once.
            Err(_) => self.shown.set(ip::ALL_WORDS),
        }
    }
}

/// `prefers-reduced-motion` in the browser.
#[cfg(feature = "hydrate")]
fn reduced_motion() -> bool {
    web_sys::window()
        .and_then(|w| w.match_media("(prefers-reduced-motion: reduce)").ok())
        .flatten()
        .is_some_and(|m| m.matches())
}

#[cfg(not(feature = "hydrate"))]
fn reduced_motion() -> bool {
    false
}

/// The run has ended with its introduction still being written: ask for it
/// alone, a fixed number of times, stop when it ends, and redraw the page once
/// if it came. The page is otherwise left alone.
pub(super) fn wait_for_intro(
    id: Uuid,
    tick: RwSignal<u32>,
    wait: StoredValue<super::RowWait, LocalStorage>,
) {
    if wait.with_value(|w| w.started) {
        return;
    }
    let stop = move || {
        if let Some(h) = wait.try_update_value(|w| w.timer.take()).flatten() {
            h.clear();
        }
    };
    let h = leptos::leptos_dom::helpers::set_interval_with_handle(
        move || {
            let attempts = wait
                .try_update_value(|w| {
                    w.attempts += 1;
                    w.attempts
                })
                .unwrap_or(WAIT_ATTEMPTS);
            if attempts >= WAIT_ATTEMPTS {
                stop();
            }
            leptos::task::spawn_local(async move {
                match super::get_my_run_intro(id).await {
                    Ok(Some(IntroView::Pending)) => {}
                    Ok(Some(IntroView::Done { .. })) => {
                        stop();
                        let _ = tick.try_update(|t| *t += 1);
                    }
                    // Failed, skipped, cap blocked or gone: nothing to show.
                    Ok(None) => stop(),
                    // A failed poll says nothing: the next attempt asks again.
                    Err(_) => {}
                }
            });
        },
        super::POLL_EVERY,
    );
    wait.update_value(|w| {
        w.started = true;
        w.timer = h.ok();
    });
}

/// The introduction in front of the results card. With nothing to show it adds
/// nothing at all: the page's markup is the card's alone, as it was.
pub(super) fn with_results(
    ui: IntroUi,
    working: bool,
    intro: Option<IntroView>,
    card: Option<AnyView>,
) -> Option<AnyView> {
    let Some(IntroView::Done { text }) = intro else {
        return card;
    };
    let paragraphs = ip::paragraphs(&text);
    if paragraphs.is_empty() {
        return card;
    }
    let block = if working {
        intro_card(ui, paragraphs)
    } else {
        intro_fold(ui, paragraphs)
    };
    Some(view! { {block} {card} }.into_any())
}

/// While the run works: the card, with the text being revealed or whole, and
/// the closing line once it is complete.
fn intro_card(ui: IntroUi, paragraphs: Vec<String>) -> AnyView {
    let total = ip::word_total(&paragraphs);
    let complete = move || ui.shown.get() >= total;
    // A redraw keeps the caret's blink in step.
    let blink = format!(
        "animation-delay:-{}ms",
        crate::library::now_utc().timestamp_millis() % 1000
    );
    let text = ip::laid_out(paragraphs)
        .into_iter()
        .map(|p| {
            let (for_visible, for_caret, blink) = (p.clone(), p.clone(), blink.clone());
            view! {
                <p hidden=move || !for_visible.visible(ui.shown.get())>
                    {move || p.shown_text(ui.shown.get()).to_string()}
                    <span
                        class="in-caret"
                        aria-hidden="true"
                        style=blink
                        hidden=move || !for_caret.caret(ui.shown.get())
                    ></span>
                </p>
            }
            .into_any()
        })
        .collect::<Vec<AnyView>>();
    let label = move || {
        if complete() {
            ip::HEADING.to_string()
        } else {
            format!("{}, being written", ip::HEADING)
        }
    };
    view! {
        <section class="rk-card in-card" aria-label=label>
            <style>{INTRO_STYLE}</style>
            <h2>{ip::HEADING}</h2>
            <div class="in-tag">{ip::TAG_RUNNING}</div>
            <div class="in-text">{text}</div>
            <p class="in-close" hidden=move || !complete()>{ip::CLOSING}</p>
        </section>
    }
    .into_any()
}

/// After the run: one folded row. Its text is there with no script, and stays
/// open across a redraw.
fn intro_fold(ui: IntroUi, paragraphs: Vec<String>) -> AnyView {
    let toggle = move |ev: leptos::ev::MouseEvent| {
        ev.prevent_default();
        ui.open.update(|v| *v = !*v);
    };
    let text = paragraphs
        .into_iter()
        .map(|p| view! { <p>{p}</p> }.into_any())
        .collect::<Vec<AnyView>>();
    view! {
        <style>{INTRO_STYLE}</style>
        <details class="in-fold" prop:open=move || ui.open.get()>
            <summary on:click=toggle>
                <span class="in-fold-title">{ip::HEADING}</span>
                <span class="in-fold-read">{ip::READ}</span>
            </summary>
            <div class="in-fold-body">
                <div class="in-tag">{ip::TAG_FINISHED}</div>
                <div class="in-text">{text}</div>
            </div>
        </details>
    }
    .into_any()
}
