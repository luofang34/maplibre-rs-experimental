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
        expression::{empty_feature_properties, FeatureProperties},
        layer::{FormattedText, StyleProperty, SymbolPaint},
    },
    vector::tessellation::{property_value, IndexDataType},
};

mod draw_order;
pub(crate) use draw_order::{drawn_order, rotated_height, sorts_by_height};
mod icon_quads;
mod layout;
mod line_anchors;
mod line_break;
mod line_merge;
mod pole;
mod prune;
mod text_layout;
mod text_offset;
use layout::CollectedSymbol;
pub(crate) use prune::{prune_symbol_features, source_feature_indices};

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
    /// Where each text already has a label along a line, so a repeat of it keeps its distance.
    line_anchors_by_text: rustc_hash::FxHashMap<String, Vec<[f64; 2]>>,
    properties: FeatureProperties,
    /// Pre-decoded shared properties indexed by feature order when supplied by MVT processing.
    pub(crate) shared_properties: Vec<Arc<FeatureProperties>>,
    /// Symbol triangles.
    pub quad_buffer: VertexBuffers<ShaderSymbolVertex, IndexDataType>,
    /// Collision bounds and exact triangle ranges per symbol.
    pub features: Vec<Feature>,
    /// Source layer extent conversion.
    pub coordinate_scale: f64,
    /// How many times the tile is magnified past the zoom its source stops at; zero or one when
    /// it is not.
    pub overscaling: f64,
    /// Source IDs indexed by the feature order supplied to the geometry processor.
    pub source_ids: Vec<Option<u64>>,
    /// Original MVT/MLT feature indices indexed by slice feature order.
    pub source_indices: Vec<Option<u32>>,
    /// Source `promoteId` and active runtime `feature-state` snapshot for this layer.
    pub(crate) feature_state_ctx:
        Option<crate::vector::tessellation::feature_paint::FeatureStateSyncContext>,
    /// When `Some`, point symbols stream straight into `quad_buffer`/`features` and prune
    /// feature properties immediately instead of buffering `CollectedSymbol`s.
    stream_point_runtime_keys: Option<Option<rustc_hash::FxHashSet<String>>>,
}

