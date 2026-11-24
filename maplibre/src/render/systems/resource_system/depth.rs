//! Depth persistence detection so transient depth and multisampled color can be safely paired.

use crate::{
    render::{
        eventually::Eventually, graph::RenderGraph, ground::FrameGround,
        settings::RendererSettings, view_state::ViewState, RenderResources,
    },
    sdf::depth::SymbolDepth,
    style::Style,
    tcs::world::World,
};

pub(super) struct CachedDepthFormatFeatures {
    pub(super) format: wgpu::TextureFormat,
    pub(super) supports_transient: bool,
}

struct CachedDepthLayerScan {
    style_rev: u64,
    pitch_tilted: bool,
    needs_persistent: bool,
}

pub(super) fn compute_needs_persistent_depth(
    world: &mut World,
    style: &Style,
    view_state: &ViewState,
    settings: &RendererSettings,
    state: &RenderResources,
    render_graph: &RenderGraph,
) -> bool {
    if !world
        .resources
        .get::<CachedDepthFormatFeatures>()
        .is_some_and(|c| c.supports_transient)
    {
        return true;
    }
    let world_needs = world
        .resources
        .get::<FrameGround>()
        .is_some_and(|g| g.0.is_some())
        || has_celestial_sun(world)
        || has_globe_daylight_frame(world)
        || matches!(
            world.resources.get::<Eventually<SymbolDepth>>(),
            Some(Eventually::Initialized(sd)) if !sd.skip_depth_copy
        );
    if settings.depth_readout
        || state.eye_depth_target.is_some()
        || style.terrain.is_some()
        || world_needs
        || !render_graph.has_main_before_translucent()
        || render_graph.has_custom_scene_depth_writers()
        || render_graph.has_named_depth_reader_passes()
    {
        return true;
    }
    let pitch_tilted = view_state.camera().get_pitch().0.abs() >= 1e-4;
    let style_rev = style.revision();
    if let Some(cached) = world.resources.get::<CachedDepthLayerScan>()
        && cached.style_rev == style_rev
        && cached.pitch_tilted == pitch_tilted
    {
        return cached.needs_persistent;
    }
    let scan = matches!(
        style.projection.as_ref().map(|p| &p.projection_type),
        Some(crate::projection::ProjectionType::VerticalPerspective)
    ) || style.layers.iter().any(|l| {
        l.type_ == "custom"
            || (pitch_tilted
                && matches!(
                    l.type_.as_str(),
                    "fill-extrusion" | "raster" | "hillshade" | "color-relief"
                ))
    });
    world.resources.insert(CachedDepthLayerScan {
        style_rev,
        pitch_tilted,
        needs_persistent: scan,
    });
    scan
}

pub(super) fn depth_texture_usage(
    supports_transient: bool,
    needs_persistent: bool,
) -> wgpu::TextureUsages {
    if supports_transient && !needs_persistent {
        wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TRANSIENT_ATTACHMENT
    } else {
        // A host compositor or downstream pass reads depth back via copy or sampling.
        wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING
    }
}

fn has_celestial_sun(world: &World) -> bool {
    #[cfg(feature = "experimental")]
    {
        crate::celestial::lighting(world).sun.is_some()
    }
    #[cfg(not(feature = "experimental"))]
    {
        let _ = world;
        false
    }
}

fn has_globe_daylight_frame(world: &World) -> bool {
    #[cfg(feature = "experimental")]
    {
        world
            .resources
            .get::<crate::background::daylight::GlobeDaylightGpu>()
            .is_some_and(crate::background::daylight::GlobeDaylightGpu::has_frame)
    }
    #[cfg(not(feature = "experimental"))]
    {
        let _ = world;
        false
    }
}
