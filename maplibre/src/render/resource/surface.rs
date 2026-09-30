//! Window presentation and offscreen render targets with deferred resizing.

use std::sync::Arc;

mod acquisition;
mod offscreen;
mod window;

pub use acquisition::SurfaceAcquireError;
pub use offscreen::BufferedTextureHead;
#[cfg(feature = "headless")]
pub use offscreen::{BufferDimensions, BufferReadbackError, WriteImageError};
pub use window::WindowHead;

use crate::{
    render::{
        error::RenderError,
        eventually::HasChanged,
        resource::{share_gpu, texture::TextureView},
        settings::{Msaa, RendererSettings},
    },
    window::{HeadedMapWindow, MapWindow, PhysicalSize},
};

/// Backing storage for a render target, including ownership of a window frame or offscreen image.
pub enum Head {
    /// A host window from which a new presentation image is acquired each frame.
    Headed(WindowHead),
    /// A persistent texture shared with capture readers, which can outlive a surface resize.
    Headless(Arc<BufferedTextureHead>),
}

/// A render target's requested dimensions and currently allocated backing storage.
/// Resizing changes the requested size; [`Self::reconfigure`] applies it to GPU resources.
pub struct Surface {
    size: PhysicalSize,
    head: Head,
}

impl Surface {
    /// Wraps an unconfigured window surface using the window's physical dimensions.
    ///
    /// Uses the requested format, otherwise prefers a supported non-sRGB format and falls back
    /// to the first advertised format. The returned window head must be configured before use.
    /// The window is read for its dimensions; ownership comes from the supplied surface.
    pub fn from_surface<MW>(
        surface: wgpu::Surface<'static>,
        adapter: &wgpu::Adapter,
        window: &MW,
        settings: &RendererSettings,
    ) -> Self
    where
        MW: MapWindow + HeadedMapWindow,
    {
        let size = window.size();

        let capabilities = surface.get_capabilities(adapter);
        log::info!("adapter capabilities on surface: {capabilities:?}");

        let texture_format = settings
            .texture_format
            .or_else(|| {
                capabilities
                    .formats
                    .iter()
                    .copied()
                    .find(|format| !format.is_srgb())
            })
            .or_else(|| capabilities.formats.first().cloned())
            .unwrap_or(wgpu::TextureFormat::Rgba8Unorm);
        let render_format = window::strip_srgb(texture_format);
        log::info!("surface format: {texture_format:?}, render format: {render_format:?}");

        let texture_format_features = adapter.get_texture_format_features(texture_format);
        log::info!("format features: {texture_format_features:?}");

        Self {
            size,
            head: Head::Headed(WindowHead {
                surface,
                size,
                texture_format,
                render_format,
                texture_format_features,
                present_mode: settings.present_mode,
            }),
        }
    }

    /// Allocates a single-sample offscreen target at the window's physical size.
    ///
    /// The format defaults to `Rgba8Unorm`. With the `headless` feature this also allocates a
    /// readback buffer with four bytes per pixel; PNG capture requires RGBA8 pixel data.
    /// Dimensions, format and usages must be supported by `device`, or wgpu reports validation
    /// errors. The window is used only for its size and is not retained.
    pub fn from_image<MW>(
        device: &wgpu::Device,
        adapter: &wgpu::Adapter,
        window: &MW,
        settings: &RendererSettings,
    ) -> Self
    where
        MW: MapWindow,
    {
        let size = window.size();

        let format = settings
            .texture_format
            .unwrap_or(wgpu::TextureFormat::Rgba8Unorm);
        Self {
            size,
            head: Head::Headless(share_gpu(BufferedTextureHead::new(
                device,
                size,
                format,
                adapter.get_texture_format_features(format),
            ))),
        }
    }

    /// Format used by render pipelines and attachment views.
    /// For a window this strips the sRGB suffix to avoid converting CSS colors a second time;
    /// an offscreen target retains its explicitly requested format.
    pub fn surface_format(&self) -> wgpu::TextureFormat {
        match &self.head {
            Head::Headed(headed) => headed.render_format,
            Head::Headless(headless) => headless.texture_format,
        }
    }

