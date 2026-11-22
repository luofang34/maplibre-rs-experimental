//! Keeps drape textures across frames and knows when a tile's content changed.
//!
//! GL JS renders a terrain tile's layers to texture once and reuses the texture until the
//! source tiles behind it change, identified by a fingerprint of the source tile keys and the
//! source's revision. The cache below does the same: a tile whose fingerprint is unchanged
//! keeps its texture untouched, and textures of tiles that left the view wait in a free list
//! for the next tile that needs one.

use std::{
    collections::VecDeque,
    hash::{Hash, Hasher},
};

use rustc_hash::FxHashMap as HashMap;

use crate::{coords::WorldTileCoords, terrain::drape_targets::TargetSpec};

/// Largest number of released textures kept for reuse.
const MAX_FREE_TEXTURES: usize = 4;
/// Hard ceiling on total drape textures (`entries + parked + free`) held in [`DrapeCache`].
pub const HARD_MAX_DRAPE_TEXTURES: usize = crate::render::memory_budget::DRAPE_TEXTURE_LIMIT;

/// What of a source tile is on the GPU right now, so a drape redraws when its own tile's
/// content arrives or leaves and not when any other tile's does.
pub(crate) trait SourceContent {
    /// Identity of the uploaded style-layer geometry, or `None` when it has no allocation.
    fn vector_layer_revision(
        &self,
        coords: WorldTileCoords,
        layer_id: crate::style::StyleLayerId,
        cached_revision: u64,
    ) -> Option<u64>;
    /// Revision of the raster binding, or `None` while no texture is uploaded.
    fn raster_revision(&self, coords: WorldTileCoords, layer: &str) -> Option<u64>;
    /// Identity of the evaluated uniforms used by a DEM-shaded layer.
    fn dem_layer_fingerprint(&self, layer: &str) -> Option<u64>;
}

/// Fingerprint of everything that decides a drape texture's content.
pub(crate) fn fingerprint(
    spec: &TargetSpec,
    content: &impl SourceContent,
    clear: wgpu::Color,
    paint_zoom: f64,
) -> u64 {
    let mut hasher = rustc_hash::FxHasher::default();
    spec.coords.hash(&mut hasher);
    crate::vector::upload_system::drape_paint_zoom(paint_zoom)
        .to_bits()
        .hash(&mut hasher);
    for component in [clear.r, clear.g, clear.b, clear.a] {
        component.to_bits().hash(&mut hasher);
    }
    if let [shape] = spec.shapes.as_slice() {
        hash_shape_spec(shape, content, &mut hasher);
    } else if spec.shapes.len() > 1 {
        let mut shapes: Vec<&crate::terrain::drape_targets::ShapeSpec> =
            spec.shapes.iter().collect();
        shapes.sort_by_key(|shape| shape.source);
        for shape in shapes {
            hash_shape_spec(shape, content, &mut hasher);
        }
    }
    hasher.finish()
}

fn hash_shape_spec(
    shape: &crate::terrain::drape_targets::ShapeSpec,
    content: &impl SourceContent,
    hasher: &mut rustc_hash::FxHasher,
) {
    shape.source.hash(hasher);
    for layer in &shape.vector_layers {
        layer.id.hash(hasher);
        layer.index.hash(hasher);
        layer.coords.hash(hasher);
        content
            .vector_layer_revision(layer.coords, layer.id, layer.content_revision)
            .hash(hasher);
    }
    for (id, index, dem) in &shape.raster_layers {
        id.hash(hasher);
        index.hash(hasher);
        dem.hash(hasher);
        content.raster_revision(shape.source, id).hash(hasher);
        if *dem {
            content.dem_layer_fingerprint(id).hash(hasher);
        }
    }
}

struct Entry<T> {
    texture: T,
    fingerprint: Option<u64>,
    has_content: bool,
    /// A view has drawn it, so it is parked with its content when its tile leaves the view;
    /// a drape prepared ahead of the view that none has shown gives its texture back instead.
    shown: bool,
}

