#![allow(clippy::expect_used, clippy::panic)]

use std::collections::HashSet;

use cgmath::{Deg, Matrix3, Matrix4, Rad};
use image::{Rgba, RgbaImage};

use super::*;
use crate::{
    background::BackgroundPlugin,
    coords::{LatLon, WorldTileCoords, ZoomLevel},
    headless::{create_headless_renderer_with_settings, HeadlessPlugin},
    render::{
        camera::EyeFrustum,
        eventually::{Eventually, Eventually::Initialized},
        frame_input::ViewSource,
        settings::{
            BufferPoolSizes, Msaa, ProceduralReliefStyle, RendererSettings, TerrainDrapeMode,
        },
        view_state::{ExternalAnchor, ExternalView},
        RenderPlugin,
    },
    sdf::SymbolBufferPool,
    style::Style,
    terrain::{
        resources::{TerrainResources, UNIFORM_STRIDE},
        transferables::DefaultDemTransferables,
        DrapePhase, TerrainPlugin,
    },
    vector::VectorBufferPool,
};

fn encode_terrarium(elevation_m: f64) -> Rgba<u8> {
    let shifted = (elevation_m + 32_768.0).clamp(0.0, 65_535.99);
    let r = (shifted / 256.0).floor() as u8;
    let rem = shifted - f64::from(r) * 256.0;
    let g = rem.floor() as u8;
    let b = ((rem - f64::from(g)) * 256.0).floor() as u8;
    Rgba([r, g, b, 255])
}

fn synthetic_ridge_dem(size: u32, base_m: f64, peak_m: f64) -> RgbaImage {
    RgbaImage::from_fn(size, size, |x, y| {
        let nx = f64::from(x) / f64::from(size.saturating_sub(1).max(1));
        let ny = f64::from(y) / f64::from(size.saturating_sub(1).max(1));
        let ridge = (1.0 - (nx - 0.5).abs() * 2.0).max(0.0);
        let undulation = (ny * std::f64::consts::TAU).sin() * 180.0;
        let elev = (base_m + ridge * (peak_m - base_m) + undulation).max(-420.0);
        encode_terrarium(elev)
    })
}

fn svs_terrain_style() -> Style {
    serde_json::from_value(serde_json::json!({
        "version": 8,
        "projection": { "type": "vertical-perspective" },
        "center": [8.0, 47.0],
        "zoom": 7.0,
        "sources": {
            "dem": {
                "type": "raster-dem",
                "tiles": ["https://dem.invalid/{z}/{x}/{y}.png"],
                "tileSize": 256,
                "minzoom": 0,
                "maxzoom": 7,
                "encoding": "terrarium"
            }
        },
        "terrain": { "source": "dem", "exaggeration": 1.0 },
        "layers": [
            { "id": "bg", "type": "background", "paint": { "background-color": "#05080c" } }
        ]
    }))
    .expect("valid SVS terrain style")
}

fn svs_external_view(lat: f64, lon: f64, alt_m: f64, pitch_deg: f64, aspect: f64) -> ExternalView {
    let base_from_local = Matrix3::new(1.0, 0.0, 0.0, 0.0, 0.0, -1.0, 0.0, 1.0, 0.0);
    let pitch = Matrix3::from_angle_x(Deg(-pitch_deg));
    ExternalView {
        anchor: ExternalAnchor {
            position: LatLon::new(lat, lon),
            altitude_meters: alt_m,
        },
        view: Matrix4::from(pitch * base_from_local),
        frustum: EyeFrustum::symmetric(Rad(0.75), aspect, 1.0, 220_000.0),
    }
}

fn count_red_warning_pixels(rgba: &[u8]) -> usize {
    rgba.as_chunks::<4>()
        .0
        .iter()
        .filter(|px| {
            px[0] > 175 && px[0] > px[1].saturating_add(55) && px[0] > px[2].saturating_add(80)
        })
        .count()
}

