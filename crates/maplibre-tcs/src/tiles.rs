//! Tile records, their typed CPU components, and optional feature-query storage.

use std::{any::TypeId, collections::BTreeMap};

use downcast_rs::{impl_downcast, Downcast};
use maplibre_coords::{Quadkey, WorldTileCoords};
use rustc_hash::FxHashMap;

use crate::{disjoint::DisjointSliceBorrow, pyramid::TilePyramid};

static EMPTY_PYRAMID: TilePyramid<()> = TilePyramid::new();

/// Per-tile geometry storage maintained in lockstep with [`Tiles`] mutations.
pub trait TileGeometryStore: Default {
    /// Drops any indexed geometry belonging to `coords` when that tile leaves the store.
    fn remove_tile(&mut self, coords: &WorldTileCoords);
}

impl TileGeometryStore for () {
    fn remove_tile(&mut self, _coords: &WorldTileCoords) {}
}

/// Coordinates identifying one tile record in a [`Tiles`] store.
#[derive(Copy, Clone, Debug)]
pub struct Tile {
    /// Canonical coordinates used to build the store's quadkey.
    pub coords: WorldTileCoords,
}

/// Data associated with a [`Tile`], with at most one component of each type per tile.
pub trait TileComponent: Downcast + 'static {
    /// Concrete [`TypeId`] of this component value, used for per-component pyramid indexing.
    fn component_type_id(&self) -> TypeId {
        TypeId::of::<Self>()
    }
}
impl_downcast!(TileComponent);

/// Owns tile records, typed components and their feature-query index.
/// Component references borrow this store and exclude mutation of its tile membership.
pub struct Tiles<G: TileGeometryStore = ()> {
    /// Tile records keyed by canonical quadkey.
    pub tiles: BTreeMap<Quadkey, Tile>,
    /// Dense component slots under each tile's key, indexed by registered component type ID.
    /// Use [`TileSpawnResult::insert`] to preserve one component per type and release replaced values.
    pub components: FxHashMap<Quadkey, Vec<Option<Box<dyn TileComponent>>>>,
    /// Geometry available to rendered-feature queries; cleared with the owning tiles.
    pub geometry_index: G,
    pyramid: TilePyramid<()>,
    component_pyramids: FxHashMap<TypeId, TilePyramid<()>>,
    component_indices: FxHashMap<TypeId, usize>,
}

impl<G: TileGeometryStore> Default for Tiles<G> {
    fn default() -> Self {
        Self {
            tiles: BTreeMap::default(),
            components: FxHashMap::default(),
            geometry_index: G::default(),
            pyramid: TilePyramid::default(),
            component_pyramids: FxHashMap::default(),
            component_indices: FxHashMap::default(),
        }
    }
}

impl<G: TileGeometryStore> Tiles<G> {
    /// Hierarchical pyramid of every resident tile in this store.
    #[inline]
    pub fn pyramid(&self) -> &TilePyramid<()> {
        &self.pyramid
    }

    /// Hierarchical pyramid of resident tiles carrying component `T`.
    #[inline]
    pub fn pyramid_of<T: TileComponent>(&self) -> &TilePyramid<()> {
        pyramid_by_id(&self.component_pyramids, TypeId::of::<T>())
    }

    /// Borrows `&T` or a tuple of shared component references from one tile.
    /// Built-in queries return `None` for invalid coordinates or missing components.
    pub fn query<Q: ComponentQuery>(&self, coords: WorldTileCoords) -> Option<Q::Item<'_>> {
        Q::query(self, Tile { coords })
    }

    /// Borrows components of one tile, or returns `None` for missing data or conflicting
    /// reference types. Repeated shared references are allowed; mutable ones must be disjoint.
    pub fn query_mut<Q: ComponentQueryMut>(
        &mut self,
        coords: WorldTileCoords,
    ) -> Option<Q::MutItem<'_>> {
        Q::query_mut(self, Tile { coords })
    }

    /// Whether a record exists at these coordinates; invalid coordinates return `false`.
    #[inline]
    pub fn exists(&self, coords: WorldTileCoords) -> bool {
        coords
            .build_quad_key()
            .is_some_and(|key| self.components.contains_key(&key))
    }

    /// Opens a tile for component insertion, preserving its components if it already exists.
    /// Coordinates outside the canonical tile grid return `None` without changing the store.
    pub fn spawn_mut(&mut self, coords: WorldTileCoords) -> Option<TileSpawnResult<'_, G>> {
        if let Some(key) = coords.build_quad_key() {
            if self.components.contains_key(&key) {
                Some(TileSpawnResult {
                    tiles: self,
                    tile: Tile { coords },
                })
            } else {
                let tile = Tile { coords };
                self.tiles.insert(key, tile);
                self.components.insert(key, Vec::new());
                self.pyramid.insert(coords, ());
                Some(TileSpawnResult { tiles: self, tile })
            }
        } else {
            None
        }
    }

    /// Drops every tile, component and feature-query geometry.
    pub fn clear(&mut self) {
        self.tiles.clear();
        self.components.clear();
        self.pyramid.clear();
        for pyramid in self.component_pyramids.values_mut() {
            pyramid.clear();
        }
        self.geometry_index = G::default();
    }

    /// Drops a tile with its components and query index; `false` when it was not present.
    pub fn remove(&mut self, coords: WorldTileCoords) -> bool {
        let Some(key) = coords.build_quad_key() else {
            return false;
        };
        if let Some(removed_components) = self.components.remove(&key) {
            for component in removed_components.into_iter().flatten() {
                let type_id = component.component_type_id();
                if let Some(pyramid) = self.component_pyramids.get_mut(&type_id) {
                    pyramid.remove(coords);
                }
            }
        }
        self.pyramid.remove(coords);
        self.geometry_index.remove_tile(&coords);
        self.tiles.remove(&key).is_some()
    }

    fn get_component<T: TileComponent>(&self, tile: Tile) -> Option<&T> {
        get_component_boxed(
            &self.component_indices,
            &self.components,
            tile.coords,
            TypeId::of::<T>(),
        )?
        .downcast_ref()
    }

    fn get_component_mut<T: TileComponent>(&mut self, tile: Tile) -> Option<&mut T> {
        get_component_boxed_mut(
            &self.component_indices,
            &mut self.components,
            tile.coords,
            TypeId::of::<T>(),
        )?
        .downcast_mut()
    }
}

