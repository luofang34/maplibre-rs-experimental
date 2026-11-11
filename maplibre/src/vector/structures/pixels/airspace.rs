//! GPU pixel tests for translucent 3D airspace volumes, floor caps, MSL/AGL datums, and
//! elevated 3D rim lines (`line-elevation`).

use cgmath::{Matrix4, Rad, Vector3};

use super::{read_blocking, render_at_anchor};
use crate::{
    coords::{LatLon, WorldTileCoords, ZoomLevel},
    headless::{create_headless_renderer, map::HeadlessMap},
    projection::ProjectionType,
    render::RenderPlugin,
    style::Style,
};

const ANCHOR: LatLon = LatLon {
    latitude: 0.044,
    longitude: 0.044,
};
const SEA_LEVEL_DEM: image::Rgba<u8> = image::Rgba([128, 0, 0, 255]);
/// `(129 * 256 + 244) - 32768 = 500 m` in Terrarium DEM encoding.
const PLATEAU_500M_DEM: image::Rgba<u8> = image::Rgba([129, 244, 0, 255]);

fn tile_z12() -> WorldTileCoords {
    WorldTileCoords::from((2048, 2047, ZoomLevel::new(12)))
}

async fn build_globe_map(
    mut style: Style,
    dem_pixel: image::Rgba<u8>,
    geojson: &serde_json::Value,
    source_layer: &str,
) -> HeadlessMap {
    let tile = tile_z12();
    style.center = Some([ANCHOR.longitude, ANCHOR.latitude]);
    style.zoom = Some(12.0);
    for (index, layer) in style.layers.iter_mut().enumerate() {
        layer.index = index as u32;
    }
    let vector_layers = style.layers[1..].to_vec();
    let (kernel, renderer) = create_headless_renderer(64, 64, None)
        .await
        .expect("headless renderer");
    let mut map = HeadlessMap::new(
        style,
        renderer,
        kernel,
        vec![
            Box::new(RenderPlugin),
            Box::new(crate::background::BackgroundPlugin),
            Box::new(crate::vector::VectorPlugin::<
                crate::vector::DefaultVectorTransferables,
            >::default()),
            Box::new(crate::terrain::TerrainPlugin::<
                crate::terrain::DefaultDemTransferables,
            >::default()),
            Box::new(crate::headless::HeadlessPlugin::new(false).preserve_tile_sources()),
        ],
    )
    .expect("headless map");
    map.load_dem_tiles(vec![
        (
            WorldTileCoords::default(),
            image::RgbaImage::from_pixel(2, 2, dem_pixel),
        ),
        (tile, image::RgbaImage::from_pixel(2, 2, dem_pixel)),
    ])
    .expect("DEM upload");
    let processed = map
        .process_geojson(
            geojson,
            source_layer,
            vector_layers,
            tile,
            ProjectionType::Globe,
        )
        .expect("geojson tessellation");
    map.render_tile(processed).expect("vector upload");
    map
}

fn center_pixel(pixels: &[u8]) -> [u8; 4] {
    let offset = (32 * 64 + 32) * 4;
    [
        pixels[offset],
        pixels[offset + 1],
        pixels[offset + 2],
        pixels[offset + 3],
    ]
}

