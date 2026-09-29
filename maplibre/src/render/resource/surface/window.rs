//! Window surface configuration and host-handle recreation.

use wgpu::TextureFormatFeatures;

use crate::{
    render::{error::RenderError, eventually::HasChanged},
    window::{HeadedMapWindow, MapWindow, PhysicalSize},
};

/// A host-owned window's presentation surface and its configured dimensions and format.
/// The host window must remain alive until this value and its acquired frames are dropped.
pub struct WindowHead {
    pub(super) surface: wgpu::Surface<'static>,
    pub(super) size: PhysicalSize,

    pub(super) texture_format: wgpu::TextureFormat,
    /// Non-sRGB variant of texture_format used for rendering.
    /// Prevents automatic linear→sRGB conversion by the GPU, since our colors
    /// (from CSS) are already in sRGB space.
    pub(super) render_format: wgpu::TextureFormat,
    pub(super) present_mode: wgpu::PresentMode,
    pub(super) texture_format_features: TextureFormatFeatures,
}

/// Returns the non-sRGB variant of a texture format.
/// This prevents the GPU from applying automatic linear→sRGB gamma conversion,
/// which would double-gamma colors that are already in sRGB space (e.g., CSS colors).
pub(super) fn strip_srgb(format: wgpu::TextureFormat) -> wgpu::TextureFormat {
    match format {
        wgpu::TextureFormat::Rgba8UnormSrgb => wgpu::TextureFormat::Rgba8Unorm,
        wgpu::TextureFormat::Bgra8UnormSrgb => wgpu::TextureFormat::Bgra8Unorm,
        other => other,
    }
}

impl WindowHead {
    /// Configures a new physical-pixel size using the selected format and presentation mode.
    ///
    /// # Panics
    /// Panics if either dimension is zero. Wgpu also rejects unsupported dimensions or
    /// configurations, and outstanding acquired frames must be dropped before this call.
    pub fn resize_and_configure(&mut self, width: u32, height: u32, device: &wgpu::Device) {
        self.size = PhysicalSize::new(width, height).unwrap();
        self.configure(device);
    }

    /// Applies the stored dimensions and presentation settings to the platform surface.
    /// Drop all acquired frames first. The device must belong to an adapter compatible with
    /// this surface and support its format, dimensions and presentation mode.
    pub fn configure(&self, device: &wgpu::Device) {
        // The base format is implicit; listing it would require unsupported WebGL view formats.
        let mut view_formats = Vec::new();
        if self.render_format != self.texture_format {
            view_formats.push(self.render_format);
        }

        let surface_config = wgpu::SurfaceConfiguration {
            alpha_mode: wgpu::CompositeAlphaMode::Auto,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format: self.texture_format,
            width: self.size.width(),
            height: self.size.height(),
            present_mode: self.present_mode,
            view_formats,
            desired_maximum_frame_latency: 2,
            color_space: Default::default(),
        };

        self.surface.configure(device, &surface_config);
    }

    /// Replaces the platform surface while retaining size and presentation settings.
    ///
    /// Returns window-handle or surface-creation failures without changing the existing surface.
    /// Call [`Self::configure`] on success before acquiring frames. The caller must keep the
    /// supplied window alive while the new surface or any of its frames exist.
    pub fn recreate_surface<MW>(
        &mut self,
        window: &MW,
        instance: &wgpu::Instance,
    ) -> Result<(), RenderError>
    where
        MW: MapWindow + HeadedMapWindow,
    {
        self.surface = unsafe {
            instance
                .create_surface_unsafe(wgpu::SurfaceTargetUnsafe::from_window(&window.handle())?)?
        };
        Ok(())
    }

    /// Platform surface for acquisition; its configuration is managed by this window head.
    pub fn surface(&self) -> &wgpu::Surface<'_> {
        &self.surface
    }
}

impl HasChanged for WindowHead {
    /// Tuple of width and height
    type Criteria = (u32, u32);

    fn has_changed(&self, criteria: &Self::Criteria) -> bool {
        self.size.width() != criteria.0 || self.size.height() != criteria.1
    }
}
