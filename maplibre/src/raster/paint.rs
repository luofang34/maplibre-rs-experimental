//! Per-layer values of the raster shader, derived from `raster-*` paint properties.

use bytemuck_derive::{Pod, Zeroable};

use crate::style::layer::{RasterPaint, StyleProperty};

/// Bind group layout of a layer's [`RasterUniforms`], group two of the raster pipeline.
pub fn layout() -> Vec<wgpu::BindGroupLayoutEntry> {
    vec![wgpu::BindGroupLayoutEntry {
        binding: 0,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }]
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
    /// Keeps the struct a multiple of sixteen bytes.
    pub padding: [f32; 3],
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
            padding: [0.0; 3],
        }
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
