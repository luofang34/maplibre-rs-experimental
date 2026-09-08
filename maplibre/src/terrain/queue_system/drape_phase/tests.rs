#![allow(clippy::expect_used, clippy::panic)]
use super::*;
use crate::{
    coords::{WorldTileCoords, ZoomLevel},
    terrain::drape_targets::ShapeSpec,
};

#[test]
fn draped_road_width_has_the_same_world_size_at_every_target_lod() {
    let view_zoom = Zoom::new(14.0);
    for target_zoom in [8, 11, 14] {
        let coords = WorldTileCoords {
            x: 0,
            y: 0,
            z: ZoomLevel::from(target_zoom),
        };
        let spec = TargetSpec {
            coords,
            shapes: vec![ShapeSpec {
                source: coords,
                vector_layers: vec![],
                raster_layers: vec![],
            }],
        };
        let (metadata, _) = drape_metadata(&[spec], &[true], 4, view_zoom);
        let width = 6.0 * metadata[0].line_width_scale as f64;
        let world_width = width / f64::from(DRAPE_SIZE) / 2_f64.powi(i32::from(target_zoom));
        let expected = 6.0 / TILE_SIZE / 2_f64.powf(view_zoom.value());
        assert!((world_width - expected).abs() < 1e-12);
    }
}
