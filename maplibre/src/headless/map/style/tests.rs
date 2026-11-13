use image::{Rgba, RgbaImage};
use serde_json::json;

use crate::{
    background::BackgroundPlugin,
    coords::{WorldTileCoords, ZoomLevel},
    headless::{
        create_headless_renderer,
        map::{
            process_geojson_layers,
            readback::{FrameReadback, PixelLayout},
            HeadlessMap,
        },
        HeadlessPlugin,
    },
    projection::ProjectionType,
    raster::{
        resource::RasterResources, AvailableRasterLayerData, DefaultRasterTransferables,
        RasterLayersDataComponent, RasterPlugin, RasterSourceId,
    },
    render::{eventually::Eventually, RenderPlugin},
    sdf::query::{QueryGeometry, QueryOptions},
    style::{
        feature_state::FeatureStateTarget,
        source::{GeoJsonData, Source},
        Style,
    },
    vector::{DefaultVectorTransferables, VectorPlugin},
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

async fn vector_map(style_json: serde_json::Value) -> HeadlessMap {
    let style: Style = serde_json::from_value(style_json).expect("valid style");
    let Some(Source::GeoJson(source)) = style.sources.get("parks") else {
        panic!("expected inline parks geojson source");
    };
    let GeoJsonData::Inline(raw_geojson) = &source.data else {
        panic!("expected inline geojson data");
    };
    let annotated = style.apply_feature_state_to_geojson("parks", raw_geojson);
    let layers = style
        .layers
        .iter()
        .filter(|layer| layer.source.as_deref() == Some("parks"))
        .cloned()
        .collect();
    let processed = process_geojson_layers(
        &annotated,
        "parks",
        layers,
        tile(),
        ProjectionType::Mercator,
    )
    .expect("processed geojson");
    let (kernel, renderer) = create_headless_renderer(64, 64, None)
        .await
        .expect("headless renderer");
    let mut map = HeadlessMap::new(
        style,
        renderer,
        kernel,
        vec![
            Box::new(RenderPlugin),
            Box::new(BackgroundPlugin),
            Box::new(VectorPlugin::<DefaultVectorTransferables>::default()),
            Box::new(HeadlessPlugin::new(false).preserve_tile_sources()),
        ],
    )
    .expect("headless map");
    map.render_frames_with_terrain(processed, vec![], vec![], 2)
        .expect("initial frames");
    map
}

fn center_rgba(map: &HeadlessMap) -> [u8; 4] {
    let mut readback = FrameReadback::new(map, 64, 64);
    let mut pixels = vec![0_u8; readback.frame_len(PixelLayout::Rgba8)];
    readback
        .read_into_blocking(
            map,
            map.head_texture().expect("head texture"),
            PixelLayout::Rgba8,
            &mut pixels,
        )
        .expect("readback");
    let offset = (32 * 64 + 32) * 4;
    [
        pixels[offset],
        pixels[offset + 1],
        pixels[offset + 2],
        pixels[offset + 3],
    ]
}

fn world_box_feature(properties: serde_json::Value) -> serde_json::Value {
    json!({
        "type": "Feature",
        "properties": properties,
        "geometry": {
            "type": "Polygon",
            "coordinates": [[[-80.0, -80.0], [80.0, -80.0], [80.0, 80.0], [-80.0, 80.0], [-80.0, -80.0]]]
        }
    })
}

#[tokio::test]
async fn data_driven_paint_and_layer_order_refresh_gpu_buffers_in_place() {
    let mut map = vector_map(json!({
        "version": 8,
        "center": [0.0, 0.0],
        "zoom": 0.0,
        "sources": {
            "parks": {
                "type": "geojson",
                "data": {
                    "type": "FeatureCollection",
                    "features": [world_box_feature(json!({"kind": "park"}))]
                }
            }
        },
        "layers": [
            {"id": "bg", "type": "background", "paint": {"background-color": "#000000"}},
            {"id": "bottom", "type": "fill", "source": "parks", "paint": {"fill-color": "#ff0000"}},
            {
                "id": "top",
                "type": "fill",
                "source": "parks",
                "paint": {"fill-color": ["match", ["get", "kind"], "park", "#00ff00", "#ffffff"]}
            }
        ]
    }))
    .await;
    assert_eq!(center_rgba(&map), [0, 255, 0, 255]);

    let paint_change = map
        .mutate_style(|style| {
            style.set_paint_property(
                "top",
                "fill-color",
                json!(["match", ["get", "kind"], "park", "#0000ff", "#ffffff"]),
            )
        })
        .expect("data-driven paint update");
    assert!(
        !paint_change.redraw_tiles,
        "data-driven fill-color must update GPU buffers in-place without re-tessellating tiles"
    );
    map.run_frame().expect("in-place paint frame");
    assert_eq!(center_rgba(&map), [0, 0, 255, 255]);

    let order_change = map
        .mutate_style(|style| style.move_layer("bottom", None))
        .expect("move_layer");
    assert!(
        !order_change.redraw_tiles,
        "reordering layers must update GPU layer indices in-place"
    );
    map.run_frame().expect("reordered frame");
    assert_eq!(center_rgba(&map), [255, 0, 0, 255]);
}

#[tokio::test]
async fn runtime_feature_state_updates_gpu_fill_and_queries_in_place() {
    let mut map = vector_map(json!({
        "version": 8,
        "center": [0.0, 0.0],
        "zoom": 0.0,
        "sources": {
            "parks": {
                "type": "geojson",
                "promoteId": "slug",
                "data": {
                    "type": "FeatureCollection",
                    "features": [world_box_feature(json!({"slug": "central", "title": "Central"}))]
                }
            }
        },
        "layers": [
            {"id": "bg", "type": "background", "paint": {"background-color": "#000000"}},
            {
                "id": "park-fill",
                "type": "fill",
                "source": "parks",
                "paint": {
                    "fill-color": ["case", ["boolean", ["feature-state", "hover"], false], "#00ff00", "#ff0000"]
                }
            }
        ]
    }))
    .await;
    assert_eq!(center_rgba(&map), [255, 0, 0, 255]);

    let target = FeatureStateTarget::geojson("parks");
    let mut hover_state = serde_json::Map::new();
    hover_state.insert("hover".to_string(), json!(true));
    map.set_feature_state(target.clone(), "central", hover_state);
    assert_eq!(
        map.feature_state(&target, "central")
            .and_then(|state| state.get("hover")),
        Some(&json!(true))
    );

    map.run_frame().expect("in-place feature-state frame");
    assert_eq!(center_rgba(&map), [0, 255, 0, 255]);

    let hovered = map
        .query_rendered_features(
            QueryGeometry::Point([32.0, 32.0]),
            &QueryOptions {
                layers: Some(vec!["park-fill".to_string()]),
                filter: None,
            },
        )
        .expect("query hovered feature");
    assert_eq!(hovered.len(), 1);
    assert_eq!(hovered[0].properties.get("title"), Some(&json!("Central")));
    assert_eq!(hovered[0].state.get("hover"), Some(&json!(true)));
    assert!(
        !hovered[0]
            .properties
            .keys()
            .any(|key| key.starts_with('\u{1}')),
        "internal state tags must never leak into queried feature properties"
    );

    map.remove_feature_state(&target, Some("central".into()), Some("hover"));
    map.run_frame().expect("cleared feature-state frame");
    assert_eq!(center_rgba(&map), [255, 0, 0, 255]);
    let after_clear = map
        .query_rendered_features(
            QueryGeometry::Point([32.0, 32.0]),
            &QueryOptions {
                layers: Some(vec!["park-fill".to_string()]),
                filter: None,
            },
        )
        .expect("query after clear");
    assert_eq!(after_clear.len(), 1);
    assert!(after_clear[0].state.is_empty());
}

#[tokio::test]
async fn paint_only_global_state_refreshes_gpu_fill_in_place() {
    let mut map = vector_map(json!({
        "version": 8,
        "center": [0.0, 0.0],
        "zoom": 0.0,
        "state": {
            "accent": {"default": "#ff0000"}
        },
        "sources": {
            "parks": {
                "type": "geojson",
                "data": {
                    "type": "FeatureCollection",
                    "features": [world_box_feature(json!({"kind": "park"}))]
                }
            }
        },
        "layers": [
            {"id": "bg", "type": "background", "paint": {"background-color": "#000000"}},
            {
                "id": "park-fill",
                "type": "fill",
                "source": "parks",
                "paint": {"fill-color": ["global-state", "accent"]}
            }
        ]
    }))
    .await;
    assert_eq!(center_rgba(&map), [255, 0, 0, 255]);

    map.set_global_state("accent", json!("#00ff00"));
    assert_eq!(map.global_state().get("accent"), Some(&json!("#00ff00")));
    map.run_frame().expect("in-place global-state frame");
    assert_eq!(center_rgba(&map), [0, 255, 0, 255]);
}

mod review_regressions;
