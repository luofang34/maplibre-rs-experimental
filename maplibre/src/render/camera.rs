//! Orbit cameras and homogeneous transforms between world, camera and clip coordinates.

#![deny(missing_docs)]

use cgmath::{num_traits::clamp, prelude::*, *};

use crate::util::SignificantlyDifferent;

#[rustfmt::skip]
/// Converts OpenGL clip depth `[-w, w]` to WebGPU depth `[0, w]` without reversing it.
pub const OPENGL_TO_WGPU_MATRIX: Matrix4<f64> = Matrix4::new(
    1.0, 0.0, 0.0, 0.0,
    0.0, 1.0, 0.0, 0.0,
    0.0, 0.0, 0.5, 0.0,
    0.0, 0.0, 0.5, 1.0,
);

#[rustfmt::skip]
/// Reflects the vertical coordinate while preserving horizontal position and depth.
pub const FLIP_Y: Matrix4<f64> = Matrix4::new(
    1.0, 0.0, 0.0, 0.0, 
    0.0, -1.0, 0.0, 0.0, 
    0.0, 0.0, 1.0, 0.0, 
    0.0, 0.0, 0.0, 1.0,
);

/// Maps WebGPU depth `[0, 1]` to reversed-Z, so the near plane lands at 1 and the far plane at 0.
///
/// Float depth spacing then cancels the perspective divide, which keeps metre-scale geometry
/// near the camera and terrain hundreds of kilometres away from fighting in one depth buffer.
/// Depth-writing pipelines compare with `GreaterEqual` and the depth attachment clears to 0.
/// CPU-side unprojection keeps unreversed depth, where the far plane sits at depth 1.
#[rustfmt::skip]
pub const REVERSED_Z: Matrix4<f64> = Matrix4::new(
    1.0, 0.0, 0.0, 0.0,
    0.0, 1.0, 0.0, 0.0,
    0.0, 0.0, -1.0, 0.0,
    0.0, 0.0, 1.0, 1.0,
);

#[derive(Debug, Clone, Copy)]
/// World-to-clip transform; its depth convention is determined by the constructor or caller.
pub struct ViewProjection(
    /// Column-major transform applied to homogeneous world coordinates.
    pub Matrix4<f64>,
);

/// A projection cannot be unprojected into finite world coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("view projection is singular or non-finite")]
pub struct ViewProjectionError;

impl ViewProjection {
    /// Inverts a projection, rejecting singular or non-finite transforms.
    #[tracing::instrument(skip_all)]
    pub fn invert(&self) -> Result<InvertedViewProjection, ViewProjectionError> {
        let inverse = self.0.invert().ok_or(ViewProjectionError)?;
        let columns: &[[f64; 4]; 4] = inverse.as_ref();
        if !columns.iter().flatten().all(|value| value.is_finite()) {
            return Err(ViewProjectionError);
        }
        Ok(InvertedViewProjection {
            clip_to_camera: inverse,
            camera_to_world: Matrix4::identity(),
        })
    }

    /// Transforms a homogeneous world position without dividing the result by its `w` component.
    pub fn project(&self, vector: Vector4<f64>) -> Vector4<f64> {
        self.0 * vector
    }

    #[tracing::instrument(skip_all)]
    /// Prepends a model-to-world transform on the input side of this view projection.
    pub fn to_model_view_projection(&self, projection: Matrix4<f64>) -> ModelViewProjection {
        ModelViewProjection(self.0 * projection)
    }

    /// Copies the matrix to GPU precision; large world translations can lose low-order bits.
    pub fn downcast(&self) -> Matrix4<f32> {
        let columns: [[f64; 4]; 4] = self.0.into();
        Matrix4::from(columns.map(|column| column.map(|value| value as f32)))
    }
}

/// Clip-to-world transform with separate projection and camera factors for numerical precision.
pub struct InvertedViewProjection {
    pub(crate) clip_to_camera: Matrix4<f64>,
    pub(crate) camera_to_world: Matrix4<f64>,
}

