#![allow(clippy::expect_used, clippy::panic)]

use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Mutex},
    time::Duration,
};

use cgmath::{Deg, Matrix3, Matrix4, Rad};
use image::ImageEncoder;

use super::*;
use crate::{
    background::BackgroundPlugin,
    coords::{LatLon, WorldTileCoords, Zoom},
    headless::create_headless_renderer_with_loader,
    io::{
        resource_loader::SharedLoader,
        source_client::{HttpClient, SourceFetchError},
    },
    overlay::{OverlayMesh, OverlayPlugin, OverlayVertex},
    raster::{DefaultRasterTransferables, RasterPlugin},
    render::{
        camera::EyeFrustum,
        host_gpu::HostGpu,
        memory_budget::MemoryBudget,
        settings::{BufferPoolSizes, Msaa, RendererSettings},
        view_state::{ExternalAnchor, ExternalView},
        RenderPlugin,
    },
    style::Style,
    terrain::{transferables::DefaultDemTransferables, DemTileComponent, TerrainPlugin},
    window::PhysicalSize,
};

#[derive(Clone, Default)]
pub(super) struct CountingTileClient {
    fetches: Arc<Mutex<HashMap<String, usize>>>,
}

impl CountingTileClient {
    pub(super) fn total_fetches(&self) -> usize {
        self.fetches.lock().expect("lock").values().sum()
    }

    pub(super) fn max_fetches_for_any_url(&self) -> usize {
        self.fetches
            .lock()
            .expect("lock")
            .values()
            .copied()
            .max()
            .unwrap_or(0)
    }
}

#[cfg_attr(not(feature = "thread-safe-futures"), async_trait::async_trait(?Send))]
#[cfg_attr(feature = "thread-safe-futures", async_trait::async_trait)]
impl HttpClient for CountingTileClient {
    async fn fetch(&self, url: &str) -> Result<Vec<u8>, SourceFetchError> {
        *self
            .fetches
            .lock()
            .expect("lock")
            .entry(url.to_owned())
            .or_insert(0) += 1;

        let rgba = if url.contains("dem.invalid") {
            // Terrarium encoding: (R * 256 + G + B / 256) - 32768 -> ~492 m MSL
            [129, 236, 0, 255]
        } else {
            [40, 120, 200, 255]
        };
        let mut pixels = Vec::with_capacity(16 * 16 * 4);
        for _ in 0..(16 * 16) {
            pixels.extend_from_slice(&rgba);
        }
        let mut bytes = Vec::new();
        image::codecs::png::PngEncoder::new(&mut bytes)
            .write_image(&pixels, 16, 16, image::ExtendedColorType::Rgba8)
            .expect("encode PNG");
        Ok(bytes)
    }
}

pub(super) async fn host_gpu() -> Option<HostGpu> {
    let instance = wgpu::Instance::default();
    let adapter = instance
        .request_adapter(&wgpu::RequestAdapterOptions::default())
        .await
        .ok()?;
    let (device, queue) = adapter
        .request_device(&wgpu::DeviceDescriptor::default())
        .await
        .ok()?;
    Some(HostGpu {
        instance,
        adapter,
        device,
        queue,
    })
}

fn external_view_at(lat: f64, lon: f64, alt_m: f64, pitch_deg: f64, aspect: f64) -> ExternalView {
    let base_from_local = Matrix3::new(1.0, 0.0, 0.0, 0.0, 0.0, -1.0, 0.0, 1.0, 0.0);
    let pitch = Matrix3::from_angle_x(Deg(-pitch_deg));
    ExternalView {
        anchor: ExternalAnchor {
            position: LatLon::new(lat, lon),
            altitude_meters: alt_m,
        },
        view: Matrix4::from(pitch * base_from_local),
        frustum: EyeFrustum::symmetric(Rad(0.8), aspect, 1.0, 250_000.0),
    }
}

pub(super) fn terrain_raster_style(bg_hex: &str) -> Style {
    serde_json::from_value(serde_json::json!({
        "version": 8,
        "projection": { "type": "vertical-perspective" },
        "center": [8.0, 47.0],
        "zoom": 6.0,
        "sources": {
            "dem": {
                "type": "raster-dem",
                "tiles": ["https://dem.invalid/{z}/{x}/{y}.png"],
                "tileSize": 256,
                "maxzoom": 8,
                "encoding": "terrarium"
            },
            "imagery": {
                "type": "raster",
                "tiles": ["https://imagery.invalid/{z}/{x}/{y}.png"],
                "tileSize": 256,
                "maxzoom": 8
            },
            "tracks": {
                "type": "geojson",
                "data": { "type": "FeatureCollection", "features": [] }
            }
        },
        "terrain": { "source": "dem", "exaggeration": 1.0 },
        "layers": [
            { "id": "bg", "type": "background", "paint": { "background-color": bg_hex } },
            { "id": "imagery", "type": "raster", "source": "imagery" },
            {
                "id": "track-line",
                "type": "line",
                "source": "tracks",
                "paint": { "line-color": "#ffaa00", "line-width": 3.0 }
            }
        ]
    }))
    .expect("valid terrain style")
}

