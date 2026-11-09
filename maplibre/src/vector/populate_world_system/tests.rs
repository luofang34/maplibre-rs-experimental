#![allow(clippy::expect_used, clippy::panic)]
use super::*;
use crate::{
    coords::WorldTileCoords,
    io::tile_backpressure::{request_budget, MAX_TILES_IN_FLIGHT},
    sdf::SymbolLayersDataComponent,
    tcs::world::World,
};
#[test]
fn visible_base_geometry_keeps_its_loading_slot_until_its_request_lands() {
    use crate::io::tile_retry::{self, RequestDisposition, RequestKind, TileRequestOutcome};
    let mut world = World::default();
    let coords = WorldTileCoords::default();
    world
        .tiles
        .spawn_mut(coords)
        .expect("tile")
        .insert(VectorLayerBucketComponent::default())
        .insert(SymbolLayersDataComponent::default());
    let attempt = tile_retry::next_attempt(&mut world);
    tile_retry::started(&mut world, coords, RequestKind::Vector, attempt, None);
    finish_tile(&mut world, &DefaultTileTessellated::build_partial(coords));
    assert!(
        world
            .tiles
            .query::<&VectorLayerBucketComponent>(coords)
            .expect("base")
            .done
    );
    assert_eq!(request_budget(&world), MAX_TILES_IN_FLIGHT - 1);
    // The worker completes the labels before it reports the request's outcome.
    finish_tile(&mut world, &DefaultTileTessellated::build_from(coords));
    assert_eq!(request_budget(&world), MAX_TILES_IN_FLIGHT - 1);
    tile_retry::completed(
        &mut world,
        TileRequestOutcome {
            coords,
            kind: RequestKind::Vector,
            attempt: Some(attempt),
            disposition: RequestDisposition::Complete,
        },
    );
    assert_eq!(request_budget(&world), MAX_TILES_IN_FLIGHT);
}

fn bucket(coords: WorldTileCoords, id: &str) -> crate::vector::AvailableVectorLayerBucket {
    crate::vector::AvailableVectorLayerBucket {
        coords,
        source_layer: id.to_owned(),
        style_layer_id: id.to_owned(),
        buffer: crate::vector::tessellation::OverAlignedVertexBuffer::empty(),
        feature_indices: Vec::new(),
        feature_colors: Vec::new(),
        feature_sort_keys: Vec::new(),
    }
}

/// A loaded tile showing `layers`, as an earlier request left it.
fn loaded(layers: &[&str]) -> (World, WorldTileCoords) {
    let mut world = World::default();
    let coords = WorldTileCoords::default();
    world
        .tiles
        .spawn_mut(coords)
        .expect("tile")
        .insert(VectorLayerBucketComponent {
            done: true,
            layers: layers
                .iter()
                .map(|id| crate::vector::VectorLayerBucket::AvailableLayer(bucket(coords, id)))
                .collect(),
            ..Default::default()
        })
        .insert(SymbolLayersDataComponent::default());
    (world, coords)
}

fn shown(world: &World, coords: WorldTileCoords) -> Vec<String> {
    crate::vector::content::vector_layers(&world.tiles, coords)
        .into_iter()
        .map(|layer| layer.style_layer_id.clone())
        .collect()
}

#[test]
fn a_finished_request_drops_the_layers_it_was_asked_for_but_left_out() {
    // The route was hidden, or its data emptied, so the new request brings roads alone.
    let (mut world, coords) = loaded(&["roads", "route"]);
    crate::vector::content::begin(&mut world, coords, vec!["roads".into(), "route".into()]);
    crate::vector::content::accept_vector(&mut world, bucket(coords, "roads"));
    finish_tile(&mut world, &DefaultTileTessellated::build_partial(coords));
    assert_eq!(shown(&world, coords), ["roads"]);
}

#[test]
fn a_request_limited_to_some_sources_keeps_the_layers_of_the_others() {
    let (mut world, coords) = loaded(&["roads", "route"]);
    crate::vector::content::begin(&mut world, coords, vec!["route".into()]);
    finish_tile(&mut world, &DefaultTileTessellated::build_from(coords));
    assert_eq!(shown(&world, coords), ["roads"]);
}

#[test]
fn a_request_that_could_not_read_its_data_keeps_what_the_tile_shows() {
    let (mut world, coords) = loaded(&["roads", "route"]);
    crate::vector::content::begin(&mut world, coords, vec!["roads".into(), "route".into()]);
    finish_tile(
        &mut world,
        &DefaultTileTessellated::build_failed(coords, false),
    );
    assert_eq!(shown(&world, coords), ["roads", "route"]);
}
