//! Ordered draw queues, binned render phases, and projection bindings consumed by render passes.

#![deny(missing_docs)]

pub use batching::*;
pub use binned::*;
pub use draw::*;
pub use indirect::*;
pub use indirect_buffers::IndirectParametersBuffers;

use crate::{render::tile_view_pattern::ShapeDrawData, style::StyleLayerId, tcs::tiles::Tile};

mod batching;
mod binned;
mod draw;
mod indirect;
/// GPU indirect draw argument buffers shared across binned render phases.
pub mod indirect_buffers;

pub use crate::render::projection::ProjectionBinding;

/// A resource to collect and sort draw requests for specific [`PhaseItems`](PhaseItem).
pub struct RenderPhase<I: PhaseItem> {
    items: Vec<I>,
    mask_group_bits: u8,
}

impl<'a, I: PhaseItem> IntoIterator for &'a RenderPhase<I> {
    type Item = <&'a Vec<I> as IntoIterator>::Item;
    type IntoIter = <&'a Vec<I> as IntoIterator>::IntoIter;

    fn into_iter(self) -> Self::IntoIter {
        self.items.iter()
    }
}

impl<I: PhaseItem> AsRef<[I]> for RenderPhase<I> {
    #[inline]
    fn as_ref(&self) -> &[I] {
        &self.items
    }
}

impl<I: PhaseItem> Default for RenderPhase<I> {
    fn default() -> Self {
        Self {
            items: Vec::new(),
            mask_group_bits: 0,
        }
    }
}

impl<I: PhaseItem> RenderPhase<I> {
    /// Adds a [`PhaseItem`] to this render phase.
    pub fn add(&mut self, item: I) {
        self.mask_group_bits |= item.mask_group_bit();
        self.items.push(item);
    }

    /// Appends pre-built items in bulk, reusing the destination vector's capacity.
    pub fn extend_from_slice(&mut self, items: &[I])
    where
        I: Clone,
    {
        for item in items {
            self.mask_group_bits |= item.mask_group_bit();
        }
        self.items.extend_from_slice(items);
    }

    /// Borrows the underlying slice of queued items.
    #[cfg(test)]
    pub fn items(&self) -> &[I] {
        &self.items
    }

    /// Sorts the suffix starting at `start` in place; used by cached phase builders.
    pub fn sort_tail(&mut self, start: usize) {
        if let Some(tail) = self.items.get_mut(start..)
            && !tail.is_sorted_by_key(|d| d.sort_key())
        {
            tail.sort_by_key(|d| d.sort_key());
        }
    }

    /// Returns `true` when items are already ordered by ascending [`PhaseItem::sort_key`].
    pub fn is_sorted(&self) -> bool {
        self.items.is_sorted_by_key(|d| d.sort_key())
    }

    /// Sorts by ascending key while preserving insertion order for equal keys.
    pub fn sort(&mut self) {
        if !self.is_sorted() {
            self.items.sort_by_key(|d| d.sort_key());
        }
    }

    /// Drops queued items while retaining the vector allocation for the next frame.
    pub fn clear(&mut self) {
        self.items.clear();
        self.mask_group_bits = 0;
    }

    /// Keeps only the items the predicate accepts.
    pub fn retain(&mut self, mut keep: impl FnMut(&I) -> bool) {
        let mut bits = 0_u8;
        self.items.retain(|item| {
            let kept = keep(item);
            if kept {
                bits |= item.mask_group_bit();
            }
            kept
        });
        self.mask_group_bits = bits;
    }

    /// Number of queued items, including items whose draw resources are not ready.
    pub fn size(&self) -> usize {
        self.items.len()
    }
}

impl RenderPhase<LayerItem> {
    /// Returns `true` when any queued layer writes scene depth that symbol occlusion must test.
    pub fn has_depth_occluders(&self, flat_view: bool) -> bool {
        let custom_bit = 1_u8 << (MaskGroup::Custom as u8);
        let mutating_bit = 1_u8 << (MaskGroup::Mutating as u8);
        (self.mask_group_bits & custom_bit) != 0
            || (!flat_view && (self.mask_group_bits & mutating_bit) != 0)
    }
}