impl InvertedViewProjection {
    /// Transforms a homogeneous clip position; divide by the returned `w` for world coordinates.
    pub fn project(&self, vector: Vector4<f64>) -> Vector4<f64> {
        // Form the camera ray before adding the world translation. Multiplying these
        // matrices first would cancel the small homogeneous depth at the far plane.
        self.camera_to_world * (self.clip_to_camera * vector)
    }
}

/// Combined model-to-world and world-to-clip transform.
pub struct ModelViewProjection(Matrix4<f64>);

impl ModelViewProjection {
    /// Copies the transform to GPU precision without changing its coordinate convention.
    pub fn downcast(&self) -> Matrix4<f32> {
        let columns: [[f64; 4]; 4] = self.0.into();
        Matrix4::from(columns.map(|column| column.map(|value| value as f32)))
    }

    /// Returns the column-major model-to-clip matrix at CPU precision.
    pub fn get(&self) -> Matrix4<f64> {
        self.0
    }

    /// Transforms a homogeneous model position without performing the perspective divide.
    pub fn project(&self, vector: Vector4<f64>) -> Vector4<f64> {
        self.0 * vector
    }
}

const MIN_PITCH: Deg<f64> = Deg(-30.0);
/// Default upper pitch bound, matching the GL JS `maxPitch` map option.
pub const DEFAULT_MAX_PITCH: Deg<f64> = Deg(60.0);

const MIN_YAW: Deg<f64> = Deg(-30.0);
const MAX_YAW: Deg<f64> = Deg(30.0);

#[derive(Debug, Clone)]
/// Orientation around a map center expressed in world pixels, with angles stored in radians.
pub struct Camera {
    position: Point2<f64>,
    yaw: Rad<f64>,
    pitch: Rad<f64>,
    bearing: Rad<f64>,
    roll: Rad<f64>,
    max_pitch: Rad<f64>,
}

impl SignificantlyDifferent for Camera {
    type Epsilon = f64;

    fn ne(&self, other: &Self, epsilon: Self::Epsilon) -> bool {
        self.position.abs_diff_ne(&other.position, epsilon)
            || self.yaw.abs_diff_ne(&other.yaw, epsilon)
            || self.pitch.abs_diff_ne(&other.pitch, epsilon)
            || self.bearing.abs_diff_ne(&other.bearing, epsilon)
            || self.roll.abs_diff_ne(&other.roll, epsilon)
    }
}

impl Camera {
    /// Creates a camera with zero bearing and roll, clamping pitch to its default limits.
    ///
    /// `position` is the orbit center in world pixels. Initial yaw is stored without clamping.
    pub fn new<V: Into<Point2<f64>>, Y: Into<Rad<f64>>, P: Into<Rad<f64>>>(
        position: V,
        yaw: Y,
        pitch: P,
    ) -> Self {
        let max_pitch: Rad<f64> = DEFAULT_MAX_PITCH.into();
        Self {
            position: position.into(),
            yaw: yaw.into(),
            pitch: Rad(pitch.into().0.clamp(Rad::from(MIN_PITCH).0, max_pitch.0)),
            bearing: Rad::zero(),
            roll: Rad::zero(),
            max_pitch,
        }
    }

    /// Returns the largest pitch the camera accepts.
    pub fn max_pitch(&self) -> Rad<f64> {
        self.max_pitch
    }

    /// Sets the largest pitch the camera accepts and clamps the current pitch to it.
    /// Callers must supply a finite limit at or above -30 degrees.
    pub fn set_max_pitch<P: Into<Rad<f64>>>(&mut self, max_pitch: P) {
        self.max_pitch = max_pitch.into();
        self.set_pitch(self.pitch);
    }

    /// Builds the world-to-camera matrix with an orbit distance in world pixels.
    ///
    /// All three input axes must use the same pixel scale; elevation scaling belongs to the caller.
    pub fn calc_matrix(&self, camera_height: f64) -> Matrix4<f64> {
        // GL JS turns the world by minus the bearing, so a bearing of 90 degrees puts east at the
        // top of the screen, and then turns the view by minus the roll about the view axis.
        Matrix4::from_translation(Vector3::new(0.0, 0.0, -camera_height))
            * Matrix4::from_angle_z(-self.roll)
            * Matrix4::from_angle_x(self.pitch)
            * Matrix4::from_angle_y(self.yaw)
            * Matrix4::from_angle_z(-self.bearing)
            * Matrix4::from_translation(Vector3::new(-self.position.x, -self.position.y, 0.0))
    }

