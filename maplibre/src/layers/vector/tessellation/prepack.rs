//! Incremental worker-side GPU vertex encoding that compacts vertices after each polygon or
//! stroke instead of materializing multi-megabyte intermediate `Vec<ShaderVertex>` buffers.

use super::{IndexDataType, OverAlignedVertexBuffer, PrepackedGpuGeometry, ZeroTessellator};
use crate::render::{
    shaders::{
        encode_compact_extrusion_words, is_compact_extrusion_vertex, unpack_compact_extrusion_word,
        GpuShaderVertex, GpuVertexKind,
    },
    ShaderVertex,
};

enum PrepackStorage {
    CompactExtrusion {
        words: Vec<GpuShaderVertex>,
        half: Option<[u32; 2]>,
        logical_len: usize,
    },
    Standard(Vec<GpuShaderVertex>),
}

/// Streams tessellated [`ShaderVertex`] slices directly into compact 8-byte extrusion pairs or
/// standard 16-byte [`GpuShaderVertex`] words.
pub(crate) struct StreamingPrepack {
    kind: GpuVertexKind,
    storage: PrepackStorage,
}

impl StreamingPrepack {
    /// Creates a streaming packer for `style_type`, using compact 8-byte extrusion words when
    /// `allow_compact_extrusion` holds and upgrading automatically if any vertex exceeds limits.
    pub(crate) fn new(style_type: &str, allow_compact_extrusion: bool) -> Option<Self> {
        let kind = GpuVertexKind::from_layer_type(style_type)?;
        let storage = if kind == GpuVertexKind::Extrusion && allow_compact_extrusion {
            PrepackStorage::CompactExtrusion {
                words: Vec::new(),
                half: None,
                logical_len: 0,
            }
        } else {
            PrepackStorage::Standard(Vec::new())
        };
        Some(Self { kind, storage })
    }

    #[inline]
    pub(crate) fn logical_len(&self) -> usize {
        match &self.storage {
            PrepackStorage::CompactExtrusion { logical_len, .. } => *logical_len,
            PrepackStorage::Standard(verts) => verts.len(),
        }
    }

    /// Encodes `vertices` into GPU form, upgrading `CompactExtrusion -> Standard` if needed.
    pub(crate) fn push_vertices(&mut self, vertices: &[ShaderVertex]) {
        if vertices.is_empty() {
            return;
        }
        match &mut self.storage {
            PrepackStorage::CompactExtrusion {
                words,
                half,
                logical_len,
            } => {
                for (idx, vertex) in vertices.iter().enumerate() {
                    if is_compact_extrusion_vertex(vertex) {
                        let pair = encode_compact_extrusion_words(vertex);
                        if let Some(first) = half.take() {
                            words.push(GpuShaderVertex([first[0], first[1], pair[0], pair[1]]));
                        } else {
                            *half = Some(pair);
                        }
                        *logical_len += 1;
                    } else {
                        let mut standard =
                            Vec::with_capacity(*logical_len + (vertices.len() - idx));
                        for word in words.iter() {
                            standard.push(unpack_compact_extrusion_word([word.0[0], word.0[1]]));
                            standard.push(unpack_compact_extrusion_word([word.0[2], word.0[3]]));
                        }
                        if let Some(pending) = half.take() {
                            standard.push(unpack_compact_extrusion_word(pending));
                        }
                        for rem in vertices.get(idx..).unwrap_or(&[]) {
                            standard.push(self.kind.encode(rem));
                        }
                        self.storage = PrepackStorage::Standard(standard);
                        return;
                    }
                }
            }
            PrepackStorage::Standard(out) => {
                out.reserve(vertices.len());
                for vertex in vertices {
                    out.push(self.kind.encode(vertex));
                }
            }
        }
    }

    /// Appends already-encoded 16-byte [`GpuShaderVertex`] words directly into streaming storage.
    pub(crate) fn push_encoded(&mut self, vertices: &[GpuShaderVertex]) {
        if vertices.is_empty() {
            return;
        }
        match &mut self.storage {
            PrepackStorage::Standard(out) => out.extend_from_slice(vertices),
            PrepackStorage::CompactExtrusion {
                words,
                half,
                logical_len,
            } => {
                let mut standard = Vec::with_capacity(*logical_len + vertices.len());
                for word in words.iter() {
                    standard.push(unpack_compact_extrusion_word([word.0[0], word.0[1]]));
                    standard.push(unpack_compact_extrusion_word([word.0[2], word.0[3]]));
                }
                if let Some(pending) = half.take() {
                    standard.push(unpack_compact_extrusion_word(pending));
                }
                standard.extend_from_slice(vertices);
                self.storage = PrepackStorage::Standard(standard);
            }
        }
    }

