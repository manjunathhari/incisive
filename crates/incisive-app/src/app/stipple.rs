//! The stipple look (`LOOK=stipple`): its two faces, the day colours and the
//! dot layer, laid over the ivory styles.
//!
//! Sent only when the look is on (`shell`), after the ivory block, and every
//! rule starts with `html[data-look="stipple"]`: with `LOOK` unset or `ivory`
//! nothing here reaches the page. Only page chrome is restyled (Ask, the
//! Library, Explore, the run page's tracker, introduction and draft, the
//! Expert view and its board, the chat, Activity and the plain pages): the
//! answer content (`.ap-main` and the original run view) is left alone.
//!
//! The `st-*` classes belong to markup drawn only in this look: the header
//! badge and wordmark (`ivory::SiteHeader`), the Ask page's hero and label
//! (`ask_page`), the footer (`ivory::PageMain`) and Explore's label and
//! heading (`explore`). The certainty spine is not restyled.

/// The Hanken Grotesk and Source Serif 4 faces (self-hosted, OFL;
/// `public/fonts/hanken-grotesk`, `public/fonts/source-serif-4`; the
/// `unicode-range` split is fontsource's), the day values of the `--iv-*`
/// variables (and of the other chrome colours that would clash with them;
/// the rest keep their ivory values), the fixed dot layer (`public/look`),
/// the weight cap (the faces stop at 500, so every heavier rule of the
/// chrome styles is restated; `tests/look.rs`), and the chrome pages in this
/// look. One rule to a line, each under `html[data-look="stipple"]` (the
/// scope test reads it line by line).
///
/// `--iv-faint` is the fading tail of a draft being written: lighter than
/// the muted text on purpose (about 3.1:1 on a white card; ivory's is about
/// 3.6:1), since it marks words that may still change.
pub const STIPPLE_STYLE: &str = r#"
@font-face { font-family:'Hanken Grotesk Variable'; font-style:normal; font-display:swap; font-weight:300 500; src:url(/fonts/hanken-grotesk/hanken-grotesk-latin-ext-wght-normal.woff2) format('woff2-variations'); unicode-range:U+0100-02BA,U+02BD-02C5,U+02C7-02CC,U+02CE-02D7,U+02DD-02FF,U+0304,U+0308,U+0329,U+1D00-1DBF,U+1E00-1E9F,U+1EF2-1EFF,U+2020,U+20A0-20AB,U+20AD-20C0,U+2113,U+2C60-2C7F,U+A720-A7FF; }
@font-face { font-family:'Hanken Grotesk Variable'; font-style:normal; font-display:swap; font-weight:300 500; src:url(/fonts/hanken-grotesk/hanken-grotesk-latin-wght-normal.woff2) format('woff2-variations'); unicode-range:U+0000-00FF,U+0131,U+0152-0153,U+02BB-02BC,U+02C6,U+02DA,U+02DC,U+0304,U+0308,U+0329,U+2000-206F,U+20AC,U+2122,U+2191,U+2193,U+2212,U+2215,U+FEFF,U+FFFD; }
@font-face { font-family:'Source Serif 4 Variable'; font-style:normal; font-display:swap; font-weight:400; src:url(/fonts/source-serif-4/source-serif-4-latin-ext-wght-normal.woff2) format('woff2-variations'); unicode-range:U+0100-02BA,U+02BD-02C5,U+02C7-02CC,U+02CE-02D7,U+02DD-02FF,U+0304,U+0308,U+0329,U+1D00-1DBF,U+1E00-1E9F,U+1EF2-1EFF,U+2020,U+20A0-20AB,U+20AD-20C4,U+2113,U+2C60-2C7F,U+A720-A7FF; }
@font-face { font-family:'Source Serif 4 Variable'; font-style:normal; font-display:swap; font-weight:400; src:url(/fonts/source-serif-4/source-serif-4-latin-wght-normal.woff2) format('woff2-variations'); unicode-range:U+0000-00FF,U+0131,U+0152-0153,U+02BB-02BC,U+02C6,U+02DA,U+02DC,U+0304,U+0308,U+0329,U+2000-206F,U+20AC,U+2122,U+2191,U+2193,U+2212,U+2215,U+FEFF,U+FFFD; }
html[data-look="stipple"] { color-scheme: light; background:var(--iv-ground);
  --iv-ground:#f7f4ec; --iv-card:#ffffff; --iv-border:#e4ded0; --iv-ink:#101a24; --iv-body:#101a24; --iv-muted:#5b6773;
  --iv-accent:#1f6bd0; --iv-tint:#e3edf8; --iv-rule:#ece7db; --iv-soft:#fbf9f4; --iv-line:#d6cfbf; --iv-off:#e4ded0;
  --iv-r:18px; --iv-r-sm:16px;
  --iv-font:"Hanken Grotesk Variable","Hanken Grotesk",system-ui,-apple-system,"Segoe UI",Roboto,"Helvetica Neue",Arial,sans-serif;
  --iv-read:"Source Serif 4 Variable","Source Serif 4",Georgia,"Times New Roman",serif;
  --iv-button:#13263c; --iv-on-button:#ffffff; --iv-fill:#1e5aa0; --iv-on-fill:#ffffff; --st-mask:#000;
  --tr-filling:var(--iv-fill); --tr-fill:var(--iv-tint); --tr-ring:var(--iv-accent); --tr-role-bg:var(--iv-tint);
  --iv-avatar-bg:var(--iv-border); --iv-dash:var(--iv-border); --iv-accent-deep:var(--iv-fill); --iv-faint:#8a94a0; }
/* The cream sits on the page root and the body is clear, so the fixed dot layer (behind everything) shows over it. */
html[data-look="stipple"] body { background:transparent; }
html[data-look="stipple"] body::before { content:""; position:fixed; inset:0; z-index:-1; pointer-events:none; background:url(/look/stipple-day.svg) 0 0/480px 480px repeat; }
/* ---- weight cap: the ivory 600 and 700 rules restated (headings light, the rest medium) ---- */
html[data-look="stipple"] .iv-name { font-weight:300; }
html[data-look="stipple"] .iv-h1 { font-weight:300; }
html[data-look="stipple"] .iv-sub b { font-weight:500; }
html[data-look="stipple"] .iv-nav a[aria-current="page"] { font-weight:500; }
html[data-look="stipple"] .iv-avatar { font-weight:500; }
html[data-look="stipple"] .iv-kicker { font-weight:500; }
html[data-look="stipple"] .iv-qbox label.iv-qlabel { font-weight:500; }
html[data-look="stipple"] .iv-count.over { font-weight:500; }
html[data-look="stipple"] .iv-pico-k { font-weight:500; }
html[data-look="stipple"] .iv-chip b { font-weight:500; }
html[data-look="stipple"] .iv-wrap button.iv-ghost { font-weight:500; }
html[data-look="stipple"] .iv-field label { font-weight:500; }
html[data-look="stipple"] .iv-wrap button.iv-primary { font-weight:500; }
html[data-look="stipple"] .iv-try-k { font-weight:500; }
html[data-look="stipple"] .iv-mode-name { font-weight:500; }
html[data-look="stipple"] .iv-wrap button.iv-send { font-weight:500; }
html[data-look="stipple"] .iv-step { font-weight:500; }
html[data-look="stipple"] .iv-latest-head a { font-weight:500; }
html[data-look="stipple"] .iv-today-big b { font-weight:500; }
html[data-look="stipple"] .iv-check-score { font-weight:500; }
html[data-look="stipple"] .iv-dot { font-weight:500; }
html[data-look="stipple"] .iv-check-label { font-weight:500; }
html[data-look="stipple"] .iv-wrap a.iv-new { font-weight:500; }
html[data-look="stipple"] .iv-wrap button.iv-filter { font-weight:500; }
html[data-look="stipple"] .iv-modepill { font-weight:500; }
html[data-look="stipple"] .iv-lib-verdict { font-weight:500; }
html[data-look="stipple"] .iv-thread-n { font-weight:500; }
html[data-look="stipple"] .iv-thread-toggle { font-weight:500; }
html[data-look="stipple"] .iv-thread-time { font-weight:500; }
html[data-look="stipple"] .iv-thread li a { font-weight:500; }
html[data-look="stipple"] .iv-actions a { font-weight:500; }
html[data-look="stipple"] .iv-actions a.iv-read { font-weight:500; }
html[data-look="stipple"] .iv-wrap .iv-actions button.iv-copy { font-weight:500; }
html[data-look="stipple"] .iv-del-q { font-weight:500; }
html[data-look="stipple"] .iv-wrap .iv-del button.iv-del-yes { font-weight:500; }
/* A status line in the verdict place stays regular, as in ivory. */
html[data-look="stipple"] .iv-lib-verdict.iv-status { font-weight:400; }
@media (max-width:640px) {
  html[data-look="stipple"] .iv-lc-verdict { font-weight:500; }
  html[data-look="stipple"] .iv-lc-verdict.iv-status { font-weight:400; }
}
/* ---- header: the round badge and the wordmark (the subtitle lines are not drawn) ---- */
html[data-look="stipple"] .st-badge { flex:none; width:34px; height:34px; border-radius:999px; background:var(--iv-ink); color:var(--iv-ground); display:flex; align-items:center; justify-content:center; }
html[data-look="stipple"] .st-badge svg { width:20px; height:20px; }
/* The mark takes the badge's colour (the ivory drawing is untouched). */
html[data-look="stipple"] .st-badge svg path { stroke:currentColor; }
html[data-look="stipple"] .st-badge svg circle { fill:currentColor; }
html[data-look="stipple"] .st-wordmark { display:block; font-family:var(--iv-font); font-size:19px; font-weight:300; line-height:1.1; letter-spacing:.08em; text-transform:uppercase; white-space:nowrap; color:var(--iv-ink); }
html[data-look="stipple"] .iv-head a:focus-visible, html[data-look="stipple"] .iv-ask a:focus-visible, html[data-look="stipple"] .iv-ask button:focus-visible { outline:3px solid var(--iv-accent); outline-offset:2px; }
/* ---- Ask: the hero (label, heading and intro over the artwork) ---- */
html[data-look="stipple"] .st-hero { position:relative; isolation:isolate; display:flex; flex-direction:column; justify-content:flex-end; min-height:330px; padding-bottom:28px; }
html[data-look="stipple"] .st-hero > :not(.st-hero-art) { max-width:640px; }
html[data-look="stipple"] .st-hero-art { position:absolute; z-index:-1; pointer-events:none; top:0; left:50%; right:auto; width:min(100%, 390px); height:440px; transform:translateX(-50%); background-image:url(/look/sai-hero-day.webp); background-repeat:no-repeat; background-position:center top; background-size:100% auto; -webkit-mask-image:radial-gradient(ellipse 60% 55% at 60% 38%, var(--st-mask) 40%, transparent 100%), linear-gradient(to bottom, var(--st-mask) 50%, transparent 92%); -webkit-mask-composite:source-in; mask-image:radial-gradient(ellipse 60% 55% at 60% 38%, var(--st-mask) 40%, transparent 100%), linear-gradient(to bottom, var(--st-mask) 50%, transparent 92%); mask-composite:intersect; }
/* The artwork runs past the hero, so what follows it is lifted over it: it stays behind the text and the cards. */
html[data-look="stipple"] .st-hero ~ *, html[data-look="stipple"] .iv-ask .iv-side { position:relative; }
html[data-look="stipple"] .st-label { display:block; margin:0 0 14px; font-size:11.5px; font-weight:500; letter-spacing:.16em; text-transform:uppercase; color:var(--iv-accent); }
html[data-look="stipple"] .st-hero .iv-h1 { font-size:52px; line-height:1.05; letter-spacing:-.03em; font-weight:300; text-wrap:balance; }
html[data-look="stipple"] .st-hero .iv-intro { margin:16px 0 0; font-family:var(--iv-read); font-size:20px; line-height:1.45; }
/* ---- Ask: the controls ---- */
html[data-look="stipple"] .iv-ask .iv-kicker { font-size:11.5px; font-weight:500; letter-spacing:.16em; color:var(--iv-accent); }
html[data-look="stipple"] .iv-wrap button.iv-send { background:var(--iv-button); color:var(--iv-on-button); align-self:center; height:52px; min-height:52px; padding:0 28px; border-radius:26px; font-size:16px; font-weight:500; }
html[data-look="stipple"] .iv-wrap button.iv-primary { background:var(--iv-button); color:var(--iv-on-button); padding:0 18px; border-radius:999px; }
/* Off: the same pill washed out, the words still readable (about 4.9:1 on a white card). */
html[data-look="stipple"] .iv-wrap button.iv-send[disabled], html[data-look="stipple"] .iv-wrap button.iv-primary[disabled] { background:var(--iv-button); color:var(--iv-on-button); opacity:.65; cursor:default; }
html[data-look="stipple"] .iv-wrap button.iv-ghost, html[data-look="stipple"] .iv-wrap button.iv-starter { border:1px solid var(--iv-border); background:var(--iv-card); color:var(--iv-ink); border-radius:999px; min-height:44px; padding:0 16px; font-size:13px; }
html[data-look="stipple"] .iv-privacy { margin:0; background:var(--iv-tint); border-radius:16px; padding:12px 14px; font-family:var(--iv-read); font-size:14.5px; color:var(--iv-muted); }
html[data-look="stipple"] .iv-privacy svg { margin-top:3px; }
html[data-look="stipple"] .iv-privacy svg path { stroke:var(--iv-accent); }
/* Solid cream boxes would hide the dots: tinted instead. Small radii follow the cards. */
html[data-look="stipple"] .iv-composed { background:var(--iv-tint); border-radius:14px; }
html[data-look="stipple"] .iv-tip { background:var(--iv-tint); border-radius:12px; padding:10px 12px; font-family:var(--iv-read); font-size:13.5px; }
html[data-look="stipple"] .iv-field input { border-radius:12px; }
html[data-look="stipple"] .iv-mode-card { border-radius:14px; }
html[data-look="stipple"] .iv-step { border-radius:999px; padding:2px 9px; }
/* Reading text in the serif (one weight: regular). */
html[data-look="stipple"] .iv-ask .iv-lc-verdict { font-family:var(--iv-read); font-weight:400; }
html[data-look="stipple"] .iv-ask .iv-composed p, html[data-look="stipple"] .iv-ask .iv-today-line, html[data-look="stipple"] .iv-ask .iv-check-hint, html[data-look="stipple"] .iv-ask .iv-empty { font-family:var(--iv-read); }
html[data-look="stipple"] .iv-ask .iv-foot { font-family:var(--iv-read); font-size:13.5px; }
html[data-look="stipple"] .iv-ask .iv-advisory { font-family:var(--iv-read); font-size:12.5px; }
/* ---- Ask: the footer and the dot band at the foot of the page ---- */
html[data-look="stipple"] .st-foot { font-family:var(--iv-font); display:flex; flex-wrap:wrap; justify-content:space-between; gap:6px 24px; margin-top:40px; padding-top:18px; padding-bottom:calc(110px + env(safe-area-inset-bottom, 0px)); border-top:1px solid var(--iv-border); font-family:var(--iv-font); font-size:13.5px; color:var(--iv-muted); }
html[data-look="stipple"] body:has(.st-foot) { position:relative; min-height:100vh; }
html[data-look="stipple"] body:has(.st-foot)::after { content:""; position:absolute; left:0; right:0; bottom:0; height:120px; z-index:-1; pointer-events:none; background:url(/look/foot-day.webp) center bottom / 560px 120px repeat-x; }
/* ==== the other chrome pages: the Library, Explore, the run page, the Expert view and its board, the chat, Activity ==== */
/* ---- weight cap, part two: bare b and strong, headings, header cells and labels inside the chrome (never the answer content). At the lowest rank (:where), so every rule after it can still choose 300 or 400. ---- */
html[data-look="stipple"] :where(.iv-wrap b), html[data-look="stipple"] :where(.iv-wrap strong), html[data-look="stipple"] :where(.iv-wrap h1), html[data-look="stipple"] :where(.iv-wrap h2), html[data-look="stipple"] :where(.iv-wrap h3), html[data-look="stipple"] :where(.iv-wrap h4), html[data-look="stipple"] :where(.iv-wrap th), html[data-look="stipple"] :where(.iv-wrap label) { font-weight:500; }
html[data-look="stipple"] :where(.run-chrome b), html[data-look="stipple"] :where(.run-chrome strong), html[data-look="stipple"] :where(.run-chrome h1), html[data-look="stipple"] :where(.run-chrome h2), html[data-look="stipple"] :where(.run-chrome h3), html[data-look="stipple"] :where(.run-chrome h4), html[data-look="stipple"] :where(.run-chrome th), html[data-look="stipple"] :where(.run-chrome label) { font-weight:500; }
html[data-look="stipple"] :where(.rx-expert b), html[data-look="stipple"] :where(.rx-expert strong), html[data-look="stipple"] :where(.rx-expert h1), html[data-look="stipple"] :where(.rx-expert h2), html[data-look="stipple"] :where(.rx-expert h3), html[data-look="stipple"] :where(.rx-expert h4), html[data-look="stipple"] :where(.rx-expert th), html[data-look="stipple"] :where(.rx-expert label) { font-weight:500; }
html[data-look="stipple"] :where(.rt b), html[data-look="stipple"] :where(.rt strong), html[data-look="stipple"] :where(.rt h1), html[data-look="stipple"] :where(.rt h2), html[data-look="stipple"] :where(.rt h3), html[data-look="stipple"] :where(.rt h4), html[data-look="stipple"] :where(.rt th), html[data-look="stipple"] :where(.rt label) { font-weight:500; }
html[data-look="stipple"] :where(.rx-chat b), html[data-look="stipple"] :where(.rx-chat strong), html[data-look="stipple"] :where(.rx-chat h1), html[data-look="stipple"] :where(.rx-chat h2), html[data-look="stipple"] :where(.rx-chat h3), html[data-look="stipple"] :where(.rx-chat h4), html[data-look="stipple"] :where(.rx-chat th), html[data-look="stipple"] :where(.rx-chat label) { font-weight:500; }
html[data-look="stipple"] :where(.rx-refusals b), html[data-look="stipple"] :where(.rx-refusals strong), html[data-look="stipple"] :where(.rx-refusals h1), html[data-look="stipple"] :where(.rx-refusals h2), html[data-look="stipple"] :where(.rx-refusals h3), html[data-look="stipple"] :where(.rx-refusals h4), html[data-look="stipple"] :where(.rx-refusals th), html[data-look="stipple"] :where(.rx-refusals label) { font-weight:500; }
html[data-look="stipple"] :where(.rx-back b), html[data-look="stipple"] :where(.rx-back strong), html[data-look="stipple"] :where(.rx-back h1), html[data-look="stipple"] :where(.rx-back h2), html[data-look="stipple"] :where(.rx-back h3), html[data-look="stipple"] :where(.rx-back h4), html[data-look="stipple"] :where(.rx-back th), html[data-look="stipple"] :where(.rx-back label) { font-weight:500; }
html[data-look="stipple"] :where(.dr b), html[data-look="stipple"] :where(.dr strong), html[data-look="stipple"] :where(.dr h1), html[data-look="stipple"] :where(.dr h2), html[data-look="stipple"] :where(.dr h3), html[data-look="stipple"] :where(.dr h4), html[data-look="stipple"] :where(.dr th), html[data-look="stipple"] :where(.dr label) { font-weight:500; }
html[data-look="stipple"] :where(main > h1) { font-weight:500; }
/* ---- weight cap, part three: each heavy rule of the other chrome styles, restated ---- */
html[data-look="stipple"] .run-back a { font-weight:500; }
html[data-look="stipple"] .run-chrome .run-question { font-weight:500; }
html[data-look="stipple"] .activity-title { font-weight:500; }
html[data-look="stipple"] .run-chrome .badge { font-weight:500; }
html[data-look="stipple"] .run-chrome h3 { font-weight:500; }
html[data-look="stipple"] .run-fold > summary h3 { font-weight:500; }
html[data-look="stipple"] .iv-wrap .kg-switch button { font-weight:500; }
html[data-look="stipple"] .kg-find label { font-weight:500; }
html[data-look="stipple"] .kg-lab.b { font-weight:500; }
html[data-look="stipple"] .iv-wrap button.kg-openfull { font-weight:500; }
html[data-look="stipple"] .kg-full-head .kg-full-title { font-weight:500; }
html[data-look="stipple"] .kg-year { font-weight:500; }
html[data-look="stipple"] .iv-wrap button.kg-play { font-weight:500; }
html[data-look="stipple"] .kg-papers h3 { font-weight:500; }
html[data-look="stipple"] .kg-num { font-weight:500; }
html[data-look="stipple"] .iv-wrap button.kg-legend-btn { font-weight:500; }
html[data-look="stipple"] .kg-selects label { font-weight:500; }
html[data-look="stipple"] .kg-group h2 { font-weight:500; }
html[data-look="stipple"] .kg-kicker { font-weight:500; }
html[data-look="stipple"] .kg-chip { font-weight:500; }
html[data-look="stipple"] .kg-card-title { font-weight:500; }
html[data-look="stipple"] .kg-flag { font-weight:500; }
html[data-look="stipple"] .iv-wrap a.kg-act { font-weight:500; }
html[data-look="stipple"] .rx-seg .rx-tab { font-weight:500; }
html[data-look="stipple"] .run-chrome .rk-card h2 { font-weight:500; }
html[data-look="stipple"] .rk-clock { font-weight:500; }
html[data-look="stipple"] .rk-name { font-weight:500; }
html[data-look="stipple"] .rk-val { font-weight:500; }
html[data-look="stipple"] .rk-state { font-weight:500; }
html[data-look="stipple"] .rk-tag { font-weight:500; }
html[data-look="stipple"] .rk-fold-title { font-weight:500; }
html[data-look="stipple"] .rk-role { font-weight:500; }
html[data-look="stipple"] .rk-more { font-weight:500; }
html[data-look="stipple"] .in-close { font-weight:500; }
html[data-look="stipple"] .in-fold-title { font-weight:500; }
html[data-look="stipple"] .in-fold-read { font-weight:500; }
html[data-look="stipple"] .dr-rewrite { font-weight:500; }
html[data-look="stipple"] .dr-banner { font-weight:500; }
html[data-look="stipple"] .dr-card h2 { font-weight:500; }
html[data-look="stipple"] .dr-on { font-weight:500; }
html[data-look="stipple"] .dr-coming { font-weight:500; }
html[data-look="stipple"] .dr-fold > summary { font-weight:500; }
html[data-look="stipple"] .dr-clock { font-weight:500; }
html[data-look="stipple"] .rx-who { font-weight:500; }
html[data-look="stipple"] .rx-verdict { font-weight:500; }
html[data-look="stipple"] .rx .rx-h { font-weight:500; }
html[data-look="stipple"] .rx-three .k { font-weight:500; }
html[data-look="stipple"] .rx-three .today { font-weight:500; }
html[data-look="stipple"] .rx-scen .lab { font-weight:500; }
html[data-look="stipple"] .rx-scen .choice { font-weight:500; }
html[data-look="stipple"] .rx .rx-btn { font-weight:500; }
html[data-look="stipple"] .rx-expert a.rx-cite { font-weight:500; }
html[data-look="stipple"] .rt a.rt-cite { font-weight:500; }
html[data-look="stipple"] .rt h1 { font-weight:500; }
html[data-look="stipple"] .rt h2 { font-weight:500; }
html[data-look="stipple"] .rt h3 { font-weight:500; }
html[data-look="stipple"] .rt-eyebrow { font-weight:500; }
html[data-look="stipple"] .rt-chip { font-weight:500; }
html[data-look="stipple"] .rt-mfade button { font-weight:500; }
html[data-look="stipple"] .rt-lab { font-weight:500; }
html[data-look="stipple"] .rt-kv .v { font-weight:500; }
html[data-look="stipple"] .rt-seats { font-weight:500; }
html[data-look="stipple"] .rt-av { font-weight:500; }
html[data-look="stipple"] .rt-who { font-weight:500; }
html[data-look="stipple"] .rt-h { font-weight:500; }
html[data-look="stipple"] .rt-hy { font-weight:500; }
html[data-look="stipple"] details.rt-f > summary::after { font-weight:500; }
html[data-look="stipple"] .rt th { font-weight:500; }
html[data-look="stipple"] .rt-ledger td:first-child { font-weight:500; }
html[data-look="stipple"] .rt-choice { font-weight:500; }
html[data-look="stipple"] details.rt-checks > summary { font-weight:500; }
html[data-look="stipple"] .rt-tx.b { font-weight:500; }
html[data-look="stipple"] .rt-split-t { font-weight:500; }
html[data-look="stipple"] .rt-f-b td[data-l]::before { font-weight:500; }
html[data-look="stipple"] .rx .rx-source { font-weight:500; }
html[data-look="stipple"] .rx-writing { font-weight:500; }
html[data-look="stipple"] .rx-flags-h { font-weight:500; }
html[data-look="stipple"] .rx-line-surgeon { font-weight:500; }
html[data-look="stipple"] .rx-line-safety { font-weight:500; }
html[data-look="stipple"] .rx .rx-suggest { font-weight:500; }
html[data-look="stipple"] .rx .rx-modes label { font-weight:500; }
html[data-look="stipple"] .rx-sheet-src { font-weight:500; }
html[data-look="stipple"] .rx .rx-close { font-weight:500; }
html[data-look="stipple"] .rx-chaton .run-question { font-weight:500; }
html[data-look="stipple"] .rx-pre:has(.rx-chat:target:not(:empty)) .run-question { font-weight:500; }
/* ---- shared: page titles, kickers, plain page text, status boxes, focus ---- */
html[data-look="stipple"] .st-title, html[data-look="stipple"] .iv-lib .iv-h1, html[data-look="stipple"] .activity-title, html[data-look="stipple"] .rt h1 { margin:0; font-family:var(--iv-font); font-size:38px; line-height:1.1; font-weight:300; letter-spacing:-.02em; color:var(--iv-ink); text-wrap:balance; }
html[data-look="stipple"] .iv-lib .iv-kicker, html[data-look="stipple"] .kg .iv-kicker { font-size:11.5px; font-weight:500; letter-spacing:.16em; color:var(--iv-accent); }
/* A page with nothing but a line of text (not found, loading): the reading face. */
html[data-look="stipple"] main > p { font-family:var(--iv-read); font-size:17px; line-height:1.5; color:var(--iv-ink); }
html[data-look="stipple"] main > p.run-back { font-family:var(--iv-font); font-size:14px; margin:0 0 6px; }
/* The dot band reaches the very foot: no margin under the narrow pages. */
html[data-look="stipple"] main:not(.wide) { margin-bottom:0; }
html[data-look="stipple"] main > p.hint { font-size:14.5px; color:var(--iv-muted); }
/* Status boxes keep their alert, warning and done colours; the bar at the left gives way to a rounded note. */
html[data-look="stipple"] main > .box, html[data-look="stipple"] .iv-wrap .box, html[data-look="stipple"] .run-chrome > .box, html[data-look="stipple"] .rx-answer > .box, html[data-look="stipple"] .rx-refusals .box { border-left:0; border-radius:16px; padding:12px 14px; font-family:var(--iv-read); font-size:15.5px; line-height:1.45; }
html[data-look="stipple"] .iv-wrap a:focus-visible, html[data-look="stipple"] .iv-wrap button:focus-visible, html[data-look="stipple"] .iv-wrap select:focus-visible, html[data-look="stipple"] .run-chrome a:focus-visible, html[data-look="stipple"] .run-chrome button:focus-visible, html[data-look="stipple"] .run-chrome summary:focus-visible, html[data-look="stipple"] .run-back a:focus-visible { outline:3px solid var(--iv-accent); outline-offset:2px; }
html[data-look="stipple"] .rx-seg a:focus-visible, html[data-look="stipple"] .rx-seg button:focus-visible, html[data-look="stipple"] .rx-expert a:focus-visible, html[data-look="stipple"] .rx-expert button:focus-visible, html[data-look="stipple"] .rt a:focus-visible, html[data-look="stipple"] .rt button:focus-visible, html[data-look="stipple"] .rt summary:focus-visible { outline:3px solid var(--iv-accent); outline-offset:2px; }
html[data-look="stipple"] .rx-chat a:focus-visible, html[data-look="stipple"] .rx-chat button:focus-visible, html[data-look="stipple"] .rx-chat textarea:focus-visible, html[data-look="stipple"] .rx .rx-modes input:focus-visible + label, html[data-look="stipple"] details.iv-thread > summary:focus-visible { outline:3px solid var(--iv-accent); outline-offset:2px; }
/* ---- the Library ---- */
html[data-look="stipple"] .iv-lib .iv-intro { margin:10px 0 0; font-family:var(--iv-read); font-size:17px; line-height:1.45; color:var(--iv-ink); }
html[data-look="stipple"] .iv-wrap a.iv-new { background:var(--iv-button); color:var(--iv-on-button); min-height:52px; padding:0 26px; border-radius:26px; font-size:16px; }
html[data-look="stipple"] .iv-wrap a.iv-new:hover { background:var(--iv-button); color:var(--iv-on-button); }
html[data-look="stipple"] .iv-search { border-radius:14px; background:var(--iv-card); }
html[data-look="stipple"] .iv-search:focus-within { outline:3px solid var(--iv-accent); outline-offset:2px; }
/* The search icon's colour is written in the drawing: the look's muted instead. */
html[data-look="stipple"] .iv-search svg circle, html[data-look="stipple"] .iv-search svg path { stroke:var(--iv-muted); }
html[data-look="stipple"] .iv-filters { background:var(--iv-soft); border:1px solid var(--iv-border); border-radius:999px; padding:3px; }
html[data-look="stipple"] .iv-wrap button.iv-filter { color:var(--iv-ink); }
html[data-look="stipple"] .iv-wrap button.iv-filter[aria-pressed="true"] { background:var(--iv-tint); color:var(--iv-accent); box-shadow:none; }
html[data-look="stipple"] .iv-lib .iv-lib-verdict { font-family:var(--iv-read); font-weight:400; font-size:18.5px; line-height:1.4; }
html[data-look="stipple"] .iv-lib .iv-lib-verdict.iv-status { font-size:16px; }
html[data-look="stipple"] .iv-lib .iv-foot, html[data-look="stipple"] .iv-lib .iv-empty, html[data-look="stipple"] .iv-thread-note { font-family:var(--iv-read); font-size:14px; }
html[data-look="stipple"] .iv-lib .iv-empty { font-size:16px; }
html[data-look="stipple"] details.iv-thread { border-radius:16px; }
html[data-look="stipple"] .iv-wrap .iv-del button.iv-keep { border:1px solid var(--iv-border); padding:0 16px; }
html[data-look="stipple"] .iv-wrap .iv-del button.iv-del-yes { padding:0 16px; }
/* The plain outlined button (Delete all my history; Copy for notes is the answer's and is not touched). */
html[data-look="stipple"] .iv-wrap button.secondary { min-height:44px; padding:0 18px; border:1px solid var(--iv-accent); border-radius:999px; background:var(--iv-card); color:var(--iv-accent); font-family:var(--iv-font); font-size:14px; font-weight:500; }
html[data-look="stipple"] .iv-danger .box p { font-family:var(--iv-read); }
html[data-look="stipple"] .iv-danger .box button { min-height:44px; border-radius:999px; font-family:var(--iv-font); font-size:14px; font-weight:500; }
/* ---- Explore ---- */
html[data-look="stipple"] .kg > .st-label { margin:10px 0 -2px; }
html[data-look="stipple"] .kg > .st-title { margin-bottom:8px; }
html[data-look="stipple"] .iv-wrap .kg-switch button { min-height:44px; color:var(--iv-ink); }
html[data-look="stipple"] .iv-wrap .kg-switch button[aria-pressed="true"] { background:var(--iv-tint); color:var(--iv-accent); }
html[data-look="stipple"] .kg-find label, html[data-look="stipple"] .kg-selects label { color:var(--iv-ink); }
html[data-look="stipple"] .kg-find input:focus { outline:3px solid var(--iv-accent); outline-offset:2px; }
html[data-look="stipple"] .kg-map.kg-full { background:var(--iv-soft); }
html[data-look="stipple"] .kg-year { font-weight:300; }
html[data-look="stipple"] .kg-foot { gap:10px; }
html[data-look="stipple"] .kg-note, html[data-look="stipple"] .kg-nomatch, html[data-look="stipple"] .kg-drawing { font-family:var(--iv-read); }
html[data-look="stipple"] .kg-note { font-size:13px; }
html[data-look="stipple"] .kg-nomatch { background:var(--iv-card); border-radius:16px; font-size:15px; color:var(--iv-ink); }
html[data-look="stipple"] .kg-group h2 { font-size:11.5px; letter-spacing:.16em; text-transform:uppercase; color:var(--iv-accent); }
html[data-look="stipple"] .kg-row-sub { font-family:var(--iv-read); font-size:12.5px; }
html[data-look="stipple"] .kg-kicker { font-size:11px; color:var(--iv-muted); }
html[data-look="stipple"] .kg-chip { border-radius:999px; padding:2px 9px; }
html[data-look="stipple"] .kg-card-body { font-family:var(--iv-read); color:var(--iv-ink); }
html[data-look="stipple"] .kg-from { background:var(--iv-tint); border-radius:12px; padding:10px 12px; font-family:var(--iv-read); color:var(--iv-ink); }
html[data-look="stipple"] .iv-wrap a.kg-act { background:var(--iv-button); color:var(--iv-on-button); }
html[data-look="stipple"] .iv-wrap a.kg-act:hover { background:var(--iv-button); color:var(--iv-on-button); }
html[data-look="stipple"] .iv-wrap a.kg-act.alt, html[data-look="stipple"] .iv-wrap a.kg-act.alt:hover { background:var(--iv-card); color:var(--iv-accent); border:1px solid var(--iv-accent); }
html[data-look="stipple"] .iv-wrap button.kg-legend-btn { font-size:14px; color:var(--iv-ink); }
html[data-look="stipple"] .iv-wrap .kg-keys button { font-size:13px; }
html[data-look="stipple"] .kg-selects select { padding:0 10px; }
html[data-look="stipple"] .kg .iv-empty { font-family:var(--iv-read); font-size:16px; }
/* ---- the run page: back link, question, badges, folded rows ---- */
html[data-look="stipple"] .run-chrome .run-question { margin:6px 0 12px; font-size:24px; line-height:1.22; font-weight:300; letter-spacing:-.01em; text-wrap:balance; }
html[data-look="stipple"] .run-chrome > p { margin:0 0 12px; }
html[data-look="stipple"] .run-chrome .badge { border-radius:999px; padding:4px 12px; font-size:12.5px; }
html[data-look="stipple"] .run-chrome .run-fold { border-radius:var(--iv-r); padding:4px 18px; }
html[data-look="stipple"] .run-fold > summary h3 { font-size:15px; }
html[data-look="stipple"] .run-fold > summary .ap-more { font-weight:500; font-size:13px; }
/* ---- the run page: the pills and the results card ---- */
html[data-look="stipple"] .rx-seg .rx-tab { color:var(--iv-ink); }
html[data-look="stipple"] .rx-seg .rx-tab[data-state="idle"] { color:var(--iv-muted); }
html[data-look="stipple"] .rx-seg .rx-tab[data-state="filling"] { color:var(--tr-filling); }
html[data-look="stipple"] .rx-seg .rx-tab[data-state="ready"] { color:var(--tr-ready); }
html[data-look="stipple"] .rk-card { border-radius:var(--iv-r); }
html[data-look="stipple"] .run-chrome .rk-card h2 { font-size:19px; }
html[data-look="stipple"] .rk-now, html[data-look="stipple"] .rk-sub, html[data-look="stipple"] .rk-leave, html[data-look="stipple"] .in-tag { font-family:var(--iv-read); }
html[data-look="stipple"] .rk-sub, html[data-look="stipple"] .in-tag { font-size:13px; }
html[data-look="stipple"] .rk-now { font-size:15px; color:var(--iv-ink); }
html[data-look="stipple"] .rk-leave { font-size:14px; color:var(--iv-ink); }
/* The finished mark's green is written in the drawing: the tracker's ready green instead. */
html[data-look="stipple"] .rk-now svg path { stroke:var(--tr-ready); }
html[data-look="stipple"] .rk-clock, html[data-look="stipple"] .rk-val { font-weight:300; letter-spacing:-.01em; }
html[data-look="stipple"] .rk-of { font-weight:400; letter-spacing:0; }
html[data-look="stipple"] .rk-tag { border-radius:16px; padding:12px 14px; font-family:var(--iv-read); font-weight:400; font-size:15px; color:var(--iv-ink); }
html[data-look="stipple"] .rk-fold { border-radius:var(--iv-r); }
html[data-look="stipple"] .rk-fold .steps { font-family:var(--iv-read); }
/* ---- the run page: the introduction ---- */
html[data-look="stipple"] .in-text p { font-family:var(--iv-read); font-size:16px; line-height:1.5; }
html[data-look="stipple"] .in-fold { border-radius:var(--iv-r); }
/* ---- the run page: the draft being written ---- */
html[data-look="stipple"] .dr-rewrite, html[data-look="stipple"] .dr-banner { border-radius:16px; padding:12px 14px; }
html[data-look="stipple"] .dr-rewrite { color:var(--iv-accent-deep); }
html[data-look="stipple"] .dr-card, html[data-look="stipple"] .dr-coming, html[data-look="stipple"] .dr-fold { border-radius:var(--iv-r); }
html[data-look="stipple"] .dr-card p, html[data-look="stipple"] .dr-ul { font-family:var(--iv-read); font-size:15.5px; }
/* Lead-ins inside the serif text: the sans at 500 (the serif has one weight). */
html[data-look="stipple"] .dr-card strong, html[data-look="stipple"] .dr-card p.dr-on { font-family:var(--iv-font); font-size:14.5px; }
/* ---- the Expert view ---- */
html[data-look="stipple"] .rx-who, html[data-look="stipple"] .rx .rx-h { font-size:11.5px; letter-spacing:.16em; text-transform:uppercase; color:var(--iv-accent); }
html[data-look="stipple"] .rx-expert { color:var(--iv-ink); }
html[data-look="stipple"] .rx-op { padding:14px 16px; }
html[data-look="stipple"] .rx-verdict { font-family:var(--iv-read); font-weight:400; font-size:18px; line-height:1.42; }
html[data-look="stipple"] .rx-card { border-radius:var(--iv-r); padding:14px 16px; }
html[data-look="stipple"] .rx-card p, html[data-look="stipple"] .rx-three > div > :not(.k), html[data-look="stipple"] .rx-scen > div { font-family:var(--iv-read); font-size:15px; }
html[data-look="stipple"] .rx-three .k, html[data-look="stipple"] .rx-scen .lab { font-family:var(--iv-font); letter-spacing:.1em; }
html[data-look="stipple"] .rx-three > div > .today { font-family:var(--iv-font); font-size:14.5px; }
html[data-look="stipple"] .rx-scen > div { background:var(--iv-soft); border:1px solid var(--iv-rule); border-radius:12px; padding:10px 12px; }
html[data-look="stipple"] .rx-scen .choice { font-family:var(--iv-font); font-size:14.5px; }
/* Source links inside the serif text: its one weight. */
html[data-look="stipple"] .rx-expert a.rx-cite, html[data-look="stipple"] .rt a.rt-cite { font-weight:400; }
html[data-look="stipple"] .rx .rx-btn { align-self:flex-start; min-height:52px; padding:0 26px; gap:14px; border-radius:26px; border-color:var(--iv-button); background:var(--iv-button); color:var(--iv-on-button); font-size:16px; }
/* ---- the Temporal board ---- */
html[data-look="stipple"] .rt p, html[data-look="stipple"] .rt li, html[data-look="stipple"] .rt-blk, html[data-look="stipple"] .rt-learned, html[data-look="stipple"] .rt-bub, html[data-look="stipple"] .rt-pat, html[data-look="stipple"] .rt-empty { font-family:var(--iv-read); }
html[data-look="stipple"] .rt h2 { font-size:20px; letter-spacing:-.01em; }
html[data-look="stipple"] .rt-eyebrow, html[data-look="stipple"] .rt-step b { font-size:11.5px; letter-spacing:.16em; color:var(--iv-accent); }
html[data-look="stipple"] .rt-panel, html[data-look="stipple"] details.rt-f, html[data-look="stipple"] details.rt-checks, html[data-look="stipple"] .rt-era { border-radius:var(--iv-r); }
html[data-look="stipple"] .rt-chip.grey { background:var(--iv-card); }
html[data-look="stipple"] .rt-era { background:var(--rt-era-bg,var(--iv-soft)); }
html[data-look="stipple"] .rt-shows.capped .rt-mfade { background:linear-gradient(to bottom,transparent,var(--rt-era-bg,var(--iv-soft)) 60%); }
html[data-look="stipple"] .rt-lab, html[data-look="stipple"] .rt-kv { font-family:var(--iv-font); }
html[data-look="stipple"] .rt-lab { letter-spacing:.12em; }
html[data-look="stipple"] .rt-learned { border-radius:14px; padding:12px; }
html[data-look="stipple"] .rt-tabs { background:var(--iv-soft); }
html[data-look="stipple"] .rt-hy { border-radius:14px; }
html[data-look="stipple"] .rt-pat { border-radius:16px; border-color:var(--iv-tint); }
html[data-look="stipple"] .rt-verdict { font-family:var(--iv-read); font-weight:400; font-size:18px; line-height:1.42; }
html[data-look="stipple"] .rt-bub b, html[data-look="stipple"] .rt li b, html[data-look="stipple"] .rt-pat b { font-family:var(--iv-font); font-size:.95em; }
/* ---- the chat ---- */
html[data-look="stipple"] .rx-chat-note, html[data-look="stipple"] .rx-intro, html[data-look="stipple"] .rx-wait, html[data-look="stipple"] .rx-text, html[data-look="stipple"] .rx-q, html[data-look="stipple"] .rx-cut, html[data-look="stipple"] .rx-foot, html[data-look="stipple"] .rx-sheet-text { font-family:var(--iv-read); }
html[data-look="stipple"] .rx-chat-note, html[data-look="stipple"] .rx-intro { font-size:16px; color:var(--iv-ink); }
html[data-look="stipple"] .rx .rx-starter { border-color:var(--iv-border); border-radius:var(--iv-r); padding:12px 16px; }
html[data-look="stipple"] .rx-q { background:var(--iv-button); color:var(--iv-on-button); border-radius:18px 18px 6px 18px; font-size:16px; }
html[data-look="stipple"] .rx-reply { border-radius:var(--iv-r); padding:14px 16px; }
html[data-look="stipple"] .rx-text { font-size:16.5px; }
html[data-look="stipple"] .rx .rx-source { border-radius:999px; padding:3px 10px; font-family:var(--iv-font); }
html[data-look="stipple"] .rx-writing, html[data-look="stipple"] .rx-flags { border-radius:12px; }
html[data-look="stipple"] .rx-line-surgeon, html[data-look="stipple"] .rx-line-safety { border-radius:16px; padding:12px 14px; }
html[data-look="stipple"] .rx-line-surgeon { color:var(--iv-accent-deep); }
/* The composer stays on screen over the page: a card, not a cream bar (which would blank the dots). */
html[data-look="stipple"] .rx-composer { padding:10px 12px; background:var(--iv-card); border:1px solid var(--iv-border); border-radius:var(--iv-r); }
html[data-look="stipple"] .rx .rx-ask textarea { border-radius:14px; }
html[data-look="stipple"] .rx .rx-send { background:var(--iv-button); color:var(--iv-on-button); }
html[data-look="stipple"] .rx .rx-send[disabled] { background:var(--iv-button); opacity:.65; }
html[data-look="stipple"] .rx-sheet { border-radius:var(--iv-r) var(--iv-r) 0 0; }
html[data-look="stipple"] .rx-chaton .run-question, html[data-look="stipple"] .rx-pre:has(.rx-chat:target:not(:empty)) .run-question { font-size:22px; font-weight:300; }
/* ---- Activity ---- */
html[data-look="stipple"] .activity-title { margin:10px 0 14px; }
html[data-look="stipple"] .activity-title ~ p { font-family:var(--iv-read); }
html[data-look="stipple"] .activity-title ~ p.chain { display:inline-flex; align-items:center; min-height:32px; margin:0 0 14px; padding:0 14px; border-radius:999px; font-family:var(--iv-font); font-size:13px; font-weight:500; }
html[data-look="stipple"] .activity-title ~ .scroll { background:var(--iv-card); border:1px solid var(--iv-border); border-radius:var(--iv-r); padding:4px 14px; }
html[data-look="stipple"] .activity-title ~ .scroll table { background:transparent; font-family:var(--iv-font); font-size:13.5px; color:var(--iv-ink); }
html[data-look="stipple"] .activity-title ~ .scroll th, html[data-look="stipple"] .activity-title ~ .scroll td { padding:10px 8px; border-bottom:1px solid var(--iv-rule); }
html[data-look="stipple"] .activity-title ~ .scroll th { font-size:11.5px; font-weight:500; letter-spacing:.12em; text-transform:uppercase; color:var(--iv-muted); border-bottom-color:var(--iv-border); }
html[data-look="stipple"] .activity-title ~ .scroll tr:last-child td { border-bottom:0; }
@media (min-width:1100px) {
  html[data-look="stipple"] .st-hero-art { top:-70px; right:-24px; left:auto; transform:none; width:600px; height:677px; background-position:center; background-size:cover; }
  /* The page column as on the mock: 1312px wide, 64px gutters. */
  html[data-look="stipple"] .iv-head-in, html[data-look="stipple"] main.wide { max-width:1440px; padding-left:64px; padding-right:64px; }
  /* The artwork reaches up behind the header. */
  html[data-look="stipple"] .iv-head { position:relative; z-index:1; }
}
@media (max-width:1099px) {
  html[data-look="stipple"] .st-hero { min-height:470px; }
}
@media (max-width:899px) {
  html[data-look="stipple"] .st-hero .iv-h1 { font-size:35px; line-height:1.07; letter-spacing:-.025em; }
  html[data-look="stipple"] .st-hero .iv-intro { font-size:17px; }
}
@media (max-width:640px) {
  html[data-look="stipple"] .iv-ask .iv-lc-verdict { font-size:14px; }
  /* The band ends where the fixed tab bar begins (the body's bottom padding). */
  html[data-look="stipple"] body:has(.st-foot)::after { bottom:84px; }
}
@media (max-width:359px) {
  html[data-look="stipple"] .st-wordmark { font-size:16px; }
  html[data-look="stipple"] .iv-head-in { padding-left:12px; padding-right:12px; }
}
@media (max-width:899px) {
  html[data-look="stipple"] .st-title, html[data-look="stipple"] .iv-lib .iv-h1, html[data-look="stipple"] .activity-title, html[data-look="stipple"] .rt h1 { font-size:28px; }
  html[data-look="stipple"] .run-chrome .run-question { font-size:20px; }
}
@media (max-width:640px) {
  html[data-look="stipple"] .iv-lib .iv-intro { font-size:15.5px; }
  html[data-look="stipple"] .iv-wrap a.iv-new { min-height:48px; padding:0 20px; font-size:15px; }
  html[data-look="stipple"] .iv-filters { background:transparent; border:0; border-radius:0; padding:0; }
  html[data-look="stipple"] .iv-wrap button.iv-filter { background:var(--iv-card); border:1px solid var(--iv-border); }
  html[data-look="stipple"] .iv-wrap button.iv-filter[aria-pressed="true"] { background:var(--iv-tint); color:var(--iv-accent); border-color:var(--iv-accent); }
  html[data-look="stipple"] .iv-lib .iv-lib-verdict { font-size:16.5px; }
  html[data-look="stipple"] .iv-lib .iv-lib-verdict.iv-status { font-size:14.5px; }
  html[data-look="stipple"] .rx-chaton .run-question, html[data-look="stipple"] .rx-pre:has(.rx-chat:target:not(:empty)) .run-question { font-size:17px; font-weight:400; }
  html[data-look="stipple"] .rx .rx-btn { align-self:stretch; justify-content:center; }
  /* The log keeps readable columns and scrolls sideways inside its card. */
  html[data-look="stipple"] .activity-title ~ .scroll table { min-width:680px; }
}
"#;
