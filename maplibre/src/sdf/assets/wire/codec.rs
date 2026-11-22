//! Binary table and per-feature encode/decode helpers for [`super::encode_symbol_features`] and
//! [`super::decode_symbol_features`].

use std::sync::Arc;

use rustc_hash::FxHashMap;

use crate::{
    euclid::{Box2D, Point2D},
    io::geometry_index::wire::{
        decode_properties_from, encode_properties, encode_str, read_f32, read_f64, read_string,
        read_u32, read_u64, read_u8,
    },
    sdf::{placement_geometry::SymbolBounds, Feature, LineLabel},
    style::{
        expression::{empty_feature_properties, FeatureProperties},
        filter::GeometryType,
    },
};

pub(super) struct SharedTables<'a> {
    poly_ids: FxHashMap<usize, u32>,
    polys: Vec<&'a [[f32; 2]]>,
    repeat_ids: FxHashMap<&'a str, u32>,
    repeat_keys: Vec<&'a str>,
    prop_ids: FxHashMap<usize, u32>,
    prop_groups: Vec<&'a FeatureProperties>,
}

pub(super) fn collect_shared_tables<'a>(features: &'a [Feature]) -> SharedTables<'a> {
    let mut tables = SharedTables {
        poly_ids: FxHashMap::default(),
        polys: Vec::new(),
        repeat_ids: FxHashMap::default(),
        repeat_keys: Vec::new(),
        prop_ids: FxHashMap::default(),
        prop_groups: Vec::new(),
    };
    for feature in features {
        if let Some(line) = &feature.line {
            let key = Arc::as_ptr(&line.polyline) as *const [f32; 2] as usize;
            if let std::collections::hash_map::Entry::Vacant(slot) = tables.poly_ids.entry(key) {
                let id = tables.polys.len() as u32;
                tables.polys.push(&line.polyline);
                slot.insert(id);
            }
        }
        if let Some(rk) = feature.repeat_key.as_deref() {
            if let std::collections::hash_map::Entry::Vacant(slot) = tables.repeat_ids.entry(rk) {
                let id = tables.repeat_keys.len() as u32;
                tables.repeat_keys.push(rk);
                slot.insert(id);
            }
        }
        if !feature.data.properties.is_empty() {
            let key = Arc::as_ptr(&feature.data.properties) as usize;
            if let std::collections::hash_map::Entry::Vacant(slot) = tables.prop_ids.entry(key) {
                let id = tables.prop_groups.len() as u32;
                tables.prop_groups.push(&feature.data.properties);
                slot.insert(id);
            }
        }
    }
    tables
}

pub(super) fn write_shared_tables(tables: &SharedTables<'_>, out: &mut Vec<u8>) {
    out.extend_from_slice(&(tables.polys.len() as u32).to_le_bytes());
    for poly in &tables.polys {
        out.extend_from_slice(&(poly.len() as u32).to_le_bytes());
        for &[x, y] in *poly {
            out.extend_from_slice(&x.to_le_bytes());
            out.extend_from_slice(&y.to_le_bytes());
        }
    }
    out.extend_from_slice(&(tables.repeat_keys.len() as u32).to_le_bytes());
    for rk in &tables.repeat_keys {
        encode_str(rk, out);
    }
    out.extend_from_slice(&(tables.prop_groups.len() as u32).to_le_bytes());
    for props in &tables.prop_groups {
        encode_properties(props, out);
    }
}

pub(super) fn encode_feature(feature: &Feature, tables: &SharedTables<'_>, out: &mut Vec<u8>) {
    encode_feature_header(feature, out);
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
        let prop_idx = tables.prop_ids.get(&key).copied().unwrap_or(0);
        out.extend_from_slice(&prop_idx.to_le_bytes());
    }
    encode_feature_layout_runs(feature, tables, out);
}

fn encode_feature_header(feature: &Feature, out: &mut Vec<u8>) {
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
    encode_str(&feature.str, out);
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
    out.extend_from_slice(
        &feature
            .data
            .source_feature_index
            .unwrap_or(u32::MAX)
            .to_le_bytes(),
    );
}

