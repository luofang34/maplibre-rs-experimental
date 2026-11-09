//! A terrain map whose tiles have all landed, failed or been refused stops asking for frames
//! once its camera rests.
#![allow(clippy::expect_used, clippy::panic)]

use std::sync::{
    atomic::{AtomicU64, AtomicUsize, Ordering},
    Arc,
};

use crate::headless::map::HeadlessMap;
use crate::{
    coords::LatLon,
    io::source_client::{HttpClient, SourceFetchError},
    render::{
        frame_signals::RedrawReasons,
        view_state::{CameraOptions, NavigationMode},
        RenderPlugin,
    },
    style::Style,
};

const WIDTH: u32 = 1024;
const HEIGHT: u32 = 640;

/// Serves Terrarium elevation and imagery the way a tile server on a network would: each
/// answer arrives a few frames after its request, staggered by tile, and tiles a source does
/// not hold answer 404.
#[derive(Clone, Default)]
struct SyntheticTiles {
    /// URLs containing this answer 503, which the map retries after a backoff.
    failing: Option<&'static str>,
    failures: Arc<AtomicUsize>,
    frames: Arc<AtomicU64>,
    advanced: Arc<tokio::sync::Notify>,
}

impl SyntheticTiles {
    /// Lets the answers due by the next frame arrive.
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

fn png(image: &image::RgbaImage) -> Vec<u8> {
    let mut png = std::io::Cursor::new(Vec::new());
    image
        .write_to(&mut png, image::ImageFormat::Png)
        .expect("PNG");
    png.into_inner()
}

#[cfg_attr(not(feature = "thread-safe-futures"), async_trait::async_trait(?Send))]
#[cfg_attr(feature = "thread-safe-futures", async_trait::async_trait)]
impl HttpClient for SyntheticTiles {
    async fn fetch(&self, url: &str) -> Result<Vec<u8>, SourceFetchError> {
        let address: Vec<u32> = url
            .trim_end_matches(".png")
            .rsplit('/')
            .take(3)
            .map(|part| part.parse().expect("tile address"))
            .collect();
        let (y, x, z) = (address[0], address[1], address[2]);
        self.arrive_after(u64::from(1 + (z * 7 + x * 3 + y * 5) % 3))
            .await;
        if self.failing.is_some_and(|failing| url.contains(failing)) {
            self.failures.fetch_add(1, Ordering::SeqCst);
            return Err(SourceFetchError::http_response(url, 503, std::fmt::Error));
        }
        if z >= 2 && (x + y) % 3 == 0 {
            return Err(SourceFetchError::not_found(url));
        }
        if url.contains("dem.example") {
            return Ok(png(&image::RgbaImage::from_fn(256, 256, |x, y| {
                let height =
                    1200.0 + 800.0 * (f64::from(x) * 0.05).sin() * (f64::from(y) * 0.03).cos();
                let value = ((height + 32768.0) * 256.0) as u32;
                image::Rgba([(value >> 16) as u8, (value >> 8) as u8, value as u8, 255])
            })));
        }
        Ok(png(&image::RgbaImage::from_fn(256, 256, |x, y| {
            image::Rgba([x as u8, y as u8, 60, 255])
        })))
    }
}

/// Imagery draped over terrain, shaded from a second source over the same elevation tiles,
/// with GeoJSON drawn on top.
fn style(zoom: f64, pitch: f64) -> Style {
    let dem = serde_json::json!({"type": "raster-dem", "encoding": "terrarium",
        "tileSize": 512, "maxzoom": 12, "tiles": ["https://dem.example/{z}/{x}/{y}.png"]});
    serde_json::from_value(serde_json::json!({
        "version": 8, "center": [104.5, 30.2], "zoom": zoom, "pitch": pitch,
        "sources": {
            "imagery": {"type": "raster", "tiles": ["https://img.example/{z}/{x}/{y}.png"],
                "tileSize": 256, "maxzoom": 14},
            "terrain-dem": dem, "hillshade-dem": dem,
            "places": {"type": "geojson", "data": {"type": "Feature", "properties": {},
                "geometry": {"type": "LineString", "coordinates": [[100, 25], [110, 35]]}}}
        },
        "layers": [
            {"id": "background", "type": "background", "paint": {"background-color": "#102030"}},
            {"id": "imagery", "type": "raster", "source": "imagery"},
            {"id": "hillshade", "type": "hillshade", "source": "hillshade-dem"},
            {"id": "edge", "type": "line", "source": "places"},
            {"id": "dots", "type": "circle", "source": "places"}
        ],
        "terrain": {"source": "terrain-dem", "exaggeration": 1.5},
        "projection": {"type": "vertical-perspective"}
    }))
    .expect("style")
}

async fn terrain_map(zoom: f64, pitch: f64, tiles: SyntheticTiles) -> HeadlessMap {
    let (kernel, renderer) = crate::headless::create_headless_renderer_with_loader(
        WIDTH,
        HEIGHT,
        Default::default(),
        crate::io::resource_loader::SharedLoader::new(tiles),
    )
    .await
    .expect("renderer");
    let mut map = HeadlessMap::new(
        style(zoom, pitch),
        renderer,
        kernel,
        vec![
            Box::new(RenderPlugin),
            Box::new(crate::background::BackgroundPlugin),
            Box::new(crate::raster::RasterPlugin::<
                crate::raster::DefaultRasterTransferables,
            >::default()),
            Box::new(crate::hillshade::HillshadePlugin),
            Box::new(crate::vector::VectorPlugin::<
                crate::vector::DefaultVectorTransferables,
            >::default()),
            Box::new(crate::terrain::TerrainPlugin::<
                crate::terrain::DefaultDemTransferables,
            >::default()),
            Box::new(crate::hud::NavigationHudPlugin),
        ],
    )
    .expect("map");
    map.set_max_pitch(cgmath::Deg(85.0));
    map.set_navigation_mode(NavigationMode::FreeGlobe)
        .expect("free globe");
    map.set_pixel_ratio(2.0);
    map
}

fn jump(map: &mut HeadlessMap, zoom: f64) {
    map.jump_to(CameraOptions {
        center: Some(LatLon::new(30.2, 104.5)),
        zoom: Some(zoom),
        ..Default::default()
    })
    .expect("jump");
}

/// Draws only while the map asks to, as a host drawing on demand does, letting answers arrive
/// between frames; returns how many frames it drew before the map stopped asking, or what
/// still asked after `limit`.
async fn frames_until_idle(
    map: &mut HeadlessMap,
    tiles: &SyntheticTiles,
    limit: usize,
) -> Result<usize, RedrawReasons> {
    for frame in 0..limit {
        if !map.needs_redraw() {
            return Ok(frame);
        }
        map.run_frame().expect("frame");
        tiles.advance();
        for _ in 0..16 {
            tokio::task::yield_now().await;
        }
    }
    if map.needs_redraw() {
        Err(map.redraw_reasons())
    } else {
        Ok(limit)
    }
}

#[tokio::test]
async fn a_resting_terrain_map_stops_asking_for_frames() {
    for (zoom, pitch) in [(1.5, 0.0), (11.0, 70.0)] {
        let tiles = SyntheticTiles::default();
        let mut map = terrain_map(zoom, pitch, tiles.clone()).await;
        let idle = frames_until_idle(&mut map, &tiles, 600).await;
        assert!(
            idle.is_ok(),
            "zoom {zoom}, pitch {pitch}: still asking after 600 frames: {idle:?}"
        );
        assert_eq!(map.redraw_reasons(), RedrawReasons::default());
    }
}

#[tokio::test]
async fn a_failed_tile_the_view_left_waits_for_the_view_to_return() {
    let tiles = SyntheticTiles {
        failing: Some("img.example/2/"),
        ..Default::default()
    };
    let mut map = terrain_map(1.5, 0.0, tiles.clone()).await;
    frames_until_idle(&mut map, &tiles, 300)
        .await
        .expect("the globe settles while its failed tiles back off");
    assert!(map.map_context.has_scheduled_retries());
    jump(&mut map, 9.0);
    frames_until_idle(&mut map, &tiles, 600)
        .await
        .expect("the closer view settles");

    // The backoff of the tiles left behind runs out with no view wanting them.
    map.frame_input_mut()
        .advance(std::time::Duration::from_secs(120));
    let idle = frames_until_idle(&mut map, &tiles, 60).await;
    assert!(
        idle.is_ok(),
        "a retry no view wants keeps the map drawing: {idle:?}"
    );

    let failures = tiles.failures.load(Ordering::SeqCst);
    jump(&mut map, 1.5);
    frames_until_idle(&mut map, &tiles, 300)
        .await
        .expect("the globe settles again");
    assert!(
        tiles.failures.load(Ordering::SeqCst) > failures,
        "the failed tiles are requested again once they are back in view"
    );
}

/// Without a runtime to run them on, every request is refused.
#[test]
fn refused_requests_wait_out_a_backoff_instead_of_asking_for_every_frame() {
    let tiles = SyntheticTiles::default();
    let mut map = futures::executor::block_on(terrain_map(1.5, 0.0, tiles));
    for _ in 0..4 {
        map.run_frame().expect("frame");
    }
    assert_eq!(
        map.redraw_reasons(),
        RedrawReasons::default(),
        "a refused request asks for no frame until its backoff passes"
    );
    let wait = map
        .map_context
        .next_retry_delay()
        .expect("refused requests are retried");
    assert!(wait > std::time::Duration::ZERO);
}
