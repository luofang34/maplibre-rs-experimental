//! Per-layer values of the raster shader, derived from `raster-*` paint properties.

use bytemuck_derive::{Pod, Zeroable};

use crate::style::layer::{RasterPaint, StyleProperty};

/// Bind group layout of a layer's [`RasterUniforms`], group two of the raster pipeline.
pub fn layout() -> Vec<wgpu::BindGroupLayoutEntry> {
    vec![wgpu::BindGroupLayoutEntry {
        binding: 0,
        visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }]
}

/// The translation, in clip units per unit of `w`, that puts the centre of the view on a whole
/// pixel for raster tiles, as GL JS aligns them while the map is at rest.
pub fn pixel_alignment(view: &crate::render::view_state::ViewState) -> [f32; 2] {
    let center = view.camera().position();
    let angle = view.camera().get_bearing().0;
    let (cos, sin) = (angle.cos(), angle.sin());
    let shift_x = (view.width() % 2.0) / 2.0;
    let shift_y = (view.height() % 2.0) / 2.0;
    let mut dx = center.x - center.x.round() + cos * shift_x + sin * shift_y;
    let mut dy = center.y - center.y.round() + cos * shift_y + sin * shift_x;
    if dx > 0.5 {
        dx -= 1.0;
    }
    if dy > 0.5 {
        dy -= 1.0;
    }
    let matrix = view.view_projection().0;
    let ndc = |x: f64, y: f64| {
        let clip = matrix * cgmath::Vector4::new(x, y, 0.0, 1.0);
        [clip.x / clip.w, clip.y / clip.w]
    };
    let (at, moved) = (ndc(center.x, center.y), ndc(center.x + dx, center.y + dy));
    [(moved[0] - at[0]) as f32, (moved[1] - at[1]) as f32]
}

/// Uniforms of the raster fragment stage; the layout mirrors the WGSL struct.
#[repr(C)]
#[derive(Copy, Clone, Debug, PartialEq, Pod, Zeroable)]
pub struct RasterUniforms {
    /// Hue rotation matrix rows in the form GL JS uses: the three dot-product weights.
    pub spin_weights: [f32; 4],
    /// Layer alpha multiplier.
    pub opacity: f32,
    /// Weight pulling colours towards their average.
    pub saturation_factor: f32,
    /// Scale of colours around middle grey.
    pub contrast_factor: f32,
    /// Output for black input.
    pub brightness_min: f32,
    /// Output for white input.
    pub brightness_max: f32,
    /// Aligns the next field to eight bytes, as WGSL does for a `vec2`.
    pub padding: f32,
    /// Screen translation in clip units per unit of `w`: the raster is placed as if the
    /// centre of the view lay on a whole pixel, which keeps texels on pixels.
    pub align: [f32; 2],
}

impl RasterUniforms {
    /// Evaluates the adjustments as GL JS `rasterUniformValues` does.
    pub fn from_paint(paint: &RasterPaint, zoom: f64) -> Self {
        let value = |property: &Option<StyleProperty<f32>>, default: f32| {
            property
                .as_ref()
                .and_then(|property| property.evaluate_at_zoom(zoom))
                .unwrap_or(default)
        };
        let hue = value(&paint.raster_hue_rotate, 0.0).to_radians();
        let (sin, cos) = hue.sin_cos();
        let root = 3.0_f32.sqrt();
        let saturation = value(&paint.raster_saturation, 0.0);
        let contrast = value(&paint.raster_contrast, 0.0);
        Self {
            spin_weights: [
                (2.0 * cos + 1.0) / 3.0,
                (-root * sin - cos + 1.0) / 3.0,
                (root * sin - cos + 1.0) / 3.0,
                0.0,
            ],
            opacity: value(&paint.raster_opacity, 1.0).clamp(0.0, 1.0),
            saturation_factor: if saturation > 0.0 {
                1.0 - 1.0 / (1.001 - saturation)
            } else {
                -saturation
            },
            contrast_factor: if contrast > 0.0 {
                1.0 / (1.0 - contrast)
            } else {
                1.0 + contrast
            },
            brightness_min: value(&paint.raster_brightness_min, 0.0),
            brightness_max: value(&paint.raster_brightness_max, 1.0),
            padding: 0.0,
            align: [0.0; 2],
        }
    }
}

#[cfg(test)]
mod alignment_tests {
    use cgmath::Deg;

    use super::pixel_alignment;
    use crate::{
        coords::{LatLon, WorldCoords, Zoom},
        render::view_state::ViewState,
        window::PhysicalSize,
    };

    fn view(bearing: f64) -> ViewState {
        let zoom = Zoom::new(16.0);
        let mut view = ViewState::new(
            PhysicalSize::new(512, 256).expect("size"),
            WorldCoords::from_lat_lon(LatLon::new(52.499167, 13.418056), zoom),
            zoom,
            Deg(0.0),
            cgmath::Rad(0.6435011087932844),
        );
        view.camera_mut().set_bearing(Deg(bearing));
        view
    }

    /// The map moves on screen by the fractional position of its centre, turned with the map.
    #[test]
    fn the_shift_is_the_fraction_of_the_centre_turned_by_the_bearing() {
        let view = view(0.0);
        let center = view.camera().position();
        let fraction = [center.x - center.x.round(), center.y - center.y.round()];
        let to_pixels = |ndc: [f32; 2]| [ndc[0] * 256.0, -ndc[1] * 128.0];
        let flat = to_pixels(pixel_alignment(&view));
        assert!((f64::from(flat[0]) - fraction[0]).abs() < 0.01, "{flat:?}");
        assert!((f64::from(flat[1]) - fraction[1]).abs() < 0.01, "{flat:?}");
        // A bearing of 90 degrees puts east at the top, so the shift turns a quarter.
        let turned = to_pixels(pixel_alignment(&self::view(90.0)));
        assert!(
            (f64::from(turned[0]) - fraction[1]).abs() < 0.01,
            "{turned:?}"
        );
        assert!(
            (f64::from(turned[1]) + fraction[0]).abs() < 0.01,
            "{turned:?}"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paint(json: serde_json::Value) -> RasterPaint {
        serde_json::from_value(json).expect("valid raster paint")
    }

    #[test]
    fn neutral_paint_changes_nothing() {
        let uniforms = RasterUniforms::from_paint(&paint(serde_json::json!({})), 0.0);
        assert_eq!(uniforms.opacity, 1.0);
        assert_eq!(uniforms.saturation_factor, 0.0);
        assert_eq!(uniforms.contrast_factor, 1.0);
        assert_eq!(uniforms.brightness_max, 1.0);
        assert!((uniforms.spin_weights[0] - 1.0).abs() < 1e-6);
        assert!(uniforms.spin_weights[1].abs() < 1e-6);
    }

    #[test]
    fn opacity_follows_zoom() {
        let paint = paint(serde_json::json!({
            "raster-opacity": {"stops": [[0, 0.0], [10, 1.0]]}
        }));
        assert_eq!(RasterUniforms::from_paint(&paint, 5.0).opacity, 0.5);
    }

    #[test]
    fn full_saturation_and_contrast_amplify() {
        let uniforms = RasterUniforms::from_paint(
            &paint(serde_json::json!({
                "raster-saturation": 0.5, "raster-contrast": 0.5, "raster-opacity": 2.0
            })),
            0.0,
        );
        assert!((uniforms.saturation_factor - (1.0 - 1.0 / 0.501)).abs() < 1e-5);
        assert_eq!(uniforms.contrast_factor, 2.0);
        assert_eq!(uniforms.opacity, 1.0);
    }
}
