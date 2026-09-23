//! Copy a colour or depth texture with four bytes per texel to CPU memory.

use std::{
    future::Future,
    pin::Pin,
    sync::{Arc, Mutex},
    task::{Context, Poll, Waker},
};

use thiserror::Error;

use crate::headless::map::HeadlessMap;

/// Why a texture could not be read back.
#[derive(Debug, Error)]
pub enum ReadbackError {
    /// The device refused to map the staging buffer.
    #[error("staging buffer could not be mapped")]
    Map(#[from] wgpu::BufferAsyncError),
    /// The device dropped the mapping callback without a result.
    #[error("staging buffer mapping was abandoned")]
    Abandoned,
}

type MapResult = Result<(), wgpu::BufferAsyncError>;

#[derive(Default)]
struct Slot {
    result: Option<MapResult>,
    waker: Option<Waker>,
}

struct Mapped(Arc<Mutex<Slot>>);

impl Future for Mapped {
    type Output = Result<(), ReadbackError>;

    fn poll(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        let Ok(mut slot) = self.0.lock() else {
            return Poll::Ready(Err(ReadbackError::Abandoned));
        };
        match slot.result.take() {
            Some(result) => Poll::Ready(result.map_err(ReadbackError::from)),
            None => {
                slot.waker = Some(context.waker().clone());
                Poll::Pending
            }
        }
    }
}

struct Staging {
    buffer: wgpu::Buffer,
    row: usize,
    padded: usize,
}

fn stage(map: &HeadlessMap, texture: &wgpu::Texture, aspect: wgpu::TextureAspect) -> Staging {
    let row = texture.width() * 4;
    let padded =
        row.div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT) * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
    let buffer = map.device().create_buffer(&wgpu::BufferDescriptor {
        label: Some("reference readback"),
        size: u64::from(padded) * u64::from(texture.height()),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = map.device().create_command_encoder(&Default::default());
    encoder.copy_texture_to_buffer(
        wgpu::ImageCopyTexture {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect,
        },
        wgpu::ImageCopyBuffer {
            buffer: &buffer,
            layout: wgpu::ImageDataLayout {
                offset: 0,
                bytes_per_row: Some(padded),
                rows_per_image: None,
            },
        },
        texture.size(),
    );
    map.queue().submit([encoder.finish()]);
    Staging {
        buffer,
        row: row as usize,
        padded: padded as usize,
    }
}

fn request(staging: &Staging) -> Mapped {
    let slot = Arc::new(Mutex::new(Slot::default()));
    let callback_slot = Arc::clone(&slot);
    staging
        .buffer
        .slice(..)
        .map_async(wgpu::MapMode::Read, move |result| {
            if let Ok(mut slot) = callback_slot.lock() {
                slot.result = Some(result);
                if let Some(waker) = slot.waker.take() {
                    waker.wake();
                }
            }
        });
    Mapped(slot)
}

fn unpad(staging: Staging) -> Vec<u8> {
    let mapped = staging.buffer.slice(..).get_mapped_range();
    let bytes = mapped
        .chunks_exact(staging.padded)
        .flat_map(|row| row[..staging.row].iter().copied())
        .collect();
    drop(mapped);
    staging.buffer.unmap();
    bytes
}

/// Copy a texture to CPU memory with tightly packed rows. Suits WebGPU.
///
/// On native targets use [`read_blocking`]; this future waits for a device
/// poll that it does not issue.
pub async fn read(
    map: &HeadlessMap,
    texture: &wgpu::Texture,
    aspect: wgpu::TextureAspect,
) -> Result<Vec<u8>, ReadbackError> {
    let staging = stage(map, texture, aspect);
    request(&staging).await?;
    Ok(unpad(staging))
}

/// Copy a texture to CPU memory with tightly packed rows, and wait for the device.
pub fn read_blocking(
    map: &HeadlessMap,
    texture: &wgpu::Texture,
    aspect: wgpu::TextureAspect,
) -> Result<Vec<u8>, ReadbackError> {
    let staging = stage(map, texture, aspect);
    let mapped = request(&staging);
    map.device().poll(wgpu::Maintain::Wait);
    let result = mapped
        .0
        .lock()
        .map_err(|_| ReadbackError::Abandoned)?
        .result
        .take()
        .ok_or(ReadbackError::Abandoned)?;
    result?;
    Ok(unpad(staging))
}