pub(super) fn fullscreen_overlay(width: u32, height: u32, color: [f32; 4]) -> OverlayMesh {
    let w = width as f32;
    let h = height as f32;
    OverlayMesh {
        vertices: vec![
            OverlayVertex {
                position: [0.0, 0.0],
                depth: 1.0,
                color,
            },
            OverlayVertex {
                position: [w, 0.0],
                depth: 1.0,
                color,
            },
            OverlayVertex {
                position: [w, h],
                depth: 1.0,
                color,
            },
            OverlayVertex {
                position: [0.0, h],
                depth: 1.0,
                color,
            },
        ],
        indices: vec![0, 1, 2, 0, 2, 3],
    }
}

pub(super) fn loaded_dem_coords(map: &HeadlessMap) -> HashSet<WorldTileCoords> {
    let tiles = &map.world().tiles;
    tiles
        .tiles
        .values()
        .map(|tile| tile.coords)
        .filter(|&coords| {
            matches!(
                tiles.query::<&DemTileComponent>(coords),
                Some(DemTileComponent::Loaded(_))
            )
        })
        .collect()
}

#[test]
fn heterogeneous_multi_view_renders_otw_and_sensor_without_skipped_slot_eviction() {
    crate::headless::map::test_async::run_async(async {
        let client = CountingTileClient::default();
        let loader = SharedLoader::new(client);
        let settings = RendererSettings {
            msaa: Msaa { samples: 1 },
            symbol_pools: BufferPoolSizes::EMPTY,
            buffer_pools: BufferPoolSizes::EMPTY,
            terrain_drape_size: 256,
            ..RendererSettings::default()
        };
        let (kernel, renderer) = create_headless_renderer_with_loader(64, 64, settings, loader)
            .await
            .expect("headless renderer");

        let mut map = HeadlessMap::new(
            terrain_raster_style("#102030"),
            renderer,
            kernel,
            vec![
                Box::new(RenderPlugin),
                Box::new(BackgroundPlugin),
                Box::new(RasterPlugin::<DefaultRasterTransferables>::default()),
                Box::new(TerrainPlugin::<DefaultDemTransferables>::default()),
                Box::new(OverlayPlugin),
            ],
        )
        .expect("headless map");

        let otw_slot = map.register_view_slot(ViewSlotConfig {
            size: PhysicalSize::new(160, 90).expect("otw size"),
            depth_readout: true,
        });
        let otw_right = map.register_view_slot(ViewSlotConfig {
            size: PhysicalSize::new(160, 90).expect("right eye size"),
            depth_readout: false,
        });
        let sensor_slot = map.register_view_slot(ViewSlotConfig {
            size: PhysicalSize::new(80, 60).expect("sensor size"),
            depth_readout: true,
        });

        let bogus = ViewSlotId::from_raw(999);
        let cam_zero = SlotView::Camera {
            center: LatLon::new(0.0, 0.0),
            zoom: Zoom::new(3.0),
            bearing_deg: 0.0,
            pitch_deg: 0.0,
        };
        assert!(matches!(
            map.run_multi_view_frame(MultiViewFrame {
                slots: vec![ViewSlotEntry { slot: bogus, view: cam_zero, policy: CoveringPolicy::Independent }],
                ..Default::default()
            }),
            Err(MultiViewError::UnknownSlot { slot }) if slot == bogus
        ));
        assert!(matches!(
            map.run_multi_view_frame(MultiViewFrame {
                slots: vec![ViewSlotEntry {
                    slot: otw_right,
                    view: SlotView::External(external_view_at(
                        47.0,
                        8.0,
                        1_500.0,
                        -8.0,
                        160.0 / 90.0
                    )),
                    policy: CoveringPolicy::StereoReuse(otw_slot),
                }],
                ..Default::default()
            }),
            Err(MultiViewError::InvalidStereoPrimary { .. })
        ));

        let otw_view =
            SlotView::External(external_view_at(47.0, 8.0, 2_500.0, -12.0, 160.0 / 90.0));
        let sensor_view = SlotView::Camera {
            center: LatLon::new(-33.86, 151.20),
            zoom: Zoom::new(6.0),
            bearing_deg: 0.0,
            pitch_deg: 0.0,
        };
        for step in 0..16 {
            map.run_multi_view_frame(MultiViewFrame {
                timestamp: Duration::from_millis(step * 16),
                opaque_environment: true,
                request_overscan: 1.0,
                slots: vec![
                    ViewSlotEntry {
                        slot: otw_slot,
                        view: otw_view,
                        policy: CoveringPolicy::Independent,
                    },
                    ViewSlotEntry {
                        slot: otw_right,
                        view: otw_view,
                        policy: CoveringPolicy::StereoReuse(otw_slot),
                    },
                    ViewSlotEntry {
                        slot: sensor_slot,
                        view: sensor_view,
                        policy: CoveringPolicy::Independent,
                    },
                ],
            })
            .expect("multi-view frame");
            crate::headless::map::test_async::sleep(Duration::from_millis(5)).await;
        }

        let otw_color = map.view_slot_color_texture(otw_slot).expect("otw color");
        let otw_depth = map.view_slot_depth_texture(otw_slot).expect("otw depth");
        let sensor_color = map
            .view_slot_color_texture(sensor_slot)
            .expect("sensor color");
        let sensor_depth = map
            .view_slot_depth_texture(sensor_slot)
            .expect("sensor depth");
        assert_eq!((otw_color.width(), otw_color.height()), (160, 90));
        assert_eq!((otw_depth.width(), otw_depth.height()), (160, 90));
        assert_eq!((sensor_color.width(), sensor_color.height()), (80, 60));
        assert_eq!((sensor_depth.width(), sensor_depth.height()), (80, 60));
        assert!(map.view_slot_depth_texture(otw_right).is_none());
        assert_eq!(map.view_slot_has_readback_buffer(otw_slot), Some(false));
        assert_eq!(map.view_slot_has_readback_buffer(sensor_slot), Some(false));

        // Per-slot OverlayMesh isolation: green on OTW, red on Sensor.
        map.set_view_slot_overlay(otw_slot, fullscreen_overlay(160, 90, [0.0, 1.0, 0.0, 1.0]))
            .expect("set otw overlay");
        map.set_view_slot_overlay(
            sensor_slot,
            fullscreen_overlay(80, 60, [1.0, 0.0, 0.0, 1.0]),
        )
        .expect("set sensor overlay");
        map.run_multi_view_frame(MultiViewFrame {
            slots: vec![
                ViewSlotEntry {
                    slot: otw_slot,
                    view: otw_view,
                    policy: CoveringPolicy::Independent,
                },
                ViewSlotEntry {
                    slot: sensor_slot,
                    view: sensor_view,
                    policy: CoveringPolicy::Independent,
                },
            ],
            ..Default::default()
        })
        .expect("overlay frame");

        let otw_rgba = crate::headless::map::readback::read_blocking(
            &map,
            map.view_slot_color_texture(otw_slot).expect("otw"),
            wgpu::TextureAspect::All,
        )
        .expect("read otw");
        let sensor_rgba = crate::headless::map::readback::read_blocking(
            &map,
            map.view_slot_color_texture(sensor_slot).expect("sensor"),
            wgpu::TextureAspect::All,
        )
        .expect("read sensor");
        assert!(
            otw_rgba[1] > 240 && otw_rgba[0] < 10,
            "OTW slot must show green overlay"
        );
        assert!(
            sensor_rgba[0] > 240 && sensor_rgba[1] < 10,
            "Sensor slot must show red overlay"
        );

        // Capture southern-hemisphere DEM tiles belonging exclusively to sensor_slot (y >= 2^(z-1)).
        let sensor_dem_before: HashSet<WorldTileCoords> = loaded_dem_coords(&map)
            .into_iter()
            .filter(|c| u8::from(c.z) >= 2 && c.y >= (1_i32 << (u8::from(c.z) - 1)))
            .collect();
        assert!(
            !sensor_dem_before.is_empty(),
            "sensor_slot must have loaded southern-hemisphere DEM tiles"
        );

        // Force critical memory pressure (0-tile out-of-view cache) and skip sensor_slot for 10 OTW frames.
        map.world_mut().resources.insert(MemoryBudget {
            available_bytes: Some(128 << 20),
        });
        for step in 0..10 {
            map.run_multi_view_frame(MultiViewFrame {
                timestamp: Duration::from_millis(500 + step * 8),
                slots: vec![ViewSlotEntry {
                    slot: otw_slot,
                    view: otw_view,
                    policy: CoveringPolicy::Independent,
                }],
                ..Default::default()
            })
            .expect("otw-only frame");
        }

        let after_skip = loaded_dem_coords(&map);
        for coords in &sensor_dem_before {
            assert!(
                after_skip.contains(coords),
                "skipped sensor_slot DEM tile {coords} must stay resident across OTW-only frames"
            );
        }

        // Once sensor_slot is unregistered, critical pressure immediately evicts its unused tiles.
        assert!(map.remove_view_slot(sensor_slot));
        map.run_multi_view_frame(MultiViewFrame {
            slots: vec![ViewSlotEntry {
                slot: otw_slot,
                view: otw_view,
                policy: CoveringPolicy::Independent,
            }],
            ..Default::default()
        })
        .expect("post-remove frame");
        let after_remove = loaded_dem_coords(&map);
        assert!(
            sensor_dem_before.iter().any(|c| !after_remove.contains(c)),
            "unregistered slot's out-of-view tiles should become eligible for eviction"
        );
    })
}
