//! An eye flying level over terrain keeps imagery on the ground out to its horizon while the
//! tiles ahead of it are still arriving.
#![allow(clippy::expect_used, clippy::panic)]

use std::{
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::Duration,
};

use cgmath::{Deg, Matrix3, Matrix4, Rad};
use image::ImageEncoder;

use super::*;
use crate::{
    background::BackgroundPlugin,
    coords::LatLon,
    headless::create_headless_renderer_with_loader,
    io::{
        resource_loader::SharedLoader,
        source_client::{HttpClient, SourceFetchError},
    },
    raster::{DefaultRasterTransferables, RasterPlugin},
    render::{
        camera::EyeFrustum,
        settings::{BufferPoolSizes, Msaa, RendererSettings},
        view_state::{ExternalAnchor, ExternalView},
        RenderPlugin,
    },
    style::Style,
    terrain::{
        queue_system::TerrainEyeFrame, transferables::DefaultDemTransferables, DrapeBudget,
        TerrainPlugin,
    },
    window::PhysicalSize,
};

/// Answers every tile a fixed number of frames after its request, staggered by tile, the way
/// a tile server on a network does.
#[derive(Clone, Default)]
struct LateTiles {
    frames: Arc<AtomicU64>,
    advanced: Arc<tokio::sync::Notify>,
}

impl LateTiles {
    fn advance(&self) {
        self.frames.fetch_add(1, Ordering::SeqCst);
        self.advanced.notify_waiters();
    }

    async fn arrive_after(&self, frames: u64) {
        let due = self.frames.load(Ordering::SeqCst) + frames;
        loop {
            let advanced = self.advanced.notified();
            if self.frames.load(Ordering::SeqCst) >= due {
                return;
            }
            advanced.await;
        }
    }
}

#[cfg_attr(not(feature = "thread-safe-futures"), async_trait::async_trait(?Send))]
#[cfg_attr(feature = "thread-safe-futures", async_trait::async_trait)]
impl HttpClient for LateTiles {
    async fn fetch(&self, url: &str) -> Result<Vec<u8>, SourceFetchError> {
        let address: Vec<u64> = url
            .trim_end_matches(".png")
            .rsplit('/')
            .take(3)
            .map(|part| part.parse().expect("tile address"))
            .collect();
        let (y, x, z) = (address[0], address[1], address[2]);
        self.arrive_after(4 + (z * 7 + x * 3 + y * 5) % 7).await;
        let rgba = if url.contains("dem.example") {
            // Terrarium: about 250 m, the Ebro valley floor.
            [128, 250, 0, 255]
        } else {
            [40, 120, 200, 255]
        };
        let pixels: Vec<u8> = rgba.iter().copied().cycle().take(16 * 16 * 4).collect();
        let mut bytes = Vec::new();
        image::codecs::png::PngEncoder::new(&mut bytes)
            .write_image(&pixels, 16, 16, image::ExtendedColorType::Rgba8)
            .expect("PNG");
        Ok(bytes)
    }
}

fn style() -> Style {
    serde_json::from_value(serde_json::json!({
        "version": 8,
        "projection": {"type": "vertical-perspective"},
        "sources": {
            "dem": {"type": "raster-dem", "encoding": "terrarium", "tileSize": 256,
                "minzoom": 0, "maxzoom": 15, "tiles": ["https://dem.example/{z}/{x}/{y}.png"]},
            "ortho": {"type": "raster", "tileSize": 256, "minzoom": 0, "maxzoom": 18,
                "tiles": ["https://ortho.example/{z}/{x}/{y}.png"]}
        },
        "terrain": {"source": "dem", "exaggeration": 1.0},
        "layers": [
            {"id": "bg", "type": "background", "paint": {"background-color": "#0b1626"}},
            {"id": "ortho", "type": "raster", "source": "ortho",
                "paint": {"raster-fade-duration": 0}}
        ]
    }))
    .expect("style")
}

/// A level eye heading east at `altitude` metres over `position`.
fn eastbound_eye(position: LatLon, altitude: f64) -> ExternalView {
    let eye_from_north_facing = Matrix3::new(1.0, 0.0, 0.0, 0.0, 0.0, -1.0, 0.0, 1.0, 0.0);
    let north_from_east = Matrix3::from_angle_z(Deg(90.0));
    ExternalView {
        anchor: ExternalAnchor {
            position,
            altitude_meters: altitude,
        },
        view: Matrix4::from(eye_from_north_facing * north_from_east),
        frustum: EyeFrustum::symmetric(Rad(45_f64.to_radians()), 16.0 / 9.0, 1.0, 350_000.0),
    }
}

