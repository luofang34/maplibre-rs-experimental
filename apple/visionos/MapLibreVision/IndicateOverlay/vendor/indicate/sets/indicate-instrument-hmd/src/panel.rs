use indicate_alerts::AlertOutput;
use indicate_instrument_descriptor::{
    BackgroundCapability, ConfigBlob, DesignFrame, GroupSet, PanelDescriptor, PanelDrawError,
    PanelSet,
};
use indicate_instrument_scene::{Anchor, LayerId, PaintMode, Rgba8, SceneWriter};

mod annunciation;
mod attitude;
mod glance;
mod guidance;
mod heading;
mod navigation;
mod readout;
mod recovery;
mod scale;
mod vertical_speed;
pub use glance::HMD_GLANCE_DESCRIPTOR;

const HUD_GREEN: Rgba8 = Rgba8::rgba(90, 255, 130, 255);
use indicate_instrument_state::{GroupId, PanelData, Sig, SignalStatus};
use indicate_instrument_symbology::{fmt_label, palette, safety};

const FRAME: DesignFrame = DesignFrame {
    width: 1200.0,
    height: 600.0,
};

/// A transparent instrument overlay, independent of the conventional PFD set.
pub const HMD_DESCRIPTOR: PanelDescriptor = PanelDescriptor {
    id: "hmd-replay",
    title: "Head-worn flight instruments",
    required_layers: (1 << LayerId::Tapes.to_u8()) | (1 << LayerId::Annunciation.to_u8()),
    required_groups: GroupSet::of(&[
        GroupId::Air,
        GroupId::Kinematics,
        GroupId::Altitude,
        GroupId::Attitude,
        GroupId::Heading,
        GroupId::Trust,
        GroupId::Dynamics,
        GroupId::ApTargets,
        GroupId::Nav,
        GroupId::FlightDirector,
        GroupId::ApModes,
        GroupId::Guidance,
        GroupId::SpeedPresentation,
    ]),
    frame_min: FRAME,
    frame_max: FRAME,
    frame_step: (1.0, 1.0),
    aspect_min: 1.99,
    aspect_max: 2.01,
    canonical_frames: &[FRAME],
    background: BackgroundCapability::NotUsed,
    config_schema: &[],
    group_regions: &[],
    extreme_states: &[indicate_instrument_descriptor::ExtremeState {
        id: "extended-flight-guidance",
        build: indicate_instrument_state::abi::v8::fixtures::extended,
    }],
    raster_baselines: &[],
    draw,
};

/// The set a compositor places above its synthetic terrain imagery.
pub const HMD_SET: PanelSet = PanelSet {
    id: "hmd-replay",
    panels: &[HMD_DESCRIPTOR, HMD_GLANCE_DESCRIPTOR],
};

fn draw(
    data: &PanelData,
    config: &ConfigBlob<'_>,
    alerts: Option<&AlertOutput>,
    _frame: DesignFrame,
    scene: &mut SceneWriter<'_>,
) -> Result<(), PanelDrawError> {
    config.require_schema(&[])?;
    draw_hwd(data, crate::DisplayContext::default(), false, alerts, scene)
}

/// Draws the HWD with explicit display context and host-selected off-axis layout.
/// Hosts apply [`crate::layer_reference`] to each layer when projecting this scene.
/// The descriptor entry points use the primary flight role with normal detail.
pub fn draw_hwd(
    data: &PanelData,
    context: crate::DisplayContext,
    compact: bool,
    alerts: Option<&AlertOutput>,
    scene: &mut SceneWriter<'_>,
) -> Result<(), PanelDrawError> {
    let plan = crate::presentation_plan(data, context, alerts);
    let compact = compact || plan.recovery;
    scene.begin_layer(LayerId::Tapes)?;
    if !plan.blank_flight {
        if compact {
            glance::instruments(data, plan.detailed, scene)?;
        } else {
            scale::draw(data, plan.detailed, scene)?;
            vertical_speed::draw(data.vsi_fpm, scene)?;
        }
    }
    scene.end_layer(LayerId::Tapes)?;
    if !plan.blank_flight {
        guidance::draw(data, compact, !plan.mission, scene)?;
    }
    annunciation::draw(data, context, plan, alerts, scene)
}

pub(crate) fn live(signal: Sig<f32>) -> bool {
    signal.status == SignalStatus::Valid
        && signal.value.is_finite()
        && signal.value.abs() < 1_000_000.0
}

fn altitude_group(data: &PanelData) -> GroupId {
    if data.altitude.class == indicate_instrument_state::AltitudeClass::LocalRelative {
        GroupId::Kinematics
    } else {
        GroupId::Air
    }
}

#[cfg(test)]
mod tests;
