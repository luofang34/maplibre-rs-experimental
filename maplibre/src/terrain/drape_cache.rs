//! Keeps drape textures across frames and knows when a tile's content changed.
//!
//! GL JS renders a terrain tile's layers to texture once and reuses the texture until the
//! source tiles behind it change, identified by a fingerprint of the source tile keys and the
//! source's revision. The cache below does the same: a tile whose fingerprint is unchanged
//! keeps its texture untouched, and textures of tiles that left the view wait in a free list
//! for the next tile that needs one.

use std::{
    collections::{HashMap, HashSet, VecDeque},
    hash::{Hash, Hasher},
};

use crate::{coords::WorldTileCoords, terrain::drape_targets::TargetSpec};

/// Largest number of released textures kept for reuse.
const MAX_FREE_TEXTURES: usize = 8;

/// What of a source tile is on the GPU right now, so a drape redraws when its own tile's
/// content arrives or leaves and not when any other tile's does.
pub(crate) trait SourceContent {
    /// Whether the style layer's geometry of the tile is in the vector buffer pool.
    fn vector_layer_loaded(&self, coords: WorldTileCoords, layer_id: &str) -> bool;
    /// Whether the raster tile has a texture bound.
    fn raster_loaded(&self, coords: WorldTileCoords) -> bool;
}

/// Fingerprint of everything that decides a drape texture's content.
pub(crate) fn fingerprint(
    spec: &TargetSpec,
    content: &impl SourceContent,
    clear: wgpu::Color,
) -> u64 {
    let mut hasher = std::hash::DefaultHasher::new();
    spec.coords.hash(&mut hasher);
    for component in [clear.r, clear.g, clear.b, clear.a] {
        component.to_bits().hash(&mut hasher);
    }
    let mut shapes: Vec<&crate::terrain::drape_targets::ShapeSpec> = spec.shapes.iter().collect();
    shapes.sort_by_key(|shape| shape.source);
    for shape in shapes {
        shape.source.hash(&mut hasher);
        for layer in &shape.vector_layers {
            layer.id.hash(&mut hasher);
            layer.index.hash(&mut hasher);
            layer.coords.hash(&mut hasher);
            content
                .vector_layer_loaded(layer.coords, &layer.id)
                .hash(&mut hasher);
        }
        for (id, index, _) in &shape.raster_layers {
            id.hash(&mut hasher);
            index.hash(&mut hasher);
            content.raster_loaded(shape.source).hash(&mut hasher);
        }
    }
    hasher.finish()
}

struct Entry<T> {
    texture: T,
    fingerprint: u64,
}

/// Released textures kept with their content, newest first, so a tile that leaves the view
/// and returns, as tiles do when the head turns, is not drawn again.
const PARKED_TEXTURES: usize = 8;

/// Drape textures by view tile, with the textures of tiles that left the view parked with
/// their content and a free list of blank ones.
pub struct DrapeCache<T> {
    entries: HashMap<WorldTileCoords, Entry<T>>,
    parked: VecDeque<(WorldTileCoords, Entry<T>)>,
    free: Vec<T>,
}

impl<T> Default for DrapeCache<T> {
    fn default() -> Self {
        Self {
            entries: HashMap::new(),
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

impl<T> DrapeCache<T> {
    /// Ensures `coords` has a texture and reports what it holds: a tile with an unchanged
    /// fingerprint keeps its content.
    pub fn acquire(
        &mut self,
        coords: WorldTileCoords,
        fingerprint: u64,
        may_create: bool,
        create: impl FnOnce() -> T,
    ) -> DrapeState {
        match self.entries.get_mut(&coords) {
            Some(entry) if entry.fingerprint == fingerprint => DrapeState::Unchanged,
            Some(entry) => {
                entry.fingerprint = fingerprint;
                DrapeState::Changed
            }
            None => {
                if let Some(index) = self.parked.iter().position(|(parked, _)| *parked == coords) {
                    let (_, mut entry) = self
                        .parked
                        .remove(index)
                        .unwrap_or_else(|| unreachable!("the parked index was just found"));
                    let state = if entry.fingerprint == fingerprint {
                        DrapeState::Unchanged
                    } else {
                        entry.fingerprint = fingerprint;
                        DrapeState::Changed
                    };
                    self.entries.insert(coords, entry);
                    return state;
                }
                // A spare texture first, then a new one while the budget allows; past the
                // budget the texture parked longest ago gives up its content, and with none
                // parked the tile goes without.
                let texture = match self.free.pop() {
                    Some(texture) => texture,
                    None if may_create => create(),
                    None => match self.parked.pop_back() {
                        Some((_, entry)) => entry.texture,
                        None => return DrapeState::Withheld,
                    },
                };
                self.entries.insert(
                    coords,
                    Entry {
                        texture,
                        fingerprint,
                    },
                );
                DrapeState::New
            }
        }
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

    /// Marks a tile acquired this frame as not drawn after all, so the next frame acquires
    /// it as changed again.
    pub fn defer(&mut self, coords: WorldTileCoords) {
        if let Some(entry) = self.entries.get_mut(&coords) {
            entry.fingerprint = entry.fingerprint.wrapping_add(1);
        }
    }

    /// Parks the textures of tiles not in `keep` with their content; the oldest parked ones
    /// go to the free list, and beyond that are dropped.
    pub fn retain(&mut self, keep: &HashSet<WorldTileCoords>) {
        let dropped: Vec<WorldTileCoords> = self
            .entries
            .keys()
            .filter(|coords| !keep.contains(coords))
            .copied()
            .collect();
        for coords in dropped {
            if let Some(entry) = self.entries.remove(&coords) {
                self.parked.push_front((coords, entry));
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

    /// Texture of a view tile, if it has one.
    pub fn get(&self, coords: WorldTileCoords) -> Option<&T> {
        self.entries.get(&coords).map(|entry| &entry.texture)
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
