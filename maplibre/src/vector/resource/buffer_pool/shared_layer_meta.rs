//! Reference-counted shared layer-metadata slab allocations across tiles of the same zoom.

use super::*;

#[derive(Clone, Debug)]
pub(super) struct SharedLayerMetaAlloc {
    pub range: Range<wgpu::BufferAddress>,
    pub ref_count: u32,
    pub last_opaque: Option<bool>,
}

impl<Q: Queue<B>, B, V: PoolVertex, I: Pod, TM: Pod, FM: Pod> BufferPool<Q, B, V, I, TM, FM> {
    /// Shares one `layer_metadata` record across all resident tiles of the same
    /// `(style_layer_id, tile_z)` so consecutive draws of the same style layer reuse one vertex
    /// buffer slice at Slot 2 across all vertex slabs.
    pub fn with_shared_layer_metadata(mut self) -> Self {
        self.shared_layer_metadata = true;
        self
    }

    #[inline]
    pub(super) fn layer_meta_share_key(
        &self,
        style_layer_id: crate::style::StyleLayerId,
        coords: WorldTileCoords,
    ) -> Option<(crate::style::StyleLayerId, u8)> {
        self.shared_layer_metadata
            .then_some((style_layer_id, u8::from(coords.z)))
    }

    pub(super) fn release_shared_layer_meta_ref(
        &mut self,
        layer_page: u16,
        style_layer_id: crate::style::StyleLayerId,
        tile_z: u8,
    ) -> bool {
        if !self.shared_layer_metadata {
            return true;
        }
        let map_key = (layer_page, style_layer_id, tile_z);
        match self.shared_layer_meta.get_mut(&map_key) {
            Some(shared) if shared.ref_count > 1 => {
                shared.ref_count -= 1;
                false
            }
            Some(_) => {
                self.shared_layer_meta.remove(&map_key);
                true
            }
            None => true,
        }
    }
}
