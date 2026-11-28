//! Custom style-layer registration on [`Map`] and [`MapContext`], and frame queue system.

use super::{CurrentMapContext, Map};
use crate::{
    context::MapContext,
    custom_layer::{
        CustomLayerContext, CustomLayerFrameState, CustomLayerRegistry, CustomStyleLayer,
        DrawCustomLayer,
    },
    environment::Environment,
    render::{
        render_phase::{LayerItem, MaskGroup, ProjectionBinding, RenderPhase, UNSET_ENTRY_SLOT},
        tile_view_pattern::ShapeDrawData,
    },
    schedule::system::{SystemError, SystemResult},
    style::StyleLayerId,
    tcs::tiles::Tile,
    window::{HeadedMapWindow, MapWindowConfig},
};

impl<E: Environment> Map<E>
where
    <<E as Environment>::MapWindowConfig as MapWindowConfig>::MapWindow: HeadedMapWindow,
{
    /// Registers a [`CustomStyleLayer`] drawn at the position of the matching `"type": "custom"`
    /// layer in `style.layers`, before or after renderer initialization.
    pub fn register_custom_layer(&mut self, layer: impl CustomStyleLayer) {
        match &mut self.map_context {
            CurrentMapContext::Ready(context) => context.register_custom_layer(layer),
            CurrentMapContext::Pending(_) => self.custom_layers.register(layer),
        }
    }

    /// Removes a registered [`CustomStyleLayer`] by `id`, returning whether one existed.
    pub fn remove_custom_layer(&mut self, id: &str) -> bool {
        match &mut self.map_context {
            CurrentMapContext::Ready(context) => context.remove_custom_layer(id),
            CurrentMapContext::Pending(_) => self.custom_layers.remove(id),
        }
    }
}

impl MapContext {
    /// Registers a [`CustomStyleLayer`] in this context's [`CustomLayerRegistry`] and marks the
    /// frame dirty.
    pub fn register_custom_layer(&mut self, layer: impl CustomStyleLayer) {
        self.world
            .resources
            .get_or_init_mut::<CustomLayerRegistry>()
            .register(layer);
        crate::render::frame_signals::mark_dirty(&mut self.world);
    }

    /// Removes a [`CustomStyleLayer`] by `id` and marks the frame dirty when one was removed.
    pub fn remove_custom_layer(&mut self, id: &str) -> bool {
        let removed = self
            .world
            .resources
            .get_or_init_mut::<CustomLayerRegistry>()
            .remove(id);
        if removed {
            crate::render::frame_signals::mark_dirty(&mut self.world);
        }
        removed
    }
}

/// Initializes, prepares, and queues visible `"type": "custom"` style layers in painter order.
pub(crate) fn queue_custom_layers_system(
    MapContext {
        world,
        style,
        view_state,
        renderer,
        ..
    }: &mut MapContext,
) -> SystemResult {
    if world
        .resources
        .get::<CustomLayerRegistry>()
        .is_none_or(|registry| registry.layers.is_empty())
    {
        return Ok(());
    }
    let zoom = view_state.zoom().value();
    let visible_custom: Vec<(u32, String)> = style
        .layers
        .iter()
        .filter(|layer| layer.type_ == "custom" && layer.is_visible_at(zoom))
        .map(|layer| (layer.index, layer.id.clone()))
        .collect();
    if visible_custom.is_empty() {
        return Ok(());
    }

    let surface = renderer.state().surface();
    let color_format = surface.surface_format();
    let depth_stencil_format = renderer.settings.depth_texture_format;
    let sample_count = if surface.is_multisampling_supported(renderer.settings.msaa) {
        renderer.settings.msaa.samples
    } else {
        1
    };
    let size = surface.size();
    let viewport = [size.width(), size.height()];
    let view_proj_mat = view_state.gpu_view_projection().0;
    let view_proj: [[f64; 4]; 4] = view_proj_mat.into();
    let view_proj_f32 = view_proj.map(|col| col.map(|v| v as f32));
    let center = view_state.center_lat_lon();
    let cgmath::Deg(pitch_deg) = view_state.camera().get_pitch().into();
    let cgmath::Deg(bearing_deg) = view_state.camera().get_bearing().into();
    let globe_transition = style
        .projection
        .as_ref()
        .map_or(0.0, |spec| spec.projection_type.globe_transition(zoom));

    let mut queued_items = Vec::new();
    if let Some(registry) = world.resources.get_mut::<CustomLayerRegistry>() {
        let prep_ctx = CustomLayerContext {
            device: &renderer.device,
            queue: &renderer.queue,
            color_format,
            depth_stencil_format,
            sample_count,
            viewport,
            view_proj,
            view_proj_f32,
            center,
            zoom,
            pitch_deg,
            bearing_deg,
            globe_transition,
        };
        for (index, id) in &visible_custom {
            if let Some(entry) = registry.layers.get_mut(id) {
                if entry.initialized_target != Some((color_format, sample_count)) {
                    entry.deinitialize_if_initialized();
                    entry.layer.initialize(
                        &renderer.device,
                        &renderer.queue,
                        color_format,
                        sample_count,
                    );
                    entry.initialized_target = Some((color_format, sample_count));
                }
                entry.layer.prepare(&prep_ctx);
                queued_items.push((*index, id.clone()));
            }
        }
    }

    if queued_items.is_empty() {
        return Ok(());
    }

    world.resources.insert(CustomLayerFrameState {
        device: renderer.device.clone(),
        queue: renderer.queue.clone_uncounted(),
        color_format,
        depth_stencil_format,
        sample_count,
        viewport,
        view_proj,
        view_proj_f32,
        center,
        zoom,
        pitch_deg,
        bearing_deg,
        globe_transition,
    });

    let Some(layer_phase) = world.resources.get_mut::<RenderPhase<LayerItem>>() else {
        return Err(SystemError::Dependencies);
    };
    for (index, style_layer) in queued_items {
        layer_phase.add(LayerItem {
            draw_function: &DrawCustomLayer,
            index,
            generate_borders: false,
            style_layer: StyleLayerId::intern(&style_layer),
            entry_slot: UNSET_ENTRY_SLOT,
            tile: Tile {
                coords: crate::coords::WorldTileCoords::default(),
            },
            source_shape: ShapeDrawData::default(),
            projection: ProjectionBinding::View,
            mask_group: MaskGroup::Custom,
            run: None,
        });
    }
    Ok(())
}
