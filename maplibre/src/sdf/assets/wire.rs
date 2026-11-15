//! Worker transport for symbol collision geometry and atlases.
use std::{collections::HashMap, sync::Arc};

use rustc_hash::FxHashMap;

use crate::{
    euclid::{Box2D, Point2D},
    io::geometry_index::wire::{
        decode_properties_from, encode_properties, encode_str, read_f32, read_f64, read_string,
        read_u32, read_u64, read_u8,
    },
    sdf::{placement_geometry::SymbolBounds, Feature},
    style::{
        expression::{empty_feature_properties, FeatureProperties},
        filter::GeometryType,
    },
};

mod json_legacy;
pub use json_legacy::{LineLabelWire, SymbolFeature};

const SYM_WIRE_MAGIC_V1: [u8; 4] = *b"SYM1";
const SYM_WIRE_MAGIC_V2: [u8; 4] = *b"SYM2";

/// Encodes symbol placement features in compact binary format, deduplicating shared polylines,
/// repeat keys, and `Arc<FeatureProperties>` maps without intermediate JSON allocations.
pub fn encode_symbol_features(features: &[Feature]) -> Vec<u8> {
    let mut poly_ids: FxHashMap<usize, u32> = FxHashMap::default();
    let mut polys: Vec<&[[f32; 2]]> = Vec::new();
    let mut repeat_ids: HashMap<&str, u32> = HashMap::new();
    let mut repeat_keys: Vec<&str> = Vec::new();
    let mut prop_ids: FxHashMap<usize, u32> = FxHashMap::default();
    let mut prop_groups: Vec<&FeatureProperties> = Vec::new();

    for feature in features {
        if let Some(line) = &feature.line {
            let key = Arc::as_ptr(&line.polyline) as *const [f32; 2] as usize;
            if let std::collections::hash_map::Entry::Vacant(slot) = poly_ids.entry(key) {
                let id = polys.len() as u32;
                polys.push(&line.polyline);
                slot.insert(id);
            }
        }
        if let Some(rk) = feature.repeat_key.as_deref() {
            if let std::collections::hash_map::Entry::Vacant(slot) = repeat_ids.entry(rk) {
                let id = repeat_keys.len() as u32;
                repeat_keys.push(rk);
                slot.insert(id);
            }
        }
        if !feature.data.properties.is_empty() {
            let key = Arc::as_ptr(&feature.data.properties) as usize;
            if let std::collections::hash_map::Entry::Vacant(slot) = prop_ids.entry(key) {
                let id = prop_groups.len() as u32;
                prop_groups.push(&feature.data.properties);
                slot.insert(id);
            }
        }
    }

    let mut out = Vec::with_capacity(32 + features.len() * 80);
    out.extend_from_slice(&SYM_WIRE_MAGIC_V2);

    out.extend_from_slice(&(polys.len() as u32).to_le_bytes());
    for poly in &polys {
        out.extend_from_slice(&(poly.len() as u32).to_le_bytes());
        for &[x, y] in *poly {
            out.extend_from_slice(&x.to_le_bytes());
            out.extend_from_slice(&y.to_le_bytes());
        }
    }

    out.extend_from_slice(&(repeat_keys.len() as u32).to_le_bytes());
    for rk in &repeat_keys {
        encode_str(rk, &mut out);
    }

    out.extend_from_slice(&(prop_groups.len() as u32).to_le_bytes());
    for props in &prop_groups {
        encode_properties(props, &mut out);
    }

    out.extend_from_slice(&(features.len() as u32).to_le_bytes());
    for feature in features {
        for c in [
            feature.bbox.min.x,
            feature.bbox.min.y,
            feature.bbox.max.x,
            feature.bbox.max.y,
        ] {
            out.extend_from_slice(&c.to_le_bytes());
        }
        out.extend_from_slice(&(feature.indices.start as u32).to_le_bytes());
        out.extend_from_slice(&(feature.indices.end as u32).to_le_bytes());
        out.extend_from_slice(&feature.text_anchor.x.to_le_bytes());
        out.extend_from_slice(&feature.text_anchor.y.to_le_bytes());
        encode_str(&feature.str, &mut out);

        let flags = u8::from(feature.parts[0].is_some())
            | (u8::from(feature.parts[1].is_some()) << 1)
            | (u8::from(feature.parts[2].is_some()) << 2)
            | (u8::from(feature.data.id.is_some()) << 3)
            | (u8::from(feature.fallback) << 4)
            | (u8::from(feature.repeat_key.is_some()) << 5)
            | (u8::from(feature.line.is_some()) << 6)
            | (u8::from(!feature.data.properties.is_empty()) << 7);
        out.push(flags);

        let geom_tag = match feature.data.geometry_type {
            GeometryType::Unknown => 0,
            GeometryType::Point => 1,
            GeometryType::LineString => 2,
            GeometryType::Polygon => 3,
        };
        out.push(geom_tag);
        out.extend_from_slice(&feature.data.sort_key.to_le_bytes());

        for part in feature.parts.iter().flatten() {
            for b in part.bounds {
                out.extend_from_slice(&b.to_le_bytes());
            }
            out.extend_from_slice(&part.height.to_le_bytes());
            out.extend_from_slice(&part.angle.to_le_bytes());
            out.push(u8::from(part.text));
        }
        if let Some(id) = feature.data.id {
            out.extend_from_slice(&id.to_le_bytes());
        }
        if !feature.data.properties.is_empty() {
            let key = Arc::as_ptr(&feature.data.properties) as usize;
            let prop_idx = prop_ids.get(&key).copied().unwrap_or(0);
            out.extend_from_slice(&prop_idx.to_le_bytes());
        }

        out.extend_from_slice(&(feature.anchor_shifts.len() as u32).to_le_bytes());
        for &[sx, sy] in &feature.anchor_shifts {
            out.extend_from_slice(&sx.to_le_bytes());
            out.extend_from_slice(&sy.to_le_bytes());
        }
        out.extend_from_slice(&(feature.text_sets.len() as u32).to_le_bytes());
        for set in &feature.text_sets {
            out.extend_from_slice(&(set.start as u32).to_le_bytes());
            out.extend_from_slice(&(set.end as u32).to_le_bytes());
        }
        out.extend_from_slice(&(feature.anchor_sets.len() as u32).to_le_bytes());
        out.extend_from_slice(&feature.anchor_sets);

        out.extend_from_slice(&(feature.text_colors.len() as u32).to_le_bytes());
        for (range, color) in &feature.text_colors {
            out.extend_from_slice(&(range.start as u32).to_le_bytes());
            out.extend_from_slice(&(range.end as u32).to_le_bytes());
            for ch in *color {
                out.extend_from_slice(&ch.to_le_bytes());
            }
        }

        if let Some(rk) = feature.repeat_key.as_deref() {
            let idx = repeat_ids.get(rk).copied().unwrap_or(0);
            out.extend_from_slice(&idx.to_le_bytes());
        }
        if let Some(line) = &feature.line {
            let key = Arc::as_ptr(&line.polyline) as *const [f32; 2] as usize;
            let poly_idx = poly_ids.get(&key).copied().unwrap_or(0);
            out.extend_from_slice(&poly_idx.to_le_bytes());
            out.extend_from_slice(&line.anchor_distance.to_le_bytes());
            out.extend_from_slice(&(line.first_glyph_index as u32).to_le_bytes());
            out.extend_from_slice(&(line.glyph_offsets.len() as u32).to_le_bytes());
            for &off in &line.glyph_offsets {
                out.extend_from_slice(&off.to_le_bytes());
            }
            out.extend_from_slice(&(line.image_sizes.len() as u32).to_le_bytes());
            for &[w, h] in &line.image_sizes {
                out.extend_from_slice(&w.to_le_bytes());
                out.extend_from_slice(&h.to_le_bytes());
            }
        }
    }
    out
}