impl<T> Entry<T> {
    fn acquire(&mut self, fingerprint: u64) -> DrapeState {
        let state = if !self.has_content {
            DrapeState::New
        } else if self.fingerprint == Some(fingerprint) {
            DrapeState::Unchanged
        } else {
            DrapeState::Changed
        };
        self.fingerprint = Some(fingerprint);
        self.has_content = true;
        state
    }
}

/// Released textures kept with their content, newest first, so a tile that leaves the view
/// and returns, as tiles do when the head turns, is not drawn again.
const PARKED_TEXTURES: usize = 4;

/// Drape textures by key, a view tile or a view tile of one of several views, with the
/// textures of tiles that left their view parked with their content and a free list of blank
/// ones, which every key shares.
pub struct DrapeCache<T, K = WorldTileCoords> {
    entries: HashMap<K, Entry<T>>,
    parked: VecDeque<(K, Entry<T>)>,
    free: Vec<T>,
}

impl<T, K> Default for DrapeCache<T, K> {
    fn default() -> Self {
        Self {
            entries: HashMap::default(),
            parked: VecDeque::new(),
            free: Vec::new(),
        }
    }
}

/// What a tile's texture holds when it is acquired for a frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DrapeState {
    /// The content of the same sources; nothing to draw.
    Unchanged,
    /// Older content of the tile, to be drawn again.
    Changed,
    /// Whatever the texture held before, another tile's content or nothing; to be drawn
    /// before it shows.
    New,
    /// No texture: none is spare and the memory budget allows no new one. The tile draws
    /// with an ancestor's drape until one frees up.
    Withheld,
}

impl<T, K: Copy + Eq + std::hash::Hash> DrapeCache<T, K> {
    /// Ensures `coords` has a texture and reports what it holds: a tile with an unchanged
    /// fingerprint keeps its content.
    #[cfg(test)]
    pub fn acquire(
        &mut self,
        coords: K,
        fingerprint: u64,
        may_create: bool,
        create: impl FnOnce() -> T,
    ) -> DrapeState {
        self.acquire_matching(coords, fingerprint, may_create, |_| true, |_| true, create)
    }

    /// Like [`acquire`](Self::acquire), matching desired texture dimensions for tiered LOD reuse.
    pub fn acquire_matching(
        &mut self,
        coords: K,
        fingerprint: u64,
        may_create: bool,
        keep_existing: impl Fn(&T) -> bool,
        reusable: impl Fn(&T) -> bool,
        create: impl FnOnce() -> T,
    ) -> DrapeState {
        self.acquire_as(
            coords,
            fingerprint,
            (may_create, true),
            keep_existing,
            reusable,
            create,
        )
    }

    /// Like [`acquire`](Self::acquire), for a drape prepared ahead of the view, which a view
    /// has not shown until one acquires it.
    #[cfg(test)]
    pub fn acquire_ahead(
        &mut self,
        coords: K,
        fingerprint: u64,
        may_create: bool,
        create: impl FnOnce() -> T,
    ) -> DrapeState {
        self.acquire_ahead_matching(coords, fingerprint, may_create, |_| true, |_| true, create)
    }

    /// Like [`acquire_ahead`](Self::acquire_ahead), matching desired texture dimensions.
    pub fn acquire_ahead_matching(
        &mut self,
        coords: K,
        fingerprint: u64,
        may_create: bool,
        keep_existing: impl Fn(&T) -> bool,
        reusable: impl Fn(&T) -> bool,
        create: impl FnOnce() -> T,
    ) -> DrapeState {
        self.acquire_as(
            coords,
            fingerprint,
            (may_create, false),
            keep_existing,
            reusable,
            create,
        )
    }

    fn recycle_to_free(&mut self, texture: T) {
        if self.free.len() < MAX_FREE_TEXTURES {
            self.free.push(texture);
        }
    }

