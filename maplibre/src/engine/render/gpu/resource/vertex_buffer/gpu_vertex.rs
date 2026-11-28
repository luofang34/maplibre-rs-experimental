//! Compact 16-byte GPU vertex encoding (`GpuShaderVertex`) for vector tile pipelines.
//!
//! CPU tessellation and spatial-elevation systems operate on 32-byte `f32` [`ShaderVertex`]
//! records, while [`BufferPool`](crate::vector::BufferPool) packs them into 16-byte
//! [`GpuShaderVertex`] words on upload (`-50%` vertex VRAM and PCIe/WebGPU `writeBuffer` bytes).

use bytemuck::Pod as PodTrait;
use bytemuck_derive::{Pod, Zeroable};

use super::{formats::ShaderVertex, symbol_vertex::ShaderSymbolVertex};

mod pack;

/// Compact 16-byte GPU vertex record stored in [`VectorBufferPool`](crate::vector::VectorBufferPool).
/// Vertex encoding family matching WGSL vertex buffer decoding for vector geometry pipelines.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum GpuVertexKind {
    /// Fill/polygon pipeline vertex encoding.
    Fill,
    /// Line/stroke pipeline vertex encoding.
    Line,
    /// Fill-extrusion pipeline vertex encoding.
    Extrusion,
}

impl GpuVertexKind {
    pub(crate) fn from_layer_type(layer_type: &str) -> Option<Self> {
        match layer_type {
            "line" => Some(Self::Line),
            "fill-extrusion" => Some(Self::Extrusion),
            "circle" | "heatmap" => None,
            _ => Some(Self::Fill),
        }
    }

    #[inline]
    pub(crate) fn encode(self, v: &ShaderVertex) -> GpuShaderVertex {
        match self {
            Self::Fill => v.encode_with_family(0),
            Self::Line => v.encode_with_family(1),
            Self::Extrusion => v.encode_with_family(2),
        }
    }
}

/// Packed GPU-ready vertex layout (`16` bytes, 4×`u32`) matching [`ShaderVertex`].
#[repr(C)]
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Pod, Zeroable)]
pub struct GpuShaderVertex(pub [u32; 4]);

/// Zero-byte layer metadata marker for pools (such as [`SymbolBufferPool`](crate::sdf::SymbolBufferPool))
/// that carry per-layer uniforms via bind groups rather than a Slot 2 vertex buffer.
#[repr(C)]
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Pod, Zeroable)]
pub struct EmptyLayerMetadata(pub [u8; 0]);

/// Maps a CPU vertex type `Self` to its GPU-resident storage representation `Self::Gpu`.
pub trait PoolVertex: PodTrait {
    /// GPU vertex record uploaded to the vertex slab.
    type Gpu: PodTrait + std::fmt::Debug;

    /// Returns a compact tag identifying which GPU bit-layout `layer_type` produces, so shared
    /// source-layer vertex slabs only alias across compatible shader families.
    #[inline]
    fn encoding_family(layer_type: &str) -> u8 {
        let _ = layer_type;
        0
    }

    /// Required alignment of `first_vertex` in elements (`4` for quad-indexed shaders that decode
    /// corners via `@builtin(vertex_index) % 4u`, `1` otherwise).
    #[inline]
    fn vertex_element_alignment(layer_type: &str) -> u64 {
        let _ = layer_type;
        1
    }

    /// Returns `true` when `src` fits losslessly in the compact 8-byte extrusion format.
    #[inline]
    fn can_encode_compact_extrusion(layer_type: &str, src: &[Self]) -> bool {
        let _ = (layer_type, src);
        false
    }

    /// Packs `src` into two 8-byte compact extrusion vertices per `Self::Gpu` word when eligible.
    #[inline]
    fn try_encode_compact_extrusion<'a>(
        layer_type: &str,
        src: &'a [Self],
        scratch: &'a mut Vec<Self::Gpu>,
    ) -> Option<&'a [Self::Gpu]> {
        let _ = (layer_type, src, scratch);
        None
    }

    /// Returns `src` in `Self::Gpu` form, either by zero-copy slice borrow or by packing into `scratch`.
    fn encode_vertices<'a>(
        layer_type: &str,
        src: &'a [Self],
        scratch: &'a mut Vec<Self::Gpu>,
    ) -> &'a [Self::Gpu];
}

impl PoolVertex for ShaderSymbolVertex {
    type Gpu = Self;

    #[inline]
    fn encode_vertices<'a>(
        _layer_type: &str,
        src: &'a [Self],
        _scratch: &'a mut Vec<Self::Gpu>,
    ) -> &'a [Self::Gpu] {
        src
    }
}

impl PoolVertex for ShaderVertex {
    type Gpu = GpuShaderVertex;

