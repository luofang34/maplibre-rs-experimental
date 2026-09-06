//! Keeps drape textures across frames and knows when a tile's content changed.
//!
//! GL JS renders a terrain tile's layers to texture once and reuses the texture until the
//! source tiles behind it change, identified by a fingerprint of the source tile keys and the
//! source's revision. The cache below does the same: a tile whose fingerprint is unchanged
//! keeps its texture untouched, and textures of tiles that left the view wait in a free list
//! for the next tile that needs one.

use std::{
    collections::{HashMap, HashSet},
    hash::{Hash, Hasher},
};

use crate::{coords::WorldTileCoords, terrain::drape_targets::TargetSpec};

/// Largest number of released textures kept for reuse.
const MAX_FREE_TEXTURES: usize = 64;

/// Revisions of the sources drawn into drape textures; any change redraws every texture.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct SourceRevisions {
    /// Advances whenever a raster tile texture is bound.
    pub raster: u64,
    /// Advances whenever vector geometry is allocated or evicted.
    pub vector: u64,
}

/// Fingerprint of everything that decides a drape texture's content.
pub fn fingerprint(spec: &TargetSpec, revisions: SourceRevisions, clear: wgpu::Color) -> u64 {
    let mut hasher = std::hash::DefaultHasher::new();
    spec.coords.hash(&mut hasher);
    revisions.hash(&mut hasher);
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
        }
        for (id, index, _) in &shape.raster_layers {
            id.hash(&mut hasher);
            index.hash(&mut hasher);
        }
    }
    hasher.finish()
}

struct Entry<T> {
    texture: T,
    fingerprint: u64,
}

/// Drape textures by view tile, with a free list of released ones.
pub struct DrapeCache<T> {
    entries: HashMap<WorldTileCoords, Entry<T>>,
    free: Vec<T>,
}

impl<T> Default for DrapeCache<T> {
    fn default() -> Self {
        Self {
            entries: HashMap::new(),
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
}

impl<T> DrapeCache<T> {
    /// Ensures `coords` has a texture and reports what it holds: a tile with an unchanged
    /// fingerprint keeps its content.
    pub fn acquire(
        &mut self,
        coords: WorldTileCoords,
        fingerprint: u64,
        create: impl FnOnce() -> T,
    ) -> DrapeState {
        match self.entries.get_mut(&coords) {
            Some(entry) if entry.fingerprint == fingerprint => DrapeState::Unchanged,
            Some(entry) => {
                entry.fingerprint = fingerprint;
                DrapeState::Changed
            }
            None => {
                let texture = self.free.pop().unwrap_or_else(create);
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

    /// Marks a tile acquired this frame as not drawn after all, so the next frame acquires
    /// it as changed again.
    pub fn defer(&mut self, coords: WorldTileCoords) {
        if let Some(entry) = self.entries.get_mut(&coords) {
            entry.fingerprint = entry.fingerprint.wrapping_add(1);
        }
    }

    /// Releases the textures of tiles not in `keep` into the free list.
    pub fn retain(&mut self, keep: &HashSet<WorldTileCoords>) {
        let dropped: Vec<WorldTileCoords> = self
            .entries
            .keys()
            .filter(|coords| !keep.contains(coords))
            .copied()
            .collect();
        for coords in dropped {
            if let Some(entry) = self.entries.remove(&coords) {
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
}

#[cfg(test)]
mod tests;
