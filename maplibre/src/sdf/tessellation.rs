//! Collects symbol anchors and feature properties for font and sprite layout.
use std::sync::Arc;

use geo_types::{Geometry, Point};
use geozero::{
    geo_types::GeoWriter, ColumnValue, FeatureProcessor, GeomProcessor, PropertyProcessor,
};
use lyon::tessellation::VertexBuffers;

use crate::{
    render::shaders::ShaderSymbolVertex,
    sdf::{
        assets::{fallback_atlas, SymbolAtlas},
        Feature,
    },
    style::{
        expression::FeatureProperties,
        layer::{StyleProperty, SymbolPaint, TextField},
    },
    vector::tessellation::{property_value, IndexDataType},
};

mod icon_quads;
mod layout;
mod line_anchors;
mod line_break;
mod line_merge;
mod text_layout;
mod text_offset;
use layout::CollectedSymbol;
type GeoResult<T> = geozero::error::Result<T>;

/// Worker-side geometry for screen-sized symbols anchored to map features.
pub struct TextTessellator {
    geo_writer: GeoWriter,
    paint: SymbolPaint,
    zoom: f64,
    atlas: Arc<SymbolAtlas>,
    collected: Vec<CollectedSymbol>,
    /// Lines to be labelled along, held until they can be merged.
    pending_lines: Vec<line_merge::PendingLine>,
    properties: FeatureProperties,
    /// Symbol triangles.
    pub quad_buffer: VertexBuffers<ShaderSymbolVertex, IndexDataType>,
    /// Collision bounds and exact triangle ranges per symbol.
    pub features: Vec<Feature>,
    /// Source layer extent conversion.
    pub coordinate_scale: f64,
    /// Source IDs indexed by the feature order supplied to the geometry processor.
    pub source_ids: Vec<Option<u64>>,
}

impl TextTessellator {
    /// A text collector with the offline fallback font.
    pub fn new(text_field: StyleProperty<TextField>, zoom: f64) -> Self {
        Self {
            paint: SymbolPaint {
                text_field: Some(text_field),
                ..Default::default()
            },
            zoom,
            ..Default::default()
        }
    }

    /// Collects features using an existing atlas without decoding fallback glyphs again.
    pub fn with_assets(paint: SymbolPaint, zoom: f64, atlas: Arc<SymbolAtlas>) -> Self {
        Self {
            geo_writer: GeoWriter::default(),
            paint,
            zoom,
            atlas,
            collected: Vec::new(),
            pending_lines: Vec::new(),
            properties: FeatureProperties::new(),
            quad_buffer: VertexBuffers::new(),
            features: Vec::new(),
            coordinate_scale: 1.0,
            source_ids: Vec::new(),
        }
    }

    /// Sets the style and the assets used by this tile's symbols.
    pub fn configure(&mut self, paint: SymbolPaint, atlas: Arc<SymbolAtlas>) {
        self.paint = paint;
        self.atlas = atlas;
    }

    /// Builds quads and collision ranges from the collected map features.
    pub fn finish(&mut self) {
        let pending = std::mem::take(&mut self.pending_lines);
        for line in line_merge::merge_lines(pending) {
            self.properties = line.properties;
            self.collect_along_lines(vec![line.line], LinePlacement::Line, line.id);
        }
        self.properties.clear();
        self.collected.sort_by(|a, b| {
            let key = |symbol: &CollectedSymbol| {
                self.paint
                    .number("symbol-sort-key", &symbol.properties, self.zoom, 0.0)
            };
            key(a).total_cmp(&key(b))
        });
        for symbol in &self.collected {
            layout::append(
                symbol,
                &self.paint,
                self.zoom,
                &self.atlas,
                &mut self.quad_buffer,
                &mut self.features,
            );
        }
    }
}

impl Default for TextTessellator {
    fn default() -> Self {
        Self::with_assets(SymbolPaint::default(), 0.0, fallback_atlas())
    }
}

