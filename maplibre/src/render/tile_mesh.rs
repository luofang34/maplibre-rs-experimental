//! GPU cache for projection-aware raster and stencil tile meshes.

use std::{collections::HashMap, num::TryFromIntError};

use bytemuck_derive::{Pod, Zeroable};
use thiserror::Error;

use crate::{
    coords::{WorldTileCoords, ZOOM_BOUNDS},
    projection::globe::tile_mesh::{
        create_tile_mesh_into, TileIndexType, TileMeshError, TileMeshIndicesRef, TileMeshOptions,
        TileMeshScratch,
    },
    render::resource::VertexBufferLayout,
};

/// 32-bit float GPU vertex position uploaded for [`GpuTileMesh`] draws.
///
/// Uploading 32-bit float `[x, y]` (`Float32x2`, 8-byte stride) with `Uint32` indices via
/// `COPY_DST` avoids sub-aligned 16-bit vertex/index stream corruption on mobile Vulkan drivers.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Pod, Zeroable)]
pub struct GpuTileMeshVertex {
    /// Tile-space `[x, y]` coordinates (`0.0..=4096.0` on body, `-32768.0`/`32767.0` on poles).
    pub position: [f32; 2],
}

impl GpuTileMeshVertex {
    /// Vertex buffer layout at `@location(0)` (`Float32x2`, stride 8).
    pub fn vertex_buffer_layout() -> VertexBufferLayout {
        VertexBufferLayout {
            array_stride: std::mem::size_of::<Self>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: vec![wgpu::VertexAttribute {
                offset: 0,
                format: wgpu::VertexFormat::Float32x2,
                shader_location: 0,
            }],
        }
    }
}

/// Rendering purpose controlling globe mesh subdivision.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TileMeshUsage {
    /// Textured tile geometry.
    Raster,
    /// Tile clipping-mask geometry.
    Stencil,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct TileMeshKey {
    granularity: u32,
    generate_borders: bool,
    extend_to_north_pole: bool,
    extend_to_south_pole: bool,
}

/// GPU buffers for one tile-mesh variant.
pub struct GpuTileMesh {
    vertex_buffer: wgpu::Buffer,
    index_buffer: wgpu::Buffer,
    index_format: wgpu::IndexFormat,
    index_count: u32,
}

impl GpuTileMesh {
    /// Returns the vertex buffer containing 32-bit float tile positions.
    pub fn vertex_buffer(&self) -> &wgpu::Buffer {
        &self.vertex_buffer
    }

    /// Returns the triangle index buffer.
    pub fn index_buffer(&self) -> &wgpu::Buffer {
        &self.index_buffer
    }

    /// Returns the index width used by the buffer.
    pub fn index_format(&self) -> wgpu::IndexFormat {
        self.index_format
    }

    /// Returns the number of indices to draw.
    pub fn index_count(&self) -> u32 {
        self.index_count
    }
}

/// Failure while preparing a GPU tile mesh.
#[derive(Debug, Error)]
pub enum GpuTileMeshError {
    /// CPU mesh generation failed.
    #[error("failed to generate tile mesh")]
    Generate {
        /// Underlying mesh error.
        #[source]
        source: TileMeshError,
    },
    /// Index count cannot be represented by a WebGPU draw range.
    #[error("tile mesh index count exceeds u32")]
    IndexCount {
        /// Failed integer conversion.
        #[source]
        source: TryFromIntError,
    },
}

/// Lazily populated GPU cache keyed by subdivision, borders, and pole extensions.
#[derive(Default)]
pub struct GlobeTileMeshCache {
    meshes: HashMap<TileMeshKey, GpuTileMesh>,
    scratch: TileMeshScratch,
    globe_active: bool,
}

impl GlobeTileMeshCache {
    /// Creates a new empty tile mesh cache.
    pub fn new() -> Self {
        Self::default()
    }

    /// Updates whether spherical globe projection (`transition > 0.0`) is active this frame.
    ///
    /// Flat Mercator frames (`!globe_active`) reuse a minimal `1x1` quad (`granularity = 1`, no
    /// pole caps) instead of uploading dense `128x128` globe grids.
    pub fn set_globe_active(&mut self, globe_active: bool) {
        self.globe_active = globe_active;
    }

    /// Returns a reference to the reusable scratch buffer.
    pub fn scratch(&self) -> &TileMeshScratch {
        &self.scratch
    }