#[tokio::test]
async fn procedural_relief_svs_renders_with_zero_drape_allocations_and_taws_bands() {
    let settings = RendererSettings {
        msaa: Msaa { samples: 1 },
        buffer_pools: BufferPoolSizes::EMPTY,
        symbol_pools: BufferPoolSizes::EMPTY,
        terrain_drape_mode: TerrainDrapeMode::ProceduralRelief(ProceduralReliefStyle::default()),
        ..RendererSettings::default()
    };
    let (kernel, renderer) = create_headless_renderer_with_settings(960, 540, None, settings)
        .await
        .expect("960x540 SVS headless renderer");
    let mut map = HeadlessMap::new(
        svs_terrain_style(),
        renderer,
        kernel,
        vec![
            Box::new(RenderPlugin),
            Box::new(BackgroundPlugin),
            Box::new(TerrainPlugin::<DefaultDemTransferables>::default()),
            Box::new(HeadlessPlugin::new(false).retain_supplied_tiles()),
        ],
    )
    .expect("SVS headless map");

    let mut dem_tiles = Vec::new();
    for z in 0..=6_u8 {
        let side = 1_i32 << z;
        let cx = ((8.0 + 180.0) / 360.0 * f64::from(side)).floor() as i32;
        let lat_rad = 47.0_f64.to_radians();
        let merc_y =
            (1.0 - (lat_rad.tan() + 1.0 / lat_rad.cos()).ln() / std::f64::consts::PI) * 0.5;
        let cy = (merc_y * f64::from(side)).floor() as i32;
        for dy in -1..=1 {
            for dx in -1..=1 {
                let x = (cx + dx).rem_euclid(side);
                let y = (cy + dy).clamp(0, side - 1);
                dem_tiles.push((
                    WorldTileCoords {
                        x,
                        y,
                        z: ZoomLevel::new(z),
                    },
                    synthetic_ridge_dem(64, 300.0, 2_750.0),
                ));
            }
        }
    }
    map.load_dem_tiles(dem_tiles)
        .expect("load synthetic ridge DEM");
    map.frame_input_mut().view =
        ViewSource::External(svs_external_view(46.88, 8.0, 3_100.0, -18.0, 960.0 / 540.0));
    map.run_frame().expect("first SVS frame");
    map.run_frame().expect("settled SVS frame");

    let drape_phase = map
        .world()
        .resources
        .get::<DrapePhase>()
        .expect("DrapePhase resource");
    assert!(
        drape_phase.targets.is_empty(),
        "ProceduralRelief must schedule 0 DrapePhase passes"
    );

    let Some(Initialized(terrain)) = map.world().resources.get::<Eventually<TerrainResources>>()
    else {
        panic!("TerrainResources must be initialized");
    };
    assert!(
        terrain.scratch().is_none(),
        "DrapeScratch must not be allocated"
    );
    assert_eq!(
        terrain.drape_texture_total(),
        0,
        "DrapeCache must hold 0 textures"
    );
    assert_eq!(terrain.drape_size(), 0);
    assert!(
        !terrain.draws().is_empty(),
        "3D terrain tiles must be drawn in MAIN_PASS"
    );

    let (drape_bytes, dem_bytes) = terrain.texture_bytes();
    assert_eq!(drape_bytes, 0);
    let terrain_gpu_bytes = terrain.vertex_buffer().size() as usize
        + terrain.index_buffer().size() as usize
        + (UNIFORM_STRIDE * 1024) as usize
        + drape_bytes
        + dem_bytes;
    let vec_bytes = match map.world().resources.get::<Eventually<VectorBufferPool>>() {
        Some(Initialized(p)) => p.device_buffer_bytes(),
        _ => 0,
    } as usize;
    let sym_bytes = match map.world().resources.get::<Eventually<SymbolBufferPool>>() {
        Some(Initialized(p)) => p.device_buffer_bytes(),
        _ => 0,
    } as usize;
    let color_bytes = 960 * 540 * 4;
    let depth_bytes = 960 * 540 * 8;
    let total_footprint = terrain_gpu_bytes + vec_bytes + sym_bytes + color_bytes + depth_bytes;
    assert!(
        total_footprint < 40 * 1024 * 1024,
        "960x540 SVS GPU footprint ({total_footprint} B) must be < 40 MiB"
    );

    let rgba_no_taws = crate::headless::map::readback::read_blocking(
        &map,
        map.color_texture().expect("color texture"),
        wgpu::TextureAspect::All,
    )
    .expect("read SVS frame");
    let unique_colors: HashSet<[u8; 3]> = rgba_no_taws
        .as_chunks::<4>()
        .0
        .iter()
        .step_by(17)
        .map(|px| [px[0], px[1], px[2]])
        .collect();
    assert!(
        unique_colors.len() > 24,
        "ProceduralRelief must produce shaded hypsometric + gridline variation (got {} colors)",
        unique_colors.len()
    );
    let red_before = count_red_warning_pixels(&rgba_no_taws);

    // Lower ownship TAWS reference altitude to 1,900 m MSL so ridge peaks (> 1,900 m) trigger TAWS red.
    map.set_procedural_relief_reference_altitude(Some(1_900.0));
    map.run_frame().expect("TAWS SVS frame");
    let rgba_taws = crate::headless::map::readback::read_blocking(
        &map,
        map.color_texture().expect("color texture"),
        wgpu::TextureAspect::All,
    )
    .expect("read TAWS frame");
    let red_after = count_red_warning_pixels(&rgba_taws);
    assert!(
        red_after > red_before + 500,
        "TAWS reference altitude must highlight terrain peaks in red (before={red_before}, after={red_after})"
    );
}

