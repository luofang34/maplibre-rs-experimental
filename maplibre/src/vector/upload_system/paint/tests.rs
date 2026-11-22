use super::*;
#[test]
fn zoom_paint_uses_one_view_zoom_for_all_source_levels() -> Result<(), serde_json::Error> {
    let layer: StyleLayer = serde_json::from_value(serde_json::json!({
        "id": "woods", "type": "fill", "paint": {
            "fill-color": ["interpolate", ["linear"], ["zoom"], 4, "#000000", 12, "#ffffff"],
            "fill-opacity": ["interpolate", ["linear"], ["zoom"], 4, 0, 12, 1]
        }
    }))?;
    assert_eq!(uniform_color(&layer, 8.0), Some([0.5, 0.5, 0.5, 0.5]));
    assert_eq!(uniform_color(&layer, 12.0), Some([1.0; 4]));
    Ok(())
}

#[test]
fn stationary_view_jitter_does_not_alternate_drape_styles() {
    let mut previous = None;
    for requested in [12.059, 12.066, 12.061, 12.071, 12.062] {
        let selected = super::next_zoom(previous, requested);
        assert_eq!(selected, 12.0);
        previous = Some(selected);
    }
    assert_eq!(super::next_zoom(previous, 12.3), 12.25);
    assert_eq!(super::next_zoom(Some(12.25), 12.13), 12.25);
}

#[test]
fn opacity_beyond_the_unit_range_is_clamped_in_the_layer_colour() {
    for (opacity, alpha) in [(2.0, 1.0), (-1.0, 0.0), (0.25, 0.25)] {
        let layer: StyleLayer = serde_json::from_value(serde_json::json!({
            "id": "land", "type": "fill", "source": "s", "source-layer": "land",
            "paint": {"fill-color": "#0000ff", "fill-opacity": opacity}
        }))
        .expect("layer");
        let color = uniform_color(&layer, 5.0).expect("uniform colour");
        assert!(
            (color[3] - alpha).abs() < 1e-6,
            "{opacity} gave {}",
            color[3]
        );
    }
}

#[test]
fn feature_zoom_threshold_coarsens_during_motion_and_snaps_when_settled() {
    assert_eq!(super::feature_zoom_threshold(false, 0.5), 1.0 / 32.0);
    assert_eq!(super::feature_zoom_threshold(false, 0.0), 1.0 / 32.0);
    assert_eq!(super::feature_zoom_threshold(true, 0.02), 0.125);
    assert_eq!(super::feature_zoom_threshold(true, 0.05), 0.25);
    assert_eq!(super::feature_zoom_threshold(true, 0.4), 0.25);
}

