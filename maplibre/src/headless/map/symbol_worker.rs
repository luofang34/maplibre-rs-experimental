//! Labels drawn through the real tile worker from the glyphs, sprites and tiles a host's loader
//! serves, so a test sees which assets a style's symbols asked for and what reached the screen.
#![allow(clippy::expect_used, clippy::panic)]

use std::sync::{Arc, Mutex};

use super::HeadlessMap;
use crate::{
    headless::create_headless_renderer_with_loader,
    io::{
        resource_loader::SharedLoader,
        source_client::{HttpClient, SourceFetchError},
    },
    render::{frame_signals::ResourceReady, RenderPlugin},
    sdf::SdfPlugin,
    style::Style,
    vector::{DefaultVectorTransferables, VectorPlugin},
};

mod curve;
mod geojson;
mod globe_precision;
mod horizon;
mod motion;
mod mvt;
mod provided;
mod retry;

/// The side of the square viewport, in physical pixels.
pub(super) const SIZE: u32 = 512;

/// The glyph template the fixtures' styles use.
pub(super) const GLYPHS: &str = "https://glyphs.test/{fontstack}/{range}.pbf";
/// The sprite the fixtures' styles use.
pub(super) const SPRITE: &str = "https://sprites.test/sprite";

/// A font that is not the bundled fallback, with Latin and Cyrillic ranges.
pub(super) const FONT: &str = "Noto Sans Regular";

/// The colour of the sprite's `marker` icon.
pub(super) const MARKER: [u8; 3] = [0, 255, 0];
/// The colour of the sprite's `shield` icon, which stretches around its text.
pub(super) const SHIELD: [u8; 3] = [255, 255, 0];

/// Bodies served for every URL that starts with their prefix.
type Served = Arc<Mutex<Vec<(String, Vec<u8>)>>>;

/// A glyph range held back, and the gate that lets its requests through.
type Hold = Arc<Mutex<Option<(&'static str, Arc<tokio::sync::Semaphore>)>>>;

/// Serves the fixture font's ranges, a sprite sheet and vector tiles, recording every URL.
#[derive(Clone, Default)]
pub(super) struct AssetServer {
    urls: Arc<Mutex<Vec<String>>>,
    tiles: Served,
    /// A glyph range that fails as a dropped connection would, until cleared.
    outage: Arc<Mutex<Option<&'static str>>>,
    /// A glyph range whose requests wait until the gate opens.
    held: Hold,
}

#[cfg_attr(not(feature = "thread-safe-futures"), async_trait::async_trait(?Send))]
#[cfg_attr(feature = "thread-safe-futures", async_trait::async_trait)]
impl HttpClient for AssetServer {
    async fn fetch(&self, url: &str) -> Result<Vec<u8>, SourceFetchError> {
        self.urls.lock().expect("urls").push(url.to_owned());
        let font = "https://glyphs.test/Noto%20Sans%20Regular/";
        let gate = self
            .held
            .lock()
            .expect("held")
            .as_ref()
            .filter(|(range, _)| url.starts_with("https://glyphs.test/") && url.contains(range))
            .map(|(_, gate)| gate.clone());
        if let Some(gate) = gate {
            gate.acquire().await.ok();
        }
        if let Some(range) = *self.outage.lock().expect("outage") {
            if url.starts_with("https://glyphs.test/") && url.contains(range) {
                return Err(SourceFetchError::temporary(std::io::Error::other("reset")));
            }
        }
        match url {
            _ if url == format!("{font}0-255.pbf") => Ok(latin()),
            _ if url == format!("{font}1024-1279.pbf") => Ok(include_bytes!(
                "../../../../render-tests/src/assets/glyphs/Noto Sans Regular/1024-1279.pbf"
            )
            .to_vec()),
            _ if url == format!("{font}256-511.pbf") => Ok(latin_extended()),
            "https://sprites.test/sprite.json" => Ok(br#"{
                "marker":{"x":0,"y":0,"width":16,"height":16,"pixelRatio":1},
                "shield":{"x":16,"y":0,"width":16,"height":16,"pixelRatio":1,
                    "stretchX":[[4,12]],"stretchY":[[4,12]],"content":[4,4,12,12]}
            }"#
            .to_vec()),
            "https://sprites.test/sprite.png" => Ok(sprite_sheet()),
            _ => self
                .tiles
                .lock()
                .expect("tiles")
                .iter()
                .find(|(prefix, _)| url.starts_with(prefix.as_str()))
                .map(|(_, tile)| tile.clone())
                .ok_or_else(|| SourceFetchError::not_found(url)),
        }
    }
}

