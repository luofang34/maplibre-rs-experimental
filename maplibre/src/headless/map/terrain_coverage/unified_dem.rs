//! Unified CPU/GPU DEM pipeline verification across terrain, hillshade, and colour relief.

use super::*;
use crate::{
    render::tile_memory::tile_bytes,
    terrain::{DemLayersComponent, DemTileComponent, GpuDemPool},
};

fn shared_dem_style() -> Style {
    let coords = target();
    let n = 2_f64.powi(i32::from(u8::from(coords.z)));
    let lon = (f64::from(coords.x) + 0.5) / n * 360.0 - 180.0;
    let lat = (std::f64::consts::PI * (1.0 - 2.0 * (f64::from(coords.y) + 0.5) / n))
        .sinh()
        .atan()
        .to_degrees();
    serde_json::from_value(serde_json::json!({
        "version": 8,
        "center": [lon, lat],
        "zoom": 12.125,
        "terrain": { "source": "dem", "exaggeration": 1.0 },
        "sources": {
            "dem": {
                "type": "raster-dem",
                "tiles": ["offline://dem/{z}/{x}/{y}.png"],
                "tileSize": 256,
                "maxzoom": 14,
                "encoding": "terrarium"
            }
        },
        "layers": [
            { "id": "background", "type": "background", "paint": { "background-color": "#101010" } },
            {
                "id": "relief",
                "type": "color-relief",
                "source": "dem",
                "paint": {
                    "color-relief-opacity": 1.0,
                    "color-relief-color": [
                        "interpolate", ["linear"], ["elevation"],
                        0, "#00aa00",
                        2000, "#0000ff"
                    ]
                }
            },
            {
                "id": "shade",
                "type": "hillshade",
                "source": "dem",
                "paint": { "hillshade-exaggeration": 0.8 }
            }
        ]
    }))
    .expect("shared DEM style")
}

fn terrarium_slope(offset: u32, base_metres: f64) -> RgbaImage {
    RgbaImage::from_fn(256, 256, |x, _| {
        let metres = base_metres + f64::from(x + offset) * 4.0;
        let encoded = metres + 32768.0;
        let r = (encoded / 256.0).floor();
        let g = (encoded - r * 256.0).floor();
        let b = ((encoded - r * 256.0 - g) * 256.0).floor();
        Rgba([r as u8, g as u8, b as u8, 255])
    })
}

async fn shared_dem_map() -> HeadlessMap {
    let (kernel, renderer) = create_headless_renderer(SIZE, SIZE, None)
        .await
        .expect("renderer");
    HeadlessMap::new(
        shared_dem_style(),
        renderer,
        kernel,
        vec![
            Box::new(RenderPlugin),
            Box::new(crate::background::BackgroundPlugin),
            Box::new(RasterPlugin::<DefaultRasterTransferables>::default()),
            Box::new(HillshadePlugin),
            Box::new(TerrainPlugin::<DefaultDemTransferables>::default()),
            Box::new(
                HeadlessPlugin::new(false)
                    .preserve_tile_sources()
                    .retain_supplied_tiles(),
            ),
        ],
    )
    .expect("map")
}