fn encode_feature_layout_runs(feature: &Feature, tables: &SharedTables<'_>, out: &mut Vec<u8>) {
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
        let idx = tables.repeat_ids.get(rk).copied().unwrap_or(0);
        out.extend_from_slice(&idx.to_le_bytes());
    }
    if let Some(line) = &feature.line {
        let key = Arc::as_ptr(&line.polyline) as *const [f32; 2] as usize;
        let poly_idx = tables.poly_ids.get(&key).copied().unwrap_or(0);
        out.extend_from_slice(&poly_idx.to_le_bytes());
        out.extend_from_slice(&line.anchor_distance.to_le_bytes());
        out.extend_from_slice(&line.text_offset_y.to_le_bytes());
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

pub(super) struct DecodedTables {
    polys: Vec<Arc<[[f32; 2]]>>,
    repeat_keys: Vec<Arc<str>>,
    prop_groups: Vec<Arc<FeatureProperties>>,
}

pub(super) fn decode_tables(buf: &mut &[u8], is_v2: bool) -> Option<DecodedTables> {
    let poly_count = read_u32(buf)? as usize;
    let mut polys: Vec<Arc<[[f32; 2]]>> = Vec::with_capacity(poly_count.min(4096));
    for _ in 0..poly_count {
        let pt_count = read_u32(buf)? as usize;
        if buf.len() < pt_count.saturating_mul(8) {
            return None;
        }
        let mut pts = Vec::with_capacity(pt_count);
        for _ in 0..pt_count {
            pts.push([read_f32(buf)?, read_f32(buf)?]);
        }
        polys.push(pts.into());
    }
    let rk_count = read_u32(buf)? as usize;
    let mut repeat_keys: Vec<Arc<str>> = Vec::with_capacity(rk_count.min(4096));
    for _ in 0..rk_count {
        repeat_keys.push(Arc::from(read_string(buf)?));
    }
    let mut prop_groups: Vec<Arc<FeatureProperties>> = Vec::new();
    if is_v2 {
        let prop_count = read_u32(buf)? as usize;
        prop_groups.reserve(prop_count.min(4096));
        for _ in 0..prop_count {
            prop_groups.push(Arc::new(decode_properties_from(buf)?));
        }
    }
    Some(DecodedTables {
        polys,
        repeat_keys,
        prop_groups,
    })
}

pub(super) fn decode_feature(
    buf: &mut &[u8],
    tables: &DecodedTables,
    is_v2: bool,
    is_v3: bool,
) -> Option<Feature> {
    let bounds = [
        read_f32(buf)?,
        read_f32(buf)?,
        read_f32(buf)?,
        read_f32(buf)?,
    ];
    let idx_start = read_u32(buf)? as usize;
    let idx_end = read_u32(buf)? as usize;
    let (ax, ay) = (read_f32(buf)?, read_f32(buf)?);
    let text = read_string(buf)?;
    let flags = read_u8(buf)?;
    let geometry_type = decode_geometry_type(read_u8(buf)?);
    let sort_key = read_f32(buf)?;
    let source_feature_index = if is_v3 {
        Some(read_u32(buf)?).filter(|&raw| raw != u32::MAX)
    } else {
        None
    };
    let parts = decode_parts(buf, flags)?;
    let id = ((flags & (1 << 3)) != 0).then(|| read_u64(buf)).flatten();
    let properties = decode_feature_properties(buf, tables, flags, is_v2)?;
    let (anchor_shifts, text_sets, anchor_sets, text_colors) = decode_layout_runs(buf)?;
    let repeat_key = if (flags & (1 << 5)) != 0 {
        Some(Arc::clone(tables.repeat_keys.get(read_u32(buf)? as usize)?))
    } else {
        None
    };
    let line = if (flags & (1 << 6)) != 0 {
        Some(decode_line_label(buf, &tables.polys)?)
    } else {
        None
    };
    Some(Feature {
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
            source_feature_index,
        },
        anchor_shifts,
        text_sets,
        anchor_sets,
        text_colors,
        fallback: (flags & (1 << 4)) != 0,
        repeat_key,
        line,
    })
}

fn decode_geometry_type(raw: u8) -> GeometryType {
    match raw {
        1 => GeometryType::Point,
        2 => GeometryType::LineString,
        3 => GeometryType::Polygon,
        _ => GeometryType::Unknown,
    }
}

