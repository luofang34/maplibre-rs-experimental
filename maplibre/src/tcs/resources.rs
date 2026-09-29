use std::{any::TypeId, cell::UnsafeCell, collections::HashMap};

use downcast_rs::{impl_downcast, Downcast};

use crate::tcs::{EphemeralQueryState, GlobalQueryState, QueryState};

pub trait Resource: Downcast + 'static {}
impl_downcast!(Resource);

impl<T> Resource for T where T: 'static {}

#[derive(Default)]
pub struct Resources {
    resources: Vec<UnsafeCell<Box<dyn Resource>>>,
    index: HashMap<TypeId, usize>,
}

impl Resources {
    pub fn init<R: Resource + Default>(&mut self) {
        self.insert(R::default());
    }

    pub fn get_or_init_mut<R: Resource + Default>(&mut self) -> &mut R {
        if self.exists::<R>() {
            self.get_mut::<R>()
                .expect("unable get get just initialized resource")
        } else {
            self.init::<R>();
            self.get_mut()
                .expect("unable get get just initialized resource")
        }
    }

    /// Stores a resource, dropping the one of the same type stored before. Systems store
    /// per-frame resources this way, so appending instead of replacing would keep every
    /// frame's value, and its GPU buffers, for the life of the map.
    pub fn insert<R: Resource>(&mut self, resource: R) {
        let cell = UnsafeCell::new(Box::new(resource) as Box<dyn Resource>);
        match self.index.get(&TypeId::of::<R>()) {
            Some(index) => self.resources[*index] = cell,
            None => {
                let index = self.resources.len();
                self.resources.push(cell);
                self.index.insert(TypeId::of::<R>(), index);
            }
        }
    }

    pub fn exists<R: Resource>(&self) -> bool {
        self.index.contains_key(&TypeId::of::<R>())
    }

    pub fn get<R: Resource>(&self) -> Option<&R> {
        let index = *self.index.get(&TypeId::of::<R>())?;
        // Safe callers share the store; mutable tuple queries check type access before
        // calling this method and cannot mutate the resource index during the borrow.
        unsafe { (&*self.resources.get(index)?.get()).downcast_ref() }
    }

    pub fn get_mut<R: Resource>(&mut self) -> Option<&mut R> {
        let index = *self.index.get(&TypeId::of::<R>())?;
        self.resources.get_mut(index)?.get_mut().downcast_mut()
    }

    pub fn query<Q: ResourceQuery>(&self) -> Option<Q::Item<'_>> {
        let mut global_state = GlobalQueryState::default();
        let state = <Q::State<'_> as QueryState>::create(&mut global_state);
        Q::query(self, state)
    }

    /// Borrows the requested resources, or returns `None` for a missing resource or alias
    /// conflict. Repeated shared references are allowed; mutable references must be disjoint.
    pub fn query_mut<Q: ResourceQueryMut>(&mut self) -> Option<Q::MutItem<'_>> {
        let mut global_state = GlobalQueryState::default();
        let state = <Q::State<'_> as QueryState>::create(&mut global_state);
        Q::query_mut(self, state)
    }
}

// ResourceQuery

pub trait ResourceQuery {
    type Item<'r>;

    type State<'s>: QueryState<'s>;

    fn query<'r, 's>(resources: &'r Resources, state: Self::State<'s>) -> Option<Self::Item<'r>>;
}

impl<R: Resource> ResourceQuery for &R {
    type Item<'r> = &'r R;
    type State<'s> = EphemeralQueryState<'s>;

    fn query<'r, 's>(
        resources: &'r Resources,
        mut state: Self::State<'s>,
    ) -> Option<Self::Item<'r>> {
        state.borrow_shared::<R>()?;
        resources.get::<R>()
    }
}

// ResourceQueryMut

pub trait ResourceQueryMut {
    type MutItem<'r>;

    type State<'s>: QueryState<'s>;

    fn query_mut<'r, 's>(
        resources: &'r mut Resources,
        state: Self::State<'s>,
    ) -> Option<Self::MutItem<'r>>;
}

impl<R: Resource> ResourceQueryMut for &R {
    type MutItem<'r> = &'r R;
    type State<'s> = EphemeralQueryState<'s>;

