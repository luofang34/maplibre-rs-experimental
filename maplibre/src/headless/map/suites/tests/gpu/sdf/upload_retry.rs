//! Upload failures preserve the committed symbol geometry, atlas and placement features.
#![cfg_attr(test, allow(clippy::expect_used))]
use std::{rc::Rc, sync::Arc};

use crate::{
    context::MapContext,
    coords::{WorldCoords, WorldTileCoords, Zoom},
    map::RenderPlugin,
    plugin::Plugin,
    render::{
        eventually::{Eventually, Eventually::Initialized},
        memory_budget::FrameUploads,
        projection::ProjectionGpuResources,
        resource::OverAlignedVertexBuffer,
        settings::BufferPoolSizes,
        shaders::ShaderSymbolVertex,
        view_state::ViewState,
    },
    schedule::{Schedule, Stage},
    sdf::{
        assets::SymbolAtlas,
        textures::{DrawBinding, SymbolTextures, TextureContext},
        upload_system::{replacements, upload_symbol_layer, SymbolStyleIndex},
        SdfPlugin, SymbolBufferPool, SymbolLayerData, SymbolLayersDataComponent, SymbolPipeline,
    },
    style::Style,
    tcs::world::World,
    vector::{DefaultVectorTransferables, VectorPlugin},
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
    use crate::sdf::SymbolLayerData;
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
    let mut symbol_style = SymbolStyleIndex::default();
    symbol_style.sync(&context.style);
    upload_symbol_layer(
        pool,
        textures,
        &TextureContext {
            device: &context.renderer.device,
            queue: &context.renderer.queue,
            atlas_layout: pipeline.atlas_layout(),
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

fn binding(context: &MapContext) -> Result<&DrawBinding, &'static str> {
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

#[tokio::test]
async fn failed_symbol_upload_keeps_atlas_and_features_while_refreshing_paint(
) -> Result<(), &'static str> {
    let mut context = context().await;
    let old = atlas(80);
    replacements::accept_symbols(&mut context.world, layer(3, old.clone(), "old"));
    upload(&mut context, 0.0)?;
    let old_allocation = allocation(&context)?;
    let old_group = binding(&context)?.group.clone();
    assert!(!binding(&context)?.separate_halo);
    let new = atlas(200);
    replacements::accept_symbols(&mut context.world, layer(257, new.clone(), "new"));
    upload(&mut context, 4.0)?;
    assert_eq!(
        allocation(&context)?,
        old_allocation,
        "capacity rejection preserves geometry"
    );
    assert_eq!(
        binding(&context)?.group,
        old_group,
        "atlas binding stays committed"
    );
    assert!(
        binding(&context)?.separate_halo,
        "retained symbols still evaluate current zoom paint"
    );
    assert_eq!(
        committed(&context)?
            .features
            .first()
            .ok_or("missing old feature")?
            .str,
        "old"
    );
    assert!(Arc::ptr_eq(
        committed(&context)?
            .atlas
            .as_ref()
            .ok_or("missing old atlas")?,
        &old
    ));
    replacements::accept_symbols(&mut context.world, layer(3, new.clone(), "new"));
    upload(&mut context, 4.0)?;
    assert_ne!(allocation(&context)?, old_allocation);
    assert_ne!(
        binding(&context)?.group,
        old_group,
        "new atlas changes with the geometry"
    );
    assert_eq!(
        committed(&context)?
            .features
            .first()
            .ok_or("missing new feature")?
            .str,
        "new"
    );
    assert!(Arc::ptr_eq(
        committed(&context)?
            .atlas
            .as_ref()
            .ok_or("missing new atlas")?,
        &new
    ));
    assert!(context
        .world
        .tiles
        .query::<&replacements::SymbolLayerReplacements>(Default::default())
        .ok_or("missing pending symbol replacements")?
        .symbols
        .is_empty());
    Ok(())
}
