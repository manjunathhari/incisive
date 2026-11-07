//! The Explore page (`/explore`, `GRAPH=on`): the user's own map as a list
//! of diseases, treatments, outcomes, questions and ranked papers, each
//! group folded at five rows, with a card for the picked row (in place on a
//! phone, at the side on a wide screen), and the map: the dots the server
//! laid out once, drawn as real buttons over SVG lines, with tap to light up
//! a dot's neighbours, zoom, drag, find, the "Show on the map" filters and a
//! time-lapse by paper year (in the browser, with no new layout).
//!
//! Everything is drawn on the server first, so the list works before the
//! page hydrates. Every fetch goes through the server functions here, which
//! use the signed-in user and know nothing with `GRAPH` off. Labels are
//! text, never markup; nothing here logs a name or a question.

use std::collections::BTreeSet;

use leptos::prelude::*;
use leptos::server_fn::codec::Json;
use serde::{Deserialize, Serialize};

use super::error_text;
use crate::explore_page::{self as ep, Card, Group, Row};
use crate::graph_view::{DotKind, MapData, MapFilter};

/// The page's styles: ivory tokens and the board's kind and rank colours.
pub const EXPLORE_STYLE: &str = r#"
.kg { --kg-organ:#635A8C; --kg-disease:var(--iv-accent); --kg-run:var(--iv-ink); --kg-treatment:#1F7A6E; --kg-outcome:#D49A1F;
  --kg-rct:#7A2E12; --kg-meta:#C2552F; --kg-pro:#D97E58; --kg-retro:#E8A98D; --kg-series:#F2CDBB; --kg-other:#BDB6A8;
  display:flex; flex-direction:column; gap:10px; }
