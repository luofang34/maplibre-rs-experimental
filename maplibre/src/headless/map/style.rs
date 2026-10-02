//! Changing a headless map's style between frames, as [`crate::map::Map`] changes its own.

use super::HeadlessMap;
use crate::style::{
    mutation::{StyleChange, StyleMutationError},
    Style,
};

impl HeadlessMap {
    /// Applies a style change and drops or reloads what it leaves stale, as
    /// [`crate::context::MapContext::mutate_style`] does.
    pub fn mutate_style(
        &mut self,
        apply: impl FnOnce(&mut Style) -> Result<StyleChange, StyleMutationError>,
    ) -> Result<StyleChange, StyleMutationError> {
        self.map_context.mutate_style(apply)
    }
}

#[cfg(test)]
mod tests;
