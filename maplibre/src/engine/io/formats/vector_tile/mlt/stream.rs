//! Zero-copy columnar geometry and property streaming from `mlt_core::ParsedLayer01` directly
//! into `geozero::FeatureProcessor` / `GeomProcessor` consumers (`ZeroTessellator`,
//! `TextTessellator`, and `IndexProcessor`).

use std::sync::Arc;

use geo_types::Geometry as Geo;
use geozero::{error::Result as GeoResult, FeatureProcessor, GeomProcessor};
use mlt_core::{Decoder, Layer01, LendingIterator, MltError, ParsedLayer01, PropValueRef};
use rustc_hash::FxHashMap;

use crate::{
    coords::{WorldTileCoords, EXTENT},
    style::{
        expression::{
            empty_feature_properties, FeatureProperties, Geometry as ExprGeometry, Value,
            GEOMETRY_PROPERTY, GEOMETRY_TYPE_PROPERTY, ID_PROPERTY,
        },
        feature_state::FeatureStateSyncContext,
        filter::GeometryType,
    },
};

pub(crate) struct ColumnarLayer<'a> {
    pub(crate) parsed: ParsedLayer01<'a>,
    pub(crate) extent_scale: f64,
    pub(crate) header: geozero::mvt::tile::Layer,
    pub(crate) ids: Vec<Option<u64>>,
    pub(crate) geom_types: Vec<GeometryType>,
    pub(crate) base_props: Vec<Arc<FeatureProperties>>,
    pub(crate) has_hide_3d: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum PropAtom<'a> {
    Bool(bool),
    Bits(u64),
    Str(&'a str),
}

impl<'a> PropAtom<'a> {
    #[inline]
    fn from_ref(val: PropValueRef<'a>) -> Self {
        match val {
            PropValueRef::Bool(b) => Self::Bool(b),
            PropValueRef::Str(s) => Self::Str(s),
            other => {
                let Value::Number(n) = prop_ref_to_value(other) else {
                    return Self::Bits(0);
                };
                Self::Bits(if n == 0.0 { 0 } else { n.to_bits() })
            }
        }
    }

    #[inline]
    fn to_value(self) -> Value {
        match self {
            Self::Bool(b) => Value::Bool(b),
            Self::Bits(bits) => Value::Number(f64::from_bits(bits)),
            Self::Str(s) => Value::String(s.to_owned()),
        }
    }
}

