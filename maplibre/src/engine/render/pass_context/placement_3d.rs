//! Full 6-DoF geodetic + quaternion placement contract and per-slot visibility mask for
//! dynamic 3D entities rendered in [`crate::render::draw_graph::node::SCENE_3D_PASS`].
//!
//! Coordinate conventions:
//! - **Local tangent frame (`ENU`, metres)**: `+X` East, `+Y` North, `+Z` geodetic Up.
//! - **Body frame (`Body`, metres)**: right-handed, ENU-aligned at zero attitude:
//!   `+X` right wing / starboard, `+Y` forward / nose, `+Z` dorsal / up.

#![deny(missing_docs)]

use cgmath::{InnerSpace, Matrix3, Matrix4, Quaternion, Rad, Rotation3, Vector3, Vector4};

use crate::{
    coords::{LatLon, WorldCoords, Zoom, TILE_SIZE},
    projection::{body::Body, globe::angular_radians_to_unit_sphere},
    render::eye_covering::{CurrentViewSlot, ViewSlotId},
    tcs::world::World,
};

/// Vertical datum for [`Placement3d::altitude_meters`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum AltitudeReference {
    /// Geometric height in metres above Mean Sea Level (`0.0 m` MSL).
    #[default]
    AboveMsl,
    /// Height in metres above local terrain (`ground_elevation_m + altitude_meters`), preserving
    /// [`Placement3d::orientation`] relative to true East-North-Up.
    AboveGround,
    /// Clamped/offset relative to local terrain (`ground_elevation_m + altitude_meters`) with the
    /// tangent frame's `+Z` tilted onto the local terrain surface normal (for ground vehicles).
    OnGroundSlopeAligned,
}

/// Unit quaternion `[x, y, z, w]` rotating right-handed body coordinates (`+X` right, `+Y`
/// forward, `+Z` up) into the local East-North-Up (`+X` East, `+Y` North, `+Z` Up) frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Orientation3d {
    /// Unit quaternion components `[x, y, z, w]` (`x*i + y*j + z*k + w`).
    pub enu_from_body: [f64; 4],
}

impl Default for Orientation3d {
    fn default() -> Self {
        Self::IDENTITY
    }
}

impl Orientation3d {
    /// Identity orientation: nose due North (`+Y`), right wing East (`+X`), dorsal Up (`+Z`).
    pub const IDENTITY: Self = Self {
        enu_from_body: [0.0, 0.0, 0.0, 1.0],
    };

    /// Returns [`Self::IDENTITY`].
    pub const fn identity() -> Self {
        Self::IDENTITY
    }

    /// Constructs a normalized quaternion from raw `(x, y, z, w)` components, falling back to
    /// [`Self::IDENTITY`] if the vector is non-finite or zero-length.
    pub fn from_xyzw(x: f64, y: f64, z: f64, w: f64) -> Self {
        Self {
            enu_from_body: [x, y, z, w],
        }
        .normalized()
    }

    /// Builds a gimbal-lock-free 6-DoF orientation from aviation Euler angles in degrees:
    /// - `heading_deg`: clockwise from true North (`0°` = North, `90°` = East).
    /// - `pitch_deg`: nose up (`+90°` = vertical climb, `-90°` = vertical dive).
    /// - `roll_deg`: right wing down (`+90°` = knife-edge right bank, `±180°` = inverted).
    pub fn from_heading_pitch_roll_degrees(
        heading_deg: f64,
        pitch_deg: f64,
        roll_deg: f64,
    ) -> Self {
        let q_yaw = Quaternion::from_angle_z(Rad(-heading_deg.to_radians()));
        let q_pitch = Quaternion::from_angle_x(Rad(pitch_deg.to_radians()));
        let q_roll = Quaternion::from_angle_y(Rad(roll_deg.to_radians()));
        Self::from_cgmath(q_yaw * q_pitch * q_roll)
    }

    /// Returns a unit-length copy of `self`, or [`Self::IDENTITY`] if degenerate.
    pub fn normalized(&self) -> Self {
        let [x, y, z, w] = self.enu_from_body;
        let norm2 = x * x + y * y + z * z + w * w;
        if !norm2.is_finite() || norm2 <= 1e-30 {
            return Self::IDENTITY;
        }
        let inv = 1.0 / norm2.sqrt();
        Self {
            enu_from_body: [x * inv, y * inv, z * inv, w * inv],
        }
    }

    /// Converts `self` into an exact `f64` 3×3 rotation matrix (`ENU <- Body`).
    pub fn to_rotation_matrix3(&self) -> Matrix3<f64> {
        let [x, y, z, w] = self.normalized().enu_from_body;
        Matrix3::from(Quaternion::new(w, x, y, z))
    }

