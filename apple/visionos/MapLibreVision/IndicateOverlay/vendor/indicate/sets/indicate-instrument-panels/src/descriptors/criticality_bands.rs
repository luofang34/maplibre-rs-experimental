//! The measured obscuration bands of the shipped panels.
//!
//! These are evidence rather than design: the harness measures them
//! from the emitted scenes and refuses a disagreement, so they live
//! beside the descriptors rather than inside them.

use indicate_instrument_descriptor::{CriticalityBands, PanelCriticality, Region};

use crate::BUILTIN_FRAME;

/// The measured criticality bands of [`crate::BUILTIN_PANELS`], pinned beside
/// the raster baselines: the union `Annunciation`/`Failure` ink bound
/// per panel × canonical frame, over the whole canonical × extreme ×
/// withheld × alerted case matrix. A screen composition validates its
/// obscuration against these.
///
/// The alert axis is what makes these honest. A composed frame fans one
/// `AlertOutput` to every slot, and all three panels draw the shared
/// alert stack into `Annunciation`; a band measured only on quiet
/// frames would exclude every alert row and licence covering warnings.
/// Each band below therefore reaches y 352, the stack's bottom row.
///
/// A shell holds this as data. The admission harness re-derives the
/// same values from the emitted scenes and its test refuses a
/// disagreement, so a paint change that moves a warning moves the pin
/// deliberately rather than silently widening what may be covered.
///
/// Read the monitor's band for what it is: the alert stack, and only
/// that. Its own `MON` flag and full-frame failure X are gated on a
/// channel status no corpus or extreme state produces, so they were
/// never drawn and are not in the bound. A set that wants them
/// protected contributes a state that drives them.
pub const BUILTIN_CRITICALITY_BANDS: CriticalityBands = CriticalityBands {
    panels: &[
        PanelCriticality {
            panel: "pfd",
            frame: BUILTIN_FRAME,
            band: Some(Region {
                x: 6.0,
                y: 5.4,
                width: 468.0,
                height: 346.6,
            }),
        },
        PanelCriticality {
            panel: "hsi",
            frame: BUILTIN_FRAME,
            band: Some(Region {
                x: 98.0,
                y: 48.0,
                width: 284.0,
                height: 304.0,
            }),
        },
        PanelCriticality {
            panel: "autoflight",
            frame: BUILTIN_FRAME,
            band: Some(Region {
                x: 51.9,
                y: 55.4,
                width: 378.8143,
                height: 296.6,
            }),
        },
        PanelCriticality {
            panel: "monitor",
            frame: BUILTIN_FRAME,
            band: Some(Region {
                x: 100.0,
                y: 276.0,
                width: 90.85715,
                height: 76.0,
            }),
        },
    ],
};