#[inline(never)]
fn pyramid_by_id(
    pyramids: &FxHashMap<TypeId, TilePyramid<()>>,
    type_id: TypeId,
) -> &TilePyramid<()> {
    pyramids.get(&type_id).unwrap_or(&EMPTY_PYRAMID)
}

#[inline(never)]
fn lookup_component_slot(indices: &FxHashMap<TypeId, usize>, type_id: TypeId) -> Option<usize> {
    indices.get(&type_id).copied()
}

#[inline(never)]
fn get_component_boxed<'t>(
    indices: &FxHashMap<TypeId, usize>,
    components: &'t FxHashMap<Quadkey, Vec<Option<Box<dyn TileComponent>>>>,
    coords: WorldTileCoords,
    type_id: TypeId,
) -> Option<&'t dyn TileComponent> {
    let index = *indices.get(&type_id)?;
    let slots = components.get(&coords.build_quad_key()?)?;
    slots.get(index)?.as_deref()
}

#[inline(never)]
fn get_component_boxed_mut<'t>(
    indices: &FxHashMap<TypeId, usize>,
    components: &'t mut FxHashMap<Quadkey, Vec<Option<Box<dyn TileComponent>>>>,
    coords: WorldTileCoords,
    type_id: TypeId,
) -> Option<&'t mut dyn TileComponent> {
    let index = *indices.get(&type_id)?;
    let slots = components.get_mut(&coords.build_quad_key()?)?;
    slots.get_mut(index)?.as_deref_mut()
}

#[inline(never)]
fn borrow_component_slots(
    components: &mut FxHashMap<Quadkey, Vec<Option<Box<dyn TileComponent>>>>,
    coords: WorldTileCoords,
) -> Option<&mut Vec<Option<Box<dyn TileComponent>>>> {
    let key = coords.build_quad_key()?;
    components.get_mut(&key)
}

#[inline(never)]
fn prepare_component_slot<'t>(
    indices: &mut FxHashMap<TypeId, usize>,
    components: &'t mut FxHashMap<Quadkey, Vec<Option<Box<dyn TileComponent>>>>,
    coords: WorldTileCoords,
    type_id: TypeId,
) -> Option<&'t mut Option<Box<dyn TileComponent>>> {
    let key = coords.build_quad_key()?;
    let next_index = indices.len();
    let slot_index = *indices.entry(type_id).or_insert(next_index);
    let slots = components.entry(key).or_default();
    if slots.len() <= slot_index {
        slots.resize_with(slot_index + 1, || None);
    }
    slots.get_mut(slot_index)
}

#[inline(never)]
fn register_component_pyramid(
    pyramids: &mut FxHashMap<TypeId, TilePyramid<()>>,
    coords: WorldTileCoords,
    type_id: TypeId,
) {
    pyramids.entry(type_id).or_default().insert(coords, ());
}

/// Exclusive insertion handle for a valid tile; chained inserts share the same tile record.
pub struct TileSpawnResult<'t, G: TileGeometryStore = ()> {
    tiles: &'t mut Tiles<G>,
    tile: Tile,
}

impl<G: TileGeometryStore> TileSpawnResult<'_, G> {
    /// Replaces this tile's component of the same type, dropping its old value immediately.
    pub fn insert<T: TileComponent>(&mut self, component: T) -> &mut Self {
        let type_id = TypeId::of::<T>();
        if let Some(slot) = prepare_component_slot(
            &mut self.tiles.component_indices,
            &mut self.tiles.components,
            self.tile.coords,
            type_id,
        ) {
            if let Some(existing) = slot.as_mut().and_then(|stored| stored.downcast_mut::<T>()) {
                *existing = component;
            } else {
                *slot = Some(Box::new(component));
                register_component_pyramid(
                    &mut self.tiles.component_pyramids,
                    self.tile.coords,
                    type_id,
                );
            }
        }
        self
    }
}

