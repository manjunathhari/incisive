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
#[cfg(feature = "ssr")]
pub mod gate;
#[cfg(feature = "ssr")]
pub mod health;
#[cfg(feature = "ssr")]
pub mod identity;
#[cfg(feature = "ssr")]
pub mod lit;
pub mod runs;
#[cfg(feature = "ssr")]
pub mod server;
#[cfg(feature = "ssr")]
pub mod spend;

#[cfg(feature = "hydrate")]
#[wasm_bindgen::prelude::wasm_bindgen]
pub fn hydrate() {
    use crate::app::App;
    console_error_panic_hook::set_once();
    leptos::mount::hydrate_body(App);
}