/// The terrain tiles drawn in the last frame, and how many of them had no drape to show.
fn drawn_terrain(map: &HeadlessMap) -> (usize, usize) {
    let Some(frame) = map.world().resources.get::<TerrainEyeFrame>() else {
        return (0, 0);
    };
    let bare = frame
        .0
        .iter()
        .filter(|(_, uniforms)| uniforms.fog_opacity[2] > 0.5)
        .count();
    (frame.0.len(), bare)
}

async fn flying_map(tiles: &LateTiles) -> (HeadlessMap, ViewSlotId) {
    let settings = RendererSettings {
        msaa: Msaa { samples: 1 },
        symbol_pools: BufferPoolSizes::EMPTY,
        terrain_drape_size: 1024,
        ..RendererSettings::default()
    };
    let (kernel, renderer) =
        create_headless_renderer_with_loader(64, 64, settings, SharedLoader::new(tiles.clone()))
            .await
            .expect("renderer");
    let mut map = HeadlessMap::new(
        style(),
        renderer,
        kernel,
        vec![
            Box::new(RenderPlugin),
            Box::new(BackgroundPlugin),
            Box::new(RasterPlugin::<DefaultRasterTransferables>::default()),
            Box::new(TerrainPlugin::<DefaultDemTransferables>::default()),
        ],
    )
    .expect("map");
    // A browser tab: a share of a frame for drapes, and memory short enough to coarsen them.
    map.set_available_memory(Some(700 << 20));
    map.world_mut().resources.insert(DrapeBudget {
        per_frame: 4,
        per_eye_frame: 3,
        moving_time: Duration::from_micros(2_000),
    });
    let slot = map.register_view_slot(ViewSlotConfig {
        size: PhysicalSize::new(800, 450).expect("size"),
        depth_readout: false,
    });
    (map, slot)
}

/// Frames of level flight east from Zaragoza at 2 km and 100 m/s, 32 frames a second.
const FLIGHT_FRAMES: u64 = 200;
/// Frames by which the ground out to the horizon must hold imagery: one tile for each draped
/// terrain tile, four requests at a time, each answered within ten frames, and three drapes
/// drawn a frame.
const COVERED_BY_FRAME: u64 = 120;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_eye_in_flight_has_imagery_out_to_its_horizon_while_the_detail_streams_in() {
    let tiles = LateTiles::default();
    let (mut map, slot) = flying_map(&tiles).await;
    let metres_per_frame = 100.0 / 32.0;
    let degrees_per_metre = 1.0 / (111_320.0 * 41.66_f64.to_radians().cos());
    let mut covered_at = None;
    let mut bare_after_covered = 0;
    for frame in 0..FLIGHT_FRAMES {
        let longitude = -1.0 + frame as f64 * metres_per_frame * degrees_per_metre;
        map.run_multi_view_frame(MultiViewFrame {
            timestamp: Duration::from_micros(frame * 31_250),
            opaque_environment: true,
            request_overscan: 1.22,
            slots: vec![ViewSlotEntry {
                slot,
                view: SlotView::External(eastbound_eye(LatLon::new(41.66, longitude), 2_049.0)),
                policy: CoveringPolicy::Independent,
            }],
        })
        .expect("frame");
        tiles.advance();
        for _ in 0..16 {
            tokio::task::yield_now().await;
        }
        let (drawn, bare) = drawn_terrain(&map);
        assert!(drawn > 0, "the eye sees terrain in frame {frame}");
        match covered_at {
            None if bare == 0 => covered_at = Some(frame),
            None => {}
            Some(_) => bare_after_covered += bare,
        }
    }
    assert!(
        covered_at.is_some_and(|frame| frame <= COVERED_BY_FRAME),
        "every terrain tile drawn holds imagery by frame {COVERED_BY_FRAME}, not {covered_at:?}"
    );
    assert_eq!(
        bare_after_covered, 0,
        "terrain coming into view holds imagery from its first frame"
    );
}
