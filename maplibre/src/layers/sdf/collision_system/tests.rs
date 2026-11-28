#![allow(clippy::expect_used, clippy::panic)]

use super::canonical_tile;
use crate::coords::{WorldTileCoords, ZoomLevel};

#[test]
fn canonical_symbol_tile_wraps_antimeridian_copies() {
    let zoom = ZoomLevel::new(2);

    assert_eq!(
        canonical_tile(WorldTileCoords {
            x: -1,
            y: 1,
            z: zoom
        })
        .map(|tile| tile.x),
        Some(3)
    );
    assert_eq!(
        canonical_tile(WorldTileCoords {
            x: 4,
            y: 1,
            z: zoom
        })
        .map(|tile| tile.x),
        Some(0)
    );
}

#[test]
fn canonical_symbol_tile_rejects_polar_overflow() {
    let zoom = ZoomLevel::new(2);

    assert!(canonical_tile(WorldTileCoords {
        x: 0,
        y: -1,
        z: zoom
    })
    .is_none());
    assert!(canonical_tile(WorldTileCoords {
        x: 0,
        y: 4,
        z: zoom
    })
    .is_none());
}

#[test]
fn the_hosts_obstacles_are_in_the_grid_before_any_label() {
    let mut obstacles = crate::sdf::obstacles::LabelObstacles::default();
    obstacles.replace(vec![[100.0, 100.0, 140.0, 110.0]]);
    let grid = super::obstructed_grid(800.0, 600.0, Some(&obstacles));
    assert!(
        grid.overlaps([130.0, 105.0, 200.0, 120.0]),
        "a label over the obstacle"
    );
    assert!(
        grid.overlaps_non_cooperative([130.0, 105.0, 200.0, 120.0]),
        "even one that lets cooperative labels overlap it"
    );
    assert!(
        !grid.overlaps([150.0, 105.0, 200.0, 120.0]),
        "a label beside it"
    );
    assert!(!super::obstructed_grid(800.0, 600.0, None).overlaps([130.0, 105.0, 200.0, 120.0]));
}

#[test]
fn can_finish_initial_boot_blocks_on_pending_retry_upload_or_provided_image() {
    use crate::{
        io::tile_retry::{self, RequestDisposition, RequestKind, TileRequestOutcome},
        sdf::{
            provided::{self, ProvidedImagesReport, ProvidedImagesState},
            upload_system::SdfUploadPending,
        },
        tcs::world::World,
    };

    let mut world = World::default();
    assert!(super::active::can_finish_initial_boot(&world, 0));
    assert!(!super::active::can_finish_initial_boot(&world, 1));

    world.resources.insert(SdfUploadPending(true));
    assert!(!super::active::can_finish_initial_boot(&world, 0));
    world.resources.insert(SdfUploadPending(false));
    assert!(super::active::can_finish_initial_boot(&world, 0));

    let coords = WorldTileCoords::from((0, 0, ZoomLevel::new(4)));
    let attempt = tile_retry::next_attempt(&mut world);
    tile_retry::started(&mut world, coords, RequestKind::Vector, attempt, None);
    provided::apply(
        &mut world,
        ProvidedImagesReport {
            coords,
            attempt: Some(attempt),
            names: vec!["pin".to_owned()],
            pixel_ratio: 1.0,
            state: ProvidedImagesState::Awaiting,
        },
    );
    assert!(!super::active::can_finish_initial_boot(&world, 0));

    provided::apply(
        &mut world,
        ProvidedImagesReport {
            coords,
            attempt: Some(attempt),
            names: vec!["pin".to_owned()],
            pixel_ratio: 1.0,
            state: ProvidedImagesState::Settled,
        },
    );
    tile_retry::completed(
        &mut world,
        TileRequestOutcome {
            coords,
            kind: RequestKind::Vector,
            attempt: Some(attempt),
            disposition: RequestDisposition::Retry,
        },
    );
    assert!(!super::active::can_finish_initial_boot(&world, 0));

    let retry_attempt = tile_retry::next_attempt(&mut world);
    tile_retry::started(&mut world, coords, RequestKind::Vector, retry_attempt, None);
    tile_retry::completed(
        &mut world,
        TileRequestOutcome {
            coords,
            kind: RequestKind::Vector,
            attempt: Some(retry_attempt),
            disposition: RequestDisposition::Complete,
        },
    );
    assert!(super::active::can_finish_initial_boot(&world, 0));
}

