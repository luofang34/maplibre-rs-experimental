//! Host-provided GPU custom style layers (`"type": "custom"`), drawn in exact style painter order.

#![deny(missing_docs)]

use std::collections::BTreeMap;

use crate::{
    context::MapContext,
    coords::LatLon,
    render::{
        render_phase::{Draw, LayerItem, ProjectionBinding, RenderPhase},
        tile_view_pattern::TileShape,
        tracked_pass::TrackedRenderPass,
        upload_queue::UploadQueue,
    },
    tcs::{
        system::{SystemError, SystemResult},
        tiles::Tile,
        world::World,
    },
};

/// Per-frame GPU and camera state passed to [`CustomStyleLayer::prepare`] and
/// [`CustomStyleLayer::render`].
#[derive(Debug)]
pub struct CustomLayerContext<'a> {
    /// Active [`wgpu::Device`] used by the map renderer.
    pub device: &'a wgpu::Device,
    /// Counted [`UploadQueue`] for GPU buffer and texture writes.
    pub queue: &'a UploadQueue,
    /// Target color attachment format of the pass.
    pub color_format: wgpu::TextureFormat,
    /// Target depth-stencil attachment format of the pass.
    pub depth_stencil_format: wgpu::TextureFormat,
    /// Multisample count (`1` when MSAA is disabled or unsupported on the target format).
    pub sample_count: u32,
    /// Physical viewport dimensions `[width, height]` in pixels.
    pub viewport: [u32; 2],
    /// Column-major 4x4 GPU view-projection matrix (`f64`, reversed-Z clip space).
    pub view_proj: [[f64; 4]; 4],
    /// Column-major 4x4 GPU view-projection matrix cast to `f32` for direct uniform upload.
    pub view_proj_f32: [[f32; 4]; 4],
    /// Geographic center (`latitude`, `longitude`) of the current camera.
    pub center: LatLon,
    /// Continuous map zoom level.
    pub zoom: f64,
    /// Camera pitch angle in degrees.
    pub pitch_deg: f64,
    /// Camera bearing angle in degrees.
    pub bearing_deg: f64,
    /// Globe transition blend factor in `0.0..=1.0` (`0.0` = flat Mercator, `1.0` = full globe).
    pub globe_transition: f32,
}

impl CustomLayerContext<'_> {
    /// Convenience [`wgpu::DepthStencilState`] matching the map's 2D painter-order layers
    /// (`CompareFunction::Always`, depth and stencil writes disabled).
    pub fn painter_depth_stencil_state(&self) -> wgpu::DepthStencilState {
        let keep = wgpu::StencilFaceState {
            compare: wgpu::CompareFunction::Always,
            fail_op: wgpu::StencilOperation::Keep,
            depth_fail_op: wgpu::StencilOperation::Keep,
            pass_op: wgpu::StencilOperation::Keep,
        };
        wgpu::DepthStencilState {
            format: self.depth_stencil_format,
            depth_write_enabled: Some(false),
            depth_compare: Some(wgpu::CompareFunction::Always),
            stencil: wgpu::StencilState {
                front: keep,
                back: keep,
                read_mask: 0xff,
                write_mask: 0x00,
            },
            bias: wgpu::DepthBiasState::default(),
        }
    }
}

/// Host-implemented GPU layer corresponding to a `"type": "custom"` entry in `style.layers`.
pub trait CustomStyleLayer: Send + Sync + 'static {
    /// Style layer identifier (`layer.id`) matched against `"type": "custom"` layers in the style.
    fn id(&self) -> &str;

    /// Called once (or again if target format / MSAA sample count changes) before [`Self::prepare`].
    fn initialize(
        &mut self,
        _device: &wgpu::Device,
        _queue: &UploadQueue,
        _color_format: wgpu::TextureFormat,
        _sample_count: u32,
    ) {
    }

    /// Called each frame during the queue stage for visible custom layers to upload uniforms or
    /// vertex buffers before the render pass opens.
    fn prepare(&mut self, _ctx: &CustomLayerContext<'_>) {}

    /// Records draw commands into `pass` at the exact painter z-order of this layer in `style.layers`.
    fn render<'pass>(&'pass self, pass: &mut wgpu::RenderPass<'pass>, ctx: &CustomLayerContext<'_>);
}

