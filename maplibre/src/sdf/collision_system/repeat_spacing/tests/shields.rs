//! Road shields laid out by the tile worker, then spaced on screen: the shield's image is part
//! of what makes two shields repeats of one route.
use std::sync::Arc;

use geozero::{ColumnValue, FeatureProcessor, GeomProcessor, PropertyProcessor};

use super::{layer, place};
use crate::sdf::{
    assets::{AtlasEntry, SymbolAtlas},
    tessellation::TextTessellator,
    Feature,
};

fn atlas() -> Arc<SymbolAtlas> {
    let icon = AtlasEntry {
        rect: [0, 0, 20, 20],
        metrics: [0., 0., 20., 1.],
        kind: 1,
        ..Default::default()
    };
    Arc::new(SymbolAtlas {
        glyphs: [(
            "test".into(),
            [(
                'A' as u32,
                AtlasEntry {
                    rect: [0, 0, 10, 18],
                    metrics: [0., -9., 10., 1.],
                    kind: 0,
                    ..Default::default()
                },
            )]
            .into(),
        )]
        .into(),
        icons: [
            ("us-interstate".into(), icon.clone()),
            ("us-highway".into(), icon),
        ]
        .into(),
        ..Default::default()
    })
}

/// The shields a road of `network` numbered "AA" shows along a line across its tile.
fn shields(network: &str) -> Vec<Feature> {
    let paint = serde_json::from_value(serde_json::json!({
        "text-field": ["get", "ref"], "text-font": ["test"], "text-size": 16,
        "symbol-placement": "line", "icon-image": ["get", "network"],
        "text-rotation-alignment": "viewport", "icon-rotation-alignment": "viewport"
    }))
    .expect("paint");
    let mut tessellator = TextTessellator::default();
    tessellator.configure(paint, atlas());
    tessellator.linestring_begin(true, 2, 0).expect("begin");
    tessellator.xy(0.0, 2048.0, 0).expect("vertex");
    tessellator.xy(4096.0, 2048.0, 1).expect("vertex");
    tessellator.linestring_end(true, 0).expect("end");
    tessellator
        .property(0, "ref", &ColumnValue::String("AA"))
        .expect("ref");
    tessellator
        .property(1, "network", &ColumnValue::String(network))
        .expect("network");
    tessellator.feature_end(0).expect("feature");
    tessellator.finish();
    std::mem::take(&mut tessellator.features)
}

#[test]
fn the_same_number_on_different_shields_is_placed_side_by_side() {
    let tile = layer("road_shield", 0);
    let interstate = shields("us-interstate");
    let highway = shields("us-highway");
    assert!(!interstate.is_empty() && !highway.is_empty());
    let mut spacing = super::RepeatSpacing::default();
    spacing.begin();
    let placed = place(
        &mut spacing,
        &[
            (&tile, &interstate[0], [400.0, 300.0], false),
            (&tile, &highway[0], [440.0, 300.0], false),
        ],
    );
    assert_eq!(placed, [true, true]);
}

#[test]
fn the_same_number_on_the_same_shield_too_close_is_placed_once() {
    let (left, right) = (layer("road_shield", 0), layer("road_shield", 1));
    let first = shields("us-interstate");
    let second = shields("us-interstate");
    let mut spacing = super::RepeatSpacing::default();
    spacing.begin();
    let placed = place(
        &mut spacing,
        &[
            (&left, &first[0], [400.0, 300.0], false),
            (&right, &second[0], [440.0, 300.0], false),
        ],
    );
    assert_eq!(placed, [true, false]);
}
