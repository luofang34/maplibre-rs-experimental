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
const WASM_BOOT_UPLOADS_PER_FRAME: usize = 4;
const WASM_BOOT_GEOMETRY_UPLOAD_BYTES: usize = 3 << 20;
const NATIVE_UPLOADS_PER_FRAME: usize = 8;
const WASM_MOVING_UPLOADS_PER_FRAME: usize = 1;
const NATIVE_MOVING_UPLOADS_PER_FRAME: usize = 2;
const WASM_MOVING_UNCOVERED_UPLOADS_PER_FRAME: usize = 2;
const WASM_MOVING_GEOMETRY_UPLOAD_BYTES: usize = 512 << 10;
const NATIVE_MOVING_GEOMETRY_UPLOAD_BYTES: usize = 2 << 20;
const WASM_MOVING_UNCOVERED_GEOMETRY_UPLOAD_BYTES: usize = 1 << 20;
const WASM_POST_BOOT_UPLOADS_PER_FRAME: usize = 2;
const WASM_POST_BOOT_GEOMETRY_UPLOAD_BYTES: usize = 1 << 20;

/// Tiles whose geometry is uploaded in one frame; the rest wait for the next, so a burst of
/// arrivals does not stage a hundred tiles' geometry at once.
#[cfg(target_arch = "wasm32")]
pub const UPLOADS_PER_FRAME: usize = WASM_BOOT_UPLOADS_PER_FRAME;
/// Tiles whose geometry is uploaded in one frame; the rest wait for the next, so a burst of
/// arrivals does not stage a hundred tiles' geometry at once.
#[cfg(not(target_arch = "wasm32"))]
pub const UPLOADS_PER_FRAME: usize = NATIVE_UPLOADS_PER_FRAME;
/// Geometry tiles uploaded per frame while the camera is actively moving on `wasm32`, pacing
/// multi-tile bursts across consecutive vsyncs to keep p95/p99 frame CPU flat.
#[cfg(all(test, target_arch = "wasm32"))]
const MOVING_UPLOADS_PER_FRAME: usize = WASM_MOVING_UPLOADS_PER_FRAME;
#[cfg(all(test, not(target_arch = "wasm32")))]
const MOVING_UPLOADS_PER_FRAME: usize = NATIVE_MOVING_UPLOADS_PER_FRAME;
/// Uncovered viewport tiles (with no loaded ancestor stand-in on the GPU) admitted per moving
/// frame so visible pan holes fill immediately on arrival.
#[cfg(all(test, target_arch = "wasm32"))]
const MOVING_UNCOVERED_UPLOADS_PER_FRAME: usize = WASM_MOVING_UNCOVERED_UPLOADS_PER_FRAME;
#[cfg(all(test, not(target_arch = "wasm32")))]
const MOVING_UNCOVERED_UPLOADS_PER_FRAME: usize = UPLOADS_PER_FRAME;
/// Geometry bytes uploaded per frame while the camera is actively moving (the first geometry
/// upload of a frame is always admitted even when larger).
#[cfg(all(test, target_arch = "wasm32"))]
const MOVING_GEOMETRY_UPLOAD_BYTES: usize = WASM_MOVING_GEOMETRY_UPLOAD_BYTES;
#[cfg(all(test, not(target_arch = "wasm32")))]
const MOVING_GEOMETRY_UPLOAD_BYTES: usize = NATIVE_MOVING_GEOMETRY_UPLOAD_BYTES;
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

fn below_critical_hysteresis(bytes: u64) -> bool {
    bytes < CRITICAL_MEMORY_BYTES + (256 << 20)
}

fn below_low_hysteresis(bytes: u64) -> bool {
    bytes < LOW_MEMORY_BYTES + (512 << 20)
}

fn clamp_available_bytes(available_bytes: Option<u64>, ceiling: u64) -> Option<u64> {
    available_bytes.map(|bytes| bytes.min(ceiling))
}