    /// Returns the orbit center in world pixels, independent of pitch and bearing.
    pub fn position(&self) -> Point2<f64> {
        self.position
    }

    /// Returns the lateral camera rotation in radians.
    pub fn get_yaw(&self) -> Rad<f64> {
        self.yaw
    }

    /// Adds a yaw delta only if the result stays between -30 and 30 degrees.
    pub fn yaw<P: Into<Rad<f64>>>(&mut self, delta: P) {
        let new_yaw = self.yaw + delta.into();

        if new_yaw <= MAX_YAW.into() && new_yaw >= MIN_YAW.into() {
            self.yaw = new_yaw;
        }
    }

    /// Bearing of the map, clockwise from north.
    pub fn get_bearing(&self) -> Rad<f64> {
        self.bearing
    }

    /// Roll of the view about its own axis.
    pub fn get_roll(&self) -> Rad<f64> {
        self.roll
    }

    /// Returns tilt away from a vertical map view in radians.
    pub fn get_pitch(&self) -> Rad<f64> {
        self.pitch
    }

    /// Adds a pitch delta only if the result stays between -30 degrees and [`Self::max_pitch`].
    pub fn pitch<P: Into<Rad<f64>>>(&mut self, delta: P) {
        let new_pitch = self.pitch + delta.into();

        if new_pitch <= self.max_pitch && new_pitch >= MIN_PITCH.into() {
            self.pitch = new_pitch;
        }
    }

    /// Offsets the orbit center in world pixels, without rotating the delta by the camera bearing.
    pub fn move_relative(&mut self, delta: Vector2<f64>) {
        self.position += delta;
    }

    /// Replaces the orbit center in world pixels without changing orientation.
    pub fn move_to(&mut self, new_position: Point2<f64>) {
        self.position = new_position;
    }

    /// Returns the world-pixel orbit center as a vector from the world origin.
    pub fn position_vector(&self) -> Vector2<f64> {
        self.position.to_vec()
    }

    /// Appends a pixel height to the orbit center; this does not apply the camera's rotations.
    pub fn to_3d(&self, camera_height: f64) -> Point3<f64> {
        Point3::new(self.position.x, self.position.y, camera_height)
    }
    /// Sets lateral rotation, clamped to -30 through 30 degrees.
    pub fn set_yaw<P: Into<Rad<f64>>>(&mut self, yaw: P) {
        let new_yaw = yaw.into();
        let max: Rad<_> = MAX_YAW.into();
        let min: Rad<_> = MIN_YAW.into();
        self.yaw = Rad(new_yaw.0.min(max.0).max(min.0))
    }
    /// Sets tilt, clamped to -30 degrees through [`Self::max_pitch`].
    pub fn set_pitch<P: Into<Rad<f64>>>(&mut self, pitch: P) {
        let new_pitch = pitch.into();
        let min: Rad<_> = MIN_PITCH.into();
        self.pitch = Rad(new_pitch.0.min(self.max_pitch.0).max(min.0))
    }
    /// Sets clockwise rotation from north without normalizing the angle.
    pub fn set_bearing<P: Into<Rad<f64>>>(&mut self, bearing: P) {
        self.bearing = bearing.into();
    }
    /// Sets rotation about the view axis without clamping or normalizing the angle.
    pub fn set_roll<P: Into<Rad<f64>>>(&mut self, roll: P) {
        self.roll = roll.into();
    }
}

#[derive(PartialEq, Copy, Clone, Default)]
/// Viewport padding in pixels that shifts the apparent map center without resizing the viewport.
pub struct EdgeInsets {
    /// Padding measured down from the top edge.
    pub top: f64,
    /// Padding measured up from the bottom edge.
    pub bottom: f64,
    /// Padding measured right from the left edge.
    pub left: f64,
    /// Padding measured left from the right edge.
    pub right: f64,
}

