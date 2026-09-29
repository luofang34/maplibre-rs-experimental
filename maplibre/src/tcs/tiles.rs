use std::{
    cell::UnsafeCell,
    collections::{btree_map, BTreeMap},
};

use downcast_rs::{impl_downcast, Downcast};

pub use crate::tcs::{EphemeralQueryState, GlobalQueryState, QueryState};
use crate::{
    coords::{Quadkey, WorldTileCoords},
    io::geometry_index::GeometryIndex,
};

#[derive(Copy, Clone, Debug)]
pub struct Tile {
    pub coords: WorldTileCoords,
}

/// Data associated with a [`Tile`], with at most one component of each type per tile.
pub trait TileComponent: Downcast + 'static {}
impl_downcast!(TileComponent);

#[derive(Default)]
pub struct Tiles {
    pub tiles: BTreeMap<Quadkey, Tile>,
    pub components: BTreeMap<Quadkey, Vec<UnsafeCell<Box<dyn TileComponent>>>>,
    pub geometry_index: GeometryIndex,
}

impl Tiles {
    pub fn query<Q: ComponentQuery>(&self, coords: WorldTileCoords) -> Option<Q::Item<'_>> {
        let mut global_state = GlobalQueryState::default();
        let state = <Q::State<'_> as QueryState>::create(&mut global_state);
        Q::query(self, Tile { coords }, state)
    }

    /// Borrows components of one tile, or returns `None` for missing data or conflicting
    /// reference types. Repeated shared references are allowed; mutable ones must be disjoint.
    pub fn query_mut<Q: ComponentQueryMut>(
        &mut self,
        coords: WorldTileCoords,
    ) -> Option<Q::MutItem<'_>> {
        let mut global_state = GlobalQueryState::default();
        let state = <Q::State<'_> as QueryState>::create(&mut global_state);
        Q::query_mut(self, Tile { coords }, state)
    }

    pub fn exists(&self, coords: WorldTileCoords) -> bool {
        if let Some(key) = coords.build_quad_key() {
            self.tiles.contains_key(&key)
        } else {
            false
        }
    }

    pub fn spawn_mut(&mut self, coords: WorldTileCoords) -> Option<TileSpawnResult<'_>> {
        if let Some(key) = coords.build_quad_key() {
            if let Some(tile) = self.tiles.get(&key) {
                let tile = *tile;
                Some(TileSpawnResult { tiles: self, tile })
            } else {
                let tile = Tile { coords };
                self.tiles.insert(key, tile);
                self.components.insert(key, Vec::new());
                Some(TileSpawnResult { tiles: self, tile })
            }
        } else {
            None
        }
    }

    pub fn clear(&mut self) {
        self.tiles.clear();
        self.components.clear();
    }

    /// Drops a tile with its components and query index; `false` when it was not present.
    pub fn remove(&mut self, coords: WorldTileCoords) -> bool {
        let Some(key) = coords.build_quad_key() else {
            return false;
        };
        self.components.remove(&key);
        self.geometry_index.remove(&coords);
        self.tiles.remove(&key).is_some()
    }
}

pub struct TileSpawnResult<'t> {
    tiles: &'t mut Tiles,
    tile: Tile,
}

impl<'w> TileSpawnResult<'w> {
    pub fn insert<T: TileComponent>(&mut self, component: T) -> &mut Self {
        let components = &mut self.tiles.components;
        let coords = self.tile.coords;

        if let Some(entry) = coords.build_quad_key().map(|key| components.entry(key)) {
            match entry {
                btree_map::Entry::Vacant(_entry) => {
                    panic!("Can not add a component at {coords}. Entity does not exist.",)
                }
                btree_map::Entry::Occupied(mut entry) => {
                    entry.get_mut().push(UnsafeCell::new(Box::new(component)));
                }
            }
        }
        self
    }
}

// ComponentQuery

pub trait ComponentQuery {
    type Item<'t>;

    type State<'s>: QueryState<'s>;

    fn query<'t, 's>(
        tiles: &'t Tiles,
        tile: Tile,
        state: Self::State<'s>,
    ) -> Option<Self::Item<'t>>;
}

impl<T: TileComponent> ComponentQuery for &T {
    type Item<'t> = &'t T;
    type State<'s> = EphemeralQueryState<'s>;

    fn query<'t, 's>(
        tiles: &'t Tiles,
        tile: Tile,
        mut state: Self::State<'s>,
    ) -> Option<Self::Item<'t>> {
        state.borrow_shared::<T>()?;
        let components = tiles.components.get(&tile.coords.build_quad_key()?)?;
        for (index, component) in components.iter().enumerate() {
            if state.state.mutably_borrowed_components.contains(&index) {
                continue;
            }
            // Even checking a trait object's type borrows its value. Mutable slots must
            // be skipped before dereferencing, including slots of a different type.
            if let Some(value) = unsafe { (&*component.get()).downcast_ref::<T>() } {
                return Some(value);
            }
        }
        None
    }
}

// ComponentQueryMut

pub trait ComponentQueryMut {
    type MutItem<'t>;

    type State<'s>: QueryState<'s>;

    fn query_mut<'t, 's>(
        tiles: &'t mut Tiles,
        tile: Tile,
        state: Self::State<'s>,
    ) -> Option<Self::MutItem<'t>>;
}

impl<T: TileComponent> ComponentQueryMut for &T {
    type MutItem<'t> = &'t T;
    type State<'s> = EphemeralQueryState<'s>;

