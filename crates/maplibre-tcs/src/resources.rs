//! One owned map-wide value per concrete Rust type, with checked tuple borrowing.

use std::any::TypeId;

use downcast_rs::{impl_downcast, Downcast};
use rustc_hash::FxHashMap;

use crate::disjoint::DisjointSliceBorrow;

/// A map-wide value that can be recovered by its concrete type.
/// Every `'static` type implements this trait; resources need not be `Send` or `Sync`.
pub trait Resource: Downcast + 'static {}
impl_downcast!(Resource);

impl<T> Resource for T where T: 'static {}

/// Owns resources until replacement, removal, or store destruction.
/// Returned references borrow the store, preventing insertion while those references are used.
#[derive(Default)]
pub struct Resources {
    resources: Vec<Box<dyn Resource>>,
    types: Vec<TypeId>,
    index: FxHashMap<TypeId, usize>,
}

#[inline(never)]
fn lookup_slot(index: &FxHashMap<TypeId, usize>, type_id: TypeId) -> Option<usize> {
    index.get(&type_id).copied()
}

impl Resources {
    #[inline(never)]
    fn get_or_init_boxed(
        &mut self,
        type_id: TypeId,
        make: fn() -> Box<dyn Resource>,
    ) -> &mut Box<dyn Resource> {
        let index = match self.index.get(&type_id).copied() {
            Some(index) if index < self.resources.len() => index,
            _ => {
                let index = self.resources.len();
                self.resources.push(make());
                self.types.push(type_id);
                self.index.insert(type_id, index);
                index
            }
        };
        if self.types[index] != type_id {
            self.resources[index] = make();
            self.types[index] = type_id;
        }
        &mut self.resources[index]
    }

    #[inline(never)]
    fn insert_boxed(&mut self, type_id: TypeId, boxed: Box<dyn Resource>) {
        match self.index.get(&type_id).copied() {
            Some(index) => {
                self.resources[index] = boxed;
                self.types[index] = type_id;
            }
            None => {
                let index = self.resources.len();
                self.resources.push(boxed);
                self.types.push(type_id);
                self.index.insert(type_id, index);
            }
        }
    }

    #[inline(never)]
    fn remove_boxed(&mut self, type_id: TypeId) -> Option<Box<dyn Resource>> {
        let index = self.index.remove(&type_id)?;
        let removed = self.resources.swap_remove(index);
        self.types.swap_remove(index);
        if let Some(&moved_type_id) = self.types.get(index) {
            self.index.insert(moved_type_id, index);
        }
        Some(removed)
    }

    #[inline(never)]
    fn exists_id(&self, type_id: TypeId) -> bool {
        self.index.contains_key(&type_id)
    }

    #[inline(never)]
    fn get_boxed(&self, type_id: TypeId) -> Option<&dyn Resource> {
        let index = *self.index.get(&type_id)?;
        self.resources.get(index).map(Box::as_ref)
    }

    #[inline(never)]
    fn get_boxed_mut(&mut self, type_id: TypeId) -> Option<&mut dyn Resource> {
        let index = *self.index.get(&type_id)?;
        self.resources.get_mut(index).map(Box::as_mut)
    }

    /// Installs a default value, replacing and dropping any existing value of the same type.
    pub fn init<R: Resource + Default>(&mut self) {
        self.insert(R::default());
    }

    /// Borrows the existing value, installing its default only when the type is absent.
    pub fn get_or_init_mut<R: Resource + Default>(&mut self) -> &mut R {
        fn make_default<R: Resource + Default>() -> Box<dyn Resource> {
            Box::new(R::default())
        }
        let slot = self.get_or_init_boxed(TypeId::of::<R>(), make_default::<R>);
        let Some(value) = slot.downcast_mut::<R>() else {
            unreachable!("slot initialized with R")
        };
        value
    }

    /// Stores a resource, dropping the one of the same type stored before. Systems store
    /// per-frame resources this way, so appending instead of replacing would keep every
    /// frame's value, and its GPU buffers, for the life of the map.
    pub fn insert<R: Resource>(&mut self, resource: R) {
        self.insert_boxed(TypeId::of::<R>(), Box::new(resource));
    }

    /// Removes and returns the stored value of this concrete type, or `None` when absent.
    pub fn remove<R: Resource>(&mut self) -> Option<R> {
        let removed = self.remove_boxed(TypeId::of::<R>())?;
        removed.downcast::<R>().ok().map(|boxed| *boxed)
    }

    /// Whether a value of exactly this concrete type is stored.
    pub fn exists<R: Resource>(&self) -> bool {
        self.exists_id(TypeId::of::<R>())
    }

    /// Borrows a value by concrete type, or returns `None` when absent.
    pub fn get<R: Resource>(&self) -> Option<&R> {
        self.get_boxed(TypeId::of::<R>())?.downcast_ref()
    }

