//! Part A: the interactive answer page (UI only).
//!
//! The blocks in the fixed order of [`ap::PAGE_ORDER`], drawn from a stored
//! [`AnswerV3`] as escaped text, links and CSS boxes (no `inner_html`, no
//! chart library). The rules (which chart, which arms, which marks) are in
//! [`crate::answer_page`]. The page is readable before hydration: outcome
//! cards are `<details>`, citation chips are `#ref-n` links, and each
//! control shows its first or default state until the browser takes over.
//!
//! Marks: every mark the earlier page draws is drawn here too ("flag,
//! never delete"), each with a `data-mark` naming its field (`w:` the
//! writer's, `c:` the completeness review's, `s:` the skeptic's, `e:<pmid>:`
//! a study's funding, `n<k>:` a code-built notable finding). Only the "not
//! traced" mark on a trial or review registration ID is not drawn (item 12;
//! counted under Other notable findings since the folding follow-up).

use std::collections::HashMap;
use std::sync::Arc;

use leptos::prelude::*;

use super::{Mark, pubmed_link};
use crate::agent::answer::{DECISION_SUPPORT, date_ist, duration_text, mode_label, rerun_href};
use crate::agent::answer_v3::{
    ADDED_LABEL, AnswerV3, Certainty, NO_GUIDELINE_SOURCE, Number, Span, WrittenAnswer,
    not_cited_at_end, whole_marked,
};
use crate::answer_page::{self as ap, Block, ChartKind, NumGroup, Piece, SrcType};

// ---------- shared pieces ----------

/// How citations are drawn.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Links {
    /// Chips: `#ref-n` links that open the study sheet after hydration.
    Yes,
    /// Reference numbers as text (inside another link or a summary).
    No,
    /// Chips, and a PubMed link for a PMID with no reference.
    PubMed,
}

/// What a text field is marked with.
#[derive(Clone, Copy)]
pub(super) struct Mk<'a> {
    pub spans: &'a [Span],
    pub mark: Mark,
    /// The `data-mark` namespace.
    pub ns: &'a str,
    /// Item 12: no "not traced" mark on a registration ID.
    pub suppress: bool,
}

/// The id of study sheet `n` (item 11).
pub(super) fn sheet_id(n: u16) -> String {
    format!("sheet-{n}")
}

/// The study sheet an address's hash names: `#ref-<n>` gives `n`.
pub(super) fn sheet_in_hash(hash: &str) -> Option<u16> {
    hash.strip_prefix("#ref-")?.parse().ok()
}

/// Open study sheet `n` (browser only): `showModal`, or the `open`
/// attribute where that is missing. `false` when there is no sheet (the
/// link then goes to the reference, as before hydration).
pub(super) fn open_sheet(n: u16) -> bool {
    #[cfg(feature = "hydrate")]
    {
        use wasm_bindgen::JsCast;
        use web_sys::js_sys::{Function, Reflect};
        let Some(el) = web_sys::window()
            .and_then(|w| w.document())
            .and_then(|d| d.get_element_by_id(&sheet_id(n)))
        else {
            return false;
        };
        match Reflect::get(&el, &"showModal".into())
            .ok()
            .and_then(|f| f.dyn_into::<Function>().ok())
        {
            Some(f) => f.call0(&el).is_ok(),
            None => el.set_attribute("open", "").is_ok(),
        }
    }
    #[cfg(not(feature = "hydrate"))]
    {
        let _ = n;
        false
    }
}

/// A click handler opening sheet `n`; the link is followed when there is
/// none.
pub(super) fn sheet_click(n: u16) -> impl FnMut(leptos::ev::MouseEvent) + Clone + 'static {
    move |ev: leptos::ev::MouseEvent| {
        if open_sheet(n) {
            ev.prevent_default();
            ev.stop_propagation();
        }
    }
}

/// The mark's word and its explanation (as the earlier page's).
fn mark_text(m: Mark) -> (&'static str, &'static str) {
    match m {
        Mark::NotTraced => (
            "not traced",
            "This number was not found in a verified quote, a structured number or an abstract given to the writer.",
        ),
        Mark::NotVerified => (
            "not verified",
            "This number was not found in the source text.",
        ),
    }
}

/// A mark, naming its field.
pub(super) fn mark_el(m: Mark, key: String) -> AnyView {
    let (label, title) = mark_text(m);
    view! { <sup class="flag" title=title data-mark=key>{label}</sup> }.into_any()
}

/// Reference chips for `ids` (the earlier page's markup, plus the sheet).
pub(super) fn chips(ids: &[String], refs: &HashMap<String, u16>, links: Links) -> AnyView {
    let mut nums: Vec<u16> = ids.iter().filter_map(|p| refs.get(p).copied()).collect();
    nums.sort_unstable();
    nums.dedup();
    let chip = if nums.is_empty() {
        ().into_any()
    } else if links == Links::No {
        let n: Vec<String> = nums.iter().map(u16::to_string).collect();
        view! { <span class="cite plain">{format!("[{}]", n.join(", "))}</span> }.into_any()
    } else {
        let last = nums.len() - 1;
        view! {
            <span class="cite">
                "["
                {nums
                    .into_iter()
                    .enumerate()
                    .map(|(i, n)| {
                        view! {
                            <a href=format!("#ref-{n}") rel="external" on:click=sheet_click(n)>{n.to_string()}</a>
                            {(i < last).then_some(", ")}
                        }
                    })
                    .collect_view()}
                "]"
            </span>
        }
        .into_any()
    };
    let unknown: Vec<String> = if links == Links::PubMed {
        ids.iter()
            .filter(|p| !refs.contains_key(p.as_str()))
            .cloned()
            .collect()
    } else {
        Vec::new()
    };
    let pubmed = unknown
        .into_iter()
        .map(|p| match pubmed_link(&p) {
            Some(href) => view! { " " <a href=href rel="noopener noreferrer" target="_blank">{format!("PMID {p}")}</a> }.into_any(),
            None => p.into_any(),
        })
        .collect_view();
    view! { {chip} {pubmed} }.into_any()
}

/// Drawn pieces ([`ap::pieces`]).
pub(super) fn render_pieces(
    ps: Vec<Piece>,
    key: &str,
    mark: Mark,
    refs: &HashMap<String, u16>,
    links: Links,
) -> AnyView {
    ps.into_iter()
        .map(|p| match p {
            Piece::Text(t) => t.into_any(),
            Piece::Cite(ids) => chips(&ids, refs, links),
            Piece::Mark => mark_el(mark, key.to_string()),
            Piece::Suppressed => ().into_any(),
        })
        .collect_view()
        .into_any()
}

/// `text` at `field` with its citations and marks.
pub(super) fn rich(
    text: &str,
    field: &str,
    mk: Mk,
    refs: &HashMap<String, u16>,
    links: Links,
) -> AnyView {
    let known = |p: &str| refs.contains_key(p);
    render_pieces(
        ap::pieces(text, field, mk.spans, known, mk.suppress),
        &format!("{}{field}", mk.ns),
        mk.mark,
        refs,
        links,
    )
}

/// The reference links of `pmids` that have references.
pub(super) fn ref_chips(pmids: &[String], refs: &HashMap<String, u16>) -> AnyView {
    let ids: Vec<String> = pmids.iter().map(|p| p.trim().to_string()).collect();
    if ids.iter().any(|p| refs.contains_key(p)) {
        view! { " " {chips(&ids, refs, Links::Yes)} }.into_any()
    } else {
        ().into_any()
    }
}

/// The certainty chip's class.
pub(super) fn cert_class(c: Certainty) -> &'static str {
    match c {
        Certainty::High => "ap-cert c-high",
        Certainty::Moderate => "ap-cert c-moderate",
        Certainty::Low => "ap-cert c-low",
        Certainty::VeryLow => "ap-cert c-verylow",
    }
}

pub(super) fn cert_chip(c: Certainty) -> AnyView {
    view! { <span class=cert_class(c)>{c.as_str()}</span> }.into_any()
}

/// The page's data, built once.
pub(super) struct Ctx<'a> {
    pub a: &'a AnswerV3,
    pub w: Option<&'a WrittenAnswer>,
    pub refs: Arc<HashMap<String, u16>>,
    pub groups: Vec<NumGroup>,
    no_spans: Vec<Span>,
}

impl<'a> Ctx<'a> {
    pub fn new(a: &'a AnswerV3) -> Ctx<'a> {
        let w = a.answer.as_ref();
        Ctx {
            a,
            w,
            refs: Arc::new(a.ref_numbers()),
            groups: w.map(ap::all_groups).unwrap_or_default(),
            no_spans: Vec::new(),
        }
    }

    /// The writer's "not traced" marks.
    pub fn wm(&self) -> Mk<'_> {
        Mk {
            spans: &self.a.untraced,
            mark: Mark::NotTraced,
            ns: "w:",
            suppress: true,
        }
    }