#[cfg(all(feature = "headless", feature = "thread-safe-futures"))]
#[tokio::test]
async fn sliced_epoch_commits_all_pending_uploads_atomically_in_one_frame() {
    use std::time::Duration;

    use bytemuck::Zeroable;
    use cgmath::{Deg, Rad};
    use rustc_hash::FxHashMap;

    use crate::{
        coords::{LatLon, WorldCoords, Zoom},
        headless::create_headless_renderer,
        render::{
            eventually::Eventually,
            resource::OverAlignedVertexBuffer,
            settings::BufferPoolSizes,
            shaders::{SDFShaderFeatureMetadata, ShaderSymbolVertex},
            view_state::ViewState,
        },
        sdf::SymbolBufferPool,
        style::{layer::StyleLayer, Style, StyleLayerId},
        tcs::world::World,
        window::PhysicalSize,
    };

    let (_, renderer) = create_headless_renderer(64, 64, None)
        .await
        .expect("renderer");
    let sizes = BufferPoolSizes {
        vertices: 16,
        indices: 32,
        feature_metadata: 16,
        layer_metadata: 4,
    };
    let mut pool = SymbolBufferPool::from_device_with_sizes(&renderer.device, sizes);
    let layer_id_str = "places";
    let layer_id = StyleLayerId::intern(layer_id_str);
    let coords_a = WorldTileCoords::from((0, 0, ZoomLevel::new(4)));
    let coords_b = WorldTileCoords::from((1, 0, ZoomLevel::new(4)));
    let style_layer = StyleLayer {
        id: layer_id_str.to_owned(),
        type_: "symbol".to_owned(),
        ..StyleLayer::default()
    };
    let geometry = OverAlignedVertexBuffer::from_iters(
        [ShaderSymbolVertex::zeroed(); 4],
        [0_u32, 1, 2, 2, 1, 3],
        6,
    );
    for coords in [coords_a, coords_b] {
        pool.allocate_layer_geometry(
            &renderer.queue,
            coords,
            style_layer.clone(),
            &geometry,
            bytemuck::Zeroable::zeroed(),
            &[SDFShaderFeatureMetadata::zeroed(); 4],
        )
        .expect("symbol bucket fits");
    }
    let mut world = World::default();
    world.resources.insert(Eventually::Initialized(pool));
    let style: Style = serde_json::from_str(
        r#"{"version":8,"sources":{},"layers":[{"id":"follower","type":"symbol"}]}"#,
    )
    .expect("style");
    let size = PhysicalSize::new(64, 64).expect("size");
    let zoom = Zoom::new(12.0);
    let view = ViewState::new(
        size,
        WorldCoords::from_lat_lon(LatLon::default(), zoom),
        zoom,
        Deg(0.0),
        Rad(0.75),
    );
    let meta_a = vec![SDFShaderFeatureMetadata::from_placement(
        crate::render::shaders::SymbolPlacementMetadata {
            target_opacity: 1.0,
            elevation: 0.0,
            start_opacity: 1.0,
            start_time_ms: 0.0,
            shift: [0.0; 2],
            color: [1.0; 4],
            halo: [0.0; 4],
            params: [0.0; 4],
        },
    )];
    let meta_b = vec![SDFShaderFeatureMetadata::from_placement(
        crate::render::shaders::SymbolPlacementMetadata {
            target_opacity: 1.0,
            elevation: 0.0,
            start_opacity: 1.0,
            start_time_ms: 0.0,
            shift: [0.0; 2],
            color: [1.0; 4],
            halo: [0.0; 4],
            params: [0.0; 4],
        },
    )];
    let mut epoch = super::sliced::ActiveEpoch::for_atomic_upload_test(
        view,
        vec![(layer_id, coords_a, meta_a), (layer_id, coords_b, meta_b)],
    );
    let mut history = super::temporal::PlacementHistory::default();
    let mut uploaded = FxHashMap::default();
    let mut scratch = super::upload_diff::MetadataScratchPool::default();

    // First step finishes mirrored work with Duration::ZERO: must defer BEFORE uploading either bucket.
    assert!(
        !epoch.step_with_budget(
            super::sliced::EpochStepContext {
                world: &world,
                style: &style,
                queue: &renderer.queue,
                history: &mut history,
                uploaded: &mut uploaded,
                scratch: &mut scratch,
                sliced: true,
            },
            Duration::ZERO,
        ),
        "must yield before committing pending_uploads once budget is spent"
    );
    assert!(
        uploaded.is_empty(),
        "zero buckets may commit prematurely on yield"
    );

    // Next step enters pending_uploads and must drain BOTH buckets atomically even with Duration::ZERO.
    assert!(epoch.step_with_budget(
        super::sliced::EpochStepContext {
            world: &world,
            style: &style,
            queue: &renderer.queue,
            history: &mut history,
            uploaded: &mut uploaded,
            scratch: &mut scratch,
            sliced: true,
        },
        Duration::ZERO,
    ));
    assert!(uploaded.contains_key(&(coords_a, layer_id)));
    assert!(uploaded.contains_key(&(coords_b, layer_id)));
}
