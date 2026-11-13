#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::{CustomLayerContext, CustomLayerRegistry, CustomStyleLayer};
use crate::style::Style;

struct DummyCustomLayer {
    id: String,
}

impl CustomStyleLayer for DummyCustomLayer {
    fn id(&self) -> &str {
        &self.id
    }

    fn render<'pass>(
        &'pass self,
        _pass: &mut wgpu::RenderPass<'pass>,
        _ctx: &CustomLayerContext<'_>,
    ) {
    }
}

#[test]
fn style_parses_validates_and_mutates_custom_layer_without_source() {
    let mut style: Style = serde_json::from_value(serde_json::json!({
        "version": 8,
        "sources": {},
        "layers": [
            { "id": "bg", "type": "background", "paint": { "background-color": "#112233" } },
            { "id": "deck-overlay", "type": "custom", "minzoom": 2.0, "maxzoom": 18.0 }
        ]
    }))
    .expect("style with custom layer deserializes");

    assert_eq!(style.validate(), Ok(()));
    assert_eq!(style.layers[1].type_, "custom");
    assert!(style.layers[1].source.is_none());

    let change = style
        .add_layer(
            serde_json::json!({
                "id": "second-custom",
                "type": "custom"
            }),
            Some("deck-overlay"),
        )
        .expect("adding custom layer without source succeeds");
    assert!(!change.redraw_tiles);
    assert_eq!(style.layers[1].id, "second-custom");
    assert_eq!(style.layers[2].id, "deck-overlay");

    let serialized = serde_json::to_value(&style.layers[1]).expect("serializes");
    assert_eq!(serialized["type"], "custom");
}

#[test]
fn custom_layer_registry_insert_and_remove() {
    let mut registry = CustomLayerRegistry::new();
    assert!(!registry.contains("weather"));
    registry.register(DummyCustomLayer {
        id: "weather".into(),
    });
    assert!(registry.contains("weather"));
    assert!(registry.remove("weather"));
    assert!(!registry.contains("weather"));
    assert!(!registry.remove("weather"));
}

#[cfg(all(
    feature = "headless",
    feature = "geojson",
    feature = "thread-safe-futures"
))]
mod gpu_tests {
    use std::sync::{Arc, Mutex};

    use super::*;
    use crate::{
        background::BackgroundPlugin,
        headless::{
            create_headless_renderer,
            map::{
                process_geojson_layers,
                readback::{FrameReadback, PixelLayout},
                HeadlessMap,
            },
        },
        projection::ProjectionType,
        render::{upload_queue::UploadQueue, RenderPlugin},
        vector::{DefaultVectorTransferables, VectorPlugin},
    };

    const FULLSCREEN_GREEN_WGSL: &str = r#"
        @vertex
        fn vs_main(@builtin(vertex_index) idx: u32) -> @builtin(position) vec4<f32> {
            var pos = array<vec2<f32>, 3>(
                vec2<f32>(-1.0, -1.0),
                vec2<f32>( 3.0, -1.0),
                vec2<f32>(-1.0,  3.0),
            );
            return vec4<f32>(pos[idx], 0.0, 1.0);
        }

        @fragment
        fn fs_main() -> @location(0) vec4<f32> {
            return vec4<f32>(0.0, 1.0, 0.0, 1.0);
        }
    "#;

    struct FullscreenGreenLayer {
        calls: Arc<Mutex<Vec<&'static str>>>,
        observed_zoom: Arc<Mutex<Option<f64>>>,
        pipeline: Option<wgpu::RenderPipeline>,
        pending_format: Option<(wgpu::TextureFormat, u32)>,
    }

    impl CustomStyleLayer for FullscreenGreenLayer {
        fn id(&self) -> &str {
            "mid-custom"
        }

        fn initialize(
            &mut self,
            _device: &wgpu::Device,
            _queue: &UploadQueue,
            color_format: wgpu::TextureFormat,
            sample_count: u32,
        ) {
            self.calls.lock().expect("lock").push("initialize");
            self.pending_format = Some((color_format, sample_count));
            self.pipeline = None;
        }

