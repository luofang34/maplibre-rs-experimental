//! The panels that supplement the primary flight instruments.
//!
//! None of these carries primary-flight credit: the monitor reports
//! non-flight status, the autoflight annunciator reports what the
//! automation is doing, and the configuration panel reports what the
//! airframe is set to. They sit apart from the PFD and HSI descriptors
//! for that reason, not because of how much room they take.

use indicate_alerts::AlertOutput;
use indicate_instrument_descriptor::{
    BackgroundCapability, ConfigBlob, DesignFrame, ExtremeState, GroupSet, PanelDescriptor,
    PanelDrawError, PanelSet, Region,
};
use indicate_instrument_scene::{LayerId, SceneWriter};
use indicate_instrument_state::{GroupId, PanelData};

use super::extreme_states;
use super::{ASPECT_MAX, ASPECT_MIN, CANONICAL_FRAMES, FRAME_STEP, layer_bit};
use crate::BUILTIN_FRAME;

fn draw_monitor_panel(
    data: &PanelData,
    config: &ConfigBlob<'_>,
    alerts: Option<&AlertOutput>,
    frame: DesignFrame,
    scene: &mut SceneWriter<'_>,
) -> Result<(), PanelDrawError> {
    config.require_schema(MONITOR_DESCRIPTOR.config_schema)?;
    crate::monitor::draw_monitor(data, alerts, frame, scene)?;
    Ok(())
}

/// The machine-monitoring text panel (AIR-IN-014) — the registry's
/// proof of modularity: it exists as this descriptor and a draw
/// function, with no shell change beyond composition.
pub const MONITOR_DESCRIPTOR: PanelDescriptor = PanelDescriptor {
    id: "monitor",
    title: "Monitor",
    required_layers: layer_bit(LayerId::Tapes) | layer_bit(LayerId::Annunciation),
    required_groups: GroupSet::of(&[GroupId::MonitorText]),
    frame_min: BUILTIN_FRAME,
    frame_max: BUILTIN_FRAME,
    frame_step: FRAME_STEP,
    aspect_min: ASPECT_MIN,
    aspect_max: ASPECT_MAX,
    canonical_frames: CANONICAL_FRAMES,
    // The panel owns its band with an opaque ground: text needs it, and
    // declaring anything weaker would hand a compositor a black
    // rectangle it was told is not painted.
    background: BackgroundCapability::Opaque,
    config_schema: &[],
    // The whole text area is the channel's region: with MONITOR_TEXT
    // withheld the panel shows dashes, never lines it was not given.
    group_regions: &[(
        GroupId::MonitorText,
        Region {
            x: 0.0,
            y: 60.0,
            width: 480.0,
            height: 300.0,
        },
    )],
    extreme_states: &[ExtremeState {
        id: "full-channel",
        build: extreme_states::monitor_full_channel,
    }],
    raster_baselines: &[(
        BUILTIN_FRAME,
        "40f44383f3ad46a0bbd65f04afc1d80fb9d94c11acff8dc66edbfcf7b8fa4c01",
    )],
    draw: draw_monitor_panel,
};

fn draw_autoflight_panel(
    data: &PanelData,
    config: &ConfigBlob<'_>,
    alerts: Option<&AlertOutput>,
    frame: DesignFrame,
    scene: &mut SceneWriter<'_>,
) -> Result<(), PanelDrawError> {
    config.require_schema(AUTOFLIGHT_DESCRIPTOR.config_schema)?;
    crate::autoflight::draw_autoflight(data, alerts, frame, scene)?;
    Ok(())
}