    /// Tilts `self` by the shortest-arc rotation taking `+Z_enu (0, 0, 1)` onto
    /// `ground_normal_enu`, preserving vehicle heading on sloped runways and roads.
    pub fn align_to_slope_enu(&self, ground_normal_enu: Vector3<f64>) -> Self {
        let n_len2 = ground_normal_enu.magnitude2();
        if !n_len2.is_finite() || n_len2 <= 1e-24 {
            return self.normalized();
        }
        let n = ground_normal_enu / n_len2.sqrt();
        let up = Vector3::new(0.0, 0.0, 1.0);
        let dot = up.dot(n).clamp(-1.0, 1.0);
        let q_slope = if dot < -0.999_999_999 {
            Quaternion::new(0.0, 1.0, 0.0, 0.0)
        } else {
            let axis = up.cross(n);
            let s = ((1.0 + dot) * 2.0).sqrt();
            let inv_s = 1.0 / s;
            Quaternion::new(s * 0.5, axis.x * inv_s, axis.y * inv_s, axis.z * inv_s)
        };
        let [x, y, z, w] = self.normalized().enu_from_body;
        Self::from_cgmath(q_slope * Quaternion::new(w, x, y, z))
    }

    fn from_cgmath(q: Quaternion<f64>) -> Self {
        Self::from_xyzw(q.v.x, q.v.y, q.v.z, q.s)
    }
}

/// Bitmask controlling per-[`ViewSlotId`] visibility for 3D entities (slots `0..64`).
///
/// For example, `ViewSlotMask::ALL.without(otw_slot)` hides ownship's exterior 3D airframe in
/// `ViewSlot 0` (Cockpit OTW) while keeping it visible in `ViewSlot 1` (Chase / External camera).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ViewSlotMask(pub u64);

impl Default for ViewSlotMask {
    fn default() -> Self {
        Self::ALL
    }
}

impl ViewSlotMask {
    /// Visible in every [`ViewSlotId`] and on the default single-view head.
    pub const ALL: Self = Self(u64::MAX);
    /// Hidden in every [`ViewSlotId`] and on the default single-view head.
    pub const NONE: Self = Self(0);

    /// Visible exclusively in `slot` (`slot.as_u32() < 64`).
    pub const fn only(slot: ViewSlotId) -> Self {
        let idx = slot.as_u32();
        if idx < 64 {
            Self(1_u64 << idx)
        } else {
            Self::NONE
        }
    }

    /// Enables visibility in `slot`.
    pub const fn with(self, slot: ViewSlotId) -> Self {
        let idx = slot.as_u32();
        if idx < 64 {
            Self(self.0 | (1_u64 << idx))
        } else {
            self
        }
    }

    /// Disables visibility in `slot`.
    pub const fn without(self, slot: ViewSlotId) -> Self {
        let idx = slot.as_u32();
        if idx < 64 {
            Self(self.0 & !(1_u64 << idx))
        } else {
            self
        }
    }

    /// Returns `true` when `slot` is enabled in this mask (`slot.as_u32() < 64`).
    pub const fn includes(self, slot: ViewSlotId) -> bool {
        let idx = slot.as_u32();
        idx < 64 && (self.0 & (1_u64 << idx)) != 0
    }

    /// Evaluates visibility for `current` (`Some(slot)` during `MultiViewFrame`, `None` on the
    /// default single-view head).
    pub const fn matches_current(self, current: Option<ViewSlotId>) -> bool {
        match current {
            Some(slot) => self.includes(slot),
            None => self.0 != 0,
        }
    }

    /// Evaluates visibility against the active [`CurrentViewSlot`] stored in `world`.
    pub fn is_visible_in_world(self, world: &World) -> bool {
        let current = world.resources.get::<CurrentViewSlot>().and_then(|s| s.0);
        self.matches_current(current)
    }
}

/// Geodetic 6-DoF placement of a dynamic 3D entity (aircraft, rotorcraft, ground vehicle, or
/// static 3D model instance).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placement3d {
    /// Geodetic latitude and longitude in degrees.
    pub position: LatLon,
    /// Vertical offset in metres interpreted according to [`Self::altitude_reference`].
    pub altitude_meters: f64,
    /// Vertical datum and slope-alignment policy.
    pub altitude_reference: AltitudeReference,
    /// Full 3D body-to-ENU quaternion attitude (no 2-angle Euler lock-in).
    pub orientation: Orientation3d,
    /// Per-slot visibility mask for heterogeneous multi-view rendering.
    pub view_mask: ViewSlotMask,
}

impl Placement3d {
    /// Creates a [`Placement3d`] visible in all slots with [`AltitudeReference::AboveMsl`].
    pub const fn new_msl(
        position: LatLon,
        altitude_meters: f64,
        orientation: Orientation3d,
    ) -> Self {
        Self {
            position,
            altitude_meters,
            altitude_reference: AltitudeReference::AboveMsl,
            orientation,
            view_mask: ViewSlotMask::ALL,
        }
    }

