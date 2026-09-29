//! Traits used by label implementations

use std::{
    any::Any,
    hash::{Hash, Hasher},
};

/// Object-safe equality that distinguishes both concrete type and value.
pub trait DynEq: Any {
    /// Borrows the concrete value for a type-checked downcast.
    fn as_any(&self) -> &dyn Any;

    /// Compares values of the same concrete type; different types are unequal.
    fn dyn_eq(&self, other: &dyn DynEq) -> bool;
}

impl<T> DynEq for T
where
    T: Any + Eq,
{
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn dyn_eq(&self, other: &dyn DynEq) -> bool {
        if let Some(other) = other.as_any().downcast_ref::<T>() {
            return self == other;
        }
        false
    }
}

/// Object-safe hashing consistent with type-aware [`DynEq`] equality.
pub trait DynHash: DynEq {
    /// Borrows the equality interface without changing the concrete value.
    fn as_dyn_eq(&self) -> &dyn DynEq;

    /// Feeds both the value and its concrete type identity into the supplied hasher.
    fn dyn_hash(&self, state: &mut dyn Hasher);
}

impl<T> DynHash for T
where
    T: DynEq + Hash,
{
    fn as_dyn_eq(&self) -> &dyn DynEq {
        self
    }

    fn dyn_hash(&self, mut state: &mut dyn Hasher) {
        T::hash(self, &mut state);
        self.type_id().hash(&mut state);
    }
}

/// Macro to define a new label trait
///
/// # Example
///
/// ```
/// # use maplibre::define_label;
/// define_label!(MyNewLabelTrait);
/// ```
#[macro_export]
macro_rules! define_label {
    ($label_trait_name:ident) => {
        /// Defines a set of strongly-typed labels for a class of objects
        pub trait $label_trait_name:
            $crate::util::label::DynHash + ::std::fmt::Debug + Send + Sync + 'static
        {
            #[doc(hidden)]
            fn dyn_clone(&self) -> Box<dyn $label_trait_name>;
        }

        impl PartialEq for dyn $label_trait_name {
            fn eq(&self, other: &Self) -> bool {
                self.dyn_eq(other.as_dyn_eq())
            }
        }

        impl Eq for dyn $label_trait_name {}

        impl ::std::hash::Hash for dyn $label_trait_name {
            fn hash<H: ::std::hash::Hasher>(&self, state: &mut H) {
                self.dyn_hash(state);
            }
        }

        impl Clone for Box<dyn $label_trait_name> {
            fn clone(&self) -> Self {
                self.dyn_clone()
            }
        }

        impl $label_trait_name for ::std::borrow::Cow<'static, str> {
            fn dyn_clone(&self) -> Box<dyn $label_trait_name> {
                Box::new(self.clone())
            }
        }

        impl $label_trait_name for &'static str {
            fn dyn_clone(&self) -> Box<dyn $label_trait_name> {
                Box::new(<&str>::clone(self))
            }
        }
    };
}
