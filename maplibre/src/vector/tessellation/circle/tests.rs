use geozero::{FeatureProcessor, GeomProcessor, PropertyProcessor};

use super::CircleOptions;
use crate::{
    style::layer::StyleProperty,
    vector::tessellation::{IndexDataType, ZeroTessellator},
};

#[test]
fn a_stroke_opacity_by_feature_rides_the_corner_code_and_leaves_the_ground_slot_free() {
    let mut tessellator = ZeroTessellator::<IndexDataType>::default().with_circles(CircleOptions {
        radius: StyleProperty::Constant(4.0),
        radius_default: 5.0,
        stroke_width: StyleProperty::Constant(1.0),
        stroke_width_default: 0.0,
        blur: None,
        stroke_opacity: Some(StyleProperty::parse(&serde_json::json!(["get", "edge"]))),
        stroke_color: None,
        zoom: 3.0,
        grid: true,
    });
    for (index, edge) in [(0, Some(0.5)), (1, None)] {
        if let Some(edge) = edge {
            tessellator
                .property(0, "edge", &geozero::ColumnValue::Double(edge))
                .expect("property");
        }
        tessellator.point_begin(0).expect("point begins");
        tessellator.xy(100.0, 200.0, 0).expect("point coordinate");
        tessellator.point_end(0).expect("point ends");
        tessellator.feature_end(index).expect("feature ends");
    }

    let vertices = &tessellator.buffer.vertices;
    assert!(
        vertices.iter().all(|vertex| vertex.elevation < -1e20),
        "the ground slot keeps the off-terrain sentinel until terrain fills it"
    );
    assert_eq!(vertices[15].corner_code, 1.0 + 7.0 * 8.0 + 7.0 + 0.5);
    assert_eq!(
        vertices[16].corner_code, 1.0,
        "a missing value leaves the layer's"
    );
}

/// The sign marks packed lanes, so a tile built under one paint draws right under the next:
/// a plain colour lane is never negative, a packed one always is, and each holds its channels.
#[test]
fn packed_circle_colours_are_negative_and_keep_both_colours() {
    let packed = super::pack_circle_colors([1.0, 0.5, 0.0, 1.0], [0.0, 0.0, 0.0, 0.0]);
    assert!(packed.iter().all(|lane| *lane < 0.0), "{packed:?}");
    let unpack = |lane: f32| {
        let pair = -lane - 1.0;
        let high = (pair / 256.0).floor();
        [high, pair - high * 256.0]
    };
    assert_eq!(unpack(packed[0]), [255.0, 128.0]);
    assert_eq!(unpack(packed[1]), [0.0, 255.0]);
    assert_eq!(unpack(packed[3]), [0.0, 0.0]);
}
