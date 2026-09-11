//! Optional host relevance limits for symbols in externally driven 3D views.
use crate::{
    render::view_state::ViewState,
    sdf::{Feature, SymbolLayerData},
};
use cgmath::InnerSpace;
use std::collections::HashMap;

/// Host presentation policy; absent limits preserve style-defined symbol visibility.
#[derive(Clone, Debug, Default)]
pub struct SymbolVisibility {
    /// Far distance in metres by style layer. Symbols fade over the last quarter.
    pub layer_distances: HashMap<String, f64>,
    /// Expands each distance limit by this multiple of eye altitude for globe overviews.
    pub altitude_scale: f64,
}
impl SymbolVisibility {
    pub(crate) fn opacity(
        &self,
        layer: &SymbolLayerData,
        feature: &Feature,
        ground: f32,
        view: &ViewState,
    ) -> f32 {
        let Some(limit) = self.layer_distances.get(&layer.style_layer_id) else {
            return 1.0;
        };
        let Some(eye) = view.external_globe_eye() else {
            return 1.0;
        };
        let Some(tile) = super::placement::canonical_tile(layer.coords) else {
            return 0.0;
        };
        let radius = view.body().radius_meters;
        let surface = crate::projection::globe::project_tile_coordinates_to_unit_sphere(
            tile.x,
            tile.y,
            u8::from(tile.z),
            f64::from(feature.text_anchor.x),
            f64::from(feature.text_anchor.y),
        );
        let distance =
            (surface * (1.0 + f64::from(ground) / radius) - eye.position).magnitude() * radius;
        let altitude = (eye.position.magnitude() - 1.0).max(0.0) * radius;
        distance_opacity(distance, limit.max(altitude * self.altitude_scale.max(0.0)))
    }
}
fn distance_opacity(distance: f64, limit: f64) -> f32 {
    if !limit.is_finite() || limit <= 0.0 {
        return 1.0;
    }
    let t = ((limit - distance) / (limit * 0.25)).clamp(0.0, 1.0);
    (t * t * (3.0 - 2.0 * t)) as f32
}
#[cfg(test)]
mod tests;
