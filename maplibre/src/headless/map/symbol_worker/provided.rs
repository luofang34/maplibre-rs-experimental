//! Road shields made on request by a provider, through the real tile worker: drawn first as
//! the style's fallback, replaced when they arrive, once per route however many tiles show it,
//! for the display's pixel ratio, and never by an answer to a style that has since changed.

mod arrival;
mod cost;
mod formatted;
mod placement;

use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc, Mutex,
};

use geozero::mvt::{tile, Message as _};

use super::{count, AssetServer, SymbolMap, MARKER, SIZE, SPRITE};
use crate::{
    sdf::assets::{
        ImageProviderError, ImageRequest, ImageResolution, ProvideFuture, ProvidedImage,
        ProviderLimits, StyleImageProvider,
    },
    style::{Style, StyleImage},
};

pub(super) const WHOLE: [u32; 4] = [0, 0, SIZE, SIZE];
/// The colour of a provided shield.
pub(super) const SHIELD: [u8; 3] = [255, 0, 255];
/// The colour of a provided shield from a newer pack.
pub(super) const NEW_SHIELD: [u8; 3] = [0, 255, 255];
/// The colour of the banner above a bannered shield's body.
pub(super) const BANNER: [u8; 3] = [255, 128, 0];
/// The side of a provided shield, in layout pixels.
pub(super) const SIDE: f32 = 20.0;

/// How the fake answers.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Answer {
    Shield([u8; 3]),
    /// A 20 x 30 picture: a 10-pixel banner above a 20 x 20 body.
    Bannered,
    Absent,
    Failed,
    /// Unavailable for the first this many calls, then a shield.
    UnavailableFor(usize),
}

/// A provider that draws solid squares, counting its calls.
pub(super) struct Shields {
    pub(super) answer: Mutex<Answer>,
    pub(super) generation: Mutex<String>,
    pub(super) calls: AtomicUsize,
    pub(super) requests: Mutex<Vec<ImageRequest>>,
    pub(super) gate: Option<Arc<tokio::sync::Semaphore>>,
    /// Set while the test draws a frame; a call made then would block drawing.
    pub(super) drawing: Arc<std::sync::atomic::AtomicBool>,
    /// Calls made while a frame was drawn.
    pub(super) during_frames: AtomicUsize,
}

impl Shields {
    pub(super) fn new(answer: Answer) -> Arc<Self> {
        Self::gated(answer, None)
    }

    pub(super) fn held(answer: Answer) -> (Arc<Self>, Arc<tokio::sync::Semaphore>) {
        let gate = Arc::new(tokio::sync::Semaphore::new(0));
        (Self::gated(answer, Some(gate.clone())), gate)
    }

    pub(super) fn gated(answer: Answer, gate: Option<Arc<tokio::sync::Semaphore>>) -> Arc<Self> {
        Arc::new(Self {
            answer: Mutex::new(answer),
            generation: Mutex::new("pack-1".into()),
            calls: AtomicUsize::new(0),
            requests: Mutex::default(),
            gate,
            drawing: Arc::default(),
            during_frames: AtomicUsize::new(0),
        })
    }

    pub(super) fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }

    pub(super) fn pixel_ratios(&self) -> Vec<f32> {
        let mut ratios: Vec<f32> = self
            .requests
            .lock()
            .expect("requests")
            .iter()
            .map(|request| request.pixel_ratio)
            .collect();
        ratios.dedup();
        ratios
    }
}

pub(super) fn square(side: u32, color: [u8; 3], pixel_ratio: f32) -> StyleImage {
    StyleImage {
        width: side,
        height: side,
        data: [color[0], color[1], color[2], 255].repeat((side * side) as usize),
        pixel_ratio,
        sdf: false,
    }
}

impl StyleImageProvider for Shields {
    fn generation(&self) -> String {
        self.generation.lock().expect("generation").clone()
    }

    fn provide(&self, request: ImageRequest) -> ProvideFuture<'_> {
        Box::pin(async move {
            let call = self.calls.fetch_add(1, Ordering::SeqCst);
            if self.drawing.load(Ordering::SeqCst) {
                self.during_frames.fetch_add(1, Ordering::SeqCst);
            }
            self.requests
                .lock()
                .expect("requests")
                .push(request.clone());
            if let Some(gate) = &self.gate {
                gate.acquire().await.expect("gate").forget();
            }
            let ratio = request.pixel_ratio;
            let side = (SIDE * ratio).round() as u32;
            let shield = |color| {
                Ok(ImageResolution::Image(ProvidedImage {
                    image: square(side, color, ratio),
                    anchor: None,
                }))
            };
            match *self.answer.lock().expect("answer") {
                Answer::Shield(color) => shield(color),
                Answer::UnavailableFor(times) if call < times => {
                    Err(ImageProviderError::Unavailable("pack downloading".into()))
                }
                Answer::UnavailableFor(_) => shield(SHIELD),
                Answer::Absent => Ok(ImageResolution::Absent),
                Answer::Failed => Err(ImageProviderError::Failed("malformed ref".into())),
                Answer::Bannered => {
                    let banner = (10.0 * ratio).round() as u32;
                    let mut image = square(side, SHIELD, ratio);
                    image.height = side + banner;
                    let mut data =
                        [BANNER[0], BANNER[1], BANNER[2], 255].repeat((side * banner) as usize);
                    data.extend(image.data);
                    image.data = data;
                    Ok(ImageResolution::Image(ProvidedImage {
                        image,
                        anchor: Some([side as f32 / 2.0, banner as f32 + side as f32 / 2.0]),
                    }))
                }
            }
        })
    }
}

