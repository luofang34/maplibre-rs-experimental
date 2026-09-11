#![allow(clippy::expect_used, clippy::panic)]

use std::collections::HashSet;

use super::{covering_shapes_for, HasTile, SourceShapes, TileViewPattern, ViewTileSources};
use crate::{
    coords::{ViewRegion, WorldTileCoords, Zoom, ZoomLevel},
    io::tile_sources::TileKind,
    render::resource::{BackingBufferDescriptor, Queue},
    tcs::world::World,
};

#[derive(Default)]
struct Loaded(HashSet<WorldTileCoords>);

struct TestBuffer;
struct TestQueue;

impl Queue<TestBuffer> for TestQueue {
    fn write_buffer(&self, _buffer: &TestBuffer, _offset: wgpu::BufferAddress, _data: &[u8]) {}
}

/// Providers that treat a fixed set of tiles as loaded for one kind.
struct LoadedRaster;
struct LoadedVector;

thread_local! {
    static LOADED_RASTER: std::cell::RefCell<HashSet<WorldTileCoords>> = Default::default();
    static LOADED_VECTOR: std::cell::RefCell<HashSet<WorldTileCoords>> = Default::default();
}

impl Default for LoadedRaster {
    fn default() -> Self {
        Self
    }
}

impl Default for LoadedVector {
    fn default() -> Self {
        Self
    }
}

impl HasTile for LoadedRaster {
    fn has_tile(&self, coords: WorldTileCoords, _world: &World) -> bool {
        LOADED_RASTER.with(|loaded| loaded.borrow().contains(&coords))
    }
}

impl HasTile for LoadedVector {
    fn has_tile(&self, coords: WorldTileCoords, _world: &World) -> bool {
        LOADED_VECTOR.with(|loaded| loaded.borrow().contains(&coords))
    }
}

fn shape_coords(shapes: &SourceShapes) -> Vec<WorldTileCoords> {
    let mut coords = Vec::new();
    shapes.for_each(&mut |shape| coords.push(shape.coords()));
    coords
}

impl HasTile for Loaded {
    fn has_tile(&self, coords: WorldTileCoords, _world: &World) -> bool {
        self.0.contains(&coords)
    }
}

fn tile(x: i32, y: i32, z: u8) -> WorldTileCoords {
    WorldTileCoords {
        x,
        y,
        z: ZoomLevel::new(z),
    }
}

#[test]
fn complete_children_cover_the_tile_across_depths() {
    let world = World::default();
    let parent = tile(1, 1, 2);
    let [a, b, c, d] = parent.get_children();
    let mut loaded: HashSet<WorldTileCoords> = [a, b, c].into();
    loaded.extend(d.get_children());
    let container = Loaded(loaded);

    let children = container
        .get_complete_children(parent, &world, 2)
        .expect("children at two depths cover the parent");

    assert_eq!(children.len(), 7);
    assert!(container.get_complete_children(parent, &world, 1).is_none());
}

#[test]
fn incomplete_children_do_not_stand_in_for_the_tile() {
    let world = World::default();
    let parent = tile(1, 1, 2);
    let [a, b, _, _] = parent.get_children();
    let container = Loaded([a, b].into());

    assert!(container.get_complete_children(parent, &world, 4).is_none());
    assert_eq!(
        container.get_available_children(parent, &world, 4),
        Some(vec![a, b])
    );
}

#[test]
fn a_view_tile_drapes_the_covering_tiles_that_overlap_it() {
    let target = tile(2, 3, 5);
    let covering = [
        tile(4, 6, 6),
        tile(5, 7, 6),
        tile(6, 6, 6),
        tile(16, 24, 8),
        tile(3, 3, 5),
        tile(0, 0, 3),
    ];

    assert_eq!(
        covering_shapes_for(target, &covering),
        vec![tile(4, 6, 6), tile(5, 7, 6), tile(16, 24, 8), tile(0, 0, 3)],
        "children, a grandchild and the ancestor overlap; the neighbours do not"
    );
    assert_eq!(covering_shapes_for(target, &[target]), vec![target]);
}