pub(crate) fn decode_columnar_layer<'a>(
    layer: Layer01<'a>,
    decoder: &mut Decoder,
) -> Result<ColumnarLayer<'a>, MltError> {
    let parsed = layer.decode_all(decoder)?;
    let extent_u32 = parsed.extent().get();
    let extent_scale = if extent_u32 > 0 {
        EXTENT / f64::from(extent_u32)
    } else {
        1.0
    };
    let col_names: Vec<String> = parsed.iterate_prop_names().map(|n| n.to_string()).collect();
    let has_hide_3d = col_names.iter().any(|name| name == "hide_3d");
    let count = parsed.feature_count();
    let mut ids = Vec::with_capacity(count);
    let mut geom_types = Vec::with_capacity(count);
    let mut base_props = Vec::with_capacity(count);
    let empty = empty_feature_properties();

    {
        let mut memo: FxHashMap<Vec<(u16, PropAtom<'_>)>, Arc<FeatureProperties>> =
            FxHashMap::default();
        let mut key_scratch: Vec<(u16, PropAtom<'_>)> = Vec::with_capacity(col_names.len());
        let mut iter = parsed.iter_features();
        while let Some(feat_res) = iter.next() {
            let feat = feat_res?;
            ids.push(feat.id());
            geom_types.push(geometry_type_of(feat.geometry()));

            key_scratch.clear();
            for (col_idx, opt_val) in feat.iter_all_properties().enumerate() {
                if let Some(val) = opt_val {
                    key_scratch.push((col_idx as u16, PropAtom::from_ref(val)));
                }
            }
            if key_scratch.is_empty() {
                base_props.push(Arc::clone(empty));
                continue;
            }
            if let Some(existing) = memo.get(key_scratch.as_slice()) {
                base_props.push(Arc::clone(existing));
                continue;
            }
            let mut props = FeatureProperties::default();
            for &(col_idx, atom) in &key_scratch {
                if let Some(name) = col_names.get(usize::from(col_idx)) {
                    props.insert(name.clone(), atom.to_value());
                }
            }
            let arc = if props.is_empty() {
                Arc::clone(empty)
            } else {
                Arc::new(props)
            };
            memo.insert(key_scratch.clone(), Arc::clone(&arc));
            base_props.push(arc);
        }
    }

    let header = geozero::mvt::tile::Layer {
        version: 2,
        name: parsed.name().to_owned(),
        extent: Some(extent_u32),
        ..Default::default()
    };

    Ok(ColumnarLayer {
        parsed,
        extent_scale,
        header,
        ids,
        geom_types,
        base_props,
        has_hide_3d,
    })
}

pub(crate) fn props_for_style(
    col: &ColumnarLayer<'_>,
    coords: WorldTileCoords,
    state_ctx: Option<&FeatureStateSyncContext>,
    reads_id: bool,
    reads_geom: bool,
) -> Result<Vec<Arc<FeatureProperties>>, MltError> {
    if state_ctx.is_none() && !reads_id && !reads_geom {
        return Ok(col.base_props.clone());
    }
    if reads_geom {
        return props_with_geometry(col, coords, state_ctx, reads_id);
    }
    Ok(props_without_geometry(col, state_ctx, reads_id))
}

fn build_feature_props(
    col: &ColumnarLayer<'_>,
    idx: usize,
    id: Option<u64>,
    state_ctx: Option<&FeatureStateSyncContext>,
    reads_id: bool,
) -> FeatureProperties {
    let mut map = col
        .base_props
        .get(idx)
        .map_or_else(FeatureProperties::default, |arc| (**arc).clone());
    if reads_id {
        let gtype = col
            .geom_types
            .get(idx)
            .copied()
            .unwrap_or(GeometryType::Unknown);
        map.insert(
            GEOMETRY_TYPE_PROPERTY.to_owned(),
            Value::String(gtype.name().to_owned()),
        );
        if let Some(id_val) = crate::style::feature_state::resolve_effective_expr_id(
            id,
            &map,
            state_ctx.and_then(|c| c.promote_property.as_deref()),
        ) {
            map.insert(ID_PROPERTY.to_owned(), id_val);
        }
    }
    if let Some(ctx) = state_ctx {
        crate::style::feature_state::sync_properties_feature_state(
            &mut map,
            id,
            ctx.promote_property.as_deref(),
            ctx.source_state.as_deref(),
            ctx.source_layer.as_deref(),
        );
    }
    map
}

fn props_with_geometry(
    col: &ColumnarLayer<'_>,
    coords: WorldTileCoords,
    state_ctx: Option<&FeatureStateSyncContext>,
    reads_id: bool,
) -> Result<Vec<Arc<FeatureProperties>>, MltError> {
    let mut out = Vec::with_capacity(col.base_props.len());
    let extent = f64::from(col.parsed.extent().get()).max(1.0);
    let mut iter = col.parsed.iter_features();
    let mut idx = 0;
    while let Some(feat_res) = iter.next() {
        let feat = feat_res?;
        let mut map = build_feature_props(col, idx, feat.id(), state_ctx, reads_id);
        let text = lonlat_i32(feat.geometry(), coords, extent)
            .to_geojson()
            .to_string();
        map.insert(GEOMETRY_PROPERTY.to_owned(), Value::String(text));
        out.push(Arc::new(map));
        idx += 1;
    }
    Ok(out)
}

fn props_without_geometry(
    col: &ColumnarLayer<'_>,
    state_ctx: Option<&FeatureStateSyncContext>,
    reads_id: bool,
) -> Vec<Arc<FeatureProperties>> {
    let empty = empty_feature_properties();
    (0..col.base_props.len())
        .map(|idx| {
            let id = col.ids.get(idx).copied().flatten();
            let map = build_feature_props(col, idx, id, state_ctx, reads_id);
            if map.is_empty() {
                Arc::clone(empty)
            } else {
                Arc::new(map)
            }
        })
        .collect()
}

pub(crate) fn stream_layer_slice(
    col: &ColumnarLayer<'_>,
    selected: Option<&[usize]>,
    processor: &mut impl FeatureProcessor,
) -> GeoResult<()> {
    processor.dataset_begin(Some(col.parsed.name()))?;
    let mut iter = col.parsed.iter_features();
    match selected {
        None => {
            let mut out_idx = 0_u64;
            while let Some(feat_res) = iter.next() {
                let feat = feat_res
                    .map_err(|err| geozero::error::GeozeroError::Geometry(err.to_string()))?;
                processor.feature_begin(out_idx)?;
                processor.geometry_begin()?;
                stream_mlt_geometry(feat.geometry(), processor)?;
                processor.geometry_end()?;
                processor.feature_end(out_idx)?;
                out_idx += 1;
            }
        }
        Some(indices) => {
            let mut sel_pos = 0_usize;
            let mut feat_idx = 0_usize;
            while let Some(&target_idx) = indices.get(sel_pos) {
                let Some(feat_res) = iter.next() else {
                    break;
                };
                if target_idx == feat_idx {
                    let feat = feat_res
                        .map_err(|err| geozero::error::GeozeroError::Geometry(err.to_string()))?;
                    let out_idx = sel_pos as u64;
                    processor.feature_begin(out_idx)?;
                    processor.geometry_begin()?;
                    stream_mlt_geometry(feat.geometry(), processor)?;
                    processor.geometry_end()?;
                    processor.feature_end(out_idx)?;
                    sel_pos += 1;
                }
                feat_idx += 1;
            }
        }
    }
    processor.dataset_end()
}

pub(crate) fn stream_mlt_geometry(
    geom: &Geo<i32>,
    processor: &mut impl GeomProcessor,
) -> GeoResult<()> {
    match geom {
        Geo::Point(p) => {
            processor.point_begin(0)?;
            processor.xy(f64::from(p.x()), f64::from(p.y()), 0)?;
            processor.point_end(0)
        }
        Geo::MultiPoint(mp) => match mp.0.as_slice() {
            [] => Ok(()),
            [single] => {
                processor.point_begin(0)?;
                processor.xy(f64::from(single.x()), f64::from(single.y()), 0)?;
                processor.point_end(0)
            }
            many => {
                processor.multipoint_begin(many.len(), 0)?;
                for (i, p) in many.iter().enumerate() {
                    processor.xy(f64::from(p.x()), f64::from(p.y()), i)?;
                }
                processor.multipoint_end(0)
            }
        },
        Geo::Line(seg) => {
            processor.linestring_begin(true, 2, 0)?;
            processor.xy(f64::from(seg.start.x), f64::from(seg.start.y), 0)?;
            processor.xy(f64::from(seg.end.x), f64::from(seg.end.y), 1)?;
            processor.linestring_end(true, 0)
        }
        Geo::LineString(ls) => stream_linestring(ls, true, 0, processor),
        Geo::MultiLineString(mls) => match mls.0.as_slice() {
            [] => Ok(()),
            [single] => stream_linestring(single, true, 0, processor),
            many => {
                processor.multilinestring_begin(many.len(), 0)?;
                for (i, ls) in many.iter().enumerate() {
                    stream_linestring(ls, false, i, processor)?;
                }
                processor.multilinestring_end(0)
            }
        },
        Geo::Polygon(poly) => stream_polygon(poly, true, 0, processor),
        Geo::MultiPolygon(mpoly) => match mpoly.0.as_slice() {
            [] => Ok(()),
            [single] => stream_polygon(single, true, 0, processor),
            many => {
                processor.multipolygon_begin(many.len(), 0)?;
                for (i, poly) in many.iter().enumerate() {
                    stream_polygon(poly, false, i, processor)?;
                }
                processor.multipolygon_end(0)
            }
        },
        Geo::Rect(r) => stream_polygon(&r.to_polygon(), true, 0, processor),
        Geo::Triangle(t) => stream_polygon(&t.to_polygon(), true, 0, processor),
        Geo::GeometryCollection(gc) => {
            for part in &gc.0 {
                stream_mlt_geometry(part, processor)?;
            }
            Ok(())
        }
    }
}

fn stream_linestring(
    ls: &geo_types::LineString<i32>,
    tagged: bool,
    idx: usize,
    processor: &mut impl GeomProcessor,
) -> GeoResult<()> {
    if ls.0.is_empty() {
        return Ok(());
    }
    processor.linestring_begin(tagged, ls.0.len(), idx)?;
    for (i, c) in ls.0.iter().enumerate() {
        processor.xy(f64::from(c.x), f64::from(c.y), i)?;
    }
    processor.linestring_end(tagged, idx)
}

fn stream_polygon(
    poly: &geo_types::Polygon<i32>,
    tagged: bool,
    idx: usize,
    processor: &mut impl GeomProcessor,
) -> GeoResult<()> {
    let ring_count = 1 + poly.interiors().len();
    processor.polygon_begin(tagged, ring_count, idx)?;
    stream_ring(poly.exterior(), 0, processor)?;
    for (i, hole) in poly.interiors().iter().enumerate() {
        stream_ring(hole, i + 1, processor)?;
    }
    processor.polygon_end(tagged, idx)
}

fn stream_ring(
    ring: &geo_types::LineString<i32>,
    ring_idx: usize,
    processor: &mut impl GeomProcessor,
) -> GeoResult<()> {
    let coords = ring.0.as_slice();
    if coords.len() < 3 {
        return Ok(());
    }
    let already_closed = coords.first() == coords.last();
    if already_closed && coords.len() < 4 {
        return Ok(());
    }
    let unclosed_len = if already_closed {
        coords.len() - 1
    } else {
        coords.len()
    };
    processor.linestring_begin(false, unclosed_len, ring_idx)?;
    for (i, c) in coords.iter().enumerate() {
        processor.xy(f64::from(c.x), f64::from(c.y), i)?;
    }
    if !already_closed {
        if let Some(first) = coords.first() {
            processor.xy(f64::from(first.x), f64::from(first.y), coords.len())?;
        }
    }
    processor.linestring_end(false, ring_idx)
}

pub(crate) fn geometry_type_of(geom: &Geo<i32>) -> GeometryType {
    match geom {
        Geo::Point(_) | Geo::MultiPoint(_) => GeometryType::Point,
        Geo::Line(_) | Geo::LineString(_) | Geo::MultiLineString(_) => GeometryType::LineString,
        Geo::Polygon(_) | Geo::MultiPolygon(_) | Geo::Rect(_) | Geo::Triangle(_) => {
            GeometryType::Polygon
        }
        Geo::GeometryCollection(gc) => gc.0.first().map_or(GeometryType::Unknown, geometry_type_of),
    }
}

#[inline]
pub(crate) fn prop_ref_to_value(val: PropValueRef<'_>) -> Value {
    match val {
        PropValueRef::Bool(b) => Value::Bool(b),
        PropValueRef::I8(i) => Value::Number(f64::from(i)),
        PropValueRef::U8(u) => Value::Number(f64::from(u)),
        PropValueRef::I32(i) => Value::Number(f64::from(i)),
        PropValueRef::U32(u) => Value::Number(f64::from(u)),
        PropValueRef::I64(i) => Value::Number(i as f64),
        PropValueRef::U64(u) => Value::Number(u as f64),
        PropValueRef::F32(f) => Value::Number(f64::from(f)),
        PropValueRef::F64(f) => Value::Number(f),
        PropValueRef::Str(s) => Value::String(s.to_owned()),
    }
}

fn lonlat_i32(geometry: &Geo<i32>, coords: WorldTileCoords, extent: f64) -> ExprGeometry {
    let tiles = 2_f64.powi(i32::from(u8::from(coords.z)));
    let convert = |c: &geo_types::Coord<i32>| {
        let x = (f64::from(coords.x) + f64::from(c.x) / extent) / tiles;
        let y = (f64::from(coords.y) + f64::from(c.y) / extent) / tiles;
        let longitude = x * 360.0 - 180.0;
        let latitude = (std::f64::consts::PI * (1.0 - 2.0 * y))
            .sinh()
            .atan()
            .to_degrees();
        [longitude, latitude]
    };
    let mut out = ExprGeometry::default();
    collect_i32(geometry, &convert, &mut out);
    out
}

fn collect_i32(
    geometry: &Geo<i32>,
    convert: &dyn Fn(&geo_types::Coord<i32>) -> [f64; 2],
    out: &mut ExprGeometry,
) {
    let line = |ls: &geo_types::LineString<i32>| ls.0.iter().map(convert).collect::<Vec<_>>();
    let polygon = |poly: &geo_types::Polygon<i32>| {
        std::iter::once(poly.exterior())
            .chain(poly.interiors())
            .map(line)
            .collect::<Vec<_>>()
    };
    match geometry {
        Geo::Point(p) => out.points.push(convert(&p.0)),
        Geo::MultiPoint(mp) => out.points.extend(mp.0.iter().map(|p| convert(&p.0))),
        Geo::Line(seg) => out.lines.push(vec![convert(&seg.start), convert(&seg.end)]),
        Geo::LineString(ls) => out.lines.push(line(ls)),
        Geo::MultiLineString(mls) => out.lines.extend(mls.0.iter().map(line)),
        Geo::Polygon(p) => out.polygons.push(polygon(p)),
        Geo::MultiPolygon(mp) => out.polygons.extend(mp.0.iter().map(polygon)),
        Geo::GeometryCollection(gc) => gc.0.iter().for_each(|part| collect_i32(part, convert, out)),
        Geo::Rect(r) => out.polygons.push(polygon(&r.to_polygon())),
        Geo::Triangle(t) => out.polygons.push(polygon(&t.to_polygon())),
    }
}
