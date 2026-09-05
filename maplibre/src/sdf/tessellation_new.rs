//! Tessellation for lines and polygons is implemented here.

use std::collections::HashMap;

use geo_types::Geometry;
use geozero::{
    geo_types::GeoWriter, ColumnValue, FeatureProcessor, GeomProcessor, PropertyProcessor,
};
use lyon::{
    geom::euclid::{Box2D, Point2D},
    tessellation::VertexBuffers,
};
use widestring::U16String;

use crate::{
    euclid::{Rect, Size2D},
    legacy::{
        bidi::{apply_arabic_shaping, Char16},
        buckets::symbol_bucket::SymbolBucketBuffer,
        font_stack::FontStackHasher,
        geometry_tile_data::{GeometryCoordinates, SymbolGeometryTileLayer},
        glyph::{Glyph, GlyphDependencies, GlyphMap, GlyphMetrics, Glyphs},
        glyph_atlas::{GlyphPosition, GlyphPositionMap, GlyphPositions},
        image::ImageMap,
        image_atlas::ImagePositions,
        layout::{
            layout::{BucketParameters, LayerTypeInfo, LayoutParameters},
            symbol_feature::{SymbolGeometryTileFeature, VectorGeometryTileFeature},
            symbol_layout::{FeatureIndex, LayerProperties, SymbolLayer, SymbolLayout},
        },
        style_types::SymbolLayoutProperties_Unevaluated,
        tagged_string::TaggedString,
        CanonicalTileID, MapMode, OverscaledTileID, TileSpace,
    },
    render::shaders::ShaderSymbolVertexNew,
    sdf::{tessellation::IndexDataType, text::GlyphSet, Feature},
    style::{
        expression::FeatureProperties,
        layer::{StyleProperty, TextField},
    },
    vector::tessellation::property_value,
};

type GeoResult<T> = geozero::error::Result<T>;

/// Build tessellations with vectors.
pub struct TextTessellatorNew {
    geo_writer: GeoWriter,

    // configuration
    text_field: StyleProperty<TextField>,
    /// Zoom of the tile, at which zoom-driven text is evaluated.
    zoom: f64,

    // output
    pub quad_buffer: VertexBuffers<ShaderSymbolVertexNew, IndexDataType>,
    pub features: Vec<Feature>,

    // collected feature data from tile processing
    collected_features: Vec<(String, f64, f64)>,

    // iteration variables
    current_index: usize,
    feature_properties: FeatureProperties,
    current_origin: Option<Box2D<f32, TileSpace>>,
    current_point: Option<(f64, f64)>,
    /// Factor from the source layer's coordinate extent to the 4096 tile grid.
    pub coordinate_scale: f64,
}

impl TextTessellatorNew {
    pub fn finish(&mut self) {
        let data = include_bytes!("../../../data/0-255.pbf");
        let glyphs = GlyphSet::try_from(data.as_slice()).unwrap();

        let font_stack = vec![
            "Open Sans Regular".to_string(),
            "Arial Unicode MS Regular".to_string(),
        ];

        let layer_name = "layer".to_string();

        let mut glyph_dependencies = GlyphDependencies::new();

        let tile_id = OverscaledTileID {
            canonical: CanonicalTileID { x: 0, y: 0, z: 0 },
            overscaled_z: 0,
        };
        let mut parameters = BucketParameters {
            tile_id: tile_id,
            mode: MapMode::Continuous,
            pixel_ratio: 1.0,
            layer_type: LayerTypeInfo,
        };

        // Build SymbolGeometryTileFeatures from the tile data collected during processing.
        // Pre-populate formatted_text so symbol_layout uses actual names instead of defaults.
        let features: Vec<SymbolGeometryTileFeature> = self
            .collected_features
            .iter()
            .map(|(text, x, y)| {
                let geometry = vec![GeometryCoordinates(vec![Point2D::new(
                    *x as i16, *y as i16,
                )])];
                let mut feature =
                    SymbolGeometryTileFeature::new(Box::new(VectorGeometryTileFeature {
                        geometry,
                    }));
                let mut tagged_string = TaggedString::default();
                tagged_string.add_text_section(
                    &apply_arabic_shaping(&U16String::from(text.as_str())),
                    1.0,
                    font_stack.clone(),
                    None,
                );
                feature.formatted_text = Some(tagged_string);
                feature
            })
            .collect();

        if features.is_empty() {
            return;
        }

        let layer_data = SymbolGeometryTileLayer {
            name: layer_name.clone(),
            features,
        };
        let layer_properties = vec![LayerProperties {
            id: layer_name.clone(),
            layer: SymbolLayer {
                layout: SymbolLayoutProperties_Unevaluated,
            },
        }];

        let image_positions = ImagePositions::new();

        let glyph_map =
            GlyphPositionMap::from_iter(glyphs.glyphs.iter().map(|(unicode_point, glyph)| {
                (
                    *unicode_point as Char16,
                    GlyphPosition {
                        rect: Rect::new(
                            Point2D::new(
                                glyph.tex_origin_x as u16 + 3,
                                glyph.tex_origin_y as u16 + 3,
                            ),
                            Size2D::new(
                                glyph.buffered_dimensions().0 as u16,
                                glyph.buffered_dimensions().1 as u16,
                            ),
                        ), // FIXME: verify if this mapping is correct
                        metrics: GlyphMetrics {
                            width: glyph.width,
                            height: glyph.height,
                            left: glyph.left_bearing,
                            top: glyph.top_bearing,
                            advance: glyph.h_advance,
                        },
                    },
                )
            }));

        let glyph_positions: GlyphPositions =
            GlyphPositions::from([(FontStackHasher::new(&font_stack), glyph_map)]);

        let glyphs: GlyphMap = GlyphMap::from([(
            FontStackHasher::new(&font_stack),
            Glyphs::from_iter(glyphs.glyphs.iter().map(|(unicode_point, glyph)| {
                (
                    *unicode_point as Char16,
                    Some(Glyph {
                        id: *unicode_point as Char16,
                        bitmap: Default::default(),
                        metrics: GlyphMetrics {
                            width: glyph.width,
                            height: glyph.height,
                            left: glyph.left_bearing,
                            top: glyph.top_bearing,
                            advance: glyph.h_advance,
                        },
                    }),
                )
            })),
        )]);

        let mut layout = SymbolLayout::new(
            &parameters,
            &layer_properties,
            Box::new(layer_data),
            &mut LayoutParameters {
                bucket_parameters: &mut parameters.clone(),
                glyph_dependencies: &mut glyph_dependencies,
                image_dependencies: &mut Default::default(),
                available_images: &mut Default::default(),
            },
        )
        .unwrap();

        let empty_image_map = ImageMap::new();
        layout.prepare_symbols(
            &glyphs,
            &glyph_positions,
            &empty_image_map,
            &image_positions,
        );

        let mut output = HashMap::new();
        layout.create_bucket(
            image_positions,
            Box::new(FeatureIndex),
            &mut output,
            false,
            false,
            &tile_id.canonical,
        );

        let new_buffer = output.remove(&layer_name).unwrap();

        let mut buffer = VertexBuffers::new();
        let text_buffer = new_buffer.bucket.text;
        let SymbolBucketBuffer {
            shared_vertices,
            triangles,
            ..
        } = text_buffer;
        buffer.vertices = shared_vertices
            .iter()
            .map(|v| ShaderSymbolVertexNew::new(v))
            .collect();
        buffer.indices = triangles.indices.iter().map(|i| *i as u32).collect();

        self.quad_buffer = buffer;
        // The layout does not attribute quads to features, so each label records only its
        // text and anchor.
        self.features = self
            .collected_features
            .iter()
            .map(|(text, x, y)| {
                let anchor = Point2D::new(*x as f32, *y as f32);
                Feature {
                    bbox: Box2D::new(anchor, anchor),
                    indices: 0..0,
                    text_anchor: anchor,
                    str: text.clone(),
                }
            })
            .collect();
    }
}

