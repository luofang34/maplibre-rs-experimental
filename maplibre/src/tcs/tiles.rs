//! Tile records, their typed CPU components and rendered-feature query geometry.

use std::{any::TypeId, collections::BTreeMap};

use downcast_rs::{impl_downcast, Downcast};
use rustc_hash::FxHashMap;

use crate::{
    coords::{Quadkey, WorldTileCoords},
    io::geometry_index::GeometryIndex,
    tcs::{disjoint::DisjointSliceBorrow, pyramid::TilePyramid},
};

static EMPTY_PYRAMID: TilePyramid<()> = TilePyramid::new();

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
#[derive(Default)]
pub struct Tiles {
    /// Tile records keyed by canonical quadkey.
    pub tiles: BTreeMap<Quadkey, Tile>,
    /// Dense component slots under each tile's key, indexed by [`Self::component_indices`].
    /// Use [`TileSpawnResult::insert`] to preserve one component per type and release replaced values.
    pub components: FxHashMap<Quadkey, Vec<Option<Box<dyn TileComponent>>>>,
    /// Geometry available to rendered-feature queries; cleared with the owning tiles.
    pub geometry_index: GeometryIndex,
    pyramid: TilePyramid<()>,
    component_pyramids: FxHashMap<TypeId, TilePyramid<()>>,
    component_indices: FxHashMap<TypeId, usize>,
}

impl Tiles {
    /// Hierarchical pyramid of every resident tile in this store.
    #[inline]
    pub fn pyramid(&self) -> &TilePyramid<()> {
        &self.pyramid
    }

    /// Hierarchical pyramid of resident tiles carrying component `T`.
    #[inline]
    pub fn pyramid_of<T: TileComponent>(&self) -> &TilePyramid<()> {
        self.component_pyramids
            .get(&TypeId::of::<T>())
            .unwrap_or(&EMPTY_PYRAMID)
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
    pub fn spawn_mut(&mut self, coords: WorldTileCoords) -> Option<TileSpawnResult<'_>> {
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
        self.geometry_index = GeometryIndex::default();
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
        self.geometry_index.remove(&coords);
        self.tiles.remove(&key).is_some()
    }

    fn get_component<T: TileComponent>(&self, tile: Tile) -> Option<&T> {
        let index = *self.component_indices.get(&TypeId::of::<T>())?;
        let slots = self.components.get(&tile.coords.build_quad_key()?)?;
        slots.get(index)?.as_ref()?.downcast_ref()
    }

    fn get_component_mut<T: TileComponent>(&mut self, tile: Tile) -> Option<&mut T> {
        let index = *self.component_indices.get(&TypeId::of::<T>())?;
        let slots = self.components.get_mut(&tile.coords.build_quad_key()?)?;
        slots.get_mut(index)?.as_mut()?.downcast_mut()
    }
}

/// Exclusive insertion handle for a valid tile; chained inserts share the same tile record.
pub struct TileSpawnResult<'t> {
    tiles: &'t mut Tiles,
    tile: Tile,
}

impl TileSpawnResult<'_> {
    /// Replaces this tile's component of the same type, dropping its old value immediately.
    pub fn insert<T: TileComponent>(&mut self, component: T) -> &mut Self {
        if let Some(key) = self.tile.coords.build_quad_key() {
            let type_id = TypeId::of::<T>();
            let next_index = self.tiles.component_indices.len();
            let slot_index = *self
                .tiles
                .component_indices
                .entry(type_id)
                .or_insert(next_index);
            let slots = self.tiles.components.entry(key).or_default();
            if slots.len() <= slot_index {
                slots.resize_with(slot_index + 1, || None);
            }
            if let Some(existing) = slots[slot_index]
                .as_mut()
                .and_then(|stored| stored.downcast_mut::<T>())
            {
                *existing = component;
            } else {
                slots[slot_index] = Some(Box::new(component));
                self.tiles
                    .component_pyramids
                    .entry(type_id)
                    .or_default()
                    .insert(self.tile.coords, ());
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
    fn query(tiles: &Tiles, tile: Tile) -> Option<Self::Item<'_>>;
}

impl<T: TileComponent> ComponentQuery for &T {
    type Item<'t> = &'t T;

    fn query(tiles: &Tiles, tile: Tile) -> Option<Self::Item<'_>> {
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
    fn query_mut(tiles: &mut Tiles, tile: Tile) -> Option<Self::MutItem<'_>> {
        let key = tile.coords.build_quad_key()?;
        let slots = tiles.components.get_mut(&key)?;
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
        let index = *component_indices.get(&TypeId::of::<T>())?;
        slice.borrow_shared(index)?.as_ref()?.downcast_ref()
    }

    fn query_mut(tiles: &mut Tiles, tile: Tile) -> Option<Self::MutItem<'_>> {
        tiles.get_component::<T>(tile)
    }
}

impl<T: TileComponent> ComponentQueryMut for &mut T {
    type MutItem<'t> = &'t mut T;

    fn borrow<'t>(
        component_indices: &FxHashMap<TypeId, usize>,
        slice: &mut DisjointSliceBorrow<'t, Option<Box<dyn TileComponent>>>,
    ) -> Option<Self::MutItem<'t>> {
        let index = *component_indices.get(&TypeId::of::<T>())?;
        slice.borrow_mut(index)?.as_mut()?.downcast_mut()
    }

    fn query_mut(tiles: &mut Tiles, tile: Tile) -> Option<Self::MutItem<'_>> {
        tiles.get_component_mut::<T>(tile)
    }
}

macro_rules! impl_component_query {
    ($($param: ident),*) => {
        impl<$($param: ComponentQuery),*> ComponentQuery for ($($param,)*) {
            type Item<'t> = ($($param::Item<'t>,)*);

            fn query(tiles: &Tiles, tile: Tile) -> Option<Self::Item<'_>> {
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
