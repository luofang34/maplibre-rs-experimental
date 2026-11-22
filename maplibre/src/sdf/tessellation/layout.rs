//! Text and sprite quads in logical pixels around a geographic anchor.
use geo::Centroid;
use geo_types::{Geometry, Point};
use lyon::tessellation::VertexBuffers;

use crate::{
    euclid::{Box2D, Point2D},
    render::shaders::ShaderSymbolVertex,
    sdf::{
        assets::{AtlasEntry, SymbolAtlas},
        Feature,
    },
    style::{expression::FeatureProperties, layer::SymbolPaint},
};

mod avoid_edges;
mod icon;

/// A polyline in tile units and the arc length from its start to a label's anchor.
pub(super) type LineContext = (std::sync::Arc<[[f32; 2]]>, f32);

/// The source feature a label is drawn for, as a query reports it.
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct SourceFeature {
    pub id: Option<u64>,
    pub kind: crate::style::filter::GeometryType,
    pub feature_index: Option<u32>,
}

pub(super) struct CollectedSymbol {
    pub source: SourceFeature,
    pub line: Option<LineContext>,
    pub anchor: Point<f64>,
    pub properties: std::sync::Arc<FeatureProperties>,
    pub angle: f32,
    /// How the text is written when the style leaves it open: `Some(true)` vertically.
    pub vertical: Option<bool>,
    /// Whether the label is the second way of writing a text that has two.
    pub fallback: bool,
    /// What makes the label a repeat of the others along lines, see [`repeat_key`].
    pub repeat_key: Option<std::sync::Arc<str>>,
}

/// What makes two line labels repeats of each other: their text and the images they show,
/// both those standing in the text as one placeholder each and the icon behind it, so the
/// same number on an Interstate shield and on a US-route shield are different routes. A
/// label without text, such as a one-way arrow, has no repeats to keep apart.
pub(super) fn repeat_key(
    paint: &SymbolPaint,
    properties: &FeatureProperties,
    zoom: f64,
    atlas: &SymbolAtlas,
) -> Option<String> {
    let images = Some(&atlas.icons as &dyn crate::style::expression::ImageSet);
    let mut key = paint.label_among(properties, zoom, images)?;
    for section in paint.label_sections_among(properties, zoom, images) {
        if let Some(image) = section.image {
            key.push('\u{0}');
            key.push_str(&image);
        }
    }
    if let Some(icon) = paint.text_among_images("icon-image", properties, zoom, &atlas.icons) {
        key.push('\u{1}');
        key.push_str(&icon);
    }
    Some(key)
}

/// Where the labels of a geometry go when they do not follow a line, as GL JS places them:
/// at every point, at the first vertex of every line, and at the pole of inaccessibility of
/// every polygon.
pub(super) fn anchors(geometry: &Geometry<f64>) -> Vec<Point<f64>> {
    let polygon = |polygon: &geo_types::Polygon<f64>| {
        let rings: Vec<Vec<[f64; 2]>> = std::iter::once(polygon.exterior())
            .chain(polygon.interiors())
            .map(|ring| ring.coords().map(|c| [c.x, c.y]).collect())
            .collect();
        super::pole::pole_of_inaccessibility(&rings, POLE_PRECISION).map(|[x, y]| Point::new(x, y))
    };
    let first = |line: &geo_types::LineString<f64>| {
        line.0.first().map(|point| Point::new(point.x, point.y))
    };
    match geometry {
        Geometry::Point(point) => vec![*point],
        Geometry::MultiPoint(points) => points.iter().copied().collect(),
        Geometry::LineString(line) => first(line).into_iter().collect(),
        Geometry::MultiLineString(lines) => lines.iter().filter_map(first).collect(),
        Geometry::Polygon(shape) => polygon(shape).into_iter().collect(),
        Geometry::MultiPolygon(shapes) => shapes.iter().filter_map(polygon).collect(),
        other => other.centroid().into_iter().collect(),
    }
}

/// How close, in tile units, a polygon's label gets to its pole of inaccessibility: two pixels.
const POLE_PRECISION: f64 = 16.0;

