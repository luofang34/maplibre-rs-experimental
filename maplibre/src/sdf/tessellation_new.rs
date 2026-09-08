//! Collects symbol anchors and feature properties for font and sprite layout.
use crate::{
    render::shaders::ShaderSymbolVertexNew,
    sdf::{
        assets::{fallback_atlas, SymbolAtlas},
        tessellation::IndexDataType,
        Feature,
    },
    style::{
        expression::FeatureProperties,
        layer::{StyleProperty, SymbolPaint, TextField},
    },
    vector::tessellation::property_value,
};
use geozero::{
    geo_types::GeoWriter, ColumnValue, FeatureProcessor, GeomProcessor, PropertyProcessor,
};
use lyon::tessellation::VertexBuffers;
use std::sync::Arc;

mod layout;
mod text_layout;
use layout::CollectedSymbol;
type GeoResult<T> = geozero::error::Result<T>;

/// Worker-side geometry for screen-sized symbols anchored to map features.
pub struct TextTessellatorNew {
    geo_writer: GeoWriter,
    paint: SymbolPaint,
    zoom: f64,
    atlas: Arc<SymbolAtlas>,
    collected: Vec<CollectedSymbol>,
    properties: FeatureProperties,
    /// Symbol triangles.
    pub quad_buffer: VertexBuffers<ShaderSymbolVertexNew, IndexDataType>,
    /// Collision bounds and exact triangle ranges per symbol.
    pub features: Vec<Feature>,
    /// Source layer extent conversion.
    pub coordinate_scale: f64,
    /// Source IDs indexed by the feature order supplied to the geometry processor.
    pub source_ids: Vec<Option<u64>>,
}

impl TextTessellatorNew {
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

impl Default for TextTessellatorNew {
    fn default() -> Self {
        Self::with_assets(SymbolPaint::default(), 0.0, fallback_atlas())
    }
}

impl GeomProcessor for TextTessellatorNew {
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

impl PropertyProcessor for TextTessellatorNew {
    fn property(&mut self, _idx: usize, name: &str, value: &ColumnValue) -> GeoResult<bool> {
        if let Some(value) = property_value(value) {
            self.properties.insert(name.to_string(), value);
        }
        Ok(false)
    }
}

impl FeatureProcessor for TextTessellatorNew {
    fn feature_end(&mut self, idx: u64) -> GeoResult<()> {
        if let Some(geometry) = self.geo_writer.take_geometry() {
            if let Some(anchor) = layout::anchor(&geometry) {
                let on_line = self
                    .paint
                    .properties
                    .get("symbol-placement")
                    .and_then(|value| value.as_str())
                    .is_some_and(|value| value == "line" || value == "line-center");
                let angle = if on_line {
                    layout::line_angle(&geometry, anchor)
                } else {
                    0.0
                };
                self.collected.push(CollectedSymbol {
                    id: usize::try_from(idx)
                        .ok()
                        .and_then(|idx| self.source_ids.get(idx).copied())
                        .flatten(),
                    anchor,
                    angle,
                    properties: std::mem::take(&mut self.properties),
                });
            }
        }
        self.properties.clear();
        Ok(())
    }
}
