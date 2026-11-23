//! Creates the symbol pipeline and geometry pools.
use crate::{
    context::MapContext,
    render::{
        eventually::{Eventually, Eventually::Initialized},
        projection::ProjectionGpuResources,
        resource::{RenderPipeline, TilePipeline},
        shaders::{Shader, SymbolShader},
    },
    sdf::{SymbolBufferPool, SymbolModePipelines, SymbolPipeline},
    tcs::system::{SystemError, SystemResult},
    vector::resource::BufferPool,
};

#[derive(Default)]
struct SdfHaloStyleCache {
    layers_rev: Option<u64>,
    symbol_layer_indices: Vec<usize>,
    key: Option<(u64, u64, bool, bool, bool)>,
    needs_combined: bool,
    needs_separate: bool,
}

fn tiles_have_symbols(world: &crate::tcs::world::World) -> bool {
    !world
        .tiles
        .pyramid_of::<super::SymbolLayersDataComponent>()
        .is_empty()
        && world.tiles.tiles.values().any(|tile| {
            world
                .tiles
                .query::<&super::SymbolLayersDataComponent>(tile.coords)
                .is_some_and(|c| c.layers.iter().any(|l| !l.buffer.buffer.indices.is_empty()))
                || world
                    .tiles
                    .query::<&crate::vector::content::LayerReplacements>(tile.coords)
                    .is_some_and(|r| {
                        r.symbols
                            .iter()
                            .any(|l| !l.buffer.buffer.indices.is_empty())
                    })
        })
}

fn resolve_halo_requirements(
    world: &mut crate::tcs::world::World,
    style: &crate::style::Style,
    style_zoom: f64,
    uses_globe: bool,
) -> (bool, bool) {
    let (tex_combined, tex_separate) = world
        .resources
        .get::<super::textures::SymbolTextures>()
        .map_or(
            (false, false),
            super::textures::SymbolTextures::separate_halo_flags,
        );
    let rev = style.revision();
    let key = (
        rev,
        style_zoom.to_bits(),
        uses_globe,
        tex_combined,
        tex_separate,
    );
    let cache = world.resources.get_or_init_mut::<SdfHaloStyleCache>();
    if cache.layers_rev != Some(rev) {
        cache.layers_rev = Some(rev);
        cache.symbol_layer_indices.clear();
        for (idx, layer) in style.layers.iter().enumerate() {
            if layer.type_ == "symbol" && !layer.is_hidden() {
                cache.symbol_layer_indices.push(idx);
            }
        }
    }
    if cache.key == Some(key) {
        return (cache.needs_combined, cache.needs_separate);
    }
    let (mut needs_combined, mut needs_separate) = (tex_combined, tex_separate);
    for &idx in &cache.symbol_layer_indices {
        let Some(layer) = style.layers.get(idx) else {
            continue;
        };
        if !layer.is_visible_at(style_zoom) {
            continue;
        }
        if let Some(crate::style::layer::LayerPaint::Symbol(paint)) = &layer.paint {
            let uniforms = super::paint::SymbolUniforms::new(paint, style_zoom, [1, 1]);
            let separate = uniforms.text[1] > 1.0
                || uniforms.icon[1] > 1.0
                || (uniforms.text[1] > 0.0
                    && paint.number("text-letter-spacing", &Default::default(), style_zoom, 0.0)
                        < 0.0);
            if separate {
                needs_separate = true;
            } else {
                needs_combined = true;
            }
        } else {
            needs_combined = true;
        }
        if needs_combined && needs_separate {
            break;
        }
    }
    if !needs_combined && !needs_separate {
        needs_combined = true;
    }
    cache.key = Some(key);
    cache.needs_combined = needs_combined;
    cache.needs_separate = needs_separate;
    (needs_combined, needs_separate)
}

