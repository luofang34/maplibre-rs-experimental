//! A cached drape is replaced only by a complete, uploaded set of visible layers.

use crate::{
    io::tile_sources::RASTER_LAYER_TYPES,
    raster::resource::RasterResources,
    render::{
        eventually::{Eventually, Eventually::Initialized},
        tile_view_pattern::coverage::covers_target,
    },
    style::Style,
    tcs::world::World,
    terrain::drape_targets::TargetSpec,
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
                let shapes: Vec<_> = spec
                    .shapes
                    .iter()
                    .filter(|shape| shape.raster_layers.iter().any(|(id, _, _)| id == &layer.id))
                    .collect();
                let sources: Vec<_> = shapes.iter().map(|shape| shape.source).collect();
                let absent = matches!(
                    world.resources.get::<Eventually<RasterResources>>(),
                    Some(Initialized(resources))
                        if resources
                            .layer_source(&layer.id)
                            .is_some_and(|source| spec.absent_sources.contains(source))
                );
                absent
                    || covers_target(spec.coords, &sources)
                    || (!shapes.is_empty() && shapes.iter().all(|shape| shape.view_complete))
            })
}

#[cfg(test)]
mod tests;