#[test]
fn shared_dem_source_allocates_and_uploads_one_gpu_texture_for_terrain_relief_and_hillshade() {
    crate::headless::map::test_async::run_async(async {
        let mut map = shared_dem_map().await;
        let west = target();

        // Load DEM once via `load_dem_tiles` without duplicating `AvailableRasterLayerData`.
        map.render_frames_with_terrain(
            ProcessedLayers::default(),
            Vec::new(),
            vec![(west, terrarium_slope(0, 500.0))],
            3,
        )
        .expect("shared DEM frame");

        let world = map.world();
        let pool = world.resources.get::<GpuDemPool>().expect("GpuDemPool");
        assert_eq!(
            pool.texture_count(),
            1,
            "terrain, hillshade, and color-relief share one GPU DEM texture"
        );
        assert_eq!(
            pool.upload_count(),
            1,
            "shared DEM tile is uploaded to GPU once"
        );
        assert_eq!(pool.texture_bytes(), 260 * 260 * 4);

        // CPU memory deduplicates `Arc<DemTile>` across `DemTileComponent` and `DemLayersComponent`.
        let Some(DemTileComponent::Loaded(terrain_dem)) =
            world.tiles.query::<&DemTileComponent>(west)
        else {
            panic!("terrain DEM loaded");
        };
        let layer_dem = world
            .tiles
            .query::<&DemLayersComponent>(west)
            .and_then(|layers| layers.get(&"dem".into()))
            .expect("layer DEM loaded");
        assert!(
            std::sync::Arc::ptr_eq(&terrain_dem.tile, &layer_dem.tile),
            "terrain and raster-dem layers share the exact same Arc<DemTile>"
        );
        assert_eq!(tile_bytes(&world.tiles, west), 260 * 260 * 4);

        // Verify rendered output is shaded colour relief rather than flat background (#101010).
        let pixels = read_blocking(&map, "shared_dem_terrain_relief_hillshade");
        let center_offset = ((SIZE / 2 * SIZE + SIZE / 2) * 4) as usize;
        let center = &pixels[center_offset..center_offset + 4];
        assert!(
            center[1] > 40 || center[2] > 40,
            "shared DEM drew shaded colour relief over terrain: {center:?}"
        );
    })
}

#[test]
fn late_neighbour_and_reload_update_shared_dem_in_place_without_extra_textures() {
    crate::headless::map::test_async::run_async(async {
        let mut map = shared_dem_map().await;
        let west = target();
        let east = WorldTileCoords::from((west.x + 1, west.y, west.z));

        map.render_frames_with_terrain(
            ProcessedLayers::default(),
            vec![AvailableRasterLayerData {
                coords: west,
                source: "dem".into(),
                image: terrarium_slope(0, 400.0),
            }],
            Vec::new(),
            2,
        )
        .expect("west frame");

        {
            let pool = map.world().resources.get::<GpuDemPool>().expect("pool");
            assert_eq!(pool.texture_count(), 1);
            assert_eq!(pool.upload_count(), 1);
        }

        // Late east neighbour arrives: backfills `west`'s 2-texel east border and updates `west` in place.
        map.render_frames_with_terrain(
            ProcessedLayers::default(),
            vec![AvailableRasterLayerData {
                coords: east,
                source: "dem".into(),
                image: terrarium_slope(256, 400.0),
            }],
            Vec::new(),
            2,
        )
        .expect("east frame");

        {
            let pool = map.world().resources.get::<GpuDemPool>().expect("pool");
            assert_eq!(
                pool.texture_count(),
                2,
                "west was updated in place; only west + east exist"
            );
            assert_eq!(
                pool.upload_count(),
                3,
                "east uploaded once + west border refreshed once in place"
            );
        }

        // Reloading `west` at a new elevation reuses both its `GpuDemPool` texture and `BindGroup` in place.
        let before_reload_rev = map
            .world()
            .resources
            .get::<crate::render::eventually::Eventually<crate::raster::resource::RasterResources>>(
            )
            .and_then(|r| match r {
                crate::render::eventually::Eventually::Initialized(res) => Some(res.revision()),
                _ => None,
            })
            .expect("raster resources");

        map.render_frames_with_terrain(
            ProcessedLayers::default(),
            vec![AvailableRasterLayerData {
                coords: west,
                source: "dem".into(),
                image: terrarium_slope(0, 1200.0),
            }],
            Vec::new(),
            2,
        )
        .expect("reload west");

        let pool = map.world().resources.get::<GpuDemPool>().expect("pool");
        assert_eq!(
            pool.texture_count(),
            2,
            "reloading west reuses its existing 260x260 GPU texture"
        );
        let after_reload_rev = map
            .world()
            .resources
            .get::<crate::render::eventually::Eventually<crate::raster::resource::RasterResources>>(
            )
            .and_then(|r| match r {
                crate::render::eventually::Eventually::Initialized(res) => Some(res.revision()),
                _ => None,
            })
            .expect("raster resources");
        assert!(
            after_reload_rev > before_reload_rev,
            "in-place DEM update advances RasterResources revision so cached drapes refresh"
        );
    })
}
