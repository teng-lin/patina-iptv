//! Patina's runner on Samsung Tizen TVs: `iptv-app`'s root and UI set in
//! the module `nsh-platform-tizen` runs in a Web Worker (the generated
//! `nsh_rust_app` crate's lines, written out, as the app is linked from
//! this repository).
//!
//! `app_ui_sets` style: only the app's own set, without `nsh/base`'s TSX
//! registry, so the module links only the base components the app uses.

#[cfg(target_arch = "wasm32")]
fn ui_sets() -> Vec<nsh_core::element::Registry> {
    vec![iptv_app::registry()]
}

#[cfg(target_arch = "wasm32")]
fn root() -> nsh_core::element::Root {
    nsh_core::element::Root::of::<iptv_app::App>()
        .app_id("com.hantekllc.iptvrs")
        .appearance(nsh_core::element::Appearance::Dark)
        .status_bar(nsh_core::StatusBarSetting::default().hidden(true))
}

nsh_platform_tizen::app!(ui_sets(), root());
