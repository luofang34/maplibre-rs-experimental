//! Upload failures preserve the committed symbol geometry, atlas and placement features.
#![cfg_attr(test, allow(clippy::expect_used))]
use std::{rc::Rc, sync::Arc};

use super::*;
use crate::{
    coords::{WorldCoords, WorldTileCoords, Zoom},
    plugin::Plugin,
    render::{
        projection::ProjectionGpuResources, settings::BufferPoolSizes, view_state::ViewState,
        RenderPlugin,
    },
    schedule::{Schedule, Stage},
    sdf::{assets::SymbolAtlas, SdfPlugin, SymbolLayersDataComponent},
    tcs::world::World,
    vector::{content, DefaultVectorTransferables, VectorPlugin},
};

async fn context() -> MapContext {
    let (kernel, mut renderer) = crate::headless::create_headless_renderer(16, 16, None)
        .await
        .expect("renderer");
    renderer.settings.symbol_pools = BufferPoolSizes {
        vertices: 64,
        indices: 64,
        feature_metadata: 64,
        layer_metadata: 8,
    };
    let mut world = World::default();
    let mut schedule = Schedule::default();
    let plugins: Vec<Box<dyn Plugin<crate::headless::environment::HeadlessEnvironment>>> = vec![
        Box::new(RenderPlugin),
        Box::new(VectorPlugin::<DefaultVectorTransferables>::default()),
        Box::new(SdfPlugin::<DefaultVectorTransferables>::default()),
    ];
    let kernel = Rc::new(kernel);
    for plugin in plugins {
        plugin
            .build(&mut schedule, kernel.clone(), &mut world, &mut renderer)
            .expect("plugin");
    }
    let style: Style =
        serde_json::from_value(serde_json::json!({"version":8,"sources":{},"layers":[{
            "id":"label","type":"symbol","source-layer":"places", "layout":{"text-field":"X"},
            "paint":{"text-halo-width":["interpolate",["linear"],["zoom"],0,0,4,2]}
        }]}))
        .expect("style");
    let mut context = MapContext {
        renderer,
        world,
        style,
        view_state: ViewState::new(
            crate::window::PhysicalSize::new(16, 16).expect("size"),
            WorldCoords::from((256.0, 256.0)),
            Zoom::new(0.0),
            cgmath::Deg(0.0),
            cgmath::Rad(0.64),
        ),
    };
    schedule.run(&mut context).expect("prepare resources");
    assert!(matches!(
        context
            .world
            .resources
            .get::<Eventually<ProjectionGpuResources>>(),
        Some(Initialized(_))
    ));
    context
        .world
        .tiles
        .spawn_mut(Default::default())
        .expect("tile")
        .insert(SymbolLayersDataComponent::default());
    context
}

fn layer(count: usize, atlas: Arc<SymbolAtlas>, text: &str) -> SymbolLayerData {
    use crate::{
        render::shaders::ShaderSymbolVertex, vector::tessellation::OverAlignedVertexBuffer,
    };
    SymbolLayerData {
        coords: Default::default(),
        source_layer: "places".into(),
        style_layer_id: "label".into(),
        atlas: Some(atlas),
        buffer: OverAlignedVertexBuffer::from_iters(
            std::iter::repeat_n(ShaderSymbolVertex::default(), count),
            [0, 1, 2],
            3,
        ),
        features: vec![crate::sdf::Feature {
            parts: [None; 3],
            data: Default::default(),
            bbox: crate::euclid::Box2D::new(
                crate::euclid::Point2D::zero(),
                crate::euclid::Point2D::new(1.0, 1.0),
            ),
            indices: 0..3,
            text_anchor: crate::euclid::Point2D::zero(),
            anchor_shifts: Vec::new(),
            text_sets: Vec::new(),
            anchor_sets: Vec::new(),
            text_colors: Vec::new(),
            fallback: false,
            repeat_key: None,
            str: text.into(),
            line: None,
        }],
    }
}

fn atlas(pixel: u8) -> Arc<SymbolAtlas> {
    Arc::new(SymbolAtlas {
        pixels: vec![pixel; 16],
        size: [2, 2],
        ..Default::default()
    })
}

fn upload(context: &mut MapContext, zoom: f32) -> Result<(), &'static str> {
    let Some((Initialized(pool), textures, Initialized(pipeline))) =
        context.world.resources.query_mut::<(
            &mut Eventually<SymbolBufferPool>,
            &mut SymbolTextures,
            &Eventually<SymbolPipeline>,
        )>()
    else {
        return Err("missing GPU resources");
    };
    let mut symbol_style = super::SymbolStyleIndex::default();
    symbol_style.sync(&context.style);
    upload_symbol_layer(
        pool,
        textures,
        &TextureContext {
            device: &context.renderer.device,
            queue: &context.renderer.queue,
            atlas_layout: &pipeline.atlas_layout,
        },
        &mut context.world.tiles,
        (
            &context.style,
            &symbol_style,
            &[WorldTileCoords::default()],
            zoom,
        ),
        &mut FrameUploads::default(),
    );
    Ok(())
}

fn binding(context: &MapContext) -> Result<&super::super::textures::DrawBinding, &'static str> {
    context
        .world
        .resources
        .get::<SymbolTextures>()
        .ok_or("missing textures")?
        .binding(Default::default(), "label")
        .ok_or("missing binding")
}

fn allocation(context: &MapContext) -> Result<u64, &'static str> {
    let Some(Initialized(pool)) = context
        .world
        .resources
        .get::<Eventually<SymbolBufferPool>>()
    else {
        return Err("missing symbol buffer pool");
    };
    pool.index()
        .get_layers(Default::default())
        .and_then(|layers| layers.front())
        .map(|entry| entry.allocation_id())
        .ok_or("missing symbol allocation")
}

fn committed(context: &MapContext) -> Result<&SymbolLayerData, &'static str> {
    context
        .world
        .tiles
        .query::<&SymbolLayersDataComponent>(Default::default())
        .and_then(|component| component.layers.first())
        .ok_or("missing committed symbol layer")
}

mod tests;
