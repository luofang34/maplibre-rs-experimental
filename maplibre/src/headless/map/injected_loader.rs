//! A host's loader serves the map thread and the tile workers alike.
#![allow(clippy::expect_used, clippy::panic)]

use std::sync::{Arc, Mutex};

use geozero::mvt::Message as _;

use super::HeadlessMap;
use crate::{
    coords::WorldTileCoords,
    headless::create_headless_renderer_with_loader,
    io::{
        resource_loader::SharedLoader,
        source_client::{HttpClient, SourceFetchError},
    },
    render::RenderPlugin,
    style::Style,
    vector::{DefaultVectorTransferables, VectorPlugin},
};

/// Answers every URL with a one-line vector tile and records the URL.
#[derive(Clone, Default)]
struct Recorder {
    urls: Arc<Mutex<Vec<String>>>,
    fetched: Arc<tokio::sync::Notify>,
}

#[cfg_attr(not(feature = "thread-safe-futures"), async_trait::async_trait(?Send))]
#[cfg_attr(feature = "thread-safe-futures", async_trait::async_trait)]
impl HttpClient for Recorder {
    async fn fetch(&self, url: &str) -> Result<Vec<u8>, SourceFetchError> {
        self.urls.lock().expect("urls").push(url.to_owned());
        self.fetched.notify_one();
        if url.ends_with(".png") {
            let mut png = std::io::Cursor::new(Vec::new());
            image::RgbaImage::from_pixel(4, 4, image::Rgba([128, 0, 0, 255]))
                .write_to(&mut png, image::ImageFormat::Png)
                .expect("PNG");
            return Ok(png.into_inner());
        }
        Ok(geozero::mvt::Tile {
            layers: vec![geozero::mvt::tile::Layer {
                version: 2,
                name: "roads".into(),
                extent: Some(4096),
                features: vec![geozero::mvt::tile::Feature {
                    r#type: Some(geozero::mvt::tile::GeomType::Linestring as i32),
                    geometry: vec![9, 2, 2, 10, 2, 2],
                    ..Default::default()
                }],
                ..Default::default()
            }],
        }
        .encode_to_vec())
    }
}

impl Recorder {
    fn saw(&self, prefix: &str) -> bool {
        self.urls
            .lock()
            .expect("urls")
            .iter()
            .any(|url| url.starts_with(prefix))
    }
}

async fn map_with(recorder: &Recorder) -> HeadlessMap {
    let (kernel, renderer) = create_headless_renderer_with_loader(
        64,
        64,
        Default::default(),
        SharedLoader::new(recorder.clone()),
    )
    .await
    .expect("renderer");
    let style: Style = serde_json::from_value(serde_json::json!({
        "version": 8, "zoom": 2,
        "sources": {"v": {"type": "vector",
            "tiles": ["https://worker.invalid/{z}/{x}/{y}.pbf"]}},
        "layers": [{"id": "roads", "type": "line", "source": "v", "source-layer": "roads"}]
    }))
    .expect("style");
    HeadlessMap::new(
        style,
        renderer,
        kernel,
        vec![
            Box::new(RenderPlugin),
            Box::new(VectorPlugin::<DefaultVectorTransferables>::default()),
        ],
    )
    .expect("map")
}

#[tokio::test]
async fn the_map_thread_and_tile_workers_fetch_through_the_injected_loader() {
    let recorder = Recorder::default();
    let mut map = map_with(&recorder).await;
    map.fetch_tile(WorldTileCoords::default())
        .await
        .expect("map-thread fetch");
    assert_eq!(recorder.urls.lock().expect("urls").len(), 1);
    for _ in 0..20 {
        if recorder.saw("https://worker.invalid/") {
            break;
        }
        map.run_frame().expect("frame");
        tokio::time::timeout(
            std::time::Duration::from_secs(5),
            recorder.fetched.notified(),
        )
        .await
        .ok();
    }
    assert!(
        recorder.saw("https://worker.invalid/"),
        "tile workers fetch through the host's loader: {:?}",
        recorder.urls.lock().expect("urls")
    );
}

#[tokio::test]
async fn an_elevation_tile_fetched_through_the_loader_is_reported_ready() {
    use crate::{io::tile_retry::RequestKind, render::frame_signals::ResourceReady};
    let recorder = Recorder::default();
    let (kernel, renderer) = create_headless_renderer_with_loader(
        64,
        64,
        Default::default(),
        SharedLoader::new(recorder.clone()),
    )
    .await
    .expect("renderer");
    let style: Style = serde_json::from_value(serde_json::json!({
        "version": 8, "zoom": 2,
        "sources": {"dem": {"type": "raster-dem", "encoding": "terrarium",
            "tiles": ["https://dem.invalid/{z}/{x}/{y}.png"]}},
        "terrain": {"source": "dem"},
        "layers": [{"id": "bg", "type": "background"}]
    }))
    .expect("style");
    let mut map = HeadlessMap::new(
        style,
        renderer,
        kernel,
        vec![
            Box::new(RenderPlugin),
            Box::new(crate::terrain::TerrainPlugin::<
                crate::terrain::DefaultDemTransferables,
            >::default()),
        ],
    )
    .expect("map");
    let mut ready = Vec::new();
    for _ in 0..40 {
        map.run_frame().expect("frame");
        ready.extend(map.take_ready_resources());
        if ready.iter().any(|resource| {
            matches!(
                resource,
                ResourceReady::Tile {
                    kind: RequestKind::Dem,
                    loaded: true,
                    ..
                }
            )
        }) {
            return;
        }
        tokio::time::timeout(
            std::time::Duration::from_millis(200),
            recorder.fetched.notified(),
        )
        .await
        .ok();
    }
    panic!(
        "no elevation tile was reported ready: {ready:?}, fetched {:?}",
        recorder.urls.lock().expect("urls")
    );
}