impl MemoryBudgetTracker {
    /// Keeps reduced resource limits until enough reserve exists to restore them safely.
    pub fn update(&mut self, available_bytes: Option<u64>) -> MemoryBudget {
        let measured = MemoryBudget { available_bytes };
        let next = match (self.pressure, available_bytes) {
            (Some(MemoryPressure::Critical), Some(bytes)) if below_critical_hysteresis(bytes) => {
                MemoryPressure::Critical
            }
            (Some(MemoryPressure::Critical | MemoryPressure::Low), Some(bytes))
                if below_low_hysteresis(bytes) =>
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
            available_bytes: clamp_available_bytes(available_bytes, ceiling),
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

const fn wasm_geometry_limits(
    moving: bool,
    post_boot: bool,
    uncovered: bool,
    full_bytes: usize,
) -> (usize, usize) {
    if moving {
        if uncovered {
            (
                WASM_MOVING_UNCOVERED_UPLOADS_PER_FRAME,
                WASM_MOVING_UNCOVERED_GEOMETRY_UPLOAD_BYTES,
            )
        } else {
            (
                WASM_MOVING_UPLOADS_PER_FRAME,
                WASM_MOVING_GEOMETRY_UPLOAD_BYTES,
            )
        }
    } else if post_boot {
        (
            WASM_POST_BOOT_UPLOADS_PER_FRAME,
            WASM_POST_BOOT_GEOMETRY_UPLOAD_BYTES,
        )
    } else {
        let boot_bytes = if WASM_BOOT_GEOMETRY_UPLOAD_BYTES < full_bytes {
            WASM_BOOT_GEOMETRY_UPLOAD_BYTES
        } else {
            full_bytes
        };
        (WASM_BOOT_UPLOADS_PER_FRAME, boot_bytes)
    }
}

const fn native_geometry_limits(
    moving: bool,
    uncovered: bool,
    full_bytes: usize,
) -> (usize, usize) {
    if moving && !uncovered {
        (
            NATIVE_MOVING_UPLOADS_PER_FRAME,
            NATIVE_MOVING_GEOMETRY_UPLOAD_BYTES,
        )
    } else {
        (NATIVE_UPLOADS_PER_FRAME, full_bytes)
    }
}

pub(super) const fn geometry_limits_for_platform(
    moving: bool,
    post_boot: bool,
    uncovered: bool,
    is_wasm: bool,
) -> (usize, usize) {
    let full_bytes = FRAME_UPLOAD_BYTES - IMAGE_UPLOAD_FLOOR_BYTES;
    if is_wasm {
        wasm_geometry_limits(moving, post_boot, uncovered, full_bytes)
    } else {
        native_geometry_limits(moving, uncovered, full_bytes)
    }
}

const fn select_phase_micros(
    moving: bool,
    post_boot: bool,
    (moving_us, settled_us, boot_us): (u64, u64, u64),
) -> u64 {
    if moving {
        moving_us
    } else if post_boot {
        settled_us
    } else {
        boot_us
    }
}

fn stationary_geometry_cap(geometry_bytes: usize, image_bytes: usize) -> usize {
    let used = geometry_bytes + image_bytes;
    FRAME_UPLOAD_BYTES
        .saturating_sub(IMAGE_UPLOAD_FLOOR_BYTES.saturating_sub(image_bytes))
        .saturating_sub(used)
}

fn image_upload_allowance(geometry_bytes: usize, image_bytes: usize) -> usize {
    let used = geometry_bytes + image_bytes;
    FRAME_UPLOAD_BYTES
        .saturating_sub(used)
        .max(IMAGE_UPLOAD_FLOOR_BYTES.saturating_sub(image_bytes))
}

/// The bytes the tile uploads of one frame share, across vector, label, raster and DEM
/// tiles. `frame_input` resets it when a new host frame starts.
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
    moving: bool,
    post_boot: bool,
}

impl FrameUploads {
    /// Resets this frame's byte and tile allowances and records whether the camera moved.
    pub(crate) fn begin_frame(&mut self, moving: bool) {
        self.geometry_bytes = 0;
        self.image_bytes = 0;
        self.geometry_started = false;
        self.image_started = false;
        self.geometry_tiles = 0;
        self.moving = moving;
        if moving {
            self.post_boot = true;
        }
    }

    /// Marks whether this frame is part of active continuous camera movement.
    #[cfg(test)]
    pub(crate) fn set_moving(&mut self, moving: bool) {
        self.moving = moving;
        if moving {
            self.post_boot = true;
        }
    }

    /// Whether the current frame is actively moving.
    pub(crate) fn is_moving(&self) -> bool {
        self.moving
    }

    /// Whether the camera has moved at least once since startup.
    #[cfg(test)]
    pub(crate) fn is_post_boot(&self) -> bool {
        self.post_boot
    }

    pub(crate) fn vector_upload_time_budget(&self) -> Option<std::time::Duration> {
        Self::upload_time_budget_for_platform(
            self.moving,
            self.post_boot,
            cfg!(target_arch = "wasm32"),
            (700, 800, 1_500),
        )
    }

    pub(crate) fn sdf_upload_time_budget(&self) -> Option<std::time::Duration> {
        Self::upload_time_budget_for_platform(
            self.moving,
            self.post_boot,
            cfg!(target_arch = "wasm32"),
            (350, 450, 800),
        )
    }

    pub(super) const fn upload_time_budget_for_platform(
        moving: bool,
        post_boot: bool,
        is_wasm: bool,
        micros_by_phase: (u64, u64, u64),
    ) -> Option<std::time::Duration> {
        if !is_wasm {
            return None;
        }
        Some(std::time::Duration::from_micros(select_phase_micros(
            moving,
            post_boot,
            micros_by_phase,
        )))
    }

    pub(super) fn can_take_geometry_for_platform(
        &self,
        bytes: usize,
        uncovered: bool,
        is_wasm: bool,
    ) -> bool {
        if bytes == 0 {
            return true;
        }
        let stationary_cap = stationary_geometry_cap(self.geometry_bytes, self.image_bytes);
        let (_, byte_limit) =
            geometry_limits_for_platform(self.moving, self.post_boot, uncovered, is_wasm);
        let cap = stationary_cap.min(byte_limit.saturating_sub(self.geometry_bytes));
        !self.geometry_started || bytes <= cap
    }

    /// Returns `true` if `bytes` of geometry fits in this frame's remaining allowance, granting
    /// uncovered hole-filling tiles a higher ceiling while moving.
    pub(crate) fn can_take_geometry(&self, bytes: usize, uncovered: bool) -> bool {
        self.can_take_geometry_for_platform(bytes, uncovered, cfg!(target_arch = "wasm32"))
    }

    /// Returns `true` if `bytes` of `kind` fits in this frame's remaining upload allowance without
    /// consuming it yet, so a saturated or skipped upload does not burn the frame budget.
    pub(crate) fn can_take(&self, kind: UploadKind, bytes: usize) -> bool {
        match kind {
            UploadKind::Geometry => self.can_take_geometry(bytes, false),
            UploadKind::Image => {
                if bytes == 0 {
                    return true;
                }
                let allowed = image_upload_allowance(self.geometry_bytes, self.image_bytes);
                !self.image_started || bytes <= allowed
            }
        }
    }

    /// Admits a geometry upload of `bytes`, granting `uncovered` tiles emergency moving headroom.
    pub(crate) fn take_geometry(&mut self, bytes: usize, uncovered: bool) -> bool {
        if !self.can_take_geometry(bytes, uncovered) {
            return false;
        }
        if bytes > 0 {
            self.geometry_started = true;
            self.geometry_bytes += bytes;
        }
        true
    }

    /// Admits an upload of `bytes` of `kind`, counting it, or refuses it for a later frame.
    pub(crate) fn take(&mut self, kind: UploadKind, bytes: usize) -> bool {
        match kind {
            UploadKind::Geometry => self.take_geometry(bytes, false),
            UploadKind::Image => {
                if !self.can_take(UploadKind::Image, bytes) {
                    return false;
                }
                if bytes > 0 {
                    self.image_started = true;
                    self.image_bytes += bytes;
                }
                true
            }
        }
    }

    /// Counts a tile whose geometry this frame uploaded, of [`UPLOADS_PER_FRAME`].
    pub(crate) fn count_geometry_tile(&mut self) {
        self.geometry_tiles += 1;
    }

    pub(super) fn geometry_tiles_exhausted_for_platform(
        &self,
        uncovered: bool,
        is_wasm: bool,
    ) -> bool {
        let (limit, _) =
            geometry_limits_for_platform(self.moving, self.post_boot, uncovered, is_wasm);
        self.geometry_tiles >= limit
    }

    /// Whether this frame's geometry tile allowance is used up for a tile (`uncovered` tiles have
    /// higher moving headroom so visible holes never stall behind covered refinements).
    pub(crate) fn geometry_tiles_exhausted_for(&self, uncovered: bool) -> bool {
        self.geometry_tiles_exhausted_for_platform(uncovered, cfg!(target_arch = "wasm32"))
    }

    /// Bytes admitted this frame.
    #[cfg(test)]
    pub(crate) fn used(&self) -> usize {
        self.geometry_bytes + self.image_bytes
    }

    /// Starts the next frame's allowance.
    #[cfg(test)]
    pub(crate) fn reset(&mut self) {
        let post_boot = self.post_boot;
        *self = Self::default();
        self.post_boot = post_boot;
    }
}

/// Callback resource supplying resident memory estimates per tile (`maplibre::map::runtime::tile_memory`).
#[derive(Clone, Copy)]
pub struct TileBytesEstimator(
    pub fn(&crate::tcs::tiles::Tiles, crate::coords::WorldTileCoords) -> usize,
);