    /// Finalizes packed GPU vertices and narrows indices to aligned `u16` when every index fits.
    pub(crate) fn finish(
        self,
        mut raw_indices: Vec<IndexDataType>,
    ) -> OverAlignedVertexBuffer<ShaderVertex, IndexDataType> {
        let logical_len = self.logical_len();
        if logical_len == 0 {
            return OverAlignedVertexBuffer::empty();
        }
        let (mut vertices, compact_extrusion) = match self.storage {
            PrepackStorage::CompactExtrusion {
                mut words, half, ..
            } => {
                if let Some(first) = half {
                    words.push(GpuShaderVertex([first[0], first[1], 0, 0]));
                }
                (words, true)
            }
            PrepackStorage::Standard(verts) => (verts, false),
        };
        vertices.shrink_to_fit();

        let usable = raw_indices.len() as u32;
        let logical_vertex_count = u32::try_from(logical_len).unwrap_or(u32::MAX);
        let can_u16 =
            logical_len <= 65_536 && raw_indices.iter().all(|&idx| idx <= u32::from(u16::MAX));
        if can_u16 {
            let padded = raw_indices.len().next_multiple_of(2);
            let mut u16s = Vec::with_capacity(padded);
            u16s.extend(raw_indices.iter().map(|&idx| idx as u16));
            if u16s.len() < padded {
                u16s.push(0);
            }
            OverAlignedVertexBuffer::from_vecs(Vec::new(), Vec::new(), usable).with_prepacked_gpu(
                PrepackedGpuGeometry {
                    vertices,
                    indices_u16: Some(u16s),
                    indices_u32: None,
                    compact_extrusion,
                    logical_vertex_count,
                },
            )
        } else {
            raw_indices.shrink_to_fit();
            OverAlignedVertexBuffer::from_vecs(Vec::new(), Vec::new(), usable).with_prepacked_gpu(
                PrepackedGpuGeometry {
                    vertices,
                    indices_u16: None,
                    indices_u32: Some(raw_indices),
                    compact_extrusion,
                    logical_vertex_count,
                },
            )
        }
    }
}

impl ZeroTessellator<IndexDataType> {
    /// Finalizes tessellated geometry into an upload-ready [`OverAlignedVertexBuffer`].
    pub(crate) fn into_over_aligned_buffer(
        mut self,
    ) -> OverAlignedVertexBuffer<ShaderVertex, IndexDataType> {
        if let Some(mut prepack) = self.streaming_prepack.take() {
            if !self.buffer.vertices.is_empty() {
                prepack.push_vertices(&self.buffer.vertices);
                self.buffer.vertices = Vec::new();
            }
            return prepack.finish(self.buffer.indices);
        }
        self.buffer.vertices.shrink_to_fit();
        self.buffer.indices.shrink_to_fit();
        self.buffer.into()
    }
}

#[cfg(test)]
mod tests {
    use geozero::GeomProcessor;

    use super::*;
    use crate::{
        render::shaders::PoolVertex,
        vector::tessellation::{ExtrusionOptions, ZeroTessellator},
    };

    #[test]
    fn streaming_fill_and_line_match_batch_prepack_bit_for_bit() {
        for (layer_type, is_line) in [("fill", false), ("line", true)] {
            let mut batch = ZeroTessellator::<IndexDataType> {
                is_line_layer: is_line,
                ..Default::default()
            };

            let mut streaming = ZeroTessellator::<IndexDataType> {
                is_line_layer: is_line,
                ..Default::default()
            };
            streaming.enable_streaming_prepack(layer_type, false);

            for t in [&mut batch, &mut streaming] {
                for offset in [100.0_f64, 500.0_f64] {
                    if is_line {
                        t.linestring_begin(true, 3, 0).expect("line begin");
                        t.xy(offset, 100.0, 0).expect("xy0");
                        t.xy(offset + 80.0, 140.0, 1).expect("xy1");
                        t.xy(offset + 120.0, 220.0, 2).expect("xy2");
                        t.linestring_end(true, 0).expect("line end");
                    } else {
                        t.polygon_begin(true, 4, 0).expect("poly begin");
                        t.xy(offset, 100.0, 0).expect("xy0");
                        t.xy(offset + 80.0, 100.0, 1).expect("xy1");
                        t.xy(offset + 80.0, 180.0, 2).expect("xy2");
                        t.xy(offset, 180.0, 3).expect("xy3");
                        t.polygon_end(true, 0).expect("poly end");
                    }
                    t.finish_feature();
                }
            }

            assert_eq!(batch.feature_indices, streaming.feature_indices);
            let mut expected = Vec::new();
            ShaderVertex::encode_vertices(layer_type, &batch.buffer.vertices, &mut expected);
            let packed = streaming.into_over_aligned_buffer();
            let pre = packed.prepacked_gpu().expect("streaming prepacked");
            assert!(!pre.compact_extrusion);
            assert_eq!(pre.vertices, expected);
            assert_eq!(
                pre.indices_u16.as_ref().expect("u16")[..batch.buffer.indices.len()]
                    .iter()
                    .copied()
                    .map(u32::from)
                    .collect::<Vec<_>>(),
                batch.buffer.indices
            );
        }
    }

