#![allow(clippy::expect_used, clippy::panic)]
use super::*;
use crate::{
    euclid::{Box2D, Point2D},
    sdf::{Feature, ShaderSymbolVertex, SymbolFeatureData, SymbolLayerData},
    vector::tessellation::OverAlignedVertexBuffer,
};

#[test]
fn a_map_without_placement_returns_no_hits() {
    let world = World::default();
    let style = Style::default();
    assert!(query_rendered_symbols(&world, &style, [0.0, 0.0], None).is_empty());
    assert!(query_rendered_symbols(&world, &style, [f64::NAN, 0.0], None).is_empty());
}

fn coords(x: i32, y: i32, z: u8) -> WorldTileCoords {
    WorldTileCoords {
        x,
        y,
        z: crate::coords::ZoomLevel::from(z),
    }
}

fn style() -> Style {
    serde_json::from_value(serde_json::json!({"version": 8, "sources": {},
        "layers": [
            {"id": "low", "type": "symbol", "source": "s", "source-layer": "places"},
            {"id": "high", "type": "symbol", "source": "s", "source-layer": "places"},
            {"id": "off", "type": "symbol", "source": "s", "source-layer": "places",
             "layout": {"visibility": "none"}}]}))
    .expect("style")
}

fn label(id: u64, sort_key: f32, at: [f32; 2]) -> Feature {
    Feature {
        parts: [None, None, None],
        data: SymbolFeatureData {
            id: Some(id),
            properties: Default::default(),
            sort_key,
        },
        bbox: Box2D::zero(),
        indices: 0..0,
        text_anchor: Point2D::new(at[0], at[1]),
        str: format!("label {id}"),
    }
}

fn bucket(at: WorldTileCoords, layer: &str, features: Vec<Feature>) -> SymbolLayerData {
    SymbolLayerData {
        atlas: None,
        coords: at,
        source_layer: "places".into(),
        style_layer_id: layer.into(),
        buffer: OverAlignedVertexBuffer::<ShaderSymbolVertex, u32>::from_iters(
            Vec::new(),
            Vec::new(),
            0,
        ),
        features,
    }
}

/// A world holding `buckets`, every label placed at the same screen rectangle.
fn placed(buckets: Vec<SymbolLayerData>) -> World {
    let mut world = World::default();
    let mut placed = PlacedSymbols::default();
    let mut by_tile: std::collections::BTreeMap<(i32, i32, u8), Vec<SymbolLayerData>> =
        Default::default();
    for bucket in buckets {
        for feature in 0..bucket.features.len() {
            placed.0.push(PlacedSymbol {
                coords: bucket.coords,
                layer: bucket.style_layer_id.clone(),
                feature,
                rectangles: [Some([100.0, 100.0, 200.0, 140.0]), None],
            });
        }
        let key = (bucket.coords.x, bucket.coords.y, u8::from(bucket.coords.z));
        by_tile.entry(key).or_default().push(bucket);
    }
    for (_, layers) in by_tile {
        let at = layers[0].coords;
        world
            .tiles
            .spawn_mut(at)
            .expect("tile")
            .insert(SymbolLayersDataComponent {
                pending_assets: false,
                layers,
            });
    }
    world.resources.insert(placed);
    world
}

fn ids(found: &[RenderedSymbol]) -> Vec<u64> {
    found.iter().filter_map(|symbol| symbol.id).collect()
}

const HERE: QueryGeometry = QueryGeometry::Point([150.0, 120.0]);

#[test]
fn results_run_from_the_topmost_style_layer_and_by_descending_sort_key_within_one() {
    let at = coords(0, 0, 0);
    let world = placed(vec![
        bucket(
            at,
            "low",
            vec![label(1, 1.0, [0.0, 0.0]), label(2, 5.0, [0.0, 0.0])],
        ),
        bucket(at, "high", vec![label(3, 0.0, [0.0, 0.0])]),
    ]);
    let found =
        query_rendered_symbols_in(&world, &style(), HERE, &QueryOptions::default()).expect("query");
    assert_eq!(
        ids(&found),
        [3, 2, 1],
        "high layer first, then the larger sort key"
    );
}

#[test]
fn hidden_layers_and_unloaded_tiles_are_not_queryable() {
    let world = placed(vec![bucket(
        coords(0, 0, 0),
        "off",
        vec![label(1, 0.0, [0.0, 0.0])],
    )]);
    let found =
        query_rendered_symbols_in(&world, &style(), HERE, &QueryOptions::default()).expect("query");
    assert!(found.is_empty(), "the layer is switched off");

    let mut world = placed(vec![bucket(
        coords(0, 0, 0),
        "low",
        vec![label(1, 0.0, [0.0, 0.0])],
    )]);
    world.tiles.remove(coords(0, 0, 0));
    let found =
        query_rendered_symbols_in(&world, &style(), HERE, &QueryOptions::default()).expect("query");
    assert!(
        found.is_empty(),
        "a placement whose tile was evicted is skipped"
    );
}

#[test]
fn coordinates_are_normalized_into_one_world() {
    // The tile store holds canonical tiles only, so a wrapped view reads the same records; the
    // reported longitude must therefore stay inside one world however far east the tile lies.
    let far_east = coords(3, 1, 2);
    let world = placed(vec![bucket(
        far_east,
        "low",
        vec![label(1, 0.0, [2048.0, 2048.0])],
    )]);
    let found = query_rendered_symbols_in(&world, &style(), HERE, &QueryOptions::default())
        .expect("query")
        .remove(0);
    assert!(
        (found.coordinates[0] - (-45.0)).abs() < 1e-9,
        "{:?}",
        found.coordinates
    );
    assert!((-180.0..180.0).contains(&found.coordinates[0]));
    assert!(found.coordinates[1].abs() < 90.0);
}

#[test]
fn a_box_overlapping_only_the_edge_of_a_label_still_finds_it() {
    let world = placed(vec![bucket(
        coords(0, 0, 0),
        "low",
        vec![label(1, 0.0, [0.0, 0.0])],
    )]);
    let edge = QueryGeometry::Box {
        min: [190.0, 130.0],
        max: [260.0, 200.0],
    };
    let miss = QueryGeometry::Box {
        min: [201.0, 141.0],
        max: [260.0, 200.0],
    };
    let query = |geometry| {
        query_rendered_symbols_in(&world, &style(), geometry, &QueryOptions::default())
            .expect("query")
            .len()
    };
    assert_eq!(query(edge), 1);
    assert_eq!(query(miss), 0);
}
