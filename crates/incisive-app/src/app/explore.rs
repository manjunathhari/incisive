//! The Explore page (`/explore`, `GRAPH=on`): the user's own map as a list
//! of diseases, treatments, outcomes, questions and ranked papers, each
//! group folded at five rows, with a card for the picked row (in place on a
//! phone, at the side on a wide screen). The map itself is drawn in a later
//! step; its place holds the counts.
//!
//! Everything is drawn on the server first, so the list works before the
//! page hydrates. Every fetch goes through the server functions here, which
//! use the signed-in user and know nothing with `GRAPH` off. Labels are
//! text, never markup; nothing here logs a name or a question.

use std::collections::BTreeSet;

use leptos::prelude::*;
use leptos::server_fn::codec::Json;

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
.kg-map-frame { position:relative; box-sizing:border-box; width:100%; aspect-ratio:860 / 720; background:var(--iv-card); border:1px solid var(--iv-border); border-radius:18px; display:flex; align-items:center; justify-content:center; padding:16px; }
.kg-map-frame p { margin:0; font-size:14px; color:var(--iv-muted); text-align:center; }
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
@media (min-width:641px) { .kg-card.kg-inline { display:none; } }
@media (max-width:640px) {
  .kg-main, .kg-side { flex-basis:100%; }
  .kg-map-frame { aspect-ratio:358 / 480; }
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
    crate::graph::read::load_map_all(&state.pool, user.id, filter)
        .await
        .map_err(|_| {
            tracing::error!(user_id = user.id, "explore: map read failed");
            super::set_status(axum::http::StatusCode::INTERNAL_SERVER_ERROR);
            ServerFnError::new("Incisive couldn't load your map. Please try again.")
        })
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
    let mut data = load(&filter).await?;
    ep::cap_for_map(&mut data, crate::graph::read::MAX_DOTS);
    let size = |v: f32, fallback: f32| {
        if v.is_finite() && v > 0.0 {
            v.min(4000.0)
        } else {
            fallback
        }
    };
    let (w, h) = (size(w, ep::DESKTOP_FRAME.0), size(h, ep::DESKTOP_FRAME.1));
    crate::graph_view::layout(&mut data.dots, &data.lines, w, h);
    Ok(data)
}

/// The signed-in user's whole map under the filter, not laid out (the List
/// view always has everything). Not found with `GRAPH` off.
#[server(prefix = "/api/fn", endpoint = "explore_list", input = Json)]
pub async fn get_list(#[server(default)] filter: MapFilter) -> Result<MapData, ServerFnError> {
    load(&filter).await
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
}

#[component]
pub fn ExplorePage() -> impl IntoView {
    // With `GRAPH` off this page does not exist.
    #[cfg(feature = "ssr")]
    if !use_context::<crate::server::AppState>().is_some_and(|s| s.graph.settings.on)
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
    };
    Effect::new(move |_| {
        ui.view.set(load_view().unwrap_or(View::Map));
        ui.ready.set(true);
    });
    let all = Resource::new(|| (), |_| get_list(MapFilter::default()));
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
                all.get()
                    .map(|r| match r {
                        // With `GRAPH` off: the app's usual not-found, nothing
                        // of this page.
                        Err(e) if error_text(&e) == ep::NOT_FOUND => {
                            view! { <p>{ep::NOT_FOUND}</p> }.into_any()
                        }
                        Err(e) => page(view! { <p class="box amber" role="alert">{error_text(&e)}</p> }.into_any()),
                        Ok(d) if d.runs == 0 => page(view! { <p class="iv-empty">{ep::EMPTY}</p> }.into_any()),
                        Ok(d) => page(explore_body(ui, d).into_any()),
                    })
            }}
        </Suspense>
    }
}

/// The switch, the find box, the organ chips, the map's place, the list and
/// the side column, for a user with runs on the map (`all`: unfiltered).
fn explore_body(ui: Ui, all: MapData) -> impl IntoView {
    let chips = ep::organs(&all);
    let all = StoredValue::new(all);
    // An organ's own list; the whole map needs no second fetch.
    let picked = Resource::new(
        move || ui.organ.get(),
        |organ| async move {
            match organ {
                None => Ok(None),
                Some(o) => get_list(MapFilter {
                    organ: Some(o),
                    ..MapFilter::default()
                })
                .await
                .map(Some),
            }
        },
    );
    let map = Resource::new(
        move || ui.organ.get(),
        |organ| {
            get_map(
                MapFilter {
                    organ,
                    ..MapFilter::default()
                },
                ep::DESKTOP_FRAME.0,
                ep::DESKTOP_FRAME.1,
            )
        },
    );
    let set_view = move |v: View| {
        ui.view.set(v);
        save_view(v);
    };
    let pick_organ = move |o: Option<i64>| {
        ui.organ.set(o);
        ui.sel.set(None);
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
        match ui.organ.get() {
            None => Some(Ok(all.get_value())),
            Some(_) => picked.get().map(|r| r.map(Option::unwrap_or_default)),
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
                    <div class="kg-map-frame">
                        <p>"The map is drawn here in the next update. List shows every dot now."</p>
                    </div>
                    <Suspense fallback=|| ()>
                        {move || {
                            map.get()
                                .map(|r| match r {
                                    Ok(m) => {
                                        let top = ep::map_note(&m);
                                        view! {
                                            <p class="kg-foot">
                                                <span>{ep::count_line(&m)}</span>
                                                {top.map(|t| view! { <span>{t}</span> })}
                                            </p>
                                        }
                                            .into_any()
                                    }
                                    Err(e) => view! { <p class="box amber" role="alert">{error_text(&e)}</p> }.into_any(),
                                })
                        }}
                    </Suspense>
                </div>
                <div class="kg-list" hidden=move || ui.view.get() != View::List>
                    <Suspense fallback=|| view! { <p class="iv-status">"Loading\u{2026}"</p> }>
                        {move || {
                            data()
                                .map(|r| match r {
                                    Ok(d) => list_view(ui, d).into_any(),
                                    Err(e) => view! { <p class="box amber" role="alert">{error_text(&e)}</p> }.into_any(),
                                })
                        }}
                    </Suspense>
                </div>
            </section>
            <section class="kg-side" aria-label="Details and filters">
                <div class="kg-side-card">
                    <Suspense fallback=|| ()>
                        {move || {
                            data()
                                .map(|r| {
                                    let d = r.unwrap_or_default();
                                    let card = ui
                                        .sel
                                        .get()
                                        .and_then(|id| ep::card_for(&d, &id))
                                        .unwrap_or_else(ep::default_card);
                                    card_view(ui, card, false)
                                })
                        }}
                    </Suspense>
                </div>
                <p class="kg-note">{ep::NOT_ALL} " " {ep::LINE_MEANS} " " {ep::SQUARES}</p>
            </section>
        </div>
    }
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
        </div>
    }
    .into_any()
}
