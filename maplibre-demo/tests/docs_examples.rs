//! Verifies that all 22 Rust examples in `docs/src/examples/code/*.rs` and the User Guide
//! quickstart API calls compile natively and build valid `Style` documents; browser-only entry
//! points are additionally checked under `wasm32-unknown-unknown` in `web/tests/docs_examples.rs`.

#![allow(clippy::expect_used)]

#[allow(dead_code)]
mod display_a_map {
    include!("../../docs/src/examples/code/display-a-map.rs");
}

#[allow(dead_code)]
mod display_a_satellite_map {
    include!("../../docs/src/examples/code/display-a-satellite-map.rs");
}

#[allow(dead_code)]
mod display_a_globe_with_a_vector_map {
    include!("../../docs/src/examples/code/display-a-globe-with-a-vector-map.rs");
}

#[allow(dead_code)]
mod display_a_globe_with_an_atmosphere {
    include!("../../docs/src/examples/code/display-a-globe-with-an-atmosphere.rs");
}

#[allow(dead_code)]
mod add_3d_terrain {
    include!("../../docs/src/examples/code/add-3d-terrain.rs");
}

#[allow(dead_code)]
mod add_a_hillshade_layer {
    include!("../../docs/src/examples/code/add-a-hillshade-layer.rs");
}

#[allow(dead_code)]
mod add_a_color_relief_layer {
    include!("../../docs/src/examples/code/add-a-color-relief-layer.rs");
}

#[allow(dead_code)]
mod display_buildings_in_3d {
    include!("../../docs/src/examples/code/display-buildings-in-3d.rs");
}

#[allow(dead_code)]
mod add_a_geojson_line {
    include!("../../docs/src/examples/code/add-a-geojson-line.rs");
}

#[allow(dead_code)]
mod add_a_geojson_polygon {
    include!("../../docs/src/examples/code/add-a-geojson-polygon.rs");
}

#[allow(dead_code)]
mod add_a_raster_tile_source {
    include!("../../docs/src/examples/code/add-a-raster-tile-source.rs");
}

#[allow(dead_code)]
mod add_an_image_source {
    include!("../../docs/src/examples/code/add-an-image-source.rs");
}

#[allow(dead_code)]
mod style_lines_with_a_data_driven_property {
    include!("../../docs/src/examples/code/style-lines-with-a-data-driven-property.rs");
}

#[allow(dead_code)]
mod create_a_gradient_line {
    include!("../../docs/src/examples/code/create-a-gradient-line.rs");
}

#[allow(dead_code)]
mod create_and_style_clusters {
    include!("../../docs/src/examples/code/create-and-style-clusters.rs");
}

#[allow(dead_code)]
mod create_a_heatmap_layer {
    include!("../../docs/src/examples/code/create-a-heatmap-layer.rs");
}

#[allow(dead_code)]
mod fly_to_a_location {
    include!("../../docs/src/examples/code/fly-to-a-location.rs");
}

#[allow(dead_code)]
mod get_features_under_the_mouse_pointer {
    include!("../../docs/src/examples/code/get-features-under-the-mouse-pointer.rs");
}

#[allow(dead_code)]
mod change_a_layers_color {
    include!("../../docs/src/examples/code/change-a-layers-color.rs");
}

#[allow(dead_code)]
mod show_and_hide_layers {
    include!("../../docs/src/examples/code/show-and-hide-layers.rs");
}

#[allow(dead_code)]
mod filter_symbols_by_toggling_a_list {
    include!("../../docs/src/examples/code/filter-symbols-by-toggling-a-list.rs");
}

#[allow(dead_code)]
mod add_live_realtime_data {
    include!("../../docs/src/examples/code/add-live-realtime-data.rs");
}

