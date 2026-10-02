//! Built-in panel descriptors: the registry entries every shell
//! composes (ADR-0029, ADR-0033).
//!
//! Each descriptor owns its panel's full contract — identity, masks,
//! required groups, the frame range it lays out against, honest-status
//! regions, its own extreme states, and the pinned raster baselines —
//! so a shell consumes composition data and never holds a panel list,
//! index, or mask of its own.

mod criticality_bands;
mod extreme_states;
mod supplementary;

use indicate_alerts::AlertOutput;
use indicate_instrument_descriptor::{
    BackgroundCapability, ConfigBlob, DesignFrame, ExtremeState, GroupSet, PanelDescriptor,
    PanelDrawError, PanelSet, Region,
};
use indicate_instrument_scene::{LayerId, SceneWriter};
use indicate_instrument_state::{GroupId, PanelData};

use crate::pfd::PFD_CONFIG_SCHEMA;
use crate::{BUILTIN_FRAME, PfdConfig, draw_hsi, draw_pfd};

const fn layer_bit(layer: LayerId) -> u8 {
    1u8 << layer.to_u8()
}

/// Every panel here declares one admissible frame, so the only whole
/// multiple of the step that lands in range is zero and every positive
/// step describes the same admissible set. The registry asks only that
/// it be positive.
const FRAME_STEP: (f32, f32) = (1.0, 1.0);

/// A bracket around the shipped 4:3 ratio rather than an equality: the
/// frame's ratio is an f32 division, and a bound written as a decimal
/// literal would not reliably equal it.
const ASPECT_MIN: f32 = 1.30;
/// Upper end of the supported width/height ratio.
const ASPECT_MAX: f32 = 1.37;

/// The pinned evidence sizes: one frame, which is both the floor and
/// the ceiling of the declared range.
const CANONICAL_FRAMES: &[DesignFrame] = &[BUILTIN_FRAME];

fn draw_pfd_panel(
    data: &PanelData,
    config: &ConfigBlob<'_>,
    alerts: Option<&AlertOutput>,
    frame: DesignFrame,
    scene: &mut SceneWriter<'_>,
) -> Result<(), PanelDrawError> {
    let cfg = PfdConfig::from_config(config, frame)?;
    draw_pfd(data, &cfg, alerts, frame, scene)?;
    Ok(())
}

fn draw_hsi_panel(
    data: &PanelData,
    config: &ConfigBlob<'_>,
    alerts: Option<&AlertOutput>,
    frame: DesignFrame,
    scene: &mut SceneWriter<'_>,
) -> Result<(), PanelDrawError> {
    // The HSI takes no configuration; the empty schema makes any keyed
    // blob a shell-side rejection before this runs, and a re-check here
    // keeps the property when a shell skips its gate.
    config.require_schema(HSI_DESCRIPTOR.config_schema)?;
    draw_hsi(data, alerts, frame, scene)?;
    Ok(())
}

