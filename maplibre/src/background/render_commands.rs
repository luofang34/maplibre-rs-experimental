//! Background and sky commands that skip draws until their GPU resources are ready.

#![deny(missing_docs)]

use crate::{
    background::resource_system::{
        AtmosphereRenderPipeline, BackgroundRenderPipeline, GlobeBackgroundRenderPipeline,
        SkyRenderPipeline,
    },
    render::{
        eventually::Eventually::{self, Initialized},
        projection::ProjectionGpuResources,
        render_phase::{LayerItem, PhaseItem, RenderCommand, RenderCommandResult},
        tile_mesh::TileMeshUsage,
    },
    tcs::world::World,
};

/// Binds the flat-background pipeline when it is initialized.
pub struct SetBackgroundPipeline;
impl RenderCommand<LayerItem> for SetBackgroundPipeline {
    fn render<'w>(
        world: &'w World,
        item: &LayerItem,
        pass: &mut crate::render::tracked_pass::TrackedRenderPass<'w>,
    ) -> RenderCommandResult {
        let Some(Initialized(pipelines)) = world
            .resources
            .get::<Eventually<BackgroundRenderPipeline>>()
        else {
            return RenderCommandResult::Failure;
        };
        let buffers = world
            .resources
            .get::<crate::background::queue_system::BackgroundBuffers>();
        let covers = buffers.is_some_and(|buffers| buffers.flat_covers_viewport);
        let opaque =
            buffers.is_some_and(|buffers| buffers.opaque_layers.contains(&item.style_layer));
        let pipeline = match (covers, opaque) {
            (true, true) => pipelines
                .solid_opaque
                .as_ref()
                .or(pipelines.solid.as_ref())
                .or(pipelines.clipped_opaque.as_ref())
                .or(pipelines.clipped.as_ref()),
            (true, false) => pipelines.solid.as_ref().or(pipelines.clipped.as_ref()),
            (false, true) => pipelines
                .clipped_opaque
                .as_ref()
                .or(pipelines.clipped.as_ref()),
            (false, false) => pipelines.clipped.as_ref(),
        };
        let Some(pipeline) = pipeline else {
            return RenderCommandResult::Failure;
        };

        pass.set_pipeline(pipeline);
        RenderCommandResult::Success
    }
}

/// Binds background metadata and draws a fullscreen quad.
pub struct DrawBackgroundQuad;
impl RenderCommand<LayerItem> for DrawBackgroundQuad {
    fn render<'w>(
        world: &'w World,
        item: &LayerItem,
        pass: &mut crate::render::tracked_pass::TrackedRenderPass<'w>,
    ) -> RenderCommandResult {
        if let Some(buf) = world
            .resources
            .get::<crate::background::queue_system::BackgroundBuffers>()
        {
            pass.set_vertex_buffer(0, buf.metadata_buffer.slice(..));

            // Each background layer paints with its own colour.
            let instance = buf.instances.get(&item.style_layer).copied().unwrap_or(0);
            pass.draw(0..6, instance..instance + 1);
            return RenderCommandResult::Success;
        }
        RenderCommandResult::Failure
    }
}

/// Binds the globe-background pipeline and view projection uniform.
pub struct SetGlobeBackgroundPipeline;
impl RenderCommand<LayerItem> for SetGlobeBackgroundPipeline {
    fn render<'w>(
        world: &'w World,
        item: &LayerItem,
        pass: &mut crate::render::tracked_pass::TrackedRenderPass<'w>,
    ) -> RenderCommandResult {
        let (Some(pipelines), Some(projection_resources)) = (
            world
                .resources
                .get::<Eventually<GlobeBackgroundRenderPipeline>>()
                .and_then(|res| match res {
                    Initialized(v) => Some(v),
                    _ => None,
                }),
            crate::render::pass_refs::PassFrameRefs::resolve_projection(pass.frame_refs(), world),
        ) else {
            return RenderCommandResult::Failure;
        };
        let opaque = world
            .resources
            .get::<crate::background::queue_system::BackgroundBuffers>()
            .is_some_and(|buffers| buffers.opaque_layers.contains(&item.style_layer));
        let pipeline = if opaque {
            pipelines.opaque.as_ref().or(pipelines.blended.as_ref())
        } else {
            pipelines.blended.as_ref()
        };
        let Some(pipeline) = pipeline else {
            return RenderCommandResult::Failure;
        };
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, projection_resources.bind_group(), &[]);
        RenderCommandResult::Success
    }
}

