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
            source_feature_index: None,
        },
        bbox: Box2D::zero(),
        indices,
        text_anchor: Point2D::new(at[0], at[1]),
        anchor_shifts: Vec::new(),
        text_sets: Vec::new(),
        anchor_sets: Vec::new(),
        text_colors: Vec::new(),
        fallback: false,
        repeat_key: None,
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

#[test]
fn quantized_bearing_buckets_absorb_sub_degree_jitter_and_preserve_cardinal_angles() {
    assert_eq!(bearing_bucket(0.0), 0);
    assert_eq!(bearing_bucket(0.5_f64.to_radians()), 0);
    assert_eq!(bearing_bucket(-0.5_f64.to_radians()), 0);
    assert_eq!(bearing_bucket(std::f64::consts::FRAC_PI_2), 24);
    assert!(
        (bucket_angle_rad(24) - std::f64::consts::FRAC_PI_2).abs() < 1e-12,
        "90 degrees maps exactly to bucket 24"
    );
    assert_ne!(bearing_bucket(0.0), bearing_bucket(4.0_f64.to_radians()));
}

#[cfg(all(feature = "headless", feature = "thread-safe-futures"))]
#[tokio::test]
async fn budgeted_follow_bearing_caps_buckets_per_frame_until_complete() {
    use std::time::Duration;

    use crate::{
        coords::ZoomLevel,
        headless::create_headless_renderer,
        render::{
            eventually::Eventually, settings::BufferPoolSizes, shaders::SDFShaderFeatureMetadata,
        },
        sdf::SymbolBufferPool,
        style::layer::{LayerPaint, StyleLayer},
        tcs::world::World,
    };

    let (_, renderer) = create_headless_renderer(64, 64, None)
        .await
        .expect("renderer");
    let mut pool = SymbolBufferPool::from_device_with_sizes(
        &renderer.device,
        BufferPoolSizes {
            vertices: 64,
            indices: 128,
            feature_metadata: 64,
            layer_metadata: 16,
        },
    );
    let style_layer: StyleLayer = serde_json::from_value(serde_json::json!({
        "id": "labels",
        "type": "symbol",
        "source": "openmaptiles",
        "source-layer": "places",
        "layout": {
            "text-field": "A",
            "symbol-z-order": "viewport-y",
            "text-allow-overlap": true
        }
    }))
    .expect("style layer");
    assert!(
        matches!(&style_layer.paint, Some(LayerPaint::Symbol(_))),
        "symbol paint"
    );
    let Some(LayerPaint::Symbol(paint)) = &style_layer.paint else {
        return;
    };

    let buckets: Vec<SymbolLayerData> = (0..5)
        .map(|x| {
            let coords = WorldTileCoords {
                x,
                y: 0,
                z: ZoomLevel::new(10),
            };
            let buffer = OverAlignedVertexBuffer::<ShaderSymbolVertex, u32>::from_iters(
                vec![ShaderSymbolVertex::default(); 6],
                vec![0, 1, 2, 3, 4, 5],
                6,
            );
            pool.allocate_layer_geometry(
                &renderer.queue,
                coords,
                style_layer.clone(),
                &buffer,
                crate::render::shaders::EmptyLayerMetadata([0; 0]),
                &[SDFShaderFeatureMetadata::default(); 6],
            )
            .expect("allocate");
            SymbolLayerData {
                atlas: None,
                coords,
                source_layer: "places".into(),
                style_layer_id: "labels".into(),
                buffer,
                features: vec![label([0.0, 100.0], 0..3), label([0.0, 0.0], 3..6)],
            }
        })
        .collect();

    let mut world = World::default();
    world.resources.insert(Eventually::Initialized(pool));
    let layers: Vec<(u32, &SymbolLayerData, &SymbolPaint)> =
        buckets.iter().map(|b| (0, b, paint)).collect();
    let target = (std::f64::consts::FRAC_PI_2, 14.0);
    let mut sort = DrawSort::default();

    let budget = Some((Duration::from_secs(1), 2));
    assert!(sort.needs_bearing_sort(&world, target, &layers));
    assert!(sort.follow_bearing(&world, &renderer.queue, target, &layers, budget));
    assert!(sort.needs_bearing_sort(&world, target, &layers));
    assert!(sort.follow_bearing(&world, &renderer.queue, target, &layers, budget));
    assert!(sort.needs_bearing_sort(&world, target, &layers));
    assert!(!sort.follow_bearing(&world, &renderer.queue, target, &layers, budget));
    assert!(!sort.needs_bearing_sort(&world, target, &layers));
}
