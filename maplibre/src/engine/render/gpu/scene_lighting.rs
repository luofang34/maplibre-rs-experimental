//! Neutral scene lighting representation for terrain and background shaders.

use cgmath::Vector3;

/// Neutral scene lighting uniform inputs for globe, terrain and atmosphere.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SceneLighting {
    /// Direction towards the sun in globe world coordinates, if set.
    pub sun_direction: Option<Vector3<f64>>,
    /// Solar twilight band: `[night_end_sin, day_start_sin, night_brightness, sun_visible_fraction]`.
    pub twilight: [f32; 4],
    /// Direction towards the moon in globe world coordinates with overhead light as `w`.
    pub moonlight: [f32; 4],
    /// Whether host atmosphere is active.
    pub atmosphere: bool,
}

impl Default for SceneLighting {
    fn default() -> Self {
        Self {
            sun_direction: None,
            twilight: [0.0, 0.0, 0.15, 1.0],
            moonlight: [0.0; 4],
            atmosphere: false,
        }
    }
}

impl SceneLighting {
    /// Creates a new default scene lighting configuration.
    pub fn new() -> Self {
        Self::default()
    }

    /// Daylight uniform passed to terrain tiles: `[x, y, z, 1.0]` if set, else `[0.0; 4]`.
    pub fn daylight_uniform(&self) -> [f32; 4] {
        self.sun_direction.map_or([0.0; 4], |sun| {
            [sun.x as f32, sun.y as f32, sun.z as f32, 1.0]
        })
    }

    /// Visible fraction of the sun disc (1.0 outside eclipse).
    pub fn sun_visible(&self) -> f32 {
        self.twilight[3]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_scene_lighting_defaults() {
        let lighting = SceneLighting::default();
        assert_eq!(lighting.sun_direction, None);
        assert_eq!(lighting.daylight_uniform(), [0.0; 4]);
        assert_eq!(lighting.sun_visible(), 1.0);
        assert!(!lighting.atmosphere);
    }

    #[test]
    fn test_scene_lighting_with_sun() {
        let lighting = SceneLighting {
            sun_direction: Some(Vector3::new(1.0, 0.0, 0.0)),
            twilight: [0.0, 0.0, 0.15, 0.5],
            ..Default::default()
        };
        assert_eq!(lighting.daylight_uniform(), [1.0, 0.0, 0.0, 1.0]);
        assert_eq!(lighting.sun_visible(), 0.5);
    }
}