    #[inline]
    fn encoding_family(layer_type: &str) -> u8 {
        match layer_type {
            "line" => 1,
            "fill-extrusion" => 2,
            "circle" | "heatmap" => 3,
            _ => 0,
        }
    }

    #[inline]
    fn vertex_element_alignment(layer_type: &str) -> u64 {
        match layer_type {
            "circle" | "heatmap" => 4,
            _ => 1,
        }
    }

    fn encode_vertices<'a>(
        layer_type: &str,
        src: &'a [Self],
        scratch: &'a mut Vec<GpuShaderVertex>,
    ) -> &'a [GpuShaderVertex] {
        Self::encode_for_family(Self::encoding_family(layer_type), src, scratch);
        scratch.as_slice()
    }

    #[inline]
    fn can_encode_compact_extrusion(layer_type: &str, src: &[Self]) -> bool {
        layer_type == "fill-extrusion" && Self::can_encode_compact_slice(src)
    }

    #[inline]
    fn try_encode_compact_extrusion<'a>(
        layer_type: &str,
        src: &'a [Self],
        scratch: &'a mut Vec<GpuShaderVertex>,
    ) -> Option<&'a [GpuShaderVertex]> {
        if layer_type != "fill-extrusion" || !Self::can_encode_compact_slice(src) {
            return None;
        }
        Self::encode_compact_into(src, scratch);
        Some(scratch.as_slice())
    }
}

impl ShaderVertex {
    fn encode_for_family(family: u8, src: &[Self], scratch: &mut Vec<GpuShaderVertex>) {
        scratch.clear();
        scratch.reserve(src.len());
        scratch.extend(src.iter().map(|v| v.encode_with_family(family)));
    }

    #[inline]
    fn encode_with_family(&self, family: u8) -> GpuShaderVertex {
        GpuShaderVertex(pack::encode_shader_vertex_words(
            family,
            self.position,
            self.normal,
            self.distance,
            self.edge_distance,
            self.elevation,
            self.corner_code,
        ))
    }

    #[inline]
    fn is_compact_vertex(&self) -> bool {
        pack::is_compact_extrusion_scalars(
            self.corner_code,
            self.edge_distance,
            self.distance,
            self.elevation,
        )
    }

    #[inline]
    fn can_encode_compact_slice(src: &[Self]) -> bool {
        !src.is_empty() && src.iter().all(Self::is_compact_vertex)
    }

    #[inline]
    fn encode_compact_words(&self) -> [u32; 2] {
        pack::encode_compact_extrusion_word_pair(
            self.position,
            self.normal,
            self.distance,
            self.elevation,
        )
    }

    fn encode_compact_into(src: &[Self], out: &mut Vec<GpuShaderVertex>) {
        out.clear();
        out.reserve(src.len().div_ceil(2));
        for chunk in src.chunks(2) {
            let [first, rest @ ..] = chunk else { continue };
            let a = first.encode_compact_words();
            let b = rest.first().map_or([0, 0], Self::encode_compact_words);
            out.push(GpuShaderVertex([a[0], a[1], b[0], b[1]]));
        }
    }
}

/// Returns `true` when `v` fits in the compact 8-byte `fill-extrusion` vertex format.
#[inline]
pub(crate) fn is_compact_extrusion_vertex(v: &ShaderVertex) -> bool {
    v.is_compact_vertex()
}

/// Returns `true` when every vertex in `src` fits in the compact 8-byte `fill-extrusion`
/// vertex format (base in `0..=511.5` m, height in `0..=1023.5` m, zero `edge_distance`,
/// default ground-datum `0`, and zero zoom-interpolated `rise`).
#[inline]
pub fn can_encode_compact_extrusion(src: &[ShaderVertex]) -> bool {
    ShaderVertex::can_encode_compact_slice(src)
}

#[inline]
pub(crate) fn encode_compact_extrusion_words(v: &ShaderVertex) -> [u32; 2] {
    v.encode_compact_words()
}

/// Expands a single 8-byte compact extrusion record into a standard 16-byte [`GpuShaderVertex`].
#[inline]
pub(crate) fn unpack_compact_extrusion_word(compact: [u32; 2]) -> GpuShaderVertex {
    GpuShaderVertex(pack::unpack_compact_extrusion_words(compact))
}

/// Expands `logical_count` compact 8-byte extrusion vertices back into standard 16-byte
/// [`GpuShaderVertex`] records for fallback non-compact pipelines.
pub fn unpack_compact_extrusion_vertices(
    packed: &[GpuShaderVertex],
    logical_count: usize,
    out: &mut Vec<GpuShaderVertex>,
) {
    out.clear();
    out.reserve(logical_count);
    let mut emitted = 0_usize;
    for word in packed {
        for half in [[word.0[0], word.0[1]], [word.0[2], word.0[3]]] {
            if emitted >= logical_count {
                return;
            }
            out.push(unpack_compact_extrusion_word(half));
            emitted += 1;
        }
    }
}