    fn query_mut<'r, 's>(
        resources: &'r mut Resources,
        state: Self::State<'s>,
    ) -> Option<Self::MutItem<'r>> {
        <&R as ResourceQuery>::query(resources, state)
    }
}

impl<R: Resource> ResourceQueryMut for &mut R {
    type MutItem<'r> = &'r mut R;
    type State<'s> = EphemeralQueryState<'s>;

    fn query_mut<'r, 's>(
        resources: &'r mut Resources,
        _state: Self::State<'s>,
    ) -> Option<Self::MutItem<'r>> {
        resources.get_mut::<R>()
    }
}

// ResourceQueryUnsafe

/// Sealed resource references used by mutable tuple queries.
///
/// Implementations must share the tuple's borrow checks, so downstream query types cannot
/// supply an unchecked implementation.
///
/// ```compile_fail
/// use maplibre::tcs::{EphemeralQueryState, resources::{Resources, ResourceQueryMut, ResourceQueryUnsafe}};
/// struct Custom;
/// impl ResourceQueryMut for Custom {
///     type MutItem<'r> = &'r u32;
///     type State<'s> = EphemeralQueryState<'s>;
///     fn query_mut<'r, 's>(_: &'r mut Resources, _: Self::State<'s>) -> Option<&'r u32> { None }
/// }
/// impl ResourceQueryUnsafe for Custom {
///     unsafe fn query_unsafe<'r, 's>(_: &'r Resources, _: Self::State<'s>) -> Option<&'r u32> { None }
/// }
/// ```
pub trait ResourceQueryUnsafe: ResourceQueryMut + sealed::Reference {
    /// # Safety
    /// The caller must prevent overlapping mutable borrows of the queried resources
    /// for the lifetime of the returned references, including across query states.
    unsafe fn query_unsafe<'r, 's>(
        resources: &'r Resources,
        state: Self::State<'s>,
    ) -> Option<Self::MutItem<'r>>;
}

impl<R: Resource> ResourceQueryUnsafe for &R {
    unsafe fn query_unsafe<'r, 's>(
        resources: &'r Resources,
        state: Self::State<'s>,
    ) -> Option<Self::MutItem<'r>> {
        <&R as ResourceQuery>::query(resources, state)
    }
}

impl<R: Resource> ResourceQueryUnsafe for &mut R {
    unsafe fn query_unsafe<'r, 's>(
        resources: &'r Resources,
        mut state: Self::State<'s>,
    ) -> Option<Self::MutItem<'r>> {
        state.borrow_mut::<R>()?;
        let index = *resources.index.get(&TypeId::of::<R>())?;
        // The type index selects one stable slot; the shared query state excludes any
        // other reference to its value for the lifetime of this tuple.
        unsafe { (&mut *resources.resources.get(index)?.get()).downcast_mut() }
    }
}

// Lift to tuples

macro_rules! impl_resource_query {
    ($($param: ident),*) => {
        impl<$($param: ResourceQuery),*> ResourceQuery for ($($param,)*) {
            type Item<'r> = ($($param::Item<'r>,)*);
            type State<'s> = EphemeralQueryState<'s>;

            fn query<'r, 's>(resources: &'r Resources, mut state: Self::State<'s>) -> Option<Self::Item<'r>> {
                Some(
                    (
                        $($param::query(resources, state.clone_to::<$param::State<'_>>())?,)*
                    )
                )
            }
        }

        impl<$($param: ResourceQueryMut + ResourceQueryUnsafe + 'static),*> ResourceQueryMut for ($($param,)*)
        {
            type MutItem<'r> = ($($param::MutItem<'r>,)*);
            type State<'s> = EphemeralQueryState<'s>;

            fn query_mut<'r, 's>(
                resources: &'r mut Resources,
                mut state: Self::State<'s>,
            ) -> Option<Self::MutItem<'r>> {
                unsafe {
                    // Only sealed reference queries participate, and every member uses
                    // the same borrow state while the caller exclusively holds the store.
                    Some(
                        (
                            $(<$param as ResourceQueryUnsafe>::query_unsafe(resources, state.clone_to::<$param::State<'_>>())?,)*
                        )
                    )
                }
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

mod sealed {
    pub trait Reference {}
    impl<R: super::Resource> Reference for &R {}
    impl<R: super::Resource> Reference for &mut R {}
}

#[cfg(test)]
mod tests;
