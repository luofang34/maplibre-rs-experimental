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

    /// Tile origin relative to the eye, and metres per tile coordinate, in one shared
    /// tangent frame. Adjacent tiles therefore agree on fog distance at their boundary.
    pub(crate) fn eye_fog_position(&self, tile: WorldTileCoords) -> Option<[f32; 4]> {
        let eye = self.external_eye?;
        let position = self.eye_position();
        let world_size = TILE_SIZE * 2_f64.powf(self.zoom().value());
        let meters_per_world = world_size / self.anchor_pixels_per_meter(eye.anchor);
        let count = 2_f64.powi(i32::from(u8::from(tile.z)));
        Some([
            ((f64::from(tile.x) / count - position.x / world_size) * meters_per_world) as f32,
            ((position.y / world_size - f64::from(tile.y) / count) * meters_per_world) as f32,
            -position.z as f32,
            (meters_per_world / count / crate::coords::EXTENT) as f32,
        ])
    }

    pub(crate) fn eye_fog_meters_per_pixel(&self) -> Option<f64> {
        self.external_eye
            .map(|eye| 1.0 / self.anchor_pixels_per_meter(eye.anchor))
    }
}