        fn prepare(&mut self, ctx: &CustomLayerContext<'_>) {
            self.calls.lock().expect("lock").push("prepare");
            *self.observed_zoom.lock().expect("lock") = Some(ctx.zoom);
            if self.pipeline.is_none() {
                let (color_format, sample_count) = self
                    .pending_format
                    .unwrap_or((ctx.color_format, ctx.sample_count));
                let shader = ctx
                    .device
                    .create_shader_module(wgpu::ShaderModuleDescriptor {
                        label: Some("custom_green_shader"),
                        source: wgpu::ShaderSource::Wgsl(FULLSCREEN_GREEN_WGSL.into()),
                    });
                let layout = ctx
                    .device
                    .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                        label: Some("custom_green_layout"),
                        bind_group_layouts: &[],
                        immediate_size: 0,
                    });
                let pipeline = ctx
                    .device
                    .create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                        label: Some("custom_green_pipeline"),
                        layout: Some(&layout),
                        vertex: wgpu::VertexState {
                            module: &shader,
                            entry_point: Some("vs_main"),
                            compilation_options: wgpu::PipelineCompilationOptions::default(),
                            buffers: &[],
                        },
                        fragment: Some(wgpu::FragmentState {
                            module: &shader,
                            entry_point: Some("fs_main"),
                            compilation_options: wgpu::PipelineCompilationOptions::default(),
                            targets: &[Some(wgpu::ColorTargetState {
                                format: color_format,
                                blend: None,
                                write_mask: wgpu::ColorWrites::ALL,
                            })],
                        }),
                        primitive: wgpu::PrimitiveState::default(),
                        depth_stencil: Some(ctx.painter_depth_stencil_state()),
                        multisample: wgpu::MultisampleState {
                            count: sample_count,
                            mask: !0,
                            alpha_to_coverage_enabled: false,
                        },
                        multiview_mask: None,
                        cache: None,
                    });
                self.pipeline = Some(pipeline);
            }
        }

        fn render<'pass>(
            &'pass self,
            pass: &mut wgpu::RenderPass<'pass>,
            _ctx: &CustomLayerContext<'_>,
        ) {
            self.calls.lock().expect("lock").push("render");
            if let Some(pipeline) = &self.pipeline {
                pass.set_pipeline(pipeline);
                pass.draw(0..3, 0..1);
            }
        }
    }

    #[tokio::test]
    async fn custom_layer_executes_in_exact_painter_z_order_between_style_layers() {
        // Layer 0: red background (#ff0000)
        // Layer 1: custom fullscreen green (#00ff00) — must overwrite red background!
        // Layer 2: blue GeoJSON fill covering ONLY the eastern half (lon 0..180, lat -80..80) —
        //          must draw AFTER custom green so right half is blue and left half stays green!
        let geojson = serde_json::json!({
            "type": "FeatureCollection",
            "features": [{
                "type": "Feature",
                "geometry": {
                    "type": "Polygon",
                    "coordinates": [[
                        [0.0, -80.0],
                        [170.0, -80.0],
                        [170.0, 80.0],
                        [0.0, 80.0],
                        [0.0, -80.0]
                    ]]
                },
                "properties": {}
            }]
        });
        let style: Style = serde_json::from_value(serde_json::json!({
            "version": 8,
            "center": [0.0, 0.0],
            "zoom": 1.0,
            "sources": {
                "east": { "type": "geojson", "data": geojson }
            },
            "layers": [
                { "id": "bg", "type": "background", "paint": { "background-color": "#ff0000" } },
                { "id": "mid-custom", "type": "custom" },
                { "id": "top-east", "type": "fill", "source": "east", "paint": { "fill-color": "#0000ff" } }
            ]
        }))
        .expect("valid style");

        let (kernel, renderer) = create_headless_renderer(128, 128, None)
            .await
            .expect("headless renderer");
        let mut map = HeadlessMap::new(
            style.clone(),
            renderer,
            kernel,
            vec![
                Box::new(RenderPlugin),
                Box::new(BackgroundPlugin),
                Box::new(VectorPlugin::<DefaultVectorTransferables>::default()),
            ],
        )
        .expect("headless map");

        let calls: Arc<Mutex<Vec<&'static str>>> = Arc::default();
        let observed_zoom: Arc<Mutex<Option<f64>>> = Arc::default();
        map.register_custom_layer(FullscreenGreenLayer {
            calls: Arc::clone(&calls),
            observed_zoom: Arc::clone(&observed_zoom),
            pipeline: None,
            pending_format: None,
        });

        // Process and render the visible tiles at z=1 so `top-east` covers the right half!
        let coords = map.required_tile_coords().expect("tile coords");
        let fill_layers: Vec<_> = style
            .layers
            .iter()
            .filter(|l| l.id == "top-east")
            .cloned()
            .collect();
        for coord in coords {
            let processed = process_geojson_layers(
                &geojson,
                "east",
                fill_layers.clone(),
                coord,
                ProjectionType::Mercator,
            )
            .expect("process geojson");
            map.render_tile(processed).expect("render tile");
        }
        // Run one more frame and inspect both lifecycle call order and output pixels.
        calls.lock().expect("lock").clear();
        map.run_frame().expect("frame succeeds");

        assert_eq!(
            *calls.lock().expect("lock"),
            vec!["prepare", "render"],
            "initialize ran once on first frame; subsequent frames run prepare -> render"
        );
        assert_eq!(*observed_zoom.lock().expect("lock"), Some(1.0));

        let mut readback = FrameReadback::new(&map, 128, 128);
        let mut rgba = vec![0_u8; readback.frame_len(PixelLayout::Rgba8)];
        readback
            .read_into_blocking(
                &map,
                map.head_texture().expect("head texture"),
                PixelLayout::Rgba8,
                &mut rgba,
            )
            .expect("readback");

        let pixel = |x: u32, y: u32| -> [u8; 4] {
            let idx = ((y * 128 + x) * 4) as usize;
            [rgba[idx], rgba[idx + 1], rgba[idx + 2], rgba[idx + 3]]
        };

        // Left half (x = 24, y = 64): uncovered by `top-east`, so `mid-custom` green (#00ff00)
        // must have overwritten `bg` red (#ff0000)!
        let left_px = pixel(24, 64);
        assert!(
            left_px[0] < 20 && left_px[1] > 235 && left_px[2] < 20,
            "expected left half to show custom green (#00ff00) over red background, got {left_px:?}"
        );

        // Right half (x = 84, y = 64): covered by `top-east`, so `top-east` blue (#0000ff)
        // must have drawn AFTER `mid-custom` green!
        let right_px = pixel(84, 64);
        assert!(
            right_px[0] < 20 && right_px[1] < 20 && right_px[2] > 235,
            "expected right half to show top-east blue (#0000ff) over custom green, got {right_px:?}"
        );
    }
}
