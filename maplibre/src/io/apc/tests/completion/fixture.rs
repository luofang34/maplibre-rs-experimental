use std::sync::{Arc, Mutex};

use geozero::mvt::{tile, Message as _, Tile};
use image::ImageEncoder;

use crate::{
    environment::{OffscreenKernel, OffscreenKernelConfig},
    io::{
        apc::{Context, IntoMessage, Message, SendError},
        source_client::{HttpClient, HttpSourceClient, SourceClient, SourceFetchError},
    },
    style::Style,
};

#[derive(Clone, Default)]
pub(super) struct Replies {
    pub messages: Arc<Mutex<Vec<Message>>>,
    pub reject: bool,
    pub attempts: Arc<std::sync::atomic::AtomicUsize>,
}

impl Context for Replies {
    fn send_back<T: IntoMessage>(&self, message: T) -> Result<(), SendError> {
        self.attempts
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if self.reject {
            return Err(SendError::Transmission {
                operation: "delivering test result",
                source: Box::new(std::io::Error::new(
                    std::io::ErrorKind::BrokenPipe,
                    "receiver gone",
                )),
            });
        }
        self.messages
            .lock()
            .expect("reply queue")
            .push(message.into());
        Ok(())
    }
}

#[derive(Default)]
pub(super) struct FetchGate {
    pub entered: tokio::sync::Notify,
    pub resume: tokio::sync::Notify,
}

#[derive(Clone, Default)]
pub(super) struct TileClient {
    pub gate: Option<Arc<FetchGate>>,
}

#[cfg_attr(not(feature = "thread-safe-futures"), async_trait::async_trait(?Send))]
#[cfg_attr(feature = "thread-safe-futures", async_trait::async_trait)]
impl HttpClient for TileClient {
    async fn fetch(&self, url: &str) -> Result<Vec<u8>, SourceFetchError> {
        if url.contains("/blocked/") {
            if let Some(gate) = &self.gate {
                gate.entered.notify_one();
                gate.resume.notified().await;
            }
        }
        if url.contains("/bad/") {
            return Ok(vec![255]);
        }
        if url.contains("/missing/") {
            return Err(SourceFetchError::not_found(url));
        }
        if url.contains("/empty/") {
            return Ok(Vec::new());
        }
        if url.contains("/image/") {
            let mut bytes = Vec::new();
            image::codecs::png::PngEncoder::new(&mut bytes)
                .write_image(&[20, 100, 220, 255], 1, 1, image::ExtendedColorType::Rgba8)
                .expect("PNG");
            return Ok(bytes);
        }
        Ok(Tile {
            layers: vec![tile::Layer {
                version: 2,
                name: "roads".into(),
                extent: Some(4096),
                features: vec![tile::Feature {
                    id: Some(42),
                    r#type: Some(tile::GeomType::Linestring as i32),
                    geometry: vec![9, 2, 2, 10, 2, 2],
                    ..Default::default()
                }],
                ..Default::default()
            }],
        }
        .encode_to_vec())
    }
}

#[derive(Default)]
pub(super) struct TileKernel(pub TileClient);

impl OffscreenKernel for TileKernel {
    type HttpClient = TileClient;
    fn create(_: OffscreenKernelConfig) -> Self {
        Self::default()
    }
    fn source_client(&self) -> SourceClient<Self::HttpClient> {
        SourceClient::new(HttpSourceClient::new(self.0.clone()))
    }
}

pub(super) fn style(raster: bool, paths: &[&str]) -> Style {
    let mut sources = serde_json::Map::new();
    let mut layers = Vec::new();
    for (index, path) in paths.iter().enumerate() {
        let id = format!("source-{index}");
        sources.insert(
            id.clone(),
            serde_json::json!({
                "type": if raster { "raster" } else { "vector" },
                "tiles": [format!("https://example.invalid/{path}/{{z}}/{{x}}/{{y}}")],
            }),
        );
        layers.push(serde_json::json!({
            "id": id, "source": id, "source-layer": "roads",
            "type": if raster { "raster" } else { "line" },
            "paint": if raster { serde_json::json!({}) } else { serde_json::json!({"line-color": "#0066cc"}) },
        }));
    }
    serde_json::from_value(serde_json::json!({"version": 8, "sources": sources, "layers": layers}))
        .expect("style")
}
