//! The style snapshot a frame's tile requests share.

use std::sync::Arc;

use crate::style::Style;

/// Copies the map's style for workers once, on the first request of a frame that needs it, and
/// hands every later request of that frame the same snapshot.
pub(crate) struct FrameStyle<'a> {
    style: &'a Style,
    shared: Option<Arc<Style>>,
}

impl<'a> FrameStyle<'a> {
    /// Starts a frame whose requests read `style`.
    pub(crate) fn new(style: &'a Style) -> Self {
        Self {
            style,
            shared: None,
        }
    }

    /// The snapshot for workers, made from the style by `make` on the frame's first call.
    pub(crate) fn share(&mut self, make: impl FnOnce(&Style) -> Style) -> Arc<Style> {
        let style = self.style;
        self.shared
            .get_or_insert_with(|| Arc::new(make(style)))
            .clone()
    }
}
