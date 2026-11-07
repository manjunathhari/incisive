//! The Explore page (`/explore`, `GRAPH=on`): the user's own map as a list
//! of diseases, treatments, outcomes, questions and ranked papers, each
//! group folded at five rows, with a card for the picked row (in place on a
//! phone, at the side on a wide screen), and the map: the dots the server
//! laid out once, drawn as real buttons over SVG lines, with tap to light up
//! a dot's neighbours, zoom (sharp: every place is worked out again, nothing
//! is scaled), pan, drag a dot, find, full screen, the "Show on the map"
//! filters and a time-lapse by paper year (in the browser, with no new
//! layout).
//!
//! The page opens on the Map. The server draws the switch, the frame
//! ("Drawing your map…" until the page runs in the browser) and the List,
//! hidden. Every fetch goes through the server functions here, which
//! use the signed-in user and know nothing with `GRAPH` off. Labels are
//! text, never markup; nothing here logs a name or a question.

use std::collections::{BTreeSet, HashMap};

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
.kg-layer { position:absolute; left:0; top:0; width:100%; height:100%; pointer-events:none; }
.kg-drawing { position:absolute; left:0; top:0; width:100%; height:100%; display:flex; align-items:center; justify-content:center; font-size:14px; color:var(--iv-muted); pointer-events:none; }
.kg-svg { position:absolute; left:0; top:0; width:100%; height:100%; pointer-events:none; overflow:visible; }
.kg-ln { stroke:var(--kg-other); } .kg-ln.on { stroke:var(--iv-body); }
.iv-wrap button.kg-dot { position:absolute; transform:translate(-50%,-50%); padding:0; border:0; background:transparent; display:flex; align-items:center; justify-content:center; cursor:pointer; border-radius:50%; pointer-events:auto; touch-action:none; }
.kg-mk { display:block; box-sizing:border-box; border-radius:50%; }
.kg-dot .kg-mk { width:var(--m); height:var(--m); }
.kg-mk.sq { border-radius:2px; }
.iv-wrap button.kg-dot.on .kg-mk { border:2px solid var(--iv-ink); }
.kg-mk.organ { background:var(--kg-organ); } .kg-mk.disease { background:var(--kg-disease); } .kg-mk.run { background:var(--kg-run); }
.kg-mk.treatment { background:var(--kg-treatment); } .kg-mk.outcome { background:var(--kg-outcome); }
.kg-mk.rct, .kg-num.rct { background:var(--kg-rct); } .kg-mk.meta, .kg-num.meta { background:var(--kg-meta); } .kg-mk.prospective, .kg-num.prospective { background:var(--kg-pro); }
.kg-mk.retrospective, .kg-num.retrospective { background:var(--kg-retro); } .kg-mk.case_series, .kg-num.case_series { background:var(--kg-series); } .kg-mk.other, .kg-num.other { background:var(--kg-other); }
.kg-lab { position:absolute; transform:translateX(-50%); pointer-events:none; font-size:11px; line-height:14px; white-space:nowrap; background:rgba(255,255,255,0.86); border-radius:4px; padding:0 3px; color:var(--iv-ink); }
.kg-lab.big { font-size:13px; line-height:16px; }
.kg-lab.b { font-weight:600; }
.iv-wrap button.kg-openfull { align-self:stretch; width:100%; min-height:46px; padding:0 16px; border-radius:999px; border:1px solid var(--iv-accent); background:var(--iv-card); color:var(--iv-accent); font-family:inherit; font-size:14.5px; font-weight:600; cursor:pointer; }
.kg-full-head { display:flex; align-items:center; gap:10px; }
.kg-full-head .kg-full-title { flex:1; font-size:16px; font-weight:600; color:var(--iv-ink); }
.iv-wrap button.kg-close { flex:none; width:44px; height:44px; padding:0; border-radius:50%; border:1px solid var(--iv-border); background:var(--iv-card); color:var(--iv-ink); font-family:inherit; font-size:22px; cursor:pointer; }
.kg-map.kg-full { position:fixed; inset:0; z-index:50; box-sizing:border-box; background:var(--iv-ground);
  padding:calc(12px + env(safe-area-inset-top, 0px)) max(12px, env(safe-area-inset-right, 0px)) calc(12px + env(safe-area-inset-bottom, 0px)) max(12px, env(safe-area-inset-left, 0px)); }
