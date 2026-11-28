#[cfg(feature = "renderer")]
use wasm_bindgen::prelude::*;

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen::prelude::wasm_bindgen(
    inline_js = "const perf = globalThis.performance; export function maplibre_perf_now_ms() { return perf ? perf.now() : Date.now(); }"
)]
extern "C" {
    fn maplibre_perf_now_ms() -> f64;
}

#[cfg(feature = "trace")]
fn enable_tracing() {
    use tracing_subscriber::{layer::SubscriberExt, Registry};

    let mut builder = tracing_wasm::WASMLayerConfigBuilder::new();
    builder.set_report_logs_in_timings(true);
    builder.set_console_config(tracing_wasm::ConsoleConfig::NoReporting);

    tracing::subscriber::set_global_default(
        Registry::default().with(tracing_wasm::WASMLayer::new(builder.build())),
    )
    .ok();
}

#[cfg_attr(feature = "renderer", wasm_bindgen(start))]
/// Installs logging and panic reporting for each wasm instance.
pub fn wasm_bindgen_start() {
    #[cfg(target_arch = "wasm32")]
    maplibre::schedule::system::timings::register_wasm_clock(maplibre_perf_now_ms);
    #[cfg(debug_assertions)]
    {
        console_log::init_with_level(log::Level::Info).ok();
        std::panic::set_hook(Box::new(console_error_panic_hook::hook));
    }
    // Pre-initialize the process-wide FontSystem only on the browser Window thread so workers
    // do not contend via Atomics.wait while booting in parallel.
    if web_sys::window().is_some() {
        drop(maplibre::font::global_font_system());
    }

    #[cfg(feature = "trace")]
    enable_tracing();
}
