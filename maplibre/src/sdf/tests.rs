#![allow(clippy::expect_used, clippy::panic)]

use std::collections::HashMap;

use crate::{
    euclid::{Point2D, Rect, Size2D},
    legacy::{
        bidi::Char16,
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
        CanonicalTileID, MapMode, OverscaledTileID,
    },
};

#[test]
fn test() {
    let fontStack = vec![
        "Open Sans Regular".to_string(),
        "Arial Unicode MS Regular".to_string(),
    ];

    let mut glyphDependencies = GlyphDependencies::new();

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
    let mut layout = SymbolLayout::new(
        &parameters,
        &vec![LayerProperties {
            id: "layer".to_string(),
            layer: SymbolLayer {
                layout: SymbolLayoutProperties_Unevaluated,
            },
        }],
        Box::new(SymbolGeometryTileLayer {
            name: "layer".to_string(),
            features: vec![SymbolGeometryTileFeature::new(Box::new(
                VectorGeometryTileFeature {
                    geometry: vec![GeometryCoordinates(vec![Point2D::new(1024, 1024)])],
                },
            ))],
        }),
        &mut LayoutParameters {
            bucket_parameters: &mut parameters.clone(),
            glyph_dependencies: &mut glyphDependencies,
            image_dependencies: &mut Default::default(),
            available_images: &mut Default::default(),
        },
    )
    .expect("symbol layout");

    assert_eq!(glyphDependencies.len(), 1);

    // Now we prepare the data, when we have the glyphs available

    let image_positions = ImagePositions::new();

    let glyphPosition = glyph_position();
    let glyphPositions: GlyphPositions = GlyphPositions::from([(
        FontStackHasher::new(&fontStack),
        GlyphPositionMap::from([('中' as Char16, glyphPosition)]),
    )]);

    let mut glyph = Glyph::default();
    glyph.id = '中' as Char16;
    glyph.metrics = glyphPosition.metrics;

    let glyphs: GlyphMap = GlyphMap::from([(
        FontStackHasher::new(&fontStack),
        Glyphs::from([('中' as Char16, Some(glyph))]),
    )]);

    let empty_image_map = ImageMap::new();
    layout.prepare_symbols(&glyphs, &glyphPositions, &empty_image_map, &image_positions);

    let mut output = HashMap::new();
    layout.create_bucket(
        image_positions,
        Box::new(FeatureIndex),
        &mut output,
        false,
        false,
        &tile_id.canonical,
    );

    tracing::debug!(?output, "symbol fixture output")
}

fn glyph_position() -> GlyphPosition {
    GlyphPosition {
        rect: Rect::new(Point2D::new(0, 0), Size2D::new(10, 10)),
        metrics: GlyphMetrics {
            width: 18,
            height: 18,
            left: 2,
            top: -8,
            advance: 21,
        },
    }
}
