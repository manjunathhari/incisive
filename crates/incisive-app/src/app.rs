//! Pages: signed-in banner, Ask page, run page, history (My questions), Activity page (owner).
//!
//! The server functions here call the same code as the JSON routes
//! (`crate::ask::run_ask` and friends). All server-only imports live inside their
//! bodies, so this module also builds for the browser (`hydrate`).
//!
//! Everything shown is plain text: Leptos escapes it, and no `inner_html` is used.
//! The answer is drawn from its structured fields: a Phase 2
//! `agent::answer::Answer` (`answer_version` 2) with its own view, or a
//! Phase 3 `agent::answer_v3::AnswerV3` (version 3) with [`answer_v3_view`],
//! whose `[PMID]` citations become numbered links to the reference list.

use leptos::prelude::*;
use leptos_meta::{MetaTags, Title, provide_meta_context};
use leptos_router::{
    components::{A, Route, Router, Routes},
    hooks::{use_params_map, use_query_map},
    path,
};

use crate::agent::answer::{
    self, Answer, DECISION_SUPPORT, INSUFFICIENT, PARTIAL_MARK, Reference, check_text, date_ist,
    duration_text, empty_bottom_line, exclusions_text, key_study_text, mode_label, render_text,
    rerun_href, screened_text, show_insufficient, stopped_text, unavailable_text,
};
use crate::agent::answer_v3::{
    ADDED_LABEL, AnswerV3, CompletenessV3, EvidenceV3, FOUND_BY_SKEPTIC, NO_GUIDELINE_SOURCE,
    NotableFinding, ReferenceV3, Segment, SkepticItem, Span, WrittenAnswer, arm_line_text,
    arm_lines, marked_runs, not_cited_at_end, split_citations, whole_marked,
};
use crate::ask::{ActivityView, AskOutcome, MeView};
// Part A: the interactive answer page (Phase 3 answers).
// The ivory redesign: header and tokens, the Ask page and the Library.
mod ask_page;
mod details;
mod draft;
// The evidence chat's option and window.
mod chat;
// Temporal T4: the Expert Opinion option, view and board page.
mod expert;
// The knowledge graph's Explore page (GRAPH=on).
mod explore;
mod intro;
mod ivory;
mod library_page;
mod page;
mod tracker;
use crate::runs::{RUN_NOT_FOUND_MESSAGE, RunMode, RunStatus, RunSummary, RunView};
/// The chat window's styles (exported for the scope test).
pub use chat::CHAT_STYLE;
/// The Expert Opinion screens' styles (exported for the scope test).
pub use expert::{ANSWER_VIEW, EXPERT_STYLE};
/// The ivory styles (exported for the scope test: none of them reach the
/// answer content).
pub use ivory::IVORY_STYLE;
use uuid::Uuid;

