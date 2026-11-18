//! Change observation, grid geometry and type-erased render labels.

#![deny(missing_docs)]

use std::ops::{Deref, DerefMut};

pub use fps_meter::FPSMeter;
pub use maplibre_coords::SignificantlyDifferent;

pub(crate) mod async_join;
pub(crate) mod css_color;
mod fps_meter;
#[cfg(not(target_arch = "wasm32"))]
pub mod grid;
pub mod json;
pub mod label;
pub mod math;

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

#[cfg(test)]
mod tests {
    use super::ChangeObserver;
    use crate::coords::Zoom;

    #[test]
    fn zoom_observer_tracks_changes_since_its_reference() {
        let mut zoom = ChangeObserver::new(Zoom::new(4.0));
        assert!(zoom.did_change(0.05));
        zoom.update_reference();
        assert!(!zoom.did_change(0.05));
        *zoom = Zoom::new(4.025);
        assert!(!zoom.did_change(0.05));
        *zoom = Zoom::new(4.125);
        assert!(zoom.did_change(0.05));
        zoom.update_reference();
        assert!(!zoom.did_change(0.05));
        *zoom = Zoom::new(4.0);
        assert!(zoom.did_change(0.05));
    }
}