fn decode_feature_properties(
    buf: &mut &[u8],
    tables: &DecodedTables,
    flags: u8,
    is_v2: bool,
) -> Option<Arc<FeatureProperties>> {
    if (flags & (1 << 7)) == 0 {
        return Some(Arc::clone(empty_feature_properties()));
    }
    if is_v2 {
        let idx = read_u32(buf)? as usize;
        Some(Arc::clone(tables.prop_groups.get(idx)?))
    } else {
        Some(Arc::new(decode_properties_from(buf)?))
    }
}

fn decode_parts(buf: &mut &[u8], flags: u8) -> Option<[Option<SymbolBounds>; 3]> {
    let mut parts: [Option<SymbolBounds>; 3] = [None, None, None];
    for (slot_idx, slot) in parts.iter_mut().enumerate() {
        if (flags & (1 << slot_idx)) != 0 {
            *slot = Some(SymbolBounds {
                bounds: [
                    read_f64(buf)?,
                    read_f64(buf)?,
                    read_f64(buf)?,
                    read_f64(buf)?,
                ],
                height: read_f64(buf)?,
                angle: read_f64(buf)?,
                text: read_u8(buf)? != 0,
            });
        }
    }
    Some(parts)
}

type LayoutRuns = (
    Vec<[f32; 2]>,
    Vec<std::ops::Range<usize>>,
    Vec<u8>,
    Vec<(std::ops::Range<usize>, [f32; 4])>,
);

fn decode_layout_runs(buf: &mut &[u8]) -> Option<LayoutRuns> {
    let shift_count = read_u32(buf)? as usize;
    if buf.len() < shift_count.saturating_mul(8) {
        return None;
    }
    let mut anchor_shifts = Vec::with_capacity(shift_count);
    for _ in 0..shift_count {
        anchor_shifts.push([read_f32(buf)?, read_f32(buf)?]);
    }
    let set_count = read_u32(buf)? as usize;
    if buf.len() < set_count.saturating_mul(8) {
        return None;
    }
    let mut text_sets = Vec::with_capacity(set_count);
    for _ in 0..set_count {
        text_sets.push((read_u32(buf)? as usize)..(read_u32(buf)? as usize));
    }
    let anchor_set_len = read_u32(buf)? as usize;
    if buf.len() < anchor_set_len {
        return None;
    }
    let (aset_bytes, rest) = buf.split_at(anchor_set_len);
    *buf = rest;
    let anchor_sets = aset_bytes.to_vec();
    let color_count = read_u32(buf)? as usize;
    if buf.len() < color_count.saturating_mul(24) {
        return None;
    }
    let mut text_colors = Vec::with_capacity(color_count);
    for _ in 0..color_count {
        let r_start = read_u32(buf)? as usize;
        let r_end = read_u32(buf)? as usize;
        let color = [
            read_f32(buf)?,
            read_f32(buf)?,
            read_f32(buf)?,
            read_f32(buf)?,
        ];
        text_colors.push((r_start..r_end, color));
    }
    Some((anchor_shifts, text_sets, anchor_sets, text_colors))
}

fn decode_line_label(buf: &mut &[u8], polys: &[Arc<[[f32; 2]]>]) -> Option<LineLabel> {
    let poly_idx = read_u32(buf)? as usize;
    let polyline = Arc::clone(polys.get(poly_idx)?);
    let anchor_distance = read_f32(buf)?;
    let text_offset_y = read_f32(buf)?;
    let first_glyph_index = read_u32(buf)? as usize;
    let off_count = read_u32(buf)? as usize;
    if buf.len() < off_count.saturating_mul(4) {
        return None;
    }
    let mut glyph_offsets = Vec::with_capacity(off_count);
    for _ in 0..off_count {
        glyph_offsets.push(read_f32(buf)?);
    }
    let img_count = read_u32(buf)? as usize;
    if buf.len() < img_count.saturating_mul(8) {
        return None;
    }
    let mut image_sizes = Vec::with_capacity(img_count);
    for _ in 0..img_count {
        image_sizes.push([read_f32(buf)?, read_f32(buf)?]);
    }
    Some(LineLabel {
        polyline,
        anchor_distance,
        glyph_offsets,
        image_sizes,
        first_glyph_index,
        text_offset_y,
    })
}