/// Draws the root globe mesh using background color and tile metadata.
pub struct DrawGlobeBackgroundQuad;
impl RenderCommand<LayerItem> for DrawGlobeBackgroundQuad {
    fn render<'w>(
        world: &'w World,
        item: &LayerItem,
        pass: &mut crate::render::tracked_pass::TrackedRenderPass<'w>,
    ) -> RenderCommandResult {
        let (Some(buffers), Some(mesh_cache)) = (
            world
                .resources
                .get::<crate::background::queue_system::BackgroundBuffers>(),
            crate::render::pass_refs::PassFrameRefs::resolve_tile_mesh_cache(
                pass.frame_refs(),
                world,
            ),
        ) else {
            return RenderCommandResult::Failure;
        };
        let instance = buffers
            .instances
            .get(&item.style_layer)
            .copied()
            .unwrap_or(0);
        // Layer metadata uses dynamic uniform offsets at `@group(1)` and tile metadata is sliced
        // per contiguous mesh-variant run with `0..count` indices for WebGL2 & WebGPU parity.
        let dynamic_offset = instance.saturating_mul(buffers.globe_layer_stride);
        pass.set_bind_group(1, &buffers.globe_layer_bind_group, &[dynamic_offset]);
        let stride = std::mem::size_of::<crate::render::shaders::ShaderTileMetadata>() as u64;
        let mut idx = 0_usize;
        let mut drawn = false;
        for chunk in buffers.globe_tiles.chunk_by(|a, b| {
            crate::render::tile_mesh::globe_mesh_group_key(*a, TileMeshUsage::Raster)
                == crate::render::tile_mesh::globe_mesh_group_key(*b, TileMeshUsage::Raster)
        }) {
            let count = chunk.len();
            if let Some(first) = chunk.first() {
                if let Some(mesh) = mesh_cache.get(*first, TileMeshUsage::Raster, false) {
                    let start_byte = (idx as u64) * stride;
                    let end_byte = ((idx + count) as u64) * stride;
                    pass.set_vertex_buffer(0, mesh.vertex_buffer().slice(..));
                    pass.set_vertex_buffer(
                        1,
                        buffers.tile_metadata_buffer.slice(start_byte..end_byte),
                    );
                    pass.set_index_buffer(mesh.index_buffer().slice(..), mesh.index_format());
                    pass.draw_indexed(0..mesh.index_count(), 0, 0..count as u32);
                    drawn = true;
                }
            }
            idx += count;
        }
        if drawn {
            RenderCommandResult::Success
        } else {
            RenderCommandResult::Failure
        }
    }
}

/// Binds the globe pattern pipeline, the image of the item's layer and the size of the world.
pub struct SetGlobeBackgroundPatternPipeline;
impl RenderCommand<LayerItem> for SetGlobeBackgroundPatternPipeline {
    fn render<'w>(
        world: &'w World,
        item: &LayerItem,
        pass: &mut crate::render::tracked_pass::TrackedRenderPass<'w>,
    ) -> RenderCommandResult {
        let (Some(gpu), Some(patterns), Some(projection_resources)) = (
            world
                .resources
                .get::<crate::background::pattern::BackgroundPatternGpu>(),
            crate::render::pass_refs::PassFrameRefs::resolve_pattern_resources(
                pass.frame_refs(),
                world,
            ),
            crate::render::pass_refs::PassFrameRefs::resolve_projection(pass.frame_refs(), world),
        ) else {
            return RenderCommandResult::Failure;
        };
        let (Some((pipeline, world_size)), Some(image)) =
            (gpu.globe(), patterns.binding(item.style_layer))
        else {
            return RenderCommandResult::Failure;
        };
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, projection_resources.bind_group(), &[]);
        pass.set_bind_group(2, image, &[]);
        pass.set_bind_group(3, world_size, &[]);
        RenderCommandResult::Success
    }
}

/// Draws the globe background from an image.
pub type DrawGlobeBackgroundPattern = (SetGlobeBackgroundPatternPipeline, DrawGlobeBackgroundQuad);

/// Binds the globe-background pipeline and draws the globe surface.
pub type DrawGlobeBackground = (SetGlobeBackgroundPipeline, DrawGlobeBackgroundQuad);

/// Binds the atmosphere pipeline and view projection uniform.
pub struct SetAtmospherePipeline;
impl<P: PhaseItem> RenderCommand<P> for SetAtmospherePipeline {
    fn render<'w>(
        world: &'w World,
        _item: &P,
        pass: &mut crate::render::tracked_pass::TrackedRenderPass<'w>,
    ) -> RenderCommandResult {
        let Some((
            Initialized(AtmosphereRenderPipeline(pipeline)),
            Initialized(projection_resources),
        )) = world.resources.query::<(
            &Eventually<AtmosphereRenderPipeline>,
            &Eventually<ProjectionGpuResources>,
        )>()
        else {
            return RenderCommandResult::Failure;
        };
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, projection_resources.bind_group(), &[]);
        RenderCommandResult::Success
    }
}

