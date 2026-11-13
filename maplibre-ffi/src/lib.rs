//! Cross-platform C ABI (`include/maplibre.h`) and shared Rust engine for embedding
//! `maplibre-rs` into C, C++, Swift/Objective-C, Kotlin/JNI, and desktop hosts.

#![deny(unused_imports)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

mod engine;
mod error;
mod lifecycle;
mod render_api;
mod style_api;
mod types;

pub use engine::MapEngine;
pub use lifecycle::{
    maplibre_handle_destroy, maplibre_handle_set_camera, maplibre_handle_set_feature_state_json,
    maplibre_handle_set_style_json, maplibre_last_error_message, maplibre_map_clone_handle,
    maplibre_map_create, maplibre_map_destroy, maplibre_map_get_camera, maplibre_map_resize,
    maplibre_map_set_camera, maplibre_string_free, maplibre_version, MaplibreCamera,
    MaplibreConfig, MaplibreHandle, MaplibreMap,
};
pub use render_api::{
    maplibre_map_needs_redraw, maplibre_map_query_rendered_features_json, maplibre_map_render_rgba8,
};
pub use style_api::{
    maplibre_map_add_image_rgba8, maplibre_map_remove_feature_state,
    maplibre_map_set_feature_state_json, maplibre_map_set_filter_json,
    maplibre_map_set_geojson_source_data, maplibre_map_set_layer_visibility,
    maplibre_map_set_layout_property_json, maplibre_map_set_paint_property_json,
    maplibre_map_set_style_json,
};
pub use types::{
    apply_whole_style, build_feature_state_target, parse_feature_state_args, parse_style_json,
    CameraState, FfiError, MapEngineConfig,
};

/// Retains every exported C ABI function pointer so downstream `staticlib` crates (such as
/// `apple`) do not dead-strip re-exported `maplibre_*` symbols during archive linking.
#[inline(never)]
pub fn retain_c_abi_symbols() -> usize {
    let symbols: [usize; 25] = [
        maplibre_version as *const () as usize,
        maplibre_last_error_message as *const () as usize,
        maplibre_string_free as *const () as usize,
        maplibre_map_create as *const () as usize,
        maplibre_map_destroy as *const () as usize,
        maplibre_map_resize as *const () as usize,
        maplibre_map_clone_handle as *const () as usize,
        maplibre_handle_destroy as *const () as usize,
        maplibre_handle_set_camera as *const () as usize,
        maplibre_handle_set_style_json as *const () as usize,
        maplibre_handle_set_feature_state_json as *const () as usize,
        maplibre_map_get_camera as *const () as usize,
        maplibre_map_set_camera as *const () as usize,
        maplibre_map_set_style_json as *const () as usize,
        maplibre_map_set_paint_property_json as *const () as usize,
        maplibre_map_set_layout_property_json as *const () as usize,
        maplibre_map_set_filter_json as *const () as usize,
        maplibre_map_set_layer_visibility as *const () as usize,
        maplibre_map_set_geojson_source_data as *const () as usize,
        maplibre_map_set_feature_state_json as *const () as usize,
        maplibre_map_remove_feature_state as *const () as usize,
        maplibre_map_add_image_rgba8 as *const () as usize,
        maplibre_map_needs_redraw as *const () as usize,
        maplibre_map_render_rgba8 as *const () as usize,
        maplibre_map_query_rendered_features_json as *const () as usize,
    ];
    std::hint::black_box(symbols.len())
}
