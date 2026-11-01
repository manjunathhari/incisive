//! The ivory redesign (1 Nov): the Figtree faces and ivory tokens, the
//! shared header and nav, and the card pieces the Ask page
//! (`ask_page`) and the Library (`library_page`) share.
//!
//! Only page chrome is styled here: the answer content (`.ap-main` and the
//! original run view) keeps its own styles and markup. No prices are shown
//! on Ask or the Library. Everything is escaped text; no `inner_html`.

use leptos::prelude::*;
use leptos_router::hooks::use_location;

use super::{MeResource, error_text};
use crate::library::{self as lib, LibraryCard};

/// The Figtree faces (self-hosted, OFL; `public/fonts/figtree`) and the
/// ivory tokens and classes. Scoped to the chrome, Ask and Library classes:
/// nothing here matches inside `.ap-main` or the original run view.
pub const IVORY_STYLE: &str = r#"
@font-face { font-family:'Figtree'; font-style:normal; font-display:swap; font-weight:400; src:url(/fonts/figtree/figtree-latin-ext-400-normal.woff2) format('woff2'); unicode-range:U+0100-02BA,U+02BD-02C5,U+02C7-02CC,U+02CE-02D7,U+02DD-02FF,U+0304,U+0308,U+0329,U+1D00-1DBF,U+1E00-1E9F,U+1EF2-1EFF,U+2020,U+20A0-20AB,U+20AD-20C0,U+2113,U+2C60-2C7F,U+A720-A7FF; }
@font-face { font-family:'Figtree'; font-style:normal; font-display:swap; font-weight:400; src:url(/fonts/figtree/figtree-latin-400-normal.woff2) format('woff2'); unicode-range:U+0000-00FF,U+0131,U+0152-0153,U+02BB-02BC,U+02C6,U+02DA,U+02DC,U+0304,U+0308,U+0329,U+2000-206F,U+20AC,U+2122,U+2191,U+2193,U+2212,U+2215,U+FEFF,U+FFFD; }
@font-face { font-family:'Figtree'; font-style:normal; font-display:swap; font-weight:500; src:url(/fonts/figtree/figtree-latin-ext-500-normal.woff2) format('woff2'); unicode-range:U+0100-02BA,U+02BD-02C5,U+02C7-02CC,U+02CE-02D7,U+02DD-02FF,U+0304,U+0308,U+0329,U+1D00-1DBF,U+1E00-1E9F,U+1EF2-1EFF,U+2020,U+20A0-20AB,U+20AD-20C0,U+2113,U+2C60-2C7F,U+A720-A7FF; }
@font-face { font-family:'Figtree'; font-style:normal; font-display:swap; font-weight:500; src:url(/fonts/figtree/figtree-latin-500-normal.woff2) format('woff2'); unicode-range:U+0000-00FF,U+0131,U+0152-0153,U+02BB-02BC,U+02C6,U+02DA,U+02DC,U+0304,U+0308,U+0329,U+2000-206F,U+20AC,U+2122,U+2191,U+2193,U+2212,U+2215,U+FEFF,U+FFFD; }
@font-face { font-family:'Figtree'; font-style:normal; font-display:swap; font-weight:600; src:url(/fonts/figtree/figtree-latin-ext-600-normal.woff2) format('woff2'); unicode-range:U+0100-02BA,U+02BD-02C5,U+02C7-02CC,U+02CE-02D7,U+02DD-02FF,U+0304,U+0308,U+0329,U+1D00-1DBF,U+1E00-1E9F,U+1EF2-1EFF,U+2020,U+20A0-20AB,U+20AD-20C0,U+2113,U+2C60-2C7F,U+A720-A7FF; }
@font-face { font-family:'Figtree'; font-style:normal; font-display:swap; font-weight:600; src:url(/fonts/figtree/figtree-latin-600-normal.woff2) format('woff2'); unicode-range:U+0000-00FF,U+0131,U+0152-0153,U+02BB-02BC,U+02C6,U+02DA,U+02DC,U+0304,U+0308,U+0329,U+2000-206F,U+20AC,U+2122,U+2191,U+2193,U+2212,U+2215,U+FEFF,U+FFFD; }
@font-face { font-family:'Figtree'; font-style:normal; font-display:swap; font-weight:700; src:url(/fonts/figtree/figtree-latin-ext-700-normal.woff2) format('woff2'); unicode-range:U+0100-02BA,U+02BD-02C5,U+02C7-02CC,U+02CE-02D7,U+02DD-02FF,U+0304,U+0308,U+0329,U+1D00-1DBF,U+1E00-1E9F,U+1EF2-1EFF,U+2020,U+20A0-20AB,U+20AD-20C0,U+2113,U+2C60-2C7F,U+A720-A7FF; }
@font-face { font-family:'Figtree'; font-style:normal; font-display:swap; font-weight:700; src:url(/fonts/figtree/figtree-latin-700-normal.woff2) format('woff2'); unicode-range:U+0000-00FF,U+0131,U+0152-0153,U+02BB-02BC,U+02C6,U+02DA,U+02DC,U+0304,U+0308,U+0329,U+2000-206F,U+20AC,U+2122,U+2191,U+2193,U+2212,U+2215,U+FEFF,U+FFFD; }
:root { --iv-ground:#F5F3EF; --iv-card:#FFFFFF; --iv-border:#E7E3DC; --iv-ink:#1C1B19; --iv-body:#4A4741; --iv-muted:#6B675F;
  --iv-accent:#2C4A85; --iv-tint:#ECEFF8; --iv-alert:#B42318; --iv-rule:#EFECE6; --iv-soft:#FAF8F5; --iv-line:#D9D3C8; --iv-off:#E8E4DC;
  --iv-r:20px; --iv-r-sm:16px; --iv-font:'Figtree',system-ui,-apple-system,"Segoe UI",sans-serif; }
