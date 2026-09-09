#![allow(clippy::expect_used, clippy::panic)]
#[test]
fn a_pool_larger_than_the_device_allows_keeps_whole_elements() {
    assert_eq!(
        super::fitting_buffer_size(48, 10_000_000, u64::MAX),
        480_000_000
    );
    assert_eq!(
        super::fitting_buffer_size(48, 10_000_000, 268_435_456),
        268_435_440
    );
    assert_eq!(
        super::fitting_buffer_size(48, 10_000_000, 268_435_440) % 48,
        0
    );
    assert_eq!(super::fitting_buffer_size(4, 10, 100), 40);
}

use std::collections::HashSet;

use lyon::tessellation::VertexBuffers;

use crate::{
    coords::WorldTileCoords,
    render::resource::{BackingBufferDescriptor, Queue},
    style::layer::StyleLayer,
    vector::{
        resource::{BackingBufferType, BufferPool},
        tessellation::OverAlignedVertexBuffer,
    },
};

#[derive(Debug)]
struct TestBuffer {
    size: wgpu::BufferAddress,
}
struct TestQueue;

impl Queue<TestBuffer> for TestQueue {
    fn write_buffer(&self, buffer: &TestBuffer, offset: wgpu::BufferAddress, data: &[u8]) {
        if offset + data.len() as wgpu::BufferAddress > buffer.size {
            panic!("write out of bounds");
        }
    }
}

#[repr(C)]
#[derive(Default, Copy, Clone, bytemuck_derive::Pod, bytemuck_derive::Zeroable)]
struct TestVertex {
    data: [u8; 24],
}

fn pool() -> BufferPool<TestQueue, TestBuffer, TestVertex, u32, u32, u32> {
    BufferPool::new(
        BackingBufferDescriptor::new(TestBuffer { size: 1024 }, 1024),
        BackingBufferDescriptor::new(TestBuffer { size: 1024 }, 1024),
        BackingBufferDescriptor::new(TestBuffer { size: 1024 }, 1024),
        BackingBufferDescriptor::new(TestBuffer { size: 1024 }, 1024),
    )
}

fn style_layer(id: &str) -> StyleLayer {
    serde_json::from_value(serde_json::json!({
        "id": id, "type": "symbol", "source": "s", "source-layer": "place"
    }))
    .expect("valid style layer")
}

fn geometry(vertex_count: usize) -> OverAlignedVertexBuffer<TestVertex, u32> {
    let mut buffer = VertexBuffers::new();
    buffer.vertices = vec![TestVertex::default(); vertex_count];
    buffer.indices = (0..vertex_count as u32).collect();
    OverAlignedVertexBuffer::from(buffer)
}

#[test]
fn a_metadata_update_of_the_wrong_size_is_refused_instead_of_written() {
    let mut pool = pool();
    let coords = WorldTileCoords::default();
    pool.allocate_layer_geometry(
        &TestQueue,
        coords,
        style_layer("place_city"),
        &geometry(2),
        0u32,
        &[0u32; 2],
    )
    .expect("allocation fits");
    pool.allocate_layer_geometry(
        &TestQueue,
        coords,
        style_layer("place_town"),
        &geometry(4),
        0u32,
        &[0u32; 4],
    )
    .expect("allocation fits");
    let entries = pool
        .index()
        .get_layers(coords)
        .expect("both layers are allocated");
    let town = entries
        .iter()
        .find(|entry| entry.style_layer.id == "place_town")
        .expect("town entry");

    pool.update_feature_metadata(&TestQueue, town, &[1u32; 2]);
    pool.update_feature_metadata(&TestQueue, town, &[1u32; 4]);

    assert_eq!(
        pool.get_loaded_style_layers_at(coords),
        Some(HashSet::from(["place_city", "place_town"]))
    );
}

#[test]
fn ring_wrap_preserves_live_allocations() {
    let mut pool = BufferPool::new(
        BackingBufferDescriptor::new(TestBuffer { size: 128 }, 128),
        BackingBufferDescriptor::new(TestBuffer { size: 128 }, 128),
        BackingBufferDescriptor::new(TestBuffer { size: 128 }, 128),
        BackingBufferDescriptor::new(TestBuffer { size: 128 }, 128),
    );
    for (vertices, remaining) in [(2, 80), (2, 32), (1, 8), (1, 24), (1, 0), (1, 24), (1, 0)] {
        pool.allocate_layer_geometry(
            &TestQueue,
            WorldTileCoords::default(),
            style_layer("road"),
            &geometry(vertices),
            0u32,
            &[] as &[u32],
        )
        .expect("fits");
        assert_eq!(pool.available_space(BackingBufferType::Vertices), remaining);
    }
}

#[test]
fn removing_tile_releases_every_layer_before_reloading_same_coordinates() {
    let mut pool = pool();
    let coords = WorldTileCoords::default();
    for layer in ["city", "town"] {
        pool.allocate_layer_geometry(
            &TestQueue,
            coords,
            style_layer(layer),
            &geometry(2),
            0,
            &[0; 2],
        )
        .expect("fits");
    }
    let revision = pool.revision();
    pool.remove_tile(coords);
    assert!(pool.get_loaded_style_layers_at(coords).is_none());
    assert!(pool.index().front().is_none());
    assert!(pool.index().back().is_none());
    assert_ne!(pool.revision(), revision);
    pool.allocate_layer_geometry(
        &TestQueue,
        coords,
        style_layer("city"),
        &geometry(4),
        0,
        &[0; 4],
    )
    .expect("reload");
    let entry = pool.index().front().expect("new geometry");
    assert_eq!(
        entry.vertices_buffer_range().end - entry.vertices_buffer_range().start,
        96
    );
}

#[test]
fn oversized_geometry_returns_context_without_evicting_resident_data() {
    let mut pool = pool();
    let coords = WorldTileCoords::default();
    pool.allocate_layer_geometry(
        &TestQueue,
        coords,
        style_layer("city"),
        &geometry(2),
        0,
        &[0; 2],
    )
    .expect("fits");
    let revision = pool.revision();
    let error = pool
        .allocate_layer_geometry(
            &TestQueue,
            coords,
            style_layer("large"),
            &geometry(100),
            0,
            &[0; 100],
        )
        .expect_err("bounded");
    assert!(error.to_string().contains("2400"));
    assert_eq!(pool.revision(), revision);
    assert_eq!(
        pool.get_loaded_style_layers_at(coords),
        Some(HashSet::from(["city"]))
    );
}

#[test]
fn replacing_geometry_changes_its_identity_even_at_the_same_buffer_address() {
    let mut pool = pool();
    let coords = WorldTileCoords::default();
    pool.allocate_layer_geometry(
        &TestQueue,
        coords,
        style_layer("city"),
        &geometry(2),
        0,
        &[0; 2],
    )
    .expect("first upload");
    let first = pool.index().front().expect("entry").clone();
    pool.remove_tile(coords);
    pool.allocate_layer_geometry(
        &TestQueue,
        coords,
        style_layer("city"),
        &geometry(2),
        0,
        &[0; 2],
    )
    .expect("replacement");
    let second = pool.index().front().expect("entry");
    assert_eq!(
        first.vertices_buffer_range(),
        second.vertices_buffer_range()
    );
    assert_ne!(first.allocation_id(), second.allocation_id());
}
