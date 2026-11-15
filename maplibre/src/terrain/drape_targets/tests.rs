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
    selected_as(target, covering, loaded, Arrivals::Together)
}

fn selected_as(
    target: WorldTileCoords,
    covering: Vec<WorldTileCoords>,
    loaded: Vec<WorldTileCoords>,
    arrivals: Arrivals,
) -> Vec<WorldTileCoords> {
    let mut world = World::default();
    let mut sources = ViewTileSources::default();
    sources.add_resource_query::<&LoadedTiles>(TileKind::Raster);
    world.resources.insert(sources);
    world
        .resources
        .insert(LoadedTiles(loaded.into_iter().collect()));
    select_targets_as(
        std::iter::once(target),
        &world,
        &[("relief".into(), covering)],
        arrivals,
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

#[test]
fn a_view_panning_over_loaded_children_keeps_the_whole_set() {
    let target = WorldTileCoords::from((2423, 1389, 12_u8.into()));
    let grandchildren: Vec<_> = target
        .get_children()
        .iter()
        .flat_map(|child| child.get_children())
        .collect();
    let whole: HashSet<_> = grandchildren.iter().copied().collect();
    // Every window of the view over the terrain tile, from one tile to all of them.
    for start in 0..grandchildren.len() {
        for end in start + 1..=grandchildren.len() {
            let selected = selected(
                target,
                grandchildren[start..end].to_vec(),
                grandchildren.clone(),
            );
            assert_eq!(selected.len(), whole.len(), "window {start}..{end}");
            assert_eq!(selected.into_iter().collect::<HashSet<_>>(), whole);
        }
    }
}

#[test]
fn a_partly_loaded_level_still_follows_the_view() {
    let target = WorldTileCoords::from((2423, 1389, 12_u8.into()));
    let children = target.get_children();
    let mut loaded = children[..3].to_vec();
    loaded.push(target);
    assert_eq!(
        selected(target, children[..2].to_vec(), loaded)
            .into_iter()
            .collect::<HashSet<_>>(),
        [target, children[0], children[1]].into_iter().collect()
    );
}

/// An eye in flight may never hold every tile it wants under a terrain tile, so a tile that
/// has arrived must not wait for the rest to show.
#[test]
fn an_eyes_mixed_lod_covering_shows_each_tile_over_its_ancestor_as_it_arrives() {
    let target = WorldTileCoords::from((2423, 1389, 12_u8.into()));
    let ancestor = target.get_parent().expect("ancestor");
    let children = target.get_children();
    let mut covering = children[..3].to_vec();
    covering.extend(children[3].get_children());
    for count in 0..=covering.len() {
        let mut loaded = covering[..count].to_vec();
        loaded.push(ancestor);
        let selected = selected_as(target, covering.clone(), loaded, Arrivals::OverStandIn);
        let expected: HashSet<_> = if count == covering.len() {
            covering.iter().copied().collect()
        } else {
            covering[..count]
                .iter()
                .copied()
                .chain([ancestor])
                .collect()
        };
        if count < covering.len() {
            assert_eq!(
                selected.first(),
                Some(&ancestor),
                "the stand-in is drawn first"
            );
        }
        assert_eq!(selected.len(), expected.len(), "{count} loaded");
        assert_eq!(
            selected.into_iter().collect::<HashSet<_>>(),
            expected,
            "{count} loaded"
        );
    }
}

#[test]
fn an_eyes_partial_children_without_an_ancestor_wait_for_complete_coverage() {
    let target = WorldTileCoords::from((2423, 1389, 12_u8.into()));
    let children = target.get_children();
    assert!(selected_as(
        target,
        children.to_vec(),
        children[..3].to_vec(),
        Arrivals::OverStandIn
    )
    .is_empty());
}

#[cfg(feature = "raster")]
#[test]
fn missing_children_wait_for_sparse_fallback_ancestor_before_settling() {
    let ancestor = WorldTileCoords::from((75, 96, 8_u8.into()));
    let target = WorldTileCoords::from((300, 384, 10_u8.into()));
    let children = target.get_children();

    let mut world = World::default();
    let mut sources = ViewTileSources::default();
    sources.add_resource_query::<&LoadedTiles>(TileKind::Raster);
    world.resources.insert(sources);
    world.resources.insert(LoadedTiles(HashSet::new()));

    let src = RasterSourceId::from("vfr_sectional");
    for child in &children {
        let mut comp = crate::raster::RasterLayersDataComponent::default();
        comp.record(crate::raster::RasterLayerData::Missing(
            crate::raster::MissingRasterLayerData {
                coords: *child,
                source: src.clone(),
            },
        ));
        world.tiles.spawn_mut(*child).expect("spawn").insert(comp);
    }

    // While z10 / z9 fallback ancestors have neither loaded nor returned 404 yet, the drape must
    // NOT prematurely declare the source absent.
    let pending_shapes: Vec<_> = select_targets(
        std::iter::once(target),
        &world,
        &[(src.clone(), children.to_vec())],
    )
    .pop()
    .expect("target")
    .1
    .into_iter()
    .filter(|shape| shape.raster_source.is_some())
    .collect();
    assert!(
        pending_shapes.is_empty(),
        "must wait for fallback walk, got {pending_shapes:?}"
    );

    // Once z10 and z9 return 404 and z8 loads, the drape settles onto z8 with view_complete = true.
    for z9_or_z10 in [target, target.get_parent().expect("z9")] {
        let mut comp = crate::raster::RasterLayersDataComponent::default();
        comp.record(crate::raster::RasterLayerData::Missing(
            crate::raster::MissingRasterLayerData {
                coords: z9_or_z10,
                source: src.clone(),
            },
        ));
        world
            .tiles
            .spawn_mut(z9_or_z10)
            .expect("spawn")
            .insert(comp);
    }
    world
        .resources
        .insert(LoadedTiles([ancestor].into_iter().collect()));
    let settled_shapes: Vec<_> =
        select_targets(std::iter::once(target), &world, &[(src, children.to_vec())])
            .pop()
            .expect("target")
            .1
            .into_iter()
            .filter(|shape| shape.raster_source.is_some())
            .collect();
    assert_eq!(settled_shapes.len(), 1);
    assert_eq!(settled_shapes[0].coords, ancestor);
    assert!(settled_shapes[0].view_complete);
    assert!(!settled_shapes[0].absent);
}
