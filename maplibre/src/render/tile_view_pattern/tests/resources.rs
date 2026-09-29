use super::*;
use crate::render::tile_view_pattern::QueryHasTile;

#[test]
fn resource_backed_tiles_are_unavailable_until_every_resource_exists() {
    let coords = tile(2, 3, 4);
    let mut world = World::default();
    LOADED_VECTOR.with(|tiles| *tiles.borrow_mut() = [coords].into());
    let source = QueryHasTile::<(&Loaded, &LoadedVector)>::default();
    assert!(!source.has_tile(coords, &world));
    world.resources.insert(Loaded([coords].into()));
    assert!(!source.has_tile(coords, &world));
    world.resources.insert(LoadedVector);
    assert!(source.has_tile(coords, &world));
    assert!(!source.has_tile(tile(3, 3, 4), &world));
}
