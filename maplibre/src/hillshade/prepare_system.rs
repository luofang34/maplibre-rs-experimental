//! Writes every visible DEM-shaded layer's uniforms for the frame.

use std::collections::HashSet;

use crate::{
    context::MapContext,
    hillshade::{
        dem_layer_kind,
        resources::{ColorReliefUniforms, DemLayerKind, HillshadeResources, HillshadeUniforms},
    },
    render::{
        eventually::{Eventually, Eventually::Initialized},
        Renderer,
    },
    style::{layer::LayerPaint, source::Source},
    tcs::system::{SystemError, SystemResult},
};

pub fn prepare_system(
    MapContext {
        world,
        style,
        view_state,
        renderer: Renderer { device, queue, .. },
        ..
    }: &mut MapContext,
) -> SystemResult {
    let Some(Initialized(resources)) = world
        .resources
        .query_mut::<&mut Eventually<HillshadeResources>>()
    else {
        return Err(SystemError::Dependencies);
    };
    let zoom = view_state.zoom().value();
    let bearing = view_state.camera().get_bearing().0;
    let mut written = HashSet::new();
    for layer in &style.layers {
        let Some(kind) = dem_layer_kind(&layer.type_) else {
            continue;
        };
        if !layer.is_visible_at(zoom) {
            continue;
        }
        let unpack = layer
            .source
            .as_ref()
            .and_then(|name| style.sources.get(name))
            .and_then(|source| match source {
                Source::RasterDem(dem) => Some(dem.unpack_vector()),
                _ => None,
            })
            .map_or([6553.6, 25.6, 0.1, 10000.0], |unpack| {
                unpack.map(|value| value as f32)
            });
        match (kind, &layer.paint) {
            (DemLayerKind::Hillshade, Some(LayerPaint::Hillshade(paint))) => {
                let uniforms = HillshadeUniforms::new(
                    unpack,
                    &paint.illumination(zoom, bearing),
                    paint.accent_at(zoom),
                    paint.exaggeration_at(zoom),
                    paint.hillshade_method.shader_code(),
                );
                resources.write_layer(
                    device,
                    queue,
                    &layer.id,
                    kind,
                    bytemuck::bytes_of(&uniforms),
                );
            }
            (DemLayerKind::ColorRelief, Some(LayerPaint::ColorRelief(paint))) => {
                let uniforms =
                    ColorReliefUniforms::new(unpack, paint.opacity_at(zoom), &paint.ramp());
                resources.write_layer(
                    device,
                    queue,
                    &layer.id,
                    kind,
                    bytemuck::bytes_of(&uniforms),
                );
            }
            _ => continue,
        }
        written.insert(layer.id.as_str());
    }
    resources.retain_layers(&written);
    Ok(())
}
