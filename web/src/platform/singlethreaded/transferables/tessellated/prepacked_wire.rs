//! Pre-packed 16-byte [`GpuShaderVertex`] + `u16`/`u32` index wire encoding/decoding.
//!
//! Moves vertex packing (`36 B -> 16 B`) and `u32 -> u16` index narrowing (`4 B -> 2 B`)
//! off the render thread onto tile workers while preserving exact [`ShaderVertex`] compatibility
//! when pre-packing is disabled (tests, polar rows, DEM elevation, shared sibling geometry).

#![deny(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use bytemuck::Zeroable;
use maplibre::{
    benchmarking::tessellation::{IndexDataType, OverAlignedVertexBuffer, PrepackedGpuGeometry},
    coords::WorldTileCoords,
    render::{
        shaders::{GpuShaderVertex, PoolVertex},
        ShaderVertex,
    },
    vector::AvailableVectorLayerBucket,
};

pub(super) const LAYER_TESSELLATED_RAW_MAGIC: u32 = 0x4D4C_5654;
pub(super) const LAYER_TESSELLATED_PREPACKED_U16_MAGIC: u32 = 0x4D4C_5655;
pub(super) const LAYER_TESSELLATED_PREPACKED_U32_MAGIC: u32 = 0x4D4C_5656;
pub(super) const LAYER_TESSELLATED_COMPACT_EXTRUSION_U16_MAGIC: u32 = 0x4D4C_5657;
pub(super) const LAYER_TESSELLATED_COMPACT_EXTRUSION_U32_MAGIC: u32 = 0x4D4C_5658;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct WireFormat {
    pub prepacked: bool,
    pub u16_indices: bool,
    pub compact_extrusion: bool,
}

pub(super) fn format_for_magic(magic: u32) -> Option<WireFormat> {
    match magic {
        LAYER_TESSELLATED_RAW_MAGIC => Some(WireFormat {
            prepacked: false,
            u16_indices: false,
            compact_extrusion: false,
        }),
        LAYER_TESSELLATED_PREPACKED_U16_MAGIC => Some(WireFormat {
            prepacked: true,
            u16_indices: true,
            compact_extrusion: false,
        }),
        LAYER_TESSELLATED_PREPACKED_U32_MAGIC => Some(WireFormat {
            prepacked: true,
            u16_indices: false,
            compact_extrusion: false,
        }),
        LAYER_TESSELLATED_COMPACT_EXTRUSION_U16_MAGIC => Some(WireFormat {
            prepacked: true,
            u16_indices: true,
            compact_extrusion: true,
        }),
        LAYER_TESSELLATED_COMPACT_EXTRUSION_U32_MAGIC => Some(WireFormat {
            prepacked: true,
            u16_indices: false,
            compact_extrusion: true,
        }),
        _ => None,
    }
}

impl WireFormat {
    #[inline]
    pub(super) fn vertex_stride(self) -> usize {
        if self.compact_extrusion {
            8
        } else if self.prepacked {
            std::mem::size_of::<GpuShaderVertex>()
        } else {
            std::mem::size_of::<ShaderVertex>()
        }
    }

