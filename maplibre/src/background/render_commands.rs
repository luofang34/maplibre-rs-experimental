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
        tile_mesh::{GlobeTileMeshCache, TileMeshUsage},
    },
    tcs::world::World,
};

/// Binds the flat-background pipeline when it is initialized.
pub struct SetBackgroundPipeline;
impl<P: PhaseItem> RenderCommand<P> for SetBackgroundPipeline {
    fn render<'w>(
        world: &'w World,
        _item: &P,
        pass: &mut crate::render::tracked_pass::TrackedRenderPass<'w>,
    ) -> RenderCommandResult {
        let Some(Initialized(BackgroundRenderPipeline(pipeline))) = world
            .resources
            .get::<Eventually<BackgroundRenderPipeline>>()
        else {
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
impl<P: PhaseItem> RenderCommand<P> for SetGlobeBackgroundPipeline {
    fn render<'w>(
        world: &'w World,
        _item: &P,
        pass: &mut crate::render::tracked_pass::TrackedRenderPass<'w>,
    ) -> RenderCommandResult {
        let Some((
            Initialized(GlobeBackgroundRenderPipeline(pipeline)),
            Initialized(projection_resources),
        )) = world.resources.query::<(
            &Eventually<GlobeBackgroundRenderPipeline>,
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

/// Draws the root globe mesh using background color and tile metadata.
pub struct DrawGlobeBackgroundQuad;
impl RenderCommand<LayerItem> for DrawGlobeBackgroundQuad {
    fn render<'w>(
        world: &'w World,
        item: &LayerItem,
        pass: &mut crate::render::tracked_pass::TrackedRenderPass<'w>,
    ) -> RenderCommandResult {
        let Some((buffers, mesh_cache)) = world.resources.query::<(
            &crate::background::queue_system::BackgroundBuffers,
            &GlobeTileMeshCache,
        )>() else {
            return RenderCommandResult::Failure;
        };
        let coords = crate::coords::WorldTileCoords::default();
        let Some(mesh) = mesh_cache.get(coords, TileMeshUsage::Raster, false) else {
            return RenderCommandResult::Failure;
        };
        pass.set_vertex_buffer(0, mesh.vertex_buffer().slice(..));
        pass.set_vertex_buffer(1, buffers.tile_metadata_buffer.slice(..));
        pass.set_vertex_buffer(2, buffers.metadata_buffer.slice(..));
        pass.set_index_buffer(mesh.index_buffer().slice(..), mesh.index_format());
        let instance = buffers
            .instances
            .get(&item.style_layer)
            .copied()
            .unwrap_or(0);
        pass.draw_indexed(0..mesh.index_count(), 0, instance..instance + 1);
        RenderCommandResult::Success
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
        let (Some(gpu), Some(patterns), Some(Initialized(projection_resources))) = (
            world
                .resources
                .get::<crate::background::pattern::BackgroundPatternGpu>(),
            world
                .resources
                .get::<crate::vector::pattern::PatternResources>(),
            world.resources.get::<Eventually<ProjectionGpuResources>>(),
        ) else {
            return RenderCommandResult::Failure;
        };
        let (Some((pipeline, world_size)), Some(image)) =
            (gpu.globe(), patterns.binding(&item.style_layer))
        else {
            return RenderCommandResult::Failure;
        };
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, projection_resources.bind_group(), &[]);
        pass.set_bind_group(1, image, &[]);
        pass.set_bind_group(2, world_size, &[]);
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
        let Some(image) = patterns.binding(&item.style_layer) else {
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
