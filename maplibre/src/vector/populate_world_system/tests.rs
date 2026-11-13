#![allow(clippy::expect_used, clippy::panic)]
use super::*;
use crate::{
    coords::WorldTileCoords,
    io::tile_backpressure::{request_budget, MAX_TILES_IN_FLIGHT},
    sdf::SymbolLayersDataComponent,
    tcs::world::World,
};
#[test]
fn visible_base_geometry_keeps_its_loading_slot_until_its_request_lands() {
    use crate::io::tile_retry::{self, RequestDisposition, RequestKind, TileRequestOutcome};
    let mut world = World::default();
    let coords = WorldTileCoords::default();
    world
        .tiles
        .spawn_mut(coords)
        .expect("tile")
        .insert(VectorLayerBucketComponent::default())
        .insert(SymbolLayersDataComponent::default());
    let attempt = tile_retry::next_attempt(&mut world);
    tile_retry::started(&mut world, coords, RequestKind::Vector, attempt, None);
    finish_tile(&mut world, &DefaultTileTessellated::build_partial(coords));
    assert!(
        world
            .tiles
            .query::<&VectorLayerBucketComponent>(coords)
            .expect("base")
            .done
    );
    assert_eq!(request_budget(&world), MAX_TILES_IN_FLIGHT - 1);
    // The worker completes the labels before it reports the request's outcome.
    finish_tile(&mut world, &DefaultTileTessellated::build_from(coords));
    assert_eq!(request_budget(&world), MAX_TILES_IN_FLIGHT - 1);
    tile_retry::completed(
        &mut world,
        TileRequestOutcome {
            coords,
            kind: RequestKind::Vector,
            attempt: Some(attempt),
            disposition: RequestDisposition::Complete,
        },
    );
    assert_eq!(request_budget(&world), MAX_TILES_IN_FLIGHT);
}

fn bucket(coords: WorldTileCoords, id: &str) -> crate::vector::AvailableVectorLayerBucket {
    crate::vector::AvailableVectorLayerBucket {
        coords,
        source_layer: id.to_owned(),
        style_layer_id: id.to_owned(),
        buffer: crate::vector::tessellation::OverAlignedVertexBuffer::empty(),
        feature_indices: Vec::new(),
        feature_colors: Vec::new(),
        feature_sort_keys: Vec::new(),
    }
}

/// A loaded tile showing `layers`, as an earlier request left it.
fn loaded(layers: &[&str]) -> (World, WorldTileCoords) {
    let mut world = World::default();
    let coords = WorldTileCoords::default();
    world
        .tiles
        .spawn_mut(coords)
        .expect("tile")
        .insert(VectorLayerBucketComponent {
            done: true,
            layers: layers
                .iter()
                .map(|id| crate::vector::VectorLayerBucket::AvailableLayer(bucket(coords, id)))
                .collect(),
            ..Default::default()
        })
        .insert(SymbolLayersDataComponent::default());
    (world, coords)
}

fn shown(world: &World, coords: WorldTileCoords) -> Vec<String> {
    crate::vector::content::vector_layers(&world.tiles, coords)
        .into_iter()
        .map(|layer| layer.style_layer_id.clone())
        .collect()
}

#[test]
fn a_finished_request_drops_the_layers_it_was_asked_for_but_left_out() {
    // The route was hidden, or its data emptied, so the new request brings roads alone.
    let (mut world, coords) = loaded(&["roads", "route"]);
    crate::vector::content::begin(&mut world, coords, vec!["roads".into(), "route".into()]);
    crate::vector::content::accept_vector(&mut world, bucket(coords, "roads"));
    finish_tile(&mut world, &DefaultTileTessellated::build_partial(coords));
    assert_eq!(shown(&world, coords), ["roads"]);
}

#[test]
fn a_request_limited_to_some_sources_keeps_the_layers_of_the_others() {
    let (mut world, coords) = loaded(&["roads", "route"]);
    crate::vector::content::begin(&mut world, coords, vec!["route".into()]);
    finish_tile(&mut world, &DefaultTileTessellated::build_from(coords));
    assert_eq!(shown(&world, coords), ["roads"]);
}

