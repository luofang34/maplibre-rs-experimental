#![allow(clippy::expect_used, clippy::panic)]

use super::{geometry, pool, style_layer, TestQueue, TestVertex};
use crate::{
    coords::WorldTileCoords,
    vector::{resource::AllocationError, tessellation::OverAlignedVertexBuffer},
};

fn shared_line_layer(id: &str) -> crate::style::layer::StyleLayer {
    serde_json::from_value(serde_json::json!({
        "id": id, "type": "line", "source": "s", "source-layer": "road"
    }))
    .expect("valid line layer")
}

#[test]
fn primary_upload_with_colliding_shared_key_allocates_fresh_vertices() {
    let mut pool = pool().with_decoupled_feature_metadata();
    let coords = WorldTileCoords::from((2, 3, 14_u8.into()));

    let primary = geometry(6).with_shared_geometry_key(Some(7));
    pool.replace_layer_geometry_layer_uniform_shared_unless(
        &TestQueue,
        coords,
        shared_line_layer("road-a"),
        &primary,
        None,
        0u32,
        0u32,
        6,
        |_| false,
    )
    .expect("primary upload");

    let secondary = OverAlignedVertexBuffer::from_iters(Vec::new(), [0_u32, 1, 2], 3)
        .with_shared_geometry_key(Some(7));
    pool.replace_layer_geometry_layer_uniform_shared_unless(
        &TestQueue,
        coords,
        shared_line_layer("road-b"),
        &secondary,
        None,
        0u32,
        0u32,
        6,
        |_| false,
    )
    .expect("secondary aliases 6-vertex slot");

    let orig_range = pool
        .index()
        .get_layers(coords)
        .expect("layers")
        .iter()
        .find(|e| e.style_layer.id == "road-a")
        .expect("road-a")
        .vertices_buffer_range()
        .clone();

    // New primary arrives with its own 12 vertices and the same colliding raw key 7.
    let colliding_primary = geometry(12).with_shared_geometry_key(Some(7));
    pool.replace_layer_geometry_layer_uniform_shared_unless(
        &TestQueue,
        coords,
        shared_line_layer("road-a"),
        &colliding_primary,
        None,
        0u32,
        0u32,
        12,
        |_| false,
    )
    .expect("colliding primary allocates fresh vertices instead of aliasing");

    let layers = pool.index().get_layers(coords).expect("updated layers");
    let new_primary = layers
        .iter()
        .find(|e| e.style_layer.id == "road-a")
        .expect("new road-a");
    let sibling = layers
        .iter()
        .find(|e| e.style_layer.id == "road-b")
        .expect("road-b");
    assert_eq!(
        new_primary.vertices_buffer_range().end - new_primary.vertices_buffer_range().start,
        12 * 24,
        "new primary must allocate its own 12-vertex range"
    );
    assert_ne!(
        new_primary.vertices_buffer_range(),
        orig_range,
        "new primary must not alias old 6-vertex slice"
    );
    assert_eq!(new_primary.shared_geometry_key(), None);
    assert_eq!(
        sibling.vertices_buffer_range(),
        orig_range,
        "existing sibling stays intact on old 6-vertex range"
    );
}

#[test]
fn empty_vertices_with_usable_indices_and_no_resident_slot_is_rejected() {
    let mut pool = pool().with_decoupled_feature_metadata();
    let coords = WorldTileCoords::from((4, 5, 14_u8.into()));
    let orphan = OverAlignedVertexBuffer::from_iters(Vec::new(), [0_u32, 1, 2], 3)
        .with_shared_geometry_key(Some(99));

    let res = pool.replace_layer_geometry_layer_uniform_shared_unless(
        &TestQueue,
        coords,
        shared_line_layer("road-b"),
        &orphan,
        None,
        0u32,
        0u32,
        0,
        |_| false,
    );
    assert!(matches!(
        res,
        Err(AllocationError::MissingVertices {
            coords: c,
            usable_indices: 3,
        }) if c == coords
    ));
    assert!(
        pool.index().get_layers(coords).is_none(),
        "rejected 0-vertex upload must not insert an IndexEntry"
    );
}

#[test]
fn mismatched_fallback_vertices_refuse_shared_slot_aliasing() {
    let mut pool = pool().with_decoupled_feature_metadata();
    let coords = WorldTileCoords::from((6, 7, 14_u8.into()));

    let primary = geometry(6).with_shared_geometry_key(Some(11));
    pool.replace_layer_geometry_layer_uniform_shared_unless(
        &TestQueue,
        coords,
        shared_line_layer("road-a"),
        &primary,
        None,
        0u32,
        0u32,
        6,
        |_| false,
    )
    .expect("primary upload");

    let secondary = OverAlignedVertexBuffer::from_iters(Vec::new(), [0_u32, 1, 2], 3)
        .with_shared_geometry_key(Some(11));
    let mismatch_vertices = vec![TestVertex::default(); 12];
    pool.replace_layer_geometry_layer_uniform_shared_unless(
        &TestQueue,
        coords,
        shared_line_layer("road-b"),
        &secondary,
        Some(&mismatch_vertices),
        0u32,
        0u32,
        12,
        |_| false,
    )
    .expect("mismatched fallback uploads fresh slice");

    let entry = pool
        .index()
        .get_layers(coords)
        .expect("layers")
        .iter()
        .find(|e| e.style_layer.id == "road-b")
        .expect("road-b");
    assert_eq!(
        entry.vertices_buffer_range().end - entry.vertices_buffer_range().start,
        12 * 24
    );
    assert_eq!(entry.shared_geometry_key(), None);
    let _ = style_layer("unused");
}