#[tokio::test]
async fn floating_translucent_volume_renders_from_above_and_floor_cap_from_below() {
    let style: Style = serde_json::from_value(serde_json::json!({
        "version": 8,
        "projection": {"type": "globe"},
        "terrain": {"source": "dem"},
        "sources": {
            "airspace": {"type": "vector", "tiles": ["https://airspace.example/{z}/{x}/{y}.pbf"]},
            "dem": {"type": "raster-dem", "tiles": ["https://dem.example/{z}/{x}/{y}.png"], "encoding": "terrarium"}
        },
        "layers": [
            {"id": "bg", "type": "background", "paint": {"background-color": "#700000"}},
            {
                "id": "shelf",
                "type": "fill-extrusion",
                "source": "airspace",
                "source-layer": "airspace",
                "paint": {
                    "fill-extrusion-translucent": true,
                    "fill-extrusion-color": "#0088ff",
                    "fill-extrusion-opacity": 0.5,
                    "fill-extrusion-base": ["get", "lower_m"],
                    "fill-extrusion-height": ["get", "upper_m"],
                    "fill-extrusion-base-datum": "msl",
                    "fill-extrusion-height-datum": "msl"
                }
            }
        ]
    }))
    .expect("style");
    let shelf = serde_json::json!({
        "type": "Feature",
        "properties": {"lower_m": 600.0, "upper_m": 1400.0},
        "geometry": {
            "type": "Polygon",
            "coordinates": [[[0.020, 0.020], [0.068, 0.020], [0.068, 0.068], [0.020, 0.068], [0.020, 0.020]]]
        }
    });
    let mut map = build_globe_map(style, SEA_LEVEL_DEM, &shelf, "airspace").await;

    // Looking down from 2500 m: blue shelf blends translucently over the dark-red ground.
    render_at_anchor(
        &mut map,
        Matrix4::from_translation(Vector3::new(0.0, 0.0, 2500.0)),
        ANCHOR,
    );
    let top_view = center_pixel(&read_blocking(&map));
    assert!(
        top_view[2] > 80 && top_view[0] > 15,
        "expected translucent blue over red ground from above, got {top_view:?}"
    );

    // Looking 35° ABOVE the horizon from inside the volume (1000 m): only the back-face roof (1400 m) is ahead.
    let upward_pitch = Rad(std::f64::consts::FRAC_PI_2 + 0.61);
    let look_up_inside = Matrix4::from_translation(Vector3::new(0.0, -700.0, 1000.0))
        * Matrix4::from_angle_x(upward_pitch);
    render_at_anchor(&mut map, look_up_inside, ANCHOR);
    let inside_up = center_pixel(&read_blocking(&map));

    // Looking 35° ABOVE the horizon from below the shelf (150 m): front-face floor cap (600 m) + roof are ahead.
    let look_up_below = Matrix4::from_translation(Vector3::new(0.0, -700.0, 150.0))
        * Matrix4::from_angle_x(upward_pitch);
    render_at_anchor(&mut map, look_up_below, ANCHOR);
    let below_up = center_pixel(&read_blocking(&map));

    assert!(
        below_up[2] > 70 && below_up[2] > inside_up[2].saturating_add(20),
        "floor cap at 600 m must render outward-facing when viewed from underneath: below={below_up:?}, inside={inside_up:?}"
    );
}