/// How a [`LayerItem`] interacts with tile stencil masks in the active render pass.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum MaskGroup {
    /// Neither tests nor mutates tile stencil masks (`background`, `sky`, `heatmap` composite,
    /// `ElevatedLine`, translucent 3D extrusions).
    #[default]
    Unmasked,
    /// Tests against the shared `TileKind::Vector` pyramid stencil (`0..=123`) without mutating
    /// bits `0..=6` (and restores bit `7` to `0` before the layer group finishes).
    Vector,
    /// Tests against its own source's [`TileMaskItem`]s and mutates stencil via bitwise inversion
    /// (`raster`, `hillshade`, `color-relief`).
    Mutating,
    /// Invokes custom render commands that may leave arbitrary stencil values.
    Custom,
}

/// Sentinel slot index indicating that a [`LayerItem`] or [`TranslucentItem`] is not backed by a
/// [`RingIndex`](crate::vector::resource::RingIndex) entry.
pub use crate::render::resource::buffer_pool::UNSET_ENTRY_SLOT;

/// Identifies the layer-group renderer eligible to batch contiguous same-layer items.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum LayerDrawKind {
    /// Custom or unbatched layer command dispatched individually via `draw_function`.
    #[default]
    Other,
    /// Solid or data-driven polygon fill bucket (`DrawVectorTiles`).
    VectorFill,
    /// Stroked vector line bucket (`DrawLineTiles`).
    VectorLine,
    /// Circle marker bucket (`DrawCircleTiles`).
    VectorCircle,
    /// Patterned polygon fill bucket (`DrawPatternTiles`).
    VectorPattern,
    /// Opaque 3D extrusion depth pre-pass (`DrawExtrusionDepth`).
    ExtrusionDepth,
    /// Opaque 3D extrusion shaded pass (`DrawExtrusionColor`).
    ExtrusionColor,
    /// Opaque 3D extrusion patterned pass (`DrawExtrusionPatternColor`).
    ExtrusionPatternColor,
    /// Translucent 3D extrusion back-face shell (`DrawExtrusionTranslucentBack`).
    ExtrusionTranslucentBack,
    /// Translucent 3D extrusion front-face shell (`DrawExtrusionTranslucentFront`).
    ExtrusionTranslucentFront,
    /// Imagery raster tile (`DrawRasterTiles`).
    Raster,
}

/// Main-pass draw sorted by style index, then interiors before borders within each layer.
#[derive(Clone, Copy)]
pub struct LayerItem {
    /// Static command implementation invoked when this item is rendered.
    pub draw_function: &'static dyn Draw<LayerItem>,
    /// Style painter-order index; lower indices are rendered first.
    pub index: u32,
    /// Whether projection-aware raster draws use the seam-expanding mesh variant.
    pub generate_borders: bool,

    /// Interned style layer ID used to locate matching geometry and paint resources.
    pub style_layer: StyleLayerId,
    /// Direct `O(1)` slot hint into the tile's [`RingIndex`](crate::vector::resource::RingIndex)
    /// queue, or [`UNSET_ENTRY_SLOT`] when not backed by a buffer pool.
    pub entry_slot: u16,

    /// Tile that owns the geometry or texture used by this draw.
    pub tile: Tile,
    /// Compact `Copy` source stencil and metadata range valid for the current frame upload.
    pub source_shape: ShapeDrawData,
    /// Projection uniform the draw binds; drape draws use the flat one.
    pub projection: ProjectionBinding,
    /// Stencil mask discipline for this draw item.
    pub mask_group: MaskGroup,
    /// Domain batching tag used by [`LayerPassHooks`](crate::render::graph::LayerPassHooks).
    pub draw_kind: LayerDrawKind,
    /// The part of the layer's geometry this draw covers, when features draw in the order of a
    /// sort key across tiles; the whole layer otherwise.
    pub run: Option<SortedRun>,
}