#[cfg(all(feature = "headless", feature = "thread-safe-futures"))]
#[tokio::test]
async fn shared_layer_metadata_fill_syncs_opaque_flag_across_cached_tiles_and_reentry() {
    use std::sync::Arc;

    use crate::{
        coords::{WorldTileCoords, ZoomLevel},
        headless::create_headless_renderer,
        render::{settings::BufferPoolSizes, ShaderVertex},
        style::Style,
        vector::{
            tessellation::OverAlignedVertexBuffer,
            upload_system::{bucket::upload_bucket, frame_state::VectorPaintFrame, DynPoints},
            AvailableVectorLayerBucket, VectorBufferPool,
        },
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
    let mut pool = VectorBufferPool::from_device_with_sizes(&renderer.device, sizes)
        .with_shared_layer_metadata();
    let style: Style = serde_json::from_str(
        r##"{"version":8,"sources":{},"layers":[{"id":"woods","type":"fill","paint":{"fill-color":"#ffffff","fill-opacity":["interpolate",["linear"],["zoom"],14,0,15,1]}}]}"##,
    )
    .expect("style");
    let style_layer = Arc::new(style.layers[0].clone());
    let tile_a = WorldTileCoords::from((0, 0, ZoomLevel::new(14)));
    let tile_b = WorldTileCoords::from((1, 0, ZoomLevel::new(14)));
    let make_bucket = |coords| AvailableVectorLayerBucket {
        coords,
        source_layer: "woods".into(),
        style_layer_id: "woods".into(),
        buffer: OverAlignedVertexBuffer::from_iters(
            vec![ShaderVertex::new([0.0, 0.0], [0.0, 0.0]); 3],
            [0_u32, 1, 2],
            3,
        ),
        feature_indices: vec![3],
        feature_colors: Vec::new(),
        feature_sort_keys: Vec::new(),
    };
    let bucket_a = make_bucket(tile_a);
    let bucket_b = make_bucket(tile_b);
    let tiles = Tiles::default();
    let empty_set = FxHashSet::default();
    let mut fm_scratch = Vec::new();
    let mut dyn_points = DynPoints::default();
    let mut scratch = PaintRefreshScratch::default();
    scratch.sync_style(&style);

    let frame_opaque = VectorPaintFrame {
        zoom: 15.0,
        bearing: 0.0,
        light: Default::default(),
    };
    let frame_translucent = VectorPaintFrame {
        zoom: 14.0,
        ..frame_opaque
    };

    // 1. Tile A uploads at z15.0 (`alpha == 1.0`, `opaque_color = true`).
    let outcome_a = upload_bucket(
        &mut pool,
        &renderer.queue,
        crate::vector::upload_system::bucket::BucketUploadInput {
            style_layer: &style_layer,
            bucket: &bucket_a,
            fallback_vertices: None,
            paint_entries: None,
            overscaled_zoom: 0,
            is_dynamic_circle: false,
            spatial: &[],
            paint: frame_opaque,
            protected: &empty_set,
        },
        crate::vector::upload_system::bucket::BucketUploadScratch {
            feature_metadata: &mut fm_scratch,
            dyn_points: &mut dyn_points,
        },
    );
    if let crate::vector::upload_system::bucket::UploadOutcome::Uploaded(Some((slot, meta))) =
        outcome_a
    {
        scratch.slots.record(slot, meta);
    }
    assert!(
        pool.index().get_layers(tile_a).expect("tile A")[0].is_opaque_color(),
        "tile A starts opaque at z15"
    );

    // 2. Tile A is out of view while zoom drops to z14.0 (`alpha == 0.0`) and Tile B uploads,
    // overwriting the shared `(layer_page, woods, z14)` GPU slot.
    let outcome_b = upload_bucket(
        &mut pool,
        &renderer.queue,
        crate::vector::upload_system::bucket::BucketUploadInput {
            style_layer: &style_layer,
            bucket: &bucket_b,
            fallback_vertices: None,
            paint_entries: None,
            overscaled_zoom: 0,
            is_dynamic_circle: false,
            spatial: &[],
            paint: frame_translucent,
            protected: &empty_set,
        },
        crate::vector::upload_system::bucket::BucketUploadScratch {
            feature_metadata: &mut fm_scratch,
            dyn_points: &mut dyn_points,
        },
    );
    if let crate::vector::upload_system::bucket::UploadOutcome::Uploaded(Some((slot, meta))) =
        outcome_b
    {
        scratch.slots.record(slot, meta);
    }
    assert!(
        !pool.index().get_layers(tile_a).expect("tile A")[0].is_opaque_color(),
        "shared-slot upload must clear opaque_color on cached Tile A to avoid black blend:None draw"
    );
    assert!(!pool.index().get_layers(tile_b).expect("tile B")[0].is_opaque_color());

    // 3. Even if Tile A's opaque_color became stale while out of view and re-enters on pure pan
    // (`zoom_changed == false`), `refresh_layer_paint` must reconcile `opaque_color`.
    pool.set_layer_opaque_color(tile_a, "woods", true);
    assert!(pool.index().get_layers(tile_a).expect("tile A")[0].is_opaque_color());
    let (_, tiles_changed) = scratch.slots.check_refresh(&[tile_a, tile_b], false, false);
    assert!(tiles_changed);
    refresh_layer_paint(
        &mut pool,
        &renderer.queue,
        PaintRefreshContext {
            tiles: &tiles,
            source_tiles: &[tile_a, tile_b],
            frame: frame_translucent,
            previous: Some(frame_translucent),
            moving: false,
            tiles_changed,
        },
        &mut scratch,
    );
    assert!(!pool.index().get_layers(tile_a).expect("tile A")[0].is_opaque_color());

    // 4. Zooming back up to z16.05 (`alpha == 1.0`) restores opaque_color on BOTH Tile A and Tile B.
    let frame_high = VectorPaintFrame {
        zoom: 16.05,
        ..frame_opaque
    };
    let (_, tiles_changed) = scratch.slots.check_refresh(&[tile_a, tile_b], true, false);
    refresh_layer_paint(
        &mut pool,
        &renderer.queue,
        PaintRefreshContext {
            tiles: &tiles,
            source_tiles: &[tile_a, tile_b],
            frame: frame_high,
            previous: Some(frame_translucent),
            moving: false,
            tiles_changed,
        },
        &mut scratch,
    );
    assert!(pool.index().get_layers(tile_a).expect("tile A")[0].is_opaque_color());
    assert!(pool.index().get_layers(tile_b).expect("tile B")[0].is_opaque_color());
}

#[cfg(all(feature = "headless", feature = "thread-safe-futures"))]
#[tokio::test]
async fn translucent_outlined_fill_stays_blended_across_tile_and_zoom_refresh() {
    use std::sync::Arc;

    use crate::{
        coords::{WorldTileCoords, ZoomLevel},
        headless::create_headless_renderer,
        render::{settings::BufferPoolSizes, ShaderVertex},
        style::Style,
        vector::{
            tessellation::OverAlignedVertexBuffer,
            upload_system::{bucket::upload_bucket, frame_state::VectorPaintFrame, DynPoints},
            AvailableVectorLayerBucket, VectorBufferPool,
        },
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
    let mut pool = VectorBufferPool::from_device_with_sizes(&renderer.device, sizes)
        .with_shared_layer_metadata();
    // Models OpenFreeMap bright's `building-top` layer: `fill-outline-color` forces per-feature
    // metadata while `fill-opacity` is < 1.0 and `fill-translate` triggers `meta_dirty`.
    let style: Style = serde_json::from_str(
        r##"{"version":8,"sources":{},"layers":[{"id":"building-top","type":"fill","paint":{"fill-color":"#f2eae2","fill-opacity":["interpolate",["linear"],["zoom"],13,0,16,1],"fill-outline-color":"#dfdbd7","fill-translate":["interpolate",["linear"],["zoom"],14,["literal",[0,0]],16,["literal",[-2,-2]]]}}]}"##,
    )
    .expect("style");
    let style_layer = Arc::new(style.layers[0].clone());
    let tile_a = WorldTileCoords::from((0, 0, ZoomLevel::new(14)));
    let tile_b = WorldTileCoords::from((1, 0, ZoomLevel::new(14)));
    let make_bucket = |coords| AvailableVectorLayerBucket {
        coords,
        source_layer: "building".into(),
        style_layer_id: "building-top".into(),
        buffer: OverAlignedVertexBuffer::from_iters(
            vec![ShaderVertex::new([0.0, 0.0], [0.0, 0.0]); 3],
            [0_u32, 1, 2],
            3,
        ),
        feature_indices: vec![3],
        feature_colors: vec![[0.89, 0.82, 0.77, 0.533]],
        feature_sort_keys: Vec::new(),
    };
    let tiles = Tiles::default();
    let empty_set = FxHashSet::default();
    let mut fm_scratch = Vec::new();
    let mut dyn_points = DynPoints::default();
    let mut scratch = PaintRefreshScratch::default();
    scratch.sync_style(&style);

    let frame_14_6 = VectorPaintFrame {
        zoom: 14.6,
        bearing: 0.0,
        light: Default::default(),
    };
    for coords in [tile_a, tile_b] {
        let bucket = make_bucket(coords);
        if let crate::vector::upload_system::bucket::UploadOutcome::Uploaded(Some((slot, meta))) =
            upload_bucket(
                &mut pool,
                &renderer.queue,
                crate::vector::upload_system::bucket::BucketUploadInput {
                    style_layer: &style_layer,
                    bucket: &bucket,
                    fallback_vertices: None,
                    paint_entries: None,
                    overscaled_zoom: 0,
                    is_dynamic_circle: false,
                    spatial: &[],
                    paint: frame_14_6,
                    protected: &empty_set,
                },
                crate::vector::upload_system::bucket::BucketUploadScratch {
                    feature_metadata: &mut fm_scratch,
                    dyn_points: &mut dyn_points,
                },
            )
        {
            scratch.slots.record(slot, meta);
        }
        assert!(
            !pool.index().get_layers(coords).expect("tile")[0].is_opaque_color(),
            "translucent outlined fill must start blended"
        );
    }

    // 1. Stationary boot / pan arrival (`zoom_changed == false`, `tiles_changed == true`).
    let (_, tiles_changed) = scratch.slots.check_refresh(&[tile_a, tile_b], false, false);
    assert!(tiles_changed);
    refresh_layer_paint(
        &mut pool,
        &renderer.queue,
        PaintRefreshContext {
            tiles: &tiles,
            source_tiles: &[tile_a, tile_b],
            frame: frame_14_6,
            previous: Some(frame_14_6),
            moving: false,
            tiles_changed,
        },
        &mut scratch,
    );
    assert!(
        !pool.index().get_layers(tile_a).expect("tile A")[0].is_opaque_color(),
        "tiles_changed refresh must not mark non-uniform translucent fill opaque"
    );
    assert!(
        !pool.index().get_layers(tile_b).expect("tile B")[0].is_opaque_color(),
        "tiles_changed refresh must not mark non-uniform translucent fill opaque"
    );

    // 2. Zoom change during fast zoom-in (`z = 14.23`).
    let frame_14_23 = VectorPaintFrame {
        zoom: 14.23,
        ..frame_14_6
    };
    let (_, tiles_changed) = scratch.slots.check_refresh(&[tile_a, tile_b], true, false);
    refresh_layer_paint(
        &mut pool,
        &renderer.queue,
        PaintRefreshContext {
            tiles: &tiles,
            source_tiles: &[tile_a, tile_b],
            frame: frame_14_23,
            previous: Some(frame_14_6),
            moving: true,
            tiles_changed,
        },
        &mut scratch,
    );
    assert!(
        !pool.index().get_layers(tile_a).expect("tile A")[0].is_opaque_color(),
        "zoom refresh must keep non-uniform translucent fill blended"
    );
    assert!(
        !pool.index().get_layers(tile_b).expect("tile B")[0].is_opaque_color(),
        "zoom refresh must keep non-uniform translucent fill blended"
    );

    // 3. Non-uniform per-tile `set_layer_opaque_color_by_id` must not overwrite sibling tiles
    // that happen to share the `(layer_page, style_layer_id, tile_z)` metadata slot.
    pool.set_layer_opaque_color(tile_a, "building-top", true);
    assert!(pool.index().get_layers(tile_a).expect("tile A")[0].is_opaque_color());
    assert!(
        !pool.index().get_layers(tile_b).expect("tile B")[0].is_opaque_color(),
        "non-uniform sibling tile must retain its independent opaque_color flag"
    );
}
