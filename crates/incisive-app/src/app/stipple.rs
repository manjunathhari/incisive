//! The stipple look (`LOOK=stipple`): its two faces, the day colours and the
//! dot layer, laid over the ivory styles.
//!
//! Sent only when the look is on (`shell`), after the ivory block, and every
//! rule starts with `html[data-look="stipple"]`: with `LOOK` unset or `ivory`
//! nothing here reaches the page. Only page chrome is restyled: the answer
//! content (`.ap-main` and the original run view) is left alone.

/// The Hanken Grotesk and Source Serif 4 faces (self-hosted, OFL;
/// `public/fonts/hanken-grotesk`, `public/fonts/source-serif-4`; the
/// `unicode-range` split is fontsource's), the day values of the `--iv-*`
/// variables, the fixed dot layer (`public/look`) and the weight cap.
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
"#;