pub(super) fn append(
    symbol: &CollectedSymbol,
    (paint, zoom, overscaling): (&SymbolPaint, f64, f64),
    atlas: &SymbolAtlas,
    buffer: &mut VertexBuffers<ShaderSymbolVertex, u32>,
    features: &mut Vec<Feature>,
) {
    let start = buffer.indices.len();
    let first_vertex = buffer.vertices.len();
    let text = paint
        .label_among(&symbol.properties, zoom, Some(&atlas.icons))
        .unwrap_or_default();
    let text_block = super::text_layout::block(symbol, paint, zoom, atlas);
    let icon_padding = icon::append_icon(symbol, paint, zoom, atlas, text_block.as_ref(), buffer);
    let first_glyph_index = buffer.indices.len();
    let laid = super::text_layout::append_block(symbol, paint, zoom, text_block.as_ref(), buffer);
    if start == buffer.indices.len() {
        return;
    }
    // Collision follows the text where there is one, as the icon alone otherwise.
    let shift = crate::sdf::translation::tile_translation(
        paint,
        if text.is_empty() { "icon" } else { "text" },
        zoom,
    );
    let anchor = Point2D::new(
        (symbol.anchor.x() + shift[0]) as f32,
        (symbol.anchor.y() + shift[1]) as f32,
    );
    let parts = measure_symbol_parts(
        symbol,
        buffer,
        first_vertex,
        icon_padding,
        text_block.as_ref(),
    );
    let anchor_shifts =
        super::text_layout::variable_shifts_from_block(symbol, paint, zoom, text_block.as_ref());
    if !avoid_edges::keep_symbol_within_tile(
        symbol,
        (paint, zoom, overscaling),
        anchor,
        &parts,
        &anchor_shifts,
        &laid.centres,
    ) {
        buffer.vertices.truncate(first_vertex);
        buffer.indices.truncate(start);
        return;
    }
    let indices = buffer.indices.get(start..).unwrap_or(&[]);
    let bbox = bounds(&buffer.vertices, indices, paint, &symbol.properties, zoom);
    push_symbol_feature(
        features,
        symbol,
        (paint, zoom, start, buffer.indices.len(), first_glyph_index),
        (parts, bbox, anchor, anchor_shifts, text, laid),
        text_block.as_ref().map_or(0.0, |b| b.line_offset_y),
    );
}

fn measure_symbol_parts(
    symbol: &CollectedSymbol,
    buffer: &mut VertexBuffers<ShaderSymbolVertex, u32>,
    first_vertex: usize,
    icon_padding: f32,
    text_block: Option<&super::text_layout::Block<'_>>,
) -> [Option<crate::sdf::placement_geometry::SymbolBounds>; 3] {
    let slice = buffer.vertices.get_mut(first_vertex..).unwrap_or(&mut []);
    let mut parts = crate::sdf::placement_geometry::measure(slice);
    // An icon collides by its image, not by the clear border its quad reaches into.
    let (_, rest_parts) = parts.split_at_mut(1);
    for part in rest_parts.iter_mut().flatten() {
        let pad = f64::from(icon_padding);
        part.bounds = [
            part.bounds[0] + pad,
            part.bounds[1] + pad,
            part.bounds[2] - pad,
            part.bounds[3] - pad,
        ];
    }
    // Text collides by the box of its lines, not by the bitmaps of its glyphs, which reach
    // a few pixels beyond it. A label of images alone has no glyphs to measure, so its box is
    // the layout's.
    let block_extent = super::text_layout::extent_from_block(text_block);
    if parts[0].is_none() {
        let image = slice.iter().find(|vertex| vertex.kind() == 3);
        if let (Some(vertex), Some(extent)) = (image, block_extent) {
            parts[0] = Some(crate::sdf::placement_geometry::SymbolBounds {
                bounds: extent.map(f64::from),
                height: f64::from(vertex.height()),
                angle: f64::from(vertex.angle()),
                text: true,
            });
        }
    }
    if let (Some(text_part), None) = (&mut parts[0], &symbol.line) {
        if let Some([left, top, right, bottom]) = block_extent {
            text_part.bounds = [left, top, right, bottom].map(f64::from);
        }
    }
    parts
}

type SymbolFeatureGeometry = (
    [Option<crate::sdf::placement_geometry::SymbolBounds>; 3],
    Box2D<f32, crate::sdf::TileSpace>,
    Point2D<f32, crate::sdf::TileSpace>,
    Vec<[f32; 2]>,
    String,
    super::text_layout::Laid,
);

fn push_symbol_feature(
    features: &mut Vec<Feature>,
    symbol: &CollectedSymbol,
    (paint, zoom, start, end, first_glyph_index): (&SymbolPaint, f64, usize, usize, usize),
    (parts, bbox, anchor, anchor_shifts, text, laid): SymbolFeatureGeometry,
    text_offset_y: f32,
) {
    let glyph_offsets = laid.centres;
    features.push(Feature {
        parts,
        data: crate::sdf::SymbolFeatureData {
            id: symbol.source.id,
            geometry_type: symbol.source.kind,
            properties: symbol.properties.clone(),
            sort_key: paint.number("symbol-sort-key", &symbol.properties, zoom, 0.0),
            source_feature_index: symbol.source.feature_index,
        },
        bbox,
        indices: start..end,
        text_anchor: anchor,
        anchor_shifts,
        text_sets: laid.sets,
        anchor_sets: laid.anchor_sets,
        text_colors: laid.colors,
        fallback: symbol.fallback,
        repeat_key: symbol.repeat_key.clone(),
        str: text,
        line: symbol
            .line
            .as_ref()
            .filter(|_| !glyph_offsets.is_empty())
            .map(|(polyline, distance)| crate::sdf::LineLabel {
                polyline: polyline.clone(),
                anchor_distance: *distance,
                glyph_offsets,
                image_sizes: laid.image_sizes,
                first_glyph_index,
                text_offset_y,
            }),
    });
}