/// The extra advance, in glyph pixels, of every glyph of the served Latin range.
pub(super) const LATIN_SPACING: i32 = 2;

/// The font's 0-255 range: Noto's glyphs, each advancing [`LATIN_SPACING`] further. The bundled
/// fallback range has the very same Noto glyphs, so only the spacing tells a label drawn from
/// the served range from one the fallback would draw.
pub(super) fn latin() -> Vec<u8> {
    use prost::Message as _;
    let mut glyphs =
        crate::sdf::glyphs::Glyphs::decode(
            &include_bytes!(
                "../../../../render-tests/src/assets/glyphs/Noto Sans Regular/0-255.pbf"
            )[..],
        )
        .expect("Noto's Latin range");
    for glyph in glyphs.stacks.iter_mut().flat_map(|stack| &mut stack.glyphs) {
        glyph.advance = glyph.advance.saturating_add_signed(LATIN_SPACING);
    }
    glyphs.encode_to_vec()
}

/// The font's 256-511 range, with one glyph, a solid block for `Ā`.
fn latin_extended() -> Vec<u8> {
    use prost::Message as _;
    let (width, height) = (14, 18);
    crate::sdf::glyphs::Glyphs {
        stacks: vec![crate::sdf::glyphs::Fontstack {
            name: FONT.into(),
            range: "256-511".into(),
            glyphs: vec![crate::sdf::glyphs::Glyph {
                id: u32::from('Ā'),
                bitmap: Some(vec![255; ((width + 6) * (height + 6)) as usize]),
                width,
                height,
                left: 1,
                top: -6,
                advance: 16,
            }],
        }],
    }
    .encode_to_vec()
}

fn sprite_sheet() -> Vec<u8> {
    let sheet = image::RgbaImage::from_fn(32, 16, |x, _| {
        let [red, green, blue] = if x < 16 { MARKER } else { SHIELD };
        image::Rgba([red, green, blue, 255])
    });
    let mut png = std::io::Cursor::new(Vec::new());
    sheet
        .write_to(&mut png, image::ImageFormat::Png)
        .expect("PNG");
    png.into_inner()
}

impl AssetServer {
    /// Answers every URL starting with `prefix` with `body`.
    pub(super) fn serve(&self, prefix: &str, body: Vec<u8>) {
        self.tiles
            .lock()
            .expect("tiles")
            .push((prefix.to_owned(), body));
    }

    /// Makes `range`'s glyph requests fail temporarily, or, with `None`, succeed again.
    pub(super) fn fail_glyphs(&self, range: Option<&'static str>) {
        *self.outage.lock().expect("outage") = range;
    }

    /// Holds `range`'s glyph requests until the returned gate is closed.
    pub(super) fn hold_glyphs(&self, range: &'static str) -> Arc<tokio::sync::Semaphore> {
        let gate = Arc::new(tokio::sync::Semaphore::new(0));
        *self.held.lock().expect("held") = Some((range, gate.clone()));
        gate
    }

    /// Every URL fetched so far.
    pub(super) fn requested(&self) -> Vec<String> {
        self.urls.lock().expect("urls").clone()
    }

    /// The glyph URLs fetched so far.
    pub(super) fn glyph_requests(&self) -> Vec<String> {
        let mut glyphs: Vec<_> = self
            .requested()
            .into_iter()
            .filter(|url| url.starts_with("https://glyphs.test/"))
            .collect();
        glyphs.sort();
        glyphs.dedup();
        glyphs
    }
}

/// A map whose tiles and symbol assets come through the worker from an [`AssetServer`].
pub(super) struct SymbolMap {
    pub(super) map: HeadlessMap,
    pub(super) server: AssetServer,
    /// Whether any tile's labels have reached the GPU.
    labels_ready: bool,
}

