use std::{ffi::c_void, ptr};

use objc2::{rc::Retained, runtime::ProtocolObject};
use objc2_metal::{MTLPixelFormat, MTLResource, MTLTexture, MTLTextureType, MTLTextureUsage};

use super::SURFACE_FORMAT;

#[cfg(all(test, target_os = "macos"))]
mod tests;

/// Imports a linear colour view, or leaves unsupported formats to the host's copy path.
///
/// # Safety
/// `pointer` must be null or a live `id<MTLTexture>` for the duration of this call.
/// The host must serialize texture access with the map's command queue.
pub(super) unsafe fn import_color_texture(
    device: &wgpu::Device,
    pointer: *const c_void,
) -> Option<wgpu::TextureView> {
    // SAFETY: the caller keeps the borrowed texture alive until it is retained.
    let raw = unsafe { retain_render_target(device, pointer) }?;
    let format = match raw.pixelFormat() {
        MTLPixelFormat::BGRA8Unorm => wgpu::TextureFormat::Bgra8Unorm,
        MTLPixelFormat::BGRA8Unorm_sRGB => wgpu::TextureFormat::Bgra8UnormSrgb,
        other => {
            tracing::warn!(?other, "colour texture format is not drawable");
            return None;
        }
    };
    if format != SURFACE_FORMAT && !raw.usage().contains(MTLTextureUsage::PixelFormatView) {
        return None;
    }
    let texture = wrap_render_target(device, raw, format, &[SURFACE_FORMAT]);
    Some(texture.create_view(&wgpu::TextureViewDescriptor {
        format: Some(SURFACE_FORMAT),
        ..Default::default()
    }))
}

/// Imports a depth target while retaining the host's Metal object.
///
/// # Safety
/// `pointer` must be null or a live `id<MTLTexture>` for the duration of this call.
/// The host must serialize texture access with the map's command queue.
pub(super) unsafe fn import_depth_texture(
    device: &wgpu::Device,
    pointer: *const c_void,
) -> Option<wgpu::TextureView> {
    // SAFETY: the caller keeps the borrowed texture alive until it is retained.
    let raw = unsafe { retain_render_target(device, pointer) }?;
    if raw.pixelFormat() != MTLPixelFormat::Depth32Float {
        tracing::warn!(format = ?raw.pixelFormat(), "depth texture format is not drawable");
        return None;
    }
    let texture = wrap_render_target(device, raw, wgpu::TextureFormat::Depth32Float, &[]);
    Some(texture.create_view(&wgpu::TextureViewDescriptor::default()))
}

unsafe fn retain_render_target(
    device: &wgpu::Device,
    pointer: *const c_void,
) -> Option<Retained<ProtocolObject<dyn MTLTexture>>> {
    // SAFETY: the caller provides a live texture or null; retaining grants an owned reference.
    let raw =
        unsafe { Retained::<ProtocolObject<dyn MTLTexture>>::retain(pointer.cast_mut().cast()) }?;
    if raw.textureType() != MTLTextureType::Type2D
        || raw.mipmapLevelCount() != 1
        || raw.arrayLength() != 1
        || raw.sampleCount() != 1
        || !raw.usage().contains(MTLTextureUsage::RenderTarget)
    {
        tracing::warn!("compositor target must be a single-level 2D render texture");
        return None;
    }
    // SAFETY: the guard only reads the device; the returned texture keeps its own reference.
    let hal_device = unsafe { device.as_hal::<wgpu_hal::api::Metal>() }?;
    if !ptr::eq(&**hal_device.raw_device(), &*raw.device()) {
        tracing::warn!("compositor target belongs to another Metal device");
        return None;
    }
    Some(raw)
}

fn wrap_render_target(
    device: &wgpu::Device,
    raw: Retained<ProtocolObject<dyn MTLTexture>>,
    format: wgpu::TextureFormat,
    view_formats: &[wgpu::TextureFormat],
) -> wgpu::Texture {
    let (width, height) = (raw.width() as u32, raw.height() as u32);
    // SAFETY: retain_render_target checks device identity, dimensions and usage. Ownership
    // moves into HAL, so dropping the wrapper releases only our reference. Each eye clears
    // its attachments; the compositor's existing contents may be discarded.
    unsafe {
        let hal = wgpu_hal::metal::Device::texture_from_raw(
            raw,
            format,
            MTLTextureType::Type2D,
            1,
            1,
            wgpu_hal::CopyExtent {
                width,
                height,
                depth: 1,
            },
            None,
        );
        device.create_texture_from_hal::<wgpu_hal::api::Metal>(
            hal,
            &wgpu::TextureDescriptor {
                label: Some("compositor target"),
                size: wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                view_formats,
            },
            wgpu::wgt::TextureUses::UNINITIALIZED,
        )
    }
}