/// A road across every tile's middle, carrying one route.
pub(super) fn road_tile(network: &str, reference: &str) -> Vec<u8> {
    let string = |value: &str| tile::Value {
        string_value: Some(value.to_owned()),
        ..Default::default()
    };
    geozero::mvt::Tile {
        layers: vec![tile::Layer {
            version: 2,
            name: "roads".into(),
            extent: Some(4096),
            keys: vec!["network".into(), "ref".into()],
            values: vec![string(network), string(reference)],
            features: vec![tile::Feature {
                r#type: Some(tile::GeomType::Linestring as i32),
                tags: vec![0, 0, 1, 1],
                geometry: vec![9, 0, 4096, 10, 8192, 0],
                ..Default::default()
            }],
        }],
    }
    .encode_to_vec()
}

/// Shields along the road, or `point` ones on a single spot at the view's centre, falling back
/// to the sprite's marker.
pub(super) fn style(placement: &str, globe: bool) -> Style {
    let mut style = serde_json::json!({
        "version":8,"center":[0.01,0.010986328],"zoom":14,"sprite":SPRITE,
        "sources":{
            "streets":{"type":"vector","tiles":["https://tiles.test/{z}/{x}/{y}.pbf"],"maxzoom":14},
            "spot":{"type":"geojson","data":{"type":"Feature",
                "properties":{"network":"US:I","ref":"287"},
                "geometry":{"type":"Point","coordinates":[0.01,0.010986328]}}}
        },
        "layers":[
            {"id":"background","type":"background","paint":{"background-color":"#223344"}},
            {"id":"road","type":"line","source":"streets","source-layer":"roads",
                "paint":{"line-color":"#808080","line-width":4}}
        ]
    });
    let icon = serde_json::json!([
        "coalesce",
        [
            "image",
            ["concat", "shield:", ["get", "network"], "=", ["get", "ref"]]
        ],
        ["image", "marker"]
    ]);
    let layer = if placement == "point" {
        serde_json::json!({"id":"shield","type":"symbol","source":"spot",
            "layout":{"icon-image":icon,"icon-allow-overlap":true}})
    } else {
        serde_json::json!({"id":"shield","type":"symbol","source":"streets","source-layer":"roads",
            "layout":{"symbol-placement":"line","symbol-spacing":200,"icon-image":icon,
                "icon-rotation-alignment":"viewport","icon-allow-overlap":true}})
    };
    style["layers"].as_array_mut().expect("layers").push(layer);
    if globe {
        style["projection"] = serde_json::json!({"type":"vertical-perspective"});
    }
    serde_json::from_value(style).expect("style")
}

pub(super) async fn shield_map(placement: &str, provider: Arc<Shields>) -> SymbolMap {
    shield_map_on(placement, false, provider).await
}

pub(super) async fn shield_map_on(
    placement: &str,
    globe: bool,
    provider: Arc<Shields>,
) -> SymbolMap {
    let server = AssetServer::default();
    server.serve("https://tiles.test/", road_tile("US:I", "287"));
    let map = SymbolMap::serving(style(placement, globe), server).await;
    map.map
        .image_providers()
        .expect("in-process workers share the registry")
        .register("shield", provider);
    map
}

/// Draws a frame with `shields` told that it is drawing, then lets the workers run.
pub(super) async fn watched_frame(map: &mut SymbolMap, shields: &Shields) {
    map.map.frame_input_mut().timestamp += std::time::Duration::from_millis(16);
    shields.drawing.store(true, Ordering::SeqCst);
    map.map.run_frame().expect("frame");
    shields.drawing.store(false, Ordering::SeqCst);
    for _ in 0..8 {
        tokio::task::yield_now().await;
    }
}

/// Draws frames until `drawn` holds for the frame's pixels, which it must within 40 seconds
/// of the map's clock: long enough for a few retry back-offs.
pub(super) async fn frames_until(
    map: &mut SymbolMap,
    what: &str,
    drawn: impl Fn(&[u8]) -> bool,
) -> Vec<u8> {
    for _ in 0..2500 {
        map.frame().await;
        let pixels = map.read();
        if drawn(&pixels) {
            return pixels;
        }
    }
    panic!(
        "{what} never drawn; provider stats {:?}",
        map.map.image_providers().map(|p| p.stats())
    );
}

pub(super) fn shown(pixels: &[u8], color: [u8; 3]) -> usize {
    count(pixels, color, WHOLE).len()
}
