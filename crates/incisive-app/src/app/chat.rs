//! The chat window on the run page (UI only; the rules are in
//! [`crate::chat_page`]), to the frozen mockup of 3 Nov.
//!
//! The Chat option sits beside "Answer | Expert opinion" ([`Chat::tab`]) and
//! is drawn only with `CHAT=on` (a page with it off has none of these pieces,
//! nor [`CHAT_STYLE`]). `#chat` opens the view, also before hydration (CSS
//! `:target`, as `#expert` does).
//!
//! Before hydration every control still works: the question, a thumb and a
//! suggestion are plain form posts to their server functions, which send the
//! browser back to the run page. A question accepted that way lands on
//! `?chat_wait=1`, where the page reloads itself until the turn finishes. Once
//! hydrated, forms are handled in place and a pending turn is read every
//! 700 ms while the page is visible.
//!
//! Everything is escaped text; no `inner_html`. Nothing here logs a question
//! or a reply.

use std::time::Duration;

use leptos::prelude::*;
use uuid::Uuid;

use super::expert::{Expert, current_hash, replace_hash};
use super::tracker::{Pill, chat_pill_off};
use super::{
    chat_ask, chat_feedback, chat_source, chat_suggest, chat_turn, error_text, get_my_run_chat,
};
use crate::chat::lines;
use crate::chat_page::{
    self as cp, AskReply, ChatPageView, ChatTurn, Entry, Phase, Refusal, ReplyPiece, SheetSource,
};
use crate::expert_page::{POLL_EVERY_SECS, POLL_MAX};
use crate::run_tracker::{self as rt, PillState};