/// Holds every fetch until the test ends, counting fetches begun and fetches dropped unfinished.
#[derive(Clone, Default)]
struct Stalled {
    urls: Arc<Mutex<Vec<String>>>,
    dropped: Arc<Mutex<Vec<String>>>,
    entered: Arc<tokio::sync::Notify>,
    never: Arc<tokio::sync::Notify>,
}

struct Unfinished(String, Arc<Mutex<Vec<String>>>);
impl Drop for Unfinished {
    fn drop(&mut self) {
        self.1.lock().expect("dropped").push(self.0.clone());
    }
}

#[cfg_attr(not(feature = "thread-safe-futures"), async_trait::async_trait(?Send))]
#[cfg_attr(feature = "thread-safe-futures", async_trait::async_trait)]
impl HttpClient for Stalled {
    async fn fetch(&self, url: &str) -> Result<Vec<u8>, SourceFetchError> {
        self.urls.lock().expect("urls").push(url.to_owned());
        let _unfinished = Unfinished(url.to_owned(), self.dropped.clone());
        self.entered.notify_one();
        self.never.notified().await;
        Err(SourceFetchError::not_found(url))
    }
}

impl Stalled {
    /// Lets workers run until `count` fetches have begun or nothing more happens.
    async fn until_fetched(&self, count: usize) {
        while self.urls.lock().expect("urls").len() < count {
            let entered = self.entered.notified();
            if tokio::time::timeout(std::time::Duration::from_secs(2), entered)
                .await
                .is_err()
            {
                return;
            }
        }
    }
}

#[tokio::test]
async fn requests_for_tiles_out_of_view_stop_and_free_their_slots() {
    use std::collections::HashSet;

    use crate::io::tile_backpressure::{tiles_in_flight, MAX_TILES_IN_FLIGHT};
    let stalled = Stalled::default();
    let (kernel, renderer) = create_headless_renderer_with_loader(
        256,
        256,
        Default::default(),
        SharedLoader::new(stalled.clone()),
    )
    .await
    .expect("renderer");
    let style: Style = serde_json::from_value(serde_json::json!({
        "version": 8, "zoom": 5, "center": [-100, 40],
        "sources": {"v": {"type": "vector",
            "tiles": ["https://worker.invalid/{z}/{x}/{y}.pbf"]}},
        "layers": [{"id": "roads", "type": "line", "source": "v", "source-layer": "roads"}]
    }))
    .expect("style");
    let mut map = HeadlessMap::new(
        style,
        renderer,
        kernel,
        vec![
            Box::new(RenderPlugin),
            Box::new(VectorPlugin::<DefaultVectorTransferables>::default()),
        ],
    )
    .expect("map");
    map.run_frame().expect("first view");
    stalled.until_fetched(MAX_TILES_IN_FLIGHT).await;
    let first: HashSet<WorldTileCoords> = map
        .world()
        .tiles
        .tiles
        .values()
        .map(|tile| tile.coords)
        .collect();
    assert_eq!(
        tiles_in_flight(&map.world().tiles),
        MAX_TILES_IN_FLIGHT,
        "the first view's requests take every slot"
    );

    // Half the world away, nothing of the first view is wanted.
    map.view_state_mut()
        .camera_mut()
        .move_relative(cgmath::Vector2::new(512.0 * 32.0 / 2.0, 0.0));
    map.run_frame().expect("moved view");
    map.run_frame().expect("following frame");
    stalled.until_fetched(2 * MAX_TILES_IN_FLIGHT).await;

    let resident: HashSet<WorldTileCoords> = map
        .world()
        .tiles
        .tiles
        .values()
        .map(|tile| tile.coords)
        .collect();
    let kept: Vec<_> = first
        .iter()
        .filter(|coords| resident.contains(coords) && u8::from(coords.z) > 0)
        .collect();
    assert!(
        kept.is_empty(),
        "requests out of view are dropped: {kept:?}"
    );
    let dropped = stalled.dropped.lock().expect("dropped").len();
    assert!(
        dropped >= MAX_TILES_IN_FLIGHT - 1,
        "their fetches stopped: {dropped} dropped"
    );
    assert!(
        stalled.urls.lock().expect("urls").len() > MAX_TILES_IN_FLIGHT,
        "the freed slots fetch the new view"
    );
}
