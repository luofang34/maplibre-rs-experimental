//! Change observation, grid geometry and type-erased render labels.

#![deny(missing_docs)]

use std::ops::{Deref, DerefMut};

pub use fps_meter::FPSMeter;

mod fps_meter;
pub mod grid;
pub mod label;
pub mod math;

/// Compares values using a caller-supplied tolerance rather than exact equality.
pub trait SignificantlyDifferent<Rhs: ?Sized = Self> {
    /// Tolerance type; its units and comparison semantics are defined by the implementation.
    type Epsilon;

    /// Whether the difference between these values exceeds the supplied tolerance.
    #[must_use]
    fn ne(&self, other: &Rhs, epsilon: Self::Epsilon) -> bool;
}

/// Tracks a mutable value against an explicitly saved reference snapshot.
/// Mutating through `DerefMut` does not update the reference; before the first snapshot,
/// [`Self::did_change`] reports a change.
#[derive(Clone)]
pub struct ChangeObserver<T> {
    inner: T,
    reference_value: Option<T>,
}

impl<T> ChangeObserver<T> {
    /// Stores a value without a reference snapshot, so its first change check is true.
    pub fn new(value: T) -> Self {
        Self {
            inner: value,
            reference_value: None,
        }
    }
}

impl<T> ChangeObserver<T>
where
    T: Clone + SignificantlyDifferent,
{
    /// Clones the current value into the reference used by subsequent change checks.
    pub fn update_reference(&mut self) {
        self.reference_value = Some(self.inner.clone());
    }

    /// Compares the saved reference with the current value, or returns true without a reference.
    pub fn did_change(&self, epsilon: T::Epsilon) -> bool {
        if let Some(reference_value) = &self.reference_value {
            reference_value.ne(&self.inner, epsilon)
        } else {
            true
        }
    }
}

impl<T> Default for ChangeObserver<T>
where
    T: Default,
{
    fn default() -> Self {
        ChangeObserver::new(T::default())
    }
}

impl<T> Deref for ChangeObserver<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl<T> DerefMut for ChangeObserver<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.inner
    }
}
