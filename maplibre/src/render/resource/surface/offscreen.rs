//! Offscreen color texture allocation and optional CPU image readback.

#[cfg(feature = "headless")]
use std::mem::size_of;

use wgpu::TextureFormatFeatures;

use crate::window::PhysicalSize;

#[cfg(feature = "headless")]
/// Physical dimensions and GPU-copy row layout for four-byte RGBA pixels.
pub struct BufferDimensions {
    /// Number of pixels in a row, excluding byte-alignment padding.
    pub width: u32,
    /// Number of rows in the image.
    pub height: u32,
    /// Bytes of RGBA pixel data in each row.
    pub unpadded_bytes_per_row: u32,
    /// Row stride rounded up to [`wgpu::COPY_BYTES_PER_ROW_ALIGNMENT`].
    pub padded_bytes_per_row: u32,
}

#[cfg(feature = "headless")]
impl BufferDimensions {
    fn new(size: PhysicalSize) -> Self {
        let bytes_per_pixel = size_of::<u32>() as u32;
        let unpadded_bytes_per_row = size.width() * bytes_per_pixel;

        let align = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let padded_bytes_per_row_padding = (align - unpadded_bytes_per_row % align) % align;
        let padded_bytes_per_row = unpadded_bytes_per_row + padded_bytes_per_row_padding;
        Self {
            width: size.width(),
            height: size.height(),
            unpadded_bytes_per_row,
            padded_bytes_per_row,
        }
    }
}

/// A persistent single-sample render target with lazy optional CPU readback storage.
/// Rendering does not allocate or populate the readback buffer until a capture requests it.
pub struct BufferedTextureHead {
    pub(super) texture: wgpu::Texture,
    pub(super) texture_view: wgpu::TextureView,
    pub(super) texture_format: wgpu::TextureFormat,
    pub(super) texture_format_features: TextureFormatFeatures,
    pub(super) destroy_texture_on_drop: bool,
    #[cfg(feature = "headless")]
    output_buffer: std::sync::OnceLock<wgpu::Buffer>,
    #[cfg(feature = "headless")]
    buffer_dimensions: BufferDimensions,
}

impl Drop for BufferedTextureHead {
    fn drop(&mut self) {
        #[cfg(feature = "headless")]
        if let Some(buffer) = self.output_buffer.get() {
            buffer.destroy();
        }
        if self.destroy_texture_on_drop {
            self.texture.destroy();
        }
    }
}

impl BufferedTextureHead {
    pub(super) fn new(
        device: &wgpu::Device,
        size: PhysicalSize,
        format: wgpu::TextureFormat,
        features: TextureFormatFeatures,
        destroy_texture_on_drop: bool,
    ) -> Self {
        #[cfg(feature = "headless")]
        let dimensions = BufferDimensions::new(size);
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Surface texture"),
            size: wgpu::Extent3d {
                width: size.width(),
                height: size.height(),
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            // A host samples the frame in its own passes; a readback copies it out.
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::COPY_SRC
                | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[format],
        });
        let texture_view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        Self {
            texture,
            texture_view,
            texture_format: format,
            texture_format_features: features,
            destroy_texture_on_drop,
            #[cfg(feature = "headless")]
            output_buffer: std::sync::OnceLock::new(),
            #[cfg(feature = "headless")]
            buffer_dimensions: dimensions,
        }
    }

    /// The single-sample color texture frames are rendered into.
    pub fn texture(&self) -> &wgpu::Texture {
        &self.texture
    }

    /// Persistent view of [`Self::texture`] for zero-copy GPU composition or shader binding.
    pub fn view(&self) -> &wgpu::TextureView {
        &self.texture_view
    }
}

