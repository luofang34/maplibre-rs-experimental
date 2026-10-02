//! The panel descriptor: everything a shell needs to wire one panel.

use indicate_alerts::AlertOutput;
use indicate_instrument_scene::{SceneError, SceneWriter};
use indicate_instrument_state::{AircraftState, GroupId, PanelData};

use crate::config::{ConfigBlob, ConfigError, ConfigKey};
use crate::group_set::GroupSet;

/// A panel's draw entry point: pure resolved-state → scene, with its
/// configuration delivered as the validated blob the shell accepted and
/// the logical frame it must lay out against.
///
/// The frame is an input, not a configuration key. Configuration is an
/// optional schema-gated wire blob and the admission harness draws the
/// empty one deliberately, so a size-adaptive panel that read its frame
/// from configuration would be unadmittable by construction. It sits
/// before the writer because it is an input and the writer is the sink.
pub type DrawFn = fn(
    &PanelData,
    &ConfigBlob<'_>,
    Option<&AlertOutput>,
    DesignFrame,
    &mut SceneWriter<'_>,
) -> Result<(), PanelDrawError>;

/// Why a panel draw failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum PanelDrawError {
    /// The scene writer refused a command. The typed reason rides
    /// along; [`SceneError`] carries no `Display` of its own.
    #[error("scene writer refused a command")]
    Scene(SceneError),
    /// The configuration blob does not decode for this panel.
    #[error("config error: {0}")]
    Config(#[from] ConfigError),
}

impl From<SceneError> for PanelDrawError {
    fn from(error: SceneError) -> Self {
        PanelDrawError::Scene(error)
    }
}

/// What the panel does with the `Background` scene band.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackgroundCapability {
    /// Never draws the band (a compositor may still not use it).
    NotUsed,
    /// Owns the band with opaque content of its own.
    Opaque,
    /// Draws the band by default but cedes it on request — the panel
    /// stays complete when the band is supplied elsewhere (SVS, video).
    Cedeable,
}

/// The logical space a panel draws against; backends scale, panels
/// never see viewport pixels.
///
/// One frame is one draw. A panel declares the *range* of frames it can
/// lay out against ([`PanelDescriptor::frame_min`] through
/// [`PanelDescriptor::frame_max`]) and receives the chosen one as a
/// [`DrawFn`] argument.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DesignFrame {
    /// Logical width.
    pub width: f32,
    /// Logical height.
    pub height: f32,
}

/// An axis-aligned rectangle in design-frame units.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Region {
    /// Left edge.
    pub x: f32,
    /// Top edge.
    pub y: f32,
    /// Width.
    pub width: f32,
    /// Height.
    pub height: f32,
}

impl Region {
    /// The whole of `frame`, as a region at its origin.
    pub const fn of(frame: DesignFrame) -> Region {
        Region {
            x: 0.0,
            y: 0.0,
            width: frame.width,
            height: frame.height,
        }
    }

    /// Right edge.
    pub fn right(&self) -> f32 {
        self.x + self.width
    }

    /// Bottom edge.
    pub fn bottom(&self) -> f32 {
        self.y + self.height
    }

    /// Whether every coordinate is finite and both extents are
    /// positive. A degenerate region names no surface, so it cannot be
    /// a readout's or a slot's.
    pub fn is_sound(&self) -> bool {
        self.x.is_finite()
            && self.y.is_finite()
            && self.width.is_finite()
            && self.height.is_finite()
            && self.width > 0.0
            && self.height > 0.0
    }

    /// Whether the two overlap on a positive area; sharing only an edge
    /// does not count.
    pub fn intersects(&self, other: &Region) -> bool {
        self.x < other.right()
            && other.x < self.right()
            && self.y < other.bottom()
            && other.y < self.bottom()
    }

    /// Whether `other` lies wholly inside this region.
    pub fn contains(&self, other: &Region) -> bool {
        other.x >= self.x
            && other.y >= self.y
            && other.right() <= self.right()
            && other.bottom() <= self.bottom()
    }

    /// The same region moved by `(dx, dy)` — a panel's own design-space
    /// rectangle placed at a composition slot's origin.
    pub fn translated(&self, dx: f32, dy: f32) -> Region {
        Region {
            x: self.x + dx,
            y: self.y + dy,
            ..*self
        }
    }
}