struct RegisteredCustomLayer {
    layer: Box<dyn CustomStyleLayer>,
    initialized_target: Option<(wgpu::TextureFormat, u32)>,
}

/// Registry of host [`CustomStyleLayer`] implementations stored in `World::resources`.
#[derive(Default)]
pub struct CustomLayerRegistry {
    layers: BTreeMap<String, RegisteredCustomLayer>,
}

impl std::fmt::Debug for CustomLayerRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CustomLayerRegistry")
            .field("ids", &self.layers.keys().collect::<Vec<_>>())
            .finish()
    }
}

impl CustomLayerRegistry {
    /// Creates an empty [`CustomLayerRegistry`].
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers or replaces a [`CustomStyleLayer`] keyed by [`CustomStyleLayer::id`].
    pub fn register(&mut self, layer: impl CustomStyleLayer) {
        self.register_boxed(Box::new(layer));
    }

    /// Registers or replaces an already-boxed [`CustomStyleLayer`].
    pub fn register_boxed(&mut self, layer: Box<dyn CustomStyleLayer>) {
        let id = layer.id().to_owned();
        self.layers.insert(
            id,
            RegisteredCustomLayer {
                layer,
                initialized_target: None,
            },
        );
    }

    /// Removes the [`CustomStyleLayer`] registered under `id`, returning whether one was present.
    pub fn remove(&mut self, id: &str) -> bool {
        self.layers.remove(id).is_some()
    }

    /// Returns whether a custom layer with `id` is currently registered.
    pub fn contains(&self, id: &str) -> bool {
        self.layers.contains_key(id)
    }

    pub(crate) fn mark_all_uninitialized(&mut self) {
        for entry in self.layers.values_mut() {
            entry.initialized_target = None;
        }
    }
}

pub(crate) struct CustomLayerFrameState {
    device: std::sync::Arc<wgpu::Device>,
    queue: UploadQueue,
    color_format: wgpu::TextureFormat,
    depth_stencil_format: wgpu::TextureFormat,
    sample_count: u32,
    viewport: [u32; 2],
    view_proj: [[f64; 4]; 4],
    view_proj_f32: [[f32; 4]; 4],
    center: LatLon,
    zoom: f64,
    pitch_deg: f64,
    bearing_deg: f64,
    globe_transition: f32,
}

impl CustomLayerFrameState {
    fn as_context(&self) -> CustomLayerContext<'_> {
        CustomLayerContext {
            device: &self.device,
            queue: &self.queue,
            color_format: self.color_format,
            depth_stencil_format: self.depth_stencil_format,
            sample_count: self.sample_count,
            viewport: self.viewport,
            view_proj: self.view_proj,
            view_proj_f32: self.view_proj_f32,
            center: self.center,
            zoom: self.zoom,
            pitch_deg: self.pitch_deg,
            bearing_deg: self.bearing_deg,
            globe_transition: self.globe_transition,
        }
    }
}

/// Draw command invoking the registered [`CustomStyleLayer`] for a queued [`LayerItem`].
pub struct DrawCustomLayer;

impl Draw<LayerItem> for DrawCustomLayer {
    fn draw<'w>(&self, pass: &mut TrackedRenderPass<'w>, world: &'w World, item: &LayerItem) {
        let Some(registry) = world.resources.get::<CustomLayerRegistry>() else {
            return;
        };
        let Some(frame) = world.resources.get::<CustomLayerFrameState>() else {
            return;
        };
        let Some(entry) = registry.layers.get(&item.style_layer) else {
            return;
        };
        let ctx = frame.as_context();
        pass.with_raw_pass(|raw_pass| {
            entry.layer.render(raw_pass, &ctx);
        });
    }
}

/// Initializes, prepares, and queues visible `"type": "custom"` style layers in painter order.
pub fn queue_custom_layers_system(
    MapContext {
        world,
        style,
        view_state,
        renderer,
        ..
    }: &mut MapContext,
) -> SystemResult {
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
        queue: UploadQueue::new(renderer.queue.inner().clone()),
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
            draw_function: Box::new(DrawCustomLayer),
            index,
            generate_borders: false,
            style_layer,
            tile: Tile {
                coords: crate::coords::WorldTileCoords::default(),
            },
            source_shape: TileShape::default(),
            projection: ProjectionBinding::View,
            run: None,
        });
    }
    Ok(())
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

#[cfg(test)]
mod tests;