impl EdgeInsets {
    /// Returns the padded center in viewport pixels, clamped to the viewport bounds.
    /// Coordinates start at the top left and increase rightward and downward.
    pub fn center(&self, width: f64, height: f64) -> Point2<f64> {
        // Clamp insets so they never overflow width/height and always calculate a valid center
        let x = clamp((self.left + width - self.right) / 2.0, 0.0, width);
        let y = clamp((self.top + height - self.bottom) / 2.0, 0.0, height);

        Point2::new(x, y)
    }
}

#[derive(Clone)]
/// Vertical field of view used to construct OpenGL-convention perspective matrices.
pub struct Perspective {
    fovy: Rad<f64>,
}

impl Perspective {
    /// Stores a vertical field of view without validating its range.
    pub fn new<F: Into<Rad<f64>>>(fovy: F) -> Self {
        let rad = fovy.into();
        Self { fovy: rad }
    }

    /// Returns the full vertical viewing angle in radians.
    pub fn fovy(&self) -> Rad<f64> {
        self.fovy
    }
    /// Returns the horizontal viewing angle for a viewport with positive width and height.
    pub fn fovx(&self, width: f64, height: f64) -> Rad<f64> {
        let aspect = width / height;
        Rad(2.0 * ((self.fovy / 2.0).tan() * aspect).atan())
    }

    /// Returns the half-height of the frustum at unit distance from the eye.
    pub fn y_tan(&self) -> f64 {
        let half_fovy = self.fovy / 2.0;
        half_fovy.tan()
    }
    /// Returns the half-width of the frustum at unit distance for the given viewport aspect.
    pub fn x_tan(&self, width: f64, height: f64) -> f64 {
        let half_fovx = self.fovx(width, height) / 2.0;
        half_fovx.tan()
    }

    /// Expresses a pixel center offset as a fraction of the viewport's half-width.
    pub fn offset_x(&self, center_offset: Point2<f64>, width: f64) -> f64 {
        center_offset.x * 2.0 / width
    }

    /// Expresses a pixel center offset as a fraction of the viewport's half-height.
    pub fn offset_y(&self, center_offset: Point2<f64>, height: f64) -> f64 {
        center_offset.y * 2.0 / height
    }

    /// Builds a centered perspective matrix with OpenGL clip depth `[-w, w]`.
    /// `aspect` is width/height; clip distances must satisfy `0 < near_z < far_z`.
    ///
    /// # Panics
    /// Panics if the field of view is outside `(0, pi)`, the aspect is approximately zero,
    /// or clip distances are nonpositive or approximately equal, as checked by [`cgmath::perspective`].
    pub fn calc_matrix(&self, aspect: f64, near_z: f64, far_z: f64) -> Matrix4<f64> {
        perspective(self.fovy, aspect, near_z, far_z)
    }

    /// Builds an off-center perspective matrix with OpenGL clip depth `[-w, w]`.
    /// Viewport dimensions and the center offset share pixel units; clip distances share world units.
    pub fn calc_matrix_with_center(
        &self,
        width: f64,
        height: f64,
        near_z: f64,
        far_z: f64,
        center_offset: Point2<f64>,
    ) -> Matrix4<f64> {
        let ymax = near_z * self.y_tan();

        //TODO maybe just: let xmax = ymax * aspect;
        let xmax = near_z * self.x_tan(width, height);

        let offset_x = self.offset_x(center_offset, width);
        let offset_y = self.offset_y(center_offset, height);
        frustum(
            // https://webglfundamentals.org/webgl/lessons/webgl-qna-how-can-i-move-the-perspective-vanishing-point-from-the-center-of-the-canvas-.html
            // Sliding the window one way moves the scene the other, so the window moves against
            // the offset for the scene's vanishing point to follow it.
            xmax * (-1.0 - offset_x),
            xmax * (1.0 - offset_x),
            ymax * (-1.0 - offset_y),
            ymax * (1.0 - offset_y),
            near_z,
            far_z,
        )
    }
}

mod frustum;

pub use frustum::EyeFrustum;

#[cfg(test)]
mod tests;

#[cfg(test)]
#[path = "camera/inversion/tests.rs"]
mod inversion_tests;
