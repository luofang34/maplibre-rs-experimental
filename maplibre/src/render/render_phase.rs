//! Ordered draw queues and projection bindings consumed by the render passes.

#![deny(missing_docs)]

pub use draw::*;

use crate::{render::tile_view_pattern::TileShape, tcs::tiles::Tile};

mod draw;

/// Which projection uniform a draw binds at group zero.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ProjectionBinding {
    /// The camera's projection for drawing to the screen.
    #[default]
    View,
    /// A flat identity projection for drawing into a drape texture.
    Flat,
}

/// A resource to collect and sort draw requests for specific [`PhaseItems`](PhaseItem).
pub struct RenderPhase<I: PhaseItem> {
    items: Vec<I>,
}

impl<'a, I: PhaseItem> IntoIterator for &'a RenderPhase<I> {
    type Item = <&'a Vec<I> as IntoIterator>::Item;
    type IntoIter = <&'a Vec<I> as IntoIterator>::IntoIter;

    fn into_iter(self) -> Self::IntoIter {
        self.items.iter()
    }
}

impl<I: PhaseItem> Default for RenderPhase<I> {
    fn default() -> Self {
        Self { items: Vec::new() }
    }
}

impl<I: PhaseItem> RenderPhase<I> {
    /// Adds a [`PhaseItem`] to this render phase.
    pub fn add(&mut self, item: I) {
        self.items.push(item);
    }

    /// Sorts by ascending key while preserving insertion order for equal keys.
    pub fn sort(&mut self) {
        self.items.sort_by_key(|d| d.sort_key());
    }

    /// Drops queued items while retaining the vector allocation for the next frame.
    pub fn clear(&mut self) {
        self.items.clear();
    }

    /// Keeps only the items the predicate accepts.
    pub fn retain(&mut self, keep: impl FnMut(&I) -> bool) {
        self.items.retain(keep);
    }

    /// Number of queued items, including items whose draw resources are not ready.
    pub fn size(&self) -> usize {
        self.items.len()
    }
}

/// Main-pass draw sorted by style index, then borders before interiors within each layer.
pub struct LayerItem {
    /// Command implementation invoked when this item is rendered.
    pub draw_function: Box<dyn Draw<LayerItem>>,
    /// Style painter-order index; lower indices are rendered first.
    pub index: u32,
    /// Whether projection-aware raster draws use the seam-expanding mesh variant.
    pub generate_borders: bool,

    /// Style layer ID used to locate matching geometry and paint resources.
    pub style_layer: String,

    /// Tile that owns the geometry or texture used by this draw.
    pub tile: Tile,
    /// Source transform and metadata range valid for the current frame upload.
    pub source_shape: TileShape,
    /// Projection uniform the draw binds; drape draws use the flat one.
    pub projection: ProjectionBinding,
}

impl PhaseItem for LayerItem {
    type SortKey = (u32, bool);

    fn sort_key(&self) -> Self::SortKey {
        (self.index, !self.generate_borders)
    }

    fn draw_function(&self) -> &dyn Draw<LayerItem> {
        self.draw_function.as_ref()
    }

    fn projection_binding(&self) -> ProjectionBinding {
        self.projection
    }
}

/// Translucent-pass draw sorted by ascending style index; equal indices keep queue order.
pub struct TranslucentItem {
    /// Command implementation invoked when this item is rendered.
    pub draw_function: Box<dyn Draw<TranslucentItem>>,
    /// Style painter-order index; lower indices are rendered first.
    pub index: u32,

    /// Style layer ID used to locate matching geometry and paint resources.
    pub style_layer: String,

    /// Tile that owns the geometry or texture used by this draw.
    pub tile: Tile,
    /// Source transform and metadata range valid for the current frame upload.
    pub source_shape: TileShape,
}

impl PhaseItem for TranslucentItem {
    type SortKey = u32;

    fn sort_key(&self) -> Self::SortKey {
        self.index
    }

    fn draw_function(&self) -> &dyn Draw<TranslucentItem> {
        self.draw_function.as_ref()
    }
}

/// Stencil draw sorted with seam-expanding masks before tile interiors.
pub struct TileMaskItem {
    /// Command implementation invoked when this item is rendered.
    pub draw_function: Box<dyn Draw<TileMaskItem>>,
    /// Source transform and metadata range valid for the current frame upload.
    pub source_shape: TileShape,
    /// Selects a mesh expanded across tile seams when `true`.
    pub generate_borders: bool,
    /// Projection uniform the draw binds; drape draws use the flat one.
    pub projection: ProjectionBinding,
}

impl PhaseItem for TileMaskItem {
    type SortKey = u32;

    fn sort_key(&self) -> Self::SortKey {
        u32::from(!self.generate_borders)
    }

    fn draw_function(&self) -> &dyn Draw<TileMaskItem> {
        self.draw_function.as_ref()
    }

    fn projection_binding(&self) -> ProjectionBinding {
        self.projection
    }
}

#[cfg(test)]
mod tests;
