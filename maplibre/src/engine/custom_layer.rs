//! Host-provided GPU custom style layers (`"type": "custom"`), drawn in exact style painter order.

#![deny(missing_docs)]

use std::collections::BTreeMap;

use crate::{
    coords::LatLon,
    render::{
        render_phase::{Draw, LayerItem},
        tracked_pass::TrackedRenderPass,
        upload_queue::UploadQueue,
    },
    tcs::world::World,
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

    /// Releases GPU/host resources owned by this layer when it is removed, replaced, torn down
    /// by a style change, reset onto a different render target, or when the map is destroyed.
    fn deinitialize(&mut self) {}
}

pub(crate) struct RegisteredCustomLayer {
    pub(crate) layer: Box<dyn CustomStyleLayer>,
    pub(crate) initialized_target: Option<(wgpu::TextureFormat, u32)>,
}

impl RegisteredCustomLayer {
    pub(crate) fn deinitialize_if_initialized(&mut self) {
        if self.initialized_target.take().is_some() {
            self.layer.deinitialize();
        }
    }
}

impl Drop for RegisteredCustomLayer {
    fn drop(&mut self) {
        self.deinitialize_if_initialized();
    }
}

/// Registry of host [`CustomStyleLayer`] implementations stored in `World::resources`.
#[derive(Default)]
pub struct CustomLayerRegistry {
    pub(crate) layers: BTreeMap<String, RegisteredCustomLayer>,
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
            entry.deinitialize_if_initialized();
        }
    }

    pub(crate) fn deinitialize_layers(&mut self, ids: &[String]) {
        for id in ids {
            if let Some(entry) = self.layers.get_mut(id) {
                entry.deinitialize_if_initialized();
            }
        }
    }
}

pub(crate) struct CustomLayerFrameState {
    pub(crate) device: std::sync::Arc<wgpu::Device>,
    pub(crate) queue: UploadQueue,
    pub(crate) color_format: wgpu::TextureFormat,
    pub(crate) depth_stencil_format: wgpu::TextureFormat,
    pub(crate) sample_count: u32,
    pub(crate) viewport: [u32; 2],
    pub(crate) view_proj: [[f64; 4]; 4],
    pub(crate) view_proj_f32: [[f32; 4]; 4],
    pub(crate) center: LatLon,
    pub(crate) zoom: f64,
    pub(crate) pitch_deg: f64,
    pub(crate) bearing_deg: f64,
    pub(crate) globe_transition: f32,
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
        let Some(entry) = registry.layers.get(item.style_layer.as_str()) else {
            return;
        };
        let ctx = frame.as_context();
        pass.with_raw_pass(|raw_pass| {
            entry.layer.render(raw_pass, &ctx);
        });
    }
}

#[cfg(test)]
mod tests;
