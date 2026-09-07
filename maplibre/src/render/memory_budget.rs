//! What the host's memory lets a frame keep.
//!
//! A headset kills the process at a fixed footprint. A flight to the ground brings a hundred
//! drawn tiles, each with a drape texture, and a burst of tile uploads, and that alone can
//! carry the footprint past the limit. The host reports how much memory is still available
//! each frame; below a reserve the frame goes carefully: only the nearest tiles keep a drape
//! texture of their own and the rest draw with an ancestor's, spare textures are released,
//! and tiles are requested a few at a time; nearly out, nothing new is taken on and every
//! tile out of view is dropped. A hard cap on drape textures holds even when the host
//! reports nothing.

/// Available memory below which the frame goes carefully: no new drape textures, spare
/// textures released, and tiles requested a few at a time.
pub const LOW_MEMORY_BYTES: u64 = 1 << 30;
/// Available memory below which the frame takes nothing new on at all and drops every tile
/// out of view, so memory comes back.
pub const CRITICAL_MEMORY_BYTES: u64 = 512 << 20;
/// Drape textures the terrain may hold, spares included, whatever the host reports.
pub const DRAPE_TEXTURE_LIMIT: usize = 96;
/// Drape textures the terrain may still hold while memory is low: the nearest tiles keep
/// their own, the rest draw with an ancestor's.
pub const DRAPE_TEXTURES_WHEN_LOW: usize = 32;
/// Drape textures the terrain may still hold while memory is critical.
pub const DRAPE_TEXTURES_WHEN_CRITICAL: usize = 12;
/// Tiles in flight while memory is low.
pub const TILES_IN_FLIGHT_WHEN_LOW: usize = 4;
/// Tiles whose geometry is uploaded in one frame; the rest wait for the next, so a burst of
/// arrivals does not stage a hundred tiles' geometry at once.
pub const UPLOADS_PER_FRAME: usize = 8;

/// How much room the host has left.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MemoryPressure {
    /// Enough for the frame to take on what it wants.
    Comfortable,
    /// Little; nothing new that could be done without, and loading slowed.
    Low,
    /// Almost none; nothing new, and everything out of view dropped.
    Critical,
}

/// The memory the host reports as still available to the process.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MemoryBudget {
    /// Bytes before the host would be killed, or `None` when the host cannot tell.
    pub available_bytes: Option<u64>,
}

impl MemoryBudget {
    /// How much room the host has left; comfortable when it cannot tell.
    pub fn pressure(&self) -> MemoryPressure {
        match self.available_bytes {
            Some(available) if available < CRITICAL_MEMORY_BYTES => MemoryPressure::Critical,
            Some(available) if available < LOW_MEMORY_BYTES => MemoryPressure::Low,
            _ => MemoryPressure::Comfortable,
        }
    }

    /// Whether the frame must take nothing new on that it could do without.
    pub fn is_tight(&self) -> bool {
        self.pressure() != MemoryPressure::Comfortable
    }

    /// Tiles that may be in flight at once.
    pub fn tiles_in_flight_allowed(&self, comfortable: usize) -> usize {
        match self.pressure() {
            MemoryPressure::Comfortable => comfortable,
            MemoryPressure::Low => TILES_IN_FLIGHT_WHEN_LOW.min(comfortable),
            MemoryPressure::Critical => 0,
        }
    }

    /// Drape textures the terrain may hold at this pressure.
    pub fn drape_textures_allowed(&self) -> usize {
        match self.pressure() {
            MemoryPressure::Comfortable => DRAPE_TEXTURE_LIMIT,
            MemoryPressure::Low => DRAPE_TEXTURES_WHEN_LOW,
            MemoryPressure::Critical => DRAPE_TEXTURES_WHEN_CRITICAL,
        }
    }

    /// Whether a drape texture may be created when `held` are held already.
    pub fn allows_drape_texture(&self, held: usize) -> bool {
        held < self.drape_textures_allowed()
    }
}

#[cfg(test)]
mod tests {
    use super::{
        MemoryBudget, MemoryPressure, CRITICAL_MEMORY_BYTES, DRAPE_TEXTURES_WHEN_CRITICAL,
        DRAPE_TEXTURES_WHEN_LOW, DRAPE_TEXTURE_LIMIT, LOW_MEMORY_BYTES, TILES_IN_FLIGHT_WHEN_LOW,
    };

    #[test]
    fn an_unknown_budget_only_holds_the_texture_cap() {
        let budget = MemoryBudget::default();
        assert!(!budget.is_tight());
        assert!(budget.allows_drape_texture(DRAPE_TEXTURE_LIMIT - 1));
        assert!(!budget.allows_drape_texture(DRAPE_TEXTURE_LIMIT));
    }

    #[test]
    fn a_low_budget_allows_no_new_texture_and_a_few_tiles_in_flight() {
        let budget = MemoryBudget {
            available_bytes: Some(LOW_MEMORY_BYTES - 1),
        };
        assert_eq!(budget.pressure(), MemoryPressure::Low);
        assert!(budget.is_tight());
        assert!(budget.allows_drape_texture(DRAPE_TEXTURES_WHEN_LOW - 1));
        assert!(!budget.allows_drape_texture(DRAPE_TEXTURES_WHEN_LOW));
        assert_eq!(budget.tiles_in_flight_allowed(24), TILES_IN_FLIGHT_WHEN_LOW);
        let roomy = MemoryBudget {
            available_bytes: Some(LOW_MEMORY_BYTES),
        };
        assert_eq!(roomy.pressure(), MemoryPressure::Comfortable);
        assert!(roomy.allows_drape_texture(0));
        assert_eq!(roomy.tiles_in_flight_allowed(24), 24);
    }

    #[test]
    fn a_critical_budget_allows_nothing_in_flight() {
        let budget = MemoryBudget {
            available_bytes: Some(CRITICAL_MEMORY_BYTES - 1),
        };
        assert_eq!(budget.pressure(), MemoryPressure::Critical);
        assert_eq!(budget.tiles_in_flight_allowed(24), 0);
        assert!(budget.allows_drape_texture(DRAPE_TEXTURES_WHEN_CRITICAL - 1));
        assert!(!budget.allows_drape_texture(DRAPE_TEXTURES_WHEN_CRITICAL));
    }
}
