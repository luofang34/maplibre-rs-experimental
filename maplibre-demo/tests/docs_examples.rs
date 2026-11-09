//! Verifies that every native Rust example in `docs/src/examples/code/*.rs` compiles and builds a
//! valid `Style`; the browser-side ones are checked by `web/tests/docs_examples.rs`.

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

#[test]
fn docs_rust_examples_build_valid_styles() {
    use maplibre::{projection::ProjectionType, style::Style};

    let styles = [
        (
            "display-a-map",
            display_a_map::build_style(Style::default()),
        ),
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

    for (name, style) in styles {
        let issues = style.validate();
        assert!(
            issues.is_ok(),
            "example `{name}` produced style validation issues: {issues:?}"
        );
        if name != "display-a-map" {
            assert_eq!(
                style.projection.as_ref().map(|p| p.projection_type.clone()),
                Some(ProjectionType::Globe),
                "example `{name}` should default to Globe projection"
            );
        }
    }
}
