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

/// A polyline in tile units and the arc length from its start to a label's anchor.
pub(super) type LineContext = (std::sync::Arc<[[f32; 2]]>, f32);

/// The source feature a label is drawn for, as a query reports it.
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct SourceFeature {
    pub id: Option<u64>,
    pub kind: crate::style::filter::GeometryType,
}

pub(super) struct CollectedSymbol {
    pub source: SourceFeature,
    pub line: Option<LineContext>,
    pub anchor: Point<f64>,
    pub properties: FeatureProperties,
    pub angle: f32,
    /// How the text is written when the style leaves it open: `Some(true)` vertically.
    pub vertical: Option<bool>,
    /// Whether the label is the second way of writing a text that has two.
    pub fallback: bool,
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
    paint: &SymbolPaint,
    zoom: f64,
    atlas: &SymbolAtlas,
    buffer: &mut VertexBuffers<ShaderSymbolVertex, u32>,
    features: &mut Vec<Feature>,
) {
    let start = buffer.indices.len();
    let first_vertex = buffer.vertices.len();
    let mut icon_padding = 0.0_f32;
    let text = paint
        .label_among(&symbol.properties, zoom, Some(&atlas.icons))
        .unwrap_or_default();
    if let Some(icon) = paint
        .text_among_images("icon-image", &symbol.properties, zoom, &atlas.icons)
        .and_then(|name| atlas.icons.get(&name))
    {
        let ratio = icon.metrics[3];
        let width = icon.rect[2] as f32 / ratio;
        let height = icon.rect[3] as f32 / ratio;
        let offset = offset(paint, "icon-offset", 1.0, (&symbol.properties, zoom));
        let fractions = anchor_fractions(
            &paint
                .text("icon-anchor", &symbol.properties, zoom)
                .unwrap_or_else(|| "center".into()),
        );
        let height_offset = if paint.uses_shared_height() {
            0.0
        } else {
            paint.height_offset("icon", &symbol.properties, zoom)
        };
        let icon_size = paint.number("icon-size", &symbol.properties, zoom, 1.0);
        // An image whose anchor is not its centre, such as a shield with a banner above its
        // body, is moved so that the anchor stands where the centre would.
        let recentre = [icon.metrics[0] / ratio, icon.metrics[1] / ratio];
        let placed = [
            -width * fractions[0] + offset[0] - recentre[0],
            -height * fractions[1] + offset[1] - recentre[1],
            width * (1.0 - fractions[0]) + offset[0] - recentre[0],
            height * (1.0 - fractions[1]) + offset[1] - recentre[1],
        ];
        let fitted = fit_to_text(paint, symbol, zoom, atlas, [width, height], offset);
        let shift = crate::sdf::translation::tile_translation(paint, "icon", zoom);
        let anchor = Point::new(symbol.anchor.x() + shift[0], symbol.anchor.y() + shift[1]);
        let rotation = paint
            .number("icon-rotate", &symbol.properties, zoom, 0.0)
            .to_radians()
            + if super::text_layout::is_vertical(symbol, paint, zoom, atlas) {
                std::f32::consts::FRAC_PI_2
            } else {
                0.0
            };
        // Layout pixels are drawn scaled by icon-size, which a fitted icon does not take.
        let undo_size = if fitted.is_some() {
            1.0 / icon_size
        } else {
            1.0
        };
        for piece in super::icon_quads::icon_quads(icon, fitted.unwrap_or(placed), fitted.is_some())
        {
            icon_padding = icon_padding.max(piece.padding * undo_size);
            let part = AtlasEntry {
                rect: piece.rect,
                ..icon.clone()
            };
            quad(
                buffer,
                anchor,
                piece.bounds.map(|edge| edge * undo_size),
                &part,
                height_offset,
                symbol.angle,
                rotation,
            );
        }
    }
    let first_glyph_index = buffer.indices.len();
    let laid = super::text_layout::append(symbol, paint, zoom, atlas, buffer);
    let glyph_offsets = laid.centres;
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
    let bbox = bounds(
        &buffer.vertices,
        &buffer.indices[start..],
        paint,
        &symbol.properties,
        zoom,
    );
    let mut parts = crate::sdf::placement_geometry::measure(&mut buffer.vertices[first_vertex..]);
    // An icon collides by its image, not by the clear border its quad reaches into.
    for part in parts[1..].iter_mut().flatten() {
        let pad = f64::from(icon_padding);
        part.bounds = [
            part.bounds[0] + pad,
            part.bounds[1] + pad,
            part.bounds[2] - pad,
            part.bounds[3] - pad,
        ];
    }
    // Text collides by the box of its lines, not by the bitmaps of its glyphs, which reach
    // a few pixels beyond it.
    // A label of images alone, such as a row of road shields along a road, has no glyphs to
    // measure, so its box is the layout's.
    if parts[0].is_none() {
        let image = buffer.vertices[first_vertex..]
            .iter()
            .find(|vertex| vertex.a_data[2] == 3);
        if let (Some(vertex), Some(extent)) = (
            image,
            super::text_layout::extent(symbol, paint, zoom, atlas),
        ) {
            parts[0] = Some(crate::sdf::placement_geometry::SymbolBounds {
                bounds: extent.map(f64::from),
                height: f64::from(f32::from_bits(vertex.a_pixeloffset[2] as u32)),
                angle: f64::from(f32::from_bits(vertex.a_pixeloffset[3] as u32)),
                text: true,
            });
        }
    }
    if let (Some(text_part), None) = (&mut parts[0], &symbol.line) {
        if let Some([left, top, right, bottom]) =
            super::text_layout::extent(symbol, paint, zoom, atlas)
        {
            text_part.bounds = [left, top, right, bottom].map(f64::from);
        }
    }
    features.push(Feature {
        parts,
        data: crate::sdf::SymbolFeatureData {
            id: symbol.source.id,
            geometry_type: symbol.source.kind,
            properties: symbol.properties.clone(),
            sort_key: paint.number("symbol-sort-key", &symbol.properties, zoom, 0.0),
        },
        bbox,
        indices: start..buffer.indices.len(),
        text_anchor: anchor,
        anchor_shifts: super::text_layout::variable_shifts(symbol, paint, zoom, atlas),
        text_sets: laid.sets,
        anchor_sets: laid.anchor_sets,
        text_colors: laid.colors,
        fallback: symbol.fallback,
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
            }),
    });
}

