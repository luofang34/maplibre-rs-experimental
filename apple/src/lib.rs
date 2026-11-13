#![deny(unused_imports)]
#![allow(unexpected_cfgs, unsafe_code)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

use maplibre::{render::settings::WgpuSettings, style::Style};
use maplibre_winit::{run_headed_map, HeadedMapOptions, WinitMapWindowConfig};

#[cfg(not(any(no_pendantic_os_check, target_os = "macos", target_os = "ios")))]
compile_error!("apple works only on macOS and iOS.");

#[unsafe(no_mangle)]
pub fn maplibre_apple_main() {
    env_logger::init_from_env(env_logger::Env::default().default_filter_or("info"));

    if let Err(error) = run_headed_map::<String>(
        None,
        WinitMapWindowConfig::new("maplibre".to_string()),
        WgpuSettings {
            backends: Some(maplibre::render::settings::Backends::all()),
            ..WgpuSettings::default()
        },
        Style::default(),
        HeadedMapOptions::default(),
    ) {
        tracing::error!(%error, ?error, "map startup failed");
    }
}