/// The primary flight display.
pub const PFD_DESCRIPTOR: PanelDescriptor = PanelDescriptor {
    id: "pfd",
    title: "PFD",
    required_layers: layer_bit(LayerId::Attitude)
        | layer_bit(LayerId::Tapes)
        | layer_bit(LayerId::Guidance)
        | layer_bit(LayerId::Annunciation),
    required_groups: GroupSet::of(&[
        GroupId::Attitude,
        GroupId::Kinematics,
        GroupId::Air,
        GroupId::Selections,
        GroupId::Trust,
        GroupId::Altitude,
        GroupId::Dynamics,
        GroupId::FlightDirector,
        GroupId::ApModes,
        GroupId::ApTargets,
        GroupId::Guidance,
        GroupId::SpeedPresentation,
    ]),
    frame_min: BUILTIN_FRAME,
    frame_max: BUILTIN_FRAME,
    frame_step: FRAME_STEP,
    aspect_min: ASPECT_MIN,
    aspect_max: ASPECT_MAX,
    canonical_frames: CANONICAL_FRAMES,
    background: BackgroundCapability::Cedeable,
    config_schema: PFD_CONFIG_SCHEMA,
    // Value-readout surfaces, keyed by the group whose data the number
    // comes from, drawn around the pointed readout and deliberately
    // excluding the scale ladder beside it — the rungs claim the same
    // group, because a numeral must claim the group it came from, and
    // they are not what these rectangles name.
    //
    // Admission asserts each of these is populated: some case must draw
    // a run claiming the group at the surface. Screen composition then
    // plans obscuration around them, which is why a rectangle over
    // blank space would be worse than no rectangle at all.
    group_regions: &[
        // IAS pointed readout value (the run anchors at x 40; the
        // scale ladder's runs anchor at x 70 and stay outside).
        (
            GroupId::Air,
            Region {
                x: 20.0,
                y: 162.0,
                width: 40.0,
                height: 36.0,
            },
        ),
        // Baro setting box.
        (
            GroupId::Air,
            Region {
                x: 390.0,
                y: 335.0,
                width: 90.0,
                height: 25.0,
            },
        ),
        // TAS box at the head of the speed tape.
        (
            GroupId::Air,
            Region {
                x: 0.0,
                y: 0.0,
                width: 90.0,
                height: 25.0,
            },
        ),
        // Groundspeed box.
        (
            GroupId::Kinematics,
            Region {
                x: 0.0,
                y: 335.0,
                width: 90.0,
                height: 25.0,
            },
        ),
        // Altitude pointed readout value (anchors at x 442; the scale
        // ladder anchors at x 408 and stays outside). The value is
        // kinematic altitude; the altitude group only qualifies its
        // datum.
        (
            GroupId::Kinematics,
            Region {
                x: 424.0,
                y: 162.0,
                width: 36.0,
                height: 36.0,
            },
        ),
        // The selected-altitude box carries no region: it shares the
        // altitude tape strip with kinematics ladder ink whose y moves
        // with altitude, so no region geometry separates the two. Its
        // honest-status coverage is the provenance claim on the
        // selected-altitude run itself — a fabricated selection is
        // refused wherever it is drawn.
        // VSI numeral strip (kinematic vertical speed), bounded to
        // exclude the selected-altitude box above and the baro box
        // below, whose numerals belong to other groups.
        (
            GroupId::Kinematics,
            Region {
                x: 440.0,
                y: 28.0,
                width: 26.0,
                height: 307.0,
            },
        ),
    ],
    extreme_states: &[
        ExtremeState {
            id: "extended-flight-guidance",
            build: indicate_instrument_state::abi::v8::fixtures::extended,
        },
        ExtremeState {
            id: "unusual-inverted",
            build: extreme_states::pfd_unusual_inverted,
        },
        ExtremeState {
            id: "readout-extremes",
            build: extreme_states::pfd_readout_extremes,
        },
        ExtremeState {
            id: "director-engaged",
            build: extreme_states::pfd_director_engaged,
        },
        ExtremeState {
            id: "level-accelerating",
            build: extreme_states::pfd_level_accelerating,
        },
    ],
    // Reference-rasterizer frame hash over the shared typical state, one
    // per canonical frame — pinned per panel here so a panel travels
    // with its own regression baselines; the raster crate asserts them
    // (REN-03).
    raster_baselines: &[(
        BUILTIN_FRAME,
        "1ffd3ff80047da054823de9cef9a5017b9c8b08d4b048b0db7db7ea1f827eea1",
    )],
    draw: draw_pfd_panel,
};