impl TextTessellator {
    /// A text collector with the offline fallback font.
    pub fn new(text_field: StyleProperty<FormattedText>, zoom: f64) -> Self {
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
    pub fn with_assets(mut paint: SymbolPaint, zoom: f64, atlas: Arc<SymbolAtlas>) -> Self {
        paint.resolve_zoom_enums(zoom);
        Self {
            geo_writer: GeoWriter::default(),
            paint,
            zoom,
            atlas,
            collected: Vec::new(),
            pending_lines: Vec::new(),
            line_anchors_by_text: rustc_hash::FxHashMap::default(),
            properties: FeatureProperties::default(),
            shared_properties: Vec::new(),
            quad_buffer: VertexBuffers::new(),
            features: Vec::new(),
            coordinate_scale: 1.0,
            overscaling: 1.0,
            source_ids: Vec::new(),
            source_indices: Vec::new(),
            feature_state_ctx: None,
            stream_point_runtime_keys: None,
        }
    }

    /// Streams point-placed labels straight into vertex/feature buffers and prunes properties
    /// per feature instead of retaining every `CollectedSymbol` until [`Self::finish`].
    pub(crate) fn enable_streaming_point_layout(
        &mut self,
        runtime_keys: Option<rustc_hash::FxHashSet<String>>,
    ) {
        self.stream_point_runtime_keys = Some(runtime_keys);
    }

    /// Whether point-placed symbols streamed and pruned properties during collection.
    pub(crate) fn streamed_point_layout(&self) -> bool {
        self.stream_point_runtime_keys.is_some()
    }

    /// Sets the style and the assets used by this tile's symbols.
    pub fn configure(&mut self, mut paint: SymbolPaint, atlas: Arc<SymbolAtlas>) {
        paint.resolve_zoom_enums(self.zoom);
        self.paint = paint;
        self.atlas = atlas;
    }

    /// Builds quads and collision ranges from the collected map features.
    pub fn finish(&mut self) {
        let pending = std::mem::take(&mut self.pending_lines);
        for line in line_merge::merge_lines(pending) {
            self.collect_along_lines(
                vec![line.line],
                LinePlacement::Line,
                line.source,
                &line.properties,
            );
        }
        self.properties.clear();
        let collected = std::mem::take(&mut self.collected);
        self.shared_properties.clear();
        prune::finish_collected(
            collected,
            (&self.paint, self.zoom, self.overscaling),
            &self.atlas,
            &mut self.quad_buffer,
            &mut self.features,
        );
        if draw_order::sorts_by_height(&self.paint, self.zoom) {
            draw_order::reorder(&mut self.quad_buffer, &mut self.features);
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

#[derive(Clone, Copy, PartialEq, Eq)]
enum LinePlacement {
    Line,
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
pub(crate) fn is_line_placed(paint: &SymbolPaint) -> bool {
    line_placement(paint).is_some()
}

const TILE_UNITS_PER_PIXEL: f64 = 8.0;

impl TextTessellator {
    pub(super) fn take_feature_properties(
        &mut self,
        idx: Option<usize>,
        id: Option<u64>,
    ) -> Arc<FeatureProperties> {
        if let Some(shared) = idx.and_then(|i| self.shared_properties.get(i)) {
            self.properties.clear();
            return Arc::clone(shared);
        }
        let mut props = std::mem::take(&mut self.properties);
        if let Some(ctx) = &self.feature_state_ctx {
            crate::style::feature_state::sync_properties_feature_state(
                &mut props,
                id,
                ctx.promote_property.as_deref(),
                ctx.source_state.as_deref(),
                ctx.source_layer.as_deref(),
            );
        }
        if props.is_empty() {
            Arc::clone(empty_feature_properties())
        } else {
            Arc::new(props)
        }
    }

    fn collect(
        &mut self,
        anchor: Point<f64>,
        angle: f32,
        source: layout::SourceFeature,
        props: &Arc<FeatureProperties>,
        line_context: (Option<layout::LineContext>, Option<&Arc<str>>),
    ) {
        prune::collect_symbol(self, anchor, angle, source, props, line_context);
    }

    fn anchor_is_too_close(
        &mut self,
        key: Option<&str>,
        point: [f64; 2],
        repeat_distance: f64,
    ) -> bool {
        let Some(key) = key else {
            return false;
        };
        let others = self
            .line_anchors_by_text
            .entry(key.to_string())
            .or_default();
        let too_close = others
            .iter()
            .any(|other| (other[0] - point[0]).hypot(other[1] - point[1]) < repeat_distance);
        if !too_close {
            others.push(point);
        }
        too_close
    }

    fn collect_along_lines(
        &mut self,
        lines: Vec<Vec<[f64; 2]>>,
        placement: LinePlacement,
        source: layout::SourceFeature,
        props: &Arc<FeatureProperties>,
    ) {
        let probe = CollectedSymbol {
            source,
            line: None,
            anchor: Point::new(0.0, 0.0),
            angle: 0.0,
            properties: Arc::clone(props),
            vertical: None,
            fallback: false,
            repeat_key: None,
        };
        let text_size = self
            .paint
            .text_size
            .as_ref()
            .and_then(|value| value.evaluate_at_zoom(self.zoom + 1.0))
            .map_or(16.0, f64::from);
        let text_width = f64::from(text_layout::unwrapped_width(
            &self.paint,
            &probe,
            self.zoom,
            &self.atlas,
        ));
        let icon_width = self
            .paint
            .text_among_images("icon-image", props, self.zoom, &self.atlas.icons)
            .and_then(|name| self.atlas.icons.get(&name))
            .map_or(0.0, |icon| {
                f64::from(icon.rect[2]) / f64::from(icon.metrics[3])
                    * f64::from(self.paint.number("icon-size", props, self.zoom, 1.0))
            });
        let label_pixels = text_width.max(icon_width) * text_size / 24.0;
        let number = |name, fallback| self.paint.number(name, props, self.zoom, fallback);
        let overscaling = self.overscaling.max(1.0);
        let units_per_pixel = TILE_UNITS_PER_PIXEL / overscaling;
        let params = line_anchors::AnchorSpacing {
            spacing: f64::from(number("symbol-spacing", 250.0)) * units_per_pixel,
            max_angle: f64::from(number("text-max-angle", 45.0)).to_radians(),
            label_length: label_pixels * units_per_pixel,
            text_size: text_size * units_per_pixel,
            checks_bends: text_width > 0.0,
            overscaling,
        };
        let parts: Vec<Vec<[f64; 2]>> = match placement {
            LinePlacement::Line => line_anchors::clip_to_tile(&lines),
            LinePlacement::Center => lines.into_iter().filter(|line| line.len() > 1).collect(),
        };
        let repeat_key: Option<Arc<str>> = match placement {
            LinePlacement::Line => {
                layout::repeat_key(&self.paint, props, self.zoom, &self.atlas).map(Arc::from)
            }
            LinePlacement::Center => None,
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
                if self.anchor_is_too_close(
                    repeat_key.as_deref(),
                    anchor.point,
                    params.spacing / 2.0,
                ) {
                    continue;
                }
                let distance = line_anchors::distance_to(&part, anchor);
                self.collect(
                    Point::new(anchor.point[0], anchor.point[1]),
                    anchor.angle as f32,
                    source,
                    props,
                    (
                        Some((polyline.clone(), distance as f32)),
                        repeat_key.as_ref(),
                    ),
                );
            }
        }
    }
}

impl FeatureProcessor for TextTessellator {
    fn feature_end(&mut self, idx: u64) -> GeoResult<()> {
        if let Some(geometry) = self.geo_writer.take_geometry() {
            let slice_idx = usize::try_from(idx).ok();
            let (source, props) = prune::build_source_feature(self, slice_idx, &geometry);
            let placement = line_placement(&self.paint);
            let lines: Option<Vec<Vec<[f64; 2]>>> = if placement.is_some() {
                match &geometry {
                    Geometry::LineString(line) => {
                        Some(vec![line.coords().map(|c| [c.x, c.y]).collect()])
                    }
                    Geometry::MultiLineString(lines) => Some(
                        lines
                            .iter()
                            .map(|line| line.coords().map(|c| [c.x, c.y]).collect())
                            .collect(),
                    ),
                    Geometry::Polygon(polygon) => Some(line_anchors::polygon_rings(polygon)),
                    Geometry::MultiPolygon(polygons) => Some(
                        polygons
                            .iter()
                            .flat_map(line_anchors::polygon_rings)
                            .collect(),
                    ),
                    _ => None,
                }
            } else {
                None
            };
            match (placement, lines) {
                (Some(LinePlacement::Line), Some(lines)) => {
                    let text = self
                        .paint
                        .label_among(&props, self.zoom, Some(&self.atlas.icons));
                    for line in lines.into_iter().filter(|line| line.len() > 1) {
                        self.pending_lines.push(line_merge::PendingLine {
                            text: text.clone(),
                            line,
                            source,
                            properties: Arc::clone(&props),
                        });
                    }
                }
                (Some(placement), Some(lines)) => {
                    self.collect_along_lines(lines, placement, source, &props)
                }
                (Some(_), None) => {}
                (None, _) => {
                    let tile = 0.0..crate::coords::EXTENT;
                    for anchor in layout::anchors(&geometry)
                        .into_iter()
                        .filter(|anchor| tile.contains(&anchor.x()) && tile.contains(&anchor.y()))
                    {
                        self.collect(anchor, 0.0, source, &props, (None, None));
                    }
                    if self.stream_point_runtime_keys.is_some() {
                        if let Ok(mut recycled) = Arc::try_unwrap(props) {
                            recycled.clear();
                            self.properties = recycled;
                        }
                    }
                }
            }
        } else {
            self.properties.clear();
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