    #[inline]
    pub(super) fn index_stride(self) -> usize {
        if self.u16_indices {
            2
        } else {
            4
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn encode_prepacked_layer_tessellated(
    header_bytes: usize,
    coords: WorldTileCoords,
    mut buffer: OverAlignedVertexBuffer<ShaderVertex, IndexDataType>,
    feature_indices: Vec<u32>,
    feature_colors: Vec<[f32; 4]>,
    feature_sort_keys: Vec<f32>,
    layer_name_str: &str,
    style_layer_id: &str,
    paint_bytes: Option<&[u8]>,
    style_type: &str,
) -> Vec<u8> {
    let shared_key = buffer.shared_geometry_key().unwrap_or(0);
    let usable_indices = buffer.usable_indices;

    let (gpu_vertices, logical_vertex_count, compact_extrusion, existing_u16, raw_indices) =
        if let Some(pre) = buffer.take_prepacked_gpu() {
            let logical = pre.logical_len();
            let compact = pre.compact_extrusion;
            (
                pre.vertices,
                logical,
                compact,
                pre.indices_u16,
                buffer.buffer.indices,
            )
        } else {
            let raw_vertices = buffer.buffer.vertices;
            let raw_indices = buffer.buffer.indices;
            let logical = raw_vertices.len();
            let mut packed = Vec::new();
            let compact = shared_key == 0
                && feature_colors.is_empty()
                && paint_bytes.is_none_or(<[u8]>::is_empty)
                && <ShaderVertex as PoolVertex>::try_encode_compact_extrusion(
                    style_type,
                    &raw_vertices,
                    &mut packed,
                )
                .is_some();
            if !compact {
                let _ = <ShaderVertex as PoolVertex>::encode_vertices(
                    style_type,
                    &raw_vertices,
                    &mut packed,
                );
            }
            drop(raw_vertices);
            (packed, logical, compact, None, raw_indices)
        };

    let paint_slice = paint_bytes.unwrap_or_default();
    let pick_magic = |u16_indices: bool| -> u32 {
        match (compact_extrusion, u16_indices) {
            (true, true) => LAYER_TESSELLATED_COMPACT_EXTRUSION_U16_MAGIC,
            (true, false) => LAYER_TESSELLATED_COMPACT_EXTRUSION_U32_MAGIC,
            (false, true) => LAYER_TESSELLATED_PREPACKED_U16_MAGIC,
            (false, false) => LAYER_TESSELLATED_PREPACKED_U32_MAGIC,
        }
    };
    let (magic, out_index_count, use_u16_usable): (u32, usize, Option<usize>) =
        if let Some(u16s) = &existing_u16 {
            (pick_magic(true), u16s.len(), None)
        } else {
            let usable = (usable_indices as usize).min(raw_indices.len());
            let can_u16 = logical_vertex_count <= 65_536
                && raw_indices[..usable]
                    .iter()
                    .all(|&idx| idx <= u32::from(u16::MAX));
            if can_u16 {
                (pick_magic(true), usable.next_multiple_of(2), Some(usable))
            } else {
                (pick_magic(false), raw_indices.len(), None)
            }
        };

    let vert_bytes_len = logical_vertex_count
        * if compact_extrusion {
            8
        } else {
            std::mem::size_of::<GpuShaderVertex>()
        };
    let total_len = header_bytes
        + layer_name_str.len()
        + style_layer_id.len()
        + vert_bytes_len
        + out_index_count
            * if existing_u16.is_some() || use_u16_usable.is_some() {
                2
            } else {
                4
            }
        + feature_indices.len() * 4
        + feature_colors.len() * 16
        + feature_sort_keys.len() * 4
        + paint_slice.len();

    let mut out = Vec::with_capacity(total_len);
    out.extend_from_slice(&magic.to_le_bytes());
    out.extend_from_slice(&coords.x.to_le_bytes());
    out.extend_from_slice(&coords.y.to_le_bytes());
    out.extend_from_slice(&u32::from(u8::from(coords.z)).to_le_bytes());
    out.extend_from_slice(&usable_indices.to_le_bytes());
    out.extend_from_slice(&(layer_name_str.len() as u32).to_le_bytes());
    out.extend_from_slice(&(style_layer_id.len() as u32).to_le_bytes());
    out.extend_from_slice(&(logical_vertex_count as u32).to_le_bytes());
    out.extend_from_slice(&(out_index_count as u32).to_le_bytes());
    out.extend_from_slice(&(feature_indices.len() as u32).to_le_bytes());
    out.extend_from_slice(&(feature_colors.len() as u32).to_le_bytes());
    out.extend_from_slice(&(feature_sort_keys.len() as u32).to_le_bytes());
    out.extend_from_slice(&(paint_slice.len() as u32).to_le_bytes());
    out.extend_from_slice(&shared_key.to_le_bytes());

    out.extend_from_slice(layer_name_str.as_bytes());
    out.extend_from_slice(style_layer_id.as_bytes());
    let raw_gpu_bytes: &[u8] = bytemuck::cast_slice(&gpu_vertices);
    out.extend_from_slice(&raw_gpu_bytes[..vert_bytes_len.min(raw_gpu_bytes.len())]);
    drop(gpu_vertices);

    if let Some(u16s) = existing_u16 {
        out.extend_from_slice(bytemuck::cast_slice(&u16s));
        drop(u16s);
    } else if let Some(usable) = use_u16_usable {
        for &idx in &raw_indices[..usable] {
            out.extend_from_slice(&(idx as u16).to_le_bytes());
        }
        if usable < out_index_count {
            out.extend_from_slice(&0_u16.to_le_bytes());
        }
        drop(raw_indices);
    } else {
        out.extend_from_slice(bytemuck::cast_slice(&raw_indices));
        drop(raw_indices);
    }

    out.extend_from_slice(bytemuck::cast_slice(&feature_indices));
    drop(feature_indices);
    out.extend_from_slice(bytemuck::cast_slice(&feature_colors));
    drop(feature_colors);
    out.extend_from_slice(bytemuck::cast_slice(&feature_sort_keys));
    drop(feature_sort_keys);
    out.extend_from_slice(paint_slice);
    out
}

pub(super) fn build_decoded_buffer(
    format: WireFormat,
    usable_indices: u32,
    shared_geometry_key: Option<u64>,
    vertex_count: usize,
    index_count: usize,
    mut fill: impl FnMut(&mut [u8], &mut [u8]) -> Option<()>,
) -> Option<OverAlignedVertexBuffer<ShaderVertex, IndexDataType>> {
    if !format.prepacked {
        let mut vertices = vec![ShaderVertex::zeroed(); vertex_count];
        let mut indices = vec![0_u32; index_count];
        fill(
            bytemuck::cast_slice_mut(&mut vertices),
            bytemuck::cast_slice_mut(&mut indices),
        )?;
        return Some(
            OverAlignedVertexBuffer::from_vecs(vertices, indices, usable_indices)
                .with_shared_geometry_key(shared_geometry_key),
        );
    }

    let word_count = if format.compact_extrusion {
        vertex_count.div_ceil(2)
    } else {
        vertex_count
    };
    let vert_bytes = vertex_count * format.vertex_stride();
    let mut gpu_vertices = vec![GpuShaderVertex::zeroed(); word_count];
    let logical_vertex_count = u32::try_from(vertex_count).unwrap_or(u32::MAX);
    if format.u16_indices {
        let mut indices_u16 = vec![0_u16; index_count];
        fill(
            &mut bytemuck::cast_slice_mut(&mut gpu_vertices)[..vert_bytes],
            bytemuck::cast_slice_mut(&mut indices_u16),
        )?;
        Some(
            OverAlignedVertexBuffer::from_vecs(Vec::new(), Vec::new(), usable_indices)
                .with_prepacked_gpu(PrepackedGpuGeometry {
                    vertices: gpu_vertices,
                    indices_u16: Some(indices_u16),
                    compact_extrusion: format.compact_extrusion,
                    logical_vertex_count,
                })
                .with_shared_geometry_key(shared_geometry_key),
        )
    } else {
        let mut indices = vec![0_u32; index_count];
        fill(
            &mut bytemuck::cast_slice_mut(&mut gpu_vertices)[..vert_bytes],
            bytemuck::cast_slice_mut(&mut indices),
        )?;
        Some(
            OverAlignedVertexBuffer::from_vecs(Vec::new(), indices, usable_indices)
                .with_prepacked_gpu(PrepackedGpuGeometry {
                    vertices: gpu_vertices,
                    indices_u16: None,
                    compact_extrusion: format.compact_extrusion,
                    logical_vertex_count,
                })
                .with_shared_geometry_key(shared_geometry_key),
        )
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn assemble_bucket(
    coords: WorldTileCoords,
    source_layer: String,
    style_layer_id: String,
    buffer: OverAlignedVertexBuffer<ShaderVertex, IndexDataType>,
    feature_indices: Vec<u32>,
    feature_colors: Vec<[f32; 4]>,
    feature_sort_keys: Vec<f32>,
) -> AvailableVectorLayerBucket {
    AvailableVectorLayerBucket {
        coords,
        source_layer,
        style_layer_id,
        buffer,
        feature_indices,
        feature_colors,
        feature_sort_keys,
    }
}
