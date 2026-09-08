#![allow(clippy::expect_used, clippy::panic)]

use super::layer_translate_tile_units;
use crate::{
    coords::ZoomLevel,
    style::layer::{FillPaint, LayerPaint, TranslateAnchor},
};

#[test]
fn viewport_translation_rotates_with_map_bearing() {
    let paint = LayerPaint::Fill(FillPaint {
        fill_translate: Some([10.0, 50.0]),
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
        fill_translate: Some([10.0, 50.0]),
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
                done: true,
                layers: vec![VectorLayerBucket::AvailableLayer(
                    AvailableVectorLayerBucket {
                        coords: *coords,
                        source_layer: "land".into(),
                        style_layer_id: "land".into(),
                        buffer: buffer.into(),
                        feature_indices: if nonempty { vec![3] } else { Vec::new() },
                        feature_colors: Vec::new(),
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
        8.0,
        0.0,
    );
    assert!(
        pool.get_loaded_style_layers_at(wanted)
            .is_some_and(|layers| layers.contains("land")),
        "empty buckets must leave upload slots for later geometry"
    );
}
