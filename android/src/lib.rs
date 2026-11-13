//! Embeddable Android SDK, JNI `MapLibreSdk` / `NativeMap` bindings, C ABI re-exports, and
//! `NativeActivity` demo entrypoints.

#![deny(unused_imports)]
#![allow(unexpected_cfgs, unsafe_code)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

use jni::{objects::JClass, EnvUnowned};
use log::Level;
pub use maplibre_ffi::*;

mod jni_api;
mod jni_helpers;
mod options;
mod sdk;
#[cfg(test)]
mod tests;

pub use jni_api::*;
pub use options::{run_android_map, AndroidMapOptions, AndroidSdkError, PlatformAndroidApp};
pub use sdk::NativeMap;

#[cfg(not(any(no_pendantic_os_check, target_os = "android")))]
compile_error!("android works only on android.");

/// Default `NativeActivity` entrypoint preserved for the Android Gradle demo application.
#[cfg(target_os = "android")]
#[unsafe(no_mangle)]
pub fn android_main(app: maplibre_winit::android_activity::AndroidApp) {
    android_logger::init_once(
        android_logger::Config::default().with_max_level(log::LevelFilter::Info),
    );
    log::log!(Level::Info, "maplibre starting");
    let options =
        AndroidMapOptions::default().with_backends(maplibre::render::settings::Backends::GL);
    if let Err(error) = run_android_map(app, options) {
        tracing::error!(%error, ?error, "map startup failed");
    }
}

/// Legacy JNI smoke entrypoint preserved for `org.maplibre_rs.MapLibreRs.android_main()`.
#[unsafe(no_mangle)]
pub extern "system" fn Java_org_maplibre_1rs_MapLibreRs_android_1main(
    _env: EnvUnowned,
    _class: JClass,
) {
    log::log!(Level::Warn, "maplibre WOORKING");
}
