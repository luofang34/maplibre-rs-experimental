//! A terrain tile's drape does not wait on a raster source that will not deliver.
#![allow(clippy::expect_used, clippy::panic)]

use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

use cgmath::{Matrix4, Rad, SquareMatrix, Vector3};

use crate::{
    coords::LatLon,
    headless::{create_headless_renderer_with_loader, map::HeadlessMap},
    io::{
        resource_loader::SharedLoader,
        source_client::{HttpClient, SourceFetchError},
    },
    raster::{DefaultRasterTransferables, RasterPlugin},
    render::{
        camera::EyeFrustum,
        eventually::Eventually::Initialized,
        view_state::ExternalAnchor,
        xr::{EyeTarget, ScenePlacement, XrEye, XrFrame},
        RenderPlugin,
    },
    style::Style,
    terrain::{resources::TerrainResources, DefaultDemTransferables, TerrainPlugin},
};

#[derive(Debug, thiserror::Error)]
#[error("service unavailable")]
struct Unavailable;

/// Serves imagery and flat terrain, and answers the detail server with 503.
#[derive(Clone, Default)]
struct Servers {
    fetched: Arc<tokio::sync::Notify>,
    detail_requests: Arc<Mutex<u32>>,
}

fn png(pixel: [u8; 4]) -> Vec<u8> {
    let mut png = std::io::Cursor::new(Vec::new());
    image::RgbaImage::from_pixel(256, 256, image::Rgba(pixel))
        .write_to(&mut png, image::ImageFormat::Png)
        .expect("PNG");
    png.into_inner()
}

#[cfg_attr(not(feature = "thread-safe-futures"), async_trait::async_trait(?Send))]
#[cfg_attr(feature = "thread-safe-futures", async_trait::async_trait)]
impl HttpClient for Servers {
    async fn fetch(&self, url: &str) -> Result<Vec<u8>, SourceFetchError> {
        self.fetched.notify_one();
        if url.starts_with("https://detail.invalid/") {
            *self.detail_requests.lock().expect("count") += 1;
            return Err(SourceFetchError::http_response(url, 503, Unavailable));
        }
        if url.starts_with("https://dem.invalid/") {
            // Terrarium zero metres.
            return Ok(png([128, 0, 0, 255]));
        }
        Ok(png([90, 140, 60, 255]))
    }
}

fn style(extra: &str) -> Style {
    serde_json::from_value(serde_json::json!({
        "version": 8,
        "projection": {"type": "mercator"},
        "terrain": {"source": "dem"},
        "sources": {
            "dem": {"type": "raster-dem", "tiles": ["https://dem.invalid/{z}/{x}/{y}.png"],
                    "tileSize": 256, "maxzoom": 12, "encoding": "terrarium"},
            "base": {"type": "raster", "tiles": ["https://base.invalid/{z}/{x}/{y}.png"],
                     "tileSize": 256, "maxzoom": 14},
            "detail": {"type": "raster", "tiles": ["https://detail.invalid/{z}/{x}/{y}.png"],
                       "tileSize": 256, "maxzoom": 14},
            "close": {"type": "raster", "tiles": ["https://close.invalid/{z}/{x}/{y}.png"],
                      "tileSize": 256, "minzoom": 21, "maxzoom": 22}
        },
        "layers": [
            {"id": "base", "type": "raster", "source": "base"},
            {"id": extra, "type": "raster", "source": extra}
        ]
    }))
    .expect("style")
}

fn frame(timestamp: u64) -> XrFrame {
    XrFrame {
        opaque_environment: true,
        timestamp: Duration::from_millis(timestamp),
        placement: ScenePlacement {
            anchor: ExternalAnchor {
                position: LatLon::new(40.577, -74.738),
                altitude_meters: 0.0,
            },
            world_from_scene: Matrix4::identity(),
        },
        eyes: vec![XrEye {
            world_from_eye: Matrix4::from_translation(Vector3::new(0.0, 0.0, 3000.0)),
            frustum: EyeFrustum::symmetric(Rad(1.0), 1.0, 1.0, 1.0e6),
            target: EyeTarget::default(),
        }],
        request_overscan: 1.0,
        prefetch: None,
    }
}

fn drapes(map: &HeadlessMap) -> usize {
    match map
        .world()
        .resources
        .get::<crate::render::eventually::Eventually<TerrainResources>>()
    {
        Some(Initialized(terrain)) => terrain.drape_counts().0,
        _ => 0,
    }
}

/// Drapes built within a bounded number of frames for the base imagery under `extra`.
async fn drapes_beside(extra: &str) -> (usize, u32) {
    let servers = Servers::default();
    let (kernel, renderer) = create_headless_renderer_with_loader(
        128,
        128,
        Default::default(),
        SharedLoader::new(servers.clone()),
    )
    .await
    .expect("renderer");
    let mut map = HeadlessMap::new(
        style(extra),
        renderer,
        kernel,
        vec![
            Box::new(RenderPlugin),
            Box::new(RasterPlugin::<DefaultRasterTransferables>::default()),
            Box::new(TerrainPlugin::<DefaultDemTransferables>::default()),
        ],
    )
    .expect("map");
    let mut built = 0;
    for step in 1..=80_u64 {
        map.run_xr_frame(frame(16 * step)).expect("frame");
        built = drapes(&map);
        if built > 0 {
            break;
        }
        // Workers answer asynchronously; the next frame folds in what arrived.
        tokio::time::timeout(Duration::from_millis(200), servers.fetched.notified())
            .await
            .ok();
    }
    let detail_requests = *servers.detail_requests.lock().expect("count");
    (built, detail_requests)
}

#[tokio::test(flavor = "multi_thread")]
async fn a_source_whose_server_fails_every_tile_does_not_hold_the_drape_back() {
    let (built, detail_requests) = drapes_beside("detail").await;
    assert!(detail_requests > 0, "the failing server was asked");
    assert!(
        built > 0,
        "terrain tiles drape the base imagery beside a failing source"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_source_with_no_tile_at_the_view_zoom_does_not_hold_the_drape_back() {
    let (built, _) = drapes_beside("close").await;
    assert!(
        built > 0,
        "terrain tiles drape the base imagery beside a source zoomed out of view"
    );
}