#[test]
fn a_request_that_could_not_read_its_data_keeps_what_the_tile_shows() {
    let (mut world, coords) = loaded(&["roads", "route"]);
    crate::vector::content::begin(&mut world, coords, vec!["roads".into(), "route".into()]);
    finish_tile(
        &mut world,
        &DefaultTileTessellated::build_failed(coords, false),
    );
    assert_eq!(shown(&world, coords), ["roads", "route"]);
}

#[test]
fn id_bearing_mvt_arrivals_without_feature_state_avoid_cloning_and_deduplicate_wire_maps() {
    use std::sync::Arc;

    use geozero::mvt::tile;
    use serde_json::json;

    use crate::{
        io::{
            apc::IntoMessage,
            geometry_index::{FeatureMeta, IndexedGeometry, TileIndex},
        },
        render::ShaderVertex,
        style::{
            expression::{FeatureProperties, Value as ExprValue, FEATURE_STATE_PREFIX},
            feature_state::FeatureStateTarget,
            Style,
        },
        vector::{
            feature_paint::{
                wire::{decode_feature_paint, encode_feature_paint},
                BucketEntryKind, BucketFeatureEntry, TileFeaturePaintComponent,
            },
            tessellation::OverAlignedVertexBuffer,
        },
    };

    let coords = WorldTileCoords::default();
    let mut style: Style = serde_json::from_value(json!({
        "version": 8,
        "sources": {"osm": {"type": "vector"}},
        "layers": [
            {
                "id": "park-fill",
                "type": "fill",
                "source": "osm",
                "source-layer": "landuse",
                "paint": {"fill-color": ["match", ["get", "class"], "park", "#00ff00", "#888888"]}
            },
            {
                "id": "park-outline",
                "type": "line",
                "source": "osm",
                "source-layer": "landuse",
                "paint": {"line-color": ["case", ["boolean", ["feature-state", "hover"], false], "#ffffff", "#004400"]}
            }
        ]
    }))
    .expect("valid style");

    // 1. Defect 2 check: arriving id-bearing feature with no feature-state preserves exact worker Arc.
    let (mut world, _) = loaded(&[]);
    let mut props = FeatureProperties::new();
    props.insert("class".into(), ExprValue::String("park".into()));
    let worker_arc = Arc::new(props);
    let worker_ptr = Arc::as_ptr(&worker_arc) as usize;
    let empty_mvt = tile::Layer {
        version: 2,
        name: "landuse".into(),
        ..Default::default()
    };
    let tri = OverAlignedVertexBuffer::from_iters([ShaderVertex::default(); 3], [0_u32, 1, 2], 3);
    let msg = DefaultLayerTessellated::build_from_with_paint(
        coords,
        tri.clone(),
        vec![3],
        vec![[0.0, 1.0, 0.0, 1.0]],
        vec![],
        empty_mvt.clone(),
        "park-fill".into(),
        vec![BucketFeatureEntry {
            raw_id: Some(42),
            properties: worker_arc,
            kind: BucketEntryKind::Feature,
        }],
    );
    apply_vector_worker_batch::<DefaultVectorTransferables>(
        &mut world,
        &style,
        vec![IntoMessage::into(msg)],
    )
    .expect("batch succeeds");
    let resident_ptr = {
        let store = world
            .tiles
            .query::<&TileFeaturePaintComponent>(coords)
            .expect("paint store");
        Arc::as_ptr(&store.committed["park-fill"][0].properties) as usize
    };
    assert_eq!(
        resident_ptr, worker_ptr,
        "id-bearing MVT features must not be cloned on the main thread when the source has no feature-state"
    );

    // 2. Defect 1 & 3 check: wire-decoded messages with strong_count == 1 in one batch with active
    // feature-state keep original Arcs alive in the memo (preventing ABA reuse) and share one
    // canonical Arc<FeatureProperties> across style layers and the query index.
    let mut state_map = serde_json::Map::new();
    state_map.insert("hover".into(), json!(true));
    style.set_feature_state(
        FeatureStateTarget::vector("osm", "landuse"),
        1_u64,
        state_map,
    );

    let (mut wire_world, _) = loaded(&[]);
    let mut props_a = FeatureProperties::new();
    props_a.insert("class".into(), ExprValue::String("park".into()));
    props_a.insert("label".into(), ExprValue::String("alpha".into()));
    let mut props_b = FeatureProperties::new();
    props_b.insert("class".into(), ExprValue::String("wood".into()));
    props_b.insert("label".into(), ExprValue::String("beta".into()));

    let raw_entries = vec![
        BucketFeatureEntry {
            raw_id: Some(1),
            properties: Arc::new(props_a.clone()),
            kind: BucketEntryKind::Feature,
        },
        BucketFeatureEntry {
            raw_id: Some(2),
            properties: Arc::new(props_b.clone()),
            kind: BucketEntryKind::Feature,
        },
    ];
    let wire_bytes = encode_feature_paint(&raw_entries);
    // Decode two independent wire payloads (just as Web FlatBuffer messages do) so every entry
    // enters the batch with its own fresh `strong_count == 1` Arc.
    let decoded_fill = decode_feature_paint(&wire_bytes).expect("fill wire decode");
    let decoded_line = decode_feature_paint(&wire_bytes).expect("line wire decode");
    assert!(!Arc::ptr_eq(
        &decoded_fill[0].properties,
        &decoded_line[0].properties
    ));

    let msg_fill = DefaultLayerTessellated::build_from_with_paint(
        coords,
        tri.clone(),
        vec![3, 6],
        vec![[0.0; 4]; 2],
        vec![],
        empty_mvt.clone(),
        "park-fill".into(),
        decoded_fill,
    );
    let msg_line = DefaultLayerTessellated::build_from_with_paint(
        coords,
        tri,
        vec![3, 6],
        vec![[0.0; 4]; 2],
        vec![],
        empty_mvt,
        "park-outline".into(),
        decoded_line,
    );
    let indexed = DefaultLayerIndexed::build_from(
        coords,
        Some("osm".into()),
        TileIndex::Linear {
            list: vec![
                IndexedGeometry::from_point(
                    (10.0, 10.0).into(),
                    FeatureMeta {
                        id: Some(1),
                        source_layer: Arc::from("landuse"),
                        properties: Arc::new(props_a),
                        feature_index: 0,
                    },
                )
                .expect("point 1"),
                IndexedGeometry::from_point(
                    (20.0, 20.0).into(),
                    FeatureMeta {
                        id: Some(2),
                        source_layer: Arc::from("landuse"),
                        properties: Arc::new(props_b),
                        feature_index: 1,
                    },
                )
                .expect("point 2"),
            ],
        },
    );

    apply_vector_worker_batch::<DefaultVectorTransferables>(
        &mut wire_world,
        &style,
        vec![
            IntoMessage::into(msg_fill),
            IntoMessage::into(msg_line),
            IntoMessage::into(indexed),
        ],
    )
    .expect("wire batch succeeds");

    let store = wire_world
        .tiles
        .query::<&TileFeaturePaintComponent>(coords)
        .expect("paint store");
    let fill_entries = &store.committed["park-fill"];
    let line_entries = &store.committed["park-outline"];
    assert_eq!(
        fill_entries[0].properties.get("label"),
        Some(&ExprValue::String("alpha".into()))
    );
    assert_eq!(
        fill_entries[0]
            .properties
            .get(&format!("{FEATURE_STATE_PREFIX}hover")),
        Some(&ExprValue::Bool(true))
    );
    assert_eq!(
        line_entries[1].properties.get("label"),
        Some(&ExprValue::String("beta".into()))
    );
    assert!(!line_entries[1]
        .properties
        .contains_key(&format!("{FEATURE_STATE_PREFIX}hover")));
    assert!(
        Arc::ptr_eq(&fill_entries[0].properties, &line_entries[0].properties),
        "decoded style layers must share one Arc<FeatureProperties> for feature 1"
    );
    assert!(
        Arc::ptr_eq(&fill_entries[1].properties, &line_entries[1].properties),
        "decoded style layers must share one Arc<FeatureProperties> for feature 2"
    );
    let mut index_arcs = Vec::new();
    wire_world
        .tiles
        .geometry_index
        .for_each_tile_property(coords, |arc| index_arcs.push(Arc::clone(arc)));
    assert_eq!(index_arcs.len(), 2);
    assert!(
        Arc::ptr_eq(&fill_entries[0].properties, &index_arcs[0])
            && Arc::ptr_eq(&fill_entries[1].properties, &index_arcs[1]),
        "query index must share the exact canonical Arc<FeatureProperties> with feature paint"
    );
}
