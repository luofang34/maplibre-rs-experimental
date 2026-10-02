use super::*;
use crate::{
    euclid::{Box2D, Point2D},
    sdf::{Feature, ShaderSymbolVertex, SymbolFeatureData},
    vector::tessellation::OverAlignedVertexBuffer,
};

fn label(at: [f32; 2], indices: std::ops::Range<usize>) -> Feature {
    Feature {
        parts: [None, None, None],
        data: SymbolFeatureData {
            id: None,
            properties: Default::default(),
            sort_key: 0.0,
            geometry_type: crate::style::filter::GeometryType::Point,
        },
        bbox: Box2D::zero(),
        indices,
        text_anchor: Point2D::new(at[0], at[1]),
        anchor_shifts: Vec::new(),
        text_sets: Vec::new(),
        anchor_sets: Vec::new(),
        text_colors: Vec::new(),
        fallback: false,
        str: String::new(),
        line: None,
    }
}

#[test]
fn a_turned_map_draws_its_labels_by_their_rotated_height() {
    // Three labels of three indices each: low left, high left, low right.
    let layer = SymbolLayerData {
        atlas: None,
        coords: WorldTileCoords::default(),
        source_layer: "places".into(),
        style_layer_id: "labels".into(),
        buffer: OverAlignedVertexBuffer::<ShaderSymbolVertex, u32>::from_iters(
            Vec::new(),
            (0..9).collect::<Vec<u32>>(),
            0,
        ),
        features: vec![
            label([0.0, 100.0], 0..3),
            label([0.0, 0.0], 3..6),
            label([100.0, 100.0], 6..9),
        ],
    };
    let unturned = indices_in_order(&layer, 0.0).expect("indices");
    assert_eq!(
        unturned,
        [3, 4, 5, 6, 7, 8, 0, 1, 2],
        "the highest first, the later of two level labels before the earlier"
    );
    let turned = indices_in_order(&layer, std::f64::consts::FRAC_PI_2).expect("indices");
    assert_eq!(
        turned,
        [6, 7, 8, 3, 4, 5, 0, 1, 2],
        "a quarter turn draws by the tile's x axis, right to left"
    );
}