    fn make_room_for_create(&mut self) {
        if self.total_textures() < HARD_MAX_DRAPE_TEXTURES {
            return;
        }
        if self.free.pop().is_some() {
            return;
        }
        if self.take_unshown().is_some() {
            return;
        }
        drop(self.parked.pop_back());
    }

    fn reuse_or_evict_existing(
        &mut self,
        coords: &K,
        fingerprint: u64,
        (may_create, shown): (bool, bool),
        has_reusable_free: bool,
        keep_existing: &impl Fn(&T) -> bool,
    ) -> Option<DrapeState> {
        if let Some(entry) = self.entries.get_mut(coords) {
            if !shown && entry.has_content {
                return Some(DrapeState::Unchanged);
            }
            if keep_existing(&entry.texture) || (!may_create && !has_reusable_free) {
                entry.shown |= shown;
                return Some(entry.acquire(fingerprint));
            }
            if let Some(old) = self.entries.remove(coords) {
                self.recycle_to_free(old.texture);
            }
            return None;
        }
        let index = self
            .parked
            .iter()
            .position(|(parked, _)| parked == coords)?;
        let (_, mut entry) = self.parked.remove(index)?;
        if keep_existing(&entry.texture) || (!may_create && !has_reusable_free) {
            entry.shown |= shown;
            let state = entry.acquire(fingerprint);
            self.entries.insert(*coords, entry);
            return Some(state);
        }
        self.recycle_to_free(entry.texture);
        None
    }

    fn take_reusable_texture(&mut self, reusable: &impl Fn(&T) -> bool) -> Option<T> {
        self.take_unshown_matching(reusable)
            .or_else(|| self.take_parked_matching(reusable))
            .or_else(|| self.free.pop())
            .or_else(|| self.take_unshown())
            .or_else(|| self.parked.pop_back().map(|(_, entry)| entry.texture))
    }

    fn acquire_as(
        &mut self,
        coords: K,
        fingerprint: u64,
        (may_create, shown): (bool, bool),
        keep_existing: impl Fn(&T) -> bool,
        reusable: impl Fn(&T) -> bool,
        create: impl FnOnce() -> T,
    ) -> DrapeState {
        let has_reusable_free = self.free.iter().any(&reusable);
        if let Some(state) = self.reuse_or_evict_existing(
            &coords,
            fingerprint,
            (may_create, shown),
            has_reusable_free,
            &keep_existing,
        ) {
            return state;
        }
        // A matching spare texture first, then a new one while the budget allows. Past the budget
        // a view takes the texture of a drape prepared ahead of it that none has shown, then the
        // one parked longest ago; a drape prepared ahead takes neither, and a tile that finds none
        // goes without.
        let matching_free = self
            .free
            .iter()
            .rposition(&reusable)
            .map(|idx| self.free.swap_remove(idx));
        let texture = match matching_free {
            Some(texture) => texture,
            None if may_create => {
                self.make_room_for_create();
                if self.total_textures() < HARD_MAX_DRAPE_TEXTURES {
                    create()
                } else if !shown {
                    return DrapeState::Withheld;
                } else if let Some(texture) = self.take_reusable_texture(&reusable) {
                    texture
                } else {
                    return DrapeState::Withheld;
                }
            }
            None if !shown => return DrapeState::Withheld,
            None => match self.take_reusable_texture(&reusable) {
                Some(texture) => texture,
                None => return DrapeState::Withheld,
            },
        };
        self.entries.insert(
            coords,
            Entry {
                texture,
                fingerprint: Some(fingerprint),
                has_content: true,
                shown,
            },
        );
        DrapeState::New
    }

    /// Removes a drape prepared ahead of the view that none has shown, for its texture.
    fn take_unshown(&mut self) -> Option<T> {
        let coords = self
            .entries
            .iter()
            .find(|(_, entry)| !entry.shown)
            .map(|(coords, _)| *coords)?;
        self.entries.remove(&coords).map(|entry| entry.texture)
    }