const STYLE: &str = r#"
:root { --fg:#1d232b; --muted:#5b6672; --bg:#f7f8fa; --card:#fff; --line:#d9dee4;
  --red:#a4161a; --red-bg:#fdecec; --amber:#8a5300; --amber-bg:#fff6e0;
  --green:#1b6b3a; --green-bg:#e9f7ee; --accent:#1f4e79; }
* { box-sizing:border-box; }
body { margin:0; font:16px/1.5 system-ui,-apple-system,"Segoe UI",sans-serif; color:var(--fg); background:var(--bg); }
main { max-width:860px; margin:24px auto; padding:0 16px; }
label { display:block; font-weight:600; margin-bottom:6px; }
textarea { width:100%; padding:10px; font:inherit; border:1px solid var(--line); border-radius:6px; resize:vertical; }
.hint { color:var(--muted); font-size:.85rem; margin:6px 0 12px; }
button { font:inherit; padding:8px 20px; border:0; border-radius:6px; background:var(--accent); color:#fff; cursor:pointer; }
button[disabled] { opacity:.6; cursor:default; }
.box { margin-top:20px; padding:12px 16px; border-radius:6px; border-left:4px solid; }
.box p { margin:0 0 6px; } .box ul { margin:6px 0; }
.red { background:var(--red-bg); border-color:var(--red); color:var(--red); }
.amber { background:var(--amber-bg); border-color:var(--amber); color:var(--amber); }
.green { background:var(--green-bg); border-color:var(--green); color:var(--green); }
.chain { display:inline-block; padding:4px 10px; border-radius:12px; font-size:.9rem; }
.scroll { overflow-x:auto; }
table { border-collapse:collapse; width:100%; font-size:.85rem; background:var(--card); }
th, td { text-align:left; padding:6px 8px; border-bottom:1px solid var(--line); vertical-align:top; }
td code { font-size:.8rem; word-break:break-all; }
button.secondary { background:#fff; color:var(--accent); border:1px solid var(--accent); margin-left:8px; }
button.danger { background:var(--red); }
button.small { padding:4px 12px; font-size:.85rem; }
.badge { display:inline-block; padding:2px 10px; border-radius:12px; font-size:.85rem; background:var(--line); color:var(--fg); }
.badge.queued, .badge.running { background:var(--amber-bg); color:var(--amber); }
.badge.done { background:var(--green-bg); color:var(--green); }
.badge.partial { background:var(--amber-bg); color:var(--amber); }
.badge.failed, .badge.interrupted { background:var(--red-bg); color:var(--red); }
.run-question { overflow-wrap:anywhere; font-size:1.2rem; }
.steps { padding-left:20px; } .steps li { margin:4px 0; }
.runs { list-style:none; padding:0; margin:12px 0; }
.runs li { display:flex; flex-wrap:wrap; gap:6px 12px; align-items:center; justify-content:space-between; padding:10px 12px; background:var(--card); border:1px solid var(--line); border-radius:6px; margin-bottom:8px; }
.runs .meta { color:var(--muted); font-size:.85rem; }
.answer { background:var(--card); border:1px solid var(--line); border-radius:6px; padding:12px 16px; margin-top:16px; }
.answer h3 { margin:18px 0 6px; font-size:1.05rem; }
.answer .decision { font-weight:600; color:var(--accent); margin:0 0 4px; }
.answer dl { display:grid; grid-template-columns:max-content 1fr; gap:2px 12px; margin:0; }
.answer dt { color:var(--muted); } .answer dd { margin:0; }
.answer code { word-break:break-word; font-size:.85rem; }
.answer ul { padding-left:20px; margin:4px 0; } .answer li { margin:4px 0; }
.answer .refs-list { list-style:none; padding-left:0; }
.conf { color:var(--muted); font-size:.9rem; }
.partial-mark { color:var(--amber); font-weight:600; }
.answer a { color:var(--accent); word-break:break-word; }
.badge.mode { background:#e6edf5; color:var(--accent); }
fieldset.depth { border:0; padding:0; margin:0 0 14px; min-width:0; }
fieldset.depth legend { font-weight:600; padding:0; margin-bottom:6px; }
.seg { display:inline-flex; flex-wrap:wrap; border:1px solid var(--accent); border-radius:6px; overflow:hidden; background:var(--card); }
.seg label { display:block; position:relative; margin:0; font-weight:400; }
.seg input { position:absolute; opacity:0; width:1px; height:1px; margin:0; }
.seg span { display:block; padding:6px 14px; cursor:pointer; color:var(--accent); font-size:.95rem; }
.seg label + label span { border-left:1px solid var(--accent); }
.seg input:checked + span { background:var(--accent); color:#fff; }
.seg input:focus-visible + span { outline:2px solid var(--fg); outline-offset:-4px; }
.answer table.wide { min-width:720px; }
.answer .cite a { text-decoration:none; }
.answer .retracted { color:var(--red); font-weight:600; }
.answer details summary { cursor:pointer; font-weight:600; margin:18px 0 6px; }
.answer .etype { display:inline-block; margin-top:2px; padding:0 6px; border-radius:8px; font-size:.72rem; background:#e6edf5; color:var(--accent); }
.answer .glance { margin-top:12px; } .answer .glance ul { margin:4px 0 6px; }
.answer ul.arms { list-style:none; padding-left:0; margin:6px 0 0; font-size:.8rem; color:var(--muted); }
.answer textarea.copy-area { position:absolute; left:-9999px; width:1px; height:1px; opacity:0; }
.answer textarea.copy-area.shown { position:static; width:100%; height:12rem; opacity:1; margin-top:8px; font-size:.8rem; }
.answer sup.flag { font-size:.68rem; color:var(--amber); margin-left:2px; white-space:nowrap; cursor:help; }
.answer .flag-label { font-size:.75rem; color:var(--amber); font-weight:600; }
.answer ul.unverified { padding-left:16px; margin:4px 0 0; font-size:.8rem; }
.answer .flag-label.added { color:var(--accent); }
.answer p.added { margin:6px 0 0; }
.answer tr.added td { background:#f4f8fc; }
/* Part A: the interactive answer page (scoped to .ap / .ap-main; system fonts). */
.ap { --ink:#15202B; --ink2:#3B4651; --mute:#55606B; --rule:#D5DBE1; --soft:#F7F9FA; --mono:ui-monospace,SFMono-Regular,Menlo,Consolas,monospace;
  margin-top:16px; }
/* The new page only: the original run view keeps main's width and colours. */
.ap-main { max-width:760px; color:var(--ink); display:flex; flex-direction:column; gap:14px; }
.ap-main h3 { margin:0; font-size:1.1rem; font-weight:600; }
.ap-main h4 { margin:0 0 4px; font-size:.9rem; font-weight:600; }
.ap-main p { margin:0; }
.ap-main a { color:#1D5FD1; word-break:break-word; }
.ap-main code { word-break:break-word; font-size:.8rem; }
.ap-head { display:flex; flex-direction:column; gap:6px; }
.ap-meta { font-family:var(--mono); font-size:.75rem; color:var(--mute); }
.ap-mode { padding:3px 8px; border-radius:6px; background:var(--ink); color:#fff; }
.ap-decision, .ap-rerun { font-size:.82rem; color:var(--mute); }
.ap-card { background:#fff; border:1px solid var(--rule); border-radius:16px; padding:16px; display:flex; flex-direction:column; gap:12px; }
.ap-kicker { font-family:var(--mono); font-size:.7rem; letter-spacing:.1em; text-transform:uppercase; color:var(--mute); }
.ap-kicker.amber { color:#7A3E06; }
.ap-row { display:flex; flex-wrap:wrap; justify-content:space-between; align-items:center; gap:8px; }
.ap-muted { font-size:.82rem; color:var(--mute); line-height:1.45; }
.ap-sub { font-size:.74rem; color:var(--mute); line-height:1.4; }
.ap-mono { font-family:var(--mono); font-size:.8rem; font-weight:500; }
.ap-main .cite { font-size:0; white-space:nowrap; }
.ap-main .cite a { display:inline-block; font-family:var(--mono); font-size:.7rem; line-height:1.4; padding:1px 6px; margin:0 1px; border-radius:5px; border:1px solid #B9CBEA; background:#EEF3FC; color:#143F8C; text-decoration:none; vertical-align:1px; min-width:22px; text-align:center; }
.ap-main .cite.plain { font-size:.75rem; font-family:var(--mono); color:#143F8C; }
.ap-main sup.flag { font-size:.62rem; font-weight:600; padding:1px 5px; margin-left:2px; border-radius:4px; background:#FBEBD3; color:#7A3E06; vertical-align:2px; white-space:nowrap; cursor:help; }
.ap-main .flag-label { font-size:.75rem; color:var(--amber); font-weight:600; }
.ap-main .flag-label.added { color:#4B2E83; }
.ap-main .retracted { color:var(--red); font-weight:600; }
.ap-main .added { margin:0; }
.ap-main ul.unverified { padding-left:16px; margin:4px 0 0; font-size:.8rem; }
.ap-main textarea.copy-area { position:absolute; left:-9999px; width:1px; height:1px; opacity:0; }
.ap-main textarea.copy-area.shown { position:static; width:100%; height:12rem; opacity:1; margin-top:8px; font-size:.8rem; }
.ap-cert { font-size:.75rem; font-weight:600; padding:4px 10px; border-radius:999px; white-space:nowrap; background:#EEF1F3; color:var(--ink2); }
.ap-cert.c-high { background:#D7EFE3; color:#0B5A36; }
.ap-cert.c-moderate { background:#DCE8FB; color:#143F8C; }
.ap-cert.c-low { background:#FBEBD3; color:#7A3E06; }
.ap-cert.c-verylow { background:#F6DADA; color:#8A1C1C; }
.ap-glance { background:var(--ink); color:#fff; border-color:var(--ink); }
.ap-glance .ap-kicker { color:#B9C6D3; }
.ap-glance .cite a { background:#22303D; border-color:#4F6E92; color:#8FB4F5; }
.ap-verdict { font-size:1.15rem; line-height:1.35; font-weight:600; }
.ap-tiles { display:grid; grid-template-columns:repeat(auto-fit,minmax(150px,1fr)); gap:8px; }
.ap-tile { background:#22303D; border:1px solid #33475A; border-radius:12px; padding:12px; display:flex; flex-direction:column; gap:6px; min-height:44px; }
.ap-tile.tappable, .ap-trial { position:relative; }
.ap-main .ap-tile.tappable a.ap-tile-src::after, .ap-main .ap-trial a.ap-stretch::after { content:""; position:absolute; inset:0; border-radius:inherit; }
.ap-tile .cite a, .ap-trial .ap-sub, .ap-trial .ap-study-details { position:relative; z-index:1; }
.ap-study-details > summary { cursor:pointer; font-size:.82rem; font-weight:600; color:#143F8C; min-height:44px; display:flex; align-items:center; }
.ap-study-details[open] { display:flex; flex-direction:column; gap:10px; }
.ap-tile-label { font-size:.75rem; color:#B9C6D3; }
.ap-tile-value { font-size:.9rem; line-height:1.4; font-weight:500; }
.ap-main a.ap-tile-src { font-family:var(--mono); font-size:.72rem; color:#8FB4F5; text-decoration:none; padding:4px 0; }
.ap-caveat { font-size:.85rem; line-height:1.5; color:#E2E8EE; border-top:1px solid #33475A; padding-top:12px; }
.ap-bl { font-size:.98rem; line-height:1.6; }
.ap-reason { font-size:.85rem; line-height:1.5; color:var(--mute); }
.ap-part p { font-size:.9rem; line-height:1.55; color:var(--ink2); }
.ap-pills { display:flex; gap:8px; overflow-x:auto; padding-bottom:4px; }
.ap-pills.wrap { flex-wrap:wrap; overflow:visible; }
.ap-main .ap-pill { flex:none; font:inherit; font-size:.82rem; padding:8px 14px; border-radius:999px; min-height:44px; white-space:nowrap; background:#fff; color:var(--ink); border:1px solid var(--rule); display:inline-flex; align-items:center; gap:6px; text-decoration:none; cursor:pointer; }
.ap-main .ap-pill.on { background:var(--ink); color:#fff; border-color:var(--ink); }
.ap-main .ap-pill.small { padding:6px 12px; }
.ap-main .ap-pills.wrap .ap-pill { white-space:normal; flex:0 1 auto; max-width:100%; text-align:left; }
.ap-pill-n { opacity:.7; }
.ap-seg { display:flex; gap:4px; background:#F3F5F7; border:1px solid var(--rule); border-radius:999px; padding:4px; }
.ap-sentence { font-size:1rem; line-height:1.5; font-weight:500; }
.ap-grids { display:flex; flex-wrap:wrap; gap:24px; }
.ap-grid-arm { display:flex; flex-direction:column; gap:10px; flex:1 1 200px; }
.ap-grid-head { display:flex; align-items:baseline; flex-wrap:wrap; gap:4px 10px; font-size:.9rem; color:var(--ink2); }
.ap-big { font-size:2.2rem; font-weight:700; line-height:1; }
.ap-grid { display:grid; grid-template-columns:repeat(10,16px); gap:4px; }
.ap-cell { width:16px; height:16px; border-radius:4px; background:#E3E7EB; }
.ap-legend { display:flex; flex-wrap:wrap; gap:6px; align-items:center; font-size:.76rem; color:var(--ink2); border-top:1px solid #E3E7EB; padding-top:12px; }
.ap-swatch { width:12px; height:12px; border-radius:3px; background:var(--ink); display:inline-block; }
.ap-box { background:var(--soft); border-radius:10px; padding:12px; font-size:.85rem; line-height:1.5; color:var(--ink2); }
.arm0.ap-cell, .arm0.ap-fill { background:#1D5FD1; } .arm0.ap-big { color:#1D5FD1; }
.arm1.ap-cell, .arm1.ap-fill { background:#B85A0B; } .arm1.ap-big { color:#B85A0B; }
.arm2.ap-cell, .arm2.ap-fill { background:#5F6673; } .arm2.ap-big { color:#5F6673; }
.arm3.ap-cell, .arm3.ap-fill { background:#0E7C74; } .arm3.ap-big { color:#0E7C74; }
.src-trials { background:#15202B; } .src-ma { background:#1D5FD1; } .src-obs { background:#7A4FA8; } .src-models { background:#0E7C74; }
.ap-frows { display:flex; flex-direction:column; }
.ap-main .ap-frow { display:flex; flex-direction:column; gap:3px; padding:8px 0; border-bottom:1px solid #EEF1F3; min-height:44px; color:inherit; text-decoration:none; }
.ap-main .ap-frow.ap-hidden { display:none; }
.ap-frow-top, .ap-est-top { display:flex; justify-content:space-between; gap:8px; font-size:.8rem; }
.ap-frow-label { font-weight:600; line-height:1.3; }
.ap-frow-top .ap-mono, .ap-est-top .ap-mono { white-space:nowrap; }
.ap-track-axis { display:block; position:relative; height:18px; margin:0 6px; }
.ap-line { position:absolute; left:0; right:0; top:8px; height:1px; background:var(--rule); }
.ap-ref-line { position:absolute; top:0; width:1px; height:18px; background:var(--ink); }
.ap-ci { position:absolute; top:6px; height:5px; border-radius:3px; opacity:.35; }
.ap-pt { position:absolute; top:2px; width:13px; height:13px; margin-left:-6px; border-radius:3px; }
.ap-pt.round { border-radius:50%; }
.ap-ci.diff, .ap-pt.diff { background:#B85A0B; }
.ap-dot { width:10px; height:10px; border-radius:3px; display:inline-block; }
.ap-ticks { position:relative; height:14px; margin:0 6px; font-family:var(--mono); font-size:.62rem; color:var(--mute); }
.ap-tick { position:absolute; top:0; transform:translateX(-50%); }
.ap-tick.one { color:var(--ink); font-weight:600; }
.ap-scale { display:flex; justify-content:space-between; font-family:var(--mono); font-size:.62rem; color:var(--mute); }
.ap-note { background:#fff; border:1px dashed #B9C2CB; border-radius:14px; padding:14px; font-size:.82rem; line-height:1.5; color:var(--mute); }
.ap-outcomes, .ap-details { display:flex; flex-direction:column; gap:10px; }
.ap-out { background:#fff; border:1px solid var(--rule); border-radius:14px; overflow:hidden; }
.ap-out > summary { list-style:none; display:flex; justify-content:space-between; gap:10px; padding:14px; cursor:pointer; min-height:56px; }
.ap-out > summary::-webkit-details-marker { display:none; }
.ap-out-head { display:flex; flex-direction:column; gap:5px; min-width:0; }
.ap-out-name { font-size:.95rem; font-weight:600; line-height:1.3; }
.ap-tags { display:flex; flex-wrap:wrap; gap:6px; align-items:center; }
.ap-tag { font-size:.7rem; padding:2px 7px; border-radius:5px; background:#EEF1F3; color:var(--ink2); }
.ap-tag.blue { background:#DCE8FB; color:#143F8C; } .ap-tag.amber { background:#FBEBD3; color:#7A3E06; }
.ap-tag.purple, .ap-tag.added { background:#E6E0F3; color:#4B2E83; }
.ap-count-note { font-size:.7rem; color:var(--mute); }
.ap-first { font-size:.82rem; color:var(--ink2); line-height:1.45; }
.ap-out[open] .ap-first { display:none; }
.ap-out-side { display:flex; flex-direction:column; align-items:flex-end; gap:10px; flex:none; }
.ap-chev { width:9px; height:9px; border-right:2px solid var(--mute); border-bottom:2px solid var(--mute); transform:rotate(45deg); margin:2px 6px 0; transition:transform .15s; }
.ap-out[open] .ap-chev { transform:rotate(-135deg); margin-top:8px; }
.ap-out-body { border-top:1px solid #EEF1F3; padding:14px; display:flex; flex-direction:column; gap:14px; font-size:.9rem; line-height:1.6; color:var(--ink2); }
.ap-small { font-size:.78rem; color:var(--mute); line-height:1.45; }
.ap-groups { display:flex; flex-direction:column; gap:10px; }
.ap-group { border:1px solid #E3E7EB; border-radius:10px; padding:12px; display:flex; flex-direction:column; gap:9px; background:var(--soft); }
.ap-group-head { display:flex; justify-content:space-between; gap:8px; align-items:flex-start; }
.ap-group-label { font-size:.82rem; font-weight:600; line-height:1.35; color:var(--ink); }
.ap-main a.ap-study { flex:none; font-size:.72rem; padding:5px 8px; border-radius:6px; border:1px solid #B9CBEA; background:#fff; color:#143F8C; max-width:160px; text-align:right; text-decoration:none; }
.ap-main a.ap-study.big { font-size:.82rem; padding:8px 12px; border-color:var(--ink); color:var(--ink); min-height:44px; max-width:none; display:inline-flex; align-items:center; }
.ap-bars { display:flex; flex-direction:column; gap:7px; }
.ap-bar-row { display:grid; grid-template-columns:minmax(0,112px) minmax(0,1fr) auto; gap:8px; align-items:center; }
.ap-arm { font-size:.75rem; font-weight:600; line-height:1.25; overflow-wrap:break-word; min-width:0; color:var(--ink); }
.ap-bar { position:relative; display:block; height:18px; background:#E3E7EB; border-radius:4px; overflow:hidden; }
.ap-bar.big { height:24px; border-radius:6px; }
.ap-fill { position:absolute; left:0; top:0; bottom:0; border-radius:4px; }
.ap-bar-ci { grid-column:2 / 4; margin-top:-4px; }
.ap-whisk { position:absolute; top:45%; height:3px; background:var(--ink); }
.ap-counts { font-size:.72rem; color:#7A3E06; }
.ap-p { font-size:.76rem; color:var(--mute); }
.ap-numline { line-height:1.5; overflow-wrap:anywhere; }
.ap-est { display:flex; flex-direction:column; gap:3px; }
.ap-why { font-size:.82rem; line-height:1.5; background:var(--soft); border-radius:10px; padding:10px 12px; }
.ap-measures { display:flex; flex-direction:column; gap:6px; }
.ap-main .ap-measure { width:100%; display:flex; justify-content:space-between; align-items:center; gap:10px; padding:10px 12px; border-radius:10px; background:#fff; color:var(--ink); min-height:48px; border:1px solid var(--rule); text-align:left; font:inherit; cursor:pointer; }
.ap-main .ap-measure.on { border:2px solid var(--ink); }
.ap-measure-text { display:flex; flex-direction:column; gap:2px; min-width:0; }
.ap-measure-label { font-size:.88rem; font-weight:600; line-height:1.3; }
.ap-panel { background:var(--soft); border:1px solid #E3E7EB; border-radius:12px; padding:14px; display:flex; flex-direction:column; gap:12px; }
.ap-panel-label { font-size:1rem; font-weight:600; line-height:1.3; }
.ap-bigbar { display:flex; flex-direction:column; gap:5px; }
.ap-bigval { font-size:1rem; font-weight:600; }
.ap-details-title { padding:6px 2px 0; }
.ap-sec { background:#fff; border:1px solid var(--rule); border-radius:14px; }
.ap-sec-head { display:flex; justify-content:space-between; align-items:center; gap:10px; padding:14px; border-bottom:1px solid #EEF1F3; }
.ap-main .ap-sec-head h3 { font-size:.98rem; }
.ap-count { font-family:var(--mono); font-size:.75rem; padding:2px 8px; border-radius:999px; background:#EEF1F3; color:var(--ink2); white-space:nowrap; }
.ap-sec-body { padding:14px; display:flex; flex-direction:column; gap:10px; font-size:.9rem; line-height:1.55; color:var(--ink2); }
.ap-sec-body ul { padding-left:20px; margin:0; } .ap-sec-body li { margin:4px 0; }
.ap-quote { margin:4px 0 0; font-size:.82rem; line-height:1.5; font-style:italic; border-left:3px solid var(--rule); padding:2px 0 2px 10px; }
.ap-dl { display:grid; grid-template-columns:max-content 1fr; gap:4px 12px; margin:0; }
.ap-dl dt { color:var(--mute); font-weight:600; } .ap-dl dd { margin:0; }
.ap-verdict-line { font-weight:600; color:var(--ink); }
.ap-trial { background:var(--soft); border:1px solid #E3E7EB; border-radius:10px; padding:12px; display:flex; flex-direction:column; gap:5px; min-height:44px; }
.ap-trial-head { display:flex; justify-content:space-between; gap:8px; }
.ap-trial-label { font-size:.9rem; font-weight:600; line-height:1.3; }
.ap-main .ap-trial-label a, .ap-main a.ap-ref-title { color:inherit; text-decoration:none; }
.ap-ref-n { font-family:var(--mono); font-size:.75rem; color:#143F8C; }
.ap-main ul.ap-refs { list-style:none; padding-left:0; }
.ap-main ul.ap-refs li { margin:8px 0; }
.ap-main dialog.ap-sheet { border:0; padding:16px 16px 28px; width:100%; max-width:760px; max-height:86vh; overflow-y:auto; border-radius:18px 18px 0 0; margin:auto auto 0; box-sizing:border-box; color:var(--ink); }
.ap-main dialog.ap-sheet[open] { display:flex; flex-direction:column; gap:12px; }
.ap-main dialog.ap-sheet::backdrop { background:rgba(21,32,43,.45); }
.ap-sheet-top { display:flex; justify-content:space-between; align-items:flex-start; gap:10px; }
.ap-sheet-top form { margin:0; }
.ap-sheet-ref { color:#143F8C; font-size:.75rem; }
.ap-main .ap-sheet-top h3 { font-size:1.05rem; line-height:1.3; margin-top:4px; }
.ap-main button.ap-close { width:44px; height:44px; padding:0; border-radius:12px; border:1px solid var(--rule); background:#fff; color:var(--ink); font-size:1rem; }
.ap-sheet-title { font-size:.85rem; line-height:1.45; color:var(--ink2); }
.ap-sheet-row { border-top:1px solid #EEF1F3; padding-top:10px; display:flex; flex-direction:column; gap:3px; font-size:.9rem; line-height:1.5; }
.ap-sheet-links { display:flex; flex-wrap:wrap; gap:10px; padding-top:6px; }
.ap-main a.ap-btn { font-size:.9rem; padding:10px 14px; border-radius:10px; border:1px solid var(--ink); color:var(--ink); text-decoration:none; min-height:44px; display:inline-flex; align-items:center; }
.ap-main a.ap-btn.dark { background:var(--ink); color:#fff; }
.ap-original { margin-top:18px; border:1px dashed #B9C2CB; border-radius:14px; padding:6px 14px; background:#fff; }
.ap-original > summary { cursor:pointer; font-weight:600; min-height:44px; display:flex; align-items:center; color:var(--accent); }
.ap-debug { font-family:var(--mono); font-size:.72rem; margin:8px 0 0; }
.run-fold { display:block; border:1px solid var(--line); border-radius:12px; background:var(--card); padding:10px 14px; margin:12px 0; }
.run-fold > summary { list-style:none; cursor:pointer; display:flex; justify-content:space-between; align-items:center; gap:8px; min-height:44px; }
.run-fold > summary::-webkit-details-marker { display:none; }
.run-fold > summary h3 { margin:0; font-size:1rem; }
.run-fold > summary .ap-more { flex:none; font-size:.82rem; font-weight:600; color:#1d5fd1; }
.run-fold > summary .ap-more::after { content:'Show all \25BE'; }
.run-fold[open] > summary .ap-more::after { content:'Show less \25B4'; }
.ap-main details.ap-fold > summary { list-style:none; cursor:pointer; display:block; }
.ap-main details.ap-fold > summary::-webkit-details-marker { display:none; }
.ap-main details.ap-fold > summary .ap-sec-head { display:flex; }
.ap-main details.ap-fold:not([open]) .ap-sec-head { border-bottom:0; }
.ap-main .ap-sec-h { font-size:.98rem; font-weight:600; line-height:1.3; color:var(--ink); }
.ap-main details.ap-fold > summary .ap-preview { padding:0 14px; margin-top:-4px; font-size:.88rem; line-height:1.5; color:var(--ink2); display:-webkit-box; -webkit-line-clamp:3; line-clamp:3; -webkit-box-orient:vertical; overflow:hidden; overflow-wrap:anywhere; }
.ap-main details.ap-fold > summary .ap-more { display:flex; align-items:center; min-height:44px; padding:0 14px 4px; font-size:.82rem; font-weight:600; color:#1d5fd1; }
.ap-main details.ap-fold > summary .ap-more::after { content:'Show all \25BE'; }
.ap-main details.ap-fold[open] > summary .ap-more::after { content:'Show less \25B4'; }
.ap-main details.ap-fold[open] > summary .ap-preview { display:none; }
.ap-main details.ap-fold[open] > summary .ap-more { padding:4px 14px; }
.ap-main .ap-copy { padding:4px 2px; }
.ap-main .ap-copy button.secondary { margin-left:0; }
@media (max-width:480px) { .ap-dl { grid-template-columns:1fr; } .ap-bar-row { grid-template-columns:minmax(0,96px) minmax(0,1fr) auto; } }
"#;

pub fn shell(options: LeptosOptions) -> impl IntoView {
    view! {
        <!DOCTYPE html>
        <html lang="en">
            <head>
                <meta charset="utf-8" />
                <meta name="viewport" content="width=device-width, initial-scale=1" />
                <style>{STYLE}</style>
                <style>{ivory::IVORY_STYLE}</style>
                <style>{tracker::TRACKER_STYLE}</style>
                <AutoReload options=options.clone() />
                <HydrationScripts options />
                <MetaTags />
            </head>
            <body>
                <App />
            </body>
        </html>
    }
}

// ---------- server functions (same pipeline as the JSON routes) ----------

/// Ask a question. Same pipeline as `POST /api/ask`; `mode` defaults to Focused.
#[server(prefix = "/api/fn", endpoint = "ask")]
pub async fn ask_question(
    question: String,
    #[server(default)] mode: RunMode,
) -> Result<AskOutcome, ServerFnError> {
    use crate::identity::CurrentUser;
    let (state, user) = signed_in().await?;
    let user = match user {
        Ok(CurrentUser(u)) => u,
        Err(rejection) => {
            crate::ask::audit_identity_rejection(&state, &rejection).await;
            set_status(rejection.status());
            return Err(ServerFnError::new(rejection.message()));
        }
    };
    crate::ask::run_ask(&state, &user, &question, mode)
        .await
        .map_err(|f| {
            set_status(axum::http::StatusCode::INTERNAL_SERVER_ERROR);
            ServerFnError::new(f.message())
        })
}

/// Who is signed in, and today's spend. Same as `GET /api/me`.
#[server(prefix = "/api/fn", endpoint = "me")]
pub async fn get_me() -> Result<MeView, ServerFnError> {
    use crate::identity::CurrentUser;
    let (state, user) = signed_in().await?;
    let CurrentUser(user) = user.map_err(|r| {
        set_status(r.status());
        ServerFnError::new(r.message())
    })?;
    crate::ask::me_view(&state, &user).await.map_err(|f| {
        set_status(axum::http::StatusCode::INTERNAL_SERVER_ERROR);
        ServerFnError::new(f.message())
    })
}

/// The audit log (owner only). Same as `GET /api/activity`.
#[server(prefix = "/api/fn", endpoint = "activity")]
pub async fn get_activity() -> Result<ActivityView, ServerFnError> {
    use crate::ask::ActivityError;
    use crate::identity::CurrentUser;
    let (state, user) = signed_in().await?;
    // Owner check here as well as on the JSON route.
    let CurrentUser(user) = user.map_err(|r| {
        set_status(r.status());
        ServerFnError::new(r.message())
    })?;
    crate::ask::activity_view(&state, &user, None)
        .await
        .map_err(|e| {
            set_status(match e {
                ActivityError::NotOwner => axum::http::StatusCode::FORBIDDEN,
                ActivityError::Failed(_) => axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            });
            ServerFnError::new(e.message())
        })
}

/// The signed-in user's runs, newest first. Same as `GET /api/runs`.
#[server(prefix = "/api/fn", endpoint = "my_runs")]
pub async fn list_my_runs() -> Result<Vec<RunSummary>, ServerFnError> {
    use crate::identity::CurrentUser;
    let (state, user) = signed_in().await?;
    let CurrentUser(user) = user.map_err(|r| {
        set_status(r.status());
        ServerFnError::new(r.message())
    })?;
    // My questions shows the most recent 100 (the API's maximum).
    crate::runs::api::list_view(&state, &user, Some(crate::runs::api::MAX_LIMIT))
        .await
        .map_err(run_api_error)
}

/// The signed-in user's runs as Library cards, newest first (read-only;
/// the same scoping and 100-run limit as [`list_my_runs`]). Nothing here
/// logs question or answer text.
#[server(prefix = "/api/fn", endpoint = "my_library")]
pub async fn list_my_library() -> Result<Vec<crate::library::LibraryCard>, ServerFnError> {
    use crate::identity::CurrentUser;
    let (state, user) = signed_in().await?;
    let CurrentUser(user) = user.map_err(|r| {
        set_status(r.status());
        ServerFnError::new(r.message())
    })?;
    crate::runs::api::library_view(&state, &user, Some(crate::runs::api::MAX_LIMIT))
        .await
        .map_err(run_api_error)
}

/// The Ask page's latest answers and Today counts for the signed-in user
/// (read-only; their own runs only; no spend).
#[server(prefix = "/api/fn", endpoint = "my_ask_overview")]
pub async fn get_my_ask_overview() -> Result<crate::library::AskOverview, ServerFnError> {
    use crate::identity::CurrentUser;
    let (state, user) = signed_in().await?;
    let CurrentUser(user) = user.map_err(|r| {
        set_status(r.status());
        ServerFnError::new(r.message())
    })?;
    crate::runs::api::ask_overview(&state, &user)
        .await
        .map_err(run_api_error)
}

/// One of the user's runs. Same as `GET /api/runs/{id}`; someone else's run is "not found".
#[server(prefix = "/api/fn", endpoint = "my_run")]
pub async fn get_my_run(run_id: Uuid) -> Result<RunView, ServerFnError> {
    use crate::identity::CurrentUser;
    let (state, user) = signed_in().await?;
    let CurrentUser(user) = user.map_err(|r| {
        set_status(r.status());
        ServerFnError::new(r.message())
    })?;
    let mut run = crate::runs::api::get_view(&state, &user, run_id)
        .await
        .map_err(run_api_error)?;
    // An error reading it reads as none: the page shows the run as before.
    run.intro = crate::runs::api::intro_view(&state, &user, run_id)
        .await
        .ok()
        .flatten();
    Ok(run)
}

/// The introduction of one of the user's runs, alone (the page's short wait
/// when the run ended before it did). Someone else's run is "not found".
#[server(prefix = "/api/fn", endpoint = "my_run_intro")]
pub async fn get_my_run_intro(
    run_id: Uuid,
) -> Result<Option<crate::runs::IntroView>, ServerFnError> {
    use crate::identity::CurrentUser;
    let (state, user) = signed_in().await?;
    let CurrentUser(user) = user.map_err(|r| {
        set_status(r.status());
        ServerFnError::new(r.message())
    })?;
    crate::runs::api::intro_view(&state, &user, run_id)
        .await
        .map_err(run_api_error)
}

/// Temporal T3: the expert opinion of one of the user's runs (`None`: not
/// run). Read-only; someone else's run is "not found". The run page's
/// Expert opinion option and the board page (`/run/:id/temporal`) read it.
#[server(prefix = "/api/fn", endpoint = "my_run_temporal")]
pub async fn get_my_run_temporal(
    run_id: Uuid,
) -> Result<Option<crate::runs::TemporalView>, ServerFnError> {
    use crate::identity::CurrentUser;
    let (state, user) = signed_in().await?;
    let CurrentUser(user) = user.map_err(|r| {
        set_status(r.status());
        ServerFnError::new(r.message())
    })?;
    crate::runs::api::temporal_view(&state, &user, run_id)
        .await
        .map_err(run_api_error)
}

/// The chat of one of the user's runs: `None` with `CHAT=off` (no chat query
/// is made). Same as `GET /api/runs/{id}/chat`; someone else's run is "not
/// found".
#[server(prefix = "/api/fn", endpoint = "my_run_chat")]
pub async fn get_my_run_chat(
    run_id: Uuid,
) -> Result<Option<crate::chat_page::ChatPageView>, ServerFnError> {
    let on = use_context::<crate::server::AppState>().is_some_and(|s| s.chat.on);
    if !on {
        return Ok(None);
    }
    let (state, user) = chat_signed_in().await?;
    crate::chat::service::state_for(&state, &user, run_id)
        .await
        .map(|v| Some(v.into()))
        .map_err(chat_error)
}

/// Ask a question of a run's evidence. Same as `POST /api/runs/{id}/chat`, as
/// a form post too: a refusal is a value the page shows in the conversation,
/// and a browser that has not hydrated is sent back to the run page. A starter
/// button sends its question as `starter`.
#[server(prefix = "/api/fn", endpoint = "chat_ask")]
pub async fn chat_ask(
    run_id: Uuid,
    #[server(default)] question: String,
    #[server(default)] mode: String,
    #[server(default)] starter: Option<String>,
) -> Result<crate::chat_page::AskReply, ServerFnError> {
    use crate::chat::{ChatMode, api, lines, service};
    use crate::chat_page::{self as cp, AskReply, ChatTurn, Refusal};
    let (state, user) = chat_signed_in().await?;
    let question = starter.filter(|s| !s.trim().is_empty()).unwrap_or(question);
    let asked = match ChatMode::parse(&mode) {
        Some(m) => service::ask(&state, &user, run_id, &question, m).await,
        // The question was readable, so it still gets the safety line.
        None => Err(service::ChatError::BadMode {
            safety_line: lines::warning_symptoms(&question),
        }),
    };
    Ok(match asked {
        Ok(a) => {
            if is_form_post() {
                leptos_axum::redirect(&cp::accepted_url(run_id));
            }
            AskReply::Accepted(ChatTurn::just_asked(
                a.turn_id,
                &question,
                &mode,
                a.safety_line,
            ))
        }
        Err(e) => {
            let (_, code, message, safety_line) = api::parts(e);
            if is_form_post() {
                // Fixed codes only: the question never goes in the address.
                let hints = cp::hint_codes(crate::deid::scan(&question).iter().map(|f| f.kind));
                leptos_axum::redirect(&cp::refused_url(run_id, code, safety_line, &hints));
            }
            AskReply::Refused(Refusal {
                question,
                message,
                safety_line,
            })
        }
    })
}

/// One of the user's turns. Same as `GET /api/chat/turns/{id}`.
#[server(prefix = "/api/fn", endpoint = "chat_turn")]
pub async fn chat_turn(turn_id: i64) -> Result<crate::chat_page::ChatTurn, ServerFnError> {
    let (state, user) = chat_signed_in().await?;
    crate::chat::service::turn_view(&state, &user, turn_id)
        .await
        .map(Into::into)
        .map_err(chat_error)
}

/// What a citation opens, if it is in one of the user's turns. Same rule as
/// `GET /api/chat/passages/{id}`.
#[server(prefix = "/api/fn", endpoint = "chat_source")]
pub async fn chat_source(stored_id: i64) -> Result<crate::chat_page::SheetSource, ServerFnError> {
    use crate::chat::store::CitedSource;
    use crate::chat_page::{paper_name, passage_sheet, qa_sheet};
    let (state, user) = chat_signed_in().await?;
    let source = crate::chat::service::cited_source(&state, &user, stored_id)
        .await
        .map_err(chat_error)?;
    Ok(match source {
        CitedSource::Passage {
            kind,
            text,
            references,
            ..
        } => passage_sheet(&kind, &text, paper_name(&references).as_deref()),
        CitedSource::Qa { text, .. } => qa_sheet(&text),
    })
}

/// A thumb on the user's own turn. Same as `POST /api/chat/turns/{id}/feedback`.
#[server(prefix = "/api/fn", endpoint = "chat_feedback")]
pub async fn chat_feedback(run_id: Uuid, turn_id: i64, value: i64) -> Result<(), ServerFnError> {
    let (state, user) = chat_signed_in().await?;
    chat_action_done(
        run_id,
        crate::chat::service::give_feedback(&state, &user, turn_id, value).await,
    )
}

/// Suggest the user's own finished turn for the library. Same as
/// `POST /api/chat/turns/{id}/suggest`.
#[server(prefix = "/api/fn", endpoint = "chat_suggest")]
pub async fn chat_suggest(run_id: Uuid, turn_id: i64) -> Result<(), ServerFnError> {
    let (state, user) = chat_signed_in().await?;
    chat_action_done(
        run_id,
        crate::chat::service::suggest(&state, &user, turn_id)
            .await
            .map(|_| ()),
    )
}

/// Delete one of the user's runs (cancelling it if active). Same as `DELETE /api/runs/{id}`.
#[server(prefix = "/api/fn", endpoint = "delete_my_run")]
pub async fn delete_my_run(run_id: Uuid) -> Result<(), ServerFnError> {
    use crate::identity::CurrentUser;
    let (state, user) = signed_in().await?;
    let CurrentUser(user) = user.map_err(|r| {
        set_status(r.status());
        ServerFnError::new(r.message())
    })?;
    crate::runs::api::delete_one(&state, &user, run_id)
        .await
        .map_err(run_api_error)
}

/// Delete all of the user's runs; returns how many. Same as `DELETE /api/runs`.
#[server(prefix = "/api/fn", endpoint = "delete_my_runs")]
pub async fn delete_my_runs() -> Result<u64, ServerFnError> {
    use crate::identity::CurrentUser;
    let (state, user) = signed_in().await?;
    let CurrentUser(user) = user.map_err(|r| {
        set_status(r.status());
        ServerFnError::new(r.message())
    })?;
    crate::runs::api::delete_all(&state, &user)
        .await
        .map_err(run_api_error)
}

#[cfg(feature = "ssr")]
fn run_api_error(e: crate::runs::api::RunApiError) -> ServerFnError {
    use crate::runs::api::RunApiError;
    set_status(match e {
        RunApiError::NotFound => axum::http::StatusCode::NOT_FOUND,
        RunApiError::Failed => axum::http::StatusCode::INTERNAL_SERVER_ERROR,
    });
    ServerFnError::new(e.message())
}

/// The state and user of a chat call. With `CHAT=off` these functions behave
/// as the REST routes do there: nothing is known.
#[cfg(feature = "ssr")]
async fn chat_signed_in() -> Result<(crate::server::AppState, crate::identity::User), ServerFnError>
{
    use crate::identity::CurrentUser;
    let (state, user) = signed_in().await?;
    if !state.chat.on {
        set_status(axum::http::StatusCode::NOT_FOUND);
        return Err(ServerFnError::new(crate::chat::lines::NOT_FOUND_MESSAGE));
    }
    let CurrentUser(user) = user.map_err(|r| {
        set_status(r.status());
        ServerFnError::new(r.message())
    })?;
    Ok((state, user))
}

/// A chat refusal as a server-function error: the REST routes' status and
/// message.
#[cfg(feature = "ssr")]
fn chat_error(e: crate::chat::service::ChatError) -> ServerFnError {
    let (status, _, message, _) = crate::chat::api::parts(e);
    set_status(status);
    ServerFnError::new(message)
}

/// A plain form post from a page that has not hydrated (the page's own code
/// never asks for `text/html`).
#[cfg(feature = "ssr")]
fn is_form_post() -> bool {
    use_context::<axum::http::request::Parts>().is_some_and(|p| {
        p.headers
            .get(axum::http::header::ACCEPT)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v.contains("text/html"))
    })
}

/// The end of a thumb or a suggestion: a form post goes back to the chat, with
/// the refusal's code if there was one.
#[cfg(feature = "ssr")]
fn chat_action_done(
    run_id: Uuid,
    done: Result<(), crate::chat::service::ChatError>,
) -> Result<(), ServerFnError> {
    use crate::chat_page as cp;
    match (done, is_form_post()) {
        (Ok(()), true) => {
            leptos_axum::redirect(&cp::chat_url(run_id));
            Ok(())
        }
        (Ok(()), false) => Ok(()),
        (Err(e), true) => {
            let (_, code, _, safety) = crate::chat::api::parts(e);
            leptos_axum::redirect(&cp::refused_url(run_id, code, safety, &[]));
            Ok(())
        }
        (Err(e), false) => Err(chat_error(e)),
    }
}

#[cfg(feature = "ssr")]
type SignedIn = (
    crate::server::AppState,
    Result<crate::identity::CurrentUser, crate::identity::IdentityRejection>,
);

/// The app state and the identity check result for this request.
#[cfg(feature = "ssr")]
async fn signed_in() -> Result<SignedIn, ServerFnError> {
    let state = use_context::<crate::server::AppState>()
        .ok_or_else(|| ServerFnError::new("Incisive isn't ready yet. Please try again."))?;
    let user = leptos_axum::extract_with_state::<
        Result<crate::identity::CurrentUser, crate::identity::IdentityRejection>,
        _,
    >(&state)
    .await
    .map_err(|_| ServerFnError::new("Incisive couldn't read this request. Please try again."))?;
    Ok((state, user))
}

/// Server-function URL prefix (see the `#[server]` attributes above).
#[cfg(feature = "ssr")]
const FN_PREFIX: &str = "/api/fn/";

/// Set the HTTP status, but only when this is a direct server-function call
/// (`/api/fn/...`). The same functions also run while a page renders on the
/// server, and there the page's own status must stay as it is.
#[cfg(feature = "ssr")]
fn set_status(code: axum::http::StatusCode) {
    let is_fn_call = use_context::<axum::http::request::Parts>()
        .is_some_and(|p| p.uri.path().starts_with(FN_PREFIX));
    if !is_fn_call {
        return;
    }
    if let Some(opts) = use_context::<leptos_axum::ResponseOptions>() {
        opts.set_status(code);
    }
}

/// The plain message inside a server-function error.
fn error_text(e: &ServerFnError) -> String {
    match e {
        ServerFnError::ServerError(m) => m.clone(),
        _ => "Incisive couldn't reach the server. Check your connection and try again.".to_string(),
    }
}

// ---------- pages ----------

#[component]
pub fn App() -> impl IntoView {
    provide_meta_context();
    let me: MeResource = Resource::new(|| (), |_| get_me());
    // The header reads it (one `/api/fn/me` per load).
    provide_context(me);
    view! {
        <Title text="Incisive" />
        <Router>
            <ivory::SiteHeader />
            <ivory::PageMain>
                <Routes fallback=|| view! { <p>"Page not found. Go back to the question box."</p> }>
                    <Route path=path!("") view=ask_page::AskPage />
                    <Route path=path!("runs/:id") view=RunPage />
                    <Route path=path!("run/:id/temporal") view=expert::TemporalPage />
                    <Route path=path!("history") view=library_page::LibraryPage />
                    <Route path=path!("explore") view=explore::ExplorePage />
                    <Route path=path!("activity") view=ActivityPage />
                </Routes>
            </ivory::PageMain>
        </Router>
    }
}

/// Who is signed in (provided by [`App`]; the header reads it).
type MeResource = Resource<Result<MeView, ServerFnError>>;

/// The depth this browser last chose (`None` when storage is absent, blocked
/// or throws; browser only).
fn load_mode() -> Option<String> {
    #[cfg(feature = "hydrate")]
    {
        // Every step returns a Result or Option: a storage exception is caught
        // by web-sys and ends here as `None`.
        web_sys::window()?
            .local_storage()
            .ok()??
            .get_item(answer::MODE_STORAGE_KEY)
            .ok()?
    }
    #[cfg(not(feature = "hydrate"))]
    None
}

/// Remember the depth choice in this browser; any storage failure is ignored.
fn save_mode(m: RunMode) {
    #[cfg(feature = "hydrate")]
    {
        if let Some(s) = web_sys::window().and_then(|w| w.local_storage().ok().flatten()) {
            let _ = s.set_item(answer::MODE_STORAGE_KEY, m.as_str());
        }
    }
    #[cfg(not(feature = "hydrate"))]
    let _ = m;
}

fn outcome_view(r: Result<AskOutcome, ServerFnError>) -> AnyView {
    match r {
        Ok(AskOutcome::BlockedIdentifiers { findings, message }) => {
            let mut labels: Vec<String> = Vec::new();
            for f in findings {
                if !labels.contains(&f.label) {
                    labels.push(f.label);
                }
            }
            view! {
                <div class="box red" role="alert">
                    <p>{message}</p>
                    <ul>{labels.into_iter().map(|l| view! { <li>{l}</li> }).collect_view()}</ul>
                </div>
            }
            .into_any()
        }
        Ok(
            AskOutcome::InvalidInput { message }
            | AskOutcome::NotMedical { message }
            | AskOutcome::Unclear { message }
            | AskOutcome::OverCap { message }
            | AskOutcome::GateUnavailable { message }
            | AskOutcome::RunInProgress { message },
        ) => view! {
            <div class="box amber" role="status">
                <p>{message}</p>
            </div>
        }
        .into_any(),
        Ok(AskOutcome::RunStarted { message, alert, .. }) => view! {
            <div class="box green" role="status">
                // The API text names "My questions"; the page is now the Library.
                <p>{message.replace("in My questions.", "in your Library.")}</p>
                {alert.map(|a| view! { <p>{a}</p> })}
            </div>
        }
        .into_any(),
        Err(e) => view! {
            <div class="box amber" role="alert">
                <p>{error_text(&e)}</p>
            </div>
        }
        .into_any(),
    }
}

fn status_label(s: RunStatus) -> &'static str {
    match s {
        RunStatus::Queued => "Waiting to start",
        RunStatus::Running => "Working",
        RunStatus::Done => "Done",
        RunStatus::Partial => "Partly done",
        RunStatus::Failed => "Didn't finish",
        RunStatus::Interrupted => "Interrupted",
    }
}

fn status_badge(s: RunStatus) -> impl IntoView {
    view! { <span class=format!("badge {}", s.as_str())>{status_label(s)}</span> }
}

fn rupees(paise: i64) -> String {
    answer::rupees(paise)
}

/// How often the run page asks for news while the run is queued or running.
const POLL_EVERY: std::time::Duration = std::time::Duration::from_secs(2);

/// How many times, one per poll interval, the page asks for a just-ended Full
/// run's expert opinion row.
const ROW_WAIT_ATTEMPTS: u32 = 10;

/// The wait for the expert opinion row: once per page.
#[derive(Default)]
struct RowWait {
    started: bool,
    attempts: u32,
    timer: Option<leptos::leptos_dom::helpers::IntervalHandle>,
}

/// A row can still come: a Full run that has ended with a written answer.
fn row_is_due(run: &RunView) -> bool {
    run.summary.mode == RunMode::Focused
        && matches!(run.summary.status, RunStatus::Done | RunStatus::Partial)
        && run.answer_version == crate::agent::answer_v3::ANSWER_VERSION
        && run
            .answer
            .as_ref()
            .is_some_and(|a| !a.get("answer").is_none_or(serde_json::Value::is_null))
}

/// Ask only for the row, a fixed number of times; the page is left alone
/// (nothing the surgeon opened closes) and redrawn once, when a row appears.
fn wait_for_row(id: Uuid, tick: RwSignal<u32>, wait: StoredValue<RowWait, LocalStorage>) {
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
                .unwrap_or(ROW_WAIT_ATTEMPTS);
            if attempts >= ROW_WAIT_ATTEMPTS {
                stop();
            }
            leptos::task::spawn_local(async move {
                if let Ok(Some(_)) = get_my_run_temporal(id).await {
                    stop();
                    let _ = tick.try_update(|t| *t += 1);
                }
            });
        },
        POLL_EVERY,
    );
    wait.update_value(|w| {
        w.started = true;
        w.timer = h.ok();
    });
}

#[component]
fn RunPage() -> impl IntoView {
    let params = use_params_map();
    let run_id = move || {
        params
            .read()
            .get("id")
            .and_then(|s| Uuid::parse_str(&s).ok())
    };
    let query = use_query_map();
    let tick = RwSignal::new(0u32);
    let ui = tracker::TrackerUi::new();
    let intro_ui = intro::IntroUi::new();
    let data = Resource::new(
        move || (run_id(), tick.get()),
        |(id, _)| async move {
            match id {
                Some(id) => run_with_temporal(id).await,
                None => Err(ServerFnError::new(RUN_NOT_FOUND_MESSAGE)),
            }
        },
    );

    // Poll while queued or running. Stop only at a terminal status or when
    // the run is not found (deleted); any other error is transient: keep
    // polling and keep showing the last good view. Effects run only in the
    // browser, so the server never starts a timer.
    let timer: StoredValue<Option<leptos::leptos_dom::helpers::IntervalHandle>, LocalStorage> =
        StoredValue::new_local(None);
    let last_good: RwSignal<Option<RunPageData>> = RwSignal::new(None);
    let live_draft: RwSignal<Option<String>> = RwSignal::new(None);
    let row_wait: StoredValue<RowWait, LocalStorage> = StoredValue::new_local(RowWait::default());
    let intro_wait: StoredValue<RowWait, LocalStorage> = StoredValue::new_local(RowWait::default());
    on_cleanup(move || {
        for wait in [row_wait, intro_wait] {
            if let Some(h) = wait.try_update_value(|w| w.timer.take()).flatten() {
                h.clear();
            }
        }
    });
    let stop = move || {
        if let Some(h) = timer.try_update_value(|t| t.take()).flatten() {
            h.clear();
        }
    };
    // Stage S1: while a draft shows, a poll asks without touching the resource,
    // so the page is drawn again only if something besides the draft changed.
    let draft_poll = StoredValue::new(false);
    let poll = move || {
        let drawn = last_good.with_untracked(|l| l.as_ref().map(|d| d.run.summary.id));
        let Some((id, shown)) = drawn.zip(live_draft.get_untracked()) else {
            tick.update(|t| *t += 1);
            return;
        };
        if draft_poll.get_value() {
            return;
        }
        draft_poll.set_value(true);
        leptos::task::spawn_local(async move {
            let now = get_my_run(id).await;
            let _ = draft_poll.try_update_value(|b| *b = false);
            let step = now.ok().and_then(|now| {
                last_good.try_with_untracked(|l| {
                    l.as_ref()
                        .map(|d| crate::draft_page::poll_step(&d.run, &shown, &now))
                })?
            });
            match step {
                Some(crate::draft_page::PollStep::Same) => {}
                Some(crate::draft_page::PollStep::Draft(text)) => {
                    // Kept for the error view, which draws the last good page.
                    let _ = last_good.try_update_untracked(|l| {
                        if let Some(d) = l {
                            d.run.draft = Some(text.clone());
                        }
                    });
                    let _ = live_draft.try_set(Some(text));
                }
                _ => {
                    let _ = tick.try_update(|t| *t += 1);
                }
            }
        });
    };
    let start = move || {
        if timer.with_value(|t| t.is_none()) {
            let h = leptos::leptos_dom::helpers::set_interval_with_handle(poll, POLL_EVERY);
            timer.set_value(h.ok());
        }
    };
    // A link to `/runs/<id>#ref-n` (Explore's "Open the study sheet", the
    // board's citations) opens that sheet once the run is drawn: the
    // reference itself sits in a closed card. Once per run.
    let hash_followed: StoredValue<Option<Uuid>> = StoredValue::new(None);
    Effect::new(move |_| match data.get() {
        Some(Ok(v)) => {
            if hash_followed.get_value() != Some(v.run.summary.id) {
                hash_followed.set_value(Some(v.run.summary.id));
                // Effects run in the browser only (the hash never reaches
                // the server).
                if let Some(n) = page::sheet_in_hash(&expert::current_hash()) {
                    // On the next frame: the sheet is drawn by then.
                    request_animation_frame(move || {
                        page::open_sheet(n);
                    });
                }
            }
            let active = v.run.summary.status.is_active();
            // A Full run that ended with a written answer: its expert opinion
            // row comes moments later.
            let row_due = v.temporal.is_none() && row_is_due(&v.run);
            let id = v.run.summary.id;
            intro_ui.observe(active, v.run.intro.as_ref());
            // The run ended before its introduction did.
            let intro_due = !active && v.run.intro == Some(crate::runs::IntroView::Pending);
            last_good.set(Some(v));
            if active {
                start()
            } else {
                stop()
            }
            if row_due {
                wait_for_row(id, tick, row_wait);
            }
            if intro_due {
                intro::wait_for_intro(id, tick, intro_wait);
            }
        }
        Some(Err(e)) if is_not_found(&e) => stop(),
        Some(Err(_)) => start(),
        None => {}
    });
    on_cleanup(stop);

    view! {
        <p class="run-back">
            <A href="/history">"\u{2190} Library"</A>
        </p>
        <Transition fallback=|| view! { <p>"Loading…"</p> }>
            {move || {
                data.get()
                    .map(|r| match r {
                        Ok(RunPageData { run, temporal, chat }) => {
                            run_view(run, temporal, chat, query, ui, intro_ui, live_draft).into_any()
                        }
                        Err(e) if is_not_found(&e) => {
                            view! { <p class="box amber" role="alert">{error_text(&e)}</p> }.into_any()
                        }
                        Err(e) => match last_good.get() {
                            Some(RunPageData { run, temporal, chat }) => view! {
                                <p class="hint" role="status">
                                    "Connection problem \u{2014} retrying. " {error_text(&e)}
                                </p>
                                {run_view(run, temporal, chat, query, ui, intro_ui, live_draft)}
                            }
                                .into_any(),
                            None => view! {
                                <p class="box amber" role="alert">
                                    {error_text(&e)} " Retrying\u{2026}"
                                </p>
                            }
                                .into_any(),
                        },
                    })
            }}
        </Transition>
    }
}

/// A run and, once it has ended, its expert opinion row (Temporal T4).
///
/// On the wire (the run resource's hydration payload) a run with no row is
/// the bare `RunView`, exactly as before T4; one with a row is
/// `{"t4_run": RunView, "t4_temporal": TemporalView}`, with `t4_chat` only
/// when `CHAT=on`.
#[derive(Clone)]
struct RunPageData {
    run: RunView,
    temporal: Option<crate::runs::TemporalView>,
    chat: Option<crate::chat_page::ChatPageView>,
}

impl serde::Serialize for RunPageData {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        #[derive(serde::Serialize)]
        struct With<'a> {
            t4_run: &'a RunView,
            t4_temporal: &'a crate::runs::TemporalView,
            #[serde(skip_serializing_if = "Option::is_none")]
            t4_chat: Option<&'a crate::chat_page::ChatPageView>,
        }
        match &self.temporal {
            None => self.run.serialize(s),
            Some(t) => With {
                t4_run: &self.run,
                t4_temporal: t,
                t4_chat: self.chat.as_ref(),
            }
            .serialize(s),
        }
    }
}

impl<'de> serde::Deserialize<'de> for RunPageData {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        // Short-lived: read once, then taken apart.
        #[allow(clippy::large_enum_variant)]
        #[derive(serde::Deserialize)]
        #[serde(untagged)]
        enum Wire {
            With {
                t4_run: RunView,
                t4_temporal: crate::runs::TemporalView,
                #[serde(default)]
                t4_chat: Option<crate::chat_page::ChatPageView>,
            },
            Plain(RunView),
        }
        Ok(match Wire::deserialize(d)? {
            Wire::With {
                t4_run,
                t4_temporal,
                t4_chat,
            } => RunPageData {
                run: t4_run,
                temporal: Some(t4_temporal),
                chat: t4_chat,
            },
            Wire::Plain(run) => RunPageData {
                run,
                temporal: None,
                chat: None,
            },
        })
    }
}

/// The run, its temporal row once the run has ended, and with `CHAT=on` its
/// chat if it has a row (an error reading either reads as none: the page shows
/// the answer as before).
async fn run_with_temporal(id: Uuid) -> Result<RunPageData, ServerFnError> {
    let v = get_my_run(id).await?;
    let t = if v.summary.status.is_active() || v.summary.mode != RunMode::Focused {
        None
    } else {
        get_my_run_temporal(id).await.ok().flatten()
    };
    let chat = if t.is_some() {
        get_my_run_chat(id).await.ok().flatten()
    } else {
        None
    };
    Ok(RunPageData {
        run: v,
        temporal: t,
        chat,
    })
}

/// The run is gone (or someone else's): polling stops.
fn is_not_found(e: &ServerFnError) -> bool {
    error_text(e) == RUN_NOT_FOUND_MESSAGE
}

fn run_view(
    v: RunView,
    temporal: Option<crate::runs::TemporalView>,
    chat: Option<crate::chat_page::ChatPageView>,
    query: Memo<leptos_router::params::ParamsMap>,
    ui: tracker::TrackerUi,
    intro_ui: intro::IntroUi,
    live_draft: RwSignal<Option<String>>,
) -> AnyView {
    let status = v.summary.status;
    let run_id = v.summary.id;
    let intro_row = v.intro;
    // Stage S1: the answer as far as it is written (the API sends it only
    // while the run is running with no answer).
    let draft_text = v.draft.filter(|t| {
        status.is_active() && crate::draft_page::has_content(&crate::draft_page::read_answer(t))
    });
    let ended_without_answer = v.answer.is_none() && !status.is_active();
    // The stored body is drawn from its fields, never shown raw: Phase 3
    // answers (version 3) with their own view, everything else as Phase 2.
    let answer = v.answer.map(|a| {
        if v.answer_version == crate::agent::answer_v3::ANSWER_VERSION {
            serde_json::from_value::<AnswerV3>(a).ok().map(Body::V3)
        } else {
            serde_json::from_value::<Answer>(a).ok().map(Body::V2)
        }
    });
    // The header is the full question once the answer holds it (§4), else
    // the stored 80-character title. Plain text: Leptos escapes it.
    let heading = answer
        .as_ref()
        .and_then(Option::as_ref)
        .map_or(v.summary.title, |a| match a {
            Body::V2(a) => a.question.clone(),
            Body::V3(a) => a.question.clone(),
        });
    // Temporal T4: the reference numbers whose study sheets the answer draws.
    let answer_refs: std::collections::BTreeSet<u16> =
        match answer.as_ref().and_then(Option::as_ref) {
            Some(Body::V3(a)) => a.references.iter().map(|r| r.n).collect(),
            _ => Default::default(),
        };
    let has_answer = answer.is_some();
    let elapsed_s = match answer.as_ref().and_then(Option::as_ref) {
        Some(Body::V2(a)) => Some(a.elapsed_s),
        Some(Body::V3(a)) => Some(a.elapsed_s),
        None => None,
    };
    // How each study was read (only a finished, readable Phase 3 answer).
    let evidence: Vec<(String, u32)> = match answer.as_ref() {
        Some(Some(Body::V3(a))) => a
            .evidence
            .iter()
            .map(|e| (e.text.clone(), e.verified_quotes))
            .collect(),
        _ => Vec::new(),
    };
    // Written: a stored body that holds an answer, not just any body.
    let (written, empty_reason) = match answer.as_ref() {
        Some(Some(Body::V3(a))) => (a.answer.is_some(), a.empty_reason.clone()),
        Some(Some(Body::V2(_))) => (true, None),
        Some(None) => (
            false,
            Some("This answer can't be shown. Please ask the question again.".to_string()),
        ),
        None => (false, None),
    };
    let answer_block = answer.map(|a| match a {
        Some(Body::V2(a)) => answer_view(a).into_any(),
        Some(Body::V3(a)) => page::answer_page_view(a).into_any(),
        None => view! {
            <p class="box amber" role="status">
                "This answer can't be shown. Please ask the question again."
            </p>
        }
        .into_any(),
    });
    let ended = ended_without_answer.then(|| {
        view! {
            <p class="box amber" role="status">
                "This question ended without an answer. You can ask it again from the Ask page."
            </p>
        }
    });
    let mode = v.summary.mode;
    // The tracker: from the stored step lines and the status only.
    let working = status.is_active();
    let answered = matches!(status, RunStatus::Done | RunStatus::Partial) && written;
    let ended_empty = matches!(status, RunStatus::Done | RunStatus::Partial) && !written;
    let run_state = if status == RunStatus::Queued {
        crate::run_tracker::RunState::Waiting
    } else if working {
        crate::run_tracker::RunState::Working
    } else if answered {
        crate::run_tracker::RunState::Answered
    } else if ended_empty {
        crate::run_tracker::RunState::EndedEmpty
    } else {
        crate::run_tracker::RunState::Stopped
    };
    let created_at = v.summary.created_at;
    let no_steps = v.steps.is_empty();
    // The clock starts at the first step, as `elapsed_s` nearly does, so a
    // queue wait cannot make it step back at the end.
    let clock_from = v.steps.first().map_or(created_at, |s| s.at);
    let last_step_at = v
        .steps
        .iter()
        .rfind(|s| !crate::run_tracker::is_expert_first_words(&s.line))
        .map_or(created_at, |s| s.at);
    let total_secs = elapsed_s.map_or((last_step_at - clock_from).num_seconds(), |s| s as i64);
    let mut read = {
        let lines: Vec<crate::run_tracker::Step<'_>> = v
            .steps
            .iter()
            .map(|s| crate::run_tracker::Step {
                at: s.at,
                line: s.line.as_str(),
            })
            .collect();
        crate::run_tracker::read(mode, &lines, run_state, empty_reason.as_deref())
    };
    if answered {
        let studies: Vec<crate::run_tracker::StudyRead<'_>> = evidence
            .iter()
            .map(|(text, verified_quotes)| crate::run_tracker::StudyRead {
                text,
                verified_quotes: *verified_quotes,
            })
            .collect();
        read.read_from_evidence(&studies);
    }
    let answer_pill = tracker::AnswerPill {
        state: if answered {
            crate::run_tracker::PillState::Ready
        } else if status == RunStatus::Queued {
            crate::run_tracker::PillState::Idle
        } else if working {
            crate::run_tracker::PillState::Filling
        } else {
            crate::run_tracker::PillState::Stopped
        },
        fill: read.fill,
        ready_at: last_step_at,
    };
    let progress = if working {
        tracker::progress_pill(ui, &read, v.steps)
    } else {
        progress_view(status, has_answer, v.steps)
    };
    let card = (read.recognised || status == RunStatus::Queued).then(|| {
        tracker::results_card(
            ui,
            mode,
            read,
            working,
            status == RunStatus::Queued || no_steps,
            clock_from,
            total_secs,
        )
    });
    // Stage S1: once the first words are there, the card and the introduction
    // fold into rows, so the answer sits near the top.
    let drafting = draft_text.is_some();
    let card = if drafting {
        card.map(|c| draft::fold_results(ui, clock_from, c))
    } else {
        card
    };
    // In front of the card, in every layout below; nothing without a text.
    let card = intro::with_results(intro_ui, working && !drafting, intro_row, card);
    if live_draft.get_untracked() != draft_text {
        live_draft.set(draft_text);
    }
    let answer_block = answer_block.or_else(|| drafting.then(|| draft::answer_view(live_draft)));
    let Some(t) = temporal else {
        return view! {
            // Page chrome (ivory redesign): Figtree; the answer below is untouched.
            <div class="run-chrome">
                <h1 class="run-question">{heading}</h1>
                <p>
                    {status_badge(status)} " " <span class="badge mode">{mode_label(mode)}</span>
                </p>
                {tracker::plain_bar(ui, mode, answer_pill)}
                {card}
                {progress}
            </div>
            {answer_block}
            {ended}
        }
        .into_any();
    };
    let x = expert::Expert::new(run_id, t, answer_refs, ui, answer_pill);
    // With `CHAT=on`: the third option and its view, in the same wrapper. A
    // run chat will never be offered on has none of it (one whose opinion is
    // still being written may yet), unless a refusal must still show.
    let asked = query.with_untracked(|q| {
        chat::ChatQuery::read(
            q.get("chat_wait"),
            q.get("chat_refused"),
            q.get("chat_safety"),
            q.get("chat_hints"),
        )
    });
    let chat = chat.filter(|c| {
        c.state != crate::chat_page::ChatState::NotOffered
            || x.opinion_writing_now()
            || asked.refused.is_some()
    });
    if let Some(chat) = chat {
        let c = chat::Chat::new(run_id, chat, asked, x);
        return view! {
            <div class=x.wrapper_class() class:rx-expon=move || x.expert.get() class:rx-chaton=move || c.open.get()>
                <style>{expert::EXPERT_STYLE}</style>
                <style>{chat::CHAT_STYLE}</style>
                <div class="run-chrome">
                    <h1 class="run-question">{heading}</h1>
                    <p>
                        {status_badge(status)} " " <span class="badge mode">{mode_label(mode)}</span>
                    </p>
                    {expert::option_bar(x, Some(c))}
                    {card}
                    {progress}
                </div>
                {expert::expert_view(x)}
                {c.section()}
                <div class="rx-answer" hidden=move || x.expert.get() || c.open.get()>
                    {answer_block}
                    {ended}
                </div>
            </div>
        }
        .into_any();
    }
    // The answer is drawn unchanged inside a wrapper the Expert opinion view
    // hides (outside `.ap-main`).
    view! {
        <div class=x.wrapper_class() class:rx-expon=move || x.expert.get()>
            <style>{expert::EXPERT_STYLE}</style>
            <div class="run-chrome">
                <h1 class="run-question">{heading}</h1>
                <p>
                    {status_badge(status)} " " <span class="badge mode">{mode_label(mode)}</span>
                </p>
                {expert::option_bar(x, None)}
                {card}
                {progress}
            </div>
            {expert::expert_view(x)}
            <div class="rx-answer" hidden=move || x.expert.get()>
                {answer_block}
                {ended}
            </div>
        </div>
    }
    .into_any()
}

/// Progress. While the run is queued or running, or when it ended with no
/// answer: the heading and the steps, as before. Once it has ended with an
/// answer (folding follow-up item 1):
/// the same heading and steps inside one native `<details>`, closed, its
/// summary "Progress · N steps · <status>" with "Show all"/"Show less"
/// (CSS); the steps' text is unchanged and opens with no script.
fn progress_view(
    status: RunStatus,
    has_answer: bool,
    steps: Vec<crate::runs::StepLine>,
) -> AnyView {
    let n = steps.len();
    let list = |steps: Vec<crate::runs::StepLine>| {
        view! {
            <ol class="steps">
                {steps.into_iter().map(|s| view! { <li>{s.line}</li> }).collect_view()}
            </ol>
        }
    };
    // Open as before while running, on a failed or interrupted run, and on
    // any run that ended with no answer: its steps say why.
    let finished_with_answer = matches!(status, RunStatus::Done | RunStatus::Partial) && has_answer;
    if !finished_with_answer || steps.is_empty() {
        return view! {
            <h3>"Progress"</h3>
            {if steps.is_empty() {
                view! { <p class="hint">"Nothing yet. This page updates by itself."</p> }.into_any()
            } else {
                list(steps).into_any()
            }}
        }
        .into_any();
    }
    let head = format!(
        "Progress \u{b7} {n} {} \u{b7} {}",
        if n == 1 { "step" } else { "steps" },
        status_label(status)
    );
    view! {
        <details class="run-fold">
            <summary>
                <h3>{head}</h3>
                <span class="ap-more" aria-hidden="true"></span>
            </summary>
            {list(steps)}
        </details>
    }
    .into_any()
}

/// A stored answer, by format. Short-lived (one per page render), so the
/// size difference between the variants does not matter.
#[allow(clippy::large_enum_variant)]
enum Body {
    V2(Answer),
    V3(AnswerV3),
}

/// `https://pubmed.ncbi.nlm.nih.gov/<pmid>/` for an all-digit PMID.
fn pubmed_link(pmid: &str) -> Option<String> {
    (!pmid.is_empty() && pmid.chars().all(|c| c.is_ascii_digit()))
        .then(|| format!("https://pubmed.ncbi.nlm.nih.gov/{pmid}/"))
}

/// `https://doi.org/<doi>`, percent-encoded except for `/`.
fn doi_link(doi: &str) -> Option<String> {
    let doi = doi.trim();
    (!doi.is_empty()).then(|| {
        let path: Vec<String> = doi.split('/').map(answer::percent_encode).collect();
        format!("https://doi.org/{}", path.join("/"))
    })
}

fn reference_view(r: Reference) -> impl IntoView {
    let mut head = r.title.trim_end_matches('.').to_string();
    head.push('.');
    match (&r.journal, r.year) {
        (Some(j), Some(y)) => head.push_str(&format!(" {j}, {y}.")),
        (Some(j), None) => head.push_str(&format!(" {j}.")),
        (None, Some(y)) => head.push_str(&format!(" {y}.")),
        (None, None) => {}
    }
    let pmid = r
        .pmid
        .clone()
        .and_then(|p| pubmed_link(&p).map(|href| (p, href)));
    let doi = r
        .doi
        .clone()
        .and_then(|d| doi_link(&d).map(|href| (d, href)));
    view! {
        <li>
            {format!("[{}] ", r.ref_id)}
            {head}
            {pmid.map(|(p, href)| view! { " " <a href=href rel="noopener noreferrer" target="_blank">{format!("PMID {p}")}</a> })}
            {doi.map(|(d, href)| view! { " " <a href=href rel="noopener noreferrer" target="_blank">{format!("DOI {d}")}</a> })}
            " " <span class="conf">{format!("({})", r.study_type)}</span>
        </li>
    }
}

/// The Edit & re-run fragment of the current address, if any, which is then
/// cleared (browser only; `None` elsewhere).
fn take_rerun_fragment() -> Option<(String, RunMode)> {
    #[cfg(feature = "hydrate")]
    {
        let w = web_sys::window()?;
        let found = answer::parse_rerun_fragment(&w.location().hash().ok()?)?;
        // Keep the question out of reloads, history and shared links.
        if let Ok(h) = w.history() {
            let _ = h.replace_state_with_url(&wasm_bindgen::JsValue::NULL, "", Some("/"));
        }
        Some(found)
    }
    #[cfg(not(feature = "hydrate"))]
    None
}

/// Copy the plain-text answer (browser only).
fn copy_text(text: &str) {
    #[cfg(feature = "hydrate")]
    {
        // The promise is not awaited: a refused copy changes nothing.
        let _ = web_sys::window().map(|w| w.navigator().clipboard().write_text(text));
    }
    #[cfg(not(feature = "hydrate"))]
    let _ = text;
}

/// A stored answer's existing Copy-as-text (the Library's Copy): Phase 3
/// answers' `copy_text`, Phase 2 answers' `render_text`. `None` when the
/// run has no readable answer.
fn copy_text_of(v: RunView) -> Option<String> {
    let a = v.answer?;
    if v.answer_version == crate::agent::answer_v3::ANSWER_VERSION {
        serde_json::from_value::<AnswerV3>(a)
            .ok()
            .map(|a| a.copy_text())
    } else {
        serde_json::from_value::<Answer>(a)
            .ok()
            .map(|a| render_text(&a))
    }
}

/// The answer, in the §4 order, as escaped text and links only.
fn answer_view(a: Answer) -> impl IntoView {
    let text = render_text(&a);
    let rerun = rerun_href(&a.question, a.mode);
    let empty = empty_bottom_line(&a);
    let insufficient = show_insufficient(&a);
    let pico = a.pico.clone();
    let check = check_text(&a.check);
    let t = a.trail.clone();
    let screened = screened_text(&t);
    let exclusions = exclusions_text(&t);
    let stopped = t.stopped_early.as_deref().map(stopped_text);
    let unavailable = unavailable_text(&t);
    let cost = format!(
        "{} \u{b7} {}",
        rupees(a.cost_paise),
        duration_text(a.elapsed_s)
    );
    let copied = RwSignal::new(false);
    view! {
        <section class="answer" aria-label="Answer">
            <p class="decision">{DECISION_SUPPORT}</p>
            <p class="meta">{format!("{} \u{b7} {}", mode_label(a.mode), date_ist(a.created_at))}</p>

            <h3>"How I read your question"</h3>
            <dl>
                <dt>"Population"</dt><dd>{pico.population}</dd>
                <dt>"Intervention"</dt><dd>{pico.intervention}</dd>
                <dt>"Comparison"</dt><dd>{pico.comparison.unwrap_or_else(|| "none stated".into())}</dd>
                {(!pico.outcomes.is_empty()).then(|| view! { <dt>"Outcomes"</dt><dd>{pico.outcomes.join("; ")}</dd> })}
                {(!pico.limits.is_empty()).then(|| view! { <dt>"Limits"</dt><dd>{pico.limits.join("; ")}</dd> })}
            </dl>
            {(!a.final_query.is_empty()).then(|| view! { <p>"PubMed query: " <code>{a.final_query.clone()}</code></p> })}
            // A full page load (`rel="external"`) so the fragment survives to the
            // home page; the browser does not send it to the server.
            <p><a href=rerun rel="external">"Edit & re-run"</a></p>

            <h3>"Bottom line"</h3>
            {insufficient.then(|| view! { <p class="box amber">{INSUFFICIENT}</p> })}
            {match empty {
                Some(msg) => view! { <p>{msg}</p> }.into_any(),
                None => view! {
                    <ul>
                        {a.bottom_line
                            .into_iter()
                            .map(|c| {
                                view! {
                                    <li>
                                        {c.partial.then(|| view! { <span class="partial-mark" title="partly supported">{PARTIAL_MARK}</span> " " })}
                                        {c.text}
                                        " " {answer::refs_text(&c.refs)}
                                        " " <span class="conf">
                                            {if c.partial {
                                                format!("({}, partly supported)", c.confidence)
                                            } else {
                                                format!("({})", c.confidence)
                                            }}
                                        </span>
                                    </li>
                                }
                            })
                            .collect_view()}
                    </ul>
                }
                    .into_any(),
            }}

            {(!a.key_studies.is_empty()).then(|| view! {
                <h3>"Key studies"</h3>
                <ul>
                    {a.key_studies
                        .into_iter()
                        .map(|k| view! { <li>{format!("[{}] {}", k.ref_id, key_study_text(&k))}</li> })
                        .collect_view()}
                </ul>
            })}

            <h3>"References"</h3>
            {if a.references.is_empty() {
                view! { <p>"None verified."</p> }.into_any()
            } else {
                view! {
                    <ul class="refs-list">
                        {a.references.into_iter().map(reference_view).collect_view()}
                    </ul>
                }
                    .into_any()
            }}

            <h3>"Check"</h3>
            <p>{check}</p>

            <h3>"Search trail"</h3>
            <ul>
                <li>{format!("PubMed: {} hits \u{b7} Europe PMC: {} hits", t.pubmed_count, t.epmc_count)}</li>
                <li>{screened}</li>
                {exclusions.map(|x| view! { <li>{x}</li> })}
                {t.refinements
                    .into_iter()
                    .map(|r| view! { <li>"Refined: " {r.reason} " " <code>{r.query}</code></li> })
                    .collect_view()}
                {stopped.map(|x| view! { <li>{x}</li> })}
                {unavailable.map(|x| view! { <li>{x}</li> })}
            </ul>

            <h3>"Cost and time"</h3>
            <p>{cost}</p>

            <button
                class="secondary small"
                on:click=move |_| {
                    copy_text(&text);
                    copied.set(true);
                }
            >
                {move || if copied.get() { "Copied" } else { "Copy as text" }}
            </button>
        </section>
    }
}

/// Reference numbers by PMID, and the id prefix of the view drawing them
/// (Part A item 13): "" for the page, "orig-" for the original run view
/// below it, so no id appears twice (`#orig-ref-N`).
struct Refs {
    nums: std::collections::HashMap<String, u16>,
    prefix: &'static str,
}

impl std::ops::Deref for Refs {
    type Target = std::collections::HashMap<String, u16>;
    fn deref(&self) -> &Self::Target {
        &self.nums
    }
}

impl Refs {
    /// " [n]" linking reference `n` ([`ref_link`] in this view).
    fn link(&self, n: u16) -> AnyView {
        ref_link(self.prefix, n)
    }
}

/// Answer text with its `[PMID]` markers as numbered links to the
/// references (`#ref-N`). Text stays plain (escaped); a PMID with no
/// reference (none should be left after the citation check) is not shown.
fn cited(text: &str, refs: &Refs) -> AnyView {
    split_citations(text, |p| refs.contains_key(p))
        .into_iter()
        .map(|seg| match seg {
            Segment::Text(t) => t.into_any(),
            Segment::Cite(ids) => {
                let mut nums: Vec<u16> = ids.iter().filter_map(|p| refs.get(p).copied()).collect();
                nums.sort_unstable();
                nums.dedup();
                if nums.is_empty() {
                    return ().into_any();
                }
                let last = nums.len() - 1;
                view! {
                    <span class="cite">
                        "["
                        {nums
                            .into_iter()
                            .enumerate()
                            .map(|(i, n)| {
                                view! {
                                    <a href=format!("#{}ref-{n}", refs.prefix) rel="external">{n.to_string()}</a>
                                    {(i < last).then_some(", ")}
                                }
                            })
                            .collect_view()}
                        "]"
                    </span>
                }
                .into_any()
            }
        })
        .collect_view()
        .into_any()
}

fn reference_v3_view(r: ReferenceV3, prefix: &str) -> impl IntoView {
    let mut head = r.title.trim_end_matches('.').to_string();
    head.push('.');
    match (&r.journal, r.year) {
        (Some(j), Some(y)) => head.push_str(&format!(" {j}, {y}.")),
        (Some(j), None) => head.push_str(&format!(" {j}.")),
        (None, Some(y)) => head.push_str(&format!(" {y}.")),
        (None, None) => {}
    }
    let pmid = pubmed_link(&r.pmid).map(|href| (r.pmid.clone(), href));
    let doi = r
        .doi
        .clone()
        .and_then(|d| doi_link(&d).map(|href| (d, href)));
    view! {
        <li id=format!("{prefix}ref-{}", r.n)>
            {format!("[{}] ", r.n)}
            {r.retracted.then(|| view! { <span class="retracted">"RETRACTED \u{2014} do not rely on this paper. "</span> })}
            {head}
            {pmid.map(|(p, href)| view! { " " <a href=href rel="noopener noreferrer" target="_blank">{format!("PMID {p}")}</a> })}
            {doi.map(|(d, href)| view! { " " <a href=href rel="noopener noreferrer" target="_blank">{format!("DOI {d}")}</a> })}
        </li>
    }
}

/// Which mark a flagged number gets (Stage 4 items 5 and 7).
#[derive(Clone, Copy)]
enum Mark {
    /// The writer's number, not traced to what it was given.
    NotTraced,
    /// An extractor or skeptic number its source does not have.
    NotVerified,
}

fn mark_view(m: Mark) -> AnyView {
    let (label, title) = match m {
        Mark::NotTraced => (
            "not traced",
            "This number was not found in a verified quote, a structured number or an abstract given to the writer.",
        ),
        Mark::NotVerified => (
            "not verified",
            "This number was not found in the source text.",
        ),
    };
    view! { <sup class="flag" title=title>{label}</sup> }.into_any()
}

/// Text with numbered citations ([`cited`]) and a small mark after each run
/// `spans` marks on `field` (the text itself is never changed).
#[derive(Clone, Copy)]
struct Marks<'a> {
    refs: &'a Refs,
    spans: &'a [Span],
    mark: Mark,
}

impl Marks<'_> {
    fn t(&self, text: &str, field: &str) -> AnyView {
        let runs = marked_runs(text, field, self.spans);
        if runs.iter().all(|(_, m)| !m) {
            return cited(text, self.refs);
        }
        let mark = self.mark;
        runs.into_iter()
            .map(|(t, m)| view! { {cited(&t, self.refs)} {m.then(|| mark_view(mark))} })
            .collect_view()
            .into_any()
    }

    /// The mark alone, when `field` is marked as a whole (a structured
    /// number).
    fn whole(&self, field: &str) -> Option<AnyView> {
        whole_marked(field, self.spans).then(|| mark_view(self.mark))
    }
}

/// The written part of a Phase 3 answer: bottom line, outcome table,
/// trials table, conflicts, guidelines, applicability, what would change it.
/// Numbers not traced to the writer's sources carry a "not traced" mark.
// The page's parts, each drawn from its own stored field.
#[allow(clippy::too_many_arguments)]
fn written_v3_view(
    w: WrittenAnswer,
    refs: &Refs,
    untraced: &[Span],
    evidence: &std::collections::HashMap<String, EvidenceV3>,
    skeptic_guidelines: Vec<(usize, SkepticItem)>,
    skeptic_spans: &[Span],
    guideline_leads: Vec<crate::agent::answer_v3::SkepticLead>,
    completeness: Option<&CompletenessV3>,
    row_leads: &std::collections::HashMap<String, Vec<String>>,
) -> impl IntoView {
    let full = !evidence.is_empty();
    // Stage 4b: the completeness review's additions, drawn in place, each
    // labelled; its numbers not traced to the writer's sources are marked.
    let comp = completeness.cloned().unwrap_or_default();
    let cm = Marks {
        refs,
        spans: &comp.untraced,
        mark: Mark::NotTraced,
    };
    let added = || view! { " " <span class="flag-label added">{ADDED_LABEL}</span> };
    let unsourced =
        |f: bool| f.then(|| view! { " " <span class="flag-label">{NO_GUIDELINE_SOURCE}</span> });
    let mk = Marks {
        refs,
        spans: untraced,
        mark: Mark::NotTraced,
    };
    let c = w.bottom_line.certainty;
    // Guidelines (Stage 4 item 1; R-S4a): the skeptic's verified guidelines
    // and its unverified guideline leads. The writer no longer writes them;
    // answers stored with writer guidelines still show them first.
    let mut guidelines: Vec<AnyView> = w
        .guidelines
        .iter()
        .enumerate()
        .map(|(i, g)| {
            let n = g.pmid.as_ref().and_then(|p| refs.get(p).copied());
            // Stage 4 review 2: no PMID (none given, or removed by the
            // citation check): flagged, never presented as sourced.
            let unsourced = g.pmid.is_none();
            view! {
                <li>
                    {mk.t(&g.body, &format!("guidelines.{i}.body"))}
                    {g.year.map(|y| format!(" ({y})"))}
                    ": " {mk.t(&g.statement, &format!("guidelines.{i}.statement"))}
                    {n.map(|n| refs.link(n))}
                    {unsourced.then(|| view! { " " <span class="flag-label">{NO_GUIDELINE_SOURCE}</span> })}
                </li>
            }
            .into_any()
        })
        .collect();
    // The skeptic's bodies keep their "not verified" marks (review 3).
    let sk = Marks {
        refs,
        spans: skeptic_spans,
        mark: Mark::NotVerified,
    };
    for (i, g) in skeptic_guidelines {
        if w.guidelines
            .iter()
            .any(|x| x.pmid.as_deref() == Some(g.pmid.as_str()))
        {
            continue;
        }
        // A reference when there is one, otherwise PubMed (as the leads).
        let link = match refs.get(&g.pmid) {
            Some(n) => refs.link(*n),
            None => pubmed_link(&g.pmid)
                .map(|href| {
                    view! { " " <a href=href rel="noopener noreferrer" target="_blank">{format!("PMID {}", g.pmid)}</a> }
                        .into_any()
                })
                .unwrap_or_else(|| ().into_any()),
        };
        let unsourced = g.pmid.trim().is_empty();
        guidelines.push(
            view! {
                <li>
                    {sk.t(&g.text, &format!("guidelines.{i}.text"))}
                    {g.quote.map(|q| view! { " \u{2014} " {cited(&format!("\u{201c}{q}\u{201d}"), refs)} })}
                    {link}
                    {unsourced.then(|| view! { " " <span class="flag-label">{NO_GUIDELINE_SOURCE}</span> })}
                </li>
            }
            .into_any(),
        );
    }
    for l in guideline_leads {
        let link = match refs.get(&l.pmid) {
            Some(n) => refs.link(*n),
            None => pubmed_link(&l.pmid)
                .map(|href| {
                    view! { " " <a href=href rel="noopener noreferrer" target="_blank">{format!("PMID {}", l.pmid)}</a> }
                        .into_any()
                })
                .unwrap_or_else(|| ().into_any()),
        };
        guidelines
            .push(view! { <li><span class="flag-label">{l.text()}</span> {link}</li> }.into_any());
    }
    // At a glance (Stage 4 item 8): about five lines, at the top.
    let glance = w.at_a_glance.as_ref().map(|g| {
        view! {
            <section class="box glance" aria-label="At a glance">
                <p><strong>"At a glance: "</strong> {mk.t(&g.verdict, "at_a_glance.verdict")}</p>
                {(!g.key_numbers.is_empty()).then(|| view! {
                    <ul>
                        {g.key_numbers
                            .iter()
                            .enumerate()
                            .map(|(i, k)| view! {
                                <li>
                                    {(!k.label.is_empty()).then(|| view! {
                                        {mk.t(&k.label, &format!("at_a_glance.key_numbers.{i}.label"))} ": "
                                    })}
                                    {mk.t(&k.value_text, &format!("at_a_glance.key_numbers.{i}.value_text"))}
                                    {refs.get(&k.pmid).copied().map(|n| refs.link(n))}
                                </li>
                            })
                            .collect_view()}
                    </ul>
                })}
                {(!g.main_caveat.is_empty()).then(|| view! {
                    <p><strong>"Main caveat: "</strong> {mk.t(&g.main_caveat, "at_a_glance.main_caveat")}</p>
                })}
            </section>
        }
    });
    // What this means for clinical practice (Stage 4 item 9).
    let practice = w.clinical_practice.as_ref().map(|p| {
        // Labelled parts as a list; text under no label as written
        // (flat schema: parsed from labelled lines, never dropped).
        let parts: Vec<AnyView> = [
            ("Default choice", &p.default_choice, "default_choice"),
            ("Who may benefit", &p.who_may_benefit, "who_may_benefit"),
            ("Counselling", &p.counselling, "counselling"),
            ("Cost per case", &p.cost_per_case, "cost_per_case"),
        ]
        .into_iter()
        .filter(|(_, t, _)| !t.is_empty())
        .map(|(label, t, f)| {
            view! { <dt>{label}</dt><dd>{mk.t(t, &format!("clinical_practice.{f}"))}</dd> }.into_any()
        })
        .collect();
        view! {
            <h3>"What this means for clinical practice"</h3>
            {(!parts.is_empty()).then(|| view! { <dl>{parts}</dl> })}
            {(!p.text.is_empty()).then(|| view! { <p>{mk.t(&p.text, "clinical_practice.text")}</p> })}
        }
    });
    // Each outcome's structured numbers, by label and study, the
    // intervention arm first (Stage 4 item 10).
    let comparison = |p: &str| {
        w.studies
            .iter()
            .find(|s| s.pmid == p)
            .map(|s| s.comparison.as_str())
    };
    let arms: Vec<Vec<AnyView>> = w
        .outcomes
        .iter()
        .enumerate()
        .map(|(i, o)| {
            arm_lines(o, comparison)
                .into_iter()
                .map(|l| {
                    let flagged = l
                        .numbers
                        .iter()
                        .any(|j| whole_marked(&format!("outcomes.{i}.numbers.{j}"), untraced));
                    view! {
                        <li>
                            {arm_line_text(o, &l)}
                            {flagged.then(|| mark_view(Mark::NotTraced))}
                            {refs.get(&l.pmid).copied().map(|n| refs.link(n))}
                        </li>
                    }
                    .into_any()
                })
                .collect()
        })
        .collect();
    let mut arms = arms.into_iter();
    view! {
        {glance}
        <h3>"Bottom line"</h3>
        <p>{mk.t(&w.bottom_line.text, "bottom_line.text")}</p>
        <p>
            <span class=c.class()>{format!("Certainty: {}", c.as_str())}</span>
            " " <span class="conf">{mk.t(&w.bottom_line.certainty_reason, "bottom_line.certainty_reason")}</span>
        </p>
        {practice}

        <h3>"Outcome by outcome"</h3>
        {if w.outcomes.is_empty() && comp.added_outcomes.is_empty() {
            view! { <p>"No outcome could be assessed from the evidence found."</p> }.into_any()
        } else {
            view! {
                <div class="scroll">
                    <table class="wide">
                        <thead>
                            <tr>
                                <th>"Outcome"</th>
                                <th>"RCT evidence"</th>
                                <th>"Effect"</th>
                                <th>"Certainty"</th>
                                <th>"Why"</th>
                            </tr>
                        </thead>
                        <tbody>
                            {w.outcomes
                                .iter()
                                .enumerate()
                                .map(|(i, o)| view! {
                                    <tr>
                                        <td>
                                            {mk.t(&o.outcome, &format!("outcomes.{i}.outcome"))}
                                            {o.evidence_type.label().map(|l| view! { <br /><span class="etype">{l}</span> })}
                                        </td>
                                        <td>{mk.t(&o.rct_evidence, &format!("outcomes.{i}.rct_evidence"))}</td>
                                        <td>
                                            {mk.t(&o.effect_text, &format!("outcomes.{i}.effect_text"))}
                                            {arms.next().filter(|a| !a.is_empty()).map(|a| view! { <ul class="arms">{a}</ul> })}
                                            {comp
                                                .additions_to(i)
                                                .map(|(k, a)| view! {
                                                    <p class="added">
                                                        {cm.t(&a.text, &format!("outcome_additions.{k}.text"))}
                                                        {ref_links(&not_cited_at_end(&a.text, &a.pmids), refs)}
                                                        {added()}
                                                        {unsourced(a.unsourced)}
                                                    </p>
                                                })
                                                .collect_view()}
                                        </td>
                                        <td><span class=o.certainty.class()>{o.certainty.as_str()}</span></td>
                                        <td>{mk.t(&o.why, &format!("outcomes.{i}.why"))}</td>
                                    </tr>
                                })
                                .collect_view()}
                            {comp
                                .added_outcomes
                                .iter()
                                .enumerate()
                                .map(|(k, x)| view! {
                                    <tr class="added">
                                        <td>
                                            {cm.t(&x.outcome, &format!("added_outcomes.{k}.outcome"))}
                                            <br />
                                            <span class="flag-label added">{ADDED_LABEL}</span>
                                        </td>
                                        <td>"\u{2013}"</td>
                                        <td>
                                            {cm.t(&x.effect_text, &format!("added_outcomes.{k}.effect_text"))}
                                            {ref_links(&not_cited_at_end(&x.effect_text, &x.pmids), refs)}
                                            {unsourced(x.unsourced)}
                                        </td>
                                        <td>
                                            {match x.certainty {
                                                Some(c) => view! { <span class=c.class()>{c.as_str()}</span> }.into_any(),
                                                None => view! { <span class="conf">"not graded"</span> }.into_any(),
                                            }}
                                        </td>
                                        <td>{cm.t(&x.why, &format!("added_outcomes.{k}.why"))}</td>
                                    </tr>
                                })
                                .collect_view()}
                        </tbody>
                    </table>
                </div>
            }
                .into_any()
        }}

        <h3>"The trials"</h3>
        {if w.studies.is_empty() {
            view! { <p>"None."</p> }.into_any()
        } else {
            view! {
                <div class="scroll">
                    <table class="wide">
                        <thead>
                            <tr>
                                <th>"Trial"</th>
                                <th>"Design / n / setting"</th>
                                <th>"Comparison"</th>
                                <th>"Key results"</th>
                                <th>"Appraisal"</th>
                                {full.then(|| view! { <th>"Evidence checked"</th> })}
                            </tr>
                        </thead>
                        <tbody>
                            {w.studies
                                .iter()
                                .enumerate()
                                .map(|(i, s)| {
                                    let n = refs.get(&s.pmid).copied();
                                    let e = evidence.get(s.pmid.trim());
                                    let checked = full.then(|| evidence_cell(e));
                                    let funding = e.and_then(|e| funding_view(e, refs));
                                    view! {
                                        <tr>
                                            <td>
                                                {mk.t(&s.label, &format!("studies.{i}.label"))}
                                                {n.map(|n| refs.link(n))}
                                                {e.filter(|e| e.found_by_skeptic).map(|e| view! {
                                                    <br /><span class="flag-label added">{format!("{FOUND_BY_SKEPTIC}; {}", e.second_wave())}</span>
                                                })}
                                                {row_leads
                                                    .get(s.pmid.trim())
                                                    .into_iter()
                                                    .flatten()
                                                    .map(|t| view! { <br /><span class="flag-label">{format!("Skeptic: {t}")}</span> })
                                                    .collect_view()}
                                            </td>
                                            <td>
                                                {mk.t(&s.design, &format!("studies.{i}.design"))}
                                                {s.n.map(|n| format!(" \u{b7} n = {n}"))}
                                                {mk.whole(&format!("studies.{i}.n"))}
                                                " \u{b7} " {mk.t(&s.setting, &format!("studies.{i}.setting"))}
                                            </td>
                                            <td>{mk.t(&s.comparison, &format!("studies.{i}.comparison"))}</td>
                                            <td>{mk.t(&s.key_results, &format!("studies.{i}.key_results"))}</td>
                                            <td>
                                                {mk.t(&s.appraisal, &format!("studies.{i}.appraisal"))}
                                                {funding}
                                            </td>
                                            {checked.map(|c| view! { <td>{c}</td> })}
                                        </tr>
                                    }
                                })
                                .collect_view()}
                        </tbody>
                    </table>
                </div>
            }
                .into_any()
        }}

        <h3>"Conflicting evidence"</h3>
        {if w.conflicts.is_empty() && comp.added_conflicts.is_empty() {
            view! { <p>"None identified."</p> }.into_any()
        } else {
            view! {
                <ul>
                    {w.conflicts
                        .iter()
                        .enumerate()
                        .map(|(i, x)| view! { <li>{mk.t(&x.text, &format!("conflicts.{i}.text"))}</li> })
                        .collect_view()}
                    {comp
                        .added_conflicts
                        .iter()
                        .enumerate()
                        .map(|(k, x)| view! {
                            <li>
                                {cm.t(&x.text, &format!("added_conflicts.{k}.text"))}
                                {ref_links(&not_cited_at_end(&x.text, &x.pmids), refs)}
                                {added()}
                                {unsourced(x.unsourced)}
                            </li>
                        })
                        .collect_view()}
                </ul>
            }
                .into_any()
        }}

        {(!guidelines.is_empty()).then(|| view! {
            <h3>"Guidelines and consensus statements"</h3>
            <ul>{guidelines}</ul>
        })}

        <h3>"Applicability"</h3>
        <dl>
            <dt>"Case mix"</dt><dd>{mk.t(&w.applicability.case_mix, "applicability.case_mix")}</dd>
            <dt>"India and cost"</dt><dd>{mk.t(&w.applicability.india_cost_context, "applicability.india_cost_context")}</dd>
            <dt>"Training"</dt><dd>{mk.t(&w.applicability.training, "applicability.training")}</dd>
        </dl>

        <h3>"What would change this answer"</h3>
        <ul>
            {w.would_change
                .iter()
                .enumerate()
                .map(|(i, x)| view! { <li>{mk.t(x, &format!("would_change.{i}"))}</li> })
                .collect_view()}
        </ul>
    }
}

/// Review minor 4: text whose citation markers become reference links, or
/// PubMed links for studies with no reference (never dropped, never raw).
fn pmid_cited(text: &str, refs: &Refs) -> AnyView {
    split_citations(text, |p| refs.contains_key(p))
        .into_iter()
        .map(|seg| match seg {
            Segment::Text(t) => t.into_any(),
            Segment::Cite(ids) => ids
                .into_iter()
                .map(|p| match refs.get(&p) {
                    Some(n) => refs.link(*n),
                    None => pubmed_link(&p)
                        .map(|href| {
                            view! { " " <a href=href rel="noopener noreferrer" target="_blank">{format!("PMID {p}")}</a> }
                                .into_any()
                        })
                        .unwrap_or_else(|| p.into_any()),
                })
                .collect_view()
                .into_any(),
        })
        .collect_view()
        .into_any()
}

/// The reference links of `pmids` that have references, in order.
fn ref_links(pmids: &[String], refs: &Refs) -> AnyView {
    let mut n: Vec<u16> = pmids
        .iter()
        .filter_map(|p| refs.get(p.trim()).copied())
        .collect();
    n.sort_unstable();
    n.dedup();
    n.into_iter()
        .map(|n| refs.link(n))
        .collect_view()
        .into_any()
}

/// Stage 4b: the completeness review's checks line, its notes and,
/// collapsed, the gaps it judged not relevant (each with why).
fn completeness_view(c: Option<CompletenessV3>, refs: &Refs) -> Option<AnyView> {
    let c = c?;
    let cm = Marks {
        refs,
        spans: &c.untraced,
        mark: Mark::NotTraced,
    };
    let notes: Vec<AnyView> = c
        .notes
        .iter()
        .enumerate()
        .map(|(k, n)| {
            view! {
                <li>
                    {cm.t(n, &format!("notes.{k}"))}
                    " " <span class="flag-label added">{ADDED_LABEL}</span>
                </li>
            }
            .into_any()
        })
        .collect();
    let not_added: Vec<AnyView> = c
        .not_added
        .iter()
        .map(|n| {
            view! { <li>{pmid_cited(&n.gap, refs)} " \u{2014} " {pmid_cited(&n.reason, refs)}</li> }
                .into_any()
        })
        .collect();
    let n_not = not_added.len();
    Some(
        view! {
            <section class="checks" aria-label="Completeness review">
                <p class="hint">{c.line()}</p>
                {(!notes.is_empty()).then(|| view! { <ul>{notes}</ul> })}
                {(n_not > 0).then(|| view! {
                    <details>
                        <summary>{format!("Judged not relevant ({n_not})")}</summary>
                        <ul>{not_added}</ul>
                    </details>
                })}
            </section>
        }
        .into_any(),
    )
}

/// " [n]" linking reference `n` (`#<prefix>ref-n`).
fn ref_link(prefix: &str, n: u16) -> AnyView {
    view! { " " <span class="cite">"[" <a href=format!("#{prefix}ref-{n}") rel="external">{n.to_string()}</a> "]"</span> }
        .into_any()
}

/// The trials table's "Evidence checked" cell: what was read, the verified
/// quotes, and (Stage 4 item 7) what was kept but not verified, labelled.
fn evidence_cell(e: Option<&EvidenceV3>) -> AnyView {
    let Some(e) = e else {
        return "not extracted".into_any();
    };
    let unverified: Vec<String> = e
        .unverified_details
        .iter()
        .map(|u| format!("{}: {}", u.outcome, u.effect_text))
        .collect();
    let numbers = (e.numbers_unverified > 0).then(|| {
        format!(
            " \u{b7} {} not verified",
            if e.numbers_unverified == 1 {
                "1 number".to_string()
            } else {
                format!("{} numbers", e.numbers_unverified)
            }
        )
    });
    let registration = e
        .trial_registration_unverified
        .as_ref()
        .map(|r| view! { " \u{b7} registration " {r.clone()} " " <span class="flag-label">"not in the source"</span> });
    view! {
        {evidence_line(e)}
        {numbers}
        {registration}
        {(!unverified.is_empty()).then(|| view! {
            <ul class="unverified">
                {unverified
                    .into_iter()
                    .map(|u| view! { <li>{u} " " <span class="flag-label">"not verified against the paper's wording"</span></li> })
                    .collect_view()}
            </ul>
        })}
    }
    .into_any()
}

/// What the extractor read for a study and how many quotes it verified.
fn evidence_line(e: &EvidenceV3) -> String {
    if e.text == "not extracted" {
        return "not extracted (abstract)".to_string();
    }
    format!(
        "{} \u{b7} {} verified {}",
        e.text,
        e.verified_quotes,
        if e.verified_quotes == 1 {
            "quote"
        } else {
            "quotes"
        }
    )
}

/// "Funding: …" from the extracted row (Stage 4 item 3), when it was
/// looked for; a number the source does not have is marked.
fn funding_view(e: &EvidenceV3, refs: &Refs) -> Option<AnyView> {
    if !e.funding_checked {
        return None;
    }
    let mk = Marks {
        refs,
        spans: &e.spans,
        mark: Mark::NotVerified,
    };
    Some(match &e.funding {
        Some(f) => {
            view! { <br /><span class="conf">"Funding: " {mk.t(f, "funding")}</span> }.into_any()
        }
        None => {
            let w = if e.text == "full text" {
                "text"
            } else {
                "abstract"
            };
            view! { <br /><span class="conf">{format!("Funding: not stated in the {w}")}</span> }
                .into_any()
        }
    })
}

/// The skeptic's review (Full): verdict, caveats, overlaps, missed trials,
/// meta-analysis pool notes, contradictions and guidelines, as escaped text
/// with numbered citations; a number no text its tools returned has carries
/// a "not verified" mark (Stage 4 item 7).
fn skeptic_v3_view(
    k: crate::agent::answer_v3::SkepticV3,
    refs: &Refs,
    on_rows: &std::collections::HashSet<String>,
) -> AnyView {
    let mk = Marks {
        refs,
        spans: &k.spans,
        mark: Mark::NotVerified,
    };
    // Each line: the skeptic's text (its path for marks, citations linked;
    // a verified quote after it) and the studies it names: a numbered
    // reference when there is one, otherwise a PubMed link (re-review 3: a
    // study whose record could not be fetched is not dropped). Only PMIDs
    // of 1 to 9 digits are linked.
    type Line = (String, String, Option<String>, Vec<String>);
    let item = |path: String, i: &SkepticItem| -> Line {
        (path, i.text.clone(), i.quote.clone(), vec![i.pmid.clone()])
    };
    let caveats: Vec<Line> = k
        .must_mention
        .iter()
        .enumerate()
        .map(|(i, m)| (format!("must_mention.{i}"), m.clone(), None, vec![]))
        .collect();
    let overlaps: Vec<Line> = k
        .overlaps
        .iter()
        .enumerate()
        .map(|(i, o)| {
            (
                format!("overlaps.{i}.note"),
                o.note.clone(),
                None,
                o.pmids.clone(),
            )
        })
        .collect();
    let picked: Vec<Line> = k
        .extract_next
        .iter()
        .enumerate()
        .map(|(i, x)| item(format!("extract_next.{i}.text"), x))
        .collect();
    let missed: Vec<Line> = k
        .missed_rcts
        .iter()
        .enumerate()
        .map(|(i, x)| item(format!("missed_rcts.{i}.text"), x))
        .collect();
    let pooled: Vec<Line> = k
        .ma_notes
        .iter()
        .enumerate()
        .map(|(i, x)| item(format!("ma_notes.{i}.text"), x))
        .collect();
    // Each with its verified quote (review 2).
    let against: Vec<Line> = k
        .contradicting
        .iter()
        .enumerate()
        .map(|(i, x)| item(format!("contradicting.{i}.text"), x))
        .chain(
            k.guidelines
                .iter()
                .enumerate()
                .map(|(i, x)| item(format!("guidelines.{i}.text"), x)),
        )
        .collect();
    // Unverified leads (review 6): PMID and kind only, no quote or claim.
    // Stage 4b item 5: a lead on a study with a trial row is drawn there.
    let leads: Vec<Line> = k
        .leads
        .iter()
        .filter(|l| !on_rows.contains(&l.pmid))
        .map(|l| (String::new(), l.text(), None, vec![l.pmid.clone()]))
        .collect();
    let study = |p: String| -> AnyView {
        if let Some(n) = refs.get(&p) {
            refs.link(*n)
        } else if (1..=9).contains(&p.len()) && p.bytes().all(|b| b.is_ascii_digit()) {
            let href = format!("https://pubmed.ncbi.nlm.nih.gov/{p}/");
            view! { " " <a href=href rel="noopener noreferrer" target="_blank">{format!("PMID {p}")}</a> }
                .into_any()
        } else {
            ().into_any()
        }
    };
    let list = |title: &'static str, xs: Vec<Line>| {
        (!xs.is_empty()).then(|| {
            view! {
                <p><strong>{title}</strong></p>
                <ul>
                    {xs
                        .into_iter()
                        .map(|(path, t, q, ps)| view! {
                            <li>
                                {mk.t(&t, &path)}
                                {q.map(|q| view! { " \u{2014} " {cited(&format!("\u{201c}{q}\u{201d}"), refs)} })}
                                {ps.into_iter().map(study).collect_view()}
                            </li>
                        })
                        .collect_view()}
                </ul>
            }
        })
    };
    view! {
        <section class="box" aria-label="Challenged by the skeptic">
            <h3>"Challenged by the skeptic"</h3>
            <p>{format!("Verdict: the conclusion {}.", k.verdict)}</p>
            {list("Caveats", caveats)}
            {list("Not independent", overlaps)}
            {list("Picked for extraction (second wave)", picked)}
            {list("Trials the search missed", missed)}
            {list("What the meta-analyses pooled", pooled)}
            {list("Evidence against, and guidelines", against)}
            {list("Unverified leads", leads)}
        </section>
    }
    .into_any()
}

/// A Phase 3 answer (AnswerV3), in the spec §2 order, as escaped text and
/// links only. Since Part A it is the collapsed "Original run view" below
/// the interactive page; `prefix` keeps its ids apart from the page's.
fn answer_v3_view(a: AnswerV3, prefix: &'static str) -> impl IntoView {
    let refs = Refs {
        nums: a.ref_numbers(),
        prefix,
    };
    let rerun = rerun_href(&a.question, a.mode);
    let cost = format!(
        "{} \u{b7} {}",
        rupees(a.cost_paise),
        duration_text(a.elapsed_s)
    );
    let notes = a.trail_notes();
    let untraced = a.untraced.clone();
    // Copy as text (Stage 4 item 10): the whole answer as plain text.
    let text = a.plain_text();
    let copy = RwSignal::new(CopyState::Ready);
    let copy_id = format!("{prefix}{COPY_AREA}");
    // Other notable findings (Stage 4 item 12): code's, then the writer's.
    let notable_writer: Vec<NotableFinding> = a
        .answer
        .as_ref()
        .map(|w| w.notable_findings.clone())
        .unwrap_or_default();
    // Stage 4b item 5: the skeptic's items are in its box only.
    let notable_code: Vec<NotableFinding> = a
        .notable_code
        .iter()
        .filter(|n| !n.repeats_skeptic())
        .cloned()
        .collect();
    let notable = notable_view(&notable_code, &notable_writer, &refs, &untraced);
    let n_searches = a.searches.len();
    // Full: per study, what the extractor read and checked.
    let evidence: std::collections::HashMap<String, EvidenceV3> = a
        .evidence
        .iter()
        .map(|e| (e.pmid.clone(), e.clone()))
        .collect();
    // Stage 4b item 5: leads on studies with a trial row are drawn on it.
    let on_rows: std::collections::HashSet<String> = a
        .skeptic
        .iter()
        .flat_map(|k| &k.leads)
        .filter(|l| a.lead_on_row(&l.pmid))
        .map(|l| l.pmid.clone())
        .collect();
    let mut row_leads: std::collections::HashMap<String, Vec<String>> =
        std::collections::HashMap::new();
    for p in &on_rows {
        row_leads.insert(
            p.clone(),
            a.leads_on_row(p).into_iter().map(|l| l.text()).collect(),
        );
    }
    let guideline_leads: Vec<crate::agent::answer_v3::SkepticLead> = a
        .skeptic
        .as_ref()
        .map(|k| {
            k.leads
                .iter()
                .filter(|l| l.kind == "guideline" && !on_rows.contains(&l.pmid))
                .cloned()
                .collect()
        })
        .unwrap_or_default();
    let skeptic_guidelines: Vec<(usize, SkepticItem)> = a
        .skeptic
        .as_ref()
        .map(|k| k.guidelines.iter().cloned().enumerate().collect())
        .unwrap_or_default();
    let skeptic_spans: Vec<Span> = a
        .skeptic
        .as_ref()
        .map(|k| k.spans.clone())
        .unwrap_or_default();
    let completeness = a.completeness.clone();
    let written = a.answer.map(|w| {
        written_v3_view(
            w,
            &refs,
            &untraced,
            &evidence,
            skeptic_guidelines,
            &skeptic_spans,
            guideline_leads,
            completeness.as_ref(),
            &row_leads,
        )
    });
    let checks = completeness_view(completeness.clone(), &refs);
    let skeptic = a.skeptic.map(|k| skeptic_v3_view(k, &refs, &on_rows));
    let run_notes = a.notes.clone();
    let empty = a.empty_reason.clone();
    view! {
        <section class="answer" aria-label="Answer">
            <p class="decision">{DECISION_SUPPORT}</p>
            <p class="meta">{format!("{} \u{b7} {}", mode_label(a.mode), date_ist(a.created_at))}</p>
            <p><a href=rerun rel="external">"Edit & re-run"</a></p>

            {match written {
                Some(w) => w.into_any(),
                None => view! {
                    <h3>"Bottom line"</h3>
                    <p>{empty.unwrap_or_else(|| answer::NO_EVIDENCE.to_string())}</p>
                }
                    .into_any(),
            }}

            {skeptic}
            {notable}
            {checks}
            {(!run_notes.is_empty())
                .then(|| view! {
                    <ul class="hint">{run_notes.into_iter().map(|n| view! { <li>{n}</li> }).collect_view()}</ul>
                })}

            <h3>"References"</h3>
            {if a.references.is_empty() {
                view! { <p>"None."</p> }.into_any()
            } else {
                view! {
                    <ul class="refs-list">
                        {a.references.into_iter().map(|r| reference_v3_view(r, prefix)).collect_view()}
                    </ul>
                }
                    .into_any()
            }}

            <details>
                <summary>{format!("Search trail ({n_searches} searches)")}</summary>
                <ul>
                    {a.searches
                        .into_iter()
                        .map(|s| view! { <li>{format!("{}: ", s.source)} <code>{s.query}</code> {format!(" \u{2014} {} hits", s.hits)}</li> })
                        .collect_view()}
                    {notes.into_iter().map(|n| view! { <li>{n}</li> }).collect_view()}
                </ul>
            </details>

            <h3>"Cost and time"</h3>
            <p>{cost}</p>

            <button
                class="secondary small"
                on:click=move |_| copy_answer(&text, copy, &copy_id)
            >
                {move || match copy.get() {
                    CopyState::Ready => "Copy as text",
                    CopyState::Copied => "Copied",
                    CopyState::Manual => "Select the text below and copy it",
                }}
            </button>
            // The fallback (shown only when the clipboard refused). It is
            // empty in the markup: Leptos does not escape a textarea's
            // children, so the text is set as its value in the click
            // handler, never written as HTML.
            <textarea
                id=format!("{prefix}{COPY_AREA}")
                class=move || if copy.get() == CopyState::Manual { "copy-area shown" } else { "copy-area" }
                readonly=true
                aria-hidden=move || (copy.get() != CopyState::Manual).to_string()
                tabindex=move || if copy.get() == CopyState::Manual { "0" } else { "-1" }
            ></textarea>
        </section>
    }
}

/// A stored Phase 3 answer as the run page draws it (server only; for
/// tests and the Stage 4 replay): the interactive page (Part A).
#[cfg(feature = "ssr")]
pub fn render_answer_v3(a: AnswerV3) -> String {
    use leptos::tachys::view::RenderHtml;
    Owner::new().with(|| page::answer_page_view(a).to_html())
}

/// The original run view (Part A item 13, owner decision 3): today's
/// answer view, unchanged but for its id prefix, inside one closed
/// `<details>` (it opens without hydration), with one debug line.
/// Removing it later is its entry in `answer_page::PAGE_ORDER` and this.
fn original_view(a: AnswerV3) -> AnyView {
    let (outcomes, numbers, studies, references, untraced, comp_untraced) =
        crate::answer_page::debug_counts(&a);
    let debug = format!(
        "Answer version {} \u{b7} {} \u{b7} {} \u{b7} {outcomes} outcomes \u{b7} {numbers} numbers \u{b7} {studies} studies \u{b7} {references} references \u{b7} {untraced} untraced spans ({comp_untraced} in the completeness review)",
        a.version,
        mode_label(a.mode),
        date_ist(a.created_at)
    );
    view! {
        <details class="ap-original">
            <summary>
                "Original run view (for checking) \u{2014} the answer as stored, in the earlier layout"
            </summary>
            <p class="hint ap-debug">{debug}</p>
            {answer_v3_view(a, ORIGINAL_PREFIX)}
        </details>
    }
    .into_any()
}

/// The id prefix of the original run view.
const ORIGINAL_PREFIX: &str = "orig-";

/// Today's answer view alone, as the original run view draws it, with
/// `prefix` on its ids (server only; for the Part A tests).
#[cfg(feature = "ssr")]
pub fn render_original_v3(a: AnswerV3, prefix: &'static str) -> String {
    use leptos::tachys::view::RenderHtml;
    Owner::new().with(|| answer_v3_view(a, prefix).to_html())
}

/// The id of the hidden textarea holding the plain-text answer.
const COPY_AREA: &str = "answer-plain-text";

/// What the Copy button has done (set only in the browser).
#[derive(Clone, Copy, PartialEq, Eq)]
#[cfg_attr(not(feature = "hydrate"), allow(dead_code))]
enum CopyState {
    Ready,
    Copied,
    /// The clipboard refused: the textarea is shown, its text selected.
    Manual,
}

/// Copy `text` (Stage 4 item 10). In the click handler: the text is set as
/// the hidden textarea's value (a property, never HTML), then
/// `navigator.clipboard.writeText` is called; if the Clipboard API is missing or refuses, the selected
/// textarea is copied with `execCommand("copy")`, and if that fails too the
/// textarea is shown with its text selected so it can be copied by hand
/// (phones). Browser only.
fn copy_answer(text: &str, state: RwSignal<CopyState>, id: &str) {
    #[cfg(feature = "hydrate")]
    {
        use wasm_bindgen::JsCast;
        use wasm_bindgen::prelude::Closure;
        use web_sys::js_sys::Reflect;
        // Owned: the fallback below outlives this call.
        let id = id.to_owned();
        let area = move || {
            web_sys::window()
                .and_then(|w| w.document())
                .and_then(|d| d.get_element_by_id(&id))
                .and_then(|e| e.dyn_into::<web_sys::HtmlTextAreaElement>().ok())
        };
        if let Some(a) = area() {
            a.set_value(text);
        }
        let select = move || -> bool {
            let Some(doc) = web_sys::window().and_then(|w| w.document()) else {
                return false;
            };
            let Some(area) = area() else {
                return false;
            };
            // iOS will not select inside a readonly textarea (review 8).
            area.set_read_only(false);
            area.set_tab_index(0);
            let _ = area.focus();
            area.select();
            let _ = area.set_selection_range(0, area.value().len() as u32);
            doc.dyn_into::<web_sys::HtmlDocument>()
                .ok()
                .and_then(|d| d.exec_command("copy").ok())
                .unwrap_or(false)
        };
        let Some(w) = web_sys::window() else {
            return;
        };
        let nav = w.navigator();
        let has_clipboard = Reflect::get(&nav, &"clipboard".into())
            .map(|c| !c.is_undefined() && !c.is_null())
            .unwrap_or(false);
        if !has_clipboard {
            state.set(if select() {
                CopyState::Copied
            } else {
                CopyState::Manual
            });
            return;
        }
        let ok = Closure::once(move |_: wasm_bindgen::JsValue| state.set(CopyState::Copied));
        let failed = Closure::once(move |_: wasm_bindgen::JsValue| {
            state.set(if select() {
                CopyState::Copied
            } else {
                CopyState::Manual
            });
        });
        let _ = nav.clipboard().write_text(text).then2(&ok, &failed);
        ok.forget();
        failed.forget();
    }
    #[cfg(not(feature = "hydrate"))]
    {
        let _ = (text, state, id);
    }
}

/// "Other notable findings from this run" (Stage 4 item 12): the items code
/// built from the team's checks, then the writer's, each with its studies.
fn notable_view(
    code: &[NotableFinding],
    writer: &[NotableFinding],
    refs: &Refs,
    untraced: &[Span],
) -> Option<AnyView> {
    if code.is_empty() && writer.is_empty() {
        return None;
    }
    let studies = |pmids: &[String]| -> AnyView {
        let mut n: Vec<u16> = pmids
            .iter()
            .filter_map(|p| refs.get(p.trim()).copied())
            .collect();
        n.sort_unstable();
        n.dedup();
        n.into_iter()
            .map(|n| refs.link(n))
            .collect_view()
            .into_any()
    };
    let wk = Marks {
        refs,
        spans: untraced,
        mark: Mark::NotTraced,
    };
    Some(
        view! {
            <h3>"Other notable findings from this run"</h3>
            <ul class="notable">
                {code
                    .iter()
                    .map(|n| {
                        let mk = Marks {
                            refs,
                            spans: &n.spans,
                            mark: Mark::NotVerified,
                        };
                        view! { <li>{mk.t(&n.text, "text")} {studies(&not_cited_at_end(&n.text, &n.pmids))}</li> }
                    })
                    .collect_view()}
                {writer
                    .iter()
                    .enumerate()
                    .map(|(i, n)| view! {
                        <li>
                            {wk.t(&n.text, &format!("notable_findings.{i}.text"))}
                            {studies(&not_cited_at_end(&n.text, &n.pmids))}
                        </li>
                    })
                    .collect_view()}
            </ul>
        }
        .into_any(),
    )
}

#[component]
fn ActivityPage() -> impl IntoView {
    let data = Resource::new(|| (), |_| get_activity());
    view! {
        <h1 class="activity-title">"Activity"</h1>
        <Suspense fallback=|| view! { <p>"Loading the activity log…"</p> }>
            {move || {
                data.get()
                    .map(|r| match r {
                        Ok(v) => {
                            let class = if v.chain.status == "valid" {
                                "chain green"
                            } else {
                                "chain red"
                            };
                            view! {
                                <p class=class>{v.chain.label}</p>
                                <div class="scroll">
                                    <table>
                                        <thead>
                                            <tr>
                                                <th>"#"</th>
                                                <th>"Time (IST)"</th>
                                                <th>"User"</th>
                                                <th>"Event"</th>
                                                <th>"Detail"</th>
                                            </tr>
                                        </thead>
                                        <tbody>
                                            {v
                                                .entries
                                                .into_iter()
                                                .map(|e| {
                                                    view! {
                                                        <tr>
                                                            <td>{e.seq}</td>
                                                            <td>{e.time_ist}</td>
                                                            <td>{e.user_email.unwrap_or_else(|| "—".to_string())}</td>
                                                            <td>{e.event}</td>
                                                            <td>
                                                                <code>{e.detail}</code>
                                                            </td>
                                                        </tr>
                                                    }
                                                })
                                                .collect_view()}
                                        </tbody>
                                    </table>
                                </div>
                            }
                                .into_any()
                        }
                        Err(e) => view! { <p class="box amber">{error_text(&e)}</p> }.into_any(),
                    })
            }}
        </Suspense>
    }
}