/// The horizontal situation indicator.
pub const HSI_DESCRIPTOR: PanelDescriptor = PanelDescriptor {
    id: "hsi",
    title: "HSI",
    required_layers: layer_bit(LayerId::Attitude)
        | layer_bit(LayerId::Tapes)
        | layer_bit(LayerId::Guidance)
        | layer_bit(LayerId::Annunciation),
    required_groups: GroupSet::of(&[
        GroupId::Kinematics,
        GroupId::Nav,
        GroupId::Wind,
        GroupId::Selections,
        GroupId::Trust,
        GroupId::Heading,
        GroupId::Variation,
        GroupId::BearingPointers,
    ]),
    frame_min: BUILTIN_FRAME,
    frame_max: BUILTIN_FRAME,
    frame_step: FRAME_STEP,
    aspect_min: ASPECT_MIN,
    aspect_max: ASPECT_MAX,
    canonical_frames: CANONICAL_FRAMES,
    background: BackgroundCapability::Opaque,
    config_schema: &[],
    group_regions: &[
        // Wind box.
        (
            GroupId::Wind,
            Region {
                x: 2.0,
                y: 2.0,
                width: 112.0,
                height: 48.0,
            },
        ),
        // Distance box.
        (
            GroupId::Nav,
            Region {
                x: 366.0,
                y: 2.0,
                width: 112.0,
                height: 48.0,
            },
        ),
        // Course box.
        (
            GroupId::Nav,
            Region {
                x: 2.0,
                y: 322.0,
                width: 112.0,
                height: 36.0,
            },
        ),
        // Heading-select box.
        (
            GroupId::Selections,
            Region {
                x: 366.0,
                y: 322.0,
                width: 112.0,
                height: 36.0,
            },
        ),
        // Digital heading readout at the panel top: the panel's
        // primary heading number must dash out with the sample gone.
        (
            GroupId::Heading,
            Region {
                x: 206.0,
                y: 2.0,
                width: 68.0,
                height: 26.0,
            },
        ),
    ],
    extreme_states: &[
        ExtremeState {
            id: "reciprocal-course",
            build: extreme_states::hsi_reciprocal_course,
        },
        ExtremeState {
            id: "track-up",
            build: extreme_states::hsi_track_up,
        },
        ExtremeState {
            id: "bearing-split-references",
            build: extreme_states::hsi_bearing_split_references,
        },
    ],
    raster_baselines: &[(
        BUILTIN_FRAME,
        "cfe8fb5eba7a2947aa69138e934f7a90be07580be22532cda099eedf990ea8ae",
    )],
    draw: draw_hsi_panel,
};

/// The panels this crate ships, in shell display order.
pub const BUILTIN_PANELS: &[PanelDescriptor] = &[
    PFD_DESCRIPTOR,
    HSI_DESCRIPTOR,
    AUTOFLIGHT_DESCRIPTOR,
    MONITOR_DESCRIPTOR,
];

/// The panels this crate ships, as the set a shell names.
///
/// A shell composing this crate alongside another provider names sets
/// rather than panels, so gaining a panel here does not edit any
/// shell. Set identity stays out of the scene digest, so this composes
/// to the same digest as the bare slice.
pub const BUILTIN_SET: PanelSet = PanelSet {
    id: "builtin",
    panels: BUILTIN_PANELS,
};

/// Every set this crate exports, in the order the layer-profile table
/// lists their panels.
///
/// A check that named the sets it walked would keep passing when a
/// third set arrived, which is how the configuration panel reached a
/// release with no row in that table. Anything asking "what does this
/// crate ship?" asks here.
pub const ALL_SETS: &[PanelSet] = &[BUILTIN_SET, CONFIG_SET];

/// The pinned scene digest over [`BUILTIN_PANELS`] and the canonical
/// corpus (ADR-0033): the composition contract every build target must
/// reproduce — the host (bench and unit pin) and the wasm build (the
/// script pins the exported value against its own literal). A shell's
/// LIVE rendering shares identity with this corpus structurally, by
/// drawing through the same descriptors, rather than by digest. The
/// value moves once per deliberate contract change, re-pinned with a
/// review note saying why.
pub const BUILTIN_SCENE_DIGEST: &str =
    "edf5815cc83179cfa7f5b8871bbe6e945dbf04d0339e9f5e55cb3a2922deff51";

pub use criticality_bands::BUILTIN_CRITICALITY_BANDS;
pub use supplementary::{
    AUTOFLIGHT_DESCRIPTOR, CONFIG_DESCRIPTOR, CONFIG_PANELS, CONFIG_SET, MONITOR_DESCRIPTOR,
};

#[cfg(test)]
mod digest_tests;
#[cfg(test)]
mod layer_profile_doc_tests;
#[cfg(test)]
mod tests;