#[tokio::test]
async fn nested_translucent_volumes_blend_through_outer_shell() {
    let style: Style = serde_json::from_value(serde_json::json!({
        "version": 8,
        "projection": {"type": "globe"},
        "terrain": {"source": "dem"},
        "sources": {
            "airspace": {"type": "vector", "tiles": ["https://airspace.example/{z}/{x}/{y}.pbf"]},
            "dem": {"type": "raster-dem", "tiles": ["https://dem.example/{z}/{x}/{y}.png"], "encoding": "terrarium"}
        },
        "layers": [
            {"id": "bg", "type": "background", "paint": {"background-color": "#000000"}},
            {
                "id": "volumes",
                "type": "fill-extrusion",
                "source": "airspace",
                "source-layer": "airspace",
                "paint": {
                    "fill-extrusion-translucent": true,
                    "fill-extrusion-color": ["get", "color"],
                    "fill-extrusion-opacity": ["get", "alpha"],
                    "fill-extrusion-base": ["get", "lower_m"],
                    "fill-extrusion-height": ["get", "upper_m"],
                    "fill-extrusion-base-datum": "msl",
                    "fill-extrusion-height-datum": "msl"
                }
            }
        ]
    }))
    .expect("style");
    let nested = serde_json::json!({
        "type": "FeatureCollection",
        "features": [
            {
                "type": "Feature",
                "properties": {"lower_m": 0.0, "upper_m": 2000.0, "color": "#0033ff", "alpha": 0.35},
                "geometry": {
                    "type": "Polygon",
                    "coordinates": [[[0.012, 0.012], [0.076, 0.012], [0.076, 0.076], [0.012, 0.076], [0.012, 0.012]]]
                }
            },
            {
                "type": "Feature",
                "properties": {"lower_m": 250.0, "upper_m": 1200.0, "color": "#00ff33", "alpha": 0.8},
                "geometry": {
                    "type": "Polygon",
                    "coordinates": [[[0.041, 0.041], [0.047, 0.041], [0.047, 0.047], [0.041, 0.047], [0.041, 0.041]]]
                }
            }
        ]
    });
    let mut map = build_globe_map(style, SEA_LEVEL_DEM, &nested, "airspace").await;
    render_at_anchor(
        &mut map,
        Matrix4::from_translation(Vector3::new(0.0, 0.0, 2500.0)),
        ANCHOR,
    );
    let pixels = read_blocking(&map);
    let inner = center_pixel(&pixels);
    let outer_only_offset = (6 * 64 + 32) * 4;
    let outer_only = [
        pixels[outer_only_offset],
        pixels[outer_only_offset + 1],
        pixels[outer_only_offset + 2],
    ];
    assert!(
        outer_only[2] > 50 && outer_only[1] < 60,
        "outer shell must render translucent blue: {outer_only:?}"
    );
    assert!(
        inner[1] > 90 && inner[2] > 35,
        "inner green cylinder must stay visible through outer blue roof: {inner:?}"
    );
}

#[tokio::test]
async fn msl_shelf_stays_at_sea_level_while_agl_volume_rises_with_terrain() {
    for (datum, expect_visible) in [("msl", false), ("agl", true)] {
        let style: Style = serde_json::from_value(serde_json::json!({
            "version": 8,
            "projection": {"type": "globe"},
            "terrain": {"source": "dem"},
            "sources": {
                "airspace": {"type": "vector", "tiles": ["https://airspace.example/{z}/{x}/{y}.pbf"]},
                "dem": {"type": "raster-dem", "tiles": ["https://dem.example/{z}/{x}/{y}.png"], "encoding": "terrarium"}
            },
            "layers": [
                {"id": "bg", "type": "background", "paint": {"background-color": "#770000"}},
                {
                    "id": "volume",
                    "type": "fill-extrusion",
                    "source": "airspace",
                    "source-layer": "airspace",
                    "paint": {
                        "fill-extrusion-translucent": true,
                        "fill-extrusion-color": "#00ff88",
                        "fill-extrusion-opacity": 0.75,
                        "fill-extrusion-base": 0.0,
                        "fill-extrusion-height": 350.0,
                        "fill-extrusion-base-datum": datum,
                        "fill-extrusion-height-datum": datum
                    }
                }
            ]
        }))
        .expect("style");
        let box_feature = serde_json::json!({
            "type": "Feature",
            "properties": {},
            "geometry": {
                "type": "Polygon",
                "coordinates": [[[0.025, 0.025], [0.063, 0.025], [0.063, 0.063], [0.025, 0.063], [0.025, 0.025]]]
            }
        });
        let mut map = build_globe_map(style, PLATEAU_500M_DEM, &box_feature, "airspace").await;
        render_at_anchor(
            &mut map,
            Matrix4::from_translation(Vector3::new(0.0, 0.0, 2800.0)),
            ANCHOR,
        );
        let green_count = read_blocking(&map)
            .as_chunks::<4>()
            .0
            .iter()
            .filter(|p| p[1] > 100)
            .count();
        assert_eq!(
            green_count > 50,
            expect_visible,
            "datum={datum} over 500m terrain produced {green_count} green pixels"
        );
    }
}

