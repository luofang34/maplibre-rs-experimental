#![allow(clippy::expect_used)]

use std::collections::HashSet;

use super::*;

#[derive(Default)]
struct LoadedTiles(HashSet<WorldTileCoords>);

impl HasTile for LoadedTiles {
    fn has_tile(&self, coords: WorldTileCoords, _world: &World) -> bool {
        self.0.contains(&coords)
    }
}

fn selected(
    target: WorldTileCoords,
    covering: Vec<WorldTileCoords>,
    loaded: Vec<WorldTileCoords>,
) -> Vec<WorldTileCoords> {
    let mut world = World::default();
    let mut sources = ViewTileSources::default();
    sources.add_resource_query::<&LoadedTiles>(TileKind::Raster);
    world.resources.insert(sources);
    world
        .resources
        .insert(LoadedTiles(loaded.into_iter().collect()));
    select_targets(
        std::iter::once(target),
        &world,
        &[("relief".into(), covering)],
    )
    .pop()
    .expect("target")
    .1
    .into_iter()
    .filter(|shape| shape.raster_source.as_ref().and_then(RasterSourceId::name) == Some("relief"))
    .map(|shape| shape.coords)
    .collect()
}

#[test]
fn every_partial_child_subset_keeps_the_loaded_parent_without_overlap() {
    let target = WorldTileCoords::from((2423, 1389, 12_u8.into()));
    let children = target.get_children();
    for bits in 0_u8..16 {
        let mut loaded = vec![target];
        loaded.extend(
            children
                .iter()
                .enumerate()
                .filter(|(i, _)| bits & (1 << i) != 0)
                .map(|(_, c)| *c),
        );
        let selected = selected(target, children.to_vec(), loaded);
        if bits == 15 {
            assert_eq!(
                selected.into_iter().collect::<HashSet<_>>(),
                children.into_iter().collect()
            );
        } else {
            assert_eq!(selected, vec![target], "loaded child mask {bits:04b}");
        }
    }
}

#[test]
fn partial_children_without_an_ancestor_wait_for_complete_coverage() {
    let target = WorldTileCoords::from((2423, 1389, 12_u8.into()));
    let children = target.get_children();
    assert!(selected(target, children.to_vec(), children[..3].to_vec()).is_empty());
}

#[test]
fn mixed_lod_covering_switches_only_when_its_last_gap_is_loaded() {
    let target = WorldTileCoords::from((2423, 1389, 12_u8.into()));
    let children = target.get_children();
    let mut covering = children[..3].to_vec();
    covering.extend(children[3].get_children());
    for count in 0..=covering.len() {
        let mut loaded = covering[..count].to_vec();
        loaded.push(target.get_parent().expect("ancestor"));
        let selected = selected(target, covering.clone(), loaded);
        if count == covering.len() {
            assert_eq!(
                selected.into_iter().collect::<HashSet<_>>(),
                covering.iter().copied().collect()
            );
        } else {
            assert_eq!(selected, vec![target.get_parent().expect("ancestor")]);
        }
    }
}