#[tokio::test]
async fn resident_terrain_patch_exports_stitched_129x129_grid_and_drape_handle() {
    let settings = RendererSettings {
        msaa: Msaa { samples: 1 },
        buffer_pools: BufferPoolSizes::EMPTY,
        symbol_pools: BufferPoolSizes::EMPTY,
        terrain_drape_mode: TerrainDrapeMode::ProceduralRelief(ProceduralReliefStyle::default()),
        ..RendererSettings::default()
    };
    let (kernel, renderer) = create_headless_renderer_with_settings(256, 256, None, settings)
        .await
        .expect("headless renderer");
    let mut map = HeadlessMap::new(
        svs_terrain_style(),
        renderer,
        kernel,
        vec![
            Box::new(RenderPlugin),
            Box::new(BackgroundPlugin),
            Box::new(TerrainPlugin::<DefaultDemTransferables>::default()),
            Box::new(HeadlessPlugin::new(false).retain_supplied_tiles()),
        ],
    )
    .expect("headless map");

    let mut dem_tiles = Vec::new();
    for z in 0..=6_u8 {
        let side = 1_i32 << z;
        for y in 0..side.min(32) {
            for x in 0..side.min(40) {
                if z <= 3 || ((30..=36).contains(&x) && (20..=25).contains(&y)) {
                    dem_tiles.push((
                        WorldTileCoords {
                            x,
                            y,
                            z: ZoomLevel::new(z),
                        },
                        synthetic_ridge_dem(32, 200.0, 1_800.0),
                    ));
                }
            }
        }
    }
    map.load_dem_tiles(dem_tiles).expect("load DEM");
    map.frame_input_mut().view =
        ViewSource::External(svs_external_view(47.0, 8.0, 4_500.0, -35.0, 1.0));
    map.run_frame().expect("procedural frame");

    let drawn_coords: Vec<WorldTileCoords> = {
        let Some(Initialized(terrain)) =
            map.world().resources.get::<Eventually<TerrainResources>>()
        else {
            panic!("terrain initialized");
        };
        terrain.draws().iter().map(|d| d.coords).collect()
    };
    assert!(!drawn_coords.is_empty());
    let first = drawn_coords[0];
    let patch = map
        .resident_terrain_patch(first)
        .expect("resident terrain patch");
    assert_eq!(patch.grid_side, 129);
    assert_eq!(patch.stitched_heights_meters.len(), 129 * 129);
    assert!(patch.stitched_to_neighbors);
    assert!(patch.drape_texture.is_none());
    assert!(patch.max_elevation_meters > patch.min_elevation_meters);

    // Check shared East-West seam continuity across any adjacent rendered pair at the same zoom.
    let drawn_set: HashSet<WorldTileCoords> = drawn_coords.iter().copied().collect();
    if let Some(&west) = drawn_coords.iter().find(|c| {
        drawn_set.contains(&WorldTileCoords {
            x: c.x + 1,
            y: c.y,
            z: c.z,
        })
    }) {
        let east = WorldTileCoords {
            x: west.x + 1,
            y: west.y,
            z: west.z,
        };
        let w_patch = map.resident_terrain_patch(west).expect("west patch");
        let e_patch = map.resident_terrain_patch(east).expect("east patch");
        for row in 0..129 {
            let wh = w_patch.height_at_grid(128, row).expect("west border");
            let eh = e_patch.height_at_grid(0, row).expect("east border");
            assert!(
                (wh - eh).abs() <= 1e-2,
                "stitched seam mismatch at row {row}: west={wh}, east={eh}"
            );
        }
    }

    // Switching back to Draped mode populates `drape_texture` on resident patches.
    map.set_terrain_drape_mode(TerrainDrapeMode::Draped { edge_px: 256 });
    map.run_frame().expect("draped frame");
    let draped_patch = map
        .resident_terrain_patch(first)
        .expect("draped terrain patch");
    assert!(
        draped_patch.drape_texture.is_some(),
        "Draped mode must expose the resident mipmapped drape texture handle"
    );
}
