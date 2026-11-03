#[cfg(feature = "ssr")]
pub mod agent;
/// Without `ssr` only the answer view types are built (the browser
/// deserializes and draws them); the path is the same `agent/answer.rs`.
#[cfg(not(feature = "ssr"))]
pub mod agent {
    pub mod answer;
    pub mod answer_v3;
}
/// Part A: the interactive answer page's rules (no Leptos).
pub mod answer_page;
pub mod app;
pub mod ask;
#[cfg(feature = "ssr")]
pub mod audit;
#[cfg(feature = "ssr")]
pub mod config;
#[cfg(feature = "ssr")]
pub mod db;
pub mod deid;
/// Temporal T4: the Expert Opinion screens' rules (no Leptos).
pub mod expert_page;
#[cfg(feature = "ssr")]
pub mod gate;
#[cfg(feature = "ssr")]
pub mod health;
#[cfg(feature = "ssr")]
pub mod identity;
/// The ivory redesign's card, Library and header rules (no Leptos).
pub mod library;
#[cfg(feature = "ssr")]
pub mod lit;
/// The memory layer (off by default).
#[cfg(feature = "ssr")]
pub mod memory;

/// The evidence chat (off by default).
#[cfg(feature = "ssr")]
pub mod chat;
/// The Ask page's advisory Question check and PICO helper (no Leptos).
pub mod question_check;
pub mod runs;
#[cfg(feature = "ssr")]
pub mod server;
#[cfg(feature = "ssr")]
pub mod spend;
/// The temporal evidence table (T1): eras, linked reports and effect
/// trajectories from a stored answer (pure code). Built for the browser too:
/// the Expert Opinion screens (T4) read its types.
pub mod temporal;

#[cfg(feature = "hydrate")]
#[wasm_bindgen::prelude::wasm_bindgen]
pub fn hydrate() {
    use crate::app::App;
    console_error_panic_hook::set_once();
    leptos::mount::hydrate_body(App);
}