    fn key_for(
        &self,
        coords: WorldTileCoords,
        usage: TileMeshUsage,
        generate_borders: bool,
    ) -> TileMeshKey {
        if self.globe_active {
            mesh_key(coords, usage, generate_borders)
        } else {
            flat_mesh_key(generate_borders)
        }
    }

    /// Creates the mesh variant needed by a draw if it is not cached.
    pub fn prepare(
        &mut self,
        device: &wgpu::Device,
        queue: &crate::render::upload_queue::UploadQueue,
        coords: WorldTileCoords,
        usage: TileMeshUsage,
        generate_borders: bool,
    ) -> Result<(), GpuTileMeshError> {
        let key = self.key_for(coords, usage, generate_borders);
        if self.meshes.contains_key(&key) {
            return Ok(());
        }
        let views = create_tile_mesh_into(
            &mut self.scratch,
            TileMeshOptions {
                granularity: key.granularity,
                generate_borders: key.generate_borders,
                extend_to_north_pole: key.extend_to_north_pole,
                extend_to_south_pole: key.extend_to_south_pole,
            },
            TileIndexType::U32,
        )
        .map_err(|source| GpuTileMeshError::Generate { source })?;
        let index_count = u32::try_from(views.indices.len())
            .map_err(|source| GpuTileMeshError::IndexCount { source })?;
        let gpu_vertices: Vec<GpuTileMeshVertex> = views
            .vertices
            .iter()
            .map(|v| GpuTileMeshVertex {
                position: [f32::from(v.x), f32::from(v.y)],
            })
            .collect();
        let vertex_buffer = upload_buffer(
            device,
            queue,
            "globe tile mesh vertices",
            wgpu::BufferUsages::VERTEX,
            bytemuck::cast_slice(&gpu_vertices),
        );
        let (index_buffer, index_format) = upload_indices(device, queue, views.indices);
        self.meshes.insert(
            key,
            GpuTileMesh {
                vertex_buffer,
                index_buffer,
                index_format,
                index_count,
            },
        );
        Ok(())
    }

    /// Returns a previously prepared mesh variant.
    pub fn get(
        &self,
        coords: WorldTileCoords,
        usage: TileMeshUsage,
        generate_borders: bool,
    ) -> Option<&GpuTileMesh> {
        self.meshes
            .get(&self.key_for(coords, usage, generate_borders))
    }
}

fn upload_buffer(
    device: &wgpu::Device,
    queue: &crate::render::upload_queue::UploadQueue,
    label: &'static str,
    usage: wgpu::BufferUsages,
    bytes: &[u8],
) -> wgpu::Buffer {
    let size = (bytes.len().max(16).next_multiple_of(16)) as wgpu::BufferAddress;
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size,
        usage: usage | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    if !bytes.is_empty() {
        queue.write_buffer(&buffer, 0, bytes);
    }
    buffer
}

fn upload_indices(
    device: &wgpu::Device,
    queue: &crate::render::upload_queue::UploadQueue,
    indices: TileMeshIndicesRef<'_>,
) -> (wgpu::Buffer, wgpu::IndexFormat) {
    let (bytes, format) = match indices {
        TileMeshIndicesRef::U16(values) => {
            (bytemuck::cast_slice(values), wgpu::IndexFormat::Uint16)
        }
        TileMeshIndicesRef::U32(values) => {
            (bytemuck::cast_slice(values), wgpu::IndexFormat::Uint32)
        }
    };
    let buffer = upload_buffer(
        device,
        queue,
        "globe tile mesh indices",
        wgpu::BufferUsages::INDEX,
        bytes,
    );
    (buffer, format)
}

fn flat_mesh_key(generate_borders: bool) -> TileMeshKey {
    TileMeshKey {
        granularity: 1,
        generate_borders,
        extend_to_north_pole: false,
        extend_to_south_pole: false,
    }
}

fn mesh_key(coords: WorldTileCoords, usage: TileMeshUsage, generate_borders: bool) -> TileMeshKey {
    let zoom = u32::from(u8::from(coords.z));
    let minimum = match usage {
        TileMeshUsage::Raster => 32,
        TileMeshUsage::Stencil => 1,
    };
    let granularity = 128_u32.checked_shr(zoom).unwrap_or(0).max(minimum);
    let last_row = i64::from(ZOOM_BOUNDS[usize::from(u8::from(coords.z))]) - 1;
    TileMeshKey {
        granularity,
        generate_borders,
        extend_to_north_pole: coords.y == 0,
        extend_to_south_pole: i64::from(coords.y) == last_row,
    }
}

#[cfg(test)]
mod tests;
