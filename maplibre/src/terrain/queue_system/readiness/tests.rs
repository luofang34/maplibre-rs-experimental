#![allow(clippy::expect_used)]

use super::*;
use crate::{coords::WorldTileCoords, terrain::drape_targets::ShapeSpec};

fn style() -> Style {
    serde_json::from_value(serde_json::json!({"version":8,"sources":{},"layers":[
        {"id":"imagery","type":"raster","source":"imagery"},
        {"id":"relief","type":"color-relief","source":"dem","maxzoom":15},
        {"id":"hidden","type":"hillshade","source":"dem","minzoom":15}
    ]}))
    .expect("style")
}

fn shape(source: WorldTileCoords, layer: &str) -> ShapeSpec {
    ShapeSpec {
        view_complete: false,
        source,
        vector_layers: Vec::new(),
        raster_layers: vec![(layer.into(), 0, layer != "imagery")],
    }
}

#[test]
fn each_visible_raster_layer_needs_its_own_complete_coverage() {
    let coords = WorldTileCoords::from((2423, 1389, 12_u8.into()));
    let children = coords.get_children();
    let world = World::default();
    let mut spec = TargetSpec {
        absent_sources: Vec::new(),
        coords,
        shapes: vec![shape(coords, "imagery")],
    };
    for child in &children[..3] {
        spec.shapes.push(shape(*child, "relief"));
        assert!(
            !ready(&spec, &style(), 12.15, &world, false),
            "imagery cannot fill relief gaps"
        );
    }
    spec.shapes.push(shape(children[3], "relief"));
    assert!(ready(&spec, &style(), 12.15, &world, false));
    assert!(
        !ready(&spec, &style(), 15.0, &world, false),
        "newly visible hillshade is missing"
    );
    spec.shapes.push(shape(coords, "hidden"));
    assert!(ready(&spec, &style(), 15.0, &world, false));
}

#[test]
fn a_finished_vector_shape_cannot_hide_missing_raster_uploads() {
    let coords = WorldTileCoords::from((2423, 1389, 12_u8.into()));
    let spec = TargetSpec {
        absent_sources: Vec::new(),
        coords,
        shapes: vec![ShapeSpec {
            view_complete: false,
            source: coords,
            vector_layers: Vec::new(),
            raster_layers: Vec::new(),
        }],
    };
    assert!(!ready(&spec, &style(), 12.15, &World::default(), false));
}
