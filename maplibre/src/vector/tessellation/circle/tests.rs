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
