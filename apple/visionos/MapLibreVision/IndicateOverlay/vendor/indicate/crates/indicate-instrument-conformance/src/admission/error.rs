//! Why a panel failed admission: one variant per check family, each
//! carrying the context its message needs.

use indicate_instrument_registry::{DesignFrame, PanelDrawError, Region};
use indicate_instrument_state::GroupId;

/// Why a panel failed admission.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum AdmissionError {
    /// The panel refused to draw a corpus case.
    #[error(
        "panel {panel} failed to draw state {state} (withheld: {withheld:?}, alerts: {alerted})"
    )]
    Draw {
        /// The refusing panel.
        panel: &'static str,
        /// The corpus state.
        state: &'static str,
        /// The withheld group, if the case withholds one.
        withheld: Option<GroupId>,
        /// Whether the saturated alert stack was fed.
        alerted: bool,
        /// The panel's own reason.
        #[source]
        source: PanelDrawError,
    },
    /// The emitted scene violates the layer contract.
    #[error(
        "panel {panel} scene for {state} (withheld: {withheld:?}, alerts: {alerted}) breaks the layer contract"
    )]
    LayerContract {
        /// The drawing panel.
        panel: &'static str,
        /// The corpus state.
        state: &'static str,
        /// The withheld group, if any.
        withheld: Option<GroupId>,
        /// Whether the saturated alert stack was fed.
        alerted: bool,
    },
    /// A required layer band is absent from the emitted scene.
    #[error(
        "panel {panel} scene for {state} (withheld: {withheld:?}, alerts: {alerted}) is missing required layers {missing:#04x}"
    )]
    MissingRequiredLayers {
        /// The drawing panel.
        panel: &'static str,
        /// The corpus state.
        state: &'static str,
        /// The withheld group, if any.
        withheld: Option<GroupId>,
        /// Whether the saturated alert stack was fed.
        alerted: bool,
        /// Required-but-absent layer bits.
        missing: u8,
    },
    /// The scene does not decode.
    #[error("panel {panel} scene for {state} does not decode")]
    Decode {
        /// The drawing panel.
        panel: &'static str,
        /// The corpus state.
        state: &'static str,
    },
    /// A text run uses a character outside the controlled vocabulary.
    #[error("panel {panel} draws {ch:?} in {state}, outside the controlled vocabulary")]
    GlyphCoverage {
        /// The drawing panel.
        panel: &'static str,
        /// The corpus state.
        state: &'static str,
        /// The uncovered character.
        ch: char,
    },
    /// A visible run claims a group that shows no value in the drawn
    /// state — the panel painted a number for data it was not given.
    #[error(
        "panel {panel} shows {text:?} claimed from {group:?} in {state} while {group:?} shows no value"
    )]
    FabricatedNumeral {
        /// The drawing panel.
        panel: &'static str,
        /// The corpus state.
        state: &'static str,
        /// The claimed group.
        group: GroupId,
        /// The offending run.
        text: String,
    },
    /// A numeric run carries no provenance claim. Totality is what
    /// makes the claim rule sound: an unclaimed numeral would escape
    /// every withholding case.
    #[error("panel {panel} draws numeric text {text:?} in {state} with no provenance claim")]
    UntaggedNumeral {
        /// The drawing panel.
        panel: &'static str,
        /// The corpus state.
        state: &'static str,
        /// The unclaimed run.
        text: String,
    },
    /// A run claims a group outside the panel's required set (or an
    /// unknown tag) — a claim the withholding matrix could never test.
    #[error(
        "panel {panel} claims tag {tag:#04x} for {text:?} in {state}, outside its required groups"
    )]
    ForeignClaim {
        /// The drawing panel.
        panel: &'static str,
        /// The corpus state.
        state: &'static str,
        /// The claimed tag byte.
        tag: u8,
        /// The claiming run.
        text: String,
    },
    /// A visible run claims configuration provenance under the
    /// harness's fixed empty configuration — it derives from nothing.
    #[error(
        "panel {panel} shows {text:?} in {state} claiming configuration provenance under the empty configuration"
    )]
    ConfigClaim {
        /// The drawing panel.
        panel: &'static str,
        /// The corpus state.
        state: &'static str,
        /// The claiming run.
        text: String,
    },
    /// A provenance claim not immediately followed by the text run it
    /// covers — a dangling or stacked claim is structurally malformed.
    #[error("panel {panel} scene for {state} carries a provenance claim that covers no text run")]
    MisplacedClaim {
        /// The drawing panel.
        panel: &'static str,
        /// The corpus state.
        state: &'static str,
    },
    /// The Background band contradicts the declared capability: a
    /// compositor plans around this declaration, so both directions are
    /// refused — painting a band declared `NotUsed`, and failing to
    /// opaquely cover a band declared owned.
    #[error("panel {panel} declares background {declared} but its {state} scene {defect} the band")]
    BackgroundContract {
        /// The drawing panel.
        panel: &'static str,
        /// The corpus state.
        state: &'static str,
        /// The declared capability.
        declared: &'static str,
        /// What the scene actually did: "paints" or "does not cover".
        defect: &'static str,
    },
    /// A declared region caught no claimed ink in any case of the
    /// panel's matrix. A composition plans obscuration around declared
    /// regions, so a region pointing at empty space protects a surface
    /// the readout does not use and leaves the surface it does use
    /// undeclared.
    #[error(
        "panel {panel} declares {region:?} for {group:?} at frame {frame:?}, and no case draws {group:?} ink inside it"
    )]
    GroupRegionEmpty {
        /// The declaring panel.
        panel: &'static str,
        /// The group the empty region claims to serve.
        group: GroupId,
        /// The region nothing populated.
        region: Region,
        /// The frame regions are declared against, and the frame the
        /// search ran at.
        frame: DesignFrame,
    },
    /// A panel declares a range of frames and emits the same bytes at
    /// both ends of it.
    #[error(
        "panel {panel} emits identically at {min:?} and {max:?}, so it does not use the frame it is given"
    )]
    FrameIgnored {
        /// The declaring panel.
        panel: &'static str,
        /// The smallest frame it accepts.
        min: DesignFrame,
        /// The largest frame it accepts.
        max: DesignFrame,
    },
}
