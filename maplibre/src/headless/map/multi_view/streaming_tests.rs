//! An eye flying level over terrain keeps imagery on the ground out to its horizon while the
//! tiles ahead of it are still arriving.
#![allow(clippy::expect_used, clippy::panic)]

use std::{
    collections::HashSet,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};

use cgmath::{Deg, Matrix3, Matrix4, Rad};
use image::ImageEncoder;

use super::*;
use crate::{
    background::BackgroundPlugin,
    coords::{LatLon, WorldTileCoords},
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
        DrapePhase, TerrainPlugin,
    },
    window::PhysicalSize,
};

/// Answers every tile a fixed number of frames after its request, staggered by tile, the way
/// a tile server on a network does.
#[derive(Clone, Default)]
struct LateTiles {
    frames: Arc<AtomicU64>,
    advanced: Arc<tokio::sync::Notify>,
    /// How many imagery tiles from level 14 down, the first the map asks for, never answer.
    /// Each holds a request in flight, so the count stays under the bound on those.
    unanswered_detail: usize,
    /// The imagery tiles left unanswered, as `(x, y, z)`.
    unanswered: Arc<Mutex<HashSet<(u64, u64, u64)>>>,
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
        if url.contains("ortho.example") && z >= 14 {
            let withheld = {
                let mut unanswered = self.unanswered.lock().expect("lock");
                unanswered.contains(&(x, y, z))
                    || (unanswered.len() < self.unanswered_detail && unanswered.insert((x, y, z)))
            };
            if withheld {
                std::future::pending::<()>().await;
            }
        }
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

async fn flying_map(tiles: &LateTiles, memory: Option<u64>) -> (HeadlessMap, ViewSlotId) {
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
    // A browser tab: a few drapes a frame, and with `memory` short, coarser drapes and fewer
    // requests in flight. The time a moving view allows its drapes is left wide open, so the
    // count alone paces them on whatever GPU runs the test.
    map.set_available_memory(memory);
    map.world_mut().resources.insert(DrapeBudget {
        per_frame: 4,
        per_eye_frame: 3,
        moving_time: Duration::from_secs(1),
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
    let (mut map, slot) = flying_map(&tiles, Some(700 << 20)).await;
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

/// Whether `tile` lies under `ancestor`.
fn under(tile: WorldTileCoords, ancestor: WorldTileCoords) -> bool {
    let mut current = tile;
    while current.z > ancestor.z {
        let Some(parent) = current.get_parent() else {
            return false;
        };
        current = parent;
    }
    current == ancestor
}

/// The web page that reported the uniform band had loaded the tiles under it, yet its drapes
/// stayed on their stand-ins: a drape whose set never completes must still show what arrived.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_eyes_drape_shows_the_detail_that_arrived_while_some_of_it_never_answers() {
    let tiles = LateTiles {
        unanswered_detail: 3,
        ..LateTiles::default()
    };
    let (mut map, slot) = flying_map(&tiles, None).await;
    let mut detail_over_stand_in = false;
    for frame in 0..FLIGHT_FRAMES {
        map.run_multi_view_frame(MultiViewFrame {
            timestamp: Duration::from_micros(frame * 31_250),
            opaque_environment: true,
            request_overscan: 1.22,
            slots: vec![ViewSlotEntry {
                slot,
                view: SlotView::External(eastbound_eye(LatLon::new(41.66, -1.0), 2_049.0)),
                policy: CoveringPolicy::Independent,
            }],
        })
        .expect("frame");
        tiles.advance();
        for _ in 0..16 {
            tokio::task::yield_now().await;
        }
        let unanswered: Vec<WorldTileCoords> = tiles
            .unanswered
            .lock()
            .expect("lock")
            .iter()
            .map(|&(x, y, z)| WorldTileCoords::from((x as i32, y as i32, (z as u8).into())))
            .collect();
        let Some(phase) = map.world().resources.get::<DrapePhase>() else {
            continue;
        };
        detail_over_stand_in |= phase.targets.iter().any(|target| {
            unanswered.iter().any(|tile| under(*tile, target.coords))
                && target
                    .layers
                    .iter()
                    .any(|layer| layer.tile.coords.z > target.coords.z)
        });
    }
    assert_eq!(
        tiles.unanswered.lock().expect("lock").len(),
        3,
        "the view wants detail the server never answers"
    );
    assert!(
        detail_over_stand_in,
        "a drape over an unanswered tile draws the tiles that did arrive"
    );
}
