//! A frame's tile uploads, counted by the renderer's queue, stay within the shared budget.

use image::{Rgba, RgbaImage};

use super::super::{FRAME_UPLOAD_BYTES, IMAGE_UPLOAD_FLOOR_BYTES};
use crate::{
    coords::WorldTileCoords,
    headless::{create_headless_renderer, map::HeadlessMap, HeadlessPlugin},
    raster::{AvailableRasterLayerData, DefaultRasterTransferables, RasterPlugin},
    render::RenderPlugin,
    style::Style,
};

/// Raster sources drawn over one tile, each with a texture of its own.
const SOURCES: usize = 40;
/// Bytes of each source's picture.
const PICTURE_BYTES: usize = 512 * 512 * 4;
/// Bytes a frame writes besides tile textures: uniforms and view buffers.
const OTHER_WRITES: u64 = 1 << 20;

async fn many_sources_map() -> HeadlessMap {
    let sources: serde_json::Map<_, _> = (0..SOURCES)
        .map(|index| {
            (
                format!("pic{index}"),
                serde_json::json!({"type": "raster", "tiles": ["offline://{z}/{x}/{y}"]}),
            )
        })
        .collect();
    let layers: Vec<_> = (0..SOURCES)
        .map(|index| {
            serde_json::json!({"id": format!("pic{index}"), "type": "raster",
                "source": format!("pic{index}"), "paint": {"raster-fade-duration": 0}})
        })
        .collect();
    let style: Style = serde_json::from_value(serde_json::json!({"version": 8, "zoom": 0,
        "sources": sources, "layers": layers}))
    .expect("style");
    let (kernel, renderer) = create_headless_renderer(64, 64, None)
        .await
        .expect("renderer");
    HeadlessMap::new(
        style,
        renderer,
        kernel,
        vec![
            Box::new(RenderPlugin),
            Box::new(RasterPlugin::<DefaultRasterTransferables>::default()),
            Box::new(HeadlessPlugin::new(false).preserve_tile_sources()),
        ],
    )
    .expect("map")
}

fn pictures() -> Vec<AvailableRasterLayerData> {
    (0..SOURCES)
        .map(|index| AvailableRasterLayerData {
            coords: WorldTileCoords::default(),
            source: format!("pic{index}").as_str().into(),
            image: RgbaImage::from_pixel(512, 512, Rgba([index as u8, 0, 0, 255])),
        })
        .collect()
}

#[tokio::test]
async fn a_burst_of_textures_is_spread_over_frames_within_the_budget() {
    const { assert!(SOURCES * PICTURE_BYTES > FRAME_UPLOAD_BYTES) };
    let mut map = many_sources_map().await;
    map.render_frames_with_terrain(Default::default(), pictures(), vec![], 1)
        .expect("frame");
    let first = map.last_frame_stats().upload_bytes;
    assert!(
        first <= FRAME_UPLOAD_BYTES as u64 + OTHER_WRITES,
        "the first frame uploads {first} bytes"
    );
    assert!(
        first >= IMAGE_UPLOAD_FLOOR_BYTES as u64,
        "textures do not starve: {first}"
    );

    map.run_frame().expect("second frame");
    let second = map.last_frame_stats().upload_bytes;
    let total = first + second;
    assert!(
        total >= (SOURCES * PICTURE_BYTES) as u64,
        "the rest follow in the next frame: {total}"
    );
    map.run_frame().expect("still frame");
    assert!(
        map.last_frame_stats().upload_bytes < PICTURE_BYTES as u64,
        "every texture was uploaded once"
    );
}
