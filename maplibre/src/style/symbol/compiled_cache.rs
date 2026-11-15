//! Cached compiled properties for symbol paint evaluation.

use std::{
    any::{Any, TypeId},
    collections::HashMap,
    sync::Mutex,
};

use crate::style::property::{CompiledProperty, PropertyValue};

struct CompiledEntry {
    kind: TypeId,
    source: serde_json::Value,
    property: Box<dyn Any + Send + Sync>,
}

/// Thread-safe cache of pre-compiled properties keyed by name and type.
#[derive(Default)]
pub struct CompiledProperties(Mutex<HashMap<String, Vec<CompiledEntry>>>);

impl Clone for CompiledProperties {
    fn clone(&self) -> Self {
        Self::default()
    }
}

impl std::fmt::Debug for CompiledProperties {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("CompiledProperties")
    }
}

impl CompiledProperties {
    /// Returns the compiled property `name` if already compiled and `source` matches,
    /// or compiles and stores it in the cache.
    pub fn get<T>(&self, name: &str, source: &serde_json::Value) -> Option<CompiledProperty<T>>
    where
        T: PropertyValue + Send + Sync + 'static,
    {
        let kind = TypeId::of::<T>();
        let mut guard = crate::platform::lock_unpoisoned(&self.0);
        if let Some(entries) = guard.get_mut(name) {
            if let Some(entry) = entries.iter().find(|e| e.kind == kind) {
                if entry.source == *source {
                    if let Some(property) = entry.property.downcast_ref::<CompiledProperty<T>>() {
                        return Some(property.clone());
                    }
                }
            }
            entries.retain(|e| e.kind != kind);
        }
        let property = CompiledProperty::<T>::parse(source)?;
        guard
            .entry(name.to_owned())
            .or_default()
            .push(CompiledEntry {
                kind,
                source: source.clone(),
                property: Box::new(property.clone()),
            });
        Some(property)
    }
}