/// The autoflight panel shares mode semantics with the PFD and HWD (AIR-IN-015).
pub const AUTOFLIGHT_DESCRIPTOR: PanelDescriptor = PanelDescriptor {
    id: "autoflight",
    title: "Autoflight",
    required_layers: layer_bit(LayerId::Tapes) | layer_bit(LayerId::Annunciation),
    required_groups: GroupSet::of(&[
        GroupId::ApModes,
        GroupId::ApTargets,
        GroupId::FlightDirector,
        GroupId::Guidance,
        GroupId::SpeedPresentation,
    ]),
    frame_min: BUILTIN_FRAME,
    frame_max: BUILTIN_FRAME,
    frame_step: FRAME_STEP,
    aspect_min: ASPECT_MIN,
    aspect_max: ASPECT_MAX,
    canonical_frames: CANONICAL_FRAMES,
    // The panel owns its band with an opaque ground: annunciation text
    // needs one, and declaring anything weaker would hand a compositor
    // a black rectangle it was told is not painted.
    background: BackgroundCapability::Opaque,
    config_schema: &[],
    group_regions: &[
        // The mode band: with the group withheld the band carries the
        // column headings and no mode, never a mode nobody sent.
        (
            GroupId::ApModes,
            Region {
                x: 0.0,
                y: 50.0,
                width: 480.0,
                height: 80.0,
            },
        ),
        // The target rows: withheld, they dash.
        (
            GroupId::ApTargets,
            Region {
                x: 140.0,
                y: 170.0,
                width: 150.0,
                height: 120.0,
            },
        ),
    ],
    extreme_states: &[
        ExtremeState {
            id: "modes-and-targets",
            build: extreme_states::autoflight_engaged,
        },
        ExtremeState {
            id: "extended-flight-guidance",
            build: indicate_instrument_state::abi::v8::fixtures::extended,
        },
        ExtremeState {
            id: "target-against-another-datum",
            build: extreme_states::autoflight_incomparable_target,
        },
    ],
    raster_baselines: &[(
        BUILTIN_FRAME,
        "943b82a02a5e6e4080359cb794b5956a2342da7034d8e2f4f1c6cfb6f95b5fcf",
    )],
    draw: draw_autoflight_panel,
};

fn draw_config_panel(
    data: &PanelData,
    config: &ConfigBlob<'_>,
    alerts: Option<&AlertOutput>,
    frame: DesignFrame,
    scene: &mut SceneWriter<'_>,
) -> Result<(), PanelDrawError> {
    config.require_schema(CONFIG_DESCRIPTOR.config_schema)?;
    crate::config::draw_config(data, alerts, frame, scene)?;
    Ok(())
}

/// The airframe-configuration panel: flap position and trim.
///
/// A conventional-instrument surface, not a primary-flight one, so it
/// ships in its own set rather than joining [`crate::BUILTIN_PANELS`] — a shell
/// composes it when the airframe has the sensors, and the builtin set's
/// composition digest does not move for a panel nobody composed.
pub const CONFIG_DESCRIPTOR: PanelDescriptor = PanelDescriptor {
    id: "config",
    title: "Configuration",
    required_layers: layer_bit(LayerId::Tapes) | layer_bit(LayerId::Annunciation),
    required_groups: GroupSet::of(&[GroupId::AirframeConfig, GroupId::Trust]),
    frame_min: BUILTIN_FRAME,
    frame_max: BUILTIN_FRAME,
    frame_step: FRAME_STEP,
    aspect_min: ASPECT_MIN,
    aspect_max: ASPECT_MAX,
    canonical_frames: CANONICAL_FRAMES,
    background: BackgroundCapability::Opaque,
    config_schema: &[],
    // The two numerals. The dashes a withheld group draws are plain
    // text carrying no claim, so they are not what this region is
    // measured against — only attributed runs are, and the panel
    // attributes exactly the flap reading and the trim reading.
    //
    // The two sit in separate columns with a gap between them, so the
    // width spans both columns and that gap. It does not exclude every
    // unclaimed mark: about ninety units of the trim ladder fall inside
    // it, because the ladder crosses the column the flap numeral needs.
    // The bottom is set by the trim numeral at y 320, not by the flap
    // numeral, which stops at y 290 with the flaps fully extended.
    //
    // Admission does not hold these bounds: `GroupRegionEmpty` fires
    // only when a region catches no claim at all, so a region far
    // smaller than the ink passes it. What holds them is
    // `every_attributed_numeral_lies_inside_the_declared_region` beside
    // the panel.
    group_regions: &[(
        GroupId::AirframeConfig,
        Region {
            x: 170.0,
            y: 50.0,
            width: 170.0,
            height: 290.0,
        },
    )],
    // The corpus reaches one flap position and one trim setting, both
    // mid-scale, so these are the only states that draw either numeral
    // anywhere else. They are coverage of the panel's geometry, not a
    // constraint on the region above: non-vacuity is monotone, so more
    // cases can only make a region easier to populate, never harder.
    extreme_states: &[
        ExtremeState {
            id: "flaps-up-trim-nose-down",
            build: extreme_states::config_low_extremes,
        },
        ExtremeState {
            id: "flaps-full-trim-nose-up",
            build: extreme_states::config_high_extremes,
        },
    ],
    // No baseline until the rasterizer covers this set; the contract
    // asserts none that was never declared.
    raster_baselines: &[],
    draw: draw_config_panel,
};

/// The configuration panel as a set a shell can name.
pub const CONFIG_SET: PanelSet = PanelSet {
    id: "config",
    panels: CONFIG_PANELS,
};

/// The panels in [`CONFIG_SET`].
pub const CONFIG_PANELS: &[PanelDescriptor] = &[CONFIG_DESCRIPTOR];