impl SymbolMap {
    pub(super) async fn new(style: Style) -> Self {
        Self::serving(style, AssetServer::default()).await
    }

    /// A map fetching from `server`, which may already serve tiles.
    pub(super) async fn serving(style: Style, server: AssetServer) -> Self {
        let (kernel, renderer) = create_headless_renderer_with_loader(
            SIZE,
            SIZE,
            Default::default(),
            SharedLoader::new(server.clone()),
        )
        .await
        .expect("renderer");
        let map = HeadlessMap::new(
            style,
            renderer,
            kernel,
            vec![
                Box::new(RenderPlugin),
                Box::new(crate::background::BackgroundPlugin),
                Box::new(crate::terrain::TerrainPlugin::<
                    crate::terrain::DefaultDemTransferables,
                >::default()),
                Box::new(VectorPlugin::<DefaultVectorTransferables>::default()),
                Box::new(SdfPlugin::<DefaultVectorTransferables>::default()),
            ],
        )
        .expect("map");
        Self {
            map,
            server,
            labels_ready: false,
        }
    }

    /// Draws frames 16 ms apart until labels have reached the GPU and then frames pass without
    /// a request, a delivery or a fade, and returns the settled frame's RGBA pixels.
    pub(super) async fn settle(&mut self) -> Vec<u8> {
        let mut quiet = 0;
        for _ in 0..600 {
            let requests = self.server.requested().len();
            let ready = self.frame().await;
            let idle = ready.is_empty()
                && !self.map.needs_redraw()
                && self.server.requested().len() == requests;
            quiet = if idle { quiet + 1 } else { 0 };
            if self.labels_ready && quiet >= 10 {
                return self.read();
            }
        }
        panic!(
            "the labels never settled: labels ready {}, requested {:?}",
            self.labels_ready,
            self.server.requested()
        );
    }

    /// Draws one frame 16 ms after the last, lets the workers run, and returns what became
    /// ready.
    pub(super) async fn frame(&mut self) -> Vec<ResourceReady> {
        // Labels fade and retries wait by the host's clock, which a host advances by its frame
        // interval.
        self.map.frame_input_mut().timestamp += std::time::Duration::from_millis(16);
        self.map.run_frame().expect("frame");
        for _ in 0..8 {
            tokio::task::yield_now().await;
        }
        let ready = self.map.take_ready_resources();
        self.labels_ready |= ready
            .iter()
            .any(|ready| matches!(ready, ResourceReady::SymbolAtlas { .. }));
        ready
    }

    /// The RGBA pixels of the last frame.
    pub(super) fn read(&self) -> Vec<u8> {
        let texture = self.map.head_texture().expect("color");
        let buffer = self.map.device().create_buffer(&wgpu::BufferDescriptor {
            label: Some("symbol worker pixels"),
            size: u64::from(SIZE * SIZE * 4),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = self
            .map
            .device()
            .create_command_encoder(&Default::default());
        encoder.copy_texture_to_buffer(
            texture.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(SIZE * 4),
                    rows_per_image: None,
                },
            },
            texture.size(),
        );
        self.map.queue().submit([encoder.finish()]);
        buffer
            .slice(..)
            .map_async(wgpu::MapMode::Read, |result| result.expect("readback"));
        self.map
            .device()
            .poll(wgpu::PollType::wait_indefinitely())
            .expect("GPU readback completes");
        let bytes = buffer
            .slice(..)
            .get_mapped_range()
            .expect("mapped readback range")
            .to_vec();
        buffer.unmap();
        bytes
    }
}

/// The pixels within `region` (`[left, top, right, bottom]`, exclusive) close to `color`.
pub(super) fn count(pixels: &[u8], color: [u8; 3], region: [u32; 4]) -> Vec<[u32; 2]> {
    let mut found = Vec::new();
    for y in region[1]..region[3] {
        for x in region[0]..region[2] {
            let index = ((y * SIZE + x) * 4) as usize;
            let pixel = &pixels[index..index + 3];
            if pixel
                .iter()
                .zip(color)
                .all(|(have, want)| have.abs_diff(want) < 60)
            {
                found.push([x, y]);
            }
        }
    }
    found
}
