//! A fixed drag across draped terrain: every frame is covered, and dragging back reuses the
//! tiles and drapes the way out loaded.

use super::*;
use crate::raster::RasterLayersDataComponent;

/// World pixels per step at the style zoom, under half the viewport, so each step keeps most
/// of the previous view and adds a strip of new tiles.
const STEP: f64 = 200.0;
const STEPS: usize = 6;

async fn cached_map() -> HeadlessMap {
    let (kernel, renderer) = create_headless_renderer(SIZE, SIZE, None)
        .await
        .expect("renderer");
    // Without retained supplied tiles the normal cache budget decides what stays resident.
    let mut map = HeadlessMap::new(
        coverage_style(false, false),
        renderer,
        kernel,
        vec![
            Box::new(RenderPlugin),
            Box::new(RasterPlugin::<DefaultRasterTransferables>::default()),
            Box::new(HillshadePlugin),
            Box::new(TerrainPlugin::<DefaultDemTransferables>::default()),
            Box::new(HeadlessPlugin::new(false).preserve_tile_sources()),
        ],
    )
    .expect("map");
    let dem = target().get_parent().and_then(|parent| parent.get_parent());
    map.load_dem_tiles(vec![(
        dem.expect("DEM ancestor"),
        RgbaImage::from_pixel(256, 256, Rgba([128, 0, 0, 255])),
    )])
    .expect("terrain");
    map
}

fn resident(map: &HeadlessMap, coords: WorldTileCoords) -> bool {
    map.map_context
        .world
        .tiles
        .query::<&RasterLayersDataComponent>(coords)
        .is_some_and(|component| !component.layers.is_empty())
}

#[derive(Debug, Default)]
struct Frame {
    loads: usize,
    hits: usize,
    drape_redraws: u32,
    upload_bytes: u64,
}

/// Supplies what the frame's covering lacks, renders it and checks every pixel shows a tile.
fn drag_frame(map: &mut HeadlessMap, name: &str) -> Frame {
    let required = map
        .required_raster_tile_coords("paint")
        .expect("source covering");
    let missing: Vec<_> = required
        .iter()
        .copied()
        .filter(|coords| !resident(map, *coords))
        .collect();
    map.render_source_frames(
        ProcessedLayers::default(),
        missing
            .iter()
            .map(|coords| tile(*coords, false, false))
            .collect(),
        1,
    )
    .expect("drag frame");
    let pixels = read_blocking(map, name);
    let holes = pixels
        .chunks_exact(4)
        .filter(|pixel| !(pixel[1] > 200 && pixel[0] < 40 && pixel[2] < 40))
        .count();
    assert_eq!(holes, 0, "{name}: every pixel shows a loaded tile");
    let stats = map.last_frame_stats();
    Frame {
        loads: missing.len(),
        hits: required.len() - missing.len(),
        drape_redraws: stats.drape_redraws,
        upload_bytes: stats.upload_bytes,
    }
}

fn drag(map: &mut HeadlessMap, direction: f64, name: &str) -> Vec<Frame> {
    (0..STEPS)
        .map(|step| {
            map.map_context
                .view_state
                .camera_mut()
                .move_relative(cgmath::Vector2::new(direction * STEP, 0.0));
            drag_frame(map, &format!("{name}-{step}"))
        })
        .collect()
}

#[tokio::test]
async fn a_drag_is_covered_every_frame_and_dragging_back_reuses_its_tiles_and_drapes() {
    let mut map = cached_map().await;
    let start = drag_frame(&mut map, "drag-start");
    assert!(start.loads > 0, "the first view loads its tiles");
    let out = drag(&mut map, 1.0, "drag-out");
    assert!(
        out.iter().any(|frame| frame.loads > 0),
        "dragging out reaches tiles the start did not load: {out:?}"
    );
    let back = drag(&mut map, -1.0, "drag-back");
    let one_tile = 256 * 256 * 4;
    for (step, frame) in back.iter().enumerate() {
        assert_eq!(
            frame.loads, 0,
            "dragging back loads nothing at {step}: {frame:?}"
        );
        assert!(frame.hits > 0, "dragging back hits the cache at {step}");
        assert!(
            frame.upload_bytes < one_tile,
            "dragging back uploads no tile at {step}: {frame:?}"
        );
    }
    // A drape follows the source tiles in view, as GL JS fingerprints it, so one that more of
    // its tile scrolls into is drawn again from resident tiles; the start view's drapes were
    // last drawn for the start view itself and come back untouched.
    let end = back.last().expect("return leg");
    assert_eq!(
        end.drape_redraws, 0,
        "the start view reuses its drapes: {end:?}"
    );
    // Leaving for tiles never seen and coming straight back finds the start view's drapes
    // parked with their content.
    let away = jump(&mut map, 4.0 * STEP * STEPS as f64, "jump-away");
    assert!(away.loads > 0 && away.drape_redraws > 0, "{away:?}");
    let home = jump(&mut map, -4.0 * STEP * STEPS as f64, "jump-home");
    assert_eq!(home.loads, 0, "the start view is still cached: {home:?}");
    assert_eq!(home.drape_redraws, 0, "its drapes come back: {home:?}");
}

fn jump(map: &mut HeadlessMap, distance: f64, name: &str) -> Frame {
    map.map_context
        .view_state
        .camera_mut()
        .move_relative(cgmath::Vector2::new(distance, 0.0));
    drag_frame(map, name)
}

mod zoom;