.kg-full .kg-map-frame { flex:1 1 auto; min-height:0; }
.kg-card.kg-float { position:absolute; left:max(20px, env(safe-area-inset-left, 0px)); right:max(20px, env(safe-area-inset-right, 0px)); bottom:calc(20px + env(safe-area-inset-bottom, 0px)); z-index:2; max-width:420px; max-height:55%; overflow:auto; margin:0 auto; box-shadow:0 8px 28px rgba(0,0,0,0.18); }
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
.kg-kicker { display:flex; flex-wrap:wrap; align-items:center; gap:8px; font-size:11px; font-weight:600; color:var(--iv-muted); }
.kg-chip { font-size:11.5px; font-weight:700; border-radius:5px; padding:1px 7px; color:var(--iv-ink); }
.kg-chip.rct { background:var(--kg-rct); color:#fff; } .kg-chip.meta { background:var(--kg-meta); color:#fff; } .kg-chip.prospective { background:var(--kg-pro); }
.kg-chip.retrospective { background:var(--kg-retro); } .kg-chip.case_series { background:var(--kg-series); } .kg-chip.other { background:var(--kg-other); }
.kg-from { margin:0; font-size:13px; line-height:1.45; color:var(--iv-body); background:var(--iv-soft); border-radius:10px; padding:8px 10px; overflow-wrap:anywhere; }
.iv-wrap button.kg-from-q { display:inline-flex; align-items:center; min-height:44px; padding:0 2px; border:0; background:none; font:inherit; color:var(--iv-accent); text-decoration:underline; text-underline-offset:2px; text-align:left; cursor:pointer; }
.kg-acts { display:flex; flex-wrap:wrap; gap:8px; }
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

/// The page's state, shared by its parts.
#[derive(Clone, Copy)]
struct Ui {
    view: RwSignal<View>,
    organ: RwSignal<Option<i64>>,
    query: RwSignal<String>,
    sel: RwSignal<Option<String>>,
    open: RwSignal<BTreeSet<String>>,
    /// The page runs in the browser (hydrated): the map is drawn and the
    /// switch works.
    ready: RwSignal<bool>,
    /// "Show on the map": the legend keys switched off, the paper years and
    /// the run certainty.
    hidden: RwSignal<BTreeSet<&'static str>>,
    years: RwSignal<&'static str>,
    certainty: RwSignal<&'static str>,
    legend_open: RwSignal<bool>,
}

impl Ui {
    /// The page as it opens: on the Map, nothing picked.
    fn new() -> Ui {
        Ui {
            view: RwSignal::new(View::Map),
            organ: RwSignal::new(None),
            query: RwSignal::new(String::new()),
            sel: RwSignal::new(None),
            open: RwSignal::new(BTreeSet::new()),
            ready: RwSignal::new(false),
            hidden: RwSignal::new(BTreeSet::new()),
            years: RwSignal::new("all"),
            certainty: RwSignal::new("all"),
            legend_open: RwSignal::new(false),
        }
    }
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

/// A laid-out map and the frame it was laid out for.
type Laid = (MapData, (f32, f32));

/// A pinch's start: the fingers' distance and middle, and the view then.
type Pinch = (f32, (f32, f32), ep::ViewBox);

/// A drag or a pinch on the map, in frame units.
#[derive(Debug, Clone, Default)]
struct Drag {
    /// The pointers down: id and where.
    pts: Vec<(i32, f32, f32)>,
    /// One pointer: where it went down and the pan then.
    start: Option<(f32, f32, f32, f32)>,
    /// The dot it went down on, if any, and that dot's offset then: the
    /// drag moves the dot rather than the map.
    dot: Option<(String, (f32, f32))>,
    /// Two pointers: their distance and middle, and the view then.
    pinch: Option<Pinch>,
    /// The pointer moved far enough to drag, so it was not a tap.
    moved: bool,
}

/// What a pointer move does.
enum Step {
    View(ep::ViewBox),
    Dot(String, (f32, f32)),
}

/// The marker a full screen entry puts in the browser's history.
#[cfg(feature = "hydrate")]
const FULL_MARK: &str = "incisive-explore-map-full";

/// The map's state.
#[derive(Clone, Copy)]
struct MapUi {
    /// The laid-out map and the frame it was laid out for.
    data: RwSignal<Laid>,
    /// The frame this browser wants (its width decides; in full screen,
    /// the frame's measured size).
    frame: RwSignal<(f32, f32)>,
    /// Zoom and pan: every dot, line and label is placed from it.
    view: RwSignal<ep::ViewBox>,
    /// Dots dragged out of place: their offsets in layout units.
    moved: RwSignal<HashMap<String, (f32, f32)>>,
    /// Frame units per screen pixel (the frame is drawn at its box's size).
    unit: RwSignal<f32>,
    /// The map fills the screen.
    full: RwSignal<bool>,
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
    /// The layer the dots are drawn on: the frame, or, while an old layout
    /// waits for its new one, as much of it as fits without stretching.
    layer: NodeRef<leptos::html::Div>,
    /// The full screen dialog.
    dialog: NodeRef<leptos::html::Div>,
    open_btn: NodeRef<leptos::html::Button>,
    close_btn: NodeRef<leptos::html::Button>,
    /// The frame's drawn size in pixels (zero until measured).
    bx: RwSignal<(f32, f32)>,
    /// The page's state (Map or List, the filter).
    ui: Ui,
    /// The map on screen, and the one being fetched: `shown` moves only
    /// when a fetch succeeds, so a failed one can be asked again ("Try
    /// again" bumps `retry`).
    shown: StoredValue<MapKey>,
    asking: StoredValue<Option<MapKey>>,
    /// The map on the page before full screen opened, put back on close.
    before: StoredValue<Option<(MapKey, Laid)>>,
    /// A full screen resize waiting to settle.
    refit: StoredValue<Option<leptos::leptos_dom::helpers::TimeoutHandle>, LocalStorage>,
}

impl MapUi {
    fn frame_now(self) -> (f32, f32) {
        self.data.with_untracked(|(_, f)| *f)
    }

    /// Put every dragged dot back.
    fn unmove(self) {
        if self.moved.try_with_untracked(|m| !m.is_empty()) == Some(true) {
            let _ = self.moved.try_set(HashMap::new());
        }
    }

    /// The whole map: 1x, no pan, every dot in its place.
    fn fit(self) {
        let _ = self.view.try_set((1.0, 0.0, 0.0));
        self.unmove();
    }

    /// + or −: about the frame's centre, at once.
    fn zoom(self, f: f32) {
        let (w, h) = self.frame_now();
        let v = self.view.get_untracked();
        self.view.set(ep::zoom_about(v, (w / 2.0, h / 2.0), f));
    }

    /// How big the frame is drawn: frame units per pixel (browser only).
    /// A hidden frame (width 0) is skipped. Safe once the page is gone
    /// (it may run on a later frame).
    fn measure(self) {
        let Some(b) = self.box_size() else {
            return;
        };
        if self.bx.try_get_untracked().is_some_and(|old| old != b) {
            let _ = self.bx.try_set(b);
        }
        let Some(f) = self.data.try_with_untracked(|(_, f)| *f) else {
            return;
        };
        let (_, _, w, _) = ep::contain(b, f);
        if w > 0.0 {
            let u = f.0 / w;
            if self
                .unit
                .try_get_untracked()
                .is_some_and(|old| (old - u).abs() > 1e-3)
            {
                let _ = self.unit.try_set(u);
            }
        }
    }

    /// The frame this window wants: in full screen the frame's own size
    /// (the server lays the map out again for it), otherwise the phone or
    /// desktop frame for the window's width (browser only).
    fn want_frame(self) {
        let full = self.full.try_get_untracked().unwrap_or(false);
        let f = if full {
            // Laid out again only for a real change of size; a small one
            // (a toolbar) keeps the layout, drawn whole.
            self.box_size().filter(|b| {
                self.frame
                    .try_get_untracked()
                    .is_some_and(|old| ep::refit_needed(old, *b))
            })
        } else {
            window_width().map(ep::frame_for)
        };
        if let Some(f) = f
            && self.frame.try_get_untracked().is_some_and(|old| old != f)
        {
            let _ = self.frame.try_set(f);
        }
        self.measure();
    }

    /// The frame's drawn size in whole pixels (browser only; `None` when
    /// hidden or gone).
    fn box_size(self) -> Option<(f32, f32)> {
        #[cfg(feature = "hydrate")]
        {
            let el = self.node.try_get_untracked().flatten()?;
            let r = el.get_bounding_client_rect();
            (r.width() >= 1.0 && r.height() >= 1.0)
                .then(|| (r.width().round() as f32, r.height().round() as f32))
        }
        #[cfg(not(feature = "hydrate"))]
        None
    }

    /// Open the map full screen: `push` adds a history entry (with
    /// `?map=full`), so Back closes it.
    fn open_full(self, push: bool) {
        if self.full.get_untracked() {
            return;
        }
        // Full screen is the map's: from the List too (Forward into a full
        // screen entry), the time-lapse stopped and every dot shown.
        self.ui.view.set(View::Map);
        self.stop();
        self.until.set(None);
        self.before
            .set_value(Some((self.shown.get_value(), self.data.get_untracked())));
        self.full.set(true);
        self.fit();
        #[cfg(feature = "hydrate")]
        {
            if push {
                set_history(true, true);
            }
            lock_scroll(true);
            request_animation_frame(move || {
                self.want_frame();
                if let Some(b) = self.close_btn.try_get_untracked().flatten() {
                    let _ = b.focus();
                }
            });
        }
        #[cfg(not(feature = "hydrate"))]
        let _ = push;
    }

    /// × or Escape: through the history entry when it is ours (Back then
    /// closes), else at once with `?map=full` taken out of the address.
    fn ask_close(self) {
        if !self.full.get_untracked() {
            return;
        }
        #[cfg(feature = "hydrate")]
        if full_entry() {
            if let Some(h) = web_sys::window().and_then(|w| w.history().ok()) {
                let _ = h.back();
                return;
            }
        } else {
            set_history(false, false);
        }
        self.close_full();
    }

    /// Close the full screen map (Back, ×, Escape) and go back to the
    /// window's frame.
    fn close_full(self) {
        if self.full.try_get_untracked() != Some(true) {
            return;
        }
        let _ = self.full.try_set(false);
        self.cancel_refit();
        // The map the page had, back at once (when the filter is the same):
        // no wait for a new layout and no frame of the full screen's shape.
        let filter = untrack(|| filter_of(self.ui));
        if let Some(Some((key, data))) = self.before.try_update_value(Option::take)
            && key.0 == filter
        {
            self.asking.set_value(None);
            let _ = self.busy.try_set(false);
            self.shown.set_value(key);
            let _ = self.data.try_set(data);
        }
        self.fit();
        #[cfg(feature = "hydrate")]
        {
            lock_scroll(false);
            request_animation_frame(move || {
                self.want_frame();
                if let Some(b) = self.open_btn.try_get_untracked().flatten() {
                    let _ = b.focus();
                }
            });
        }
    }

    /// A full screen resize: the layout waits until the size has settled
    /// (200 ms after the last change); the drawing follows at once
    /// (browser only).
    #[cfg(feature = "hydrate")]
    fn resized(self) {
        if !self.full.get_untracked() {
            request_animation_frame(move || self.want_frame());
            return;
        }
        self.measure();
        self.cancel_refit();
        let h = leptos::leptos_dom::helpers::set_timeout_with_handle(
            move || {
                let _ = self.refit.try_update_value(|h| *h = None);
                self.want_frame();
            },
            std::time::Duration::from_millis(200),
        );
        self.refit.set_value(h.ok());
    }

    fn cancel_refit(self) {
        if let Some(h) = self.refit.try_update_value(Option::take).flatten() {
            h.clear();
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

/// Whether the browser's current history entry is a full screen one this
/// page pushed (browser only).
#[cfg(feature = "hydrate")]
fn full_entry() -> bool {
    web_sys::window()
        .and_then(|w| w.history().ok())
        .and_then(|h| h.state().ok())
        .and_then(|s| s.as_string())
        .is_some_and(|s| s == FULL_MARK)
}

/// The address's query string (browser only).
fn location_search() -> String {
    #[cfg(feature = "hydrate")]
    {
        web_sys::window()
            .and_then(|w| w.location().search().ok())
            .unwrap_or_default()
    }
    #[cfg(not(feature = "hydrate"))]
    String::new()
}

/// Put `?map=full` in the address (`push`: as a new entry carrying the
/// marker) or take it out of the current one. Nothing else in the address
/// changes, and nothing but `map=full` is ever written (browser only).
#[cfg(feature = "hydrate")]
fn set_history(full: bool, push: bool) {
    let Some(w) = web_sys::window() else { return };
    let loc = w.location();
    let (Ok(path), Ok(search), Ok(hash)) = (loc.pathname(), loc.search(), loc.hash()) else {
        return;
    };
    let url = format!("{path}{}{hash}", ep::map_query(&search, full));
    if let Ok(h) = w.history() {
        let _ = if push {
            h.push_state_with_url(&wasm_bindgen::JsValue::from_str(FULL_MARK), "", Some(&url))
        } else {
            h.replace_state_with_url(&wasm_bindgen::JsValue::NULL, "", Some(&url))
        };
    }
}

/// Tab and Shift+Tab stay inside the full screen map: off its last control
/// to its first and back (browser only).
#[cfg(feature = "hydrate")]
fn trap_focus(mu: MapUi, ev: &leptos::ev::KeyboardEvent) {
    use wasm_bindgen::JsCast;
    let Some(dialog) = mu.dialog.try_get_untracked().flatten() else {
        return;
    };
    let Ok(list) = dialog.query_selector_all("button, a[href], input, select") else {
        return;
    };
    // Shown controls in the tab order only.
    let controls: Vec<web_sys::HtmlElement> = (0..list.length())
        .filter_map(|i| list.item(i))
        .filter_map(|n| n.dyn_into::<web_sys::HtmlElement>().ok())
        .filter(|e| e.offset_parent().is_some() && e.tab_index() >= 0)
        .collect();
    let active = web_sys::window()
        .and_then(|w| w.document())
        .and_then(|d| d.active_element());
    let at = active.and_then(|a| {
        controls
            .iter()
            .position(|c| AsRef::<web_sys::Element>::as_ref(c) == &a)
    });
    if let Some(i) = ep::trap_step(controls.len(), at, ev.shift_key()) {
        ev.prevent_default();
        let _ = controls[i].focus();
    }
}

/// Stop (or let) the page scroll under the full screen map (browser only).
#[cfg(feature = "hydrate")]
fn lock_scroll(on: bool) {
    use wasm_bindgen::JsCast;
    let Some(doc) = web_sys::window().and_then(|w| w.document()) else {
        return;
    };
    let value = if on { "hidden" } else { "" };
    if let Some(b) = doc.body() {
        let _ = b.style().set_property("overflow", value);
    }
    if let Some(h) = doc
        .document_element()
        .and_then(|e| e.dyn_into::<web_sys::HtmlElement>().ok())
    {
        let _ = h.style().set_property("overflow", value);
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
    // The page opens on the Map, every time. The server draws the switch
    // (Map pressed) and the frame saying "Drawing your map…"; the map
    // itself shows once the page runs in the browser. The List is in the
    // first HTML, hidden.
    let ui = Ui::new();
    Effect::new(move |_| ui.ready.set(true));
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
        moved: RwSignal::new(HashMap::new()),
        unit: RwSignal::new(1.0),
        full: RwSignal::new(false),
        until: RwSignal::new(None),
        playing: RwSignal::new(false),
        busy: RwSignal::new(false),
        error: RwSignal::new(None),
        retry: RwSignal::new(0),
        drag: StoredValue::new(Drag::default()),
        timer: StoredValue::new_local(None),
        node: NodeRef::new(),
        layer: NodeRef::new(),
        dialog: NodeRef::new(),
        open_btn: NodeRef::new(),
        close_btn: NodeRef::new(),
        bx: RwSignal::new((0.0, 0.0)),
        ui,
        shown: StoredValue::new((MapFilter::default(), start.frame, 0u32)),
        asking: StoredValue::new(None),
        before: StoredValue::new(None),
        refit: StoredValue::new_local(None),
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
    // The frame follows the window across the breakpoint (the CSS's too),
    // and in full screen the frame's own size. Back closes the full screen
    // map, Escape too; an address with `?map=full` opens it.
    Effect::new(move |_| {
        mu.want_frame();
        #[cfg(feature = "hydrate")]
        {
            let resize = window_event_listener(leptos::ev::resize, move |_| mu.resized());
            let back = window_event_listener(leptos::ev::popstate, move |_| {
                if full_entry() {
                    mu.open_full(false);
                } else {
                    mu.close_full();
                }
            });
            let keys = window_event_listener(leptos::ev::keydown, move |ev| {
                if !mu.full.get_untracked() {
                    return;
                }
                match ev.key().as_str() {
                    "Escape" => mu.ask_close(),
                    "Tab" => trap_focus(mu, &ev),
                    _ => {}
                }
            });
            on_cleanup(move || {
                mu.cancel_refit();
                resize.remove();
                back.remove();
                keys.remove();
                // Leaving the page with the map full screen: let it scroll.
                if mu.full.try_get_untracked() == Some(true) {
                    lock_scroll(false);
                }
            });
        }
        if ep::wants_full(&location_search()) {
            mu.open_full(false);
        }
    });
    // Dragged dots go back when the map changes: a new filter or organ, a
    // refetch, or the full screen opening or closing.
    Effect::new(move |first: Option<()>| {
        mu.data.track();
        mu.full.track();
        let _ = filter_of(ui);
        if first.is_some() {
            mu.unmove();
        }
    });
    let (shown, asking) = (mu.shown, mu.asking);
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
                    // A new layout starts whole: a zoom or pan made on the
                    // old one would not fit it.
                    mu.fit();
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
    };
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
        <div role="group" aria-label="View" class="kg-switch">
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
                {organ_chips(ui, mu, chips.clone())}
                {map_view(ui, mu, chips)}
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
                        view! { {card_view(ui, card, false, "kg-card")} {papers} }
                    }}
                </div>
                {legend_view(ui)}
                <p class="kg-note">{ep::NOT_ALL} " " {ep::LINE_MEANS} " " {ep::SQUARES}</p>
            </section>
        </div>
    }
}

/// Pick an organ (`None`: All): the map starts again at 1x.
fn pick_organ(ui: Ui, mu: MapUi, o: Option<i64>) {
    ui.organ.set(o);
    ui.sel.set(None);
    mu.fit();
}

/// The organ chips (under the find box, and again in full screen).
fn organ_chips(ui: Ui, mu: MapUi, chips: Vec<(i64, String)>) -> AnyView {
    let chip_views = chips
        .into_iter()
        .map(|(id, name)| {
            view! {
                <button
                    type="button"
                    aria-pressed=move || (ui.organ.get() == Some(id)).to_string()
                    on:click=move |_| pick_organ(ui, mu, Some(id))
                >
                    {name}
                </button>
            }
        })
        .collect_view();
    view! {
        <div role="group" aria-label="Organ" class="kg-chips">
            <button type="button" aria-pressed=move || ui.organ.get().is_none().to_string() on:click=move |_| pick_organ(ui, mu, None)>
                "All"
            </button>
            {chip_views}
        </div>
    }
    .into_any()
}

/// A coordinate as a share of the frame, for CSS.
fn pct(v: f32, of: f32) -> String {
    format!("{:.3}%", if of > 0.0 { v / of * 100.0 } else { 0.0 })
}

/// The map, drawn sharp at every zoom: each dot, line and label is placed
/// on screen from the view (`tx + x * k`), nothing is scaled. Dots are
/// real buttons; drag a dot to move it, drag the background to pan, pinch
/// or scroll to zoom. Also the zoom buttons, the time-lapse, the counts,
/// "Open full screen" and the full screen head and card.
fn map_view(ui: Ui, mu: MapUi, chips: Vec<(i64, String)>) -> AnyView {
    use std::collections::HashSet;
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
    // The lit dots when something is in focus (`None`: nothing is dimmed).
    let focus = Memo::new(move |_| {
        if sel.with(Option::is_some) {
            Some(near.get())
        } else if searching.get() {
            Some(found.get())
        } else {
            None
        }
    });
    let lit = move |id: &str| focus.with(|f| f.as_ref().is_none_or(|f| f.contains(id)));
    let frame = move || mu.data.with(|(_, f)| *f);
    // Where each dot is on screen (frame units), dragged offsets and the
    // view applied, and its radius at 1x.
    let pos = Memo::new(move |_| {
        let v = mu.view.get();
        mu.moved.with(|mv| {
            mu.data.with(|(m, _)| {
                m.dots
                    .iter()
                    .map(|d| {
                        let off = mv.get(&d.id).copied().unwrap_or_default();
                        (d.id.clone(), (ep::screen_at((d.x, d.y), off, v), d.r))
                    })
                    .collect::<HashMap<String, ((f32, f32), f32)>>()
            })
        })
    });
    let grow = Memo::new(move |_| ep::grow(mu.view.with(|v| v.0)));
    let place = move |id: &str| pos.with(|p| p.get(id).copied());

    // Lines between screen places, at a constant screen width. Rebuilt when
    // the map, the time-lapse or the focus changes; a pan or zoom only
    // moves their ends.
    let lines = move || {
        let (w, h) = frame();
        let s = sel.get();
        let focusing = s.is_some() || searching.get();
        let l = live.get();
        let views = mu.data.with(|(m, _)| {
            m.lines
                .iter()
                .filter(|x| l.contains(&x.a) && l.contains(&x.b))
                .map(|x| {
                    let on = match s.as_deref() {
                        Some(s) => x.a == s || x.b == s,
                        None => !focusing,
                    };
                    let class = if on && s.is_some() {
                        "kg-ln on"
                    } else {
                        "kg-ln"
                    };
                    let end = move |id: String, i: usize| {
                        move || {
                            place(&id).map_or_else(String::new, |((x, y), _)| {
                                format!("{:.1}", if i == 0 { x } else { y })
                            })
                        }
                    };
                    view! {
                        <line
                            class=class
                            x1=end(x.a.clone(), 0)
                            y1=end(x.a.clone(), 1)
                            x2=end(x.b.clone(), 0)
                            y2=end(x.b.clone(), 1)
                            stroke-width=if x.paper { "1" } else { "1.6" }
                            stroke-dasharray=x.paper.then_some("3 3")
                            vector-effect="non-scaling-stroke"
                            opacity=if on { "1" } else { "0.18" }
                        ></line>
                    }
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
    // rank), its place and size follow the view.
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
                let g = grow.get();
                place(&id).map_or_else(String::new, |((x, y), r)| {
                    let r = r * g;
                    let hit = ep::hit_size(r);
                    format!(
                        "left:{};top:{};width:{hit:.1}px;height:{hit:.1}px;--m:{:.1}px",
                        pct(x, w),
                        pct(y, h),
                        r * 2.0
                    )
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
                data-dot=id
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

    // Which labels show: worked out again only when the rule's inputs
    // change (the zoom matters only across 1.6x); a pan or zoom moves them.
    let zoomed = Memo::new(move |_| mu.view.with(|v| v.0) >= ep::LABELS_FROM_ZOOM);
    let shown_labels = Memo::new(move |_| {
        zoomed.track();
        let k = mu.view.with_untracked(|v| v.0);
        let l = live.get();
        let s = sel.get();
        let full = mu.full.get();
        let spots: Vec<ep::Spot> = mu.data.with(|(m, _)| {
            m.dots
                .iter()
                .filter(|d| l.contains(&d.id))
                .map(ep::Spot::of)
                .collect()
        });
        focus.with(|f| ep::visible_labels(&spots, k, s.as_deref(), f.as_ref(), full))
    });
    let labels = move || {
        shown_labels
            .get()
            .into_iter()
            .map(|b| {
                let id = b.id.clone();
                let style = move || {
                    let (w, h) = frame();
                    let g = grow.get();
                    place(&id).map_or_else(String::new, |((x, y), r)| {
                        format!(
                            "left:{};top:calc({} + {:.1}px)",
                            pct(x, w),
                            pct(y, h),
                            r * g + 2.0
                        )
                    })
                };
                let class = match (b.bold, b.big) {
                    (true, true) => "kg-lab big b",
                    (true, false) => "kg-lab b",
                    (false, true) => "kg-lab big",
                    (false, false) => "kg-lab",
                };
                view! {
                    <div class=class style=style aria-hidden="true">
                        {b.text}
                    </div>
                }
            })
            .collect_view()
    };

    // Pointers: drag the background to pan, drag a dot to move it, pinch
    // to zoom, on the frame only. A pointer is captured by what it went
    // down on, as on the board.
    let to_frame = move |x: i32, y: i32| -> Option<(f32, f32)> {
        #[cfg(feature = "hydrate")]
        {
            let el = mu.layer.get_untracked()?;
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
        // The zoom buttons are buttons, not part of the map: no pan or drag.
        #[cfg(feature = "hydrate")]
        {
            use wasm_bindgen::JsCast;
            if ev
                .target()
                .and_then(|t| t.dyn_into::<web_sys::Element>().ok())
                .and_then(|t| t.closest(".kg-zoom").ok().flatten())
                .is_some()
            {
                return;
            }
        }
        let Some((x, y)) = to_frame(ev.client_x(), ev.client_y()) else {
            return;
        };
        let id = ev.pointer_id();
        // A primary pointer starts a new gesture: any pointer still listed
        // was lost (its up never came) and must not turn this into a pinch.
        if ev.is_primary() {
            mu.drag.update_value(|g| g.pts.clear());
        }
        // The dot it went down on, if any (by its id, never its name).
        #[allow(unused_mut)]
        let mut on_dot: Option<String> = None;
        #[cfg(feature = "hydrate")]
        {
            use wasm_bindgen::JsCast;
            if let Some(t) = ev
                .target()
                .and_then(|t| t.dyn_into::<web_sys::Element>().ok())
            {
                let _ = t.set_pointer_capture(id);
                on_dot = t
                    .closest("[data-dot]")
                    .ok()
                    .flatten()
                    .and_then(|d| d.get_attribute("data-dot"));
            }
        }
        let v = mu.view.get_untracked();
        let off = on_dot.as_ref().map(|d| {
            mu.moved
                .with_untracked(|m| m.get(d).copied().unwrap_or_default())
        });
        mu.drag.update_value(|g| {
            g.pts.retain(|p| p.0 != id);
            g.pts.push((id, x, y));
            match g.pts.len() {
                1 => {
                    g.moved = false;
                    g.start = Some((x, y, v.1, v.2));
                    g.dot = on_dot.zip(off);
                    g.pinch = None;
                }
                2 => {
                    let (d, mid) = two(&g.pts);
                    g.pinch = Some((d, mid, v));
                    g.start = None;
                    g.dot = None;
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
        let (k, _, _) = mu.view.get_untracked();
        let unit = mu.unit.get_untracked().max(1e-3);
        let next = mu.drag.try_update_value(|g| {
            for p in &mut g.pts {
                if p.0 == id {
                    p.1 = x;
                    p.2 = y;
                }
            }
            if let Some((d0, mid0, v0)) = g.pinch
                && g.pts.len() >= 2
            {
                let (d, mid) = two(&g.pts);
                g.moved = true;
                return Some(Step::View(ep::pinch_view(v0, d0, mid0, d, mid)));
            }
            let (sx, sy, tx0, ty0) = g.start?;
            let (dx, dy) = (x - sx, y - sy);
            // Under ten screen pixels it is still a tap.
            if !g.moved && !ep::is_drag(dx / unit, dy / unit) {
                return None;
            }
            g.moved = true;
            Some(match &g.dot {
                Some((dot, (ox, oy))) => Step::Dot(dot.clone(), (ox + dx / k, oy + dy / k)),
                None => Step::View((k, tx0 + dx, ty0 + dy)),
            })
        });
        match next.flatten() {
            Some(Step::View(v)) => mu.view.set(v),
            Some(Step::Dot(dot, off)) => mu.moved.update(|m| {
                m.insert(dot, off);
            }),
            None => {}
        }
    };
    let up = move |ev: leptos::ev::PointerEvent| {
        let id = ev.pointer_id();
        let (_, tx, ty) = mu.view.get_untracked();
        mu.drag.try_update_value(|g| {
            g.pts.retain(|p| p.0 != id);
            if g.pinch.take().is_some() {
                // A finger left after a pinch carries on as a pan.
                g.dot = None;
                g.start = g.pts.first().map(|p| (p.1, p.2, tx, ty));
            } else if g.pts.is_empty() {
                g.start = None;
                g.dot = None;
            }
        });
    };
    // A wheel notch zooms about the cursor (on the frame itself, so the
    // page does not scroll under it).
    let wheel = move |ev: leptos::ev::WheelEvent| {
        ev.prevent_default();
        if let Some(p) = to_frame(ev.client_x(), ev.client_y()) {
            let f = if ev.delta_y() < 0.0 {
                ep::WHEEL_STEP
            } else {
                1.0 / ep::WHEEL_STEP
            };
            let v = mu.view.get_untracked();
            mu.view.set(ep::zoom_about(v, p, f));
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
    // How big the frame is drawn, once it shows and whenever the map, the
    // view or full screen changes. On a switch to Map the frame may still
    // be hidden (width 0, skipped) when this runs: measured again on the
    // next frame.
    Effect::new(move |_| {
        mu.data.track();
        mu.full.track();
        let v = ui.view.get();
        mu.measure();
        #[cfg(feature = "hydrate")]
        if v == View::Map {
            request_animation_frame(move || mu.measure());
        }
        #[cfg(not(feature = "hydrate"))]
        let _ = v;
    });

    // The selected dot's card, floating over the full screen map.
    let float_card = move || {
        if !mu.full.get() {
            return None;
        }
        let id = sel.get()?;
        let card = mu.data.with(|(m, _)| ep::card_for(m, &id))?;
        Some(card_view(ui, card, false, "kg-card kg-float"))
    };
    let full = move || mu.full.get();

    view! {
        <div
            id="kg-map"
            class="kg-map"
            node_ref=mu.dialog
            class:kg-full=full
            hidden=move || ui.view.get() != View::Map
            role=move || full().then_some("dialog")
            aria-modal=move || full().then_some("true")
            aria-label=move || full().then_some("Your map, full screen")
        >
            <div class="kg-full-head" hidden=move || !full()>
                <span class="kg-full-title">"Your map"</span>
                {move || mu.busy.get().then(|| view! { <span class="iv-status">"Drawing your map\u{2026}"</span> })}
                <button type="button" class="kg-close" aria-label="Close full screen" node_ref=mu.close_btn on:click=move |_| mu.ask_close()>
                    "\u{d7}"
                </button>
            </div>
            <div hidden=move || !full()>{organ_chips(ui, mu, chips)}</div>
            <div
                class="kg-map-frame"
                node_ref=mu.node
                style=move || {
                    if full() {
                        return String::new();
                    }
                    let (w, h) = frame();
                    format!("aspect-ratio:{w} / {h}")
                }
                on:pointerdown=down
                on:pointermove=moving
                on:pointerup=up
                on:pointercancel=up
                on:lostpointercapture=up
                on:wheel=wheel
                on:keydown=move |ev: leptos::ev::KeyboardEvent| {
                    // In full screen Escape closes it instead.
                    if ev.key() == "Escape" && !full() {
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
                // Until the page runs in the browser the frame says so.
                <div class="kg-drawing" hidden=move || ui.ready.get()>"Drawing your map\u{2026}"</div>
                <div
                    class="kg-layer"
                    node_ref=mu.layer
                    hidden=move || !ui.ready.get()
                    style=move || {
                        // The map's own shape inside the frame: the whole
                        // frame, or, while an old layout waits for the new
                        // one, as much as fits, centred (never stretched).
                        let b = mu.bx.get();
                        if b.0 <= 0.0 || b.1 <= 0.0 {
                            return String::new();
                        }
                        let (l, t, w, h) = ep::contain(b, frame());
                        format!(
                            "left:{};top:{};width:{};height:{}",
                            pct(l, b.0),
                            pct(t, b.1),
                            pct(w, b.0),
                            pct(h, b.1)
                        )
                    }
                >
                    {lines}
                    {dot_views}
                    {labels}
                </div>
                <div class="kg-zoom" hidden=move || !ui.ready.get()>
                    <button type="button" aria-label="Zoom in" on:click=move |_| mu.zoom(ep::ZOOM_STEP)>"+"</button>
                    <button type="button" aria-label="Zoom out" on:click=move |_| mu.zoom(1.0 / ep::ZOOM_STEP)>"\u{2212}"</button>
                    <button type="button" class="fit" aria-label="Whole map" on:click=move |_| mu.fit()>"Fit"</button>
                </div>
                {move || mu.until.get().map(|y| view! { <div class="kg-year" aria-hidden="true">{y}</div> })}
            </div>
            <button type="button" class="kg-openfull" node_ref=mu.open_btn hidden=full on:click=move |_| mu.open_full(true)>
                "Open full screen"
            </button>
            <div class="kg-foot" hidden=full>
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
            {float_card}
        </div>
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
            {card.map(|c| card_view(ui, c, true, "kg-card kg-inline"))}
        </div>
    }
    .into_any()
}

/// A card: in place under its row (`inline`, phones: no title, no link
/// buttons), in the side column, or floating over the full screen map.
/// A paper's has its rank chip, its details line, the questions it came
/// from, "Open in PubMed ↗" (a new tab) and its study sheet; no abstract.
fn card_view(ui: Ui, c: Card, inline: bool, class: &'static str) -> AnyView {
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
    let chip = c
        .chip
        .map(|(text, rank)| view! { <span class=format!("kg-chip {rank}")>{text}</span> });
    // Each question it came from: a text-style button that picks it.
    let from = (!c.from.is_empty()).then(|| {
        let n = c.from.len();
        let names = c
            .from
            .into_iter()
            .enumerate()
            .map(|(i, (id, label))| {
                view! {
                    <button type="button" class="kg-from-q" on:click=move |_| ui.sel.set(Some(id.clone()))>
                        {label}
                    </button>
                    {(i + 1 < n).then_some("; ")}
                }
            })
            .collect_view();
        view! { <p class="kg-from">"From your question: " {names}</p> }
    });
    let acts = (c.answer.is_some() || c.pubmed.is_some() || c.sheet.is_some() || c.ask.is_some())
        .then(|| {
            view! {
                <div class="kg-acts">
                    {c.answer.map(|href| view! { <a class="kg-act" href=href>"Open the full answer"</a> })}
                    {c.pubmed.map(|href| view! {
                        <a class="kg-act" href=href target="_blank" rel="noopener noreferrer">"Open in PubMed \u{2197}"</a>
                    })}
                    {c.sheet.map(|href| view! { <a class="kg-act alt" href=href rel="external">"Open the study sheet"</a> })}
                    {c.ask.map(|href| view! { <a class="kg-act alt" href=href>"Ask about this"</a> })}
                </div>
            }
        });
    view! {
        <div class=class>
            <div class="kg-kicker">{chip} <span>{c.kicker}</span></div>
            {(!inline).then(|| view! { <div class="kg-card-title">{c.title}</div> })}
            <div class="kg-card-body">{c.body}</div>
            {flags}
            {from}
            {links}
            {acts}
        </div>
    }
    .into_any()
}

#[cfg(all(test, feature = "ssr"))]
mod tests {
    use super::*;
    use leptos::tachys::view::RenderHtml;

    fn render(c: Card, inline: bool) -> String {
        Owner::new().with(|| card_view(Ui::new(), c, inline, "kg-card").to_html())
    }

    fn paper_card() -> Card {
        Card {
            kicker: "Paper".into(),
            chip: Some(("1 \u{b7} RCT".into(), "rct")),
            title: "RIVAL 2020, JAMA Surg".into(),
            body: "Multicentre pilot RCT \u{b7} 2020 \u{b7} n=113 \u{b7} PMID 32186683".into(),
            flags: vec!["Retracted".into()],
            sheet: Some("/runs/00000000-0000-0000-0000-000000000001#ref-4".into()),
            pubmed: Some("https://pubmed.ncbi.nlm.nih.gov/32186683/".into()),
            from: vec![
                ("r:1".into(), "Inguinal hernia question".into()),
                ("r:2".into(), "Achalasia question".into()),
            ],
            ..Card::default()
        }
    }

    #[test]
    fn a_papers_card_previews_it_and_opens_pubmed_in_a_new_tab() {
        let html = render(paper_card(), false);
        let at = |s: &str| html.find(s).unwrap_or_else(|| panic!("no {s} in {html}"));
        // The rank chip, coloured by rank, then "Paper".
        assert!(at("<span class=\"kg-chip rct\">1 \u{b7} RCT</span>") < at(">Paper<"));
        assert!(html.contains("RIVAL 2020, JAMA Surg"));
        assert!(html.contains("PMID 32186683"));
        assert!(html.contains("n=113"));
        assert!(html.contains("Retracted"));
        assert!(html.contains("From your question:"));
        // Each question is a button that picks it.
        let q =
            regex::Regex::new(r#"<button [^>]*class="kg-from-q"[^>]*>([^<]*)</button>"#).unwrap();
        let names: Vec<String> = q.captures_iter(&html).map(|c| c[1].to_string()).collect();
        assert_eq!(names, ["Inguinal hernia question", "Achalasia question"]);
        // PubMed first, a real link to a new tab; then the study sheet.
        let pubmed = regex::Regex::new("<a [^>]*>Open in PubMed \u{2197}</a>")
            .unwrap()
            .find(&html)
            .expect("Open in PubMed")
            .as_str()
            .to_string();
        assert!(
            pubmed.contains(r#"href="https://pubmed.ncbi.nlm.nih.gov/32186683/""#),
            "{pubmed}"
        );
        assert!(pubmed.contains(r#"target="_blank""#), "{pubmed}");
        assert!(pubmed.contains(r#"rel="noopener noreferrer""#), "{pubmed}");
        assert!(pubmed.contains(r#"class="kg-act""#), "primary: {pubmed}");
        let sheet = at("Open the study sheet");
        assert!(at("Open in PubMed") < sheet);
        let sheet = regex::Regex::new("<a [^>]*>Open the study sheet</a>")
            .unwrap()
            .find(&html)
            .expect("Open the study sheet")
            .as_str()
            .to_string();
        assert!(
            sheet.contains(r#"href="/runs/00000000-0000-0000-0000-000000000001#ref-4""#),
            "{sheet}"
        );
        assert!(
            sheet.contains(r#"class="kg-act alt""#),
            "secondary: {sheet}"
        );
        assert!(!sheet.contains("_blank"), "{sheet}");
        // Title and details only: no abstract.
        assert!(!html.to_lowercase().contains("abstract"), "{html}");
    }

    #[test]
    fn other_cards_have_no_chip() {
        let c = Card {
            kicker: "Disease".into(),
            title: "Achalasia".into(),
            ask: Some("/?topic=7".into()),
            ..Card::default()
        };
        let html = render(c, false);
        assert!(!html.contains("kg-chip"));
        assert!(!html.contains("PubMed"));
        assert!(
            html.contains(r#"<a href="/?topic=7" class="kg-act alt">Ask about this</a>"#),
            "{html}"
        );
    }
}