#[test]
fn docs_rust_examples_build_valid_styles() {
    use maplibre::{projection::ProjectionType, style::Style};

    let standalone_globe_styles = [
        ("display-a-satellite-map", display_a_satellite_map::style()),
        (
            "display-a-globe-with-a-vector-map",
            display_a_globe_with_a_vector_map::build_style(Style::default()),
        ),
        (
            "display-a-globe-with-an-atmosphere",
            display_a_globe_with_an_atmosphere::style(),
        ),
        ("add-3d-terrain", add_3d_terrain::style()),
        ("add-a-hillshade-layer", add_a_hillshade_layer::style()),
        (
            "add-a-color-relief-layer",
            add_a_color_relief_layer::style(),
        ),
        (
            "display-buildings-in-3d",
            display_buildings_in_3d::build_style(Style::default()),
        ),
        (
            "add-a-geojson-line",
            add_a_geojson_line::build_style(Style::default()),
        ),
        (
            "add-a-geojson-polygon",
            add_a_geojson_polygon::build_style(Style::default()),
        ),
        (
            "add-a-raster-tile-source",
            add_a_raster_tile_source::style(),
        ),
        (
            "add-an-image-source",
            add_an_image_source::build_style(Style::default()),
        ),
        (
            "style-lines-with-a-data-driven-property",
            style_lines_with_a_data_driven_property::build_style(Style::default()),
        ),
        (
            "create-a-gradient-line",
            create_a_gradient_line::build_style(Style::default()),
        ),
        (
            "create-and-style-clusters",
            create_and_style_clusters::build_style(Style::default()),
        ),
        (
            "create-a-heatmap-layer",
            create_a_heatmap_layer::build_style(Style::default()),
        ),
    ];

    let base_overlay_styles = [
        (
            "display-a-map",
            display_a_map::build_style(Style::default()),
        ),
        (
            "fly-to-a-location",
            fly_to_a_location::build_style(Style::default()),
        ),
        (
            "get-features-under-the-mouse-pointer",
            get_features_under_the_mouse_pointer::build_style(Style::default()),
        ),
        (
            "change-a-layers-color",
            change_a_layers_color::build_style(Style::default()).expect("valid style JSON"),
        ),
        (
            "show-and-hide-layers",
            show_and_hide_layers::build_style(Style::default()),
        ),
        (
            "filter-symbols-by-toggling-a-list",
            filter_symbols_by_toggling_a_list::build_style(Style::default())
                .expect("valid style JSON"),
        ),
        (
            "add-live-realtime-data",
            add_live_realtime_data::build_style(Style::default()).expect("valid style JSON"),
        ),
    ];

    assert_eq!(
        standalone_globe_styles.len() + base_overlay_styles.len(),
        22,
        "all 22 book examples must be compiled and validated natively"
    );

    for (name, style) in standalone_globe_styles {
        let issues = style.validate();
        assert!(
            issues.is_ok(),
            "example `{name}` produced style validation issues: {issues:?}"
        );
        assert_eq!(
            style.projection.as_ref().map(|p| p.projection_type.clone()),
            Some(ProjectionType::Globe),
            "example `{name}` should default to Globe projection"
        );
    }

    for (name, style) in base_overlay_styles {
        let issues = style.validate();
        assert!(
            issues.is_ok(),
            "example `{name}` produced style validation issues: {issues:?}"
        );
    }
}

/// Compile-time and option-construction guard for the User Guide quickstart (`docs/src/user-guide/index.md`).
#[test]
fn user_guide_quickstart_api_compiles() {
    use std::path::PathBuf;

    use maplibre::{render::settings::WgpuSettings, style::Style};
    use maplibre_winit::{
        run_headed_map, run_headed_map_with, HeadedMapError, HeadedMapHost, HeadedMapOptions,
        WinitMapWindowConfig,
    };

    fn _quickstart_windowed() -> Result<(), HeadedMapError> {
        let style = Style::default();
        let options = HeadedMapOptions {
            max_pitch_degrees: 75.0,
            show_navigation_controls: true,
            ..HeadedMapOptions::default()
        };
        run_headed_map(
            Some(PathBuf::from("./maplibre-cache")),
            WinitMapWindowConfig::new("MapLibre Quickstart".to_string()),
            WgpuSettings::default(),
            style,
            options,
        )
    }

    fn _quickstart_with_host() -> Result<(), HeadedMapError> {
        let host = HeadedMapHost::default().with_configure(|map| {
            let _ = map.is_initialized();
        });
        run_headed_map_with(
            None::<PathBuf>,
            WinitMapWindowConfig::new("MapLibre Host".to_string()),
            WgpuSettings::default(),
            Style::default(),
            HeadedMapOptions::default(),
            host,
        )
    }

    let options = HeadedMapOptions::default();
    assert!(options.show_navigation_controls);
    assert!(!options.debug_tiles);
    let _ = (
        _quickstart_windowed as fn() -> _,
        _quickstart_with_host as fn() -> _,
    );
}
