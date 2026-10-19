//! Copy a rendered texture to CPU memory.
//!
//! [`read`] and [`read_blocking`] copy any texture with four bytes per texel, colour or
//! depth, into a new vector. [`FrameReadback`] copies a colour target into a buffer the
//! caller keeps, as RGBA or RGB in that order whatever the texture's channel order, and
//! reuses its staging buffer from frame to frame, so a video producer allocates nothing
//! per frame.

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
    /// The mapped range could not be accessed.
    #[error("staging buffer range could not be accessed")]
    Range(#[from] wgpu::MapRangeError),
    /// Waiting for GPU work failed.
    #[error("waiting for texture readback failed")]
    Poll(#[from] wgpu::PollError),
    /// The device dropped the mapping callback without a result.
    #[error("staging buffer mapping was abandoned")]
    Abandoned,
    /// The texture's channels are not four bytes of RGBA or BGRA colour.
    #[error("texture format {0:?} is not 8-bit RGBA or BGRA colour")]
    Format(wgpu::TextureFormat),
    /// The texture was made without `COPY_SRC`, so nothing can copy out of it.
    #[error("texture usage {0:?} lacks COPY_SRC")]
    Usage(wgpu::TextureUsages),
    /// The texture is not the size the readback was made for.
    #[error("texture is {texture:?} pixels but the readback holds {readback:?}")]
    Size {
        /// Width and height of the texture.
        texture: (u32, u32),
        /// Width and height the readback was made for.
        readback: (u32, u32),
    },
    /// The output buffer cannot hold the image.
    #[error("output needs {needed} bytes but has {provided}")]
    BufferTooSmall {
        /// Bytes the image takes in the requested layout.
        needed: usize,
        /// Bytes the caller's buffer has.
        provided: usize,
    },
}

/// Byte order of each pixel written by a [`FrameReadback`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PixelLayout {
    /// Red, green, blue, alpha: four bytes per pixel.
    Rgba8,
    /// Red, green, blue: three bytes per pixel, alpha dropped.
    Rgb8,
}

impl PixelLayout {
    /// Bytes each pixel takes.
    pub const fn bytes_per_pixel(self) -> usize {
        match self {
            Self::Rgba8 => 4,
            Self::Rgb8 => 3,
        }
    }
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

impl Mapped {
    /// Waits for the device, then takes the mapping result the poll delivered.
    fn wait_blocking(self, device: &wgpu::Device) -> Result<(), ReadbackError> {
        device.poll(wgpu::PollType::wait_indefinitely())?;
        let result = self
            .0
            .lock()
            .map_err(|_| ReadbackError::Abandoned)?
            .result
            .take()
            .ok_or(ReadbackError::Abandoned)?;
        result.map_err(ReadbackError::from)
    }
}

/// Row stride of a texture-to-buffer copy: whole rows rounded up to the copy alignment.
fn padded_row(row: u32) -> u32 {
    row.div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT) * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT
}

fn staging_buffer(device: &wgpu::Device, padded: u32, height: u32) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("texture readback"),
        size: u64::from(padded) * u64::from(height),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    })
}

fn copy(
    map: &HeadlessMap,
    texture: &wgpu::Texture,
    aspect: wgpu::TextureAspect,
    buffer: &wgpu::Buffer,
    padded: u32,
) {
    let mut encoder = map.device().create_command_encoder(&Default::default());
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect,
        },
        wgpu::TexelCopyBufferInfo {
            buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(padded),
                rows_per_image: None,
            },
        },
        texture.size(),
    );
    map.queue().submit([encoder.finish()]);
}

