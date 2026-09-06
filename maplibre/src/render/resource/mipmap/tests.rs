#![allow(clippy::expect_used, clippy::panic)]

use super::{mip_level_count, MipmapGenerator};
use crate::render::resource::Texture;

#[test]
fn a_texture_has_levels_down_to_one_texel() {
    assert_eq!(mip_level_count(1, 1), 1);
    assert_eq!(mip_level_count(4, 4), 3);
    assert_eq!(mip_level_count(1024, 1024), 11);
    assert_eq!(mip_level_count(1024, 16), 11);
    assert_eq!(mip_level_count(0, 0), 1);
}

async fn device() -> (wgpu::Device, wgpu::Queue) {
    let instance = wgpu::Instance::default();
    let adapter = instance
        .request_adapter(&wgpu::RequestAdapterOptions::default())
        .await
        .expect("an adapter");
    adapter
        .request_device(&wgpu::DeviceDescriptor::default(), None)
        .await
        .expect("a device")
}

/// Reads one mip level of an RGBA8 texture, `size` texels square, as bytes.
fn read_level(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
    level: u32,
    size: u32,
) -> Vec<u8> {
    let bytes_per_row = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: u64::from(bytes_per_row * size),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    encoder.copy_texture_to_buffer(
        wgpu::ImageCopyTexture {
            texture,
            mip_level: level,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::ImageCopyBuffer {
            buffer: &buffer,
            layout: wgpu::ImageDataLayout {
                offset: 0,
                bytes_per_row: Some(bytes_per_row),
                rows_per_image: None,
            },
        },
        wgpu::Extent3d {
            width: size,
            height: size,
            depth_or_array_layers: 1,
        },
    );
    queue.submit([encoder.finish()]);
    buffer.slice(..).map_async(wgpu::MapMode::Read, |_| ());
    device.poll(wgpu::Maintain::Wait);
    let data = buffer.slice(..).get_mapped_range();
    let pixels: Vec<u8> = (0..size)
        .flat_map(|row| {
            let start = (row * bytes_per_row) as usize;
            data[start..start + (size * 4) as usize].to_vec()
        })
        .collect();
    drop(data);
    buffer.unmap();
    pixels
}

#[tokio::test]
async fn each_level_averages_the_one_above() {
    let (device, queue) = device().await;
    let format = wgpu::TextureFormat::Rgba8Unorm;
    let texture = Texture::new_mipmapped(
        Some("mipmap test"),
        &device,
        format,
        4,
        4,
        wgpu::TextureUsages::RENDER_ATTACHMENT
            | wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::COPY_DST
            | wgpu::TextureUsages::COPY_SRC,
    );
    assert_eq!(texture.texture.mip_level_count(), 3);
    // Four quadrants, so a downsample that flips the image or swaps the axes fails.
    let mut top = Vec::with_capacity(4 * 4 * 4);
    for row in 0..4 {
        for column in 0..4 {
            top.extend_from_slice(match (row < 2, column < 2) {
                (true, true) => &[255, 0, 0, 255],
                (true, false) => &[0, 0, 255, 255],
                (false, true) => &[0, 255, 0, 255],
                (false, false) => &[255, 255, 255, 255],
            });
        }
    }
    queue.write_texture(
        texture.texture.as_image_copy(),
        &top,
        wgpu::ImageDataLayout {
            offset: 0,
            bytes_per_row: Some(16),
            rows_per_image: None,
        },
        texture.size,
    );
    let generator = MipmapGenerator::new(&device, format);
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    generator.generate(&device, &mut encoder, &texture.texture);
    queue.submit([encoder.finish()]);

    let half = read_level(&device, &queue, &texture.texture, 1, 2);
    assert_eq!(&half[0..4], &[255, 0, 0, 255], "top left stays red");
    assert_eq!(&half[4..8], &[0, 0, 255, 255], "top right stays blue");
    assert_eq!(&half[8..12], &[0, 255, 0, 255], "bottom left stays green");
    assert_eq!(
        &half[12..16],
        &[255, 255, 255, 255],
        "bottom right stays white"
    );
    let one = read_level(&device, &queue, &texture.texture, 2, 1);
    assert!(
        one[..3].iter().all(|channel| (126..=129).contains(channel)),
        "the last level mixes all four quadrants: {one:?}"
    );
}
