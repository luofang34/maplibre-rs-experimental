use image::{Rgba, RgbaImage};

use crate::{
    coords::{WorldTileCoords, ZoomLevel},
    headless::{create_headless_renderer, map::HeadlessMap, HeadlessPlugin},
    raster::{
        resource::RasterResources, AvailableRasterLayerData, DefaultRasterTransferables,
        RasterLayersDataComponent, RasterPlugin, RasterSourceId,
    },
    render::{eventually::Eventually, RenderPlugin},
    style::Style,
};

fn tile() -> WorldTileCoords {
    WorldTileCoords {
        x: 0,
        y: 0,
        z: ZoomLevel::from(0),
    }
}

fn textures(map: &HeadlessMap) -> usize {
    match map.world().resources.get::<Eventually<RasterResources>>() {
        Some(Eventually::Initialized(raster)) => raster.texture_count(&RasterSourceId::from("pic")),
        _ => 0,
    }
}

fn results(map: &HeadlessMap) -> usize {
    map.world()
        .tiles
        .query::<&RasterLayersDataComponent>(tile())
        .map_or(0, |component| component.layers.len())
}

#[tokio::test]
async fn a_removed_image_source_leaves_no_tile_or_texture_behind() {
    let style: Style = serde_json::from_value(serde_json::json!({"version": 8, "zoom": 0,
        "sources": {"pic": {"type": "image", "url": "offline://pic.png",
            "coordinates": [[-180, 85.0511], [180, 85.0511], [180, -85.0511], [-180, -85.0511]]}},
        "layers": [{"id": "pic", "type": "raster", "source": "pic"}]}))
    .expect("style");
    let (kernel, renderer) = create_headless_renderer(64, 64, None)
        .await
        .expect("renderer");
    let mut map = HeadlessMap::new(
        style,
        renderer,
        kernel,
        vec![
            Box::new(RenderPlugin),
            Box::new(RasterPlugin::<DefaultRasterTransferables>::default()),
            Box::new(HeadlessPlugin::new(false).preserve_tile_sources()),
        ],
    )
    .expect("map");
    let picture = || AvailableRasterLayerData {
        coords: tile(),
        source: "pic".into(),
        image: RgbaImage::from_pixel(8, 8, Rgba([255, 0, 0, 255])),
    };
    map.render_frames_with_terrain(Default::default(), vec![picture()], vec![], 2)
        .expect("frames");
    assert_eq!(textures(&map), 1, "the drawn image has a texture");

    map.mutate_style(|style| style.remove_layer("pic"))
        .expect("remove layer");
    map.mutate_style(|style| style.remove_source("pic"))
        .expect("remove source");
    assert_eq!(textures(&map), 0, "the texture goes with the source");
    assert_eq!(results(&map), 0, "so does the tile's image");

    // A result that was already on its way lands after the source left.
    map.render_frames_with_terrain(Default::default(), vec![picture()], vec![], 2)
        .expect("frames after removal");
    assert_eq!(textures(&map), 0, "a late result gets no texture");
}