#[test]
fn raster_shapes_follow_the_source_covering_while_vector_shapes_use_the_view_tile() {
    let view = tile(2, 3, 5);
    let [a, b, c, d] = view.get_children();
    // The finer grandchildren are loaded too, but the covering asks for the children.
    LOADED_RASTER.with(|loaded| {
        let mut loaded = loaded.borrow_mut();
        loaded.extend([a, b, c, d]);
        loaded.extend(a.get_children());
    });
    LOADED_VECTOR.with(|loaded| loaded.borrow_mut().insert(view));
    let mut sources = ViewTileSources::default();
    sources
        .add::<LoadedVector>(TileKind::Vector)
        .add::<LoadedRaster>(TileKind::Raster);
    let pattern: TileViewPattern<TestQueue, TestBuffer> =
        TileViewPattern::new(BackingBufferDescriptor::new(TestBuffer, 0));
    let coverings = [("photo".to_string(), vec![a, b, c, d, tile(9, 9, 6)])];

    let tiles = pattern.generate_pattern(
        &ViewRegion::from_tiles(vec![view], ZoomLevel::new(5), 8),
        &sources,
        &coverings,
        Zoom::new(5.0),
        &World::default(),
    );

    let [view_tile] = tiles.as_slice() else {
        panic!("one view tile, got {}", tiles.len());
    };
    assert_eq!(shape_coords(&view_tile.vector), vec![view]);
    assert_eq!(shape_coords(&view_tile.raster), vec![a, b, c, d]);
}

#[test]
fn raster_shapes_fall_back_to_the_pyramid_until_the_covering_loads() {
    let view = tile(1, 1, 4);
    LOADED_RASTER.with(|loaded| loaded.borrow_mut().insert(tile(0, 0, 3)));
    let mut sources = ViewTileSources::default();
    sources.add::<LoadedRaster>(TileKind::Raster);
    let pattern: TileViewPattern<TestQueue, TestBuffer> =
        TileViewPattern::new(BackingBufferDescriptor::new(TestBuffer, 0));
    let coverings = [("photo".to_string(), view.get_children().to_vec())];

    let tiles = pattern.generate_pattern(
        &ViewRegion::from_tiles(vec![view], ZoomLevel::new(4), 8),
        &sources,
        &coverings,
        Zoom::new(4.0),
        &World::default(),
    );

    assert_eq!(shape_coords(&tiles[0].raster), vec![tile(0, 0, 3)]);
}

#[test]
fn bridge_width_units_follow_style_scale_independently_of_gaze_zoom() {
    use super::{TileShape, ViewTile};
    use crate::render::{camera::ViewProjection, shaders::ShaderTileMetadata};
    use cgmath::{Matrix4, SquareMatrix};
    struct Captured(std::cell::RefCell<Vec<u8>>);
    impl Queue<TestBuffer> for Captured {
        fn write_buffer(&self, _: &TestBuffer, _: u64, bytes: &[u8]) {
            self.0.replace(bytes.to_vec());
        }
    }
    let queue = Captured(Default::default());
    let coords = tile(8709, 5744, 14);
    let style_zoom = Zoom::new(14.0);
    for gaze_zoom in [10.0, 12.0, 14.0, 16.0] {
        let mut pattern = TileViewPattern::new(BackingBufferDescriptor {
            buffer: TestBuffer,
            inner_size: 104 * 8,
        });
        pattern.update_pattern(vec![ViewTile {
            target: coords,
            vector: SourceShapes::SourceEqTarget(TileShape::new(coords, Zoom::new(gaze_zoom))),
            raster: SourceShapes::None,
        }]);
        pattern.upload_pattern(
            &queue,
            &ViewProjection(Matrix4::identity()),
            1920.0,
            1080.0,
            style_zoom,
        );
        let bytes = queue.0.borrow();
        let metadata = bytemuck::pod_read_unaligned::<ShaderTileMetadata>(&bytes[..104]);
        assert_eq!(metadata.line_units_per_pixel, 8.0, "gaze zoom {gaze_zoom}");
    }
}
