#![allow(clippy::expect_used, clippy::panic)]
use crate::{
    coords::{LatLon, WorldTileCoords},
    headless::{create_headless_renderer, map::HeadlessMap},
    render::{
        camera::EyeFrustum,
        view_state::ExternalAnchor,
        xr::{EyeTarget, ScenePlacement, XrEye, XrFrame},
        RenderPlugin,
    },
    style::Style,
};
use cgmath::{Matrix4, Rad, SquareMatrix, Vector3};
use std::time::Duration;

#[tokio::test]
async fn elevated_bridge_draws_over_terrain_and_buried_tunnel_is_occluded() {
    for (kind, elevation, visible) in [("bridge", "1000", true), ("tunnel", "-1000", false)] {
        let mut map = structure_map(kind, elevation).await;
        render_at(
            &mut map,
            Matrix4::from_translation(Vector3::new(0.0, 0.0, 4000.0)),
        );
        let pixels = read_blocking(&map);
        let green = pixels
            .chunks_exact(4)
            .filter(|p| p[1] > 180 && p[0] < 80 && p[2] < 80)
            .count();
        assert_eq!(
            green > 20,
            visible,
            "{kind} elevation {elevation} produced {green} green pixels"
        );
    }
}

fn render_at(map: &mut HeadlessMap, camera: Matrix4<f64>) {
    render_at_anchor(map, camera, LatLon::new(0.0, 0.0));
}

fn render_at_anchor(map: &mut HeadlessMap, camera: Matrix4<f64>, anchor: LatLon) {
    for timestamp in [0, 16, 32] {
        map.run_xr_frame(XrFrame {
            opaque_environment: true,
            timestamp: Duration::from_millis(timestamp),
            placement: ScenePlacement {
                anchor: ExternalAnchor {
                    position: anchor,
                    altitude_meters: 0.0,
                },
                world_from_scene: Matrix4::identity(),
            },
            eyes: vec![XrEye {
                world_from_eye: camera,
                frustum: EyeFrustum::symmetric(Rad(1.0), 1.0, 0.05, 1e9),
                target: EyeTarget::default(),
            }],
            request_overscan: 1.0,
            prefetch: None,
        })
        .expect("structure frame");
    }
}

#[tokio::test]
async fn bridge_width_foreshortens_with_the_terrain_in_perspective() {
    let lines = serde_json::json!({"type":"Feature", "properties":{}, "geometry":{
        "type":"MultiLineString", "coordinates":[[[-0.1,0],[0.1,0]],[[-0.1,0.036],[0.1,0.036]]]
    }});
    let mut map = map_with_lines("bridge", "10", lines).await;
    let camera = Matrix4::from_translation(Vector3::new(0.0, -2000.0, 4000.0))
        * Matrix4::from_angle_x(Rad(std::f64::consts::FRAC_PI_4));
    render_at(&mut map, camera);
    let pixels = read_blocking(&map);
    let mut runs = Vec::<usize>::new();
    let mut length = 0;
    for y in 0..64 {
        let pixel = &pixels[(y * 64 + 32) * 4..][..4];
        if pixel[1] > 180 && pixel[0] < 80 && pixel[2] < 80 {
            length += 1;
        } else if length > 0 {
            runs.push(length);
            length = 0;
        }
    }
    if length > 0 {
        runs.push(length);
    }
    assert_eq!(runs.len(), 2, "two visible decks: {runs:?}");
    assert!(
        runs[1] > runs[0],
        "near deck must be wider than far deck: {runs:?}"
    );
}

#[tokio::test]
async fn parent_road_uses_finer_terrain_when_the_dem_refines() {
    let mut map = structure_map("bridge", "").await;
    let fine = (0..2)
        .map(|x| {
            (
                WorldTileCoords::from((x, 1, crate::coords::ZoomLevel::new(1))),
                image::RgbaImage::from_pixel(2, 2, image::Rgba([129, 244, 0, 255])),
            )
        })
        .collect();
    map.load_dem_tiles(fine).expect("refined 500 metre terrain");
    render_at(
        &mut map,
        Matrix4::from_translation(Vector3::new(0.0, 0.0, 4000.0)),
    );
    let green = read_blocking(&map)
        .chunks_exact(4)
        .filter(|p| p[1] > 180 && p[0] < 80 && p[2] < 80)
        .count();
    assert!(
        green > 20,
        "parent road must stay above the finer terrain: {green}"
    );
}

async fn structure_map(kind: &str, elevation: &str) -> HeadlessMap {
    map_with_lines(
        kind,
        elevation,
        serde_json::json!({"type":"Feature","properties":{},
        "geometry":{"type":"LineString","coordinates":[[-0.1,0],[0.1,0]]}}),
    )
    .await
}

async fn map_with_lines(kind: &str, elevation: &str, line: serde_json::Value) -> HeadlessMap {
    map_with_tile(kind, elevation, line, WorldTileCoords::default()).await
}