#[cfg(feature = "headless")]
#[derive(thiserror::Error, Debug)]
/// File creation, PNG encoding or stream-writing failure during an offscreen capture.
pub enum WriteImageError {
    /// PNG header or image encoding failed, including errors from the output stream.
    #[error("error while rendering to image")]
    WriteImage(#[from] png::EncodingError),
    /// The output file could not be created, or writing encoded bytes failed.
    #[error("could not create file to save as an image")]
    CreateImageFileFailed(#[from] std::io::Error),
}

#[cfg(feature = "headless")]
impl BufferedTextureHead {
    /// Waits for GPU work and maps the entire readback buffer for CPU reads.
    ///
    /// Submit a texture-to-buffer copy first. Call only while the buffer is unmapped, drop all
    /// mapped views and call [`Self::unmap`] before copying or mapping again. Returns polling,
    /// mapping or callback-delivery failures. This blocking path is intended for native captures;
    /// browser callbacks require yielding to the browser event loop.
    pub fn map_blocking(
        &self,
        device: &wgpu::Device,
    ) -> Result<wgpu::BufferSlice<'_>, BufferReadbackError> {
        let buffer_slice = self.ensure_buffer(device).slice(..);
        let (sender, receiver) = std::sync::mpsc::sync_channel(1);
        buffer_slice.map_async(wgpu::MapMode::Read, move |result| {
            sender.send(result).ok();
        });
        device.poll(wgpu::PollType::wait_indefinitely())?;
        receiver.recv()??;
        Ok(buffer_slice)
    }

    /// Releases CPU access so the buffer can receive another GPU copy.
    /// All [`wgpu::BufferView`] values must be dropped first, or wgpu panics.
    pub fn unmap(&self) {
        if let Some(buffer) = self.output_buffer.get() {
            buffer.unmap();
        }
    }

    /// Writes mapped RGBA8 pixels as a PNG, skipping GPU row-alignment padding.
    ///
    /// Blocks on filesystem I/O and creates or truncates `png_output_path`. Pass a full mapped
    /// view of this target's readback buffer after GPU completion. Other texture formats are
    /// not converted. The caller retains responsibility for dropping the view and unmapping
    /// the buffer on success or error.
    pub fn write_png(
        &self,
        padded_buffer: &wgpu::BufferView,
        png_output_path: &str,
    ) -> Result<(), WriteImageError> {
        use std::{fs::File, io::Write};
        let mut png_encoder = png::Encoder::new(
            File::create(png_output_path)?,
            self.buffer_dimensions.width,
            self.buffer_dimensions.height,
        );
        png_encoder.set_depth(png::BitDepth::Eight);
        png_encoder.set_color(png::ColorType::Rgba);
        let mut png_writer = png_encoder
            .write_header()?
            .into_stream_writer_with_size(self.buffer_dimensions.unpadded_bytes_per_row as usize)?;

        for chunk in padded_buffer.chunks(self.buffer_dimensions.padded_bytes_per_row as usize) {
            png_writer
                .write_all(&chunk[..self.buffer_dimensions.unpadded_bytes_per_row as usize])?
        }
        png_writer.finish()?;
        Ok(())
    }

    /// Copy source at mip level zero and the origin of this target's only layer.
    pub fn copy_texture(&self) -> wgpu::TexelCopyTextureInfo<'_> {
        self.texture.as_image_copy()
    }

    /// Returns `true` when the `MAP_READ | COPY_DST` staging buffer has been lazily allocated.
    pub fn has_readback_buffer(&self) -> bool {
        self.output_buffer.get().is_some()
    }

    /// Borrows the readback buffer if already allocated, without creating one.
    pub fn try_buffer(&self) -> Option<&wgpu::Buffer> {
        self.output_buffer.get()
    }

    /// Lazily allocates (on first call) and returns the `COPY_DST | MAP_READ` staging buffer.
    pub fn ensure_buffer(&self, device: &wgpu::Device) -> &wgpu::Buffer {
        self.output_buffer.get_or_init(|| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("BufferedTextureHead buffer"),
                size: (self.buffer_dimensions.padded_bytes_per_row * self.buffer_dimensions.height)
                    as u64,
                usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        })
    }

    /// Padded byte stride to use in the texture-to-buffer copy layout.
    pub fn bytes_per_row(&self) -> u32 {
        self.buffer_dimensions.padded_bytes_per_row
    }
}

/// Failure while mapping an offscreen capture buffer.
#[cfg(feature = "headless")]
#[derive(thiserror::Error, Debug)]
pub enum BufferReadbackError {
    /// Waiting for GPU completion failed.
    #[error("waiting for offscreen capture failed")]
    Poll(#[from] wgpu::PollError),
    /// GPU buffer mapping failed.
    #[error("mapping offscreen capture failed")]
    Map(#[from] wgpu::BufferAsyncError),
    /// The mapping callback ended without delivering a result.
    #[error("offscreen capture callback was abandoned")]
    Callback(#[from] std::sync::mpsc::RecvError),
    /// Access to the mapped bytes failed.
    #[error("reading mapped offscreen capture failed")]
    Range(#[from] wgpu::MapRangeError),
}
