//! Geometry uploads follow the actual sources selected for each terrain texture.
use crate::{
    coords::WorldTileCoords,
    io::tile_sources::{clamp_to_max_zoom, source_max_zoom, TileKind},
    style::Style,
    tcs::world::World,
    terrain::{drape_targets::select_targets, request_system::DrapeRequests},
};
use std::collections::HashSet;

pub(super) fn terrain_sources(world: &World, style: &Style) -> Vec<WorldTileCoords> {
    let max_zoom = source_max_zoom(style, TileKind::Vector);
    let targets = std::iter::once(WorldTileCoords::default()).chain(
        world
            .resources
            .get::<DrapeRequests>()
            .into_iter()
            .flat_map(|requests| requests.0.iter().copied()),
    );
    let selected = select_targets(targets, world, &[]);
    let mut seen = HashSet::new();
    let mut sources = Vec::new();
    for (target, shapes) in selected {
        // A CPU-ready ancestor or complete child cover must reach the GPU even when
        // the requested tile exceeds maxzoom or has not arrived yet.
        for source in shapes
            .into_iter()
            .filter(|shape| shape.raster_source.is_none())
            .map(|shape| shape.coords)
            .chain(std::iter::once(clamp_to_max_zoom(target, max_zoom)))
        {
            if seen.insert(source) {
                sources.push(source);
            }
        }
    }
    sources
}
