//! Transforms that place a source tile's geometry inside a drape target's texture.

use cgmath::{Matrix4, SquareMatrix, Vector3};

use crate::coords::{WorldTileCoords, EXTENT};

/// Homogeneous scale keeping layer z-indices well inside the clip depth range when `w` is one.
const CLIP_SCALE: f64 = 65536.0;

/// Maps tile-local coordinates of `source` to clip space of the drape texture of `target`.
///
/// `source` must be `target` itself, an ancestor, or a descendant; the texture covers exactly
/// `target`, with tile row zero at the top of the texture.
pub fn drape_transform(target: WorldTileCoords, source: WorldTileCoords) -> Option<Matrix4<f64>> {
    let source_to_target = source_to_target(target, source)?;
    #[rustfmt::skip]
    let ortho = Matrix4::new(
        2.0 / EXTENT, 0.0, 0.0, 0.0,
        0.0, -2.0 / EXTENT, 0.0, 0.0,
        0.0, 0.0, 1.0, 0.0,
        -1.0, 1.0, 0.0, 1.0,
    );
    #[rustfmt::skip]
    let clip_scale = Matrix4::new(
        CLIP_SCALE, 0.0, 0.0, 0.0,
        0.0, CLIP_SCALE, 0.0, 0.0,
        0.0, 0.0, CLIP_SCALE, 0.0,
        0.0, 0.0, 0.0, CLIP_SCALE,
    );
    Some(clip_scale * ortho * source_to_target)
}

/// Maps `source` tile-local coordinates in `0..EXTENT` to `target` tile-local coordinates.
fn source_to_target(target: WorldTileCoords, source: WorldTileCoords) -> Option<Matrix4<f64>> {
    let target_zoom = i32::from(u8::from(target.z));
    let source_zoom = i32::from(u8::from(source.z));
    let delta = target_zoom - source_zoom;
    if delta == 0 {
        return (target.x == source.x && target.y == source.y).then(Matrix4::identity);
    }
    if delta > 0 {
        // The target is a descendant: cut its square out of the source and blow it up.
        let scale = 2_f64.powi(delta);
        if target.x >> delta != source.x || target.y >> delta != source.y {
            return None;
        }
        let origin_x = f64::from(target.x - (source.x << delta)) * EXTENT / scale;
        let origin_y = f64::from(target.y - (source.y << delta)) * EXTENT / scale;
        return Some(
            Matrix4::from_scale(scale)
                * Matrix4::from_translation(Vector3::new(-origin_x, -origin_y, 0.0)),
        );
    }
    // The source is a descendant: shrink it into its quadrant of the target.
    let delta = -delta;
    let scale = 2_f64.powi(delta);
    if source.x >> delta != target.x || source.y >> delta != target.y {
        return None;
    }
    let origin_x = f64::from(source.x - (target.x << delta)) * EXTENT / scale;
    let origin_y = f64::from(source.y - (target.y << delta)) * EXTENT / scale;
    Some(
        Matrix4::from_translation(Vector3::new(origin_x, origin_y, 0.0))
            * Matrix4::from_scale(1.0 / scale),
    )
}

#[cfg(test)]
mod tests;
