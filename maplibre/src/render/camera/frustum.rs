//! An eye's frustum as the tangents of its half angles, the form head-mounted displays report
//! a per-eye projection in and the form that survives a change of units.

use cgmath::{frustum, Matrix4, Rad};

/// Viewing frustum of one eye: tangents of the half angles between the view axis and each
/// side of the image, and the clip distances along the axis.
///
/// The tangents are positive when the side lies on its own half of the axis, so a symmetric
/// frustum has `left == right` and `top == bottom`; an off-axis eye shifts them. The clip
/// distances are in whatever length unit the caller renders in.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EyeFrustum {
    /// Tangent of the half angle to the left edge.
    pub left: f64,
    /// Tangent of the half angle to the right edge.
    pub right: f64,
    /// Tangent of the half angle to the top edge.
    pub top: f64,
    /// Tangent of the half angle to the bottom edge.
    pub bottom: f64,
    /// Near clip distance.
    pub near: f64,
    /// Far clip distance.
    pub far: f64,
}

impl EyeFrustum {
    /// A symmetric frustum from a vertical field of view and the image's width over height.
    pub fn symmetric(field_of_view: Rad<f64>, aspect: f64, near: f64, far: f64) -> Self {
        let vertical = (field_of_view.0 / 2.0).tan();
        let horizontal = vertical * aspect;
        Self {
            left: horizontal,
            right: horizontal,
            top: vertical,
            bottom: vertical,
            near,
            far,
        }
    }

    /// The frustum an OpenGL projection matrix describes, given the clip distances it was
    /// built with; an off-center perspective becomes its asymmetric tangents.
    pub fn from_projection(projection: Matrix4<f64>, near: f64, far: f64) -> Self {
        Self {
            left: (1.0 - projection.z.x) / projection.x.x,
            right: (1.0 + projection.z.x) / projection.x.x,
            top: (1.0 + projection.z.y) / projection.y.y,
            bottom: (1.0 - projection.z.y) / projection.y.y,
            near,
            far,
        }
    }

    /// The OpenGL projection matrix of the frustum: camera space with x right, y up and the
    /// eye looking along negative z, to clip space with depth from minus one to one.
    pub fn projection(&self) -> Matrix4<f64> {
        frustum(
            -self.left * self.near,
            self.right * self.near,
            -self.bottom * self.near,
            self.top * self.near,
            self.near,
            self.far,
        )
    }

    /// The same angles with the clip distances multiplied by `factor`, for a change of units.
    pub fn scaled(&self, factor: f64) -> Self {
        Self {
            near: self.near * factor,
            far: self.far * factor,
            ..*self
        }
    }

    /// Vertical field of view from the top edge to the bottom edge.
    pub fn vertical_field_of_view(&self) -> Rad<f64> {
        Rad(self.top.atan() + self.bottom.atan())
    }

    /// Width over height of the image plane.
    pub fn aspect(&self) -> f64 {
        (self.left + self.right) / (self.top + self.bottom)
    }
}

#[cfg(test)]
mod tests {
    use cgmath::{perspective, Rad};

    use super::EyeFrustum;

    fn assert_close(a: f64, b: f64, what: &str) {
        assert!(
            (a - b).abs() <= 1e-12 * a.abs().max(b.abs()).max(1.0),
            "{what}: {a} vs {b}"
        );
    }

    #[test]
    fn a_symmetric_frustum_matches_the_perspective_it_came_from() {
        let field_of_view = Rad(0.6435011087932844);
        let expected = perspective(field_of_view, 4.0 / 3.0, 0.1, 100.0);
        let frustum = EyeFrustum::symmetric(field_of_view, 4.0 / 3.0, 0.1, 100.0);
        let projection = frustum.projection();
        for column in 0..4 {
            for row in 0..4 {
                assert_close(projection[column][row], expected[column][row], "projection");
            }
        }
        assert_close(
            frustum.vertical_field_of_view().0,
            field_of_view.0,
            "field of view",
        );
        assert_close(frustum.aspect(), 4.0 / 3.0, "aspect");
    }

    #[test]
    fn an_asymmetric_projection_round_trips_through_its_tangents() {
        let original = EyeFrustum {
            left: 1.2,
            right: 0.7,
            top: 0.9,
            bottom: 1.1,
            near: 0.05,
            far: 250.0,
        };
        let recovered = EyeFrustum::from_projection(original.projection(), 0.05, 250.0);
        assert_close(recovered.left, original.left, "left");
        assert_close(recovered.right, original.right, "right");
        assert_close(recovered.top, original.top, "top");
        assert_close(recovered.bottom, original.bottom, "bottom");
    }

    #[test]
    fn scaling_changes_the_clip_distances_and_keeps_the_angles() {
        let frustum = EyeFrustum::symmetric(Rad(1.0), 1.5, 0.1, 10.0).scaled(1000.0);
        assert_close(frustum.near, 100.0, "near");
        assert_close(frustum.far, 10_000.0, "far");
        assert_close(frustum.vertical_field_of_view().0, 1.0, "field of view");
    }
}
