//! What the host's memory lets a frame keep.
//!
//! A headset can terminate memory-heavy processes. A flight to the ground brings a hundred
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
pub const DRAPE_TEXTURE_LIMIT: usize = 64;
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
/// Bytes of tile geometry, labels and textures one frame uploads; the rest wait for the next
/// frame, so a burst of arrivals is spread over frames instead of stalling one.
pub const FRAME_UPLOAD_BYTES: usize = 32 << 20;
/// Bytes of each frame's uploads that only raster and DEM textures may use. GL JS uploads a
/// raster tile as soon as it arrives, so images keep this share however much geometry waits.
pub const IMAGE_UPLOAD_FLOOR_BYTES: usize = 8 << 20;

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

/// Applies immediate reductions and delayed recovery to noisy host memory reports.
#[derive(Default)]
pub struct MemoryBudgetTracker {
    pressure: Option<MemoryPressure>,
}

impl MemoryBudgetTracker {
    /// Keeps reduced resource limits until enough reserve exists to restore them safely.
    pub fn update(&mut self, available_bytes: Option<u64>) -> MemoryBudget {
        let measured = MemoryBudget { available_bytes };
        let next = match (self.pressure, available_bytes) {
            (Some(MemoryPressure::Critical), Some(bytes))
                if bytes < CRITICAL_MEMORY_BYTES + (256 << 20) =>
            {
                MemoryPressure::Critical
            }
            (Some(MemoryPressure::Critical | MemoryPressure::Low), Some(bytes))
                if bytes < LOW_MEMORY_BYTES + (512 << 20) =>
            {
                if bytes < CRITICAL_MEMORY_BYTES {
                    MemoryPressure::Critical
                } else {
                    MemoryPressure::Low
                }
            }
            _ => measured.pressure(),
        };
        self.pressure = Some(next);
        let ceiling = match next {
            MemoryPressure::Comfortable => u64::MAX,
            MemoryPressure::Low => LOW_MEMORY_BYTES - 1,
            MemoryPressure::Critical => CRITICAL_MEMORY_BYTES - 1,
        };
        MemoryBudget {
            available_bytes: available_bytes.map(|bytes| bytes.min(ceiling)),
        }
    }
}

#[cfg(test)]
mod tests;

/// What an upload puts on the GPU.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum UploadKind {
    /// Vector and label geometry.
    Geometry,
    /// Raster and DEM textures.
    Image,
}

/// The bytes the tile uploads of one frame share, across vector, label, raster and DEM
/// tiles. [`crate::render::frame_input`] resets it when a new host frame starts.
///
/// The first upload of each kind in a frame is always admitted, so a layer or texture larger
/// than the allowance still gets uploaded.
#[derive(Clone, Debug, Default)]
pub(crate) struct FrameUploads {
    geometry_bytes: usize,
    image_bytes: usize,
    geometry_started: bool,
    image_started: bool,
    geometry_tiles: usize,
}

impl FrameUploads {
    /// Admits an upload of `bytes` of `kind`, counting it, or refuses it for a later frame.
    pub(crate) fn take(&mut self, kind: UploadKind, bytes: usize) -> bool {
        if bytes == 0 {
            return true;
        }
        let used = self.geometry_bytes + self.image_bytes;
        let (allowed, started, counted) = match kind {
            // Geometry leaves the images their floor.
            UploadKind::Geometry => (
                FRAME_UPLOAD_BYTES
                    .saturating_sub(IMAGE_UPLOAD_FLOOR_BYTES.saturating_sub(self.image_bytes))
                    .saturating_sub(used),
                &mut self.geometry_started,
                &mut self.geometry_bytes,
            ),
            UploadKind::Image => (
                FRAME_UPLOAD_BYTES
                    .saturating_sub(used)
                    .max(IMAGE_UPLOAD_FLOOR_BYTES.saturating_sub(self.image_bytes)),
                &mut self.image_started,
                &mut self.image_bytes,
            ),
        };
        if *started && bytes > allowed {
            return false;
        }
        *started = true;
        *counted += bytes;
        true
    }

    /// Counts a tile whose geometry this frame uploaded, of [`UPLOADS_PER_FRAME`].
    pub(crate) fn count_geometry_tile(&mut self) {
        self.geometry_tiles += 1;
    }

    /// Whether this frame's geometry tile allowance is used up.
    pub(crate) fn geometry_tiles_exhausted(&self) -> bool {
        self.geometry_tiles >= UPLOADS_PER_FRAME
    }

    /// Bytes admitted this frame.
    #[cfg(test)]
    pub(crate) fn used(&self) -> usize {
        self.geometry_bytes + self.image_bytes
    }

    /// Starts the next frame's allowance.
    pub(crate) fn reset(&mut self) {
        *self = Self::default();
    }
}
