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
    let radius = view.body().radius_meters;
    let Some((basis, origin)) = compute_basis(tile, radius) else {
        block.globe_origin = [0.0; 4];
        return None;
    };
    apply_basis_or_clear(block, globe_camera, Some(&basis), origin)
}

/// Computes tile-local tangent basis and globe origin uniform for `tile` (`z >= 8`).
pub(in crate::terrain::queue_system) fn compute_basis(
    tile: WorldTileCoords,
    radius: f64,
) -> Option<(Matrix4<f64>, [f32; 4])> {
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
    let east = Vector3::new(cl, 0.0, -sl);
    let north = Vector3::new(-sl * s, c, -cl * s);
    let up = Vector3::new(sl * c, s, cl * c);
    let basis = Matrix4::from_cols(
        (east / radius).extend(0.0),
        (north / radius).extend(0.0),
        (up / radius).extend(0.0),
        up.extend(1.0),
    );
    let origin = [
        s as f32,
        c as f32,
        (std::f64::consts::TAU / (n * EXTENT)) as f32,
        radius as f32,
    ];
    Some((basis, origin))
}

/// Applies precomputed globe tangent basis into `block`, clearing `globe_origin` on fallback.
pub(super) fn apply_basis_or_clear(
    block: &mut TerrainTileUniforms,
    camera: Option<&GlobeCameraState>,
    basis: Option<&Matrix4<f64>>,
    origin: [f32; 4],
) -> Option<Matrix4<f64>> {
    if let (Some(camera), Some(basis)) = (camera, basis)
        && let Some(matrix) = (camera.wgpu_view_projection() * basis).cast::<f32>()
    {
        block.globe_transform = matrix.into();
        block.globe_origin = origin;
        return Some(*basis);
    }
    block.globe_origin = [0.0; 4];
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
