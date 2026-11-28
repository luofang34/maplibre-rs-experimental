//! Physical eye measurements independent of the gaze-derived map center.
use super::*;
use crate::{coords::WorldTileCoords, projection::globe::covering_tiles::lod::LodContext};

impl ViewState {
    /// Zoom used to evaluate styles and convert cartographic pixels into ground distances.
    /// A tracked head can look around without changing the map's level of detail styling.
    pub fn style_zoom(&self) -> crate::coords::Zoom {
        self.external_eye
            .map_or(self.zoom(), |eye| crate::coords::Zoom::new(eye.zoom))
    }

    pub(crate) fn symbol_distance_ratio(&self, clip: Vector4<f64>) -> f64 {
        let Some(eye) = self.external_eye else {
            return 1.0;
        };
        let f = eye.frustum;
        let position = Vector3::new(
            (clip.x * (f.left + f.right) + clip.w * (f.right - f.left)) * 0.5,
            (clip.y * (f.top + f.bottom) + clip.w * (f.top - f.bottom)) * 0.5,
            clip.w,
        );
        let height = self.camera_to_center_distance() * 2_f64.powf(self.zoom().value() - eye.zoom);
        height / position.magnitude().max(1e-6)
    }

    pub(crate) fn external_lod_focal_pixels(&self) -> Option<f64> {
        self.external_eye.map(|eye| eye.lod_focal_pixels)
    }

    pub(crate) fn eye_lod_context(&self, requested_zoom: f64) -> Option<LodContext> {
        let eye = self.external_eye?;
        let position = self.eye_position();
        let world_size = TILE_SIZE * 2_f64.powf(self.zoom().value());
        let height = (position.z - eye.anchor.altitude_meters).max(1.0);
        Some(LodContext::from_eye(
            Point2::new(position.x / world_size, position.y / world_size),
            height * self.anchor_pixels_per_meter(eye.anchor) / world_size,
            eye.lod_focal_pixels * 2_f64.powf(requested_zoom - self.zoom().value()),
        ))
    }

    /// Precomputed per-frame fog origin scalars, hoisted out of per-tile uniform loops.
    #[inline]
    pub(crate) fn eye_fog_params(
        &self,
        resolved_eye: Option<Vector3<f64>>,
    ) -> Option<EyeFogParams> {
        let eye = self.external_eye?;
        let position = resolved_eye.unwrap_or_else(|| self.eye_position());
        let world_size = TILE_SIZE * 2_f64.powf(self.zoom().value());
        let meters_per_world = world_size / self.anchor_pixels_per_meter(eye.anchor);
        Some(EyeFogParams {
            eye_x_world: position.x / world_size,
            eye_y_world: position.y / world_size,
            eye_neg_z: -position.z as f32,
            meters_per_world,
        })
    }

    /// Tile origin relative to the eye, and metres per tile coordinate, in one shared
    /// tangent frame. Adjacent tiles therefore agree on fog distance at their boundary.
    #[cfg(test)]
    pub(crate) fn eye_fog_position(&self, tile: WorldTileCoords) -> Option<[f32; 4]> {
        self.eye_fog_params(None)
            .map(|params| params.for_tile(tile))
    }

    pub(crate) fn eye_fog_meters_per_pixel(&self) -> Option<f64> {
        self.external_eye
            .map(|eye| 1.0 / self.anchor_pixels_per_meter(eye.anchor))
    }
}

/// Frame-invariant tangent-frame fog parameters evaluated once per view.
#[derive(Clone, Copy, Debug)]
pub(crate) struct EyeFogParams {
    eye_x_world: f64,
    eye_y_world: f64,
    eye_neg_z: f32,
    meters_per_world: f64,
}

impl EyeFogParams {
    #[inline]
    pub(crate) fn for_tile(self, tile: WorldTileCoords) -> [f32; 4] {
        let count = 2_f64.powi(i32::from(u8::from(tile.z)));
        [
            ((f64::from(tile.x) / count - self.eye_x_world) * self.meters_per_world) as f32,
            ((self.eye_y_world - f64::from(tile.y) / count) * self.meters_per_world) as f32,
            self.eye_neg_z,
            (self.meters_per_world / count / crate::coords::EXTENT) as f32,
        ]
    }
}