.kg [hidden] { display:none !important; }
.kg-switch { align-self:flex-start; display:inline-flex; gap:2px; background:var(--iv-card); border:1px solid var(--iv-border); border-radius:999px; padding:3px; }
.iv-wrap .kg-switch button { min-height:42px; padding:0 18px; border:0; border-radius:999px; background:transparent; color:var(--iv-body); font-family:inherit; font-size:14px; font-weight:600; cursor:pointer; }
.iv-wrap .kg-switch button[aria-pressed="true"] { background:var(--iv-tint); color:var(--iv-accent); }
.kg-body { display:flex; flex-wrap:wrap; gap:16px; align-items:flex-start; }
.kg-main { flex:1 1 560px; max-width:860px; min-width:0; display:flex; flex-direction:column; gap:10px; }
.kg-side { flex:0 1 340px; min-width:0; display:flex; flex-direction:column; gap:10px; }
.kg-find { display:flex; flex-direction:column; gap:4px; }
.kg-find label { margin:0; font-size:12px; font-weight:600; color:var(--iv-body); }
.kg-find input { box-sizing:border-box; width:100%; min-height:46px; padding:0 14px; border:1px solid var(--iv-line); border-radius:14px; background:var(--iv-card); font-family:inherit; font-size:16px; color:var(--iv-ink); }
.kg-find input:focus { outline:2px solid var(--iv-accent); outline-offset:1px; }
.kg-chips { display:flex; gap:6px; overflow-x:auto; padding-bottom:2px; }
.iv-wrap .kg-chips button { flex:none; min-height:44px; padding:0 14px; border-radius:999px; border:1px solid var(--iv-border); background:var(--iv-card); color:var(--iv-ink); font-family:inherit; font-size:14px; cursor:pointer; }
.iv-wrap .kg-chips button[aria-pressed="true"] { border:2px solid var(--iv-ink); padding:0 13px; }
.kg-map { display:flex; flex-direction:column; gap:10px; }
.kg-map-frame { position:relative; box-sizing:border-box; width:100%; background:var(--iv-card); border:1px solid var(--iv-border); border-radius:18px; overflow:hidden; touch-action:none; user-select:none; -webkit-user-select:none; }
.iv-wrap button.kg-bg { position:absolute; left:0; top:0; width:100%; height:100%; padding:0; border:0; background:transparent; cursor:grab; }
.kg-layer { position:absolute; left:0; top:0; width:100%; height:100%; transform-origin:0 0; will-change:transform; pointer-events:none; }
.kg-svg { position:absolute; left:0; top:0; width:100%; height:100%; pointer-events:none; overflow:visible; }
.kg-ln { stroke:var(--kg-other); } .kg-ln.on { stroke:var(--iv-body); }
.iv-wrap button.kg-dot { position:absolute; transform:translate(-50%,-50%); width:calc(max(30px, var(--d) * var(--g, 1) + 8px) * var(--ik, 1)); height:calc(max(30px, var(--d) * var(--g, 1) + 8px) * var(--ik, 1)); padding:0; border:0; background:transparent; display:flex; align-items:center; justify-content:center; cursor:pointer; border-radius:50%; pointer-events:auto; }
.kg-mk { display:block; box-sizing:border-box; border-radius:50%; }
.kg-dot .kg-mk { width:calc(var(--d) * var(--g, 1) * var(--ik, 1)); height:calc(var(--d) * var(--g, 1) * var(--ik, 1)); }
.kg-mk.sq { border-radius:2px; }
.iv-wrap button.kg-dot.on .kg-mk { border:calc(2px * var(--ik, 1)) solid var(--iv-ink); }
.kg-mk.organ { background:var(--kg-organ); } .kg-mk.disease { background:var(--kg-disease); } .kg-mk.run { background:var(--kg-run); }
.kg-mk.treatment { background:var(--kg-treatment); } .kg-mk.outcome { background:var(--kg-outcome); }
.kg-mk.rct, .kg-num.rct { background:var(--kg-rct); } .kg-mk.meta, .kg-num.meta { background:var(--kg-meta); } .kg-mk.prospective, .kg-num.prospective { background:var(--kg-pro); }
.kg-mk.retrospective, .kg-num.retrospective { background:var(--kg-retro); } .kg-mk.case_series, .kg-num.case_series { background:var(--kg-series); } .kg-mk.other, .kg-num.other { background:var(--kg-other); }
.kg-lab { position:absolute; transform:scale(var(--ik, 1)); transform-origin:0 0; pointer-events:none; font-size:12px; line-height:14px; white-space:nowrap; overflow:hidden; background:rgba(255,255,255,0.86); border-radius:4px; padding:0 3px; color:var(--iv-ink); }
.kg-lab.b { font-weight:600; }
.kg-zoom { position:absolute; right:10px; top:10px; display:flex; flex-direction:column; gap:8px; }
.iv-wrap .kg-zoom button { width:44px; height:44px; padding:0; border-radius:12px; border:1px solid var(--iv-border); background:var(--iv-card); font-family:inherit; font-size:20px; color:var(--iv-ink); cursor:pointer; }
.iv-wrap .kg-zoom button.fit { font-size:13px; }
.kg-year { position:absolute; left:12px; top:10px; font-size:28px; font-weight:700; color:var(--iv-accent); pointer-events:none; }
.iv-wrap button.kg-play { min-height:44px; padding:0 16px; border-radius:999px; border:1px solid var(--iv-accent); background:var(--iv-card); color:var(--iv-accent); font-family:inherit; font-size:14px; font-weight:600; cursor:pointer; }
.kg-papers { display:flex; flex-direction:column; gap:6px; }
.kg-papers h3 { margin:0; font-size:13px; font-weight:600; color:var(--iv-ink); }
.iv-wrap button.kg-paper { display:flex; gap:10px; width:100%; text-align:left; padding:10px 12px; background:var(--iv-card); border:1px solid var(--iv-border); border-radius:14px; font-family:inherit; color:var(--iv-ink); cursor:pointer; }
.kg-num { flex:none; width:24px; height:24px; border-radius:4px; color:var(--iv-ink); font-size:12px; font-weight:700; display:flex; align-items:center; justify-content:center; }
.kg-num.rct, .kg-num.meta { color:#fff; }
.kg-legend { background:var(--iv-card); border:1px solid var(--iv-border); border-radius:16px; }
.iv-wrap button.kg-legend-btn { display:flex; justify-content:space-between; align-items:center; width:100%; min-height:48px; padding:0 14px; background:transparent; border:0; border-radius:16px; font-family:inherit; font-size:13px; font-weight:600; color:var(--iv-body); cursor:pointer; }
.kg-legend-body { padding:0 14px 14px; display:flex; flex-direction:column; gap:10px; }
.kg-keys { display:flex; flex-wrap:wrap; gap:6px; }
.iv-wrap .kg-keys button { display:inline-flex; align-items:center; gap:6px; min-height:44px; padding:0 10px; border-radius:999px; border:1px solid var(--iv-border); background:var(--iv-card); font-family:inherit; font-size:12px; color:var(--iv-ink); cursor:pointer; }
.iv-wrap .kg-keys button[aria-pressed="false"] { color:var(--iv-muted); }
.iv-wrap .kg-keys button[aria-pressed="false"] .kg-mk { background:#E8E4DC; }
.kg-selects { display:grid; grid-template-columns:repeat(2, minmax(0, 1fr)); gap:8px; }
.kg-selects div { display:flex; flex-direction:column; gap:4px; min-width:0; }
.kg-selects label { margin:0; font-size:12px; font-weight:600; color:var(--iv-body); }
.kg-selects select { width:100%; min-height:44px; border:1px solid var(--iv-line); border-radius:12px; background:var(--iv-card); font-family:inherit; font-size:13px; color:var(--iv-ink); }
.kg-foot { display:flex; flex-wrap:wrap; align-items:center; gap:8px; font-size:12px; color:var(--iv-muted); }
.kg-groups { display:flex; flex-direction:column; gap:14px; }
.kg-group { display:flex; flex-direction:column; gap:6px; }
.kg-group h2 { margin:0; font-size:14px; font-weight:600; color:var(--iv-body); }
.kg-row { background:var(--iv-card); border:1px solid var(--iv-border); border-radius:14px; }
.kg-row.on { border-color:var(--iv-accent); }
.iv-wrap button.kg-row-btn { display:flex; gap:10px; align-items:center; width:100%; min-height:48px; text-align:left; padding:9px 12px; background:transparent; border:0; border-radius:14px; font-family:inherit; color:var(--iv-ink); cursor:pointer; }
.kg-sw { flex:none; width:12px; height:12px; border-radius:50%; }
.kg-sw.sq { border-radius:2px; }
.kg-sw.organ { background:var(--kg-organ); } .kg-sw.disease { background:var(--kg-disease); } .kg-sw.run { background:var(--kg-run); }
.kg-sw.treatment { background:var(--kg-treatment); } .kg-sw.outcome { background:var(--kg-outcome); }
.kg-sw.rct { background:var(--kg-rct); } .kg-sw.meta { background:var(--kg-meta); } .kg-sw.prospective { background:var(--kg-pro); }
.kg-sw.retrospective { background:var(--kg-retro); } .kg-sw.case_series { background:var(--kg-series); } .kg-sw.other { background:var(--kg-other); }
.kg-row-text { display:flex; flex-direction:column; gap:1px; min-width:0; }
.kg-row-title { font-size:14px; font-weight:500; line-height:1.3; overflow-wrap:anywhere; }
.kg-row-sub { font-size:12px; line-height:1.35; color:var(--iv-muted); }
.iv-wrap button.kg-more { align-self:flex-start; min-height:44px; padding:0 14px; border:1px solid var(--iv-line); border-radius:999px; background:var(--iv-card); color:var(--iv-accent); font-family:inherit; font-size:13px; font-weight:500; cursor:pointer; }
.kg-nomatch { margin:0; padding:14px; border:1px dashed var(--iv-line); border-radius:14px; font-size:14px; line-height:1.5; color:var(--iv-body); }
.kg-card { background:var(--iv-card); border:1px solid var(--iv-border); border-radius:16px; padding:14px; display:flex; flex-direction:column; gap:8px; }
.kg-card.kg-inline { background:transparent; border:0; border-radius:0; padding:0 12px 12px; }
.kg-kicker { font-size:11px; font-weight:600; color:var(--iv-muted); }
.kg-card-title { font-size:16px; font-weight:600; line-height:1.3; overflow-wrap:anywhere; }
.kg-card-body { font-size:14px; line-height:1.5; color:var(--iv-body); overflow-wrap:anywhere; }
.kg-flag { margin:0; align-self:flex-start; font-size:12px; font-weight:600; padding:3px 9px; border-radius:999px; background:var(--iv-warn-bg); color:var(--iv-warn); }
.kg-flag.alert { background:#FDECEA; color:var(--iv-alert); }
.kg-links { display:flex; flex-wrap:wrap; gap:6px; }
.iv-wrap .kg-links button { min-height:44px; padding:0 12px; border:1px solid var(--iv-line); border-radius:999px; background:var(--iv-tint); color:var(--iv-accent); font-family:inherit; font-size:13px; font-weight:500; cursor:pointer; }
.iv-wrap a.kg-act { align-self:flex-start; display:inline-flex; align-items:center; min-height:44px; padding:0 16px; border-radius:999px; background:var(--iv-accent); color:#fff; text-decoration:none; font-size:14px; font-weight:600; }
.iv-wrap a.kg-act:hover { color:#fff; background:#223a6a; }
.iv-wrap a.kg-act.alt { background:var(--iv-card); color:var(--iv-accent); border:1px solid var(--iv-accent); }
.kg-note { margin:0; font-size:12px; line-height:1.5; color:var(--iv-muted); }
@media (min-width:700px) { .kg-card.kg-inline { display:none; } }
@media (max-width:699.98px) {
  .kg-main, .kg-side { flex-basis:100%; }
  .kg[data-view="list"] .kg-side-card { display:none; }
}
"#;

// ---------- server functions ----------

/// The signed-in user, with `GRAPH=on`; with it off nothing is known (404).
#[cfg(feature = "ssr")]
async fn explore_user() -> Result<(crate::server::AppState, crate::identity::User), ServerFnError> {
    use crate::identity::CurrentUser;
    let (state, user) = super::signed_in().await?;
    if !state.graph.settings.on {
        super::set_status(axum::http::StatusCode::NOT_FOUND);
        return Err(ServerFnError::new(ep::NOT_FOUND));
    }
    let CurrentUser(user) = user.map_err(|r| {
        super::set_status(r.status());
        ServerFnError::new(r.message())
    })?;
    Ok((state, user))
}

/// Every dot the filter keeps, not laid out (the List view).
#[cfg(feature = "ssr")]
async fn load(filter: &MapFilter) -> Result<MapData, ServerFnError> {
    let (state, user) = explore_user().await?;
    let mut data = crate::graph::read::load_map_all(&state.pool, user.id, filter)
        .await
        .map_err(|_| {
            tracing::error!(user_id = user.id, "explore: map read failed");
            super::set_status(axum::http::StatusCode::INTERNAL_SERVER_ERROR);
            ServerFnError::new("Incisive couldn't load your map. Please try again.")
        })?;
    // A question card shows the bottom line as text: no citation marks.
    for d in &mut data.dots {
        d.bottom_line = d
            .bottom_line
            .take()
            .map(|b| crate::answer_page::strip_citations(&b))
            .filter(|b| !b.is_empty());
    }
    Ok(data)
}

/// A frame size the page sent, kept sane (non-finite or not positive: the
/// desktop frame; at most 4000).
#[cfg(feature = "ssr")]
fn frame_size(w: f32, h: f32) -> (f32, f32) {
    let size = |v: f32, fallback: f32| {
        if v.is_finite() && v > 0.0 {
            v.min(4000.0)
        } else {
            fallback
        }
    };
    (size(w, ep::DESKTOP_FRAME.0), size(h, ep::DESKTOP_FRAME.1))
}

/// Cut the map to [`crate::graph::read::MAX_DOTS`] and lay it out, once,
/// for the frame.
#[cfg(feature = "ssr")]
fn for_frame(mut data: MapData, (w, h): (f32, f32)) -> MapData {
    ep::cap_for_map(&mut data, crate::graph::read::MAX_DOTS);
    crate::graph_view::layout(&mut data.dots, &data.lines, w, h);
    data
}

/// The signed-in user's map for a `w` by `h` frame: at most
/// [`crate::graph::read::MAX_DOTS`] dots (papers cut to the highest ranked,
/// newest first), laid out here, once. Not found with `GRAPH` off.
#[server(prefix = "/api/fn", endpoint = "explore_map", input = Json)]
pub async fn get_map(
    #[server(default)] filter: MapFilter,
    #[server(default)] w: f32,
    #[server(default)] h: f32,
) -> Result<MapData, ServerFnError> {
    let data = load(&filter).await?;
    Ok(for_frame(data, frame_size(w, h)))
}

/// The signed-in user's whole map under the filter, not laid out (the List
/// view always has everything). Not found with `GRAPH` off.
#[server(prefix = "/api/fn", endpoint = "explore_list", input = Json)]
pub async fn get_list(#[server(default)] filter: MapFilter) -> Result<MapData, ServerFnError> {
    load(&filter).await
}

/// What the page opens with: the whole map (the List) and the same dots
/// cut and laid out for the frame, from one read.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ExploreStart {
    pub all: MapData,
    pub map: MapData,
    pub frame: (f32, f32),
}

/// The page's first data: one read of the signed-in user's map, for the
/// List and for a `w` by `h` map. Not found with `GRAPH` off.
#[server(prefix = "/api/fn", endpoint = "explore_start", input = Json)]
pub async fn get_start(
    #[server(default)] w: f32,
    #[server(default)] h: f32,
) -> Result<ExploreStart, ServerFnError> {
    let all = load(&MapFilter::default()).await?;
    let frame = frame_size(w, h);
    let map = for_frame(all.clone(), frame);
    Ok(ExploreStart { all, map, frame })
}

/// The name of concept `concept_id` for "Ask about this", only when it is
/// on the signed-in user's own map (reached through their run links, and
/// through merges as the map does); otherwise `None`, as with `GRAPH` off,
/// a signed-out caller aside (that one is refused). The name is never
/// logged.
#[server(prefix = "/api/fn", endpoint = "topic_label", input = Json)]
pub async fn topic_label(concept_id: i64) -> Result<Option<String>, ServerFnError> {
    use crate::identity::CurrentUser;
    let (state, user) = super::signed_in().await?;
    if !state.graph.settings.on {
        return Ok(None);
    }
    let CurrentUser(user) = user.map_err(|r| {
        super::set_status(r.status());
        ServerFnError::new(r.message())
    })?;
    crate::graph::read::topic_label(&state.pool, user.id, concept_id)
        .await
        .map_err(|_| {
            tracing::error!(user_id = user.id, "explore: topic read failed");
            ServerFnError::new("Incisive couldn't load that topic. Please try again.")
        })
}

// ---------- the page ----------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum View {
    Map,
    List,
}

impl View {
    fn as_str(self) -> &'static str {
        match self {
            View::Map => "map",
            View::List => "list",
        }
    }
}

/// The view this browser last chose (`None` when storage is absent,
/// blocked or throws; browser only).
fn load_view() -> Option<View> {
    #[cfg(feature = "hydrate")]
    {
        // Every step returns a Result or Option: a storage exception is
        // caught by web-sys and ends here as `None`.
        let v = web_sys::window()?
            .local_storage()
            .ok()??
            .get_item(ep::VIEW_STORAGE_KEY)
            .ok()??;
        match v.as_str() {
            "map" => Some(View::Map),
            "list" => Some(View::List),
            _ => None,
        }
    }
    #[cfg(not(feature = "hydrate"))]
    None
}

/// Remember the view in this browser; any storage failure is ignored.
fn save_view(v: View) {
    #[cfg(feature = "hydrate")]
    {
        if let Some(s) = web_sys::window().and_then(|w| w.local_storage().ok().flatten()) {
            let _ = s.set_item(ep::VIEW_STORAGE_KEY, v.as_str());
        }
    }
    #[cfg(not(feature = "hydrate"))]
    let _ = v;
}

/// The page's state, shared by its parts.
#[derive(Clone, Copy)]
struct Ui {
    view: RwSignal<View>,
    organ: RwSignal<Option<i64>>,
    query: RwSignal<String>,
    sel: RwSignal<Option<String>>,
    open: RwSignal<BTreeSet<String>>,
    /// The page runs in the browser (hydrated): the switch works.
    ready: RwSignal<bool>,
    /// "Show on the map": the legend keys switched off, the paper years and
    /// the run certainty.
    hidden: RwSignal<BTreeSet<&'static str>>,
    years: RwSignal<&'static str>,
    certainty: RwSignal<&'static str>,
    legend_open: RwSignal<bool>,
}

/// The filter the chips and "Show on the map" make.
fn filter_of(ui: Ui) -> MapFilter {
    let years = ui.years.get();
    let (_, _, year_from, year_to) = ep::YEARS
        .iter()
        .copied()
        .find(|y| y.0 == years)
        .unwrap_or(ep::YEARS[0]);
    let certainty = ui.certainty.get();
    MapFilter {
        organ: ui.organ.get(),
        year_from,
        year_to,
        certainty: (certainty != "all").then(|| certainty.to_string()),
        hidden: ui
            .hidden
            .get()
            .iter()
            .flat_map(|k| ep::hidden_codes(k))
            .collect(),
    }
}

/// What a map fetch is for: the filter, the frame and the "Try again"
/// count.
type MapKey = (MapFilter, (f32, f32), u32);

/// A pinch's start: the fingers' distance and middle, and k, tx, ty.
type Pinch = (f32, (f32, f32), f32, f32, f32);

/// A drag or a pinch on the map, in frame units.
#[derive(Debug, Clone, Default)]
struct Drag {
    /// The pointers down: id and where.
    pts: Vec<(i32, f32, f32)>,
    /// One pointer: where it went down and the pan then.
    start: Option<(f32, f32, f32, f32)>,
    /// Two pointers: their distance and middle, and the zoom and pan then.
    pinch: Option<Pinch>,
    /// The map moved since the pointer went down, so it was not a tap.
    moved: bool,
}

/// The map's state.
#[derive(Clone, Copy)]
struct MapUi {
    /// The laid-out map and the frame it was laid out for.
    data: RwSignal<(MapData, (f32, f32))>,
    /// The frame this browser wants (its width decides).
    frame: RwSignal<(f32, f32)>,
    /// Zoom and pan as drawn, live while a finger moves: k, tx, ty. Only the
    /// map layer's transform follows it.
    view: RwSignal<(f32, f32, f32)>,
    /// Zoom and pan at rest (after a zoom or at the end of a drag): the
    /// labels are worked out for this.
    settled: RwSignal<(f32, f32, f32)>,
    /// Frame units per screen pixel (the frame is drawn at its box's size).
    unit: RwSignal<f32>,
    /// The time-lapse's year (`None`: every dot) and whether it plays.
    until: RwSignal<Option<i32>>,
    playing: RwSignal<bool>,
    busy: RwSignal<bool>,
    error: RwSignal<Option<String>>,
    /// Bumped by "Try again" after a failed fetch.
    retry: RwSignal<u32>,
    drag: StoredValue<Drag>,
    timer: StoredValue<Option<leptos::leptos_dom::helpers::IntervalHandle>, LocalStorage>,
    node: NodeRef<leptos::html::Div>,
}

impl MapUi {
    fn frame_now(self) -> (f32, f32) {
        self.data.with_untracked(|(_, f)| *f)
    }

    /// Zoom and pan, drawn and at rest.
    fn place(self, v: (f32, f32, f32)) {
        self.view.set(v);
        self.settled.set(v);
    }

    /// The labels catch up with a drag or a pinch.
    fn settle(self) {
        let v = self.view.get_untracked();
        if self.settled.get_untracked() != v {
            self.settled.set(v);
        }
    }

    /// Zoom by `f` about a point of the frame.
    fn zoom_at(self, f: f32, (cx, cy): (f32, f32)) {
        let (k, tx, ty) = self.view.get_untracked();
        let nk = (k * f).clamp(ep::ZOOM_MIN, ep::ZOOM_MAX);
        let m = nk / k;
        self.place((nk, cx - (cx - tx) * m, cy - (cy - ty) * m));
    }

    fn zoom(self, f: f32) {
        let (w, h) = self.frame_now();
        self.zoom_at(f, (w / 2.0, h / 2.0));
    }

    /// How big the frame is drawn: frame units per pixel (browser only).
    /// A hidden frame (width 0) is skipped. Safe once the page is gone
    /// (it may run on a later frame).
    fn measure(self) {
        #[cfg(feature = "hydrate")]
        if let Some(el) = self.node.try_get_untracked().flatten() {
            let w = el.get_bounding_client_rect().width();
            let Some(fw) = self.data.try_with_untracked(|(_, f)| f.0) else {
                return;
            };
            if w > 0.0 {
                let u = fw / w as f32;
                if self
                    .unit
                    .try_get_untracked()
                    .is_some_and(|old| (old - u).abs() > 1e-3)
                {
                    let _ = self.unit.try_set(u);
                }
            }
        }
    }

    /// Ends the time-lapse; also called from `on_cleanup`, when the
    /// signals may already be gone.
    fn stop(self) {
        if let Some(h) = self.timer.try_update_value(Option::take).flatten() {
            h.clear();
        }
        let _ = self.playing.try_set(false);
    }
}

/// The frame the server draws first: from the browser's User-Agent.
fn first_frame() -> (f32, f32) {
    #[cfg(feature = "ssr")]
    {
        let ua = use_context::<axum::http::request::Parts>().and_then(|p| {
            p.headers
                .get(axum::http::header::USER_AGENT)
                .and_then(|v| v.to_str().ok())
                .map(str::to_string)
        });
        ep::frame_for_agent(ua.as_deref())
    }
    #[cfg(not(feature = "ssr"))]
    ep::DESKTOP_FRAME
}

/// This browser window's width (browser only).
fn window_width() -> Option<f64> {
    #[cfg(feature = "hydrate")]
    {
        web_sys::window()?.inner_width().ok()?.as_f64()
    }
    #[cfg(not(feature = "hydrate"))]
    None
}

/// With `GRAPH` off (or no app state) this page does not exist (server
/// only).
#[cfg(feature = "ssr")]
pub(super) fn graph_off() -> bool {
    !use_context::<crate::server::AppState>().is_some_and(|s| s.graph.settings.on)
}

#[component]
pub fn ExplorePage() -> impl IntoView {
    // With `GRAPH` off this page does not exist.
    #[cfg(feature = "ssr")]
    if graph_off()
        && let Some(o) = use_context::<leptos_axum::ResponseOptions>()
    {
        o.set_status(axum::http::StatusCode::NOT_FOUND);
    }
    // Drawn on the server as the List, which works with no script; the
    // switch shows, and Map (the default) or the remembered choice is
    // taken, once the page runs in the browser.
    let ui = Ui {
        view: RwSignal::new(View::List),
        organ: RwSignal::new(None),
        query: RwSignal::new(String::new()),
        sel: RwSignal::new(None),
        open: RwSignal::new(BTreeSet::new()),
        ready: RwSignal::new(false),
        hidden: RwSignal::new(BTreeSet::new()),
        years: RwSignal::new("all"),
        certainty: RwSignal::new("all"),
        legend_open: RwSignal::new(false),
    };
    Effect::new(move |_| {
        ui.view.set(load_view().unwrap_or(View::Map));
        ui.ready.set(true);
    });
    // Blocking: the list and the map are in the first HTML, not streamed
    // in by script. One read serves both.
    let frame = first_frame();
    let start = Resource::new_blocking(|| (), move |_| get_start(frame.0, frame.1));
    // In the browser: a page that does not exist is drawn at the width of
    // any unknown path (the server has already drawn it so).
    if let Some(super::ivory::PageMissing(missing)) = use_context() {
        Effect::new(move |_| {
            let off = start.with(|r| matches!(r, Some(Err(e)) if error_text(e) == ep::NOT_FOUND));
            if missing.get_untracked() != off {
                missing.set(off);
            }
        });
        on_cleanup(move || {
            let _ = missing.try_set(false);
        });
    }
    let page = move |inner: AnyView| {
        view! {
            <style>{EXPLORE_STYLE}</style>
            <div class="iv-wrap kg" data-view=move || ui.view.get().as_str()>
                <h1 class="iv-sr">{ep::TITLE}</h1>
                {inner}
            </div>
        }
        .into_any()
    };
    view! {
        <Suspense fallback=|| ()>
            {move || {
                start
                    .get()
                    .map(|r| match r {
                        // With `GRAPH` off: the app's usual not-found, nothing
                        // of this page.
                        Err(e) if error_text(&e) == ep::NOT_FOUND => {
                            view! { <p>{ep::NOT_FOUND}</p> }.into_any()
                        }
                        Err(e) => page(view! { <p class="box amber" role="alert">{error_text(&e)}</p> }.into_any()),
                        Ok(d) if d.all.runs == 0 => page(view! { <p class="iv-empty">{ep::EMPTY}</p> }.into_any()),
                        Ok(d) => page(explore_body(ui, d).into_any()),
                    })
            }}
        </Suspense>
    }
}

/// The switch, the find box, the organ chips, the map, the list and the
/// side column, for a user with runs on the map.
fn explore_body(ui: Ui, start: ExploreStart) -> impl IntoView {
    let chips = ep::organs(&start.all);
    let all = StoredValue::new(start.all);
    let mu = MapUi {
        data: RwSignal::new((start.map, start.frame)),
        frame: RwSignal::new(start.frame),
        view: RwSignal::new((1.0, 0.0, 0.0)),
        settled: RwSignal::new((1.0, 0.0, 0.0)),
        unit: RwSignal::new(1.0),
        until: RwSignal::new(None),
        playing: RwSignal::new(false),
        busy: RwSignal::new(false),
        error: RwSignal::new(None),
        retry: RwSignal::new(0),
        drag: StoredValue::new(Drag::default()),
        timer: StoredValue::new_local(None),
        node: NodeRef::new(),
    };
    // The list under a filter; the whole map needs no second fetch.
    let picked = Resource::new(
        move || filter_of(ui),
        |f| async move {
            if f == MapFilter::default() {
                Ok(None)
            } else {
                get_list(f).await.map(Some)
            }
        },
    );
    // In the browser: the frame this window wants, then a new map whenever
    // the filter or the frame changes (laid out by the server, never here).
    // The frame follows the window across the breakpoint (the CSS's too).
    Effect::new(move |_| {
        let fit = move || {
            if let Some(w) = window_width() {
                let f = ep::frame_for(w);
                if mu.frame.try_get_untracked() != Some(f) {
                    let _ = mu.frame.try_set(f);
                }
            }
            mu.measure();
        };
        fit();
        #[cfg(feature = "hydrate")]
        {
            let h = window_event_listener(leptos::ev::resize, move |_| fit());
            on_cleanup(move || h.remove());
        }
    });
    // The map on screen, and the one being fetched: `shown` moves only
    // when a fetch succeeds, so a failed one can be asked again ("Try
    // again" bumps `retry`).
    let shown = StoredValue::new((MapFilter::default(), start.frame, 0u32));
    let asking: StoredValue<Option<MapKey>> = StoredValue::new(None);
    Effect::new(move |_| {
        let key = (filter_of(ui), mu.frame.get(), mu.retry.get());
        let same = |k: &MapKey| k.0 == key.0 && k.1 == key.1;
        let in_flight = asking.with_value(|a| a.as_ref().map(|a| *a == key));
        match ep::map_fetch(shown.with_value(same), in_flight) {
            ep::MapFetch::Nothing => return,
            // Back to the map on screen while another loads: forget that
            // one, so its reply is not drawn under this filter.
            ep::MapFetch::Drop => {
                asking.set_value(None);
                mu.busy.set(false);
                return;
            }
            ep::MapFetch::Ask => {}
        }
        asking.set_value(Some(key.clone()));
        mu.busy.set(true);
        leptos::task::spawn_local(async move {
            let (f, (w, h), _) = key.clone();
            let r = get_map(f, w, h).await;
            // A newer request has taken over.
            if asking
                .try_with_value(|a| a.as_ref() != Some(&key))
                .unwrap_or(true)
            {
                return;
            }
            asking.set_value(None);
            let _ = mu.busy.try_set(false);
            match r {
                Ok(m) => {
                    shown.set_value(key);
                    let _ = mu.data.try_set((m, (w, h)));
                    let _ = mu.error.try_set(None);
                }
                Err(e) => {
                    let _ = mu.error.try_set(Some(error_text(&e)));
                }
            }
        });
    });
    let set_view = move |v: View| {
        mu.stop();
        mu.until.set(None);
        ui.view.set(v);
        save_view(v);
    };
    let pick_organ = move |o: Option<i64>| {
        ui.organ.set(o);
        ui.sel.set(None);
        mu.place((1.0, 0.0, 0.0));
    };
    let chip_views = chips
        .into_iter()
        .map(|(id, name)| {
            view! {
                <button
                    type="button"
                    aria-pressed=move || (ui.organ.get() == Some(id)).to_string()
                    on:click=move |_| pick_organ(Some(id))
                >
                    {name}
                </button>
            }
        })
        .collect_view();
    let data = move || -> Option<Result<MapData, ServerFnError>> {
        if filter_of(ui) == MapFilter::default() {
            return Some(Ok(all.get_value()));
        }
        match picked.get()? {
            Ok(Some(d)) => Some(Ok(d)),
            // Still the whole map's answer: the filtered list is on its way.
            Ok(None) => None,
            Err(e) => Some(Err(e)),
        }
    };
    view! {
        <div role="group" aria-label="View" class="kg-switch" hidden=move || !ui.ready.get()>
            <button type="button" aria-pressed=move || (ui.view.get() == View::Map).to_string() on:click=move |_| set_view(View::Map)>
                "Map"
            </button>
            <button type="button" aria-pressed=move || (ui.view.get() == View::List).to_string() on:click=move |_| set_view(View::List)>
                "List"
            </button>
        </div>
        <div class="kg-body">
            <section class="kg-main" aria-label="Map or list">
                <div class="kg-find">
                    <label for="kg-find">{ep::FIND_LABEL}</label>
                    <input
                        id="kg-find"
                        type="text"
                        placeholder=ep::FIND_HINT
                        prop:value=move || ui.query.get()
                        on:input=move |ev| ui.query.set(event_target_value(&ev))
                    />
                </div>
                <div role="group" aria-label="Organ" class="kg-chips">
                    <button type="button" aria-pressed=move || ui.organ.get().is_none().to_string() on:click=move |_| pick_organ(None)>
                        "All"
                    </button>
                    {chip_views}
                </div>
                <div id="kg-map" class="kg-map" hidden=move || ui.view.get() != View::Map>
                    {map_view(ui, mu)}
                </div>
                <div class="kg-list" hidden=move || ui.view.get() != View::List>
                    // No Suspense: the whole map is already here, so the list
                    // is in the first HTML; a filtered list loads in the
                    // browser.
                    {move || match data() {
                        Some(Ok(d)) => list_view(ui, d).into_any(),
                        Some(Err(e)) => view! { <p class="box amber" role="alert">{error_text(&e)}</p> }.into_any(),
                        None => view! { <p class="iv-status">"Loading\u{2026}"</p> }.into_any(),
                    }}
                </div>
            </section>
            <section class="kg-side" aria-label="Details and filters">
                <div class="kg-side-card">
                    {move || {
                        let d = data().and_then(Result::ok).unwrap_or_default();
                        let sel = ui.sel.get();
                        let card = sel
                            .as_deref()
                            .and_then(|id| ep::card_for(&d, id))
                            .unwrap_or_else(ep::default_card);
                        let papers = sel
                            .as_deref()
                            .map(|id| side_papers(ui, &d, id, mu.until.get()));
                        view! { {card_view(ui, card, false)} {papers} }
                    }}
                </div>
                {legend_view(ui)}
                <p class="kg-note">{ep::NOT_ALL} " " {ep::LINE_MEANS} " " {ep::SQUARES}</p>
            </section>
        </div>
    }
}

/// A coordinate as a share of the frame, for CSS.
fn pct(v: f32, of: f32) -> String {
    format!("{:.3}%", if of > 0.0 { v / of * 100.0 } else { 0.0 })
}

/// The map: lines, dots and labels on one layer that zoom and pan move
/// with a CSS transform (a move changes one style), the zoom buttons, the
/// time-lapse and the counts. Labels are worked out again only when the
/// map comes to rest.
fn map_view(ui: Ui, mu: MapUi) -> AnyView {
    use std::collections::{HashMap, HashSet};
    // What shows and what is lit.
    let live = Memo::new(move |_| mu.data.with(|(m, _)| ep::visible_until(m, mu.until.get())));
    let sel = Memo::new(move |_| ui.sel.get().filter(|s| live.with(|l| l.contains(s))));
    let near = Memo::new(move |_| {
        sel.get()
            .map(|s| mu.data.with(|(m, _)| ep::near(m, &s)))
            .unwrap_or_default()
    });
    let searching = Memo::new(move |_| !ui.query.with(|q| q.trim().is_empty()));
    let found = Memo::new(move |_| {
        let q = ui.query.get();
        let l = live.get();
        mu.data.with(|(m, _)| {
            ep::found_on_map(m, &q)
                .into_iter()
                .filter(|id| l.contains(id))
                .collect::<HashSet<String>>()
        })
    });
    let lit = move |id: &str| {
        if sel.with(Option::is_some) {
            near.with(|n| n.contains(id))
        } else if searching.get() {
            found.with(|f| f.contains(id))
        } else {
            true
        }
    };
    let frame = move || mu.data.with(|(_, f)| *f);
    // Where each dot sits at 1x (frame units) and its radius in pixels.
    let base = Memo::new(move |_| {
        mu.data.with(|(m, _)| {
            m.dots
                .iter()
                .map(|d| (d.id.clone(), (d.x, d.y, d.r)))
                .collect::<HashMap<String, (f32, f32, f32)>>()
        })
    });
    // The layer: the one style a drag or a pinch changes.
    let layer_style = move || {
        let (k, tx, ty) = mu.view.get();
        let (w, h) = frame();
        format!(
            "transform:translate({},{}) scale({k});--k:{k};--ik:{};--g:{}",
            pct(tx, w),
            pct(ty, h),
            1.0 / k,
            ep::zoom_radius(1.0, k)
        )
    };

    let lines = move || {
        let (w, h) = frame();
        let s = sel.get();
        let focusing = s.is_some() || searching.get();
        let l = live.get();
        let views = mu.data.with(|(m, _)| {
            let at: HashMap<&str, (f32, f32)> =
                m.dots.iter().map(|d| (d.id.as_str(), (d.x, d.y))).collect();
            m.lines
                .iter()
                .filter(|x| l.contains(&x.a) && l.contains(&x.b))
                .filter_map(|x| {
                    let (a, b) = (at.get(x.a.as_str())?, at.get(x.b.as_str())?);
                    let on = match s.as_deref() {
                        Some(s) => x.a == s || x.b == s,
                        None => !focusing,
                    };
                    let class = if on && s.is_some() {
                        "kg-ln on"
                    } else {
                        "kg-ln"
                    };
                    Some(view! {
                        <line
                            class=class
                            x1=format!("{:.1}", a.0)
                            y1=format!("{:.1}", a.1)
                            x2=format!("{:.1}", b.0)
                            y2=format!("{:.1}", b.1)
                            stroke-width=if x.paper { "1" } else { "1.6" }
                            stroke-dasharray=x.paper.then_some("4 3")
                            vector-effect="non-scaling-stroke"
                            opacity=if on { "1" } else { "0.18" }
                        ></line>
                    })
                })
                .collect::<Vec<_>>()
        });
        view! {
            <svg class="kg-svg" viewBox=format!("0 0 {w} {h}") preserveAspectRatio="none" aria-hidden="true">
                {views}
            </svg>
        }
    };

    // One real button per dot, kept across redraws (keyboard focus stays):
    // its name and mark follow the map (a refetch can change a paper's
    // rank), as does where it sits; its size follows the zoom through the
    // layer's CSS variables.
    let dot_button = move |id: String| {
        let info = {
            let id = id.clone();
            Memo::new(move |_| {
                mu.data.with(|(m, _)| {
                    let d = m.dots.iter().find(|d| d.id == id)?;
                    let mark = if d.kind == DotKind::Paper {
                        format!("kg-mk sq {}", ep::rank_class(d.rank.as_deref()))
                    } else {
                        format!("kg-mk {}", d.kind.as_str())
                    };
                    Some((ep::dot_aria(m, d), mark))
                })
            })
        };
        if info.with_untracked(Option::is_none) {
            return ().into_any();
        }
        // A dot that leaves the map is removed by the list.
        let aria = move || info.with(|i| i.as_ref().map(|i| i.0.clone()));
        let mark = move || info.with(|i| i.as_ref().map(|i| i.1.clone()));
        let on = {
            let id = id.clone();
            Memo::new(move |_| sel.with(|s| s.as_deref() == Some(id.as_str())))
        };
        let style = {
            let id = id.clone();
            move || {
                let (w, h) = frame();
                base.with(|b| b.get(&id).copied())
                    .map_or_else(String::new, |(x, y, r)| {
                        format!("left:{};top:{};--d:{:.1}px", pct(x, w), pct(y, h), r * 2.0)
                    })
            }
        };
        let mark_style = {
            let id = id.clone();
            move || format!("opacity:{}", if lit(&id) { "1" } else { "0.16" })
        };
        let pick = id.clone();
        view! {
            <button
                type="button"
                class=move || if on.get() { "kg-dot on" } else { "kg-dot" }
                aria-label=aria
                aria-pressed=move || on.get().to_string()
                style=style
                on:click=move |ev: leptos::ev::MouseEvent| {
                    let moved = mu.drag.try_update_value(|g| std::mem::take(&mut g.moved)).unwrap_or(false);
                    if ep::is_tap(moved, ev.detail()) {
                        ui.sel.update(|s| {
                            *s = (s.as_deref() != Some(pick.as_str())).then(|| pick.clone());
                        });
                    }
                }
            >
                <span class=mark style=mark_style></span>
            </button>
        }
        .into_any()
    };
    let dot_views = view! {
        <For
            each=move || {
                let l = live.get();
                mu.data.with(|(m, _)| {
                    m.dots
                        .iter()
                        .filter(|d| l.contains(&d.id))
                        .map(|d| d.id.clone())
                        .collect::<Vec<_>>()
                })
            }
            key=|id| id.clone()
            children=dot_button
        />
    };

    // Labels for the map at rest, drawn on the layer at their dot's 1x
    // place and kept at 12px whatever the zoom.
    let labels = move || {
        let (k, tx, ty) = mu.settled.get();
        let fr = frame();
        let unit = mu.unit.get();
        let s = sel.get();
        let l = live.get();
        let spots: Vec<ep::Spot> = mu.data.with(|(m, _)| {
            m.dots
                .iter()
                .filter(|d| l.contains(&d.id))
                .map(|d| ep::Spot::of(d, k, tx, ty))
                .collect()
        });
        let boxes = near
            .with(|n| found.with(|f| ep::visible_labels(&spots, k, s.as_deref(), n, f, fr, unit)));
        boxes
            .into_iter()
            .map(|b| {
                let style = format!(
                    "left:{};top:{};max-width:{:.0}px;opacity:{}",
                    pct((b.x - tx) / k, fr.0),
                    pct((b.y - ty) / k, fr.1),
                    b.w / unit + 10.0,
                    if lit(&b.id) { "1" } else { "0.3" }
                );
                view! {
                    <div class=if b.bold { "kg-lab b" } else { "kg-lab" } style=style aria-hidden="true">
                        {b.text}
                    </div>
                }
            })
            .collect_view()
    };

    // Pointers: drag to move, pinch to zoom, on the frame only. A pointer
    // is captured by what it went down on, as on the board.
    let to_frame = move |x: i32, y: i32| -> Option<(f32, f32)> {
        #[cfg(feature = "hydrate")]
        {
            let el = mu.node.get_untracked()?;
            let rect = el.get_bounding_client_rect();
            let (w, h) = mu.frame_now();
            if rect.width() <= 0.0 || rect.height() <= 0.0 {
                return None;
            }
            Some((
                ((f64::from(x) - rect.left()) * f64::from(w) / rect.width()) as f32,
                ((f64::from(y) - rect.top()) * f64::from(h) / rect.height()) as f32,
            ))
        }
        #[cfg(not(feature = "hydrate"))]
        {
            let _ = (x, y);
            None
        }
    };
    let two = |p: &[(i32, f32, f32)]| {
        let (a, b) = (p[0], p[1]);
        let d = ((a.1 - b.1).powi(2) + (a.2 - b.2).powi(2)).sqrt().max(1.0);
        (d, ((a.1 + b.1) / 2.0, (a.2 + b.2) / 2.0))
    };
    let down = move |ev: leptos::ev::PointerEvent| {
        let Some((x, y)) = to_frame(ev.client_x(), ev.client_y()) else {
            return;
        };
        let id = ev.pointer_id();
        // A primary pointer starts a new gesture: any pointer still listed
        // was lost (its up never came) and must not turn this into a pinch.
        if ev.is_primary() {
            mu.drag.update_value(|g| g.pts.clear());
        }
        #[cfg(feature = "hydrate")]
        {
            use wasm_bindgen::JsCast;
            if let Some(t) = ev
                .target()
                .and_then(|t| t.dyn_into::<web_sys::Element>().ok())
            {
                let _ = t.set_pointer_capture(id);
            }
        }
        let (k, tx, ty) = mu.view.get_untracked();
        mu.drag.update_value(|g| {
            g.pts.retain(|p| p.0 != id);
            g.pts.push((id, x, y));
            match g.pts.len() {
                1 => {
                    g.moved = false;
                    g.start = Some((x, y, tx, ty));
                    g.pinch = None;
                }
                2 => {
                    let (d, mid) = two(&g.pts);
                    g.pinch = Some((d, mid, k, tx, ty));
                    g.start = None;
                }
                _ => {}
            }
        });
    };
    let moving = move |ev: leptos::ev::PointerEvent| {
        let id = ev.pointer_id();
        if !mu.drag.with_value(|g| g.pts.iter().any(|p| p.0 == id)) {
            return;
        }
        let Some((x, y)) = to_frame(ev.client_x(), ev.client_y()) else {
            return;
        };
        let next = mu.drag.try_update_value(|g| {
            for p in &mut g.pts {
                if p.0 == id {
                    p.1 = x;
                    p.2 = y;
                }
            }
            if let Some((d0, mid0, k0, tx0, ty0)) = g.pinch
                && g.pts.len() >= 2
            {
                let (d, mid) = two(&g.pts);
                let k = (k0 * d / d0).clamp(ep::ZOOM_MIN, ep::ZOOM_MAX);
                // The point under the fingers stays under them.
                let (wx, wy) = ((mid0.0 - tx0) / k0, (mid0.1 - ty0) / k0);
                g.moved = true;
                return Some((k, mid.0 - wx * k, mid.1 - wy * k));
            }
            let (sx, sy, tx0, ty0) = g.start?;
            let (dx, dy) = (x - sx, y - sy);
            if dx.abs() + dy.abs() > 4.0 {
                g.moved = true;
            }
            g.moved
                .then(|| (mu.view.get_untracked().0, tx0 + dx, ty0 + dy))
        });
        if let Some(Some(v)) = next {
            mu.view.set(v);
        }
    };
    let up = move |ev: leptos::ev::PointerEvent| {
        let id = ev.pointer_id();
        let (_, tx, ty) = mu.view.get_untracked();
        let rest = mu
            .drag
            .try_update_value(|g| {
                g.pts.retain(|p| p.0 != id);
                g.pinch = None;
                // A finger left after a pinch carries on as a drag.
                g.start = g.pts.first().map(|p| (p.1, p.2, tx, ty));
                g.pts.is_empty()
            })
            .unwrap_or(true);
        // The labels catch up once the map stops (and after a pinch).
        if rest || mu.drag.with_value(|g| g.pts.len() == 1) {
            mu.settle();
        }
    };

    // The time-lapse: a year a step, by paper year, in the browser.
    let span = move || mu.data.with(|(m, _)| ep::year_span(m));
    let play = move |_| {
        if mu.playing.get_untracked() {
            mu.stop();
            return;
        }
        let Some((first, last)) = span() else {
            return;
        };
        let from = match mu.until.get_untracked() {
            Some(y) if y < last => y,
            _ => first,
        };
        mu.until.set(Some(from));
        mu.playing.set(true);
        ui.sel.set(None);
        let tick = move || {
            let next = mu.until.get_untracked().map_or(first, |y| y + 1);
            if next > last {
                mu.stop();
                let _ = mu.until.try_set(Some(last));
            } else {
                let _ = mu.until.try_set(Some(next));
            }
        };
        let h = leptos::leptos_dom::helpers::set_interval_with_handle(
            tick,
            std::time::Duration::from_millis(800),
        );
        mu.timer.set_value(h.ok());
    };
    on_cleanup(move || mu.stop());
    // How big the frame is drawn, once it shows and whenever the map or the
    // view changes. On a switch to Map the frame may still be hidden (width
    // 0, skipped) when this runs: measured again on the next frame.
    Effect::new(move |_| {
        mu.data.track();
        let v = ui.view.get();
        mu.measure();
        #[cfg(feature = "hydrate")]
        if v == View::Map {
            request_animation_frame(move || mu.measure());
        }
        #[cfg(not(feature = "hydrate"))]
        let _ = v;
    });

    view! {
        <div
            class="kg-map-frame"
            node_ref=mu.node
            style=move || {
                let (w, h) = frame();
                format!("aspect-ratio:{w} / {h}")
            }
            on:pointerdown=down
            on:pointermove=moving
            on:pointerup=up
            on:pointercancel=up
            on:lostpointercapture=up
            on:keydown=move |ev: leptos::ev::KeyboardEvent| {
                if ev.key() == "Escape" {
                    ui.sel.set(None);
                }
            }
        >
            // A pointer surface, not a control: out of the tab order (Escape
            // clears the selection from the keyboard).
            <button
                type="button"
                class="kg-bg"
                tabindex="-1"
                aria-label="Clear selection and drag to move the map"
                on:click=move |ev: leptos::ev::MouseEvent| {
                    let moved = mu.drag.try_update_value(|g| std::mem::take(&mut g.moved)).unwrap_or(false);
                    if ep::is_tap(moved, ev.detail()) {
                        ui.sel.set(None);
                    }
                }
            ></button>
            <div class="kg-layer" style=layer_style>
                {lines}
                {dot_views}
                {labels}
            </div>
            <div class="kg-zoom">
                <button type="button" aria-label="Zoom in" on:click=move |_| mu.zoom(ep::ZOOM_STEP)>"+"</button>
                <button type="button" aria-label="Zoom out" on:click=move |_| mu.zoom(1.0 / ep::ZOOM_STEP)>"\u{2212}"</button>
                <button type="button" class="fit" aria-label="Whole map" on:click=move |_| mu.place((1.0, 0.0, 0.0))>"Fit"</button>
            </div>
            {move || mu.until.get().map(|y| view! { <div class="kg-year" aria-hidden="true">{y}</div> })}
        </div>
        <div class="kg-foot">
            <button type="button" class="kg-play" on:click=play>
                {move || ep::play_label(mu.playing.get(), mu.until.get(), span().map(|s| s.1))}
            </button>
            <span>
                {move || {
                    let l = live.get();
                    mu.data.with(|(m, _)| {
                        let lines = m
                            .lines
                            .iter()
                            .filter(|x| l.contains(&x.a) && l.contains(&x.b))
                            .count();
                        ep::count_words(l.len(), lines)
                    })
                }}
            </span>
            {move || mu.data.with(|(m, _)| ep::map_note(m)).map(|t| view! { <span>{t}</span> })}
            {move || mu.busy.get().then(|| view! { <span class="iv-status">"Loading your map\u{2026}"</span> })}
        </div>
        {move || {
            mu.error.get().map(|e| {
                view! {
                    <p class="box amber" role="alert">
                        {e} " "
                        <button type="button" class="kg-more" on:click=move |_| mu.retry.update(|r| *r += 1)>
                            "Try again"
                        </button>
                    </p>
                }
            })
        }}
    }
    .into_any()
}

/// The picked question's or disease's papers, ranked, folded at five.
fn side_papers(ui: Ui, d: &MapData, id: &str, until: Option<i32>) -> Option<AnyView> {
    let live = ep::visible_until(d, until);
    let rows: Vec<Row> = ep::papers_for(d, id)
        .into_iter()
        .filter(|r| live.contains(&r.id))
        .collect();
    if rows.is_empty() {
        return None;
    }
    const KEY: &str = "side papers";
    let open = ui.open.with(|o| o.contains(KEY));
    let total = rows.len();
    let (rows, button) = ep::fold(&rows, open);
    let items = rows
        .into_iter()
        .map(|r| {
            let dot = d.dots.iter().find(|x| x.id == r.id);
            let (_, num, _) = ep::rank_info(r.rank.as_deref());
            let class = format!("kg-num {}", ep::rank_class(r.rank.as_deref()));
            let sub = dot
                .map(|x| {
                    let mut parts: Vec<String> = Vec::new();
                    if !x.design.trim().is_empty() {
                        parts.push(x.design.trim().to_string());
                    }
                    if let Some(y) = x.first_year {
                        parts.push(y.to_string());
                    }
                    if x.retracted {
                        parts.push("Retracted".into());
                    }
                    parts.join(" \u{b7} ")
                })
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| r.sub.clone());
            let id = r.id.clone();
            view! {
                <button type="button" class="kg-paper" on:click=move |_| ui.sel.set(Some(id.clone()))>
                    <span class=class aria-hidden="true">{num}</span>
                    <span class="kg-row-text">
                        <span class="kg-row-title">{r.title}</span>
                        <span class="kg-row-sub">{sub}</span>
                    </span>
                </button>
            }
        })
        .collect_view();
    let toggle = move |_| {
        ui.open.update(|o| {
            if !o.remove(KEY) {
                o.insert(KEY.to_string());
            }
        })
    };
    Some(
        view! {
            <div class="kg-papers">
                <h3>{format!("Papers, ranked \u{b7} {total}")}</h3>
                {items}
                {button.map(|label| view! {
                    <button type="button" class="kg-more" aria-expanded=open.to_string() on:click=toggle>
                        {label}
                    </button>
                })}
            </div>
        }
        .into_any(),
    )
}

/// "Show on the map": the kinds and ranks to show, the paper years and the
/// run certainty (each refetches the map, laid out by the server).
fn legend_view(ui: Ui) -> AnyView {
    let keys = ep::LEGEND
        .iter()
        .map(|&(key, name, class, square)| {
            let mark = if square {
                format!("kg-mk sq {class}")
            } else {
                format!("kg-mk {class}")
            };
            view! {
                <button
                    type="button"
                    aria-pressed=move || (!ui.hidden.with(|h| h.contains(key))).to_string()
                    on:click=move |_| {
                        ui.hidden.update(|h| {
                            if !h.remove(key) {
                                h.insert(key);
                            }
                        })
                    }
                >
                    <span class=mark style="width:11px;height:11px" aria-hidden="true"></span>
                    {name}
                </button>
            }
        })
        .collect_view();
    let year_options = ep::YEARS
        .iter()
        .map(|&(v, label, _, _)| view! { <option value=v selected=move || ui.years.get() == v>{label}</option> })
        .collect_view();
    let cert_options = ep::CERTAINTIES
        .iter()
        .map(|&(v, label)| view! { <option value=v selected=move || ui.certainty.get() == v>{label}</option> })
        .collect_view();
    view! {
        <div class="kg-legend">
            <button
                type="button"
                class="kg-legend-btn"
                aria-expanded=move || ui.legend_open.get().to_string()
                on:click=move |_| ui.legend_open.update(|o| *o = !*o)
            >
                <span>
                    "Show on the map"
                    {move || {
                        let n = ui.hidden.with(BTreeSet::len);
                        (n > 0).then(|| format!(" \u{b7} {n} hidden"))
                    }}
                </span>
                <span aria-hidden="true">{move || if ui.legend_open.get() { "\u{25be}" } else { "\u{25b8}" }}</span>
            </button>
            <div class="kg-legend-body" hidden=move || !ui.legend_open.get()>
                <div class="kg-keys">{keys}</div>
                <div class="kg-selects">
                    <div>
                        <label for="kg-year">"Paper year"</label>
                        <select
                            id="kg-year"
                            on:change=move |ev| {
                                let v = event_target_value(&ev);
                                if let Some(y) = ep::YEARS.iter().find(|y| y.0 == v) {
                                    ui.years.set(y.0);
                                }
                            }
                        >
                            {year_options}
                        </select>
                    </div>
                    <div>
                        <label for="kg-cert">"Run certainty"</label>
                        <select
                            id="kg-cert"
                            on:change=move |ev| {
                                let v = event_target_value(&ev);
                                if let Some(c) = ep::CERTAINTIES.iter().find(|c| c.0 == v) {
                                    ui.certainty.set(c.0);
                                }
                            }
                        >
                            {cert_options}
                        </select>
                    </div>
                </div>
            </div>
        </div>
    }
    .into_any()
}

/// The groups that match the find box, each folded at five rows.
fn list_view(ui: Ui, data: MapData) -> impl IntoView {
    let groups = StoredValue::new(ep::groups(&data));
    let data = StoredValue::new(data);
    move || {
        let found = groups.with_value(|g| ep::find_in(g, &ui.query.get()));
        if found.is_empty() {
            return view! { <p class="kg-nomatch">{ep::NO_MATCH}</p> }.into_any();
        }
        let sel = ui.sel.get();
        let open = ui.open.get();
        let views = found
            .into_iter()
            .map(|g| data.with_value(|d| group_view(ui, d, g, sel.as_deref(), &open)))
            .collect_view();
        view! { <div class="kg-groups">{views}</div> }.into_any()
    }
}

fn group_view(
    ui: Ui,
    data: &MapData,
    g: Group,
    sel: Option<&str>,
    open: &BTreeSet<String>,
) -> AnyView {
    let is_open = open.contains(&g.name);
    let (rows, button) = ep::fold(&g.rows, is_open);
    let heading = ep::group_heading(&g, g.rows.len());
    let rows = rows
        .into_iter()
        .map(|r| {
            let on = sel == Some(r.id.as_str());
            let card = on.then(|| ep::card_for(data, &r.id)).flatten();
            row_view(ui, r, on, card)
        })
        .collect_view();
    let name = g.name.clone();
    let toggle = move |_| {
        ui.open.update(|o| {
            if !o.remove(&name) {
                o.insert(name.clone());
            }
        })
    };
    view! {
        <div class="kg-group">
            <h2>{heading}</h2>
            {rows}
            {button.map(|label| view! {
                <button type="button" class="kg-more" aria-expanded=is_open.to_string() on:click=toggle>
                    {label}
                </button>
            })}
        </div>
    }
    .into_any()
}

/// A row's colour: its kind's dot, or a paper's rank square.
fn swatch_class(r: &Row) -> String {
    if r.kind == DotKind::Paper {
        const RANKED: [&str; 5] = ["rct", "meta", "prospective", "retrospective", "case_series"];
        let rank = r
            .rank
            .as_deref()
            .filter(|c| RANKED.contains(c))
            .unwrap_or("other");
        format!("kg-sw sq {rank}")
    } else {
        format!("kg-sw {}", r.kind.as_str())
    }
}

fn row_view(ui: Ui, r: Row, on: bool, card: Option<Card>) -> AnyView {
    let id = r.id.clone();
    let swatch = swatch_class(&r);
    view! {
        <div class="kg-row" class:on=on>
            <button
                type="button"
                class="kg-row-btn"
                aria-expanded=on.to_string()
                on:click=move |_| ui.sel.set((!on).then(|| id.clone()))
            >
                <span class=swatch aria-hidden="true"></span>
                <span class="kg-row-text">
                    <span class="kg-row-title">{r.title}</span>
                    <span class="kg-row-sub">{r.sub}</span>
                </span>
            </button>
            {card.map(|c| card_view(ui, c, true))}
        </div>
    }
    .into_any()
}

/// A card: in place under its row (`inline`, phones) or in the side column.
fn card_view(ui: Ui, c: Card, inline: bool) -> AnyView {
    let class = if inline {
        "kg-card kg-inline"
    } else {
        "kg-card"
    };
    let flags = c
        .flags
        .into_iter()
        .map(|f| {
            let class = if f == "Retracted" {
                "kg-flag alert"
            } else {
                "kg-flag"
            };
            view! { <p class=class>{f}</p> }
        })
        .collect_view();
    let links = (!inline && !c.links.is_empty()).then(|| {
        let buttons = c
            .links
            .into_iter()
            .map(|(id, label)| {
                view! {
                    <button type="button" on:click=move |_| ui.sel.set(Some(id.clone()))>
                        {label}
                    </button>
                }
            })
            .collect_view();
        view! { <div class="kg-links">{buttons}</div> }
    });
    view! {
        <div class=class>
            <div class="kg-kicker">{c.kicker}</div>
            {(!inline).then(|| view! { <div class="kg-card-title">{c.title}</div> })}
            <div class="kg-card-body">{c.body}</div>
            {flags}
            {links}
            {c.answer.map(|href| view! { <a class="kg-act" href=href>"Open the full answer"</a> })}
            {c.sheet.map(|href| view! { <a class="kg-act" href=href rel="external">"Open the study sheet"</a> })}
            {c.pubmed.map(|href| view! { <a class="kg-act alt" href=href rel="external">"Open the paper"</a> })}
            {c.ask.map(|href| view! { <a class="kg-act alt" href=href>"Ask about this"</a> })}
        </div>
    }
    .into_any()
}
