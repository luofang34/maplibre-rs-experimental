#![allow(clippy::expect_used, clippy::panic)]

use std::ptr::NonNull;

use objc2_metal::{
    MTLCommandBuffer, MTLCommandBufferStatus, MTLCommandQueue, MTLDevice, MTLOrigin, MTLRegion,
    MTLSize, MTLStorageMode, MTLTextureDescriptor,
};

use super::*;

fn renderer() -> (wgpu::Device, wgpu::Queue) {
    tokio::runtime::Runtime::new()
        .expect("test runtime")
        .block_on(async {
            let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
                backends: wgpu::Backends::METAL,
                ..wgpu::InstanceDescriptor::new_without_display_handle()
            });
            instance
                .request_adapter(&wgpu::RequestAdapterOptions::default())
                .await
                .expect("Metal adapter")
                .request_device(&wgpu::DeviceDescriptor::default())
                .await
                .expect("Metal device")
        })
}

fn host_texture(
    device: &wgpu::Device,
    format: MTLPixelFormat,
    usage: MTLTextureUsage,
) -> Retained<ProtocolObject<dyn MTLTexture>> {
    // SAFETY: all descriptor values describe a valid single-pixel 2D texture.
    let descriptor = unsafe {
        MTLTextureDescriptor::texture2DDescriptorWithPixelFormat_width_height_mipmapped(
            format, 1, 1, false,
        )
    };
    descriptor.setUsage(usage);
    descriptor.setStorageMode(if format == MTLPixelFormat::Depth32Float {
        MTLStorageMode::Private
    } else {
        MTLStorageMode::Shared
    });
    // SAFETY: the guard retains the device while it allocates the host's texture.
    unsafe { device.as_hal::<wgpu_hal::api::Metal>() }
        .expect("Metal device")
        .raw_device()
        .newTextureWithDescriptor(&descriptor)
        .expect("host texture")
}

fn host_waits_blocking(queue: &wgpu::Queue) {
    // SAFETY: the guard borrows the queue only while creating an owned command buffer.
    let buffer = unsafe { queue.as_hal::<wgpu_hal::api::Metal>() }
        .expect("Metal queue")
        .as_raw()
        .commandBuffer()
        .expect("host command buffer");
    buffer.commit();
    buffer.waitUntilCompleted();
    assert_eq!(buffer.status(), MTLCommandBufferStatus::Completed);
}

fn clear(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    color: &wgpu::TextureView,
    depth: &wgpu::TextureView,
) {
    let mut encoder = device.create_command_encoder(&Default::default());
    {
        let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: color,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color {
                        r: 0.25,
                        g: 0.5,
                        b: 0.75,
                        a: 1.0,
                    }),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: depth,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(0.5),
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: None,
            }),
            ..Default::default()
        });
    }
    queue.submit([encoder.finish()]);
}

#[test]
fn imported_targets_retain_host_textures_and_share_queue_order() {
    let (device, queue) = renderer();
    let raw_color = host_texture(
        &device,
        MTLPixelFormat::BGRA8Unorm_sRGB,
        MTLTextureUsage::RenderTarget | MTLTextureUsage::PixelFormatView,
    );
    let raw_depth = host_texture(
        &device,
        MTLPixelFormat::Depth32Float,
        MTLTextureUsage::RenderTarget,
    );
    let color_pointer = ptr::from_ref(&*raw_color);
    // SAFETY: both raw textures are alive and belong to this device and queue.
    let (color, depth) = unsafe {
        (
            import_color_texture(&device, color_pointer.cast()).expect("linear colour view"),
            import_depth_texture(&device, ptr::from_ref(&*raw_depth).cast()).expect("depth view"),
        )
    };
    drop(raw_color);
    drop(raw_depth);
    clear(&device, &queue, &color, &depth);
    host_waits_blocking(&queue);

    let mut pixel = [0u8; 4];
    // SAFETY: the imported view retains the texture even after the host drops its reference.
    // Host work on the same queue has completed, so reading one shared pixel is synchronized.
    unsafe {
        (&*color_pointer).getBytes_bytesPerRow_fromRegion_mipmapLevel(
            NonNull::from(&mut pixel).cast(),
            4,
            MTLRegion {
                origin: MTLOrigin { x: 0, y: 0, z: 0 },
                size: MTLSize {
                    width: 1,
                    height: 1,
                    depth: 1,
                },
            },
            0,
        );
    }
    assert_eq!(
        pixel,
        [191, 128, 64, 255],
        "the sRGB host target receives linear colour values"
    );
}

#[test]
fn unsupported_host_targets_fall_back_without_importing() {
    let (device, _queue) = renderer();
    let srgb = host_texture(
        &device,
        MTLPixelFormat::BGRA8Unorm_sRGB,
        MTLTextureUsage::RenderTarget,
    );
    let color = host_texture(
        &device,
        MTLPixelFormat::BGRA8Unorm,
        MTLTextureUsage::RenderTarget,
    );
    let sampled = host_texture(
        &device,
        MTLPixelFormat::BGRA8Unorm,
        MTLTextureUsage::ShaderRead,
    );
    // SAFETY: each pointer is null or borrows one of the live textures above.
    unsafe {
        assert!(import_color_texture(&device, ptr::null()).is_none());
        assert!(import_depth_texture(&device, ptr::null()).is_none());
        assert!(import_color_texture(&device, ptr::from_ref(&*srgb).cast()).is_none());
        assert!(import_depth_texture(&device, ptr::from_ref(&*color).cast()).is_none());
        assert!(import_color_texture(&device, ptr::from_ref(&*sampled).cast()).is_none());
    }
}
