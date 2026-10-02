//! Why a screen composition was refused. Every variant names the slot
//! index, because a slot index is the composition's own vocabulary —
//! it is the z-order, and it is what an author edits.

use indicate_instrument_descriptor::{DesignFrame, Region};

/// Why [`crate::validate_composition`] refused a descriptor.
#[derive(Debug, Clone, Copy, PartialEq, thiserror::Error)]
pub enum CompositionError {
    /// The composition places nothing.
    #[error("composition declares no slots")]
    NoSlots,
    /// More slots than the composed-frame budget allows.
    #[error("composition declares {slots} slots, over the ceiling of {ceiling}")]
    TooManySlots {
        /// The declared slot count.
        slots: usize,
        /// [`crate::MAX_COMPOSITION_SLOTS`].
        ceiling: usize,
    },
    /// The logical screen is not a frame.
    #[error("screen frame {screen:?} is not finite and positive on both axes")]
    BadScreen {
        /// The offending declaration.
        screen: DesignFrame,
    },
    /// A slot names a panel the registry does not compose.
    #[error("slot {slot} names panel {panel}, which this registry does not compose")]
    UnknownPanel {
        /// The offending slot.
        slot: usize,
        /// The name it gave.
        panel: &'static str,
    },
    /// A slot rect is non-finite or has a non-positive extent.
    #[error("slot {slot} declares {rect:?}, which is not a finite, non-degenerate rectangle")]
    SlotRectDegenerate {
        /// The offending slot.
        slot: usize,
        /// Its rect.
        rect: Region,
    },
    /// A slot rect leaves the screen. Nothing clips it back: a slot is
    /// a placement, and a placement off the screen paints nowhere the
    /// composition can account for.
    #[error("slot {slot} rect {rect:?} is not inside the screen {screen:?}")]
    SlotOutsideScreen {
        /// The offending slot.
        slot: usize,
        /// Its rect.
        rect: Region,
        /// The declared screen.
        screen: DesignFrame,
    },
    /// A slot asks a panel for a size it never declared. Composition is
    /// placement only — there is no `SCALE` opcode and a scene is never
    /// rewritten to fit — so the slot's dimensions *are* the frame the
    /// panel is asked to emit.
    #[error("slot {slot} asks {panel} for frame {frame:?}, which it does not support")]
    SlotFrameUnsupported {
        /// The offending slot.
        slot: usize,
        /// The panel asked.
        panel: &'static str,
        /// The frame the slot's dimensions ask for.
        frame: DesignFrame,
    },
    /// A slot lies entirely under opaque slots above it. Painting it
    /// costs a full scene and shows nothing, so it is a declaration
    /// error rather than a rendering curiosity.
    #[error("slot {slot} is wholly covered by the opaque slots above it")]
    DeadSlot {
        /// The dead slot.
        slot: usize,
    },
    /// No criticality band was measured for this panel at this frame,
    /// so nothing bounds where its warnings land and no obscuration
    /// above it can be judged.
    #[error("no criticality band is pinned for {panel} at frame {frame:?} (slot {slot})")]
    CriticalityUnknown {
        /// The slot whose panel has no band.
        slot: usize,
        /// The panel.
        panel: &'static str,
        /// The frame its slot asks for.
        frame: DesignFrame,
    },
    /// A slot covers a lower panel's ordinary readout surface without
    /// the composition saying so. Obscuration is permitted, drift is
    /// not: the lower panel's id must appear in the covering slot's
    /// `occludes` list, where a reviewer sees it.
    #[error(
        "slot {upper} covers readout surface {region:?} of {panel} in slot {lower} without declaring it"
    )]
    UndeclaredOcclusion {
        /// The covering slot.
        upper: usize,
        /// The covered slot.
        lower: usize,
        /// The covered panel.
        panel: &'static str,
        /// The covered surface, in screen units.
        region: Region,
    },
    /// A slot overlaps a lower panel whose criticality band was
    /// measured empty. An empty measurement is the absence of a
    /// witness, not a proof of absence: no case that ran drew warning
    /// ink, which says nothing about where warning ink goes.
    #[error(
        "slot {upper} overlaps {panel} in slot {lower}, whose criticality band no case ever witnessed"
    )]
    CriticalityUnwitnessed {
        /// The covering slot.
        upper: usize,
        /// The covered slot.
        lower: usize,
        /// The covered panel.
        panel: &'static str,
    },
    /// A slot covers a lower panel's criticality band. Declaring an
    /// obscuration does not licence this and no list can: a declared
    /// obscuration may cover ordinary symbology, never a warning or a
    /// failure indication (AIR-OUT-011). The band is what was measured
    /// in the `Annunciation` and `Failure` layers, so a panel's
    /// simulation labelling is protected exactly when the panel paints
    /// it there.
    #[error(
        "slot {upper} covers the criticality band {band:?} of {panel} in slot {lower}; no declaration permits this"
    )]
    CriticalityObscured {
        /// The covering slot.
        upper: usize,
        /// The covered slot.
        lower: usize,
        /// The covered panel.
        panel: &'static str,
        /// The covered band, in screen units.
        band: Region,
    },
}