    /// Acquires a window frame or creates a view of the persistent offscreen texture.
    ///
    /// Window acquisition retries once after reconfiguration for an outdated or suboptimal
    /// frame. Other acquisition failures are returned directly. A successful window frame
    /// must be presented through [`TextureView::take_surface_texture`] or dropped before
    /// reconfiguration. This does not apply a pending [`Self::resize`].
    #[tracing::instrument(name = "create_view", skip_all)]
    pub fn create_view(&self, device: &wgpu::Device) -> Result<TextureView, SurfaceAcquireError> {
        Ok(match &self.head {
            Head::Headed(window) => {
                let WindowHead {
                    surface,
                    render_format,
                    ..
                } = window;
                let frame = acquisition::acquire(
                    || surface.get_current_texture(),
                    || window.configure(device),
                )?;
                // Create view with non-sRGB format to prevent double-gamma on CSS colors
                let view = frame.texture.create_view(&wgpu::TextureViewDescriptor {
                    format: Some(*render_format),
                    ..Default::default()
                });
                TextureView::SurfaceTexture {
                    view,
                    texture: frame,
                }
            }
            Head::Headless(arc) => arc
                .texture
                .create_view(&wgpu::TextureViewDescriptor::default())
                .into(),
        })
    }

    /// Requested physical-pixel dimensions, which may differ from storage until reconfiguration.
    pub fn size(&self) -> PhysicalSize {
        self.size
    }

    /// Records nonzero physical-pixel dimensions without allocating or configuring GPU resources.
    pub fn resize(&mut self, size: PhysicalSize) {
        self.size = size;
    }

    /// Applies a pending size change; an unchanged size leaves resources untouched.
    ///
    /// Drop outstanding window frames before calling. An offscreen resize allocates a new
    /// texture and readback buffer; existing [`Arc`] clones retain the previous allocation.
    /// This does not recreate dependent depth or multisample attachments.
    pub fn reconfigure(&mut self, device: &wgpu::Device) {
        match &mut self.head {
            Head::Headed(window) => {
                if window.has_changed(&(self.size.width(), self.size.height())) {
                    window.resize_and_configure(self.size.width(), self.size.height(), device);
                }
            }
            Head::Headless(head) => {
                if head.texture.width() != self.size.width()
                    || head.texture.height() != self.size.height()
                {
                    *head = share_gpu(BufferedTextureHead::new(
                        device,
                        self.size,
                        head.texture_format,
                        head.texture_format_features,
                    ));
                }
            }
        }
    }

    /// Replaces a window surface when its configured dimensions differ from the requested size.
    ///
    /// Offscreen surfaces and window surfaces with no pending size change are left alone.
    /// Recreation returns host-handle or surface-creation failures without replacing the
    /// existing surface. On success, call [`Self::reconfigure`] before acquiring another frame.
    /// The recreated surface retains a cloned window-handle owner.
    pub fn recreate<MW>(
        &mut self,
        window: &MW,
        instance: &wgpu::Instance,
    ) -> Result<(), RenderError>
    where
        MW: MapWindow + HeadedMapWindow,
    {
        match &mut self.head {
            Head::Headed(window_head) => {
                if window_head.has_changed(&(self.size.width(), self.size.height())) {
                    window_head.recreate_surface(window, instance)?;
                }
            }
            Head::Headless(_) => {}
        }
        Ok(())
    }

    /// Currently allocated backing storage, which may still have the size before a pending resize.
    pub fn head(&self) -> &Head {
        &self.head
    }

    /// Mutable backing storage; replacing it requires matching the requested size and render format.
    pub fn head_mut(&mut self) -> &mut Head {
        &mut self.head
    }

    /// Whether the adapter advertises the exact sample count for the color target format.
    ///
    /// A count of one is supported; zero and non-power-of-two counts are rejected. This checks
    /// neither the selected depth format nor whether optional device features have been enabled.
    pub fn is_multisampling_supported(&self, msaa: Msaa) -> bool {
        let flags = match &self.head {
            Head::Headed(headed) => headed.texture_format_features.flags,
            Head::Headless(headless) => headless.texture_format_features.flags,
        };
        let is_supported = flags.sample_count_supported(msaa.samples);
        if !is_supported {
            log::debug!("Multisampling is not supported on surface");
        }
        is_supported
    }
}

#[cfg(all(test, feature = "headless"))]
mod tests;
