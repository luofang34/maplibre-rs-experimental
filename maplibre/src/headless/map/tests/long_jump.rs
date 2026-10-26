//! A jump across the world over terrain: the camera never takes the ground it left along, so
//! it does not end up underground where the new ground has not loaded.

use image::{Rgba, RgbaImage};

use crate::{
    coords::{LatLon, WorldTileCoords},
    headless::{create_headless_renderer, map::HeadlessMap, HeadlessPlugin},
    render::{
        view_state::{CameraOptions, NavigationMode},
        RenderPlugin,
    },
    terrain::{interaction::camera_ground_position, DefaultDemTransferables, TerrainPlugin},
};

const OCEAN: LatLon = LatLon {
    latitude: -30.0,
    longitude: -20.0,
};
const ORYOL: LatLon = LatLon {
    latitude: 52.97,
    longitude: 36.06,
};

/// A Terrarium DEM tile standing flat at `meters`.
fn flat_dem(meters: f64) -> RgbaImage {
    let value = meters + 32768.0;
    let red = (value / 256.0).floor();
    RgbaImage::from_pixel(
        256,
        256,
        Rgba([red as u8, (value - red * 256.0) as u8, 0, 255]),
    )
}

fn tile_at(position: LatLon, zoom: u8) -> WorldTileCoords {
    let n = 2_f64.powi(i32::from(zoom));
    let latitude = position.latitude.to_radians();
    let x = (position.longitude + 180.0) / 360.0 * n;
    let y = (1.0 - (latitude.tan() + 1.0 / latitude.cos()).ln() / std::f64::consts::PI) / 2.0 * n;
    WorldTileCoords::from((x.floor() as i32, y.floor() as i32, zoom.into()))
}

async fn ocean_map() -> HeadlessMap {
    let style = serde_json::from_value(serde_json::json!({
        "version":8,"center":[OCEAN.longitude, OCEAN.latitude],"zoom":9,"pitch":50,
        "projection":{"type":"vertical-perspective"},"terrain":{"source":"dem"},
        "sources":{"dem":{"type":"raster-dem","tiles":["offline://dem"],
            "tileSize":256,"maxzoom":14,"encoding":"terrarium"}},
        "layers":[]
    }))
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
            Box::new(TerrainPlugin::<DefaultDemTransferables>::default()),
            Box::new(HeadlessPlugin::new(false)),
        ],
    )
    .expect("map");
    map.set_navigation_mode(NavigationMode::FreeGlobe)
        .expect("free navigation");
    map.load_dem_tiles(vec![(tile_at(OCEAN, 6), flat_dem(-5468.0))])
        .expect("ocean floor");
    map.run_frame().expect("frame");
    map
}

fn eye_altitude(map: &HeadlessMap) -> f64 {
    camera_ground_position(map.view_state()).1
}

#[tokio::test]
async fn a_long_jump_stands_at_sea_level_until_the_new_ground_loads() {
    let mut map = ocean_map().await;
    assert!(
        (map.view_state().center_elevation() + 5468.0).abs() < 1.0,
        "the center stands on the ocean floor: {}",
        map.view_state().center_elevation()
    );
    map.jump_to(CameraOptions {
        center: Some(ORYOL),
        zoom: Some(15.0),
        pitch: Some(cgmath::Deg(55.0)),
        ..Default::default()
    })
    .expect("jump");
    for _ in 0..3 {
        assert_eq!(map.view_state().center_elevation(), 0.0);
        assert!(eye_altitude(&map) > 0.0, "eye at {}", eye_altitude(&map));
        map.run_frame().expect("frame");
    }
    map.load_dem_tiles(vec![(tile_at(ORYOL, 10), flat_dem(180.0))])
        .expect("Oryol ground");
    assert!(
        (map.view_state().center_elevation() - 180.0).abs() < 1.0,
        "the center stands on the loaded ground: {}",
        map.view_state().center_elevation()
    );
    assert!(eye_altitude(&map) > 180.0, "eye at {}", eye_altitude(&map));
}