    /// The completeness review's "not traced" marks.
    pub fn cm(&self) -> Mk<'_> {
        Mk {
            spans: self
                .a
                .completeness
                .as_ref()
                .map_or(&self.no_spans, |c| &c.untraced),
            mark: Mark::NotTraced,
            ns: "c:",
            suppress: true,
        }
    }

    /// The skeptic's "not verified" marks.
    pub fn sm(&self) -> Mk<'_> {
        Mk {
            spans: self.a.skeptic.as_ref().map_or(&self.no_spans, |k| &k.spans),
            mark: Mark::NotVerified,
            ns: "s:",
            suppress: false,
        }
    }

    /// `text` with chips and marks.
    pub fn t(&self, text: &str, field: &str, mk: Mk) -> AnyView {
        rich(text, field, mk, &self.refs, Links::Yes)
    }

    /// `text` with marks, citations as plain numbers (inside a link).
    pub fn t_plain(&self, text: &str, field: &str, mk: Mk) -> AnyView {
        rich(text, field, mk, &self.refs, Links::No)
    }

    pub fn ref_of(&self, pmid: &str) -> Option<u16> {
        self.refs.get(pmid.trim()).copied()
    }

    /// The short name of study `pmid` with its reference number: "ROGER
    /// (2025) [3]".
    pub fn study_name(&self, pmid: &str) -> String {
        let short = self
            .w
            .and_then(|w| ap::study_of(w, pmid))
            .map(|s| ap::study_short(&s.label))
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "Ref".to_string());
        match self.ref_of(pmid) {
            Some(n) => format!("{short} [{n}]"),
            None => format!("PMID {}", pmid.trim()),
        }
    }

    /// A button-styled link opening the sheet of `pmid` (a PubMed link
    /// when it has no reference).
    pub fn study_link(&self, pmid: &str, class: &'static str) -> AnyView {
        let name = self.study_name(pmid);
        match self.ref_of(pmid) {
            Some(n) => view! {
                <a class=class href=format!("#ref-{n}") rel="external" on:click=sheet_click(n)>{name}</a>
            }
            .into_any(),
            None => match pubmed_link(pmid.trim()) {
                Some(href) => view! {
                    <a class=class href=href rel="noopener noreferrer" target="_blank">{name}</a>
                }
                .into_any(),
                None => ().into_any(),
            },
        }
    }

    /// The "not traced" mark of the writer's structured number `j` of
    /// outcome `i`, when it is marked.
    pub fn num_mark(&self, i: usize, j: usize) -> Option<AnyView> {
        let f = format!("outcomes.{i}.numbers.{j}");
        whole_marked(&f, &self.a.untraced).then(|| mark_el(Mark::NotTraced, format!("w:{f}")))
    }

    /// The completeness review's additions' "added" and "no source"
    /// labels.
    pub fn added_label(&self) -> AnyView {
        view! { " " <span class="flag-label added">{ADDED_LABEL}</span> }.into_any()
    }
}

pub(super) fn unsourced_label(flag: bool) -> Option<AnyView> {
    flag.then(|| view! { " " <span class="flag-label">{NO_GUIDELINE_SOURCE}</span> }.into_any())
}

/// A block's heading line: a small capitals kicker.
fn kicker(text: &'static str) -> AnyView {
    view! { <div class="ap-kicker">{text}</div> }.into_any()
}

// ---------- the page ----------

/// The interactive answer page (Part A), in [`ap::PAGE_ORDER`]; with
/// `layout` (S1e, `ANSWER_LAYOUT=on`) in [`ap::LAYOUT_ORDER`] instead
/// ([`layout_page_view`]).
pub(super) fn answer_page_view(a: AnswerV3, layout: bool) -> AnyView {
    if layout {
        return layout_page_view(a);
    }
    let c = Ctx::new(&a);
    let vis = ap::visible(&a);
    let blocks: Vec<AnyView> = ap::PAGE_ORDER
        .iter()
        .filter_map(|b| match b {
            Block::Header => Some(header_view(&c)),
            Block::Glance => glance_view(&c),
            Block::BottomLine => Some(bottom_line_view(&c)),
            Block::Practice => practice_view(&c),
            Block::OutOf100 => vis.out_of_100.then(|| out_of_100_view(&c)),
            Block::Forest => {
                if vis.forest {
                    Some(forest_view(&c))
                } else {
                    vis.no_numbers.then(no_numbers_view)
                }
            }
            Block::Outcomes => vis.outcomes.then(|| outcomes_view(&c, false)),
            Block::Compare => vis.compare.then(|| compare_view(&c)),
            Block::Details => Some(
                view! { {super::details::details_view(&c)} {super::details::sheets_view(&c)} }
                    .into_any(),
            ),
            Block::Original => None,
        })
        .collect();
    // The new page's styles are scoped to `.ap-main`, so the original run
    // view (drawn after it, outside it) looks exactly as it did.
    let original = ap::PAGE_ORDER
        .contains(&Block::Original)
        .then(|| super::original_view(a.clone()));
    view! {
        <div class="ap">
            <div class="ap-main">{blocks}</div>
            {original}
        </div>
    }
    .into_any()
}

// ---------- S1e: the new layout ----------