    fn take_unshown_matching(&mut self, reusable: impl Fn(&T) -> bool) -> Option<T> {
        let coords = self
            .entries
            .iter()
            .find(|(_, entry)| !entry.shown && reusable(&entry.texture))
            .map(|(coords, _)| *coords)?;
        self.entries.remove(&coords).map(|entry| entry.texture)
    }

    fn take_parked_matching(&mut self, reusable: impl Fn(&T) -> bool) -> Option<T> {
        let idx = self
            .parked
            .iter()
            .rposition(|(_, entry)| reusable(&entry.texture))?;
        self.parked.remove(idx).map(|(_, entry)| entry.texture)
    }

    /// Iterates over every active, parked, and free texture currently held by the cache.
    pub fn iter_all_textures(&self) -> impl Iterator<Item = &T> {
        self.entries
            .values()
            .map(|entry| &entry.texture)
            .chain(self.parked.iter().map(|(_, entry)| &entry.texture))
            .chain(self.free.iter())
    }

    /// Textures held, parked and spare together.
    pub fn total_textures(&self) -> usize {
        self.entries.len() + self.parked.len() + self.free.len()
    }

    /// Drops every parked and spare texture, for a host short of memory.
    pub fn shed_spares(&mut self) {
        self.parked.clear();
        self.free.clear();
    }

    /// Cancels an acquired draw, preserving whether the texture has valid tile content.
    pub fn defer(&mut self, coords: K, state: DrapeState) {
        if let Some(entry) = self.entries.get_mut(&coords) {
            entry.fingerprint = None;
            entry.has_content = state != DrapeState::New;
        }
    }

    /// Parks the textures of the tiles `keep` refuses with their content; the oldest parked
    /// ones go to the free list, and beyond that are dropped. A test of the key, so one view's
    /// tiles can be released without touching another's.
    pub fn retain(&mut self, keep: impl Fn(&K) -> bool) {
        let dropped: Vec<K> = self
            .entries
            .keys()
            .filter(|coords| !keep(coords))
            .copied()
            .collect();
        for coords in dropped {
            match self.entries.remove(&coords) {
                Some(entry) if entry.shown => self.parked.push_front((coords, entry)),
                Some(entry) if self.free.len() < MAX_FREE_TEXTURES => self.free.push(entry.texture),
                _ => {}
            }
        }
        while self.parked.len() > PARKED_TEXTURES {
            if let Some((_, entry)) = self.parked.pop_back() {
                if self.free.len() < MAX_FREE_TEXTURES {
                    self.free.push(entry.texture);
                }
            }
        }
    }

    /// Whether a view has drawn the tile's drape, in use or parked after it left the view.
    pub fn holds_shown(&self, coords: K) -> bool {
        self.entries.get(&coords).is_some_and(|entry| entry.shown)
            || self.parked.iter().any(|(parked, _)| *parked == coords)
    }

    /// Texture of a view tile, if it has one.
    pub fn get(&self, coords: K) -> Option<&T> {
        self.entries
            .get(&coords)
            .filter(|entry| entry.has_content)
            .map(|entry| &entry.texture)
    }

    /// Whether the key holds a texture, drawn or still to be.
    pub fn holds(&self, coords: K) -> bool {
        self.entries.contains_key(&coords)
    }

    /// Number of keys `counted` accepts that hold a texture.
    pub fn count(&self, counted: impl Fn(&K) -> bool) -> usize {
        self.entries.keys().filter(|key| counted(key)).count()
    }

    /// Number of tiles holding a texture.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Number of released textures waiting for reuse.
    pub fn free_len(&self) -> usize {
        self.free.len()
    }

    /// Number of textures parked with the content of tiles that left the view.
    pub fn parked_len(&self) -> usize {
        self.parked.len()
    }
}

#[cfg(test)]
mod tests;
