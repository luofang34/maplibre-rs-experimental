//! Terrain standing a free-globe camera's target on the ground, unless that would bury its eye.
//!
//! A free-globe camera orbits its target, so standing the target on the ground under it carries
//! the eye by the same height. Looking down, the eye stays above the ground it carries over.
//! Looking up from near the ground, the target lies in the air ahead and grounding it would
//! swing the eye through the surface, where no camera can be drawn; such a pose keeps its
//! target where it was placed.

use cgmath::{InnerSpace, Rotation, Vector3};

use crate::render::view_state::ViewState;

impl ViewState {
    /// Stands the free-globe camera's target on ground `meters` above the mean radius, the eye
    /// moving with it, unless the eye would then lie on or under that ground. Returns whether
    /// the target now stands on the ground.
    pub(crate) fn ground_free_globe_target(&mut self, meters: f64) -> bool {
        let Some(free) = self.free_globe else {
            return false;
        };
        let radius = self.body().radius_meters;
        let forward = free
            .orientation
            .conjugate()
            .rotate_vector(Vector3::new(0.0, 0.0, -1.0));
        let grounded_eye = free.target * (radius + meters) - forward * free.distance_meters;
        if grounded_eye.magnitude() <= radius + meters {
            return false;
        }
        self.set_center_elevation(meters);
        true
    }
}
