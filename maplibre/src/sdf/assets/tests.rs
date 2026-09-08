#![allow(clippy::expect_used, clippy::panic)]
use super::*;
use std::collections::BTreeSet;

#[test]
fn glyph_subset_keeps_spaces_and_unicode_metrics_without_uploading_unused_letters() {
    let mut builder = AtlasBuilder::new();
    builder
        .glyph_subset(
            "test",
            include_bytes!("../../../../data/0-255.pbf"),
            Some(&BTreeSet::from([32, 65, 233])),
        )
        .expect("font");
    let atlas = builder.finish();
    let font = &atlas.glyphs["test"];
    assert_eq!(font.len(), 3);
    assert!(font[&32].metrics[2] > 0.0, "spaces must advance the pen");
    assert!(font[&233].rect[2] > 0, "accented glyph must have pixels");
    assert_eq!(
        font[&65].metrics[1], -6.0,
        "PBF metrics use the map font baseline"
    );
}

#[test]
fn packing_a_short_sprite_does_not_erase_a_tall_glyph_and_borders_stay_clear() {
    let mut builder = AtlasBuilder::new();
    let tall = builder
        .pack(8, 80, &[255, 0, 0, 255].repeat(640))
        .expect("tall glyph");
    let short = builder
        .pack(8, 8, &[0, 255, 0, 128].repeat(64))
        .expect("short icon");
    assert!(builder.pack(u32::MAX, 1, &[]).is_none());
    let atlas = builder.finish();
    let pixel = |x, y| {
        &atlas.pixels
            [((y * atlas.size[0] + x) * 4) as usize..((y * atlas.size[0] + x) * 4 + 4) as usize]
    };
    assert_eq!(pixel(tall[0], tall[1] + 79), [255, 0, 0, 255]);
    assert_eq!(pixel(short[0], short[1]), [0, 255, 0, 128]);
    assert_eq!(pixel(short[0] - 1, short[1]), [0, 0, 0, 0]);
}

#[test]
fn worker_wire_roundtrip_preserves_exact_collision_ranges_and_anchor() {
    let feature = crate::sdf::Feature {
        data: crate::sdf::SymbolFeatureData {
            id: Some(42),
            properties: [(
                "name".into(),
                crate::style::expression::Value::String("Zürich".into()),
            )]
            .into(),
            sort_key: 3.5,
        },
        parts: [
            Some(crate::sdf::placement_geometry::SymbolBounds {
                bounds: [-12.0, -8.0, 34.0, 16.0],
                height: 24.0,
                angle: 0.2,
                text: true,
            }),
            None,
            None,
        ],
        bbox: crate::euclid::Box2D::new(
            crate::euclid::Point2D::new(-12., -8.),
            crate::euclid::Point2D::new(34., 16.),
        ),
        indices: 6..30,
        text_anchor: crate::euclid::Point2D::new(123., 456.),
        str: "Zürich".into(),
    };
    let bytes = serde_json::to_vec(&wire::SymbolFeature::from(&feature)).expect("serialize");
    let decoded: wire::SymbolFeature = serde_json::from_slice(&bytes).expect("deserialize");
    let decoded = crate::sdf::Feature::from(decoded);
    assert_eq!(decoded.indices, feature.indices);
    assert_eq!(decoded.bbox, feature.bbox);
    assert_eq!(decoded.text_anchor, feature.text_anchor);
    assert_eq!(decoded.str, feature.str);
    assert_eq!(decoded.data.id, feature.data.id);
    assert_eq!(decoded.data.properties, feature.data.properties);
    assert_eq!(decoded.data.sort_key, feature.data.sort_key);
    assert_eq!(
        decoded.parts[0].as_ref().expect("bounds").bounds,
        [-12.0, -8.0, 34.0, 16.0]
    );
}
