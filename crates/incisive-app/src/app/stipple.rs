//! The stipple look (`LOOK=stipple`): its two faces, the day colours and the
//! dot layer, laid over the ivory styles.
//!
//! Sent only when the look is on (`shell`), after the ivory block, and every
//! rule starts with `html[data-look="stipple"]`: with `LOOK` unset or `ivory`
//! nothing here reaches the page. Only page chrome is restyled: the answer
//! content (`.ap-main` and the original run view) is left alone.
//!
//! The `st-*` classes belong to markup drawn only in this look: the header
//! badge and wordmark (`ivory::SiteHeader`), and the Ask page's hero, label
//! and footer (`ask_page`). The certainty spine is not restyled.

/// The Hanken Grotesk and Source Serif 4 faces (self-hosted, OFL;
/// `public/fonts/hanken-grotesk`, `public/fonts/source-serif-4`; the
/// `unicode-range` split is fontsource's), the day values of the `--iv-*`
/// variables, the fixed dot layer (`public/look`), the weight cap, and the
/// header and Ask page in this look. One rule to a line, each under
/// `html[data-look="stipple"]` (the scope test reads it line by line).
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
  --iv-button:#13263c; --iv-on-button:#ffffff; --iv-fill:#1e5aa0; --iv-on-fill:#ffffff; }
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
html[data-look="stipple"] .st-hero-art { position:absolute; z-index:-1; pointer-events:none; top:0; left:50%; right:auto; width:min(100%, 390px); height:440px; transform:translateX(-50%); background-image:url(/look/sai-hero-day.webp); background-repeat:no-repeat; background-position:center top; background-size:100% auto; -webkit-mask-image:radial-gradient(ellipse 60% 55% at 60% 38%, #000 40%, transparent 100%), linear-gradient(to bottom, #000 50%, transparent 92%); -webkit-mask-composite:source-in; mask-image:radial-gradient(ellipse 60% 55% at 60% 38%, #000 40%, transparent 100%), linear-gradient(to bottom, #000 50%, transparent 92%); mask-composite:intersect; }
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
"#;
