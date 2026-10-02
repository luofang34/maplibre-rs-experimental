//! A renderer on the host's own GPU objects, drawing into a texture the host samples.
//!
//! A host that composites the map itself hands over the wgpu instance, adapter, device and
//! queue it renders with. The map draws each frame into its offscreen texture, in the format
//! and sample count the host asks for and resolved to one sample, and the host binds that
//! texture in its own passes: nothing is read back to the CPU. The handles are reference
//! counted, so the host keeps using its own, and any texture it still holds, after the map
//! resizes or is dropped.

use super::{
    resource::{share_gpu, Surface},
    settings::{RendererSettings, WgpuSettings},
    upload_queue, RenderResources, Renderer,
};
use crate::window::MapWindow;

/// The GPU objects a host renders with.
#[derive(Clone, Debug)]
pub struct HostGpu {
    /// The instance the adapter came from.
    pub instance: wgpu::Instance,
    /// The adapter the device was requested from.
    pub adapter: wgpu::Adapter,
    /// The device both the host and the map draw with.
    pub device: wgpu::Device,
    /// The queue both submit to.
    pub queue: wgpu::Queue,
}

impl Renderer {
    /// A renderer drawing with the host's device into an offscreen texture of `window`'s size,
    /// in `settings.texture_format` (RGBA8 when unset) at `settings.msaa` samples where the
    /// format allows them.
    pub fn on_host_gpu<MW: MapWindow>(
        window: &MW,
        gpu: HostGpu,
        settings: RendererSettings,
    ) -> Self {
        let HostGpu {
            instance,
            adapter,
            device,
            queue,
        } = gpu;
        // The device is already made, so its features and limits are what the map gets.
        let wgpu_settings = WgpuSettings {
            features: device.features(),
            limits: device.limits(),
            ..Default::default()
        };
        let settings = settings
            .with_float_depth_if_supported(device.features())
            .with_backend_msaa(adapter.get_info().backend);
        let surface = Surface::from_image(&device, &adapter, window, &settings);
        Self {
            instance,
            device: share_gpu(device),
            queue: upload_queue::UploadQueue::new(queue),
            adapter,
            wgpu_settings,
            settings,
            resources: RenderResources::new(surface),
            render_graph: Default::default(),
        }
    }
}

#[cfg(test)]
mod tests;