/// Shared component lookup; built-in queries are references and tuples of up to six references.
pub trait ComponentQuery {
    /// Value returned with a lifetime bounded by the tile store.
    type Item<'t>;

    /// Looks up components at `tile`.
    fn query<G: TileGeometryStore>(tiles: &Tiles<G>, tile: Tile) -> Option<Self::Item<'_>>;
}

impl<T: TileComponent> ComponentQuery for &T {
    type Item<'t> = &'t T;

    fn query<G: TileGeometryStore>(tiles: &Tiles<G>, tile: Tile) -> Option<Self::Item<'_>> {
        tiles.get_component::<T>(tile)
    }
}

/// Component lookup with exclusive access to the tile store while constructing the result.
/// Built-in tuples may mix shared and mutable references to disjoint component types.
pub trait ComponentQueryMut {
    /// Value returned with a lifetime bounded by the exclusive tile-store borrow.
    type MutItem<'t>;

    /// Borrows this query's component(s) from the tile's disjoint component-slot borrow.
    fn borrow<'t>(
        component_indices: &FxHashMap<TypeId, usize>,
        slice: &mut DisjointSliceBorrow<'t, Option<Box<dyn TileComponent>>>,
    ) -> Option<Self::MutItem<'t>>;

    /// Looks up components; built-in queries return `None` on absence or conflicting access.
    fn query_mut<G: TileGeometryStore>(
        tiles: &mut Tiles<G>,
        tile: Tile,
    ) -> Option<Self::MutItem<'_>> {
        let slots = borrow_component_slots(&mut tiles.components, tile.coords)?;
        let mut slice = DisjointSliceBorrow::new(slots);
        Self::borrow(&tiles.component_indices, &mut slice)
    }
}

impl<T: TileComponent> ComponentQueryMut for &T {
    type MutItem<'t> = &'t T;

    fn borrow<'t>(
        component_indices: &FxHashMap<TypeId, usize>,
        slice: &mut DisjointSliceBorrow<'t, Option<Box<dyn TileComponent>>>,
    ) -> Option<Self::MutItem<'t>> {
        let index = lookup_component_slot(component_indices, TypeId::of::<T>())?;
        slice.borrow_shared(index)?.as_ref()?.downcast_ref()
    }

    fn query_mut<G: TileGeometryStore>(
        tiles: &mut Tiles<G>,
        tile: Tile,
    ) -> Option<Self::MutItem<'_>> {
        tiles.get_component::<T>(tile)
    }
}

impl<T: TileComponent> ComponentQueryMut for &mut T {
    type MutItem<'t> = &'t mut T;

    fn borrow<'t>(
        component_indices: &FxHashMap<TypeId, usize>,
        slice: &mut DisjointSliceBorrow<'t, Option<Box<dyn TileComponent>>>,
    ) -> Option<Self::MutItem<'t>> {
        let index = lookup_component_slot(component_indices, TypeId::of::<T>())?;
        slice.borrow_mut(index)?.as_mut()?.downcast_mut()
    }

    fn query_mut<G: TileGeometryStore>(
        tiles: &mut Tiles<G>,
        tile: Tile,
    ) -> Option<Self::MutItem<'_>> {
        tiles.get_component_mut::<T>(tile)
    }
}

macro_rules! impl_component_query {
    ($($param: ident),*) => {
        impl<$($param: ComponentQuery),*> ComponentQuery for ($($param,)*) {
            type Item<'t> = ($($param::Item<'t>,)*);

            fn query<G: TileGeometryStore>(tiles: &Tiles<G>, tile: Tile) -> Option<Self::Item<'_>> {
                Some(($($param::query(tiles, tile)?,)*))
            }
        }

        impl<$($param: ComponentQueryMut),*> ComponentQueryMut for ($($param,)*) {
            type MutItem<'t> = ($($param::MutItem<'t>,)*);

            fn borrow<'t>(
                component_indices: &FxHashMap<TypeId, usize>,
                slice: &mut DisjointSliceBorrow<'t, Option<Box<dyn TileComponent>>>,
            ) -> Option<Self::MutItem<'t>> {
                Some(($($param::borrow(component_indices, slice)?,)*))
            }
        }
    };
}

impl_component_query!(CQ1);
impl_component_query!(CQ1, CQ2);
impl_component_query!(CQ1, CQ2, CQ3);
impl_component_query!(CQ1, CQ2, CQ3, CQ4);
impl_component_query!(CQ1, CQ2, CQ3, CQ4, CQ5);
impl_component_query!(CQ1, CQ2, CQ3, CQ4, CQ5, CQ6);

#[cfg(test)]
mod tests;