async fn map_with_tile(
    kind: &str,
    elevation: &str,
    line: serde_json::Value,
    root: WorldTileCoords,
) -> HeadlessMap {
    let mut style = structure_style(kind, elevation);
    let count = 2_f64.powi(i32::from(u8::from(root.z)));
    style.center = Some([
        (f64::from(root.x) + 0.5) / count * 360.0 - 180.0,
        (std::f64::consts::PI * (1.0 - 2.0 * (f64::from(root.y) + 0.5) / count))
            .sinh()
            .atan()
            .to_degrees(),
    ]);
    style.zoom = Some(f64::from(u8::from(root.z)));
    let layers = style.layers[1..].to_vec();
    assert!(layers.iter().all(|layer| super::kind(layer).is_some()));
    let (kernel, renderer) = create_headless_renderer(64, 64, None)
        .await
        .expect("renderer");
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
    .expect("map");
    map.load_dem_tiles(vec![(
        root,
        image::RgbaImage::from_pixel(2, 2, image::Rgba([128, 0, 0, 255])),
    )])
    .expect("DEM");
    let layers = map
        .process_geojson(
            &line,
            "roads",
            layers,
            root,
            crate::projection::ProjectionType::Globe,
        )
        .expect("line");
    assert!(!layers.vector.is_empty(), "processed geometry");
    assert!(
        !layers.vector[0].buffer.buffer.vertices.is_empty(),
        "line vertices"
    );
    map.render_tile(layers).expect("upload line");
    assert!(
        map.world()
            .tiles
            .query::<&crate::vector::VectorLayerBucketComponent>(root)
            .is_some(),
        "root bucket"
    );
    map
}

fn read_blocking(map: &HeadlessMap) -> Vec<u8> {
    let buffer = map.device().create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: 64 * 64 * 4,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = map.device().create_command_encoder(&Default::default());
    let texture = map.head_texture().expect("color");
    encoder.copy_texture_to_buffer(
        texture.as_image_copy(),
        wgpu::ImageCopyBuffer {
            buffer: &buffer,
            layout: wgpu::ImageDataLayout {
                offset: 0,
                bytes_per_row: Some(256),
                rows_per_image: None,
            },
        },
        texture.size(),
    );
    map.queue().submit([encoder.finish()]);
    buffer
        .slice(..)
        .map_async(wgpu::MapMode::Read, |result| result.expect("readback"));
    map.device().poll(wgpu::Maintain::Wait);
    let bytes = buffer.slice(..).get_mapped_range().to_vec();
    buffer.unmap();
    bytes
}

#[tokio::test]
async fn coplanar_bridge_casing_cannot_hide_the_deck_during_head_motion() {
    let mut map = structure_map("bridge", "1000").await;
    for roll in [-0.002, 0.002, 0.0, 0.01, -0.01] {
        render_at(
            &mut map,
            Matrix4::from_translation(Vector3::new(0.0, 0.0, 4000.0))
                * Matrix4::from_angle_z(Rad(roll)),
        );
        let pixels = read_blocking(&map);
        let green = pixels
            .chunks_exact(4)
            .filter(|p| p[1] > 180 && p[0] < 80 && p[2] < 80)
            .count();
        assert!(
            green > 20,
            "deck disappeared beneath its coplanar casing: {green}"
        );
    }
}

fn structure_style(kind: &str, elevation: &str) -> Style {
    let mut style: Style = serde_json::from_value(serde_json::json!({
            "version": 8, "projection": {"type":"globe"}, "terrain":{"source":"dem"},
            "sources": {
                "roads": {"type":"vector", "tiles":["https://roads.example/{z}/{x}/{y}.pbf"]},
                "dem": {"type":"raster-dem", "tiles":["https://dem.example/{z}/{x}/{y}.png"], "encoding":"terrarium"}
            }, "layers": [
                {"id":"background","type":"background","paint":{"background-color":"#990000"}},
                {"id":"casing","type":"line","source":"roads","source-layer":"roads",
                 "metadata":{"maplibre-rs:terrain-structure":kind,"maplibre-rs:structure-elevation-meters":elevation},
                 "paint":{"line-color":"#000000","line-width":14}},
                {"id":"road","type":"line","source":"roads","source-layer":"roads",
                 "metadata":{"maplibre-rs:terrain-structure":kind,"maplibre-rs:structure-elevation-meters":elevation},
                 "paint":{"line-color":"#00ff00","line-width":8}}
            ]
        })).expect("style");
    for (index, layer) in style.layers.iter_mut().enumerate() {
        layer.index = index as u32;
    }
    style
}

#[tokio::test]
async fn detailed_bridge_deck_survives_pitch_and_roll_near_innsbruck() {
    let anchor = LatLon::new(47.26, 11.39);
    let zoom = crate::coords::ZoomLevel::new(14);
    let x = ((anchor.longitude / 360.0 + 0.5) * 16384.0).floor() as i32;
    let y = ((1.0 - anchor.latitude.to_radians().tan().asinh() / std::f64::consts::PI) * 8192.0)
        .floor() as i32;
    let line = serde_json::json!({"type":"Feature","properties":{},"geometry":{
        "type":"LineString","coordinates":[[11.385,47.26],[11.395,47.26]]}});
    let mut map = map_with_tile("bridge", "20", line, WorldTileCoords::from((x, y, zoom))).await;
    for pitch in [-0.25, 0.0, 0.25] {
        for roll in [-0.1, 0.0, 0.1] {
            let eye = Matrix4::from_translation(Vector3::new(0.0, 0.0, 500.0))
                * Matrix4::from_angle_x(Rad(pitch))
                * Matrix4::from_angle_z(Rad(roll));
            render_at_anchor(&mut map, eye, anchor);
            let pixels = read_blocking(&map);
            let green = pixels
                .chunks_exact(4)
                .filter(|p| p[1] > 160 && p[0] < 80 && p[2] < 80)
                .count();
            assert!(
                green > 12,
                "missing deck at pitch {pitch}, roll {roll}: {green}"
            );
        }
    }
}