    /// Resolves geometric altitude in metres MSL given `ground_elevation_meters` MSL under
    /// [`Self::position`].
    pub fn resolved_altitude_msl(&self, ground_elevation_meters: f64) -> f64 {
        match self.altitude_reference {
            AltitudeReference::AboveMsl => self.altitude_meters,
            AltitudeReference::AboveGround | AltitudeReference::OnGroundSlopeAligned => {
                ground_elevation_meters + self.altitude_meters
            }
        }
    }

    /// Resolves the effective body-to-ENU orientation, applying terrain normal alignment when
    /// [`AltitudeReference::OnGroundSlopeAligned`] is selected.
    pub fn effective_orientation(&self, ground_normal_enu: Option<Vector3<f64>>) -> Orientation3d {
        match (self.altitude_reference, ground_normal_enu) {
            (AltitudeReference::OnGroundSlopeAligned, Some(normal)) => {
                self.orientation.align_to_slope_enu(normal)
            }
            _ => self.orientation.normalized(),
        }
    }

    /// Returns `true` when this entity should be drawn in `world`'s active [`CurrentViewSlot`].
    pub fn is_visible_in_world(&self, world: &World) -> bool {
        self.view_mask.is_visible_in_world(world)
    }

    /// Exact `f64` model-to-unit-sphere matrix mapping body-frame metres into `body`'s unit-sphere
    /// world space (matching [`crate::projection::globe`] and `ExternalView`).
    pub fn model_to_unit_sphere(
        &self,
        body: &Body,
        ground_elevation_meters: f64,
        ground_normal_enu: Option<Vector3<f64>>,
    ) -> Matrix4<f64> {
        let alt_msl = self.resolved_altitude_msl(ground_elevation_meters);
        let (east, north, up) = enu_axes_on_unit_sphere(self.position);
        let sphere_from_enu = Matrix3::from_cols(east, north, up);
        let enu_from_body = self
            .effective_orientation(ground_normal_enu)
            .to_rotation_matrix3();
        let linear = (sphere_from_enu * enu_from_body) * (1.0 / body.radius_meters);
        let origin = up * body.unit_radius_at(alt_msl);
        Matrix4::from_cols(
            linear.x.extend(0.0),
            linear.y.extend(0.0),
            linear.z.extend(0.0),
            Vector4::new(origin.x, origin.y, origin.z, 1.0),
        )
    }

    /// Multiplies `view_projection * model_to_unit_sphere(...)` in full `f64` precision before
    /// downcasting to `[[f32; 4]; 4]` for GPU upload, avoiding `f32` world-origin jitter.
    pub fn model_view_projection_globe(
        &self,
        view_projection: Matrix4<f64>,
        body: &Body,
        ground_elevation_meters: f64,
        ground_normal_enu: Option<Vector3<f64>>,
    ) -> [[f32; 4]; 4] {
        let mvp = view_projection
            * self.model_to_unit_sphere(body, ground_elevation_meters, ground_normal_enu);
        mvp.cast::<f32>()
            .unwrap_or_else(|| Matrix4::from_scale(0.0))
            .into()
    }

    /// Exact `f64` model-to-Mercator-world matrix mapping body-frame metres into zoom-scaled
    /// Web Mercator world coordinates (`+X` East px, `+Y` South px, `+Z` Up metres).
    pub fn model_to_mercator_world(
        &self,
        zoom: Zoom,
        body: &Body,
        ground_elevation_meters: f64,
        ground_normal_enu: Option<Vector3<f64>>,
    ) -> Matrix4<f64> {
        let alt_msl = self.resolved_altitude_msl(ground_elevation_meters);
        let world_xy = WorldCoords::from_lat_lon(self.position, zoom);
        let world_size = TILE_SIZE * 2_f64.powf(zoom.value());
        let cos_lat = self.position.latitude.to_radians().cos().abs().max(1e-6);
        let px_per_m = world_size / (body.circumference_meters() * cos_lat);
        let merc_from_enu = Matrix3::from_cols(
            Vector3::new(px_per_m, 0.0, 0.0),
            Vector3::new(0.0, -px_per_m, 0.0),
            Vector3::new(0.0, 0.0, 1.0),
        );
        let enu_from_body = self
            .effective_orientation(ground_normal_enu)
            .to_rotation_matrix3();
        let linear = merc_from_enu * enu_from_body;
        Matrix4::from_cols(
            linear.x.extend(0.0),
            linear.y.extend(0.0),
            linear.z.extend(0.0),
            Vector4::new(world_xy.x, world_xy.y, alt_msl, 1.0),
        )
    }
}

fn enu_axes_on_unit_sphere(location: LatLon) -> (Vector3<f64>, Vector3<f64>, Vector3<f64>) {
    let lon = location.longitude.to_radians();
    let lat = location.latitude.to_radians();
    let up = angular_radians_to_unit_sphere(lon, lat);
    let east = Vector3::new(lon.cos(), 0.0, -lon.sin());
    let north = Vector3::new(-lon.sin() * lat.sin(), lat.cos(), -lon.cos() * lat.sin());
    (east, north, up)
}

#[cfg(test)]
mod tests;