/// Decodes symbol placement features from [`encode_symbol_features`] (falling back to JSON for
/// legacy payloads), sharing `Arc` polylines, repeat keys, and property maps across labels.
pub fn decode_symbol_features(bytes: &[u8]) -> Option<Vec<Feature>> {
    let is_v2 = bytes.starts_with(&SYM_WIRE_MAGIC_V2);
    if !is_v2 && !bytes.starts_with(&SYM_WIRE_MAGIC_V1) {
        return serde_json::from_slice::<Vec<SymbolFeature>>(bytes)
            .ok()
            .map(|items| items.into_iter().map(Feature::from).collect());
    }
    let mut buf = &bytes[4..];

    let poly_count = read_u32(&mut buf)? as usize;
    let mut polys: Vec<Arc<[[f32; 2]]>> = Vec::with_capacity(poly_count.min(4096));
    for _ in 0..poly_count {
        let pt_count = read_u32(&mut buf)? as usize;
        if buf.len() < pt_count.saturating_mul(8) {
            return None;
        }
        let mut pts = Vec::with_capacity(pt_count);
        for _ in 0..pt_count {
            pts.push([read_f32(&mut buf)?, read_f32(&mut buf)?]);
        }
        polys.push(pts.into());
    }

    let rk_count = read_u32(&mut buf)? as usize;
    let mut repeat_keys: Vec<Arc<str>> = Vec::with_capacity(rk_count.min(4096));
    for _ in 0..rk_count {
        repeat_keys.push(Arc::from(read_string(&mut buf)?));
    }

    let mut prop_groups: Vec<Arc<FeatureProperties>> = Vec::new();
    if is_v2 {
        let prop_count = read_u32(&mut buf)? as usize;
        prop_groups.reserve(prop_count.min(4096));
        for _ in 0..prop_count {
            prop_groups.push(Arc::new(decode_properties_from(&mut buf)?));
        }
    }

    let feat_count = read_u32(&mut buf)? as usize;
    let mut features = Vec::with_capacity(feat_count.min(16_384));
    for _ in 0..feat_count {
        let bounds = [
            read_f32(&mut buf)?,
            read_f32(&mut buf)?,
            read_f32(&mut buf)?,
            read_f32(&mut buf)?,
        ];
        let idx_start = read_u32(&mut buf)? as usize;
        let idx_end = read_u32(&mut buf)? as usize;
        let ax = read_f32(&mut buf)?;
        let ay = read_f32(&mut buf)?;
        let text = read_string(&mut buf)?;
        let flags = read_u8(&mut buf)?;
        let geometry_type = match read_u8(&mut buf)? {
            1 => GeometryType::Point,
            2 => GeometryType::LineString,
            3 => GeometryType::Polygon,
            _ => GeometryType::Unknown,
        };
        let sort_key = read_f32(&mut buf)?;

        let mut parts: [Option<SymbolBounds>; 3] = [None, None, None];
        for (slot_idx, slot) in parts.iter_mut().enumerate() {
            if (flags & (1 << slot_idx)) != 0 {
                *slot = Some(SymbolBounds {
                    bounds: [
                        read_f64(&mut buf)?,
                        read_f64(&mut buf)?,
                        read_f64(&mut buf)?,
                        read_f64(&mut buf)?,
                    ],
                    height: read_f64(&mut buf)?,
                    angle: read_f64(&mut buf)?,
                    text: read_u8(&mut buf)? != 0,
                });
            }
        }
        let id = if (flags & (1 << 3)) != 0 {
            Some(read_u64(&mut buf)?)
        } else {
            None
        };
        let properties = if (flags & (1 << 7)) != 0 {
            if is_v2 {
                let idx = read_u32(&mut buf)? as usize;
                Arc::clone(prop_groups.get(idx)?)
            } else {
                Arc::new(decode_properties_from(&mut buf)?)
            }
        } else {
            Arc::clone(empty_feature_properties())
        };

        let shift_count = read_u32(&mut buf)? as usize;
        if buf.len() < shift_count.saturating_mul(8) {
            return None;
        }
        let mut anchor_shifts = Vec::with_capacity(shift_count);
        for _ in 0..shift_count {
            anchor_shifts.push([read_f32(&mut buf)?, read_f32(&mut buf)?]);
        }

        let set_count = read_u32(&mut buf)? as usize;
        if buf.len() < set_count.saturating_mul(8) {
            return None;
        }
        let mut text_sets = Vec::with_capacity(set_count);
        for _ in 0..set_count {
            text_sets.push((read_u32(&mut buf)? as usize)..(read_u32(&mut buf)? as usize));
        }

        let anchor_set_len = read_u32(&mut buf)? as usize;
        if buf.len() < anchor_set_len {
            return None;
        }
        let (aset_bytes, rest) = buf.split_at(anchor_set_len);
        buf = rest;
        let anchor_sets = aset_bytes.to_vec();

        let color_count = read_u32(&mut buf)? as usize;
        if buf.len() < color_count.saturating_mul(24) {
            return None;
        }
        let mut text_colors = Vec::with_capacity(color_count);
        for _ in 0..color_count {
            let r_start = read_u32(&mut buf)? as usize;
            let r_end = read_u32(&mut buf)? as usize;
            let color = [
                read_f32(&mut buf)?,
                read_f32(&mut buf)?,
                read_f32(&mut buf)?,
                read_f32(&mut buf)?,
            ];
            text_colors.push((r_start..r_end, color));
        }

        let repeat_key = if (flags & (1 << 5)) != 0 {
            let idx = read_u32(&mut buf)? as usize;
            Some(Arc::clone(repeat_keys.get(idx)?))
        } else {
            None
        };

        let line = if (flags & (1 << 6)) != 0 {
            let poly_idx = read_u32(&mut buf)? as usize;
            let polyline = Arc::clone(polys.get(poly_idx)?);
            let anchor_distance = read_f32(&mut buf)?;
            let first_glyph_index = read_u32(&mut buf)? as usize;
            let off_count = read_u32(&mut buf)? as usize;
            if buf.len() < off_count.saturating_mul(4) {
                return None;
            }
            let mut glyph_offsets = Vec::with_capacity(off_count);
            for _ in 0..off_count {
                glyph_offsets.push(read_f32(&mut buf)?);
            }
            let img_count = read_u32(&mut buf)? as usize;
            if buf.len() < img_count.saturating_mul(8) {
                return None;
            }
            let mut image_sizes = Vec::with_capacity(img_count);
            for _ in 0..img_count {
                image_sizes.push([read_f32(&mut buf)?, read_f32(&mut buf)?]);
            }
            Some(crate::sdf::LineLabel {
                polyline,
                anchor_distance,
                glyph_offsets,
                image_sizes,
                first_glyph_index,
            })
        } else {
            None
        };

        features.push(Feature {
            bbox: Box2D::new(
                Point2D::new(bounds[0], bounds[1]),
                Point2D::new(bounds[2], bounds[3]),
            ),
            indices: idx_start..idx_end,
            text_anchor: Point2D::new(ax, ay),
            str: text,
            parts,
            data: crate::sdf::SymbolFeatureData {
                id,
                properties,
                sort_key,
                geometry_type,
            },
            anchor_shifts,
            text_sets,
            anchor_sets,
            text_colors,
            fallback: (flags & (1 << 4)) != 0,
            repeat_key,
            line,
        });
    }
    Some(features)
}

#[cfg(test)]
mod tests;
