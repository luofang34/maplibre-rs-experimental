use indicate_alerts::AlertOutput;
use indicate_instrument_descriptor::{
    BackgroundCapability, ConfigBlob, DesignFrame, GroupSet, PanelDescriptor, PanelDrawError,
    PanelSet,
};
use indicate_instrument_scene::{Anchor, LayerId, PaintMode, Rgba8, SceneWriter};

mod annunciation;
mod attitude;
mod glance;
mod heading;
mod readout;
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
    extreme_states: &[],
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
    _alerts: Option<&AlertOutput>,
    _frame: DesignFrame,
    scene: &mut SceneWriter<'_>,
) -> Result<(), PanelDrawError> {
    config.require_schema(&[])?;
    scene.begin_layer(LayerId::Tapes)?;
    scale::draw(data, scene)?;
    vertical_speed::draw(data.vsi_fpm, scene)?;
    heading::draw(data, scene)?;
    scene.end_layer(LayerId::Tapes)?;
    annunciation::draw(data, scene)
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
