use wasm_bindgen::prelude::*;

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

#[wasm_bindgen(start)]
/// Installs logging and panic reporting for each wasm instance.
pub fn wasm_bindgen_start() {
    console_log::init_with_level(log::Level::Info).ok();
    std::panic::set_hook(Box::new(console_error_panic_hook::hook));

    #[cfg(feature = "trace")]
    enable_tracing();
}