#[tokio::test]
async fn elevated_dashed_rim_renders_above_terrain_from_above_pitched_and_below() {
    for (alt_m, expect_visible) in [(200.0, false), (1200.0, true)] {
        let style: Style = serde_json::from_value(serde_json::json!({
            "version": 8,
            "projection": {"type": "globe"},
            "terrain": {"source": "dem"},
            "sources": {
                "airspace": {"type": "vector", "tiles": ["https://airspace.example/{z}/{x}/{y}.pbf"]},
                "dem": {"type": "raster-dem", "tiles": ["https://dem.example/{z}/{x}/{y}.png"], "encoding": "terrarium"}
            },
            "layers": [
                {"id": "bg", "type": "background", "paint": {"background-color": "#770000"}},
                {
                    "id": "rim",
                    "type": "line",
                    "source": "airspace",
                    "source-layer": "airspace",
                    "paint": {
                        "line-color": "#00ff44",
                        "line-width": 4,
                        "line-dasharray": [2, 2],
                        "line-elevation": ["get", "alt_m"],
                        "line-elevation-datum": "msl"
                    }
                }
            ]
        }))
        .expect("style");
        let line = serde_json::json!({
            "type": "Feature",
            "properties": {"alt_m": alt_m},
            "geometry": {"type": "LineString", "coordinates": [[0.015, 0.044], [0.073, 0.044]]}
        });
        let mut map = build_globe_map(style, PLATEAU_500M_DEM, &line, "airspace").await;

        // Top-down view at 2500 m.
        render_at_anchor(
            &mut map,
            Matrix4::from_translation(Vector3::new(0.0, 0.0, 2500.0)),
            ANCHOR,
        );
        let pixels = read_blocking(&map);
        let green_count = pixels
            .as_chunks::<4>()
            .0
            .iter()
            .filter(|p| p[1] > 140 && p[0] < 90)
            .count();
        assert_eq!(
            green_count > 20,
            expect_visible,
            "alt_m={alt_m} top-down green pixels={green_count}"
        );
        if !expect_visible {
            continue;
        }

        // Verify dash gaps exist along row y=32.
        let mut dash_runs = 0;
        let mut in_dash = false;
        for x in 0..64 {
            let p = &pixels[(32 * 64 + x) * 4..][..4];
            let green = p[1] > 140 && p[0] < 90;
            if green && !in_dash {
                dash_runs += 1;
            }
            in_dash = green;
        }
        assert!(
            dash_runs >= 2,
            "expected dashed rim gaps, got {dash_runs} runs"
        );

        // Pitched view from south (verifying no z=0 ground stencil clipping).
        let pitched = Matrix4::from_translation(Vector3::new(0.0, -1400.0, 1900.0))
            * Matrix4::from_angle_x(Rad(0.75));
        render_at_anchor(&mut map, pitched, ANCHOR);
        let pitched_green = read_blocking(&map)
            .as_chunks::<4>()
            .0
            .iter()
            .filter(|p| p[1] > 140 && p[0] < 90)
            .count();
        assert!(
            pitched_green > 15,
            "pitched elevated rim disappeared: {pitched_green}"
        );

        // Upward view (35° above horizon) from z=750 m (above 500 m terrain, below 1200 m rim).
        let below = Matrix4::from_translation(Vector3::new(0.0, -640.0, 750.0))
            * Matrix4::from_angle_x(Rad(std::f64::consts::FRAC_PI_2 + 0.61));
        render_at_anchor(&mut map, below, ANCHOR);
        let below_green = read_blocking(&map)
            .as_chunks::<4>()
            .0
            .iter()
            .filter(|p| p[1] > 140)
            .count();
        assert!(
            below_green > 10,
            "elevated rim vanished when viewed from below: {below_green}"
        );
    }
}