/// The chat's styles: the frozen mockup's sizes on the ivory tokens. Scoped to
/// `.rx-chat` and its own option pieces; nothing here matches the answer.
pub const CHAT_STYLE: &str = r##"
.rx-chat-note { font-family:var(--iv-font); margin:0 0 10px; font-size:14.5px; line-height:1.45; color:var(--iv-body); }
.rx-chat[hidden] { display:none; }
.rx-chat { font-family:var(--iv-font); color:var(--iv-ink); display:flex; flex-direction:column; gap:14px; width:100%; max-width:760px; margin:6px auto 0; min-width:0; overflow-wrap:anywhere; }
.rx-convo { display:flex; flex-direction:column; gap:14px; min-width:0; }
.rx-empty { display:flex; flex-direction:column; gap:10px; }
.rx-intro { margin:0; font-size:15px; line-height:1.45; color:var(--iv-body); }
.rx .rx-starter { font:inherit; text-align:left; background:var(--iv-card); border:1px solid var(--iv-line); border-radius:16px; padding:12px 14px; min-height:44px; font-size:15px; font-weight:500; color:var(--iv-ink); line-height:1.35; cursor:pointer; width:100%; }
.rx-turn { display:flex; flex-direction:column; gap:8px; min-width:0; }
.rx-q { align-self:flex-end; max-width:86%; background:var(--iv-accent); color:#fff; border-radius:16px; padding:10px 14px; font-size:15px; line-height:1.4; white-space:pre-wrap; }
.rx-reply { background:var(--iv-card); border:1px solid var(--iv-border); border-radius:16px; padding:12px 14px; display:flex; flex-direction:column; gap:10px; min-width:0; }
.rx-wait { font-size:14px; color:var(--iv-muted); line-height:1.4; }
.rx-text { font-size:15.5px; line-height:1.5; color:var(--iv-ink); white-space:pre-wrap; }
.rx .rx-source { position:relative; font:inherit; border:0; background:var(--iv-tint); color:var(--iv-accent); font-size:13px; font-weight:700; border-radius:8px; padding:3px 8px; margin:0 2px; cursor:pointer; }
.rx .rx-source::after { content:''; position:absolute; left:0; right:0; top:50%; height:44px; transform:translateY(-50%); }
.rx-writing { font-size:13px; font-weight:600; color:var(--iv-warn); background:var(--iv-warn-bg); border-radius:10px; padding:7px 10px; }
.rx-cut { font-size:13px; line-height:1.35; color:var(--iv-muted); }
.rx-flags { background:var(--iv-warn-bg); border-radius:10px; padding:8px 10px; display:flex; flex-direction:column; gap:4px; }
.rx-flags-h { font-size:13px; font-weight:700; color:var(--iv-warn); }
.rx-flag { font-size:13px; line-height:1.35; color:#6B4600; }
.rx-line-surgeon { font-size:14.5px; font-weight:600; color:var(--iv-accent); background:var(--iv-tint); border-radius:10px; padding:9px 12px; line-height:1.4; }
.rx-line-safety { font-size:14.5px; font-weight:700; color:var(--iv-alert); background:#FDECEA; border-radius:10px; padding:9px 12px; line-height:1.4; }
.rx-acts { display:flex; align-items:center; gap:4px; flex-wrap:wrap; }
.rx-acts form { display:contents; }
.rx .rx-thumb { width:44px; height:44px; padding:0; border-radius:999px; border:0; display:flex; align-items:center; justify-content:center; cursor:pointer; background:none; color:var(--iv-muted); }
.rx .rx-thumb[aria-pressed="true"] { background:var(--iv-tint); color:var(--iv-accent); }
.rx .rx-suggest { font:inherit; border:0; background:none; color:var(--iv-accent); font-size:14px; font-weight:600; min-height:44px; padding:0 8px; cursor:pointer; }
.rx .rx-suggest[disabled] { cursor:default; opacity:1; }
.rx-composer { position:sticky; bottom:0; z-index:5; margin:0; padding:8px 0 6px; background:var(--iv-ground); border-top:1px solid var(--iv-border); display:flex; flex-direction:column; gap:6px; }
.rx-modes { display:flex; gap:2px; align-self:flex-start; background:var(--iv-card); border:1px solid var(--iv-border); border-radius:999px; padding:2px; }
.rx .rx-modes label { display:flex; align-items:center; margin:0; min-height:44px; padding:0 14px; border-radius:999px; font-size:14px; font-weight:700; color:var(--iv-muted); cursor:pointer; }
.rx .rx-modes input:checked + label { background:var(--iv-tint); color:var(--iv-accent); }
.rx .rx-modes input:focus-visible + label { outline:2px solid var(--iv-accent); outline-offset:2px; }
.rx-ask { display:flex; gap:8px; align-items:flex-end; }
.rx .rx-ask textarea { flex:1 1 auto; width:auto; min-width:0; resize:none; field-sizing:content; max-height:140px; border:1px solid var(--iv-line); border-radius:16px; padding:12px 14px; font-family:var(--iv-font); font-size:15px; line-height:1.3; background:var(--iv-card); color:var(--iv-ink); min-height:46px; }
.rx .rx-send { flex:none; width:46px; height:46px; padding:0; border-radius:999px; border:0; display:flex; align-items:center; justify-content:center; color:#fff; background:var(--iv-accent); cursor:pointer; }
.rx .rx-send[disabled] { background:var(--iv-muted); opacity:1; cursor:default; }
.rx-foot { font-size:12px; line-height:1.35; color:var(--iv-muted); }
.rx-back { position:fixed; inset:0; z-index:50; background:rgba(28,27,25,.42); display:flex; align-items:flex-end; justify-content:center; }
.rx-sheet { background:var(--iv-card); width:100%; max-width:760px; max-height:80vh; overflow-y:auto; border-radius:20px 20px 0 0; padding:16px 16px 20px; display:flex; flex-direction:column; gap:10px; }
.rx-sheet-head { position:sticky; top:0; z-index:1; background:var(--iv-card); display:flex; align-items:center; justify-content:space-between; gap:8px; }
.rx-sheet-src { font-size:13px; font-weight:700; color:var(--iv-muted); }
.rx .rx-close { font:inherit; flex:none; border:0; background:var(--iv-tint); color:var(--iv-accent); font-size:14px; font-weight:700; border-radius:999px; padding:0 14px; min-height:44px; cursor:pointer; }
.rx-sheet-text { font-size:15px; line-height:1.5; color:var(--iv-ink); white-space:pre-wrap; }
.rx-end { scroll-margin-bottom:170px; }
.rx-retry { margin:0; }
.rx-refusals { font-family:var(--iv-font); display:flex; flex-direction:column; gap:10px; max-width:760px; margin:0 0 10px; overflow-wrap:anywhere; }
.rx-chat a:focus-visible, .rx-chat button:focus-visible, .rx-chat textarea:focus-visible { outline:2px solid var(--iv-accent); outline-offset:2px; }
/* The chat view shows the title and the option only, as in the mockup. */
.rx-chaton .run-chrome { max-width:760px; margin-inline:auto; }
.rx-chaton .run-question { font-size:20px; font-weight:700; line-height:1.3; }
.rx-chaton .run-chrome > :not(.run-question):not(.rx-seg):not(.rx-chat-note) { display:none; }
/* Before hydration: #chat opens a ready view (CSS :target) and hides the rest. */
.rx-pre .rx-chat:target:not(:empty)[hidden] { display:flex; }
.rx-pre:has(.rx-chat:target:not(:empty)) .rx-answer { display:none; }
.rx-pre:has(.rx-chat:target:not(:empty)) .rx-tab.on { background:none; color:var(--iv-muted); }
.rx-pre:has(.rx-chat:target:not(:empty)) a.rx-tab[href="#chat"] { background:var(--iv-tint); color:var(--iv-accent); }
.rx-pre:has(.rx-chat:target:not(:empty)) .run-chrome { max-width:760px; margin-inline:auto; }
.rx-pre:has(.rx-chat:target:not(:empty)) .run-question { font-size:20px; font-weight:700; line-height:1.3; }
.rx-pre:has(.rx-chat:target:not(:empty)) .run-chrome > :not(.run-question):not(.rx-seg):not(.rx-chat-note) { display:none; }
@media (max-width:640px) {
  /* Above the phone's fixed nav. */
  .rx-composer { bottom:calc(78px + env(safe-area-inset-bottom)); }
  .rx-end { scroll-margin-bottom:calc(250px + env(safe-area-inset-bottom)); }
  /* Under 16 px an iPhone zooms on focus. */
  .rx .rx-ask textarea { font-size:16px; }
  .rx-chaton .run-question, .rx-pre:has(.rx-chat:target:not(:empty)) .run-question { font-size:15px; }
}
"##;

/// The id of the option's Chat tab (labels the view's panel).
const TAB_ID: &str = "rx-tab-chat";
const FORM_ID: &str = "rx-chat-form";
const QUESTION_ID: &str = "rx-chat-q";
const END_ID: &str = "rx-chat-end";
const CLOSE_ID: &str = "rx-sheet-close";
/// Thumb values as stored.
const UP: i64 = 1;
const DOWN: i64 = -1;

/// What the address said after a form post.
pub(super) struct ChatQuery {
    /// The page reloads itself while a turn is pending.
    pub wait: bool,
    pub refused: Option<Refusal>,
}

impl ChatQuery {
    pub fn read(
        wait: Option<String>,
        refused: Option<String>,
        safety: Option<String>,
        hints: Option<String>,
    ) -> Self {
        ChatQuery {
            wait: wait.as_deref() == Some("1"),
            refused: refused.and_then(|c| {
                let hints = hints.as_deref().unwrap_or_default();
                cp::refusal_from_code(&c, safety.as_deref() == Some("1"), hints)
            }),
        }
    }
}

/// The chat's state on the run page (signals; copied into views).
#[derive(Clone, Copy)]
pub(super) struct Chat {
    run_id: Uuid,
    /// The chat view is shown (`#chat`).
    pub open: RwSignal<bool>,
    view: RwSignal<ChatPageView>,
    /// Refusals this page was given, with how many turns there were then.
    refusals: RwSignal<Vec<(usize, Refusal)>>,
    /// A question is on its way to the server.
    sending: RwSignal<bool>,
    sheet: RwSignal<Option<SheetSource>>,
    /// The page is not visible, so nothing is polled.
    hidden: RwSignal<bool>,
    state: Memo<cp::ChatState>,
    pending: Memo<Option<i64>>,
    /// Came from a form post (see [`ChatQuery`]); cleared on hydration.
    wait: RwSignal<cp::PageReload>,
    /// The citation read the sheet is waiting for: a stale reply is dropped.
    sheet_request: StoredValue<u64>,
    /// Reads of a pending turn are failing: why.
    connection: RwSignal<Option<String>>,
}

/// `tick` every `period` while `run` says so (browser only: effects run only
/// there), stopped on unmount.
fn ticker(period: Duration, run: impl Fn() -> bool + 'static, tick: impl Fn() + 'static) {
    type Timer = Option<leptos::leptos_dom::helpers::IntervalHandle>;
    let timer: StoredValue<Timer, LocalStorage> = StoredValue::new_local(None);
    let stop = move || {
        if let Some(h) = timer.try_update_value(|t| t.take()).flatten() {
            h.clear();
        }
    };
    let tick = StoredValue::new_local(tick);
    Effect::new(move |_| {
        if !run() {
            return stop();
        }
        if timer.with_value(|t| t.is_none()) {
            let h = leptos::leptos_dom::helpers::set_interval_with_handle(
                move || tick.with_value(|t| t()),
                period,
            );
            timer.set_value(h.ok());
        }
    });
    on_cleanup(stop);
}

/// Whether the browser is hiding this page (browser only).
fn page_hidden() -> bool {
    #[cfg(feature = "hydrate")]
    {
        web_sys::window()
            .and_then(|w| w.document())
            .is_some_and(|d| d.hidden())
    }
    #[cfg(not(feature = "hydrate"))]
    false
}

/// Take the form post's marks out of the address (browser only), so a reload
/// does not show an old refusal again.
fn clean_address() {
    #[cfg(feature = "hydrate")]
    {
        let Some(w) = web_sys::window() else { return };
        let loc = w.location();
        if !loc.search().unwrap_or_default().contains("chat_") {
            return;
        }
        let url = format!(
            "{}{}",
            loc.pathname().unwrap_or_default(),
            loc.hash().unwrap_or_default()
        );
        if let Ok(h) = w.history() {
            let _ = h.replace_state_with_url(&wasm_bindgen::JsValue::NULL, "", Some(&url));
        }
    }
}

/// The question box's text and the chosen style, as the page now has them,
/// whatever was typed before hydration (browser only).
fn read_form() -> Option<(String, String)> {
    #[cfg(feature = "hydrate")]
    {
        use wasm_bindgen::JsCast;
        let d = web_sys::window()?.document()?;
        let q = d
            .get_element_by_id(QUESTION_ID)?
            .dyn_into::<web_sys::HtmlTextAreaElement>()
            .ok()?
            .value();
        let mode = d
            .query_selector(&format!("#{FORM_ID} input[name=\"mode\"]:checked"))
            .ok()??
            .get_attribute("value")?;
        Some((q, mode))
    }
    #[cfg(not(feature = "hydrate"))]
    None
}

/// Empty the question box (browser only).
fn clear_question() {
    #[cfg(feature = "hydrate")]
    {
        use wasm_bindgen::JsCast;
        if let Some(t) = web_sys::window()
            .and_then(|w| w.document())
            .and_then(|d| d.get_element_by_id(QUESTION_ID))
            .and_then(|e| e.dyn_into::<web_sys::HtmlTextAreaElement>().ok())
        {
            t.set_value("");
        }
    }
}

/// Bring the end of the conversation into view, above the sticky question box
/// (browser only). On the next frame: what was just added is not drawn yet.
/// The box grows with its draft, so its height is measured now and sets the
/// end's scroll margin; the CSS value stands until then and if this fails.
fn scroll_to_end() {
    #[cfg(feature = "hydrate")]
    request_animation_frame(|| {
        use wasm_bindgen::JsCast;
        let Some(w) = web_sys::window() else { return };
        let Some(d) = w.document() else { return };
        let Some(end) = d.get_element_by_id(END_ID) else {
            return;
        };
        if let (Some(form), Ok(end)) = (
            d.get_element_by_id(FORM_ID),
            end.clone().dyn_into::<web_sys::HtmlElement>(),
        ) {
            let under = w
                .get_computed_style(&form)
                .ok()
                .flatten()
                .and_then(|s| s.get_property_value("bottom").ok())
                .map_or(0.0, |b| cp::px_of(&b));
            let margin = cp::end_scroll_margin(form.get_bounding_client_rect().height(), under);
            let _ = end
                .style()
                .set_property("scroll-margin-bottom", &format!("{margin}px"));
        }
        end.scroll_into_view_with_bool(false);
    });
}

/// Move focus to the sheet's Close on the next frame, once it is drawn
/// (browser only).
fn focus_close() {
    #[cfg(feature = "hydrate")]
    request_animation_frame(|| {
        use wasm_bindgen::JsCast;
        if let Some(e) = web_sys::window()
            .and_then(|w| w.document())
            .and_then(|d| d.get_element_by_id(CLOSE_ID))
            .and_then(|e| e.dyn_into::<web_sys::HtmlElement>().ok())
        {
            let _ = e.focus();
        }
    });
}

impl Chat {
    /// The state for `run_id`'s chat `view`. In the browser: follows the hash,
    /// reads a pending turn and watches a chat that is not ready yet.
    pub fn new(run_id: Uuid, view: ChatPageView, query: ChatQuery, x: Expert) -> Chat {
        let refused: Vec<(usize, Refusal)> = query
            .refused
            .into_iter()
            .map(|r| (view.turns.len(), r))
            .collect();
        let view = RwSignal::new(view);
        let c = Chat {
            run_id,
            open: RwSignal::new(false),
            view,
            refusals: RwSignal::new(refused),
            sending: RwSignal::new(false),
            sheet: RwSignal::new(None),
            hidden: RwSignal::new(false),
            state: Memo::new(move |_| view.with(|v| v.state)),
            pending: Memo::new(move |_| {
                view.with(|v| v.turns.iter().find(|t| t.pending()).map(|t| t.id))
            }),
            wait: RwSignal::new(cp::PageReload::from_address(query.wait)),
            sheet_request: StoredValue::new(0),
            connection: RwSignal::new(None),
        };
        // Effects run in the browser only: the server draws the Answer view.
        Effect::new(move |_| {
            clean_address();
            c.wait.set(c.wait.get_untracked().hydrated());
            c.hidden.set(page_hidden());
        });
        Effect::new(move |_| {
            // Also when the chat turns ready under an open `#chat`.
            let ready = c.state.get().selectable();
            let want = ready && current_hash() == "#chat";
            if c.open.get_untracked() != want {
                c.open.set(want);
            }
        });
        #[cfg(feature = "hydrate")]
        {
            let hash = window_event_listener(leptos::ev::hashchange, move |_| {
                let want = c.state.get_untracked().selectable() && current_hash() == "#chat";
                if c.open.get_untracked() != want {
                    c.open.set(want);
                }
            });
            on_cleanup(move || hash.remove());
            // `visibilitychange` bubbles from the document to the window.
            let seen = leptos::leptos_dom::helpers::window_event_listener_untyped(
                "visibilitychange",
                move |_| c.hidden.set(page_hidden()),
            );
            on_cleanup(move || seen.remove());
        }
        c.poll_turn();
        c.watch_state(x);
        c
    }

    /// While a turn is pending and the page is visible: read it every 700 ms,
    /// one read at a time. Coming back to the page reads the stored result.
    /// Repeated failures slow the reads down and say so, as the run page does.
    fn poll_turn(self) {
        let reading = StoredValue::new(false);
        let failures = StoredValue::new(0u32);
        let skip = StoredValue::new(0u32);
        // Coming back to the page reads at the next tick, even if a read
        // from before never settled.
        Effect::new(move |_| {
            if !self.hidden.get() {
                skip.set_value(0);
                reading.set_value(false);
            }
        });
        ticker(
            Duration::from_millis(cp::POLL_TURN_MS),
            move || cp::should_poll(self.pending.get().is_some(), self.hidden.get()),
            move || {
                let Some(id) = self.pending.get_untracked() else {
                    return;
                };
                if reading.get_value() {
                    return;
                }
                if skip.get_value() > 0 {
                    return skip.update_value(|s| *s -= 1);
                }
                reading.set_value(true);
                leptos::task::spawn_local(async move {
                    let failed = match chat_turn(id).await {
                        Ok(t) => {
                            self.put_turn(t);
                            None
                        }
                        // The turn or its run has gone: stop asking.
                        Err(e) if error_text(&e) == lines::NOT_FOUND_MESSAGE => {
                            self.drop_turn(id);
                            None
                        }
                        // A transient error keeps the last text.
                        Err(e) => Some(error_text(&e)),
                    };
                    let n = match failed {
                        None => 0,
                        Some(_) => failures
                            .try_get_value()
                            .unwrap_or_default()
                            .saturating_add(1),
                    };
                    let _ = failures.try_update_value(|f| *f = n);
                    let _ = skip.try_update_value(|s| *s = cp::ticks_between_reads(n) - 1);
                    let shown = failed.filter(|_| n >= cp::CONNECTION_AFTER_FAILURES);
                    let _ = self.connection.try_maybe_update(|c| {
                        let changed = *c != shown;
                        *c = shown;
                        (changed, ())
                    });
                    let _ = reading.try_update_value(|r| *r = false);
                });
            },
        );
    }

    /// While the chat is getting ready, or its opinion is being written or has
    /// finished on this page: read its state on the opinion's own schedule,
    /// while the page is visible, so "Chat · Getting ready" turns into Chat and
    /// the option appears with no reload.
    fn watch_state(self, x: Expert) {
        let count = RwSignal::new(0u32);
        let was_writing = RwSignal::new(false);
        // The opinion finishing starts the wait for its record afresh.
        Effect::new(move |before: Option<bool>| {
            let writing = x.opinion_writing();
            if writing {
                was_writing.set(true);
            } else if before == Some(true) {
                count.set(0);
            }
            writing
        });
        ticker(
            Duration::from_secs(POLL_EVERY_SECS),
            move || {
                cp::reads_state(self.state.get(), x.opinion_writing(), was_writing.get())
                    && count.get() < POLL_MAX
                    && !self.hidden.get()
            },
            move || {
                count.update(|n| *n += 1);
                let id = self.run_id;
                leptos::task::spawn_local(async move {
                    if let Ok(Some(v)) = get_my_run_chat(id).await {
                        let _ = self.view.try_maybe_update(|c| {
                            let changed = c.state != v.state || c.starters != v.starters;
                            c.state = v.state;
                            c.starters = v.starters;
                            (changed, ())
                        });
                    }
                });
            },
        );
    }

    /// Store a turn just read. An unchanged read touches nothing, so the
    /// conversation is not redrawn and focus stays where it is.
    fn put_turn(self, new: ChatTurn) {
        let _ = self.view.try_maybe_update(|v| {
            let changed = match v.turns.iter_mut().find(|t| t.id == new.id) {
                Some(t) if *t == new => false,
                Some(t) => {
                    *t = new;
                    true
                }
                None => {
                    v.turns.push(new);
                    true
                }
            };
            (changed, ())
        });
    }

    fn drop_turn(self, id: i64) {
        let _ = self.view.try_update(|v| v.turns.retain(|t| t.id != id));
    }

    fn busy(self) -> bool {
        self.sending.get() || self.pending.get().is_some()
    }

    /// Show a refusal at the end of the conversation.
    fn refuse(self, r: Refusal) {
        // After an await: the page may have gone.
        let Some(at) = self.view.try_with_untracked(|v| v.turns.len()) else {
            return;
        };
        let _ = self.refusals.try_update(|l| l.push((at, r)));
        scroll_to_end();
    }

    /// Ask `question`. A refusal, or a connection that fails, is shown in the
    /// conversation with the safety line if the question needs it.
    fn ask(self, question: String, typed: bool) {
        if self.sending.get_untracked() || self.pending.get_untracked().is_some() {
            return;
        }
        let mode = read_form().map_or_else(
            || {
                self.view
                    .with_untracked(|v| cp::default_mode(&v.turns).to_string())
            },
            |(_, m)| m,
        );
        self.sending.set(true);
        let run = self.run_id;
        leptos::task::spawn_local(async move {
            let lost = match chat_ask(run, question.clone(), mode, None).await {
                Ok(AskReply::Accepted(t)) => {
                    if typed {
                        clear_question();
                    }
                    self.put_turn(t);
                    scroll_to_end();
                    false
                }
                Ok(AskReply::Refused(r)) => {
                    let in_flight = r.message == lines::IN_FLIGHT_MESSAGE;
                    self.refuse(r);
                    in_flight
                }
                // The line shows now, whatever the read below finds.
                Err(e) => {
                    self.refuse(Refusal {
                        safety_line: lines::warning_symptoms(&question),
                        question: question.clone(),
                        message: error_text(&e),
                    });
                    true
                }
            };
            let _ = self.sending.try_set(false);
            if lost {
                self.recover(question, typed).await;
            }
        });
    }

    /// The ask's answer was lost, or another reply is in flight: the server may
    /// hold a turn this page has not seen. Read the chat and merge it, so
    /// polling picks up a pending one. If this very question was stored, its
    /// turn replaces the refusal.
    async fn recover(self, question: String, typed: bool) {
        let Ok(Some(read)) = get_my_run_chat(self.run_id).await else {
            return;
        };
        let merged = self.view.try_maybe_update(|v| {
            let stored = cp::stored_unseen(&v.turns, &read.turns, &question);
            let changed = cp::merge_turns(&mut v.turns, read.turns);
            (changed, (changed, stored))
        });
        let Some((changed, stored)) = merged else {
            return;
        };
        if stored {
            let _ = self
                .refusals
                .try_update(|l| l.retain(|(_, r)| r.question != question));
            // Only if it is still this question: the reader may have typed on.
            if typed && read_form().is_some_and(|(q, _)| cp::box_holds(&q, &question)) {
                clear_question();
            }
        }
        if changed {
            scroll_to_end();
        }
    }

    fn give_feedback(self, id: i64, value: i64) {
        let run = self.run_id;
        leptos::task::spawn_local(async move {
            match chat_feedback(run, id, value).await {
                Ok(()) => {
                    let _ = self.view.try_update(|v| {
                        if let Some(t) = v.turns.iter_mut().find(|t| t.id == id) {
                            t.feedback = i16::try_from(value).ok();
                        }
                    });
                }
                Err(e) => self.refuse(Refusal {
                    question: String::new(),
                    message: error_text(&e),
                    safety_line: false,
                }),
            }
        });
    }

    fn suggest(self, id: i64) {
        let run = self.run_id;
        leptos::task::spawn_local(async move {
            match chat_suggest(run, id).await {
                Ok(()) => {
                    let _ = self.view.try_update(|v| {
                        if let Some(t) = v.turns.iter_mut().find(|t| t.id == id) {
                            t.suggested = true;
                        }
                    });
                }
                Err(e) => self.refuse(Refusal {
                    question: String::new(),
                    message: error_text(&e),
                    safety_line: false,
                }),
            }
        });
    }

    /// Open a citation's sheet. Its text comes from the same scoped read as
    /// the REST route.
    fn open_source(self, stored_id: i64) {
        self.sheet_request.update_value(|n| *n += 1);
        let request = self.sheet_request.get_value();
        self.sheet.set(Some(SheetSource {
            label: String::new(),
            text: "Loading\u{2026}".to_string(),
        }));
        leptos::task::spawn_local(async move {
            let shown = match chat_source(stored_id).await {
                Ok(s) => s,
                Err(e) => SheetSource {
                    label: String::new(),
                    text: error_text(&e),
                },
            };
            // A newer citation's sheet, or the page gone: this reply is stale.
            if self.sheet_request.try_get_value() != Some(request) {
                return;
            }
            let _ = self.sheet.try_update(|s| {
                if s.is_some() {
                    *s = Some(shown);
                }
            });
        });
    }

    /// The Chat with me pill: a link once it is ready, otherwise a disabled
    /// button; visible and not clickable where chat is not offered.
    pub fn tab(self, x: Expert) -> impl IntoView {
        let ui = x.ui;
        // When this page first saw the opinion ready.
        let since: StoredValue<Option<chrono::DateTime<chrono::Utc>>> = StoredValue::new(None);
        let to_chat = move |ev: leptos::ev::MouseEvent| {
            ev.prevent_default();
            ui.clicked.set(Some(Pill::Chat));
            x.expert.set(false);
            self.open.set(true);
            replace_hash("#chat");
        };
        move || {
            let s = self.state.get();
            let Some(label) = s.tab_label() else {
                return chat_pill_off();
            };
            if s.selectable() {
                view! {
                    <a
                        class="rx-tab"
                        id=TAB_ID
                        class:on=move || self.open.get()
                        role="tab"
                        aria-selected=move || self.open.get().to_string()
                        aria-controls="chat"
                        aria-label=PillState::Ready.aria(label)
                        data-state=PillState::Ready.as_str()
                        data-clicked=ui.clicked_attr(Pill::Chat)
                        href="#chat"
                        rel="external"
                        on:click=to_chat
                    >
                        <span class="rk-dot" aria-hidden="true"></span>
                        {label}
                    </a>
                }
                .into_any()
            } else {
                // Getting ready: fills by the usual time from when this page
                // saw the opinion ready; unavailable: the fill has stopped.
                let state = if s == cp::ChatState::GettingReady {
                    PillState::Filling
                } else {
                    PillState::Stopped
                };
                let style = move || {
                    if state == PillState::Filling {
                        let now = ui.now.get();
                        let secs = if x.opinion_ready() {
                            let from = since.get_value().unwrap_or(now);
                            since.set_value(Some(from));
                            (now - from).num_seconds()
                        } else {
                            0
                        };
                        rt::fill_background(rt::timed_fill(secs, rt::CHAT_USUAL_SECS))
                    } else {
                        String::new()
                    }
                };
                view! {
                    <button
                        class="rx-tab"
                        id=TAB_ID
                        type="button"
                        role="tab"
                        aria-selected="false"
                        aria-label=state.aria(label)
                        data-state=state.as_str()
                        style=style
                        disabled=true
                    >
                        {label}
                    </button>
                }
                .into_any()
            }
        }
    }

    /// Whether the option has a third tab.
    pub fn offered(self) -> bool {
        self.state.get().tab_label().is_some()
    }

    /// The line under the option when chat is down.
    pub fn note(self) -> impl IntoView {
        move || {
            self.state
                .get()
                .note()
                .map(|n| view! { <p class="rx-chat-note" role="status">{n}</p> })
        }
    }

    /// The chat view (`#chat`): drawn once ready, hidden unless chosen. A
    /// refusal shows whatever the state, above it: its safety line must be
    /// seen even when the view stays shut.
    pub fn section(self) -> AnyView {
        let ready = Memo::new(move |_| self.state.get().selectable());
        view! {
            {move || {
                (!ready.get())
                    .then(|| {
                        self.refusals
                            .with(|l| {
                                (!l.is_empty())
                                    .then(|| {
                                        let shown = l
                                            .iter()
                                            .map(|(_, r)| refusal_view(r))
                                            .collect_view();
                                        view! { <div class="rx-refusals">{shown}</div> }
                                    })
                            })
                    })
                    .flatten()
            }}
            <section
                class="rx-chat"
                id="chat"
                role="tabpanel"
                aria-labelledby=TAB_ID
                hidden=move || !self.open.get()
            >
                {move || ready.get().then(|| self.body())}
            </section>
        }
        .into_any()
    }

    fn body(self) -> AnyView {
        let reload = cp::refresh_content(self.run_id);
        view! {
            {move || {
                self.wait
                    .get()
                    .tag(self.pending.get().is_some())
                    .then(|| view! { <meta http-equiv="refresh" content=reload.clone() /> })
            }}
            <div class="rx-convo">{move || self.entries()}</div>
            {move || {
                self.connection
                    .get()
                    .map(|e| view! { <p class="hint rx-retry" role="status">{cp::CONNECTION_RETRYING} {e}</p> })
            }}
            <div class="rx-end" id=END_ID></div>
            {composer(self)}
            {move || self.sheet.get().map(|s| sheet_view(self, s))}
        }
        .into_any()
    }

    fn entries(self) -> AnyView {
        self.view.with(|v| {
            self.refusals.with(|refusals| {
                if v.turns.is_empty() && refusals.is_empty() {
                    return empty_view(self, &v.starters);
                }
                cp::conversation(&v.turns, refusals)
                    .into_iter()
                    .map(|e| match e {
                        Entry::Turn(t) => turn_view(self, t),
                        Entry::Refusal(r) => refusal_view(r),
                    })
                    .collect_view()
                    .into_any()
            })
        })
    }
}

/// The empty state: the intro and the starter questions. Each starter is a
/// submit button of the question form.
fn empty_view(c: Chat, starters: &[String]) -> AnyView {
    let buttons = starters
        .iter()
        .map(|q| {
            let asked = q.clone();
            view! {
                <button
                    class="rx-starter"
                    type="submit"
                    form=FORM_ID
                    name="starter"
                    value=q.clone()
                    on:click=move |ev: leptos::ev::MouseEvent| {
                        ev.prevent_default();
                        c.ask(asked.clone(), false);
                    }
                >
                    {q.clone()}
                </button>
            }
            .into_any()
        })
        .collect_view();
    view! {
        <div class="rx-empty">
            <p class="rx-intro">{cp::EMPTY_INTRO}</p>
            {buttons}
        </div>
    }
    .into_any()
}

fn safety_line() -> AnyView {
    view! { <div class="rx-line-safety">{lines::WARNING_LINE}</div> }.into_any()
}

fn refusal_view(r: &Refusal) -> AnyView {
    let bubble = (!r.question.is_empty()).then(|| {
        view! { <div class="rx-q">{r.question.clone()}</div> }
    });
    view! {
        <div class="rx-turn">
            {bubble}
            <div class="rx-reply">
                <div class="rx-text">{r.message.clone()}</div>
                {r.safety_line.then(safety_line)}
            </div>
        </div>
    }
    .into_any()
}

fn thumb_icon(up: bool) -> AnyView {
    let d = if up {
        "M7 11v9H4v-9h3zM7 11l4-8a2.5 2.5 0 0 1 2.5 2.5V9h5a2 2 0 0 1 2 2.3l-1 6.7a2 2 0 0 1-2 1.7H7"
    } else {
        "M17 13V4h3v9h-3zM17 13l-4 8a2.5 2.5 0 0 1-2.5-2.5V15h-5a2 2 0 0 1-2-2.3l1-6.7a2 2 0 0 1 2-1.7H17"
    };
    view! {
        <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
            <path d=d></path>
        </svg>
    }
    .into_any()
}

/// A thumb: a form post before hydration, handled in place after it.
fn thumb(c: Chat, t: &ChatTurn, value: i64) -> AnyView {
    let id = t.id;
    let up = value == UP;
    let pressed = t.feedback.map(i64::from) == Some(value);
    view! {
        <form method="post" action="/api/fn/chat_feedback">
            <input type="hidden" name="run_id" value=c.run_id.to_string() />
            <input type="hidden" name="turn_id" value=id.to_string() />
            <button
                class="rx-thumb"
                type="submit"
                name="value"
                value=value.to_string()
                aria-label=if up { "Helpful" } else { "Not helpful" }
                aria-pressed=pressed.to_string()
                on:click=move |ev: leptos::ev::MouseEvent| {
                    ev.prevent_default();
                    c.give_feedback(id, value);
                }
            >
                {thumb_icon(up)}
            </button>
        </form>
    }
    .into_any()
}

fn suggest_button(c: Chat, t: &ChatTurn) -> AnyView {
    let id = t.id;
    view! {
        <form method="post" action="/api/fn/chat_suggest">
            <input type="hidden" name="run_id" value=c.run_id.to_string() />
            <input type="hidden" name="turn_id" value=id.to_string() />
            <button
                class="rx-suggest"
                type="submit"
                disabled=t.suggested
                on:click=move |ev: leptos::ev::MouseEvent| {
                    ev.prevent_default();
                    c.suggest(id);
                }
            >
                {t.suggest_label()}
            </button>
        </form>
    }
    .into_any()
}

/// A reply's text with its citations as buttons.
fn reply_text(c: Chat, t: &ChatTurn) -> AnyView {
    let pieces = t
        .pieces()
        .into_iter()
        .map(|p| match p {
            ReplyPiece::Text(s) => view! { <span>{s}</span> }.into_any(),
            ReplyPiece::Cite { n, source } => view! {
                <button
                    class="rx-source"
                    type="button"
                    aria-label=format!("Open source {n}")
                    on:click=move |_| c.open_source(source)
                >
                    {n.to_string()}
                </button>
            }
            .into_any(),
        })
        .collect_view();
    view! { <div class="rx-text">{pieces}</div> }.into_any()
}

fn flags_view(flags: Vec<String>) -> AnyView {
    let lines = flags
        .into_iter()
        .map(|f| view! { <div class="rx-flag">{f}</div> })
        .collect_view();
    view! {
        <div class="rx-flags">
            <div class="rx-flags-h">{cp::FLAGS_HEAD}</div>
            {lines}
        </div>
    }
    .into_any()
}

fn turn_view(c: Chat, t: &ChatTurn) -> AnyView {
    let phase = t.phase();
    let waiting = (phase == Phase::Waiting).then(|| {
        view! { <div class="rx-wait">{cp::WAITING_LINE}</div> }
    });
    let text = (phase != Phase::Waiting).then(|| reply_text(c, t));
    let writing = (phase == Phase::Writing).then(|| {
        view! { <div class="rx-writing">{cp::WRITING_LINE}</div> }
    });
    let cut = t
        .cut_short()
        .then(|| view! { <div class="rx-cut">{cp::CUT_SHORT_LINE}</div> });
    let flags = t.flag_lines();
    let flags = (!flags.is_empty()).then(|| flags_view(flags));
    let surgeon = t.shows_surgeon().then(|| {
        view! { <div class="rx-line-surgeon">{lines::SEE_YOUR_SURGEON}</div> }
    });
    let safety = t.safety_line.then(safety_line);
    let actions = t.shows_thumbs().then(|| {
        view! {
            <div class="rx-acts">
                {thumb(c, t, UP)}
                {thumb(c, t, DOWN)}
                {t.can_suggest().then(|| suggest_button(c, t))}
            </div>
        }
        .into_any()
    });
    view! {
        <div class="rx-turn">
            <div class="rx-q">{t.question.clone()}</div>
            <div class="rx-reply">
                {waiting}
                {text}
                {writing}
                {cut}
                {flags}
                {surgeon}
                {safety}
                {actions}
            </div>
        </div>
    }
    .into_any()
}

/// The question form: style, question box, send and the footer. A plain form
/// post before hydration, handled in place after it.
fn composer(c: Chat) -> AnyView {
    let mode = c
        .view
        .with_untracked(|v| cp::default_mode(&v.turns).to_string());
    let radio = |value: &'static str, label: &'static str, id: &'static str| {
        view! {
            <input class="iv-sr" type="radio" name="mode" id=id value=value checked=mode == value />
            <label for=id>{label}</label>
        }
        .into_any()
    };
    let on_submit = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        // An empty question is no question: nothing to refuse, and the
        // starters stay. (A form post before hydration gets its message.)
        if let Some((q, _)) = read_form().filter(|(q, _)| !q.trim().is_empty()) {
            c.ask(q, true);
        }
    };
    view! {
        <form class="rx-composer" id=FORM_ID method="post" action="/api/fn/chat_ask" on:submit=on_submit>
            <input type="hidden" name="run_id" value=c.run_id.to_string() />
            <div class="rx-modes" role="group" aria-label="Reply style">
                {radio("clinical", "Clinical", "rx-mode-clinical")}
                {radio("plain", "Plain language", "rx-mode-plain")}
            </div>
            <div class="rx-ask">
                <label class="iv-sr" for=QUESTION_ID>"Your question"</label>
                <textarea id=QUESTION_ID name="question" rows="1" maxlength="1000" placeholder="Ask about this evidence"></textarea>
                <button class="rx-send" type="submit" aria-label="Send" disabled=move || c.busy()>
                    <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
                        <path d="M12 19V5M6 11l6-6 6 6"></path>
                    </svg>
                </button>
            </div>
            <div class="rx-foot">{lines::FOOTER}</div>
        </form>
    }
    .into_any()
}

/// A citation's bottom sheet.
fn sheet_view(c: Chat, s: SheetSource) -> AnyView {
    // Drawn afresh when its text arrives, so each draw takes the focus back.
    focus_close();
    view! {
        <div class="rx-back">
            <div class="rx-sheet" role="dialog" aria-label="Source passage">
                <div class="rx-sheet-head">
                    <div class="rx-sheet-src">{s.label}</div>
                    <button class="rx-close" id=CLOSE_ID type="button" on:click=move |_| c.sheet.set(None)>"Close"</button>
                </div>
                <div class="rx-sheet-text">{s.text}</div>
            </div>
        </div>
    }
    .into_any()
}