impl GeomProcessor for TextTessellator {
    fn xy(&mut self, x: f64, y: f64, idx: usize) -> GeoResult<()> {
        self.geo_writer
            .xy(x * self.coordinate_scale, y * self.coordinate_scale, idx)
    }
    fn point_begin(&mut self, idx: usize) -> GeoResult<()> {
        self.geo_writer.point_begin(idx)
    }
    fn point_end(&mut self, idx: usize) -> GeoResult<()> {
        self.geo_writer.point_end(idx)
    }
    fn multipoint_begin(&mut self, size: usize, idx: usize) -> GeoResult<()> {
        self.geo_writer.multipoint_begin(size, idx)
    }
    fn multipoint_end(&mut self, idx: usize) -> GeoResult<()> {
        self.geo_writer.multipoint_end(idx)
    }
    fn linestring_begin(&mut self, tagged: bool, size: usize, idx: usize) -> GeoResult<()> {
        self.geo_writer.linestring_begin(tagged, size, idx)
    }
    fn linestring_end(&mut self, tagged: bool, idx: usize) -> GeoResult<()> {
        self.geo_writer.linestring_end(tagged, idx)
    }
    fn multilinestring_begin(&mut self, size: usize, idx: usize) -> GeoResult<()> {
        self.geo_writer.multilinestring_begin(size, idx)
    }
    fn multilinestring_end(&mut self, idx: usize) -> GeoResult<()> {
        self.geo_writer.multilinestring_end(idx)
    }
    fn polygon_begin(&mut self, tagged: bool, size: usize, idx: usize) -> GeoResult<()> {
        self.geo_writer.polygon_begin(tagged, size, idx)
    }
    fn polygon_end(&mut self, tagged: bool, idx: usize) -> GeoResult<()> {
        self.geo_writer.polygon_end(tagged, idx)
    }
    fn multipolygon_begin(&mut self, size: usize, idx: usize) -> GeoResult<()> {
        self.geo_writer.multipolygon_begin(size, idx)
    }
    fn multipolygon_end(&mut self, idx: usize) -> GeoResult<()> {
        self.geo_writer.multipolygon_end(idx)
    }
}

impl PropertyProcessor for TextTessellator {
    fn property(&mut self, _idx: usize, name: &str, value: &ColumnValue) -> GeoResult<bool> {
        if let Some(value) = property_value(value) {
            self.properties.insert(name.to_string(), value);
        }
        Ok(false)
    }
}

/// How a symbol follows a line, from `symbol-placement`.
#[derive(Clone, Copy, PartialEq, Eq)]
enum LinePlacement {
    /// Repeated along the line.
    Line,
    /// Once, at the middle of the line.
    Center,
}

fn line_placement(paint: &SymbolPaint) -> Option<LinePlacement> {
    match paint
        .properties
        .get("symbol-placement")
        .and_then(|value| value.as_str())
    {
        Some("line") => Some(LinePlacement::Line),
        Some("line-center") => Some(LinePlacement::Center),
        _ => None,
    }
}

/// Whether the layer places its text along lines.
pub(super) fn is_line_placed(paint: &SymbolPaint) -> bool {
    line_placement(paint).is_some()
}

/// Tile units in one pixel of a tile drawn at its own zoom: 4096 units span 512 pixels.
const TILE_UNITS_PER_PIXEL: f64 = 8.0;

impl TextTessellator {
    fn collect(
        &mut self,
        anchor: Point<f64>,
        angle: f32,
        id: Option<u64>,
        line: Option<layout::LineContext>,
    ) {
        self.collected.push(CollectedSymbol {
            id,
            line,
            anchor,
            angle,
            properties: self.properties.clone(),
        });
    }

