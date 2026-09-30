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

pub(super) struct CollectedSymbol {
    pub id: Option<u64>,
    pub line: Option<LineContext>,
    pub anchor: Point<f64>,
    pub properties: FeatureProperties,
    pub angle: f32,
}

pub(super) fn anchor(geometry: &Geometry<f64>) -> Option<Point<f64>> {
    match geometry {
        Geometry::LineString(line) => {
            let lengths: Vec<_> = line
                .lines()
                .map(|line| (line, (line.dx().powi(2) + line.dy().powi(2)).sqrt()))
                .collect();
            let mut remaining = lengths.iter().map(|(_, length)| length).sum::<f64>() / 2.0;
            for (line, length) in lengths {
                if remaining <= length && length > 0.0 {
                    let t = remaining / length;
                    return Some(Point::new(
                        line.start.x + line.dx() * t,
                        line.start.y + line.dy() * t,
                    ));
                }
                remaining -= length;
            }
            line.0.first().map(|point| Point::new(point.x, point.y))
        }
        _ => geometry.centroid(),
    }
}

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
    let text = paint.label(&symbol.properties, zoom).unwrap_or_default();
    if let Some(icon) = paint
        .text("icon-image", &symbol.properties, zoom)
        .and_then(|name| atlas.icons.get(&name))
    {
        let ratio = icon.metrics[3];
        let width = icon.rect[2] as f32 / ratio;
        let height = icon.rect[3] as f32 / ratio;
        let offset = offset(paint, "icon-offset", 1.0);
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
        quad(
            buffer,
            symbol.anchor,
            [
                -width * fractions[0] + offset[0],
                -height * fractions[1] + offset[1],
                width * (1.0 - fractions[0]) + offset[0],
                height * (1.0 - fractions[1]) + offset[1],
            ],
            icon,
            height_offset,
            symbol.angle,
            paint
                .number("icon-rotate", &symbol.properties, zoom, 0.0)
                .to_radians(),
        );
    }
    let first_glyph_index = buffer.indices.len();
    let glyph_offsets = super::text_layout::append(symbol, paint, zoom, atlas, buffer);
    if start == buffer.indices.len() {
        return;
    }
    let anchor = Point2D::new(symbol.anchor.x() as f32, symbol.anchor.y() as f32);
    let bbox = bounds(
        &buffer.vertices,
        &buffer.indices[start..],
        paint,
        &symbol.properties,
        zoom,
    );
    let parts = crate::sdf::placement_geometry::measure(&mut buffer.vertices[first_vertex..]);
    features.push(Feature {
        parts,
        data: crate::sdf::SymbolFeatureData {
            id: symbol.id,
            properties: symbol.properties.clone(),
            sort_key: paint.number("symbol-sort-key", &symbol.properties, zoom, 0.0),
        },
        bbox,
        indices: start..buffer.indices.len(),
        text_anchor: anchor,
        str: text,
        line: symbol
            .line
            .as_ref()
            .filter(|_| !glyph_offsets.is_empty())
            .map(|(polyline, distance)| crate::sdf::LineLabel {
                polyline: polyline.clone(),
                anchor_distance: *distance,
                glyph_offsets,
                first_glyph_index,
            }),
    });
}

pub(super) fn offset(paint: &SymbolPaint, name: &str, scale: f32) -> [f32; 2] {
    let read = |i| {
        paint
            .properties
            .get(name)
            .and_then(|value| value.get(i))
            .and_then(|value| value.as_f64())
            .unwrap_or(0.0) as f32
            * scale
    };
    [read(0), read(1)]
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
        let scale = if vertex.a_data[2] == 0 {
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
