//! Upload failures preserve the committed symbol geometry, atlas and placement features.
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
    sdf::{assets::SymbolAtlas, SdfPlugin},
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
        plugin.build(
            &mut schedule,
            kernel.clone(),
            &mut world,
            &mut renderer.render_graph,
        );
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
            std::iter::repeat_n(
                ShaderSymbolVertex {
                    a_pos_offset: [0; 4],
                    a_data: [0; 4],
                    a_pixeloffset: [0; 4],
                },
                count,
            ),
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

fn upload(context: &mut MapContext, zoom: f32) {
    let Some((Initialized(pool), textures, Initialized(pipeline))) =
        context.world.resources.query_mut::<(
            &mut Eventually<SymbolBufferPool>,
            &mut SymbolTextures,
            &Eventually<SymbolPipeline>,
        )>()
    else {
        panic!("GPU resources");
    };
    upload_symbol_layer(
        pool,
        textures,
        &TextureContext {
            device: &context.renderer.device,
            queue: &context.renderer.queue,
            pipeline: &pipeline.combined,
        },
        &mut context.world.tiles,
        &context.style,
        &[WorldTileCoords::default()],
        zoom,
    );
}

fn binding(context: &MapContext) -> &super::super::textures::DrawBinding {
    context
        .world
        .resources
        .get::<SymbolTextures>()
        .expect("textures")
        .binding(Default::default(), "label")
        .expect("binding")
}

fn allocation(context: &MapContext) -> u64 {
    let Some(Initialized(pool)) = context
        .world
        .resources
        .get::<Eventually<SymbolBufferPool>>()
    else {
        panic!("pool");
    };
    pool.index()
        .get_layers(Default::default())
        .expect("allocation")[0]
        .allocation_id()
}

fn committed(context: &MapContext) -> &SymbolLayerData {
    &context
        .world
        .tiles
        .query::<&SymbolLayersDataComponent>(Default::default())
        .expect("symbols")
        .layers[0]
}

mod tests;
