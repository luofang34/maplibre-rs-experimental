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

    /// Reuses [`Style`]'s cached `Arc<Style>` worker snapshot across frames until the style mutates.
    pub(crate) fn worker_snapshot(&mut self) -> Arc<Style> {
        let style = self.style;
        self.shared
            .get_or_insert_with(|| style.worker_snapshot())
            .clone()
    }

    /// Reuses [`Style`]'s cached `Arc<Style>` worker snapshot filtered for disabled subsystems.
    pub(crate) fn filtered_worker_snapshot(
        &mut self,
        vectors_disabled: bool,
        symbols_disabled: bool,
    ) -> Arc<Style> {
        let style = self.style;
        self.shared
            .get_or_insert_with(|| {
                style.filtered_worker_snapshot(vectors_disabled, symbols_disabled)
            })
            .clone()
    }

    /// Raw style reference borrowed for the current frame.
    #[inline]
    pub(crate) fn style(&self) -> &'a Style {
        self.style
    }
}
