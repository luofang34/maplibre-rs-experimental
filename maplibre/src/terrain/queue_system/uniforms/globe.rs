//! Tile-relative sphere coordinates preserve sub-metre imagery at low altitude.

use cgmath::{Matrix4, Vector3};

use crate::{
    coords::{WorldTileCoords, EXTENT},
    projection::globe::camera::GlobeCameraState,
    render::view_state::ViewState,
    terrain::resources::TerrainTileUniforms,
};

pub(super) fn set_uniforms(
    block: &mut TerrainTileUniforms,
    tile: WorldTileCoords,
    view: &ViewState,
    globe_camera: Option<&GlobeCameraState>,
) -> Option<Matrix4<f64>> {
    block.globe_origin = [0.0; 4];
    let camera = globe_camera?;
    // Coarse tiles and polar fans use the global sphere path. Their local angular span is large.
    if u8::from(tile.z) < 8 {
        return None;
    }
    let n = 2_f64.powi(i32::from(u8::from(tile.z)));
    let longitude = ((f64::from(tile.x) + 0.5) / n * 2.0 - 1.0) * std::f64::consts::PI;
    let latitude = (std::f64::consts::PI * (1.0 - 2.0 * (f64::from(tile.y) + 0.5) / n))
        .sinh()
        .atan();
    let (s, c) = latitude.sin_cos();
    let (sl, cl) = longitude.sin_cos();
    let radius = view.body().radius_meters;
    let east = Vector3::new(cl, 0.0, -sl);
    let north = Vector3::new(-sl * s, c, -cl * s);
    let up = Vector3::new(sl * c, s, cl * c);
    let basis = Matrix4::from_cols(
        (east / radius).extend(0.0),
        (north / radius).extend(0.0),
        (up / radius).extend(0.0),
        up.extend(1.0),
    );
    if let Some(matrix) = (camera.wgpu_view_projection() * basis).cast::<f32>() {
        block.globe_transform = matrix.into();
        block.globe_origin = [
            s as f32,
            c as f32,
            (std::f64::consts::TAU / (n * EXTENT)) as f32,
            radius as f32,
        ];
        return Some(basis);
    }
    None
}

/// Re-projects a tile's precomputed globe tangent basis for a stereo sibling eye without
/// re-evaluating `sinh`/`atan`/`sin_cos` trigonometry.
pub(super) fn apply_basis(
    block: &mut TerrainTileUniforms,
    camera: Option<&GlobeCameraState>,
    basis: Option<&Matrix4<f64>>,
) {
    let (Some(camera), Some(basis)) = (camera, basis) else {
        return;
    };
    if let Some(matrix) = (camera.wgpu_view_projection() * basis).cast::<f32>() {
        block.globe_transform = matrix.into();
    }
}