/// S1e's own styles, drawn only with `ANSWER_LAYOUT=on` (the page is
/// otherwise byte for byte today's). The run page's column widens to
/// 1180 px while the answer page (or its early sections) shows, with the
/// tracker's cards and rows to the same edges; the Expert Opinion and chat
/// views keep today's width. Pairs are a two-column grid: the right card
/// folds to the left card's height with CSS only (its content laid out out
/// of flow, so it adds nothing to the row), with a checkbox and label as the
/// button, so it works before hydration and nothing moves after load.
/// Below 1,100 px (and in print) the pairs stack and nothing folds. A link
/// or tab into a folded card unfolds it: the page ticks the checkbox (after
/// hydration, [`open_fold`]), and before that `:has(:target, :focus-visible)`
/// unclips the box.
pub const LAYOUT_STYLE: &str = r##"
main:has(.ap-lx):not(:has(.rx-expon, .rx-chaton, .rx-expert:target:not(:empty), .rx-chat:target:not(:empty))) { max-width:1180px; }
main:has(.ap-lx):not(:has(.rx-expon, .rx-chaton, .rx-expert:target:not(:empty), .rx-chat:target:not(:empty))) .run-chrome :is(.rk-card, .rx-seg, details[class$="-fold"]) { max-width:none; }
.ap-lx .ap-main { max-width:none; }
.ap-lx .ap-lx-head { display:flex; flex-direction:column; gap:2px; padding-top:14px; }
.ap-lx .ap-main .ap-lx-h, .ap-lx .ap-main .ap-details-title { margin:0; padding:0; font-size:22px; font-weight:700; line-height:1.25; color:var(--ink); }
.ap-lx .ap-main .ap-details-title { padding-top:14px; }
.ap-lx .ap-lx-sub { font-size:14px; line-height:1.45; color:var(--mute); }
.ap-lx-pair { display:grid; grid-template-columns:minmax(0,1fr) minmax(0,1fr); gap:16px; }
.ap-lx-col { display:flex; flex-direction:column; min-width:0; }
.ap-lx-col > * { flex:1 1 auto; }
.ap-lx-pair.fold > .ap-lx-col { align-self:start; }
.ap-lx-pair.fold > .ap-lx-r { position:relative; display:grid; grid-template-columns:minmax(0,1fr); grid-template-rows:minmax(0,1fr) auto; row-gap:10px; min-width:0; }
.ap-lx-cb { position:absolute; top:0; left:0; width:1px; height:1px; margin:0; opacity:0; pointer-events:none; }
.ap-lx-box { position:relative; min-height:0; overflow:hidden; border-radius:16px; }
.ap-lx-box > * { position:absolute; top:0; left:0; right:0; min-height:100%; }
.ap-lx-box::after { content:""; position:absolute; left:0; right:0; bottom:0; height:72px; background:linear-gradient(rgba(255,255,255,0), #fff 80%); border:1px solid var(--rule); border-top:0; border-radius:0 0 16px 16px; pointer-events:none; }
.ap-lx-cb:checked ~ .ap-lx-box, .ap-lx-box:has(:target, :focus-visible) { overflow:visible; }
.ap-lx-cb:checked ~ .ap-lx-box > *, .ap-lx-box:has(:target, :focus-visible) > * { position:static; }
.ap-lx-cb:checked ~ .ap-lx-box::after, .ap-lx-box:has(:target, :focus-visible)::after { display:none; }
.ap-lx-r:has(.ap-lx-box :target, .ap-lx-box :focus-visible) > .ap-lx-cb:not(:checked) ~ .ap-lx-more { display:none; }
.ap-lx-more { display:flex; align-items:center; justify-content:center; width:100%; min-height:44px; margin:0; padding:0 14px; border:1px solid var(--rule); border-radius:12px; background:#fff; color:#1F4E79; font-size:15px; font-weight:600; cursor:pointer; user-select:none; }
.ap-lx-more:hover { background:#F2F5F9; }
.ap-lx-more > span { align-items:center; gap:8px; }
.ap-lx-all { display:inline-flex; }
.ap-lx-less { display:none; }
.ap-lx-sign { display:inline-flex; align-items:center; justify-content:center; width:22px; height:22px; border-radius:50%; background:#1F4E79; color:#fff; font-size:16px; line-height:1; }
.ap-lx-cb:checked ~ .ap-lx-more .ap-lx-all { display:none; }
.ap-lx-cb:checked ~ .ap-lx-more .ap-lx-less { display:inline-flex; }
.ap-lx-cb:focus-visible ~ .ap-lx-more { outline:2px solid #2C4A85; outline-offset:2px; }
@media (max-width:1099.98px) {
  .ap-lx-pair { grid-template-columns:minmax(0,1fr); }
  .ap-lx-pair.fold > .ap-lx-r { display:block; }
  .ap-lx-box { overflow:visible; }
  .ap-lx-box > * { position:static; }
  .ap-lx-box::after, .ap-lx-more, .ap-lx-cb { display:none; }
}
@media print {
  .ap-lx-box { overflow:visible; }
  .ap-lx-box > * { position:static; }
  .ap-lx-box::after, .ap-lx-more, .ap-lx-cb { display:none; }
}
"##;

/// A heading over a pair (22 px, the left edge of the cards).
fn lx_head(title: &'static str, sub: &'static str) -> AnyView {
    view! {
        <div class="ap-lx-head">
            <h2 class="ap-lx-h">{title}</h2>
            <p class="ap-lx-sub">{sub}</p>
        </div>
    }
    .into_any()
}

/// Two cards side by side. With `fold` (its checkbox id and button text)
/// the right card is cut to the left card's height, with the button under
/// it; without, both stretch to the taller one. One card alone takes the
/// full width.
fn lx_pair(
    left: Option<AnyView>,
    right: Option<AnyView>,
    fold: Option<(&'static str, String)>,
) -> Option<AnyView> {
    match (left, right) {
        (Some(l), Some(r)) => Some(match fold {
            Some((id, more)) => view! {
                <div class="ap-lx-pair fold">
                    <div class="ap-lx-col ap-lx-l">{l}</div>
                    <div class="ap-lx-r">
                        <input type="checkbox" class="ap-lx-cb" id=id aria-label=more.clone() aria-controls=format!("{id}-box") />
                        <div class="ap-lx-box" id=format!("{id}-box") on:focusin=move |_| open_fold(id)>{r}</div>
                        <label class="ap-lx-more" for=id>
                            <span class="ap-lx-all" aria-hidden="true"><span class="ap-lx-sign">"+"</span>{more}</span>
                            <span class="ap-lx-less" aria-hidden="true"><span class="ap-lx-sign">"\u{2212}"</span>"Show less"</span>
                        </label>
                    </div>
                </div>
            }
            .into_any(),
            None => view! {
                <div class="ap-lx-pair">
                    <div class="ap-lx-col ap-lx-l">{l}</div>
                    <div class="ap-lx-col ap-lx-r">{r}</div>
                </div>
            }
            .into_any(),
        }),
        (Some(one), None) | (None, Some(one)) => Some(one),
        (None, None) => None,
    }
}

/// The checkbox ids of the two folds (one answer page per document).
pub(super) const FOLD_WHY: &str = "ap-lx-why";
pub(super) const FOLD_DASH: &str = "ap-lx-dash";

/// The fold holding the element `id` (the page's in-page links go only to
/// outcomes: `outcome-<i>` and `outcome-added-<k>`).
pub(super) fn fold_of(id: &str) -> Option<&'static str> {
    id.starts_with("outcome-").then_some(FOLD_WHY)
}

/// Unfold the fold `id` (browser only; nothing when it is not drawn, as
/// with the switch off or when the rule drew no fold).
pub(super) fn open_fold(id: &str) {
    #[cfg(feature = "hydrate")]
    {
        if let Some(el) = web_sys::window()
            .and_then(|w| w.document())
            .and_then(|d| d.get_element_by_id(id))
        {
            let _ = web_sys::js_sys::Reflect::set(&el, &"checked".into(), &true.into());
        }
    }
    #[cfg(not(feature = "hydrate"))]
    {
        let _ = id;
    }
}

/// The button's text for the outcomes card: the real count.
pub(super) fn show_all_outcomes(n: usize) -> String {
    format!("Show all {n} outcomes")
}

/// The button's text for the comparison card.
pub(super) const SHOW_MORE: &str = "Show more";

/// S1e: the answer page in [`ap::LAYOUT_ORDER`]: the summary cards full
/// width, then "Why this conclusion" (CI | outcomes) and "Counselling
/// dashboard" (Out of 100 | Compare the arms) as pairs, then Details as
/// today. The same components; the outcomes in a card.
fn layout_page_view(a: AnswerV3) -> AnyView {
    let c = Ctx::new(&a);
    let vis = ap::visible(&a);
    let w = c.w;
    let n_outcomes = w.map_or(0, |w| w.outcomes.len())
        + a.completeness
            .as_ref()
            .map_or(0, |cp| cp.added_outcomes.len());
    let forest_rows = w.map_or(0, |w| ap::forest_rows(w, &c.groups).len());
    let measures = w.map_or(0, |w| {
        let pairs: Vec<NumGroup> = c
            .groups
            .iter()
            .filter(|g| g.kind == ChartKind::Pair)
            .cloned()
            .collect();
        ap::comparison_groups(w, &pairs)
            .first()
            .map_or(0, |g| g.items.len())
    });
    let mut blocks: Vec<AnyView> = Vec::new();
    for b in ap::LAYOUT_ORDER {
        let pair = ap::PAIRS.iter().find(|p| p.left == b);
        if let Some(p) = pair {
            let (left, right, fold) = if p.left == Block::Forest {
                let left = if vis.forest {
                    Some(forest_view(&c))
                } else {
                    vis.no_numbers.then(no_numbers_view)
                };
                let right = vis.outcomes.then(|| outcomes_view(&c, true));
                let fold = (vis.forest
                    && ap::folds(ap::forest_px(forest_rows), ap::outcomes_px(n_outcomes)))
                .then(|| (FOLD_WHY, show_all_outcomes(n_outcomes)));
                (left, right, fold)
            } else {
                let left = vis.out_of_100.then(|| out_of_100_view(&c));
                let right = vis.compare.then(|| compare_view(&c));
                let fold = ap::folds(ap::OUT_OF_100_PX, ap::compare_px(measures))
                    .then(|| (FOLD_DASH, SHOW_MORE.to_string()));
                (left, right, fold)
            };
            // The "no numbers" note is no card: it and the outcomes stack.
            let stack = left.is_some() && right.is_some() && !vis.forest && p.left == Block::Forest;
            if left.is_some() || right.is_some() {
                blocks.push(lx_head(p.heading, p.sub));
            }
            if stack {
                blocks.extend(left);
                blocks.extend(right);
            } else {
                blocks.extend(lx_pair(left, right, fold));
            }
            continue;
        }
        if ap::PAIRS.iter().any(|p| p.right == b) {
            continue;
        }
        match b {
            Block::Header => blocks.push(header_view(&c)),
            Block::Glance => blocks.extend(glance_view(&c)),
            Block::BottomLine => blocks.push(bottom_line_view(&c)),
            Block::Practice => blocks.extend(practice_view(&c)),
            Block::Details => blocks.push(
                view! { {super::details::details_view(&c)} {super::details::sheets_view(&c)} }
                    .into_any(),
            ),
            _ => {}
        }
    }
    let original = ap::LAYOUT_ORDER
        .contains(&Block::Original)
        .then(|| super::original_view(a.clone()));
    view! {
        <style>{LAYOUT_STYLE}</style>
        <div class="ap ap-lx">
            <div class="ap-main">{blocks}</div>
            {original}
        </div>
    }
    .into_any()
}

// ---------- S1c: the finished sections, early ----------

/// The early page's own styles: the placeholders only (the finished sections
/// use the final page's). Drawn only with an early answer.
const EARLY_STYLE: &str = r##"
.ap-main .ap-wait { background:#fff; border:1px dashed #D9D3C8; border-radius:16px; padding:12px 16px; display:flex; flex-direction:column; gap:10px; }
.ap-main .ap-wait-head { display:flex; justify-content:space-between; align-items:center; gap:12px; min-height:24px; }
.ap-main .ap-wait-title { font-size:.95rem; font-weight:600; color:#4A4741; }
.ap-main .ap-wait-state { flex:none; font-size:.78rem; font-weight:600; color:#2C4A85; animation:ap-wait 1.6s ease-in-out infinite; }
.ap-main .ap-wait-state.later { font-weight:500; color:#6B675F; animation:none; }
.ap-main .ap-wait-note { font-size:.85rem; line-height:1.45; color:#6B675F; }
.ap-main .ap-wait-lines { display:flex; flex-direction:column; gap:6px; }
.ap-main .ap-wait-lines span { display:block; height:10px; border-radius:6px; background:#EFECE6; }
@keyframes ap-wait { 0%,100% { opacity:.45; } 50% { opacity:1; } }
@media (prefers-reduced-motion:reduce) { .ap-main .ap-wait-state { animation:none; } }
"##;

/// What a placeholder says while its part is written.
pub(super) const BEING_WRITTEN: &str = "Being written";
/// What the charts' placeholder says: they are drawn from the outcomes.
pub(super) const AFTER_OUTCOMES: &str = "After outcomes";

/// A quiet card for a part still being written (owner Q1).
fn wait_card(
    title: &'static str,
    state: &'static str,
    note: Option<&'static str>,
    lines: bool,
) -> AnyView {
    let class = if state == BEING_WRITTEN {
        "ap-wait-state"
    } else {
        "ap-wait-state later"
    };
    view! {
        <section class="ap-wait" aria-label=title>
            <div class="ap-wait-head">
                <span class="ap-wait-title">{title}</span>
                <span class=class>{state}</span>
            </div>
            {note.map(|n| view! { <p class="ap-wait-note">{n}</p> })}
            {lines.then(|| view! {
                <div class="ap-wait-lines" aria-hidden="true">
                    <span style="width:92%"></span>
                    <span style="width:78%"></span>
                    <span style="width:85%"></span>
                </div>
            })}
        </section>
    }
    .into_any()
}

/// S1c: the run page while an answer written in four parts is still being
/// written and its summary or context part is done and checked: those
/// sections drawn by the final page's own components from `e.answer`
/// (references numbered as on the final page, the "not traced" marks of the
/// number check), quiet placeholders for the rest. The final page replaces
/// it when the answer is stored.
pub(super) fn early_page_view(e: crate::draft_page::EarlyAnswer, layout: bool) -> AnyView {
    if layout {
        return early_layout_view(e);
    }
    let c = Ctx::new(&e.answer);
    let mut blocks: Vec<AnyView> = Vec::new();
    if e.summary {
        blocks.extend(glance_view(&c));
        blocks.push(bottom_line_view(&c));
        blocks.extend(practice_view(&c));
    } else {
        blocks.push(wait_card(
            "At a glance \u{b7} Bottom line \u{b7} Clinical practice",
            BEING_WRITTEN,
            None,
            false,
        ));
    }
    blocks.push(wait_card(
        "Outcome by outcome",
        BEING_WRITTEN,
        Some("Each outcome, with its numbers and certainty, appears when it has been checked against the trials."),
        true,
    ));
    blocks.push(wait_card(
        "Out of 100 people \u{b7} Every estimate with 95% CI",
        AFTER_OUTCOMES,
        None,
        false,
    ));
    blocks.push(wait_card(
        "The trials \u{b7} Conflicting evidence",
        BEING_WRITTEN,
        None,
        false,
    ));
    if e.context {
        let parts: Vec<AnyView> = [
            super::details::applicability(&c),
            super::details::would_change(&c),
        ]
        .into_iter()
        .flatten()
        .collect();
        if !parts.is_empty() {
            blocks.push(
                view! {
                    <div class="ap-details">
                        <h3 class="ap-details-title">"Details"</h3>
                        {parts}
                    </div>
                }
                .into_any(),
            );
        }
    } else {
        blocks.push(wait_card(
            "Applicability \u{b7} What would change this answer",
            BEING_WRITTEN,
            None,
            false,
        ));
    }
    blocks.push(super::details::sheets_view(&c));
    view! {
        <style>{EARLY_STYLE}</style>
        <div class="ap">
            <div class="ap-main ap-early">{blocks}</div>
        </div>
    }
    .into_any()
}

/// S1e: the early page in the new layout's order: the summary's sections
/// (or their placeholder), "Why this conclusion" with the CI and outcomes
/// placeholders side by side, "Counselling dashboard" with the Out of 100
/// and Compare the arms placeholders, then the Details rows.
fn early_layout_view(e: crate::draft_page::EarlyAnswer) -> AnyView {
    let c = Ctx::new(&e.answer);
    let mut blocks: Vec<AnyView> = Vec::new();
    if e.summary {
        blocks.extend(glance_view(&c));
        blocks.push(bottom_line_view(&c));
        blocks.extend(practice_view(&c));
    } else {
        blocks.push(wait_card(
            "At a glance \u{b7} Bottom line \u{b7} Clinical practice",
            BEING_WRITTEN,
            None,
            false,
        ));
    }
    let [why, dash] = ap::PAIRS;
    blocks.push(lx_head(why.heading, why.sub));
    blocks.extend(lx_pair(
        Some(wait_card(
            "Every estimate with a 95% CI",
            BEING_WRITTEN,
            None,
            true,
        )),
        Some(wait_card(
            "Outcome by outcome",
            BEING_WRITTEN,
            Some("Each outcome, with its numbers and certainty, appears when it has been checked against the trials."),
            true,
        )),
        None,
    ));
    blocks.push(lx_head(dash.heading, dash.sub));
    blocks.extend(lx_pair(
        Some(wait_card(
            "Out of 100 patients",
            AFTER_OUTCOMES,
            None,
            false,
        )),
        Some(wait_card("Compare the arms", AFTER_OUTCOMES, None, false)),
        None,
    ));
    let context: Vec<AnyView> = if e.context {
        [
            super::details::applicability(&c),
            super::details::would_change(&c),
        ]
        .into_iter()
        .flatten()
        .collect()
    } else {
        vec![wait_card(
            "Applicability \u{b7} What would change this answer",
            BEING_WRITTEN,
            None,
            false,
        )]
    };
    blocks.push(
        view! {
            <div class="ap-details">
                <h3 class="ap-details-title">"Details"</h3>
                {wait_card("The trials \u{b7} Conflicting evidence", BEING_WRITTEN, None, false)}
                {context}
            </div>
        }
        .into_any(),
    );
    blocks.push(super::details::sheets_view(&c));
    view! {
        <style>{EARLY_STYLE}</style>
        <style>{LAYOUT_STYLE}</style>
        <div class="ap ap-lx">
            <div class="ap-main ap-early">{blocks}</div>
        </div>
    }
    .into_any()
}

// ---------- 1. title and subtitle ----------

fn header_view(c: &Ctx) -> AnyView {
    let a = c.a;
    view! {
        <div class="ap-head">
            <p class="ap-meta">
                <span class="ap-mode">{mode_label(a.mode)}</span>
                " " <span>{date_ist(a.created_at)}</span>
                " \u{b7} " <span>{duration_text(a.elapsed_s)}</span>
            </p>
            <p class="ap-decision">{DECISION_SUPPORT}</p>
            <p class="ap-rerun"><a href=rerun_href(&a.question, a.mode) rel="external">"Edit & re-run"</a></p>
        </div>
    }
    .into_any()
}

// ---------- 2. at a glance ----------

fn glance_view(c: &Ctx) -> Option<AnyView> {
    let g = c.w?.at_a_glance.as_ref()?;
    let tiles: Vec<AnyView> = g
        .key_numbers
        .iter()
        .enumerate()
        .map(|(i, k)| {
            let label = (!k.label.is_empty()).then(|| {
                view! { <span class="ap-tile-label">{c.t(&k.label, &format!("at_a_glance.key_numbers.{i}.label"), c.wm())}</span> }
            });
            let value = c.t(
                &k.value_text,
                &format!("at_a_glance.key_numbers.{i}.value_text"),
                c.wm(),
            );
            let src = c.study_link(&k.pmid, "ap-tile-src");
            // The source link's area covers the tile (CSS), so the whole
            // tile opens the sheet; the tile itself is not clickable.
            let class = if c.ref_of(&k.pmid).is_some() {
                "ap-tile tappable"
            } else {
                "ap-tile"
            };
            view! {
                <div class=class>{label} <span class="ap-tile-value">{value}</span> {src}</div>
            }
            .into_any()
        })
        .collect();
    Some(
        view! {
            <section class="ap-card ap-glance" aria-label="At a glance">
                {kicker("At a glance")}
                <p class="ap-verdict">{c.t(&g.verdict, "at_a_glance.verdict", c.wm())}</p>
                {(!tiles.is_empty()).then(|| view! { <div class="ap-tiles">{tiles}</div> })}
                {(!g.main_caveat.is_empty()).then(|| view! {
                    <p class="ap-caveat"><strong>"Main caveat: "</strong> {c.t(&g.main_caveat, "at_a_glance.main_caveat", c.wm())}</p>
                })}
            </section>
        }
        .into_any(),
    )
}

// ---------- 3. bottom line ----------

fn bottom_line_view(c: &Ctx) -> AnyView {
    let Some(w) = c.w else {
        let reason =
            c.a.empty_reason
                .clone()
                .unwrap_or_else(|| crate::agent::answer::NO_EVIDENCE.to_string());
        return view! {
            <section class="ap-card" aria-label="Bottom line">
                {kicker("Bottom line")}
                <p class="ap-bl">{reason}</p>
            </section>
        }
        .into_any();
    };
    let b = &w.bottom_line;
    view! {
        <section class="ap-card" aria-label="Bottom line">
            <div class="ap-row">
                {kicker("Bottom line")}
                <span class=cert_class(b.certainty)>{format!("Certainty: {}", b.certainty.as_str())}</span>
            </div>
            <p class="ap-bl">{c.t(&b.text, "bottom_line.text", c.wm())}</p>
            <p class="ap-reason">{c.t(&b.certainty_reason, "bottom_line.certainty_reason", c.wm())}</p>
        </section>
    }
    .into_any()
}

// ---------- 4. clinical practice ----------

fn practice_view(c: &Ctx) -> Option<AnyView> {
    let p = c.w?.clinical_practice.as_ref()?;
    let parts: Vec<AnyView> = [
        ("Default choice", &p.default_choice, "default_choice"),
        ("Who may benefit", &p.who_may_benefit, "who_may_benefit"),
        ("Counselling", &p.counselling, "counselling"),
        ("Cost per case", &p.cost_per_case, "cost_per_case"),
    ]
    .into_iter()
    .filter(|(_, t, _)| !t.is_empty())
    .map(|(label, t, f)| {
        view! {
            <div class="ap-part">
                <h4>{label}</h4>
                <p>{c.t(t, &format!("clinical_practice.{f}"), c.wm())}</p>
            </div>
        }
        .into_any()
    })
    .collect();
    Some(
        view! {
            <section class="ap-card" aria-label="What this means for clinical practice">
                {kicker("What this means for clinical practice")}
                {parts}
                {(!p.text.is_empty()).then(|| view! { <p>{c.t(&p.text, "clinical_practice.text", c.wm())}</p> })}
            </section>
        }
        .into_any(),
    )
}

// ---------- 5. out of 100 patients ----------

/// One "Out of 100" group, as plain data for the reactive panel.
#[derive(Clone)]
struct FreqItem {
    label: String,
    /// (filled cells, arm name, mark key when the number is not traced).
    arms: Vec<(u8, String, Option<String>)>,
    exact: String,
    p: Option<String>,
    certainty: Certainty,
    why: Vec<Piece>,
    why_key: String,
    study: String,
}

fn out_of_100_view(c: &Ctx) -> AnyView {
    let Some(w) = c.w else {
        return ().into_any();
    };
    let items: Vec<FreqItem> = ap::out_of_100(w, &c.groups)
        .into_iter()
        .map(|k| {
            let g = &c.groups[k];
            let o = &w.outcomes[g.outcome];
            let nums = g.nums(w);
            let arms = g
                .numbers
                .iter()
                .zip(&nums)
                .enumerate()
                .map(|(idx, (&j, n))| {
                    let arm = n
                        .arm
                        .as_deref()
                        .map(str::trim)
                        .filter(|a| !a.is_empty())
                        .map_or_else(|| format!("arm {}", idx + 1), String::from);
                    let f = format!("outcomes.{}.numbers.{j}", g.outcome);
                    let mark = whole_marked(&f, &c.a.untraced).then(|| format!("w:{f}"));
                    (ap::out_of_100_count(n.value), arm, mark)
                })
                .collect::<Vec<_>>();
            let exact = nums
                .iter()
                .zip(&arms)
                .map(|(n, (_, arm, _))| format!("{}% {arm}", ap::fmt_value(n.value)))
                .collect::<Vec<_>>()
                .join(" vs ");
            let why_key = format!("outcomes.{}.why", g.outcome);
            FreqItem {
                label: g.label.clone(),
                arms,
                exact,
                p: g.p_text(w),
                certainty: o.certainty,
                why: ap::pieces(
                    &o.why,
                    &why_key,
                    &c.a.untraced,
                    |p| c.refs.contains_key(p),
                    true,
                ),
                why_key: format!("w:{why_key}"),
                study: c.study_name(&g.pmid),
            }
        })
        .collect();
    let who = ap::first_sentence(&w.applicability.case_mix);
    let sel = RwSignal::new(0usize);
    let patient = RwSignal::new(false);
    let pills: Vec<AnyView> = items
        .iter()
        .enumerate()
        .map(|(k, it)| {
            view! {
                <button
                    type="button"
                    class=move || if sel.get() == k { "ap-pill on" } else { "ap-pill" }
                    aria-pressed=move || (sel.get() == k).to_string()
                    on:click=move |_| sel.set(k)
                >
                    {it.label.clone()}
                </button>
            }
            .into_any()
        })
        .collect();
    let mode = move |p: bool, label: &'static str| {
        view! {
            <button
                type="button"
                class=move || if patient.get() == p { "ap-pill small on" } else { "ap-pill small" }
                aria-pressed=move || (patient.get() == p).to_string()
                on:click=move |_| patient.set(p)
            >
                {label}
            </button>
        }
    };
    let items = Arc::new(items);
    let refs = c.refs.clone();
    let panel = move || {
        let Some(it) = items.get(sel.get()).or_else(|| items.first()).cloned() else {
            return ().into_any();
        };
        let pat = patient.get();
        let sentence = if pat {
            let arms: Vec<(u8, String)> = it.arms.iter().map(|(n, a, _)| (*n, a.clone())).collect();
            ap::patient_sentence(&it.label, &arms)
        } else {
            format!(
                "{}: {}{}. Certainty for this outcome: {}.",
                it.label,
                it.exact,
                it.p.as_ref().map(|p| format!(" ({p})")).unwrap_or_default(),
                it.certainty.as_str()
            )
        };
        let grids: Vec<AnyView> = it
            .arms
            .iter()
            .enumerate()
            .map(|(idx, (n, arm, mark))| {
                let n = *n;
                let cells: Vec<AnyView> = (0..100u8)
                    .map(|k| {
                        let class = if k < n {
                            format!("ap-cell arm{}", idx % 4)
                        } else {
                            "ap-cell".to_string()
                        };
                        view! { <span class=class aria-hidden="true"></span> }.into_any()
                    })
                    .collect();
                view! {
                    <div class="ap-grid-arm">
                        <div class="ap-grid-head">
                            <span class=format!("ap-big arm{}", idx % 4)>{n.to_string()}</span>
                            <span>{format!("of 100 \u{b7} {arm}")}</span>
                            {mark.clone().map(|k| mark_el(Mark::NotTraced, k))}
                        </div>
                        <div class="ap-grid" role="img" aria-label=format!("{n} of 100 people, {arm}")>{cells}</div>
                    </div>
                }
                .into_any()
            })
            .collect();
        let caveat = if pat {
            view! {
                <div class="ap-box">
                    <h4>"How sure are we?"</h4>
                    <p>{ap::plain_certainty(it.certainty)}</p>
                </div>
            }
            .into_any()
        } else {
            view! {
                <div class="ap-box">
                    <h4>{format!("Why {}", it.certainty.as_str())}</h4>
                    <p>{render_pieces(it.why.clone(), &it.why_key, Mark::NotTraced, &refs, Links::Yes)}</p>
                </div>
            }
            .into_any()
        };
        view! {
            <p class="ap-sentence">{sentence}</p>
            <div class="ap-grids">{grids}</div>
            <p class="ap-legend">
                <span class="ap-swatch" aria-hidden="true"></span>
                {format!("Filled = {}", ap::measure(&it.label))}
                " \u{b7} "
                <span class="ap-muted">{format!("Exact: {} \u{b7} {}", it.exact, it.study)}</span>
            </p>
            {caveat}
        }
        .into_any()
    };
    view! {
        <section class="ap-card" aria-label="Out of 100 patients">
            <div class="ap-row">
                <div>
                    {kicker("Out of 100 patients")}
                    <h3>"Out of 100 people like those in the trials"</h3>
                </div>
                <div class="ap-seg" role="group" aria-label="Reader">
                    {mode(false, "Clinician")} {mode(true, "Patient")}
                </div>
            </div>
            <div class="ap-pills">{pills}</div>
            {panel}
            {(!who.is_empty()).then(|| view! {
                <div class="ap-box">
                    <h4>"Who these numbers apply to"</h4>
                    <p>{who}</p>
                </div>
            })}
        </section>
    }
    .into_any()
}

// ---------- 6. every estimate with a 95% CI ----------

fn forest_view(c: &Ctx) -> AnyView {
    let Some(w) = c.w else {
        return ().into_any();
    };
    let rows = ap::forest_rows(w, &c.groups);
    let filter: RwSignal<Option<SrcType>> = RwSignal::new(None);
    let mut present: Vec<SrcType> = Vec::new();
    let mut ratio_rows: Vec<AnyView> = Vec::new();
    let mut diff_rows: Vec<AnyView> = Vec::new();
    let mut types: Vec<SrcType> = Vec::new();
    for (k, j) in rows {
        let g = &c.groups[k];
        let o = &w.outcomes[g.outcome];
        let n = &o.numbers[j];
        let st = ap::src_type_of(w, &n.pmid);
        if !present.contains(&st) {
            present.push(st);
        }
        types.push(st);
        let ratio = ap::is_ratio(n);
        let label = format!("{} \u{b7} {}", o.outcome, n.label);
        let sub = [
            n.arm.clone().unwrap_or_default(),
            c.study_name(&n.pmid),
            st.label().to_string(),
        ]
        .into_iter()
        .filter(|s| !s.trim().is_empty())
        .collect::<Vec<_>>()
        .join(" \u{b7} ");
        let (lo, hi) = (n.ci_low.unwrap_or(n.value), n.ci_high.unwrap_or(n.value));
        let track = if ratio {
            let (a, b, v) = (ap::log_pos(lo), ap::log_pos(hi), ap::log_pos(n.value));
            view! {
                <span class="ap-track-axis">
                    <span class="ap-line"></span>
                    <span class="ap-ref-line" style=format!("left:{:.2}%", ap::log_pos(1.0))></span>
                    <span class=format!("ap-ci {}", st.class()) style=format!("left:{a:.2}%;width:{:.2}%", (b - a).max(0.5))></span>
                    <span class=format!("ap-pt {}", st.class()) style=format!("left:{v:.2}%")></span>
                </span>
            }
            .into_any()
        } else {
            let m = ap::diff_scale(n);
            let (a, b, v) = (
                ap::diff_pos(lo, m),
                ap::diff_pos(hi, m),
                ap::diff_pos(n.value, m),
            );
            view! {
                <span class="ap-track-axis">
                    <span class="ap-line"></span>
                    <span class="ap-ref-line" style="left:50%"></span>
                    <span class="ap-ci diff" style=format!("left:{a:.2}%;width:{:.2}%", (b - a).max(0.5))></span>
                    <span class="ap-pt diff round" style=format!("left:{v:.2}%")></span>
                </span>
            }
            .into_any()
        };
        let est = if ratio {
            ap::ratio_estimate(n)
        } else {
            ap::diff_estimate(n)
        };
        let hide = move || filter.get().is_some_and(|f| f != st);
        let body = view! {
            <span class="ap-frow-top">
                <span class="ap-frow-label">{label}</span>
                <span class="ap-mono">{est} {c.num_mark(g.outcome, j)}</span>
            </span>
            <span class="ap-sub">{sub}</span>
            {track}
        };
        let row = match c.ref_of(&n.pmid) {
            Some(r) => view! {
                <a class="ap-frow" class:ap-hidden=hide href=format!("#ref-{r}") rel="external" on:click=sheet_click(r)>{body}</a>
            }
            .into_any(),
            None => view! { <div class="ap-frow" class:ap-hidden=hide>{body}</div> }.into_any(),
        };
        if ratio {
            ratio_rows.push(row);
        } else {
            diff_rows.push(row);
        }
    }
    let pill = move |t: Option<SrcType>| {
        let label = t.map_or("All", SrcType::label);
        let dot = t.map(
            |t| view! { <span class=format!("ap-dot {}", t.class()) aria-hidden="true"></span> },
        );
        view! {
            <button
                type="button"
                class=move || if filter.get() == t { "ap-pill on" } else { "ap-pill" }
                aria-pressed=move || (filter.get() == t).to_string()
                on:click=move |_| filter.set(t)
            >
                {dot}
                {label}
            </button>
        }
        .into_any()
    };
    let pills: Vec<AnyView> = std::iter::once(None)
        .chain(
            SrcType::ALL
                .iter()
                .filter(|t| present.contains(t))
                .map(|t| Some(*t)),
        )
        .map(pill)
        .collect();
    let ticks: Vec<AnyView> = ap::LOG_TICKS
        .iter()
        .map(|t| {
            let class = if *t == 1.0 { "ap-tick one" } else { "ap-tick" };
            view! { <span class=class style=format!("left:{:.2}%", ap::log_pos(*t))>{ap::fmt_value(*t)}</span> }
                .into_any()
        })
        .collect();
    let empty = move || {
        filter.get().is_some_and(|f| !types.contains(&f)).then(
            || view! { <p class="ap-muted">"No estimates with a CI from this source type."</p> },
        )
    };
    let n_ratio = ratio_rows.len();
    let n_diff = diff_rows.len();
    view! {
        <section class="ap-card" aria-label="Every estimate with a 95% CI">
            <h3>"Every estimate with a 95% CI"</h3>
            <p class="ap-muted">
                "Ratios on a log scale (line at 1); differences centred on 0. A bar crossing the line is compatible with no difference. Tap a row for its study."
            </p>
            <div class="ap-pills">{pills}</div>
            {(n_ratio > 0).then(|| view! {
                {kicker("Ratios (RR \u{b7} OR \u{b7} HR)")}
                <div class="ap-frows">{ratio_rows}</div>
                <div class="ap-ticks" aria-hidden="true">{ticks}</div>
            })}
            {(n_diff > 0).then(|| view! {
                {kicker("Differences (each on its own scale)")}
                <div class="ap-frows">{diff_rows}</div>
            })}
            {empty}
        </section>
    }
    .into_any()
}

fn no_numbers_view() -> AnyView {
    view! {
        <p class="ap-note">
            "This answer has no structured numbers, so the arm comparison and CI charts are not shown. Everything else renders from the stored text."
        </p>
    }
    .into_any()
}

// ---------- 7. outcome by outcome ----------

/// One group of numbers, drawn by its chart kind (item 7).
fn group_view(c: &Ctx, g: &NumGroup) -> AnyView {
    let Some(w) = c.w else {
        return ().into_any();
    };
    let nums = g.nums(w);
    let i = g.outcome;
    let chart = match g.kind {
        ChartKind::Pair => {
            let max = ap::pair_max(&nums);
            let bars: Vec<AnyView> = g
                .numbers
                .iter()
                .zip(&nums)
                .enumerate()
                .map(|(idx, (&j, n))| bar_row(c, i, j, n, idx, max))
                .collect();
            view! {
                <div class="ap-bars">{bars}</div>
                {g.has_counts(w).then(|| view! {
                    <p class="ap-counts">"Counts as reported, not rates; arm sizes may differ."</p>
                })}
            }
            .into_any()
        }
        ChartKind::Ratio => {
            let rows: Vec<AnyView> = g
                .numbers
                .iter()
                .zip(&nums)
                .map(|(&j, n)| {
                    // Coloured by source type, as in the main forest.
                    let src = ap::src_type_of(w, &n.pmid).class();
                    let (a, b, v) = (
                        ap::log_pos(n.ci_low.unwrap_or(n.value)),
                        ap::log_pos(n.ci_high.unwrap_or(n.value)),
                        ap::log_pos(n.value),
                    );
                    view! {
                        <div class="ap-est">
                            <div class="ap-est-top">
                                <span>{n.arm.clone().unwrap_or_default()}</span>
                                <span class="ap-mono">{ap::ratio_estimate(n)} {c.num_mark(i, j)}</span>
                            </div>
                            <span class="ap-track-axis">
                                <span class="ap-line"></span>
                                <span class="ap-ref-line" style=format!("left:{:.2}%", ap::log_pos(1.0))></span>
                                <span class=format!("ap-ci {src}") style=format!("left:{a:.2}%;width:{:.2}%", (b - a).max(0.5))></span>
                                <span class=format!("ap-pt {src}") style=format!("left:{v:.2}%")></span>
                            </span>
                        </div>
                    }
                    .into_any()
                })
                .collect();
            let ticks: Vec<AnyView> = ap::LOG_TICKS
                .iter()
                .map(|t| {
                    let class = if *t == 1.0 { "ap-tick one" } else { "ap-tick" };
                    view! { <span class=class style=format!("left:{:.2}%", ap::log_pos(*t))>{ap::fmt_value(*t)}</span> }
                        .into_any()
                })
                .collect();
            view! { {rows} <div class="ap-ticks" aria-hidden="true">{ticks}</div> }.into_any()
        }
        ChartKind::Diff => {
            let (j, n) = (g.numbers[0], nums[0]);
            let m = ap::diff_scale(n);
            let (a, b, v) = (
                ap::diff_pos(n.ci_low.unwrap_or(n.value), m),
                ap::diff_pos(n.ci_high.unwrap_or(n.value), m),
                ap::diff_pos(n.value, m),
            );
            view! {
                <div class="ap-est">
                    <div class="ap-est-top">
                        <span>{n.arm.clone().unwrap_or_default()}</span>
                        <span class="ap-mono">{ap::diff_estimate(n)} {c.num_mark(i, j)}</span>
                    </div>
                    <span class="ap-track-axis">
                        <span class="ap-line"></span>
                        <span class="ap-ref-line" style="left:50%"></span>
                        <span class="ap-ci diff" style=format!("left:{a:.2}%;width:{:.2}%", (b - a).max(0.5))></span>
                        <span class="ap-pt diff round" style=format!("left:{v:.2}%")></span>
                    </span>
                    <div class="ap-scale" aria-hidden="true">
                        <span>{format!("\u{2212}{}", ap::fmt_axis(m))}</span>
                        <span>"0 \u{b7} no difference"</span>
                        <span>{format!("+{}", ap::fmt_axis(m))}</span>
                    </div>
                </div>
            }
            .into_any()
        }
        ChartKind::Text => {
            let last = nums.len().saturating_sub(1);
            let parts: Vec<AnyView> = g
                .numbers
                .iter()
                .zip(&nums)
                .enumerate()
                .map(|(k, (&j, n))| {
                    view! {
                        {ap::number_line(n)}
                        {c.num_mark(i, j)}
                        {(k < last).then_some("  vs  ")}
                    }
                    .into_any()
                })
                .collect();
            view! { <p class="ap-mono ap-numline">{parts}</p> }.into_any()
        }
    };
    let p = (g.kind != ChartKind::Text)
        .then(|| g.p_text(w))
        .flatten()
        .map(|p| view! { <p class="ap-p">{p}</p> });
    view! {
        <div class="ap-group">
            <div class="ap-group-head">
                <span class="ap-group-label">{g.label.clone()}</span>
                {c.study_link(&g.pmid, "ap-study")}
            </div>
            {chart}
            {p}
        </div>
    }
    .into_any()
}

/// One arm's bar: name, bar (with its CI whisker) and value.
fn bar_row(c: &Ctx, i: usize, j: usize, n: &Number, idx: usize, max: f64) -> AnyView {
    let fill = format!("width:{:.2}%", ap::pct(n.value, max).max(1.5));
    let whisk = ap::has_ci(n).then(|| {
        let (a, b) = (
            ap::pct(n.ci_low.unwrap_or(0.0), max),
            ap::pct(n.ci_high.unwrap_or(0.0), max),
        );
        view! { <span class="ap-whisk" style=format!("left:{a:.2}%;width:{:.2}%", (b - a).max(0.0))></span> }
    });
    view! {
        <div class="ap-bar-row">
            <span class="ap-arm">{n.arm.clone().unwrap_or_default()}</span>
            <span class="ap-bar"><span class=format!("ap-fill arm{}", idx % 4) style=fill></span>{whisk}</span>
            <span class="ap-mono">{ap::with_unit(n.value, n.unit.as_deref())} {c.num_mark(i, j)}</span>
            {ap::has_ci(n).then(|| view! {
                <span class="ap-sub ap-bar-ci">
                    {format!(
                        "95% CI {} to {}",
                        ap::fmt_value(n.ci_low.unwrap_or(0.0)),
                        ap::fmt_value(n.ci_high.unwrap_or(0.0))
                    )}
                </span>
            })}
        </div>
    }
    .into_any()
}

/// Outcome by outcome; `card` (S1e) draws it inside a card like the CI
/// card's, so the tops of a pair line up.
fn outcomes_view(c: &Ctx, card: bool) -> AnyView {
    let Some(w) = c.w else {
        return ().into_any();
    };
    let comp = c.a.completeness.as_ref();
    let cards: Vec<AnyView> = w
        .outcomes
        .iter()
        .enumerate()
        .map(|(i, o)| {
            let groups: Vec<AnyView> = c
                .groups
                .iter()
                .filter(|g| g.outcome == i)
                .map(|g| group_view(c, g))
                .collect();
            let additions: Vec<AnyView> = comp
                .into_iter()
                .flat_map(|cp| cp.additions_to(i))
                .map(|(k, a)| {
                    view! {
                        <p class="added">
                            {c.t(&a.text, &format!("outcome_additions.{k}.text"), c.cm())}
                            {ref_chips(&not_cited_at_end(&a.text, &a.pmids), &c.refs)}
                            {c.added_label()}
                            {unsourced_label(a.unsourced)}
                        </p>
                    }
                    .into_any()
                })
                .collect();
            let n_numbers = o.numbers.len();
            view! {
                <details class="ap-out" id=format!("outcome-{i}")>
                    <summary>
                        <span class="ap-out-head">
                            <span class="ap-out-name">{c.t_plain(&o.outcome, &format!("outcomes.{i}.outcome"), c.wm())}</span>
                            <span class="ap-tags">
                                {o.evidence_type.label().map(|l| view! { <span class="ap-tag">{l}</span> })}
                                {(n_numbers > 0).then(|| view! { <span class="ap-count-note">{crate::agent::answer_v3::count_noun(n_numbers as u32, "number", "numbers")}</span> })}
                            </span>
                            <span class="ap-first">{ap::first_sentence(&o.effect_text)}</span>
                        </span>
                        <span class="ap-out-side">{cert_chip(o.certainty)} <span class="ap-chev" aria-hidden="true"></span></span>
                    </summary>
                    <div class="ap-out-body">
                        {(!o.rct_evidence.is_empty()).then(|| view! {
                            <p class="ap-small"><strong>"Evidence: "</strong> {c.t(&o.rct_evidence, &format!("outcomes.{i}.rct_evidence"), c.wm())}</p>
                        })}
                        <p>{c.t(&o.effect_text, &format!("outcomes.{i}.effect_text"), c.wm())}</p>
                        {additions}
                        {(!groups.is_empty()).then(|| view! {
                            {kicker("Numbers")}
                            <div class="ap-groups">{groups}</div>
                        })}
                        <div class="ap-why">
                            <strong>{format!("Why {}: ", o.certainty.as_str())}</strong>
                            {c.t(&o.why, &format!("outcomes.{i}.why"), c.wm())}
                        </div>
                    </div>
                </details>
            }
            .into_any()
        })
        .collect();
    let added: Vec<AnyView> = comp
        .map(|cp| cp.added_outcomes.as_slice())
        .unwrap_or_default()
        .iter()
        .enumerate()
        .map(|(k, x)| {
            view! {
                <details class="ap-out added" id=format!("outcome-added-{k}")>
                    <summary>
                        <span class="ap-out-head">
                            <span class="ap-out-name">{c.t_plain(&x.outcome, &format!("added_outcomes.{k}.outcome"), c.cm())}</span>
                            <span class="ap-tags"><span class="ap-tag added">{ADDED_LABEL}</span></span>
                            <span class="ap-first">{ap::first_sentence(&x.effect_text)}</span>
                        </span>
                        <span class="ap-out-side">
                            {match x.certainty {
                                Some(c) => cert_chip(c),
                                None => view! { <span class="ap-cert">"not graded"</span> }.into_any(),
                            }}
                            <span class="ap-chev" aria-hidden="true"></span>
                        </span>
                    </summary>
                    <div class="ap-out-body">
                        <p>
                            {c.t(&x.effect_text, &format!("added_outcomes.{k}.effect_text"), c.cm())}
                            {ref_chips(&not_cited_at_end(&x.effect_text, &x.pmids), &c.refs)}
                            {unsourced_label(x.unsourced)}
                        </p>
                        <div class="ap-why">
                            <strong>{match x.certainty {
                                Some(c) => format!("Why {}: ", c.as_str()),
                                None => "Why: ".to_string(),
                            }}</strong>
                            {c.t(&x.why, &format!("added_outcomes.{k}.why"), c.cm())}
                        </div>
                    </div>
                </details>
            }
            .into_any()
        })
        .collect();
    let total = cards.len() + added.len();
    if card {
        return view! {
            <section class="ap-outcomes ap-card" aria-label="Outcome by outcome">
                <div class="ap-row">
                    <h3>"Outcome by outcome"</h3>
                    <span class="ap-muted">{format!("{total} outcomes \u{b7} tap to open")}</span>
                </div>
                {cards}
                {added}
            </section>
        }
        .into_any();
    }
    view! {
        <section class="ap-outcomes" aria-label="Outcome by outcome">
            <div class="ap-row">
                <h3>"Outcome by outcome"</h3>
                <span class="ap-muted">{format!("{total} outcomes \u{b7} tap to open")}</span>
            </div>
            {cards}
            {added}
        </section>
    }
    .into_any()
}

// ---------- 8. compare the arms ----------

/// One paired group, as plain data for the reactive panel.
#[derive(Clone)]
struct PairItem {
    label: String,
    outcome: String,
    certainty: Certainty,
    study: String,
    study_ref: Option<u16>,
    /// (arm, value, fill %, whisker (left %, width %), CI text, mark key).
    bars: Vec<PairBar>,
    p: Option<String>,
    counts: bool,
}

#[derive(Clone)]
struct PairBar {
    arm: String,
    value: String,
    fill: f64,
    whisk: Option<(f64, f64)>,
    ci: Option<String>,
    mark: Option<String>,
}

fn compare_view(c: &Ctx) -> AnyView {
    let Some(w) = c.w else {
        return ().into_any();
    };
    let pairs: Vec<NumGroup> = c
        .groups
        .iter()
        .filter(|g| g.kind == ChartKind::Pair)
        .cloned()
        .collect();
    let comparisons = ap::comparison_groups(w, &pairs);
    let items: Vec<PairItem> = pairs
        .iter()
        .map(|g| {
            let nums = g.nums(w);
            let max = ap::pair_max(&nums);
            let o = &w.outcomes[g.outcome];
            PairItem {
                label: g.label.clone(),
                outcome: o.outcome.clone(),
                certainty: o.certainty,
                study: c.study_name(&g.pmid),
                study_ref: c.ref_of(&g.pmid),
                bars: g
                    .numbers
                    .iter()
                    .zip(&nums)
                    .map(|(&j, n)| {
                        let f = format!("outcomes.{}.numbers.{j}", g.outcome);
                        PairBar {
                            arm: n.arm.clone().unwrap_or_default(),
                            value: ap::with_unit(n.value, n.unit.as_deref()),
                            fill: ap::pct(n.value, max).max(1.5),
                            whisk: ap::has_ci(n).then(|| {
                                let a = ap::pct(n.ci_low.unwrap_or(0.0), max);
                                let b = ap::pct(n.ci_high.unwrap_or(0.0), max);
                                (a, (b - a).max(0.0))
                            }),
                            ci: ap::has_ci(n).then(|| {
                                format!(
                                    "95% CI {} to {}",
                                    ap::fmt_value(n.ci_low.unwrap_or(0.0)),
                                    ap::fmt_value(n.ci_high.unwrap_or(0.0))
                                )
                            }),
                            mark: whole_marked(&f, &c.a.untraced).then(|| format!("w:{f}")),
                        }
                    })
                    .collect(),
                p: g.p_text(w),
                counts: g.has_counts(w),
            }
        })
        .collect();
    let cmp = RwSignal::new(0usize);
    let item = RwSignal::new(0usize);
    let pills: Vec<AnyView> = comparisons
        .iter()
        .enumerate()
        .map(|(k, g)| {
            view! {
                <button
                    type="button"
                    class=move || if cmp.get() == k { "ap-pill on" } else { "ap-pill" }
                    aria-pressed=move || (cmp.get() == k).to_string()
                    on:click=move |_| {
                        cmp.set(k);
                        item.set(0);
                    }
                >
                    {g.label.clone()}
                    <span class="ap-pill-n">{format!(" \u{b7} {}", g.items.len())}</span>
                </button>
            }
            .into_any()
        })
        .collect();
    let items = Arc::new(items);
    let comparisons = Arc::new(comparisons);
    let list = {
        let items = items.clone();
        let comparisons = comparisons.clone();
        move || {
            let Some(g) = comparisons.get(cmp.get()).or_else(|| comparisons.first()) else {
                return ().into_any();
            };
            g.items
                .iter()
                .enumerate()
                .map(|(pos, &k)| {
                    let it = &items[k];
                    view! {
                        <button
                            type="button"
                            class=move || if item.get() == pos { "ap-measure on" } else { "ap-measure" }
                            aria-pressed=move || (item.get() == pos).to_string()
                            on:click=move |_| item.set(pos)
                        >
                            <span class="ap-measure-text">
                                <span class="ap-measure-label">{it.label.clone()}</span>
                                <span class="ap-sub">{format!("{} \u{b7} {}", it.study, it.outcome)}</span>
                            </span>
                            {cert_chip(it.certainty)}
                        </button>
                    }
                    .into_any()
                })
                .collect_view()
                .into_any()
        }
    };
    let panel = move || {
        let Some(g) = comparisons.get(cmp.get()).or_else(|| comparisons.first()) else {
            return ().into_any();
        };
        let Some(&k) = g.items.get(item.get()).or_else(|| g.items.first()) else {
            return ().into_any();
        };
        let it = items[k].clone();
        let bars: Vec<AnyView> = it
            .bars
            .iter()
            .enumerate()
            .map(|(idx, b)| {
                view! {
                    <div class="ap-bigbar">
                        <div class="ap-est-top">
                            <span class="ap-arm">{b.arm.clone()}</span>
                            <span class="ap-mono ap-bigval">{b.value.clone()} {b.mark.clone().map(|k| mark_el(Mark::NotTraced, k))}</span>
                        </div>
                        <span class="ap-bar big">
                            <span class=format!("ap-fill arm{}", idx % 4) style=format!("width:{:.2}%", b.fill)></span>
                            {b.whisk.map(|(l, wd)| view! { <span class="ap-whisk" style=format!("left:{l:.2}%;width:{wd:.2}%")></span> })}
                        </span>
                        {b.ci.clone().map(|t| view! { <span class="ap-sub">{t}</span> })}
                    </div>
                }
                .into_any()
            })
            .collect();
        let study = match it.study_ref {
            Some(n) => view! {
                <a class="ap-study big" href=format!("#ref-{n}") rel="external" on:click=sheet_click(n)>{it.study.clone()}</a>
            }
            .into_any(),
            None => view! { <span class="ap-sub">{it.study.clone()}</span> }.into_any(),
        };
        view! {
            <div class="ap-panel">
                <div class="ap-row">
                    <div>
                        <div class="ap-panel-label">{it.label.clone()}</div>
                        <div class="ap-sub">{format!("Outcome: {}", it.outcome)}</div>
                    </div>
                    {cert_chip(it.certainty)}
                </div>
                {bars}
                <div class="ap-row">
                    <span>{it.p.clone().unwrap_or_else(|| "No p value stored".to_string())}</span>
                    {study}
                </div>
                {it.counts.then(|| view! { <p class="ap-counts">"Counts as reported, not rates; arm sizes may differ."</p> })}
            </div>
        }
        .into_any()
    };
    view! {
        <section class="ap-card" aria-label="Compare the arms">
            <h3>"Compare the arms"</h3>
            <p class="ap-muted">"Side-by-side arm results, grouped by each study's comparison. Most have no confidence interval."</p>
            <div class="ap-pills">{pills}</div>
            <div class="ap-measures">{list}</div>
            {panel}
        </section>
    }
    .into_any()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// S1e (review I1): a jump to an outcome unfolds its fold first; the
    /// CSS unclips a box holding the target or the focus before hydration.
    #[test]
    fn a_jump_into_a_fold_opens_it() {
        assert_eq!(fold_of("outcome-40"), Some(FOLD_WHY));
        assert_eq!(fold_of("outcome-added-2"), Some(FOLD_WHY));
        assert_eq!(fold_of("ref-3"), None);
        assert_eq!(fold_of("sheet-1"), None);
        for rule in [
            ".ap-lx-box:has(:target, :focus-visible) { overflow:visible; }",
            ".ap-lx-box:has(:target, :focus-visible) > * { position:static; }",
            ".ap-lx-box:has(:target, :focus-visible)::after { display:none; }",
            ".ap-lx-r:has(.ap-lx-box :target, .ap-lx-box :focus-visible) > .ap-lx-cb:not(:checked) ~ .ap-lx-more { display:none; }",
            "@media print {",
            "@media (max-width:1099.98px) {",
        ] {
            assert!(LAYOUT_STYLE.contains(rule), "{rule}");
        }
        // go_to_outcome opens the fold before the link scrolls.
        let src = include_str!("details.rs");
        let f = &src[src.find("fn go_to_outcome").unwrap()..];
        let f = &f[..f.find("\n}\n").unwrap()];
        let open = f.find("open_fold(").expect("go_to_outcome opens the fold");
        assert!(f.contains("fold_of(id)"));
        assert!(open < f.find("set_attribute(\"open\"").unwrap());
    }

    #[test]
    fn a_ref_hash_names_its_study_sheet() {
        assert_eq!(sheet_in_hash("#ref-3"), Some(3));
        assert_eq!(sheet_in_hash("#ref-12"), Some(12));
        for h in [
            "",
            "#",
            "#expert",
            "#ref-",
            "#ref-x",
            "#ref-3x",
            "ref-3",
            "#ref-70000",
        ] {
            assert_eq!(sheet_in_hash(h), None, "{h}");
        }
    }
}
