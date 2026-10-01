#![allow(clippy::expect_used, clippy::panic)]

use super::layer_translate_tile_units;
use crate::{
    coords::ZoomLevel,
    style::layer::{FillPaint, LayerPaint, TranslateAnchor},
};

#[test]
fn viewport_translation_rotates_with_map_bearing() {
    let paint = LayerPaint::Fill(FillPaint {
        fill_translate: Some([10.0, 50.0].into()),
        fill_translate_anchor: TranslateAnchor::Viewport,
        ..FillPaint::default()
    });
    let translation =
        layer_translate_tile_units(Some(&paint), ZoomLevel::new(1), 1.0, 45.0_f32.to_radians());

    assert!((translation[0] + 226.274_17).abs() < 1e-4);
    assert!((translation[1] - 339.411_25).abs() < 1e-4);
}

#[test]
fn map_translation_scales_pixels_for_parent_tile() {
    let paint = LayerPaint::Fill(FillPaint {
        fill_translate: Some([10.0, 50.0].into()),
        ..FillPaint::default()
    });
    let translation = layer_translate_tile_units(Some(&paint), ZoomLevel::new(0), 2.0, 0.0);

    assert_eq!(translation, [20.0, 100.0]);
}

#[cfg(feature = "headless")]
#[tokio::test]
async fn empty_tiles_do_not_starve_later_geometry_uploads() {
    use crate::{
        coords::WorldTileCoords,
        headless::create_headless_renderer,
        render::{memory_budget::UPLOADS_PER_FRAME, ShaderVertex},
        style::Style,
        tcs::tiles::Tiles,
        vector::{
            AvailableVectorLayerBucket, VectorBufferPool, VectorLayerBucket,
            VectorLayerBucketComponent,
        },
    };
    let (_, renderer) = create_headless_renderer(64, 64, None)
        .await
        .expect("renderer");
    let mut pool = VectorBufferPool::from_device(&renderer.device);
    let mut tiles = Tiles::default();
    let style: Style = serde_json::from_str(r##"{"version":8,"sources":{},"layers":[{"id":"land","type":"fill","paint":{"fill-color":"#718474"}}]}"##).expect("style");
    let coords: Vec<_> = (0..=UPLOADS_PER_FRAME)
        .map(|x| WorldTileCoords {
            x: x as i32,
            y: 0,
            z: ZoomLevel::new(8),
        })
        .collect();
    for (index, coords) in coords.iter().enumerate() {
        let nonempty = index == UPLOADS_PER_FRAME;
        let buffer = lyon::tessellation::VertexBuffers {
            vertices: if nonempty {
                vec![ShaderVertex::default(); 3]
            } else {
                Vec::new()
            },
            indices: if nonempty { vec![0, 1, 2] } else { Vec::new() },
        };
        tiles
            .spawn_mut(*coords)
            .expect("tile")
            .insert(VectorLayerBucketComponent {
                failed: false,
                done: true,
                overscaled_zoom: 0,
                layers: vec![VectorLayerBucket::AvailableLayer(
                    AvailableVectorLayerBucket {
                        coords: *coords,
                        source_layer: "land".into(),
                        style_layer_id: "land".into(),
                        buffer: buffer.into(),
                        feature_indices: if nonempty { vec![3] } else { Vec::new() },
                        feature_colors: Vec::new(),
                        feature_sort_keys: Vec::new(),
                    },
                )],
            });
    }
    let wanted = *coords.last().expect("geometry tile");
    super::upload_tessellated_layer(
        &mut pool,
        &renderer.queue,
        &mut tiles,
        &style,
        coords,
        &[],
        super::VectorPaintFrame {
            zoom: 8.0,
            bearing: 0.0,
            light: Default::default(),
        },
    );
    assert!(
        pool.get_loaded_style_layers_at(wanted)
            .is_some_and(|layers| layers.contains("land")),
        "empty buckets must leave upload slots for later geometry"
    );
}

#[test]
fn a_heatmap_layer_carries_radius_and_intensity_at_the_view_zoom() {
    let layer: crate::style::layer::StyleLayer = serde_json::from_value(serde_json::json!({
        "id": "heat", "type": "heatmap", "source": "points",
        "paint": {
            "heatmap-radius": ["interpolate", ["linear"], ["zoom"], 0, 10, 10, 30],
            "heatmap-intensity": ["interpolate", ["linear"], ["zoom"], 0, 1, 10, 3]
        }
    }))
    .expect("layer");
    let metadata = super::metadata_for_layer(
        &layer,
        Default::default(),
        super::VectorPaintFrame {
            zoom: 5.0,
            bearing: 0.0,
            light: Default::default(),
        },
    );
    assert_eq!(metadata.line_width, 20.0);
    assert_eq!(metadata.circle_params[0], 2.0);
}

#[test]
fn a_line_layer_carries_offset_gap_width_and_blur_at_the_view_zoom() {
    let layer: crate::style::layer::StyleLayer = serde_json::from_value(serde_json::json!({
        "id": "road", "type": "line", "source": "s",
        "paint": {
            "line-offset": ["interpolate", ["linear"], ["zoom"], 0, 0, 10, 20],
            "line-gap-width": 3,
            "line-blur": -1
        }
    }))
    .expect("layer");
    let metadata = super::metadata_for_layer(
        &layer,
        Default::default(),
        super::VectorPaintFrame {
            zoom: 5.0,
            bearing: 0.0,
            light: Default::default(),
        },
    );
    // A negative blur is clamped: the edge cannot be sharper than the antialiasing.
    assert_eq!(metadata.circle_params, [10.0, 3.0, 0.0, 0.0]);
}

#[test]
fn features_sharing_a_pattern_form_one_run_of_indices() {
    use crate::style::pattern_key::pattern_value;

    let colors = [
        [pattern_value(Some("a")), 0.0, 0.0, 1.0],
        [pattern_value(Some("a")), 0.0, 0.0, 1.0],
        [pattern_value(Some("b")), 0.0, 0.0, 1.0],
        [pattern_value(None), 0.0, 0.0, 1.0],
    ];
    // Four features of three vertices each, one triangle apiece.
    let indices = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11];
    let runs = super::pattern_runs(&[3, 3, 3, 3], &colors, &indices);
    assert_eq!(runs.len(), 2);
    assert_eq!(runs[0].1, 0..6);
    assert_eq!(runs[1].1, 6..9);
}

#[test]
fn features_with_one_sort_key_draw_as_a_single_run() {
    // Three features of three indices each, the first two sharing a key.
    let indices: Vec<u32> = vec![0, 1, 2, 3, 4, 5, 6, 7, 8];
    let runs = super::sort_runs(&[3, 3, 3], &[1.0, 1.0, 2.0], &indices);
    assert_eq!(runs, vec![(1.0, 0..6), (2.0, 6..9)]);
}

#[test]
fn a_feature_without_indices_makes_no_run() {
    let runs = super::sort_runs(&[3, 0, 3], &[1.0, 1.0, 1.0], &[0, 1, 2, 3, 4, 5]);
    assert_eq!(runs, vec![(1.0, 0..6)]);
}