/// Features of a layer that share a sort key, as a range of its indices.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SortedRun {
    /// The sort key; greater keys draw later.
    pub key: f32,
    /// Start index relative to the layer's first index.
    pub start: u32,
    /// Exclusive end index relative to the layer's first index.
    pub end: u32,
}

impl SortedRun {
    /// The key as an integer that orders like the float.
    fn order(&self) -> u32 {
        let bits = self.key.to_bits();
        if bits >> 31 == 1 {
            !bits
        } else {
            bits | 0x8000_0000
        }
    }
}

impl PhaseItem for LayerItem {
    type SortKey = (u32, bool, u32);

    fn sort_key(&self) -> Self::SortKey {
        (
            self.index,
            self.generate_borders,
            self.run.as_ref().map_or(0, SortedRun::order),
        )
    }

    fn draw_function(&self) -> &dyn Draw<LayerItem> {
        self.draw_function
    }

    fn projection_binding(&self) -> ProjectionBinding {
        self.projection
    }

    fn mask_group_bit(&self) -> u8 {
        1_u8 << (self.mask_group as u8)
    }
}

/// Translucent-pass draw sorted by ascending style index; equal indices keep queue order.
#[derive(Clone, Copy)]
pub struct TranslucentItem {
    /// Static command implementation invoked when this item is rendered.
    pub draw_function: &'static dyn Draw<TranslucentItem>,
    /// Style painter-order index; lower indices are rendered first.
    pub index: u32,

    /// Interned style layer ID used to locate matching geometry and paint resources.
    pub style_layer: StyleLayerId,
    /// Direct `O(1)` slot hint into the tile's [`RingIndex`](crate::vector::resource::RingIndex)
    /// queue, or [`UNSET_ENTRY_SLOT`] when not backed by a buffer pool.
    pub entry_slot: u16,

    /// Tile that owns the geometry or texture used by this draw.
    pub tile: Tile,
    /// Compact `Copy` source stencil and metadata range valid for the current frame upload.
    pub source_shape: ShapeDrawData,
}

impl PhaseItem for TranslucentItem {
    type SortKey = u32;

    fn sort_key(&self) -> Self::SortKey {
        self.index
    }

    fn draw_function(&self) -> &dyn Draw<TranslucentItem> {
        self.draw_function
    }
}

/// Stencil draw sorted with seam-expanding masks before tile interiors.
#[derive(Clone, Copy)]
pub struct TileMaskItem {
    /// Static command implementation invoked when this item is rendered.
    pub draw_function: &'static dyn Draw<TileMaskItem>,
    /// Compact `Copy` source stencil and metadata range valid for the current frame upload.
    pub source_shape: ShapeDrawData,
    /// Selects a mesh expanded across tile seams when `true`.
    pub generate_borders: bool,
    /// Projection uniform the draw binds; drape draws use the flat one.
    pub projection: ProjectionBinding,
    /// Whether this mask belongs to the shared `TileKind::Vector` pyramid.
    pub is_vector: bool,
}

impl PhaseItem for TileMaskItem {
    type SortKey = (u32, u8, bool, bool);

    /// Coarser tiles write their masks first, so the masks of finer tiles lie over them.
    fn sort_key(&self) -> Self::SortKey {
        let coords = self.source_shape.coords();
        let (_, north, south) = crate::render::tile_mesh::globe_mesh_group_key(
            coords,
            crate::render::tile_mesh::TileMeshUsage::Stencil,
        );
        (
            u32::from(!self.generate_borders),
            u8::from(coords.z),
            north,
            south,
        )
    }

    fn draw_function(&self) -> &dyn Draw<TileMaskItem> {
        self.draw_function
    }

    fn projection_binding(&self) -> ProjectionBinding {
        self.projection
    }
}

#[cfg(test)]
mod tests;