/// Draws atmospheric scattering using the frame's evaluated camera and light metadata.
pub struct DrawAtmosphereFullscreen;
impl<P: PhaseItem> RenderCommand<P> for DrawAtmosphereFullscreen {
    fn render<'w>(
        world: &'w World,
        _item: &P,
        pass: &mut crate::render::tracked_pass::TrackedRenderPass<'w>,
    ) -> RenderCommandResult {
        let Some(buffers) = world
            .resources
            .get::<crate::background::queue_system::BackgroundBuffers>()
        else {
            return RenderCommandResult::Failure;
        };
        pass.set_vertex_buffer(0, buffers.atmosphere_metadata_buffer.slice(..));
        pass.draw(0..3, 0..1);
        RenderCommandResult::Success
    }
}

/// Binds the atmosphere pipeline and draws scattering over the existing scene.
pub type DrawAtmosphere = (SetAtmospherePipeline, DrawAtmosphereFullscreen);

/// Binds the sky pipeline when it is initialized.
pub struct SetSkyPipeline;
impl<P: PhaseItem> RenderCommand<P> for SetSkyPipeline {
    fn render<'w>(
        world: &'w World,
        _item: &P,
        pass: &mut crate::render::tracked_pass::TrackedRenderPass<'w>,
    ) -> RenderCommandResult {
        let Some((Initialized(SkyRenderPipeline(pipeline)), Initialized(projection_resources))) =
            world.resources.query::<(
                &Eventually<SkyRenderPipeline>,
                &Eventually<ProjectionGpuResources>,
            )>()
        else {
            return RenderCommandResult::Failure;
        };
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, projection_resources.bind_group(), &[]);
        RenderCommandResult::Success
    }
}

/// Draws the horizon-clipped sky when sky metadata is available.
pub struct DrawSkyFullscreen;
impl<P: PhaseItem> RenderCommand<P> for DrawSkyFullscreen {
    fn render<'w>(
        world: &'w World,
        _item: &P,
        pass: &mut crate::render::tracked_pass::TrackedRenderPass<'w>,
    ) -> RenderCommandResult {
        let Some(buffers) = world
            .resources
            .get::<crate::background::queue_system::BackgroundBuffers>()
        else {
            return RenderCommandResult::Failure;
        };
        let Some(sky) = &buffers.sky_metadata_buffer else {
            return RenderCommandResult::Failure;
        };
        pass.set_vertex_buffer(0, sky.slice(..));
        pass.draw(0..3, 0..1);
        RenderCommandResult::Success
    }
}

/// Fills the screen above the horizon with the sky.
pub type DrawSky = (SetSkyPipeline, DrawSkyFullscreen);

/// Binds the pattern pipeline, the image of the item's layer and the view of the map.
pub struct SetBackgroundPatternPipeline;
impl RenderCommand<LayerItem> for SetBackgroundPatternPipeline {
    fn render<'w>(
        world: &'w World,
        item: &LayerItem,
        pass: &mut crate::render::tracked_pass::TrackedRenderPass<'w>,
    ) -> RenderCommandResult {
        let (Some(gpu), Some(patterns)) = (
            world
                .resources
                .get::<crate::background::pattern::BackgroundPatternGpu>(),
            world
                .resources
                .get::<crate::vector::pattern::PatternResources>(),
        ) else {
            return RenderCommandResult::Failure;
        };
        let Some(image) = patterns.binding(item.style_layer) else {
            return RenderCommandResult::Failure;
        };
        pass.set_pipeline(gpu.pipeline());
        pass.set_bind_group(0, image, &[]);
        pass.set_bind_group(1, gpu.view(), &[]);
        RenderCommandResult::Success
    }
}

/// Draws the flat background from an image.
pub struct DrawBackgroundPattern;
impl RenderCommand<LayerItem> for DrawBackgroundPattern {
    fn render<'w>(
        world: &'w World,
        item: &LayerItem,
        pass: &mut crate::render::tracked_pass::TrackedRenderPass<'w>,
    ) -> RenderCommandResult {
        <(SetBackgroundPatternPipeline, DrawBackgroundQuad)>::render(world, item, pass)
    }
}

/// Binds and draws the flat background, stopping if either resource lookup fails.
pub struct DrawBackground;
impl RenderCommand<LayerItem> for DrawBackground {
    fn render<'w>(
        world: &'w World,
        item: &LayerItem,
        pass: &mut crate::render::tracked_pass::TrackedRenderPass<'w>,
    ) -> RenderCommandResult {
        <(SetBackgroundPipeline, DrawBackgroundQuad)>::render(world, item, pass)
    }
}
