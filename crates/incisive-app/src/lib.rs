pub mod app;
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
