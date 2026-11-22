#![allow(clippy::expect_used)]

use super::*;

async fn test_device_and_queue() -> Option<(wgpu::Device, wgpu::Queue)> {
    let instance = wgpu::Instance::default();
    let adapter = instance
        .request_adapter(&wgpu::RequestAdapterOptions::default())
        .await
        .ok()?;
    adapter
        .request_device(&wgpu::DeviceDescriptor::default())
        .await
        .ok()
}

async fn readback_bytes(
    device: &wgpu::Device,
    queue: &UploadQueue,
    src: &wgpu::Buffer,
    size: u64,
) -> Vec<u8> {
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("readback"),
        size,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    encoder.copy_buffer_to_buffer(src, 0, &readback, 0, size);
    queue.submit([encoder.finish()]);

    let slice = readback.slice(..);
    let (tx, rx) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |res| {
        drop(tx.send(res));
    });
    drop(device.poll(wgpu::PollType::wait_indefinitely()));
    rx.recv().expect("map recv").expect("map ok");
    let bytes = slice.get_mapped_range().expect("mapped range").to_vec();
    readback.unmap();
    bytes
}

#[tokio::test]
async fn buffer_and_texture_writes_are_counted_and_taken() {
    let Some((device, queue)) = test_device_and_queue().await else {
        return;
    };
    let queue = UploadQueue::new(queue);
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: 64,
        usage: wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: None,
        size: wgpu::Extent3d {
            width: 4,
            height: 4,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    queue.write_buffer(&buffer, 0, &[0; 64]);
    queue.write_texture(
        texture.as_image_copy(),
        &[0; 64],
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(16),
            rows_per_image: None,
        },
        wgpu::Extent3d {
            width: 4,
            height: 4,
            depth_or_array_layers: 1,
        },
    );
    assert_eq!(queue.take_written_bytes(), 128);
    assert_eq!(queue.take_written_bytes(), 0, "taking starts a new count");
    queue.create_buffer_init(
        &device,
        &wgpu::util::BufferInitDescriptor {
            label: None,
            contents: &[0; 32],
            usage: wgpu::BufferUsages::VERTEX,
        },
    );
    queue.create_texture_with_data(
        &device,
        &wgpu::TextureDescriptor {
            label: None,
            size: wgpu::Extent3d {
                width: 4,
                height: 4,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        },
        wgpu::util::TextureDataOrder::LayerMajor,
        &[0; 64],
    );
    assert_eq!(
        queue.take_written_bytes(),
        96,
        "buffers and textures created with their contents are uploads too"
    );
}

#[tokio::test]
async fn staged_small_writes_coalesce_and_preserve_overwrite_order() {
    let Some((device, queue)) = test_device_and_queue().await else {
        return;
    };
    let queue = UploadQueue::with_staging(queue);
    let buf_a = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("buf_a"),
        size: 64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });
    let buf_b = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("buf_b"),
        size: 32,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });

    // Initial write + immediate identical-range overwrite + interleaved buffer write + partial overwrite.
    queue.write_buffer(&buf_a, 0, &[1u8; 64]);
    queue.write_buffer(&buf_a, 0, &[2u8; 64]);
    queue.write_buffer(&buf_b, 0, &[9u8; 32]);
    queue.write_buffer(&buf_a, 16, &[7u8; 16]);
    assert_eq!(queue.take_written_bytes(), 64 + 64 + 32 + 16);

    let staged_cmd = queue
        .flush_staged(&device)
        .expect("staged writes produce copy command buffer");
    queue.submit([staged_cmd]);
    assert_eq!(
        queue.take_written_bytes(),
        0,
        "staging flush does not double-count logical bytes"
    );

    let got_a = readback_bytes(&device, &queue, &buf_a, 64).await;
    let got_b = readback_bytes(&device, &queue, &buf_b, 32).await;

    let mut expected_a = vec![2u8; 64];
    expected_a[16..32].fill(7u8);
    assert_eq!(got_a, expected_a);
    assert_eq!(got_b, vec![9u8; 32]);
}