impl TextTessellatorNew {
    /// A tessellator labelling features with `text_field` evaluated at the tile's `zoom`.
    pub fn new(text_field: StyleProperty<TextField>, zoom: f64) -> Self {
        Self {
            text_field,
            zoom,
            ..Default::default()
        }
    }

    /// The label of the feature whose properties were collected; empty text draws nothing.
    fn current_text(&self) -> Option<String> {
        self.text_field
            .evaluate_for(&self.feature_properties, self.zoom)
            .map(|text| text.0)
            .filter(|text| !text.is_empty())
    }
}

impl Default for TextTessellatorNew {
    fn default() -> Self {
        Self {
            geo_writer: Default::default(),
            text_field: StyleProperty::Constant(TextField::default()),
            zoom: 0.0,
            quad_buffer: VertexBuffers::new(),
            features: vec![],
            collected_features: vec![],
            current_index: 0,
            feature_properties: FeatureProperties::default(),
            current_origin: None,
            current_point: None,
            coordinate_scale: 1.0,
        }
    }
}

impl GeomProcessor for TextTessellatorNew {
    fn xy(&mut self, x: f64, y: f64, idx: usize) -> GeoResult<()> {
        let (x, y) = (x * self.coordinate_scale, y * self.coordinate_scale);
        self.current_point = Some((x, y));
        self.geo_writer.xy(x, y, idx)
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
    fn property(
        &mut self,
        _idx: usize,
        name: &str,
        value: &ColumnValue,
    ) -> geozero::error::Result<bool> {
        if let Some(value) = property_value(value) {
            self.feature_properties.insert(name.to_string(), value);
        }
        Ok(false)
    }
}

impl FeatureProcessor for TextTessellatorNew {
    fn feature_end(&mut self, _idx: u64) -> geozero::error::Result<()> {
        let geometry = self.geo_writer.take_geometry();

        // Only features with text and a point geometry become labels.
        let text = self.current_text();
        self.feature_properties.clear();
        if let (Some(text), Some((x, y))) = (text, self.current_point.take()) {
            self.collected_features.push((text, x, y));
        }

        match geometry {
            Some(Geometry::Point(_point)) => {}
            Some(Geometry::Polygon(_polygon)) => {}
            Some(Geometry::LineString(_linestring)) => {}
            Some(Geometry::Line(_))
            | Some(Geometry::MultiPoint(_))
            | Some(Geometry::MultiLineString(_))
            | Some(Geometry::MultiPolygon(_))
            | Some(Geometry::GeometryCollection(_))
            | Some(Geometry::Rect(_))
            | Some(Geometry::Triangle(_)) => {
                log::debug!("Unsupported geometry in text tessellation")
            }
            None => {
                log::debug!("No geometry in feature")
            }
        };

        Ok(())
    }
}