    fn query_mut<'t, 's>(
        tiles: &'t mut Tiles,
        tile: Tile,
        state: Self::State<'s>,
    ) -> Option<Self::MutItem<'t>> {
        <&T as ComponentQuery>::query(tiles, tile, state)
    }
}

impl<T: TileComponent> ComponentQueryMut for &mut T {
    type MutItem<'t> = &'t mut T;
    type State<'s> = EphemeralQueryState<'s>;

    fn query_mut<'t, 's>(
        tiles: &'t mut Tiles,
        tile: Tile,
        _state: Self::State<'s>,
    ) -> Option<Self::MutItem<'t>> {
        let components = tiles.components.get_mut(&tile.coords.build_quad_key()?)?;

        components
            .iter_mut()
            .find_map(|component| component.get_mut().downcast_mut())
    }
}

// ComponentQueryUnsafe

/// Sealed component references used by mutable tuple queries.
///
/// Implementations share the tuple's borrow state and cannot inspect slots already borrowed
/// mutably by another member of the tuple.
///
/// ```compile_fail
/// use maplibre::tcs::{EphemeralQueryState, tiles::{Tiles, Tile, ComponentQueryMut, ComponentQueryUnsafe}};
/// struct Custom;
/// impl ComponentQueryMut for Custom {
///     type MutItem<'t> = &'t u32;
///     type State<'s> = EphemeralQueryState<'s>;
///     fn query_mut<'t, 's>(_: &'t mut Tiles, _: Tile, _: Self::State<'s>) -> Option<&'t u32> { None }
/// }
/// impl ComponentQueryUnsafe for Custom {
///     unsafe fn query_unsafe<'t, 's>(_: &'t Tiles, _: Tile, _: Self::State<'s>) -> Option<&'t u32> { None }
/// }
/// ```
pub trait ComponentQueryUnsafe: ComponentQueryMut + sealed::Reference {
    /// # Safety
    /// The caller must prevent overlapping mutable borrows of the queried tile components
    /// for the lifetime of the returned references, including across query states.
    unsafe fn query_unsafe<'t, 's>(
        tiles: &'t Tiles,
        tile: Tile,
        state: Self::State<'s>,
    ) -> Option<Self::MutItem<'t>>;
}

impl<T: TileComponent> ComponentQueryUnsafe for &T {
    unsafe fn query_unsafe<'t, 's>(
        tiles: &'t Tiles,
        tile: Tile,
        state: Self::State<'s>,
    ) -> Option<Self::MutItem<'t>> {
        <&T as ComponentQuery>::query(tiles, tile, state)
    }
}

impl<T: TileComponent> ComponentQueryUnsafe for &mut T {
    unsafe fn query_unsafe<'t, 's>(
        tiles: &'t Tiles,
        tile: Tile,
        mut state: Self::State<'s>,
    ) -> Option<Self::MutItem<'t>> {
        state.borrow_mut::<T>()?;
        let components = tiles.components.get(&tile.coords.build_quad_key()?)?;
        for (index, component) in components.iter().enumerate() {
            if state.state.mutably_borrowed_components.contains(&index) {
                continue;
            }
            // Type inspection is shared only until the test ends. No prior mutable slot
            // is inspected, and the type check excludes prior shared references to this slot.
            if unsafe { (&*component.get()).is::<T>() } {
                state.state.mutably_borrowed_components.insert(index);
                return unsafe { (&mut *component.get()).downcast_mut() };
            }
        }
        None
    }
}

// Lift to tuples

impl<CQ1: ComponentQuery, CQ2: ComponentQuery> ComponentQuery for (CQ1, CQ2) {
    type Item<'t> = (CQ1::Item<'t>, CQ2::Item<'t>);
    type State<'s> = EphemeralQueryState<'s>;

    fn query<'t, 's>(
        tiles: &'t Tiles,
        tile: Tile,
        mut state: Self::State<'s>,
    ) -> Option<Self::Item<'t>> {
        Some((
            CQ1::query(tiles, tile, state.clone_to::<CQ1::State<'_>>())?,
            CQ2::query(tiles, tile, state.clone_to::<CQ2::State<'_>>())?,
        ))
    }
}

impl<
        CQ1: ComponentQueryMut + ComponentQueryUnsafe + 'static,
        CQ2: ComponentQueryMut + ComponentQueryUnsafe + 'static,
    > ComponentQueryMut for (CQ1, CQ2)
{
    type MutItem<'t> = (CQ1::MutItem<'t>, CQ2::MutItem<'t>);
    type State<'s> = EphemeralQueryState<'s>;

    fn query_mut<'t, 's>(
        tiles: &'t mut Tiles,
        tile: Tile,
        mut state: Self::State<'s>,
    ) -> Option<Self::MutItem<'t>> {
        // Sealed reference queries share one access ledger under an exclusive store borrow.
        unsafe {
            Some((
                <CQ1 as ComponentQueryUnsafe>::query_unsafe(
                    tiles,
                    tile,
                    state.clone_to::<CQ1::State<'_>>(),
                )?,
                <CQ2 as ComponentQueryUnsafe>::query_unsafe(
                    tiles,
                    tile,
                    state.clone_to::<CQ2::State<'_>>(),
                )?,
            ))
        }
    }
}

mod sealed {
    pub trait Reference {}
    impl<T: super::TileComponent> Reference for &T {}
    impl<T: super::TileComponent> Reference for &mut T {}
}

#[cfg(test)]
mod tests;