#[tokio::test]
async fn interleaved_staged_and_large_direct_writes_preserve_buffer_order() {
    let Some((device, queue)) = test_device_and_queue().await else {
        return;
    };
    let queue = UploadQueue::with_staging(queue);
    let size = 65_536u64;
    let buf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("interleaved_buf"),
        size,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });

    // 1. Small staged write at [0..32] (= 0xAA), followed by >=64KiB direct write (= 0x55),
    //    followed by small staged patch at [8..16] (= 0xCC).
    queue.write_buffer(&buf, 0, &[0xAA; 32]);
    queue.write_buffer(&buf, 0, &vec![0x55; size as usize]);
    queue.write_buffer(&buf, 8, &[0xCC; 8]);

    let staged_cmd = queue
        .flush_staged(&device)
        .expect("trailing patch should be staged");
    queue.submit([staged_cmd]);

    let got = readback_bytes(&device, &queue, &buf, size).await;
    assert_eq!(&got[0..8], &[0x55; 8]);
    assert_eq!(&got[8..16], &[0xCC; 8]);
    assert_eq!(&got[16..64], &[0x55; 48]);
}

#[tokio::test]
async fn cancel_staged_buffer_before_destroy_prevents_validation_error() {
    let Some((device, queue)) = test_device_and_queue().await else {
        return;
    };
    let queue = UploadQueue::with_staging(queue);
    let dead = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("dead_buf"),
        size: 64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });
    let live = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("live_buf"),
        size: 64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });

    let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
    queue.write_buffer(&dead, 0, &[0xDE; 64]);
    queue.write_buffer(&live, 0, &[0x42; 64]);
    queue.cancel_staged_buffer(&dead);
    dead.destroy();

    let staged_cmd = queue
        .flush_staged(&device)
        .expect("live buffer copy still staged");
    queue.submit([staged_cmd]);

    let err = scope.pop().await;
    assert!(err.is_none(), "unexpected validation error: {err:?}");

    let got_live = readback_bytes(&device, &queue, &live, 64).await;
    assert_eq!(got_live, vec![0x42; 64]);
}

#[test]
fn staging_is_enabled_for_non_gl_backends() {
    assert!(UploadQueue::should_stage_for_backend(
        true,
        wgpu::Backend::BrowserWebGpu
    ));
    assert!(UploadQueue::should_stage_for_backend(
        false,
        wgpu::Backend::Metal
    ));
    assert!(UploadQueue::should_stage_for_backend(
        false,
        wgpu::Backend::Vulkan
    ));
    assert!(UploadQueue::should_stage_for_backend(
        false,
        wgpu::Backend::Dx12
    ));
    assert!(!UploadQueue::should_stage_for_backend(
        true,
        wgpu::Backend::Gl
    ));
    assert!(!UploadQueue::should_stage_for_backend(
        false,
        wgpu::Backend::Gl
    ));
}

#[tokio::test]
async fn consecutive_staged_frames_reuse_mapped_ring_chunks_after_poll() {
    let Some((device, queue)) = test_device_and_queue().await else {
        return;
    };
    let queue = UploadQueue::with_staging(queue);
    let dst = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("ring_dst"),
        size: 128,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });

    for round in 1_u8..=4 {
        queue.write_buffer(&dst, 0, &[round; 64]);
        queue.write_buffer(&dst, 64, &[round.wrapping_mul(3); 64]);
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        assert!(queue.encode_staged_into(&device, &mut encoder));
        queue.submit([encoder.finish()]);
        drop(device.poll(wgpu::PollType::wait_indefinitely()));

        let got = readback_bytes(&device, &queue, &dst, 128).await;
        assert_eq!(got.get(..64), Some(&[round; 64][..]));
        assert_eq!(got.get(64..), Some(&[round.wrapping_mul(3); 64][..]));
    }
}
