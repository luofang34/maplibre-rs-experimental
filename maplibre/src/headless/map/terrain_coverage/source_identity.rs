//! Named raster sources delivered through real worker decoding and GPU upload.
use super::*;
use crate::{
    environment::{OffscreenKernel, OffscreenKernelConfig},
    headless::environment::HeadlessEnvironment,
    io::{
        apc::{tests::reply_context, AsyncProcedureFuture, Input},
        source_client::{HttpClient, HttpSourceClient, SourceClient, SourceFetchError},
    },
    raster::{populate_world_system::PopulateWorldSystem, RasterLayersDataComponent},
    tcs::system::System,
};
use image::ImageEncoder;
use std::sync::Arc;

#[derive(Default)]
struct SourceGate {
    entered: tokio::sync::Notify,
    resume: tokio::sync::Notify,
}

#[derive(Clone)]
struct SourceClientFixture {
    gate_b: Option<Arc<SourceGate>>,
    relief_b: bool,
}

#[cfg_attr(not(feature = "thread-safe-futures"), async_trait::async_trait(?Send))]
#[cfg_attr(feature = "thread-safe-futures", async_trait::async_trait)]
impl HttpClient for SourceClientFixture {
    async fn fetch(&self, url: &str) -> Result<Vec<u8>, SourceFetchError> {
        let is_b = url.contains("/b/");
        if is_b {
            if let Some(gate) = &self.gate_b {
                gate.entered.notify_one();
                gate.resume.notified().await;
            }
        }
        let color = if is_b {
            if self.relief_b {
                [128, 100, 0, 255]
            } else {
                [0, 0, 255, 255]
            }
        } else {
            [0, 255, 0, 255]
        };
        let mut bytes = Vec::new();
        image::codecs::png::PngEncoder::new(&mut bytes)
            .write_image(&color, 1, 1, image::ExtendedColorType::Rgba8)
            .expect("PNG");
        Ok(bytes)
    }
}

struct SourceKernel(SourceClientFixture);
impl OffscreenKernel for SourceKernel {
    type HttpClient = SourceClientFixture;
    fn create(_: OffscreenKernelConfig) -> Self {
        Self(SourceClientFixture {
            gate_b: None,
            relief_b: false,
        })
    }
    fn source_client(&self) -> SourceClient<Self::HttpClient> {
        SourceClient::new(HttpSourceClient::new(self.0.clone()))
    }
}

fn source_style(relief_b: bool) -> Style {
    let mut value = serde_json::to_value(coverage_style(false, false)).expect("style JSON");
    let sources = value["sources"].as_object_mut().expect("sources");
    sources.remove("paint");
    for (id, kind) in [
        ("a", "raster"),
        ("b", if relief_b { "raster-dem" } else { "raster" }),
    ] {
        sources.insert(id.into(), serde_json::json!({
            "type":kind, "tiles":[format!("https://example.invalid/{id}/{{z}}/{{x}}/{{y}}.png")],
            "tileSize":256, "maxzoom":14, "encoding":"terrarium"
        }));
    }
    let b = if relief_b {
        serde_json::json!({"id":"b","type":"color-relief","source":"b",
            "paint":{"color-relief-opacity":0.5,"color-relief-color":
                ["interpolate",["linear"],["elevation"],0,"#ff0000",100,"#0000ff"]}})
    } else {
        serde_json::json!({"id":"b","type":"raster","source":"b"})
    };
    value["layers"] = serde_json::json!([
        {"id":"background","type":"background","paint":{"background-color":"#101010"}},
        {"id":"a","type":"raster","source":"a"}, b
    ]);
    serde_json::from_value(value).expect("named source style")
}

async fn empty_map(relief_b: bool) -> HeadlessMap {
    source_map(relief_b, true).await
}

async fn source_map(relief_b: bool, terrain: bool) -> HeadlessMap {
    let (kernel, renderer) = create_headless_renderer(SIZE, SIZE, None)
        .await
        .expect("renderer");
    let mut style = source_style(relief_b);
    if !terrain {
        style.terrain = None;
    }
    let mut map = HeadlessMap::new(
        style,
        renderer,
        kernel,
        vec![
            Box::new(RenderPlugin),
            Box::new(RasterPlugin::<DefaultRasterTransferables>::default()),
            Box::new(HillshadePlugin),
            Box::new(TerrainPlugin::<DefaultDemTransferables>::default()),
            Box::new(
                HeadlessPlugin::new(false)
                    .preserve_tile_sources()
                    .retain_supplied_tiles(),
            ),
        ],
    )
    .expect("map");
    map.load_dem_tiles(vec![(
        target(),
        RgbaImage::from_pixel(256, 256, Rgba([128, 0, 0, 255])),
    )])
    .expect("terrain");
    map
}

fn prepare_pending(map: &mut HeadlessMap) {
    map.map_context
        .world
        .tiles
        .spawn_mut(target())
        .expect("tile")
        .insert(RasterLayersDataComponent::default());
}

fn source_request(
    map: &HeadlessMap,
    relief_b: bool,
    gate_b: Option<Arc<SourceGate>>,
) -> AsyncProcedureFuture {
    crate::raster::request_system::fetch_raster_apc::<_, DefaultRasterTransferables, _>(
        Input::TileRequest {
            coords: target(),
            style: map.map_context.style.clone(),
        },
        reply_context(map.kernel.apc()),
        SourceKernel(SourceClientFixture { gate_b, relief_b }),
    )
}

fn apply_worker_sources(map: &mut HeadlessMap) {
    PopulateWorldSystem::<HeadlessEnvironment, DefaultRasterTransferables>::new(&map.kernel)
        .run(&mut map.map_context)
        .expect("apply worker replies");
    map.run_frame().expect("worker pixels");
}

async fn deliver_worker_sources(map: &mut HeadlessMap, relief_b: bool) {
    prepare_pending(map);
    source_request(map, relief_b, None).await.expect("worker");
    apply_worker_sources(map);
}

mod tests;
