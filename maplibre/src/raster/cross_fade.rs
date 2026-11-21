//! Raster tiles a zoom leaves behind while the tiles that replace them fade in.
//!
//! GL JS keeps a source's loaded descendants of a newly shown tile and draws each mixed with
//! that tile, by the share of `raster-fade-duration` that has passed. For opaque tiles this is
//! the new tile drawn whole with the departing ones over it at the share still to go.

use rustc_hash::{FxHashMap as HashMap, FxHashSet as HashSet};

use crate::coords::WorldTileCoords;

/// Default `raster-fade-duration` in milliseconds per the MapLibre Style Specification.
pub const DEFAULT_RASTER_FADE_DURATION_MS: u32 = 300;

/// The departing tiles of each raster source, by source name.
#[derive(Default)]
pub struct RasterCrossFade {
    sources: HashMap<String, DepartingTiles>,
    previous_tiles: HashMap<String, HashSet<WorldTileCoords>>,
}

/// Tiles of a source that are fading out, and how opaque they still are.
pub struct DepartingTiles {
    /// The departing tiles; only descendants of a tile the view shows are drawn.
    pub tiles: HashSet<WorldTileCoords>,
    /// Opacity left to the departing tiles, from one when the fade starts to zero when it ends.
    pub opacity: f32,
    /// When a live timed fade started; `None` for harness/manual fixed-opacity fades.
    started_at: Option<instant::Instant>,
    /// Duration of a live timed fade in milliseconds.
    duration_ms: u32,
}

impl RasterCrossFade {
    /// Fades `tiles` of `source` out, with `opacity` of them still showing.
    pub fn insert(&mut self, source: &str, tiles: HashSet<WorldTileCoords>, opacity: f32) {
        self.sources.insert(
            source.to_owned(),
            DepartingTiles {
                tiles,
                opacity: opacity.clamp(0.0, 1.0),
                started_at: None,
                duration_ms: 0,
            },
        );
    }

    /// Starts a live timed cross-fade for `tiles` of `source` over `duration_ms` milliseconds.
    pub fn start_fade(
        &mut self,
        source: &str,
        tiles: HashSet<WorldTileCoords>,
        duration_ms: u32,
        now: instant::Instant,
    ) {
        if duration_ms == 0 || tiles.is_empty() {
            self.sources.remove(source);
            return;
        }
        self.sources.insert(
            source.to_owned(),
            DepartingTiles {
                tiles,
                opacity: 1.0,
                started_at: Some(now),
                duration_ms,
            },
        );
    }

    /// Advances all active live timed fades to `now`, dropping finished ones.
    /// Returns `true` while at least one timed fade is still in progress.
    pub fn advance(&mut self, now: instant::Instant) -> bool {
        let mut active = false;
        self.sources.retain(|_, departing| {
            let Some(started) = departing.started_at else {
                // Manual fixed-opacity fade injected by tests/harness stays until replaced.
                return true;
            };
            if departing.duration_ms == 0 {
                return false;
            }
            let elapsed_ms = now.saturating_duration_since(started).as_secs_f32() * 1000.0;
            let remaining = (1.0 - elapsed_ms / departing.duration_ms as f32).clamp(0.0, 1.0);
            departing.opacity = remaining;
            if remaining > 0.0 {
                active = true;
                true
            } else {
                false
            }
        });
        active
    }

    /// Compares `current_tiles` against the previous frame's tiles for `source` and starts a live
    /// timed fade for any previously-shown descendants replaced by coarser ancestors.
    pub fn update_live_fades(
        &mut self,
        source: &str,
        current_tiles: &HashSet<WorldTileCoords>,
        duration_ms: u32,
        now: instant::Instant,
    ) {
        let is_manual = self
            .sources
            .get(source)
            .is_some_and(|d| d.started_at.is_none());
        if !is_manual && duration_ms > 0 {
            if let Some(prev) = self.previous_tiles.get(source) {
                let departing: HashSet<WorldTileCoords> = prev
                    .iter()
                    .copied()
                    .filter(|&old| {
                        !current_tiles.contains(&old)
                            && current_tiles.iter().any(|&cur| is_descendant_of(old, cur))
                    })
                    .collect();
                if !departing.is_empty() {
                    self.start_fade(source, departing, duration_ms, now);
                }
            }
        }
        self.previous_tiles
            .insert(source.to_owned(), current_tiles.clone());
    }

    /// Whether any live timed fade is currently in progress.
    pub fn has_active_timed_fade(&self) -> bool {
        self.sources
            .values()
            .any(|d| d.started_at.is_some() && d.opacity > 0.0)
    }

    /// The departing tiles of `source`, if it is fading.
    pub fn departing(&self, source: Option<&str>) -> Option<&DepartingTiles> {
        self.sources.get(source?)
    }

    /// Whether `coords` of `source` is fading out.
    pub fn is_departing(&self, source: Option<&str>, coords: &WorldTileCoords) -> bool {
        self.departing(source)
            .is_some_and(|departing| departing.tiles.contains(coords))
    }

    /// Whether no raster source has departing tiles fading out.
    pub(crate) fn is_empty(&self) -> bool {
        self.sources.is_empty()
    }
}

fn is_descendant_of(candidate: WorldTileCoords, ancestor: WorldTileCoords) -> bool {
    let dz = u8::from(candidate.z).checked_sub(u8::from(ancestor.z));
    match dz {
        Some(shift) if shift > 0 && shift < 32 => {
            (candidate.x >> shift) == ancestor.x && (candidate.y >> shift) == ancestor.y
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests;