    #[test]
    fn streaming_compact_extrusion_upgrades_cleanly_for_tall_skyscrapers() {
        let mut t = ZeroTessellator::<IndexDataType> {
            extrusion: Some(ExtrusionOptions {
                height: None,
                base: None,
                base_datum: None,
                height_datum: None,
                part: None,
                rounded_corner_distance: None,
                translucent: false,
                clip_buffer: None,
                has_pattern: false,
                overscale: 0,
            }),
            ..Default::default()
        };
        t.enable_streaming_prepack("fill-extrusion", true);

        t.polygon_begin(true, 4, 0).expect("poly");
        t.xy(100.0, 100.0, 0).expect("p0");
        t.xy(150.0, 100.0, 1).expect("p1");
        t.xy(150.0, 150.0, 2).expect("p2");
        t.xy(100.0, 150.0, 3).expect("p3");
        t.polygon_end(true, 0).expect("end");
        t.finish_feature();

        let compact_buf = t.into_over_aligned_buffer();
        let compact_pre = compact_buf.prepacked_gpu().expect("compact prepack");
        assert!(compact_pre.compact_extrusion);
        assert!(compact_pre.logical_vertex_count > 0);
        assert_eq!(
            compact_pre.vertices.len(),
            (compact_pre.logical_vertex_count as usize).div_ceil(2)
        );

        let mut packer = StreamingPrepack::new("fill-extrusion", true).expect("packer");
        let small = ShaderVertex::new([20.0, 30.0], [1.0, 0.0]);
        let mut tall = ShaderVertex::new([40.0, 50.0], [0.0, 1.0]);
        tall.elevation = 1500.0; // Exceeds compact 1023.5 m ceiling.
        packer.push_vertices(&[small, tall]);
        let upgraded = packer.finish(vec![0, 1, 0]);
        let up_pre = upgraded.prepacked_gpu().expect("upgraded prepack");
        assert!(!up_pre.compact_extrusion);
        assert_eq!(up_pre.logical_vertex_count, 2);
        assert_eq!(up_pre.vertices.len(), 2);
        assert_eq!(up_pre.vertices[1], GpuVertexKind::Extrusion.encode(&tall));
    }

    #[test]
    fn streaming_fill_with_outline_matches_batch_prepack_bit_for_bit() {
        use geozero::FeatureProcessor;

        use crate::style::layer::StyleProperty;

        for omit_feature_colors in [false, true] {
            let mut batch = ZeroTessellator::<IndexDataType> {
                style_property: Some(StyleProperty::Constant(csscolorparser::Color::from_rgba8(
                    240, 232, 224, 255,
                ))),
                outline_property: Some(StyleProperty::Constant(csscolorparser::Color::from_rgba8(
                    220, 212, 204, 255,
                ))),
                omit_feature_colors,
                ..Default::default()
            };

            let mut streaming = ZeroTessellator::<IndexDataType> {
                style_property: Some(StyleProperty::Constant(csscolorparser::Color::from_rgba8(
                    240, 232, 224, 255,
                ))),
                outline_property: Some(StyleProperty::Constant(csscolorparser::Color::from_rgba8(
                    220, 212, 204, 255,
                ))),
                omit_feature_colors,
                record_outline_entries: true,
                ..Default::default()
            };
            streaming.maybe_enable_streaming_prepack("fill");

            for t in [&mut batch, &mut streaming] {
                for (feat_idx, x0) in [100.0_f64, 400.0_f64].into_iter().enumerate() {
                    t.feature_begin(feat_idx as u64).expect("feat begin");
                    t.geometry_begin().expect("geom begin");
                    t.polygon_begin(true, 4, 0).expect("poly begin");
                    t.xy(x0, 100.0, 0).expect("p0");
                    t.xy(x0 + 80.0, 100.0, 1).expect("p1");
                    t.xy(x0 + 80.0, 180.0, 2).expect("p2");
                    t.xy(x0, 180.0, 3).expect("p3");
                    t.polygon_end(true, 0).expect("poly end");
                    t.geometry_end().expect("geom end");
                    t.feature_end(feat_idx as u64).expect("feat end");
                }
            }

            assert_eq!(batch.feature_indices, streaming.feature_indices);
            assert_eq!(batch.feature_colors, streaming.feature_colors);
            assert_eq!(streaming.outline_entries, vec![false, true, false, true]);
            let mut expected = Vec::new();
            ShaderVertex::encode_vertices("fill", &batch.buffer.vertices, &mut expected);
            let packed = streaming.into_over_aligned_buffer();
            let pre = packed
                .prepacked_gpu()
                .expect("streaming fill outline prepack");
            assert!(!pre.compact_extrusion);
            assert_eq!(pre.vertices, expected);
            assert_eq!(
                pre.indices_u16.as_ref().expect("u16")[..batch.buffer.indices.len()]
                    .iter()
                    .copied()
                    .map(u32::from)
                    .collect::<Vec<_>>(),
                batch.buffer.indices
            );
        }
    }

