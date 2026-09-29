//! A cached drape is replaced only by a complete, uploaded set of visible layers.

use crate::{
    io::tile_sources::RASTER_LAYER_TYPES,
    style::Style,
    tcs::world::World,
    terrain::drape_targets::{coverage::covers_target, TargetSpec},
    vector::geometry_uploaded,
};

pub(super) fn ready(
    spec: &TargetSpec,
    style: &Style,
    zoom: f64,
    world: &World,
    strict: bool,
) -> bool {
    !spec.shapes.is_empty()
        && spec.shapes.iter().all(|shape| {
            !shape.raster_layers.is_empty()
                || ((!strict || shape.source == spec.coords)
                    && geometry_uploaded(shape.source, world))
        })
        && style
            .layers
            .iter()
            .filter(|layer| {
                RASTER_LAYER_TYPES.contains(&layer.type_.as_str()) && layer.is_visible_at(zoom)
            })
            .all(|layer| {
                let sources: Vec<_> = spec
                    .shapes
                    .iter()
                    .filter(|shape| shape.raster_layers.iter().any(|(id, _, _)| id == &layer.id))
                    .map(|shape| shape.source)
                    .collect();
                covers_target(spec.coords, &sources)
            })
}

#[cfg(test)]
mod tests;