pub fn resource_system(
    MapContext {
        world,
        style,
        view_state,
        renderer,
        ..
    }: &mut MapContext,
) -> SystemResult {
    let style_zoom = view_state.style_zoom().value();
    let uses_globe = style.uses_globe_at(style_zoom);
    let has_symbol_layers = super::style_has_symbol_layers(style);
    if let Some(Eventually::Uninitialized) = world.resources.get::<Eventually<SymbolPipeline>>() {
        if !has_symbol_layers {
            if let Some(pool) = world.resources.get_mut::<Eventually<SymbolBufferPool>>() {
                pool.initialize(|| {
                    BufferPool::from_device_with_sizes(
                        &renderer.device,
                        renderer.settings.symbol_pools,
                    )
                });
            }
            return Ok(());
        }
    }
    let (needs_combined, needs_separate) = if has_symbol_layers {
        resolve_halo_requirements(world, style, style_zoom, uses_globe)
    } else {
        (false, false)
    };
    let mode_already_complete = matches!(
        world.resources.get::<Eventually<SymbolPipeline>>(),
        Some(Initialized(sp))
            if (!needs_combined && !needs_separate)
                || sp.for_globe(uses_globe).is_some_and(|m| {
                    (!needs_combined || m.combined.is_some())
                        && (!needs_separate || (m.halo.is_some() && m.fill.is_some()))
                })
    );
    let has_symbol_geometry = !mode_already_complete
        && (matches!(
            world.resources.get::<Eventually<SymbolBufferPool>>(),
            Some(Initialized(pool)) if !pool.index().is_empty()
        ) || tiles_have_symbols(world));
    let (needs_combined, needs_separate) = if has_symbol_geometry {
        (needs_combined, needs_separate)
    } else {
        (false, false)
    };

    let Some((pool, pipeline, Initialized(projection))) = world.resources.query_mut::<(
        &mut Eventually<SymbolBufferPool>,
        &mut Eventually<SymbolPipeline>,
        &mut Eventually<ProjectionGpuResources>,
    )>() else {
        return Err(SystemError::Dependencies);
    };
    pool.initialize(|| {
        BufferPool::from_device_with_sizes(&renderer.device, renderer.settings.symbol_pools)
    });
    if matches!(pipeline, Eventually::Uninitialized) && !has_symbol_layers {
        return Ok(());
    }
    if renderer.settings.symbol_pools.is_empty() {
        return Ok(());
    }
    let symbol_pipeline = pipeline.initialize(|| SymbolPipeline::new(&renderer.device));
    if has_symbol_geometry {
        let mode = symbol_pipeline.for_globe(uses_globe);
        let compile_combined = needs_combined && mode.is_none_or(|m| m.combined.is_none());
        let compile_halo = needs_separate && mode.is_none_or(|m| m.halo.is_none());
        let compile_fill = needs_separate && mode.is_none_or(|m| m.fill.is_none());
        if compile_combined || compile_halo || compile_fill {
            let mut descriptor = create_symbol_descriptor(
                renderer,
                projection,
                &symbol_pipeline.atlas_layout,
                &symbol_pipeline.depth_layout,
                uses_globe,
            );
            let prefix = [
                projection.bind_group_layout(),
                &symbol_pipeline.atlas_layout,
                &symbol_pipeline.depth_layout,
            ];
            let combined = compile_combined
                .then(|| descriptor.initialize_with_prefix_layouts(&renderer.device, &prefix));
            let halo = compile_halo.then(|| {
                descriptor.fragment.entry_point = "halo";
                descriptor.initialize_with_prefix_layouts(&renderer.device, &prefix)
            });
            let fill = compile_fill.then(|| {
                descriptor.fragment.entry_point = "fill";
                descriptor.initialize_with_prefix_layouts(&renderer.device, &prefix)
            });
            let slot = symbol_pipeline
                .slot_mut(uses_globe)
                .get_or_insert_with(SymbolModePipelines::default);
            if let Some(p) = combined {
                slot.combined = Some(p);
            }
            if let Some(p) = halo {
                slot.halo = Some(p);
            }
            if let Some(p) = fill {
                slot.fill = Some(p);
            }
        }
    }
    let Some((depth, Initialized(pipeline))) = world.resources.query_mut::<(
        &mut Eventually<super::depth::SymbolDepth>,
        &Eventually<SymbolPipeline>,
    )>() else {
        return Err(SystemError::Dependencies);
    };
    let Initialized(source) = &renderer.resources.depth_texture else {
        return Err(SystemError::Dependencies);
    };
    let size = source.texture.size();
    let samples = source.texture.sample_count();
    depth.reinitialize(
        || {
            super::depth::SymbolDepth::new(
                &renderer.device,
                size,
                samples,
                pipeline.depth_layout.clone(),
            )
        },
        &(size.width, size.height, samples),
    );
    Ok(())
}

fn create_symbol_descriptor(
    renderer: &crate::render::Renderer,
    _projection: &ProjectionGpuResources,
    _atlas_layout: &wgpu::BindGroupLayout,
    _depth_layout: &wgpu::BindGroupLayout,
    globe: bool,
) -> crate::render::resource::RenderPipelineDescriptor {
    let surface = &renderer.resources.surface;
    let shader = SymbolShader {
        format: surface.surface_format(),
    };
    let mut descriptor = TilePipeline::new(
        if globe {
            "symbol_globe_pipeline"
        } else {
            "symbol_flat_pipeline"
        }
        .into(),
        renderer.settings,
        shader.describe_vertex_for_globe(globe),
        shader.describe_fragment(),
        crate::render::resource::TilePipelineOptions {
            depth_stencil_enabled: true,
            update_stencil: false,
            debug_stencil: true,
            wireframe: false,
            multisampling: surface.is_multisampling_supported(renderer.settings.msaa),
            textured: false,
        },
    )
    .with_depth_write()
    .describe_render_pipeline();
    descriptor.layout = None;
    if let Some(depth) = descriptor.depth_stencil.as_mut() {
        depth.depth_compare = Some(wgpu::CompareFunction::Always);
    }
    descriptor
}
