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