body { background:var(--iv-ground); }
.iv-sr { position:absolute; width:1px; height:1px; margin:-1px; padding:0; overflow:hidden; clip:rect(0 0 0 0); white-space:nowrap; border:0; }
/* ---- shared header and nav ---- */
.iv-head { font-family:var(--iv-font); color:var(--iv-ink); }
.iv-head-in { max-width:1180px; margin:0 auto; padding:20px 24px 4px; display:flex; flex-wrap:wrap; align-items:center; justify-content:space-between; gap:14px; }
.iv-head a:focus-visible, .iv-wrap a:focus-visible, .iv-wrap button:focus-visible, .run-chrome a:focus-visible { outline:2px solid var(--iv-accent); outline-offset:2px; }
.iv-brand { display:flex; align-items:center; gap:12px; color:inherit; text-decoration:none; min-height:44px; }
.iv-tile { flex:none; width:40px; height:40px; border-radius:12px; background:var(--iv-tint); display:flex; align-items:center; justify-content:center; }
.iv-name { display:block; font-weight:700; font-size:20px; line-height:1.1; color:var(--iv-ink); }
.iv-sub { display:block; font-size:12px; color:var(--iv-muted); }
.iv-sub.phone { display:none; }
.iv-head-right { display:flex; align-items:center; gap:12px; flex-wrap:wrap; }
.iv-nav { display:flex; gap:2px; background:var(--iv-card); border:1px solid var(--iv-border); border-radius:999px; padding:4px; }
.iv-nav a { display:inline-flex; align-items:center; min-height:44px; padding:0 16px; border-radius:999px; color:var(--iv-body); text-decoration:none; font-size:14px; font-weight:500; }
.iv-nav a:hover { color:var(--iv-ink); }
.iv-nav a[aria-current="page"] { color:var(--iv-accent); background:var(--iv-tint); font-weight:600; }
.iv-nav-icon { display:none; }
.iv-date { font-size:13px; color:var(--iv-muted); }
.iv-avatar { flex:none; width:40px; height:40px; border-radius:999px; background:#ECE8E1; color:var(--iv-ink); font-size:13px; font-weight:700; display:flex; align-items:center; justify-content:center; }
.iv-signin { margin:0; font-size:13px; color:var(--iv-muted); max-width:320px; }
main.wide { max-width:1180px; margin:0 auto; padding:24px 24px 40px; }
/* ---- shared page pieces ---- */
.iv-wrap { font-family:var(--iv-font); color:var(--iv-ink); }
.iv-wrap a { color:var(--iv-accent); }
.iv-wrap a:hover { color:var(--iv-ink); }
.iv-h1 { margin:0; font-weight:600; font-size:28px; line-height:1.2; color:var(--iv-ink); }
.iv-intro { margin:6px 0 0; font-size:14px; line-height:1.5; color:var(--iv-body); }
.iv-card { background:var(--iv-card); border:1px solid var(--iv-border); border-radius:var(--iv-r); }
.iv-kicker { margin:0; font-size:11px; font-weight:600; letter-spacing:1.2px; text-transform:uppercase; color:var(--iv-muted); }
.iv-foot { margin:0; font-size:12px; line-height:1.5; color:var(--iv-muted); }
.iv-spine { width:5px; flex:none; display:flex; flex-direction:column; gap:2px; }
.iv-spine span { flex:1; background:var(--iv-off); }
.iv-spine span.on { background:var(--iv-accent); }
.iv-spine.none span { background:var(--iv-rule); }
.iv-pips { color:var(--iv-accent); letter-spacing:1px; }
.iv-status { font-style:italic; color:var(--iv-muted); }
/* ---- Ask ---- */
.iv-ask { display:flex; flex-wrap:wrap; gap:24px; align-items:flex-start; }
.iv-ask-main { flex:999 1 600px; min-width:0; display:flex; flex-direction:column; gap:16px; }
.iv-side { flex:1 1 280px; max-width:340px; min-width:0; display:flex; flex-direction:column; gap:12px; }
.iv-form { display:flex; flex-direction:column; gap:16px; margin:0; }
.iv-qbox { overflow:hidden; }
.iv-qbox label.iv-qlabel { display:block; margin:0; padding:12px 16px 0; font-size:12px; font-weight:600; color:var(--iv-body); }
.iv-qbox textarea { display:block; width:100%; border:0; border-radius:0; resize:vertical; padding:6px 16px 12px; font-family:var(--iv-font); font-size:17px; line-height:1.5; color:var(--iv-ink); background:transparent; min-height:92px; }
.iv-qbox textarea:focus { outline:2px solid var(--iv-accent); outline-offset:-2px; }
.iv-pico-row { border-top:1px solid var(--iv-rule); padding:8px 12px 8px 16px; display:flex; flex-wrap:wrap; align-items:center; gap:6px; background:var(--iv-soft); }
.iv-pico-k { font-size:12px; font-weight:600; color:var(--iv-muted); margin-right:4px; }
.iv-chip { display:inline-flex; gap:4px; align-items:center; font-size:12px; padding:5px 10px; border-radius:999px; background:var(--iv-card); color:var(--iv-muted); border:1px dashed var(--iv-line); }
.iv-chip.on { background:var(--iv-accent); color:#fff; border:1px solid var(--iv-accent); }
.iv-chip b { font-weight:600; }
.iv-wrap button.iv-ghost { margin-left:auto; border:1px solid var(--iv-line); background:var(--iv-card); color:var(--iv-accent); font-family:inherit; font-size:12px; font-weight:600; padding:0 12px; border-radius:8px; min-height:44px; cursor:pointer; }
.iv-pico-panel { border-top:1px solid var(--iv-rule); padding:18px; display:grid; grid-template-columns:repeat(2,minmax(0,1fr)); gap:14px; }
.iv-field { display:flex; flex-direction:column; gap:6px; }
.iv-field label { margin:0; font-size:12px; font-weight:600; color:var(--iv-body); }
.iv-field input { font-family:inherit; font-size:14px; padding:10px 12px; min-height:44px; border:1px solid var(--iv-line); border-radius:8px; color:var(--iv-ink); background:var(--iv-card); }
.iv-field input:focus { outline:2px solid var(--iv-accent); outline-offset:1px; }
.iv-composed { grid-column:1 / -1; display:flex; flex-wrap:wrap; align-items:center; gap:12px; background:var(--iv-ground); border-radius:10px; padding:12px 14px; }
.iv-composed p { flex:1 1 320px; margin:0; font-size:15px; line-height:1.45; color:var(--iv-body); }
.iv-wrap button.iv-primary { border:0; background:var(--iv-accent); color:#fff; font-family:inherit; font-size:13px; font-weight:600; padding:0 14px; border-radius:8px; min-height:44px; cursor:pointer; }
.iv-wrap button.iv-primary[disabled] { background:var(--iv-line); color:var(--iv-body); opacity:1; cursor:default; }
.iv-privacy { margin:-6px 0 0; display:flex; align-items:flex-start; gap:8px; font-size:12px; line-height:1.45; color:var(--iv-body); }
.iv-privacy svg { flex:none; margin-top:1px; }
.iv-try { display:flex; flex-wrap:wrap; align-items:center; gap:6px; }
.iv-try-k { font-size:12px; font-weight:600; color:var(--iv-muted); margin-right:4px; }
.iv-wrap button.iv-starter { border:1px solid var(--iv-border); background:var(--iv-card); color:var(--iv-ink); font-family:inherit; font-size:12px; padding:0 12px; border-radius:999px; min-height:44px; cursor:pointer; text-align:left; }
fieldset.iv-depth { margin:0; min-width:0; padding:12px; display:flex; flex-wrap:wrap; align-items:stretch; gap:10px; }
.iv-depth-cards { flex:1 1 420px; display:grid; grid-template-columns:repeat(2,minmax(0,1fr)); gap:8px; }
label.iv-mode { display:block; position:relative; margin:0; font-weight:400; }
label.iv-mode input { position:absolute; opacity:0; width:1px; height:1px; margin:0; }
.iv-mode-card { display:flex; flex-direction:column; gap:4px; height:100%; color:var(--iv-ink); cursor:pointer; padding:10px 12px; border-radius:10px; min-height:44px; background:var(--iv-card); border:1.5px solid var(--iv-border); }
.iv-mode-top { display:flex; align-items:center; gap:8px; }
.iv-radio { flex:none; width:16px; height:16px; border-radius:999px; border:2px solid #BDB6AA; background:#fff; }
.iv-mode-name { font-size:15px; font-weight:600; }
.iv-mode-time { margin-left:auto; font-size:12px; color:var(--iv-muted); }
.iv-mode-desc { font-size:12px; line-height:1.4; color:var(--iv-body); padding-left:24px; }
label.iv-mode input:checked + .iv-mode-card { background:var(--iv-tint); border-color:var(--iv-accent); }
label.iv-mode input:checked + .iv-mode-card .iv-radio { border:5px solid var(--iv-accent); }
label.iv-mode input:focus-visible + .iv-mode-card { outline:2px solid var(--iv-accent); outline-offset:2px; }
.iv-wrap button.iv-send { flex:0 0 auto; border:0; font-family:inherit; font-size:15px; font-weight:600; padding:0 22px; border-radius:10px; min-height:48px; cursor:pointer; background:var(--iv-accent); color:#fff; }
.iv-wrap button.iv-send[disabled] { background:var(--iv-line); color:var(--iv-body); opacity:1; cursor:default; }
.iv-steps { flex:1 1 100%; margin:0; display:flex; flex-wrap:wrap; align-items:center; gap:4px; font-size:11px; color:var(--iv-muted); border-top:1px dashed #ECE8E1; padding-top:8px; }
.iv-step { font-weight:600; padding:2px 7px; border-radius:5px; background:var(--iv-tint); color:var(--iv-accent); }
.iv-steps-note { margin-left:auto; }
.iv-latest { display:flex; flex-direction:column; gap:8px; margin-top:4px; }
.iv-latest-head { display:flex; justify-content:space-between; align-items:baseline; gap:8px; }
.iv-latest-head a { font-size:13px; font-weight:600; text-decoration:none; min-height:44px; display:inline-flex; align-items:center; }
.iv-latest-grid { display:grid; grid-template-columns:repeat(2,minmax(0,1fr)); gap:10px; }
.iv-wrap a.iv-lc { text-decoration:none; color:var(--iv-ink); background:var(--iv-card); border:1px solid var(--iv-border); border-radius:var(--iv-r-sm); display:flex; overflow:hidden; min-height:108px; }
.iv-lc-body { flex:1; min-width:0; padding:10px 12px; display:flex; flex-direction:column; gap:5px; }
.iv-lc-top { display:flex; justify-content:space-between; gap:8px; font-size:11px; color:var(--iv-muted); }
.iv-lc-topic { white-space:nowrap; overflow:hidden; text-overflow:ellipsis; min-width:0; }
.iv-lc-cert { flex:none; }
.iv-lc-verdict { font-size:15px; line-height:1.4; color:var(--iv-ink); display:-webkit-box; -webkit-line-clamp:4; line-clamp:4; -webkit-box-orient:vertical; overflow:hidden; }
.iv-lc-verdict.iv-status { font-size:14px; color:var(--iv-muted); }
.iv-lc-meta { margin-top:auto; font-size:11px; color:var(--iv-muted); }
.iv-empty { margin:0; padding:20px; text-align:center; font-size:14px; color:var(--iv-body); background:var(--iv-card); border:1px dashed var(--iv-line); border-radius:var(--iv-r); }
.iv-today { padding:14px 16px; }
.iv-today-big { display:flex; align-items:baseline; gap:8px; margin-top:4px; }
.iv-today-big b { font-size:24px; font-weight:600; }
.iv-today-big span, .iv-today-line { font-size:13px; color:var(--iv-muted); }
.iv-today-line { margin:4px 0 0; color:var(--iv-body); }
.iv-check { padding:14px 16px; display:flex; flex-direction:column; gap:10px; }
.iv-check-head { display:flex; justify-content:space-between; align-items:baseline; }
.iv-check-score { font-size:12px; font-weight:600; color:var(--iv-accent); }
.iv-check-bar { display:flex; gap:3px; }
.iv-check-bar span { flex:1; height:5px; border-radius:3px; background:var(--iv-off); }
.iv-check-bar span.ok { background:var(--iv-accent); }
.iv-check-bar span.warn { background:var(--iv-alert); }
.iv-check ul { list-style:none; margin:0; padding:0; display:flex; flex-direction:column; gap:8px; }
.iv-check li { display:flex; gap:10px; align-items:flex-start; font-size:13px; line-height:1.4; }
.iv-dot { flex:none; width:18px; height:18px; border-radius:999px; font-size:11px; font-weight:700; display:flex; align-items:center; justify-content:center; margin-top:1px; background:#fff; border:1.5px solid #BDB6AA; color:#fff; }
.iv-dot.ok { background:var(--iv-accent); border-color:var(--iv-accent); }
.iv-dot.warn { background:var(--iv-alert); border-color:var(--iv-alert); }
.iv-check-label { font-weight:600; color:var(--iv-body); }
.iv-check-label.ok { color:var(--iv-ink); }
.iv-check-label.warn { color:var(--iv-alert); }
.iv-check-hint { display:block; font-size:12px; color:var(--iv-muted); }
.iv-tip { margin:0; font-size:12px; line-height:1.45; color:var(--iv-body); background:var(--iv-ground); border-radius:8px; padding:8px 10px; }
.iv-advisory { margin:0; font-size:11px; line-height:1.4; color:var(--iv-muted); }
/* ---- Library ---- */
.iv-lib { display:flex; flex-direction:column; gap:18px; }
.iv-lib-head { display:flex; flex-wrap:wrap; align-items:flex-end; justify-content:space-between; gap:14px; }
.iv-wrap a.iv-new { text-decoration:none; background:var(--iv-accent); color:#fff; font-weight:700; font-size:14px; padding:0 18px; border-radius:14px; min-height:46px; display:inline-flex; align-items:center; gap:8px; }
.iv-wrap a.iv-new:hover { color:#fff; background:#223a6a; }
.iv-tools { display:flex; flex-wrap:wrap; align-items:center; gap:10px; padding:10px; }
.iv-search { flex:1 1 260px; display:flex; align-items:center; gap:8px; padding:0 12px; border:1px solid var(--iv-border); border-radius:12px; background:var(--iv-soft); }
.iv-search input { flex:1; min-width:0; border:0; background:transparent; font-family:inherit; font-size:14px; padding:11px 0; min-height:44px; color:var(--iv-ink); }
.iv-search:focus-within { outline:2px solid var(--iv-accent); outline-offset:1px; }
.iv-search input:focus { outline:none; }
.iv-filters { display:flex; flex-wrap:wrap; gap:2px; background:var(--iv-ground); border-radius:22px; padding:4px; }
.iv-wrap button.iv-filter { border:0; font-family:inherit; font-size:13px; font-weight:600; padding:0 14px; border-radius:999px; cursor:pointer; min-height:44px; background:transparent; color:var(--iv-body); }
.iv-wrap button.iv-filter[aria-pressed="true"] { background:var(--iv-card); color:var(--iv-accent); box-shadow:0 1px 2px rgba(28,27,25,0.12); }
.iv-day { display:flex; flex-wrap:wrap; align-items:baseline; gap:10px; padding:4px 2px 0; }
.iv-day-sum { font-size:13px; color:var(--iv-muted); }
.iv-grid { display:grid; grid-template-columns:repeat(auto-fill,minmax(min(100%,480px),1fr)); gap:12px; }
.iv-libcard { background:var(--iv-card); border:1px solid var(--iv-border); border-radius:var(--iv-r); display:flex; overflow:hidden; }
.iv-libcard .iv-spine { width:6px; }
.iv-lib-body { flex:1; min-width:0; padding:16px 18px; display:flex; flex-direction:column; gap:8px; }
.iv-lib-top { display:flex; flex-wrap:wrap; align-items:center; gap:8px; font-size:12px; color:var(--iv-muted); }
.iv-modepill { font-weight:700; color:var(--iv-accent); background:var(--iv-tint); padding:3px 9px; border-radius:999px; }
.iv-lib-cert { margin-left:auto; display:inline-flex; align-items:center; gap:6px; color:var(--iv-body); }
.iv-wrap a.iv-lib-main { text-decoration:none; color:var(--iv-ink); display:flex; flex-direction:column; gap:4px; }
.iv-lib-q { font-size:13px; line-height:1.45; color:var(--iv-muted); overflow-wrap:anywhere; display:-webkit-box; -webkit-line-clamp:3; line-clamp:3; -webkit-box-orient:vertical; overflow:hidden; }
.iv-lib-verdict { font-size:18px; line-height:1.4; font-weight:600; color:var(--iv-ink); }
.iv-lib-verdict.iv-status { font-size:16px; font-weight:400; color:var(--iv-muted); }
details.iv-thread { border:1px solid var(--iv-rule); border-radius:14px; background:var(--iv-soft); }
details.iv-thread > summary { list-style:none; display:flex; align-items:center; gap:10px; font-size:13px; color:var(--iv-ink); padding:0 12px; cursor:pointer; min-height:44px; }
details.iv-thread > summary::-webkit-details-marker { display:none; }
details.iv-thread > summary:focus-visible { outline:2px solid var(--iv-accent); outline-offset:2px; }
.iv-dots { display:inline-flex; gap:3px; }
.iv-dots span { width:8px; height:8px; border-radius:999px; background:#BDB6AA; }
.iv-dots span:first-child { background:var(--iv-accent); }
.iv-thread-n { font-weight:700; }
.iv-thread-sub { color:var(--iv-muted); }
.iv-thread-toggle { margin-left:auto; color:var(--iv-accent); font-weight:700; }
.iv-thread-toggle::after { content:'Show runs'; }
details.iv-thread[open] .iv-thread-toggle::after { content:'Hide'; }
.iv-thread ol { list-style:none; margin:0; border-top:1px solid var(--iv-rule); padding:4px 12px 8px; display:flex; flex-direction:column; }
.iv-thread li { display:flex; align-items:center; gap:12px; padding:2px 0; border-bottom:1px dashed var(--iv-border); font-size:13px; }
.iv-thread li:last-child { border-bottom:0; }
.iv-thread-time { min-width:52px; font-weight:700; }
.iv-thread-note { flex:1; color:var(--iv-body); }
.iv-thread li a { font-weight:700; text-decoration:none; min-height:44px; padding:0 4px; display:inline-flex; align-items:center; }
.iv-actions { display:flex; flex-wrap:wrap; align-items:center; gap:4px; margin-top:auto; padding-top:4px; border-top:1px solid var(--iv-rule); }
.iv-actions a { font-size:13px; font-weight:600; text-decoration:none; padding:0 10px; min-height:44px; display:inline-flex; align-items:center; }
.iv-actions a.iv-read { font-weight:700; padding-left:0; }
.iv-wrap .iv-actions button.iv-copy { border:0; background:transparent; color:var(--iv-accent); font-family:inherit; font-size:13px; font-weight:600; padding:0 10px; min-height:44px; cursor:pointer; border-radius:8px; }
.iv-actions textarea.copy-area { position:absolute; left:-9999px; width:1px; height:1px; opacity:0; }
.iv-actions textarea.copy-area.shown { position:static; width:100%; height:10rem; opacity:1; margin-top:8px; font-size:.8rem; }
.iv-del { margin-left:auto; display:inline-flex; align-items:center; gap:6px; }
.iv-del-q { font-size:13px; color:var(--iv-alert); font-weight:600; }
.iv-wrap .iv-del button { font-family:inherit; font-size:13px; min-height:44px; cursor:pointer; border-radius:999px; }
.iv-wrap .iv-del button.iv-del-icon { width:44px; padding:0; border:0; background:transparent; color:var(--iv-muted); display:inline-flex; align-items:center; justify-content:center; }
.iv-wrap .iv-del button.iv-del-icon:hover { color:var(--iv-alert); }
.iv-wrap .iv-del button.iv-keep { border:1px solid var(--iv-line); background:var(--iv-card); color:var(--iv-ink); padding:0 12px; }
.iv-wrap .iv-del button.iv-del-yes { border:0; background:var(--iv-alert); color:#fff; font-weight:700; padding:0 12px; }
.iv-danger { padding-top:4px; }
.iv-wrap button.iv-delete-all { margin-left:0; }
.iv-danger .box p { margin:0 0 8px; }
/* ---- the run page chrome (the answer content is not touched) ---- */
.run-chrome, .run-back { font-family:var(--iv-font); }
.run-back a { color:var(--iv-accent); font-weight:600; text-decoration:none; display:inline-flex; align-items:center; min-height:44px; }
.run-chrome .run-question { font-size:1.2rem; margin:.83em 0; font-weight:600; color:var(--iv-ink); line-height:1.3; }
.activity-title { font-size:1.5em; margin:.83em 0; font-weight:bold; }
.run-chrome .badge { font-family:var(--iv-font); font-weight:600; }
.run-chrome .badge.mode { background:var(--iv-tint); color:var(--iv-accent); }
.run-chrome h3, .run-fold > summary h3 { font-family:var(--iv-font); font-weight:600; color:var(--iv-ink); }
.run-chrome .run-fold { border-color:var(--iv-border); }
.run-fold > summary .ap-more { font-family:var(--iv-font); color:var(--iv-accent); }
/* ---- phones ---- */
@media (max-width:640px) {
  body { padding-bottom:84px; }
  .iv-head-in { padding:16px 16px 6px; flex-wrap:nowrap; gap:10px; }
  .iv-brand { flex:1; min-width:0; gap:10px; }
  .iv-tile { width:36px; height:36px; border-radius:11px; }
  .iv-name { font-size:18px; }
  .iv-sub { font-size:11px; }
  .iv-sub.wide, .iv-date { display:none; }
  .iv-sub.phone { display:block; }
  .iv-avatar { width:38px; height:38px; }
  .iv-nav { position:fixed; left:0; right:0; bottom:0; z-index:20; display:grid; grid-auto-flow:column; grid-auto-columns:minmax(0,1fr); gap:0; padding:0 0 env(safe-area-inset-bottom); border:0; border-top:1px solid var(--iv-border); border-radius:0; }
  .iv-nav a { flex-direction:column; justify-content:center; gap:4px; padding:10px 0 16px; border-radius:0; font-size:12px; color:var(--iv-muted); }
  .iv-nav a[aria-current="page"] { background:transparent; color:var(--iv-accent); font-weight:700; }
  .iv-nav-icon { display:flex; width:44px; height:28px; border-radius:999px; align-items:center; justify-content:center; }
  .iv-nav a[aria-current="page"] .iv-nav-icon { background:var(--iv-tint); }
  main.wide { padding:10px 16px 16px; }
  .iv-h1 { font-size:24px; font-weight:700; }
  .iv-intro { font-size:13px; }
  .iv-ask, .iv-ask-main, .iv-form { gap:14px; }
  .iv-side { max-width:none; }
  .iv-today { display:none; }
  .iv-qbox textarea { font-size:16px; padding:6px 14px 12px; }
  .iv-pico-k, .iv-chip .iv-chip-label { display:none; }
  .iv-chip { width:26px; height:26px; padding:0; justify-content:center; font-size:11px; }
  .iv-wrap button.iv-ghost { border-radius:999px; }
  .iv-pico-panel { grid-template-columns:1fr; padding:14px; }
  fieldset.iv-depth { flex-direction:column; padding:10px; }
  .iv-depth-cards { flex:none; }
  .iv-mode-card { min-height:64px; border-radius:14px; padding:10px; }
  .iv-mode-top { gap:7px; }
  .iv-mode-name { font-weight:700; }
  .iv-mode-time { margin-left:0; }
  .iv-mode-desc { padding-left:0; font-size:11px; line-height:1.35; }
  .iv-wrap button.iv-send { width:100%; min-height:50px; border-radius:14px; font-weight:700; }
  .iv-steps { justify-content:center; border-top:0; padding-top:0; }
  .iv-steps-note { display:none; }
  .iv-latest-grid { gap:8px; }
  .iv-wrap a.iv-lc { min-height:118px; }
  .iv-lc-top { flex-direction:column; gap:4px; font-size:10px; }
  .iv-lc-verdict { font-size:13px; line-height:1.35; font-weight:600; }
  .iv-lc-verdict.iv-status { font-size:12px; font-weight:400; }
  .iv-lc-meta { font-size:10px; }
  .iv-check { padding:12px 14px; gap:8px; }
  .iv-check ul { display:grid; grid-template-columns:repeat(2,minmax(0,1fr)); gap:6px 10px; }
  .iv-check li { font-size:12px; gap:6px; align-items:center; }
  .iv-check-hint { display:none; }
  .iv-dot { width:16px; height:16px; font-size:10px; margin-top:0; }
  .iv-tools { padding:0; background:transparent; border:0; }
  .iv-search { flex-basis:100%; background:var(--iv-card); border-radius:14px; }
  .iv-filters { flex-wrap:nowrap; overflow-x:auto; background:transparent; padding:0; gap:6px; max-width:100%; }
  .iv-wrap button.iv-filter { flex:none; background:var(--iv-card); border:1px solid var(--iv-border); }
  .iv-wrap button.iv-filter[aria-pressed="true"] { background:var(--iv-accent); color:#fff; border-color:var(--iv-accent); box-shadow:none; }
  .iv-filter .iv-pips { display:none; }
  .iv-lib-body { padding:12px 14px; gap:6px; }
  .iv-lib-verdict { font-size:16px; }
  .iv-lib-verdict.iv-status { font-size:14px; }
}
"#;

/// Which page the nav marks as current.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NavPage {
    Ask,
    Library,
    Activity,
    Other,
}

/// The current page for a path.
pub fn nav_page(path: &str) -> NavPage {
    match path.trim_end_matches('/') {
        "" => NavPage::Ask,
        "/history" => NavPage::Library,
        "/activity" => NavPage::Activity,
        _ => NavPage::Other,
    }
}

/// Ask and the Library use the wide ivory layout; the run page and
/// Activity keep today's width.
pub fn is_wide(path: &str) -> bool {
    matches!(nav_page(path), NavPage::Ask | NavPage::Library)
}

fn mark_svg() -> impl IntoView {
    view! {
        <svg width="22" height="22" viewBox="0 0 30 30" aria-hidden="true" focusable="false">
            <path d="M8 22 L22 8" stroke="#2C4A85" stroke-width="2.6" stroke-linecap="round"></path>
            <circle cx="22" cy="8" r="2.4" fill="#2C4A85"></circle>
        </svg>
    }
}

fn nav_icon(page: NavPage) -> AnyView {
    match page {
        NavPage::Ask => view! {
            <svg width="18" height="18" viewBox="0 0 20 20" aria-hidden="true" focusable="false">
                <path d="M4 16 L16 4" stroke="currentColor" stroke-width="2" stroke-linecap="round"></path>
                <circle cx="16" cy="4" r="1.8" fill="currentColor"></circle>
            </svg>
        }
        .into_any(),
        NavPage::Library => view! {
            <svg width="18" height="18" viewBox="0 0 20 20" aria-hidden="true" focusable="false">
                <rect x="3" y="3" width="4" height="14" rx="1" fill="none" stroke="currentColor" stroke-width="1.6"></rect>
                <rect x="9" y="3" width="4" height="14" rx="1" fill="none" stroke="currentColor" stroke-width="1.6"></rect>
                <path d="M15 4 L18 16" stroke="currentColor" stroke-width="1.6" stroke-linecap="round"></path>
            </svg>
        }
        .into_any(),
        _ => view! {
            <svg width="18" height="18" viewBox="0 0 20 20" aria-hidden="true" focusable="false">
                <path d="M2 11 H6 L8 5 L12 15 L14 11 H18" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"></path>
            </svg>
        }
        .into_any(),
    }
}

/// The shared header: the tinted logo tile, the name and subtitle, the nav
/// pill (a bottom tab bar on a phone, by CSS: one `<nav>`), today's date in
/// IST and the initials avatar. The sign-in states are as before.
#[component]
pub fn SiteHeader() -> impl IntoView {
    let me = use_context::<MeResource>().expect("App provides the sign-in resource");
    let path = use_location().pathname;
    let current = move || nav_page(&path.get());
    let date = lib::header_date(lib::now_utc());
    let phone_sub = format!("{date} \u{b7} Medical literature, summarised");
    let link = move |page: NavPage, href: &'static str, label: &'static str| {
        view! {
            <a href=href aria-current=move || (current() == page).then_some("page")>
                <span class="iv-nav-icon" aria-hidden="true">{nav_icon(page)}</span>
                {label}
            </a>
        }
    };
    view! {
        <header class="iv-head">
            <div class="iv-head-in">
                <a class="iv-brand" href="/">
                    <span class="iv-tile" aria-hidden="true">{mark_svg()}</span>
                    <span>
                        <span class="iv-name">"Incisive"</span>
                        <span class="iv-sub wide">"Medical literature \u{2014} searched, checked, summarised"</span>
                        <span class="iv-sub phone">{phone_sub}</span>
                    </span>
                </a>
                <div class="iv-head-right">
                    <nav class="iv-nav" aria-label="Main">
                        {link(NavPage::Ask, "/", "Ask")}
                        {link(NavPage::Library, "/history", "Library")}
                        // Activity is the owner's page, shown once sign-in is known.
                        <Suspense fallback=|| ()>
                            {move || {
                                me.get()
                                    .and_then(|r| r.ok())
                                    .filter(|m| m.role == "owner")
                                    .map(|_| link(NavPage::Activity, "/activity", "Activity"))
                            }}
                        </Suspense>
                    </nav>
                    <span class="iv-date">{date}</span>
                    <Suspense fallback=|| view! { <p class="iv-signin">"Checking sign-in\u{2026}"</p> }>
                        {move || {
                            me.get()
                                .map(|r| match r {
                                    Ok(m) => {
                                        let who = format!("Signed in as {}", m.display_name);
                                        view! {
                                            <span class="iv-avatar" role="img" aria-label=who.clone() title=who>
                                                {lib::initials(&m.display_name)}
                                            </span>
                                        }
                                            .into_any()
                                    }
                                    Err(e) => view! { <p class="iv-signin">{error_text(&e)}</p> }.into_any(),
                                })
                        }}
                    </Suspense>
                </div>
            </div>
        </header>
    }
}

/// `<main>`, wide on Ask and the Library.
#[component]
pub fn PageMain(children: Children) -> impl IntoView {
    let path = use_location().pathname;
    let class = move || is_wide(&path.get()).then_some("wide");
    view! { <main class=class>{children()}</main> }
}

// ---------- shared card pieces ----------

pub(super) fn spine_view(level: Option<u8>) -> impl IntoView {
    let s = lib::spine(level);
    let class = if level.is_none() {
        "iv-spine none"
    } else {
        "iv-spine"
    };
    view! {
        <span class=class aria-hidden="true">
            {s.into_iter().map(|on| view! { <span class:on=on></span> }).collect_view()}
        </span>
    }
}

pub(super) fn cert_view(level: Option<u8>) -> impl IntoView {
    level.map(|l| {
        view! {
            <span class="iv-pips" aria-hidden="true">{lib::pips(l)}</span>
            " "
            {lib::certainty_name(l)}
            <span class="iv-sr">" certainty"</span>
        }
    })
}

/// The verdict, or the run's status when it has no answer.
pub(super) fn verdict_or_status(c: &LibraryCard, class: &'static str) -> AnyView {
    match &c.verdict {
        Some(v) => view! { <span class=class>{v.clone()}</span> }.into_any(),
        None => view! { <span class=format!("{class} iv-status")>{lib::no_answer_text(c.status)}</span> }
            .into_any(),
    }
}

pub(super) fn asked_times(n: usize) -> String {
    format!("asked {n} times")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nav_marks_pages_by_path() {
        assert_eq!(nav_page("/"), NavPage::Ask);
        assert_eq!(nav_page(""), NavPage::Ask);
        assert_eq!(nav_page("/history"), NavPage::Library);
        assert_eq!(nav_page("/history/"), NavPage::Library);
        assert_eq!(nav_page("/activity"), NavPage::Activity);
        assert_eq!(nav_page("/runs/abc"), NavPage::Other);
        assert!(is_wide("/") && is_wide("/history"));
        assert!(!is_wide("/activity") && !is_wide("/runs/x"));
    }
}