    /// Exclusively borrows a value by concrete type, or returns `None` when absent.
    pub fn get_mut<R: Resource>(&mut self) -> Option<&mut R> {
        self.get_boxed_mut(TypeId::of::<R>())?.downcast_mut()
    }

    /// Borrows `&R` or tuples of one through six shared resource references.
    /// Built-in queries return `None` when any requested type is absent.
    pub fn query<Q: ResourceQuery>(&self) -> Option<Q::Item<'_>> {
        Q::query(self)
    }

    /// Borrows the requested resources, or returns `None` for a missing resource or alias
    /// conflict. Repeated shared references are allowed; mutable references must be disjoint.
    pub fn query_mut<Q: ResourceQueryMut>(&mut self) -> Option<Q::MutItem<'_>> {
        Q::query_mut(self)
    }
}

/// Shared resource lookup; built-in queries are references and tuples of up to six members.
pub trait ResourceQuery {
    /// Value returned with a lifetime bounded by the resource store.
    type Item<'r>;

    /// Looks up resources in `resources`.
    fn query(resources: &Resources) -> Option<Self::Item<'_>>;
}

impl<R: Resource> ResourceQuery for &R {
    type Item<'r> = &'r R;

    fn query(resources: &Resources) -> Option<Self::Item<'_>> {
        resources.get::<R>()
    }
}

/// Resource lookup with exclusive access to the store while constructing the result.
/// Built-in tuples may mix shared and mutable references to disjoint resource types.
pub trait ResourceQueryMut {
    /// Value returned with a lifetime bounded by the exclusive store borrow.
    type MutItem<'r>;

    /// Borrows this query's item(s) from the store's index and disjoint slice borrow.
    fn borrow<'r>(
        index: &FxHashMap<TypeId, usize>,
        slice: &mut DisjointSliceBorrow<'r, Box<dyn Resource>>,
    ) -> Option<Self::MutItem<'r>>;

    /// Looks up resources; built-in queries return `None` on absence or conflicting access.
    fn query_mut(resources: &mut Resources) -> Option<Self::MutItem<'_>> {
        let mut slice = DisjointSliceBorrow::new(&mut resources.resources);
        Self::borrow(&resources.index, &mut slice)
    }
}

impl<R: Resource> ResourceQueryMut for &R {
    type MutItem<'r> = &'r R;

    fn borrow<'r>(
        index: &FxHashMap<TypeId, usize>,
        slice: &mut DisjointSliceBorrow<'r, Box<dyn Resource>>,
    ) -> Option<Self::MutItem<'r>> {
        let slot = lookup_slot(index, TypeId::of::<R>())?;
        slice.borrow_shared(slot)?.downcast_ref()
    }

    fn query_mut(resources: &mut Resources) -> Option<Self::MutItem<'_>> {
        resources.get::<R>()
    }
}

impl<R: Resource> ResourceQueryMut for &mut R {
    type MutItem<'r> = &'r mut R;

    fn borrow<'r>(
        index: &FxHashMap<TypeId, usize>,
        slice: &mut DisjointSliceBorrow<'r, Box<dyn Resource>>,
    ) -> Option<Self::MutItem<'r>> {
        let slot = lookup_slot(index, TypeId::of::<R>())?;
        slice.borrow_mut(slot)?.downcast_mut()
    }

    fn query_mut(resources: &mut Resources) -> Option<Self::MutItem<'_>> {
        resources.get_mut::<R>()
    }
}

macro_rules! impl_resource_query {
    ($($param: ident),*) => {
        impl<$($param: ResourceQuery),*> ResourceQuery for ($($param,)*) {
            type Item<'r> = ($($param::Item<'r>,)*);

            fn query(resources: &Resources) -> Option<Self::Item<'_>> {
                Some(($($param::query(resources)?,)*))
            }
        }

        impl<$($param: ResourceQueryMut),*> ResourceQueryMut for ($($param,)*) {
            type MutItem<'r> = ($($param::MutItem<'r>,)*);

            fn borrow<'r>(
                index: &FxHashMap<TypeId, usize>,
                slice: &mut DisjointSliceBorrow<'r, Box<dyn Resource>>,
            ) -> Option<Self::MutItem<'r>> {
                Some(($($param::borrow(index, slice)?,)*))
            }
        }
    };
}

impl_resource_query!(R1);
impl_resource_query!(R1, R2);
impl_resource_query!(R1, R2, R3);
impl_resource_query!(R1, R2, R3, R4);
impl_resource_query!(R1, R2, R3, R4, R5);
impl_resource_query!(R1, R2, R3, R4, R5, R6);

#[cfg(test)]
mod tests;