/// The `[x, y]` offset a layout property gives one symbol, times `scale`; zero without one.
pub(super) fn offset(
    paint: &SymbolPaint,
    name: &str,
    scale: f32,
    (properties, zoom): (&crate::style::expression::FeatureProperties, f64),
) -> [f32; 2] {
    let Some(value) = paint.properties.get(name) else {
        return [0.0; 2];
    };
    // A written pair is the common case and needs no expression.
    let [x, y] = match crate::style::translation::Pair::from_json(value) {
        Some(pair) => pair.0,
        None => {
            crate::style::property::StyleProperty::<crate::style::translation::Pair>::parse(value)
                .evaluate_for(properties, zoom)
                .map_or([0.0; 2], |pair| pair.0)
        }
    };
    [x as f32 * scale, y as f32 * scale]
}

pub(super) fn anchor_fractions(anchor: &str) -> [f32; 2] {
    [
        if anchor.contains("left") {
            0.0
        } else if anchor.contains("right") {
            1.0
        } else {
            0.5
        },
        if anchor.contains("top") {
            0.0
        } else if anchor.contains("bottom") {
            1.0
        } else {
            0.5
        },
    ]
}

pub(super) fn quad(
    buffer: &mut VertexBuffers<ShaderSymbolVertex, u32>,
    anchor: Point<f64>,
    bounds: [f32; 4],
    image: &AtlasEntry,
    height: f32,
    angle: f32,
    rotation: f32,
) {
    let base = buffer.vertices.len() as u32;
    let (sin, cos) = rotation.sin_cos();
    let [x, y, width, height_pixels] = image.rect;
    for (index, (u, v)) in [
        (x, y),
        (x + width, y),
        (x + width, y + height_pixels),
        (x, y + height_pixels),
    ]
    .into_iter()
    .enumerate()
    {
        let px = if index == 0 || index == 3 {
            bounds[0]
        } else {
            bounds[2]
        };
        let py = if index < 2 { bounds[1] } else { bounds[3] };
        // The quad turns about the anchor, offsets included, as GL JS rotates its corners.
        let (px, py) = (px * cos - py * sin, px * sin + py * cos);
        buffer.vertices.push(ShaderSymbolVertex::new(
            [anchor.x().round() as i32, anchor.y().round() as i32],
            [(px * 32.0).round() as i32, (py * 32.0).round() as i32],
            [u, v],
            image.kind,
            height,
            angle,
        ));
    }
    buffer
        .indices
        .extend([base, base + 1, base + 2, base, base + 2, base + 3]);
}

fn bounds(
    vertices: &[ShaderSymbolVertex],
    indices: &[u32],
    paint: &SymbolPaint,
    properties: &FeatureProperties,
    zoom: f64,
) -> Box2D<f32, crate::sdf::TileSpace> {
    let text_size = paint
        .text_size
        .as_ref()
        .and_then(|value| value.evaluate_for(properties, zoom))
        .unwrap_or(16.0);
    let icon_size = paint.number("icon-size", properties, zoom, 1.0);
    let mut bounds = Box2D::new(
        Point2D::new(f32::INFINITY, f32::INFINITY),
        Point2D::new(f32::NEG_INFINITY, f32::NEG_INFINITY),
    );
    for index in indices {
        let Some(vertex) = vertices.get(*index as usize) else {
            continue;
        };
        let scale = if matches!(vertex.kind(), 0 | 3) {
            text_size / 24.0
        } else {
            icon_size
        };
        let point: Point2D<f32, crate::sdf::TileSpace> = Point2D::new(
            vertex.a_pos_offset[2] as f32 / 32.0 * scale,
            vertex.a_pos_offset[3] as f32 / 32.0 * scale,
        );
        bounds.min.x = bounds.min.x.min(point.x);
        bounds.min.y = bounds.min.y.min(point.y);
        bounds.max.x = bounds.max.x.max(point.x);
        bounds.max.y = bounds.max.y.max(point.y);
    }
    bounds
}