/// The icon box `[left, top, right, bottom]` that `icon-text-fit` stretches around the text,
/// in text-size pixels, or `None` when the icon keeps its own size.
fn fit_to_text(
    paint: &SymbolPaint,
    symbol: &CollectedSymbol,
    zoom: f64,
    atlas: &SymbolAtlas,
    [icon_width, icon_height]: [f32; 2],
    offset: [f32; 2],
) -> Option<[f32; 4]> {
    let fit = paint.text("icon-text-fit", &symbol.properties, zoom)?;
    if fit == "none" {
        return None;
    }
    let text_size = paint
        .text_size
        .as_ref()
        .and_then(|value| value.evaluate_for(&symbol.properties, zoom))
        .unwrap_or(16.0);
    let scale = text_size / 24.0;
    // A vertical label's icon is fitted before the symbol turns.
    let [left, top, right, bottom] =
        super::text_layout::layout_extent(symbol, paint, zoom, atlas)?.map(|edge| edge * scale);
    let padding = paint
        .properties
        .get("icon-text-fit-padding")
        .and_then(|value| value.as_array())
        .filter(|values| values.len() == 4)
        .map_or([0.0; 4], |values| {
            let read = |index: usize| values[index].as_f64().unwrap_or(0.0) as f32;
            [read(0), read(1), read(2), read(3)]
        });
    let (min_x, max_x) = if matches!(fit.as_str(), "width" | "both") {
        (
            offset[0] + left - padding[3],
            offset[0] + right + padding[1],
        )
    } else {
        let min = offset[0] + (left + right - icon_width) / 2.0;
        (min, min + icon_width)
    };
    let (min_y, max_y) = if matches!(fit.as_str(), "height" | "both") {
        (
            offset[1] + top - padding[0],
            offset[1] + bottom + padding[2],
        )
    } else {
        let min = offset[1] + (top + bottom - icon_height) / 2.0;
        (min, min + icon_height)
    };
    Some([min_x, min_y, max_x, max_y])
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
        buffer.vertices.push(ShaderSymbolVertex {
            a_pos_offset: [
                anchor.x().round() as i32,
                anchor.y().round() as i32,
                (px * 32.0).round() as i32,
                (py * 32.0).round() as i32,
            ],
            a_data: [u, v, image.kind, 0],
            a_pixeloffset: [0, 0, height.to_bits() as i32, angle.to_bits() as i32],
        });
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
        let vertex = &vertices[*index as usize];
        let scale = if matches!(vertex.a_data[2], 0 | 3) {
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
