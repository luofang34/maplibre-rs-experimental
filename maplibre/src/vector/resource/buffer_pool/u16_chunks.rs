//! Greedy `u16` sub-chunking for large (`> 65,536` vertices) index buffers when
//! [`wgpu::DownlevelFlags::BASE_VERTEX`] is supported.

use std::ops::Range;

use crate::style::layer::StyleLayer;

/// Maximum number of contiguous `base_vertex` chunks per layer before falling back to `u32`
/// indices, preventing draw-call explosion on wildly non-local triangle streams.
const MAX_U16_CHUNKS: usize = 64;

/// Sub-ranges `(vertex_offset, local_index_range)` when > 65,536 vertices pack into `u16` indices.
pub(super) type U16Chunks = Box<[(u32, Range<u32>)]>;

/// Plan produced by [`plan_u16_chunks`]: `(is_u16, aligned_bytes, chunks)`.
pub(super) type IndexPlan = (bool, u64, Option<U16Chunks>);

/// Returns `true` when `style_layer` can safely emit multiple `draw_indexed` sub-ranges without
/// colliding with `sort_runs` (`circle`), 3-pass `fill-extrusion` (`DEPTH`/`COLOR`/`CLEAR`),
/// or per-feature `pattern_runs`.
#[inline]
pub(super) fn layer_allows_u16_chunks(style_layer: &StyleLayer) -> bool {
    style_layer.type_ != "circle"
        && style_layer.type_ != "fill-extrusion"
        && style_layer
            .paint
            .as_ref()
            .and_then(crate::vector::pattern::per_feature_pattern)
            .is_none()
}

/// Attempts to partition `u32_slice[..usable]` triangles into contiguous `(base_vertex_offset,
/// index_range)` chunks whose local index span fits in `u16`.
pub(super) fn plan_u16_chunks(u32_slice: &[u32], usable: usize) -> Option<U16Chunks> {
    let slice = (usable > 0).then(|| u32_slice.get(..usable)).flatten()?;
    let (tris, rem) = slice.as_chunks::<3>();
    if !rem.is_empty() || tris.is_empty() {
        return None;
    }

    let mut chunks: Vec<(u32, Range<u32>)> = Vec::with_capacity(4);
    let mut cur_min = u32::MAX;
    let mut cur_max = 0_u32;
    let mut cur_start = 0_u32;

    for (tri_idx, &[a, b, c]) in tris.iter().enumerate() {
        let t_min = a.min(b).min(c);
        let t_max = a.max(b).max(c);
        if t_max - t_min > u32::from(u16::MAX) {
            return None;
        }
        let next_min = cur_min.min(t_min);
        let next_max = cur_max.max(t_max);
        if next_max - next_min > u32::from(u16::MAX) {
            if i32::try_from(cur_min).is_err() || chunks.len() + 1 >= MAX_U16_CHUNKS {
                return None;
            }
            let end = (tri_idx as u32) * 3;
            chunks.push((cur_min, cur_start..end));
            cur_min = t_min;
            cur_max = t_max;
            cur_start = end;
        } else {
            cur_min = next_min;
            cur_max = next_max;
        }
    }
    if i32::try_from(cur_min).is_err() || chunks.len() >= MAX_U16_CHUNKS {
        return None;
    }
    chunks.push((cur_min, cur_start..usable as u32));
    Some(chunks.into_boxed_slice())
}

/// Writes `chunks` into `scratch`, subtracting each chunk's `v_off` so every index fits in `u16`.
pub(super) fn write_chunked_u16_indices(
    u32_slice: &[u32],
    usable: usize,
    chunks: &[(u32, Range<u32>)],
    scratch: &mut Vec<u16>,
) {
    let padded = usable.next_multiple_of(2);
    scratch.clear();
    scratch.reserve(padded);
    for (v_off, range) in chunks {
        let start = range.start as usize;
        let end = range.end as usize;
        let off = *v_off;
        if let Some(slice) = u32_slice.get(start..end) {
            scratch.extend(slice.iter().map(|&idx| (idx - off) as u16));
        }
    }
    if scratch.len() < padded {
        scratch.push(0);
    }
}

#[cfg(test)]
#[cfg_attr(test, allow(clippy::expect_used))]
mod tests {
    use super::*;

    fn layer(ty: &str) -> StyleLayer {
        serde_json::from_value(serde_json::json!({
            "id": ty,
            "type": ty,
            "source": "s",
            "source-layer": "l",
        }))
        .expect("valid style layer")
    }

    #[test]
    fn layer_allows_u16_chunks_excludes_circle_and_fill_extrusion() {
        assert!(!layer_allows_u16_chunks(&layer("fill-extrusion")));
        assert!(!layer_allows_u16_chunks(&layer("circle")));
        assert!(layer_allows_u16_chunks(&layer("fill")));
        assert!(layer_allows_u16_chunks(&layer("line")));
    }
}