fn request(buffer: &wgpu::Buffer) -> Mapped {
    let slot = Arc::new(Mutex::new(Slot::default()));
    let callback_slot = Arc::clone(&slot);
    buffer
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

struct Staging {
    buffer: wgpu::Buffer,
    row: usize,
    padded: usize,
}

fn stage(map: &HeadlessMap, texture: &wgpu::Texture, aspect: wgpu::TextureAspect) -> Staging {
    let row = texture.width() * 4;
    let padded = padded_row(row);
    let buffer = staging_buffer(map.device(), padded, texture.height());
    copy(map, texture, aspect, &buffer, padded);
    Staging {
        buffer,
        row: row as usize,
        padded: padded as usize,
    }
}

fn unpad(staging: Staging) -> Result<Vec<u8>, ReadbackError> {
    let mapped = staging.buffer.slice(..).get_mapped_range()?;
    let bytes = mapped
        .chunks_exact(staging.padded)
        .flat_map(|row| row[..staging.row].iter().copied())
        .collect();
    drop(mapped);
    staging.buffer.unmap();
    Ok(bytes)
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
    request(&staging.buffer).await?;
    unpad(staging)
}

/// Copy a texture to CPU memory with tightly packed rows, and wait for the device.
pub fn read_blocking(
    map: &HeadlessMap,
    texture: &wgpu::Texture,
    aspect: wgpu::TextureAspect,
) -> Result<Vec<u8>, ReadbackError> {
    let staging = stage(map, texture, aspect);
    request(&staging.buffer).wait_blocking(map.device())?;
    unpad(staging)
}

/// Copies colour frames of one size into caller buffers through one reusable staging buffer.
///
/// A texture to read must have `COPY_SRC` usage, the readback's size, and an 8-bit RGBA or
/// BGRA format. The output is RGBA or RGB, rows packed top to bottom, whichever channel
/// order the texture stores. sRGB formats are read as stored, without decoding.
pub struct FrameReadback {
    buffer: wgpu::Buffer,
    width: u32,
    height: u32,
    padded: u32,
}

impl FrameReadback {
    /// Allocate the staging buffer for frames of `width` by `height` pixels.
    pub fn new(map: &HeadlessMap, width: u32, height: u32) -> Self {
        let padded = padded_row(width * 4);
        Self {
            buffer: staging_buffer(map.device(), padded, height),
            width,
            height,
            padded,
        }
    }

    /// Width and height of the frames this readback copies.
    pub fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    /// Bytes one frame takes in `layout`.
    pub fn frame_len(&self, layout: PixelLayout) -> usize {
        self.width as usize * self.height as usize * layout.bytes_per_pixel()
    }

    /// Copy `texture` into the start of `out` in `layout`. Suits WebGPU.
    ///
    /// On native targets use [`Self::read_into_blocking`]; this future waits for a device
    /// poll that it does not issue. Bytes of `out` past the frame keep their values. A
    /// future dropped before it resolves leaves the staging buffer mapping, so make a new
    /// readback after cancelling one.
    pub async fn read_into(
        &mut self,
        map: &HeadlessMap,
        texture: &wgpu::Texture,
        layout: PixelLayout,
        out: &mut [u8],
    ) -> Result<(), ReadbackError> {
        let bgra = self.check(texture, layout, out)?;
        copy(
            map,
            texture,
            wgpu::TextureAspect::All,
            &self.buffer,
            self.padded,
        );
        request(&self.buffer).await?;
        self.unpack(bgra, layout, out)
    }

    /// Copy `texture` into the start of `out` in `layout`, and wait for the device.
    pub fn read_into_blocking(
        &mut self,
        map: &HeadlessMap,
        texture: &wgpu::Texture,
        layout: PixelLayout,
        out: &mut [u8],
    ) -> Result<(), ReadbackError> {
        let bgra = self.check(texture, layout, out)?;
        copy(
            map,
            texture,
            wgpu::TextureAspect::All,
            &self.buffer,
            self.padded,
        );
        request(&self.buffer).wait_blocking(map.device())?;
        self.unpack(bgra, layout, out)
    }

    /// Rejects a texture or buffer the copy cannot serve before any GPU work, because the
    /// device reports a bad copy as an uncaptured error rather than to the caller. Returns
    /// whether the texture stores blue first.
    fn check(
        &self,
        texture: &wgpu::Texture,
        layout: PixelLayout,
        out: &[u8],
    ) -> Result<bool, ReadbackError> {
        let bgra = match texture.format() {
            wgpu::TextureFormat::Rgba8Unorm | wgpu::TextureFormat::Rgba8UnormSrgb => false,
            wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Bgra8UnormSrgb => true,
            format => return Err(ReadbackError::Format(format)),
        };
        if !texture.usage().contains(wgpu::TextureUsages::COPY_SRC) {
            return Err(ReadbackError::Usage(texture.usage()));
        }
        let size = (texture.width(), texture.height());
        if size != self.size() || texture.depth_or_array_layers() != 1 {
            return Err(ReadbackError::Size {
                texture: size,
                readback: self.size(),
            });
        }
        let needed = self.frame_len(layout);
        if out.len() < needed {
            return Err(ReadbackError::BufferTooSmall {
                needed,
                provided: out.len(),
            });
        }
        Ok(bgra)
    }

    fn unpack(&self, bgra: bool, layout: PixelLayout, out: &mut [u8]) -> Result<(), ReadbackError> {
        let mapped = match self.buffer.slice(..).get_mapped_range() {
            Ok(mapped) => mapped,
            Err(error) => {
                self.buffer.unmap();
                return Err(error.into());
            }
        };
        let row = self.width as usize * 4;
        let size = layout.bytes_per_pixel();
        let (red, blue) = if bgra { (2, 0) } else { (0, 2) };
        for (source, target) in mapped
            .chunks_exact(self.padded as usize)
            .zip(out.chunks_exact_mut(self.width as usize * size))
        {
            let source = &source[..row];
            if layout == PixelLayout::Rgba8 && !bgra {
                target.copy_from_slice(source);
                continue;
            }
            for (pixel, texel) in target.chunks_exact_mut(size).zip(source.as_chunks::<4>().0) {
                pixel[0] = texel[red];
                pixel[1] = texel[1];
                pixel[2] = texel[blue];
                if let Some(alpha) = pixel.get_mut(3) {
                    *alpha = texel[3];
                }
            }
        }
        drop(mapped);
        self.buffer.unmap();
        Ok(())
    }
}

#[cfg(test)]
mod tests;