    #[test]
    fn streaming_multipolygon_outline_compacts_without_buffering_vertices() {
        use geozero::FeatureProcessor;

        use crate::style::layer::StyleProperty;

        let mut batch = ZeroTessellator::<IndexDataType> {
            style_property: Some(StyleProperty::Constant(csscolorparser::Color::from_rgba8(
                240, 232, 224, 255,
            ))),
            outline_property: Some(StyleProperty::Constant(csscolorparser::Color::from_rgba8(
                220, 212, 204, 255,
            ))),
            omit_feature_colors: true,
            ..Default::default()
        };

        let mut streaming = ZeroTessellator::<IndexDataType> {
            style_property: Some(StyleProperty::Constant(csscolorparser::Color::from_rgba8(
                240, 232, 224, 255,
            ))),
            outline_property: Some(StyleProperty::Constant(csscolorparser::Color::from_rgba8(
                220, 212, 204, 255,
            ))),
            omit_feature_colors: true,
            record_outline_entries: true,
            ..Default::default()
        };
        streaming.maybe_enable_streaming_prepack("fill");

        for t in [&mut batch, &mut streaming] {
            // Feature 0: MultiPolygon with 80 polygons (> 1000 vertices total so any
            // accumulation in `buffer.vertices` or `outline.vertices` must exceed 512).
            t.feature_begin(0).expect("feat 0 begin");
            t.geometry_begin().expect("geom 0 begin");
            t.multipolygon_begin(80, 0).expect("multipoly begin");
            for poly_idx in 0..80 {
                let x0 = f64::from(poly_idx) * 80.0;
                t.polygon_begin(false, 4, poly_idx as usize)
                    .expect("poly begin");
                t.xy(x0, 50.0, 0).expect("p0");
                t.xy(x0 + 60.0, 50.0, 1).expect("p1");
                t.xy(x0 + 60.0, 110.0, 2).expect("p2");
                t.xy(x0 + 20.0, 90.0, 3).expect("p3");
                t.polygon_end(false, poly_idx as usize).expect("poly end");
            }
            t.multipolygon_end(0).expect("multipoly end");
            t.geometry_end().expect("geom 0 end");
            t.feature_end(0).expect("feat 0 end");

            // Feature 1: Second polygon feature after MultiPolygon.
            t.feature_begin(1).expect("feat 1 begin");
            t.geometry_begin().expect("geom 1 begin");
            t.polygon_begin(true, 4, 0).expect("poly 1 begin");
            t.xy(500.0, 200.0, 0).expect("p0");
            t.xy(560.0, 200.0, 1).expect("p1");
            t.xy(560.0, 260.0, 2).expect("p2");
            t.xy(500.0, 260.0, 3).expect("p3");
            t.polygon_end(true, 0).expect("poly 1 end");
            t.geometry_end().expect("geom 1 end");
            t.feature_end(1).expect("feat 1 end");
        }

        assert!(
            batch.buffer.vertices.len() > 640,
            "test MultiPolygon must produce > 640 vertices (got {})",
            batch.buffer.vertices.len()
        );
        assert!(
            streaming.buffer.vertices.capacity() <= 512,
            "streaming buffer.vertices must never accumulate MultiPolygon outlines (cap={}, total={})",
            streaming.buffer.vertices.capacity(),
            batch.buffer.vertices.len()
        );
        assert!(
            streaming.outline.vertices.capacity() <= 512,
            "streaming outline.vertices must stay bounded per polygon (cap={}, total={})",
            streaming.outline.vertices.capacity(),
            batch.buffer.vertices.len()
        );
        assert_eq!(batch.feature_indices, streaming.feature_indices);
        assert_eq!(streaming.outline_entries, vec![false, true, false, true]);
        let mut expected = Vec::new();
        ShaderVertex::encode_vertices("fill", &batch.buffer.vertices, &mut expected);
        let packed = streaming.into_over_aligned_buffer();
        let pre = packed.prepacked_gpu().expect("multipolygon prepack");
        assert!(!pre.compact_extrusion);
        assert_eq!(pre.vertices, expected);
        assert_eq!(
            pre.indices_u16.as_ref().expect("u16")[..batch.buffer.indices.len()]
                .iter()
                .copied()
                .map(u32::from)
                .collect::<Vec<_>>(),
            batch.buffer.indices
        );
    }
}