    /// Anchors along the lines of a feature, as many as the spacing allows.
    fn collect_along_lines(
        &mut self,
        lines: Vec<Vec<[f64; 2]>>,
        placement: LinePlacement,
        id: Option<u64>,
    ) {
        let probe = CollectedSymbol {
            id,
            line: None,
            anchor: Point::new(0.0, 0.0),
            angle: 0.0,
            properties: self.properties.clone(),
        };
        let text_size = self
            .paint
            .text_size
            .as_ref()
            .and_then(|value| value.evaluate_at_zoom(self.zoom))
            .map_or(16.0, f64::from);
        let text_width = f64::from(text_layout::unwrapped_width(
            &self.paint,
            &probe,
            self.zoom,
            &self.atlas,
        ));
        // GL JS spaces labels by the longer of the text and the icon, and scales both by the
        // text size; an icon-only label has no text to bend, so its bends are not checked.
        let icon_width = self
            .paint
            .text("icon-image", &self.properties, self.zoom)
            .and_then(|name| self.atlas.icons.get(&name))
            .map_or(0.0, |icon| {
                f64::from(icon.rect[2]) / f64::from(icon.metrics[3])
                    * f64::from(
                        self.paint
                            .number("icon-size", &self.properties, self.zoom, 1.0),
                    )
            });
        let label_pixels = text_width.max(icon_width) * text_size / 24.0;
        let number = |name, fallback| {
            self.paint
                .number(name, &self.properties, self.zoom, fallback)
        };
        let params = line_anchors::AnchorSpacing {
            spacing: f64::from(number("symbol-spacing", 250.0)) * TILE_UNITS_PER_PIXEL,
            max_angle: f64::from(number("text-max-angle", 45.0)).to_radians(),
            label_length: label_pixels * TILE_UNITS_PER_PIXEL,
            text_size: text_size * TILE_UNITS_PER_PIXEL,
            checks_bends: text_width > 0.0,
        };
        let parts: Vec<Vec<[f64; 2]>> = match placement {
            LinePlacement::Line => line_anchors::clip_to_tile(&lines),
            LinePlacement::Center => lines.into_iter().filter(|line| line.len() > 1).collect(),
        };
        for part in parts {
            let anchors = match placement {
                LinePlacement::Line => line_anchors::line_anchors(&part, params),
                LinePlacement::Center => line_anchors::center_anchor(&part, params)
                    .into_iter()
                    .collect(),
            };
            if anchors.is_empty() {
                continue;
            }
            let polyline: Arc<[[f32; 2]]> = part
                .iter()
                .map(|point| [point[0] as f32, point[1] as f32])
                .collect();
            for anchor in anchors {
                let distance = line_anchors::distance_to(&part, anchor);
                self.collect(
                    Point::new(anchor.point[0], anchor.point[1]),
                    anchor.angle as f32,
                    id,
                    Some((polyline.clone(), distance as f32)),
                );
            }
        }
    }
}

/// The rings of a polygon wound as GL JS winds them before it places labels: the outer ring
/// runs clockwise on screen and holes run counter-clockwise, so labels start at the same end.
fn polygon_rings(polygon: &geo_types::Polygon<f64>) -> Vec<Vec<[f64; 2]>> {
    let wound = |ring: &geo_types::LineString<f64>, outer: bool| {
        let mut points: Vec<[f64; 2]> = ring.coords().map(|c| [c.x, c.y]).collect();
        let area: f64 = points
            .iter()
            .zip(points.iter().cycle().skip(1))
            .map(|(a, b)| a[0] * b[1] - b[0] * a[1])
            .sum();
        // Tile coordinates grow downwards, so positive area is clockwise on screen.
        if (area > 0.0) != outer {
            points.reverse();
        }
        points
    };
    std::iter::once(wound(polygon.exterior(), true))
        .chain(polygon.interiors().iter().map(|ring| wound(ring, false)))
        .collect()
}

impl FeatureProcessor for TextTessellator {
    fn feature_end(&mut self, idx: u64) -> GeoResult<()> {
        if let Some(geometry) = self.geo_writer.take_geometry() {
            let id = usize::try_from(idx)
                .ok()
                .and_then(|idx| self.source_ids.get(idx).copied())
                .flatten();
            let lines: Option<Vec<Vec<[f64; 2]>>> = match &geometry {
                Geometry::LineString(line) => {
                    Some(vec![line.coords().map(|c| [c.x, c.y]).collect()])
                }
                Geometry::MultiLineString(lines) => Some(
                    lines
                        .iter()
                        .map(|line| line.coords().map(|c| [c.x, c.y]).collect())
                        .collect(),
                ),
                // The rings of a polygon are lines to follow, as in GL JS.
                Geometry::Polygon(polygon) => Some(polygon_rings(polygon)),
                Geometry::MultiPolygon(polygons) => {
                    Some(polygons.iter().flat_map(polygon_rings).collect())
                }
                _ => None,
            };
            match (line_placement(&self.paint), lines) {
                // Lines that continue one another are merged once all of them are known.
                (Some(LinePlacement::Line), Some(lines)) => {
                    let text = self.paint.label(&self.properties, self.zoom);
                    for line in lines.into_iter().filter(|line| line.len() > 1) {
                        self.pending_lines.push(line_merge::PendingLine {
                            text: text.clone(),
                            line,
                            id,
                            properties: self.properties.clone(),
                        });
                    }
                }
                (Some(placement), Some(lines)) => self.collect_along_lines(lines, placement, id),
                // A point has no line to follow.
                (Some(_), None) => {}
                (None, _) => {
                    if let Some(anchor) = layout::anchor(&geometry) {
                        self.collect(anchor, 0.0, id, None);
                    }
                }
            }
        }
        self.properties.clear();
        Ok(())
    }
}

#[cfg(test)]
mod tests;