/// A panel-contributed extreme state for conformance and digest runs:
/// the panel names the situations that stress it beyond the shared
/// canonical set.
#[derive(Debug, Clone, Copy)]
pub struct ExtremeState {
    /// Stable identity of the fixture (lowercase, digits, dashes).
    pub id: &'static str,
    /// Builds the state; a plain fn keeps descriptors `static`.
    pub build: fn() -> AircraftState,
}

/// One panel, as data. A shell composes descriptors into a registry and
/// consumes only this — no shell may hold a panel list, index, or layer
/// mask of its own (ADR-0029).
#[derive(Debug, Clone, Copy)]
pub struct PanelDescriptor {
    /// Stable identity (lowercase, digits, dashes): canvas ids, health
    /// keys, and evidence records key off this.
    pub id: &'static str,
    /// Operator-facing title.
    pub title: &'static str,
    /// Scene layers that must be present and complete in every frame,
    /// as a bitset over [`indicate_instrument_scene::LayerId`].
    pub required_layers: u8,
    /// State groups this panel consumes — the withholding matrix the
    /// admission harness drives honest-status checks from.
    pub required_groups: GroupSet,
    /// The smallest frame this panel lays out against: the readability
    /// floor, where conspicuity must still hold (AIR-OUT-004). Group
    /// regions are validated against it, because the floor is where a
    /// region has to fit.
    pub frame_min: DesignFrame,
    /// The largest frame this panel lays out against.
    pub frame_max: DesignFrame,
    /// Per-axis quantization step: admissible dimensions are
    /// `frame_min + k * step`, which keeps the behaviour space and the
    /// evidence matrix finite. A degenerate range declares any positive
    /// step, since the only admissible `k` is zero.
    pub frame_step: (f32, f32),
    /// Smallest width/height ratio the layout supports. Per-axis bounds
    /// alone would admit shapes the layout was never designed for, so
    /// the ratio is bounded separately.
    pub aspect_min: f32,
    /// Largest width/height ratio the layout supports.
    pub aspect_max: f32,
    /// The pinned evidence sizes: the frames the digest, the admission
    /// matrix, and the raster baselines are taken at. Must include both
    /// [`PanelDescriptor::frame_min`] and [`PanelDescriptor::frame_max`],
    /// and every entry must itself be admissible.
    pub canonical_frames: &'static [DesignFrame],
    /// What the panel does with the `Background` band.
    pub background: BackgroundCapability,
    /// Configuration keys this panel understands; a shell refuses a
    /// blob carrying any other key.
    pub config_schema: &'static [ConfigKey],
    /// Where each consumed group's *value* is drawn: the pointed
    /// readout, the data box — not the scale ladder or the tick labels
    /// beside it, which carry the same group's claim because a numeral
    /// must carry one.
    ///
    /// Validated for geometry here — inside [`PanelDescriptor::frame_min`],
    /// non-degenerate, only for required groups — and asserted by the
    /// admission harness for non-vacuity: every declared region must be
    /// populated by claimed ink somewhere in the panel's case matrix. A
    /// region over blank space is what the assertion is for, because
    /// screen composition plans obscuration around these rectangles and
    /// would protect a surface the readout does not use.
    ///
    /// Silence is a declaration too: a group with no region here is not
    /// judged positionally, because some readouts share a strip with a
    /// neighbouring group's ink and no geometry separates them.
    ///
    /// Criticality content is bounded separately and by measurement
    /// rather than declaration ([`crate::PanelCriticality`]).
    pub group_regions: &'static [(GroupId, Region)],
    /// Panel-contributed stress fixtures beyond the canonical states.
    pub extreme_states: &'static [ExtremeState],
    /// Pinned raster baselines: per canonical frame, the SHA-256 hex of
    /// the reference rasterizer's render of the shared "typical" corpus
    /// state at that frame. Every entry must name a frame in
    /// [`PanelDescriptor::canonical_frames`]; an empty slice is the
    /// honest declaration for a panel with no baseline pinned yet.
    pub raster_baselines: &'static [(DesignFrame, &'static str)],
    /// The draw entry point.
    pub draw: DrawFn,
}
