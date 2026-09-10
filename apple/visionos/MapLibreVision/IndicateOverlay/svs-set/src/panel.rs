use indicate_alerts::AlertOutput;
use indicate_instrument_descriptor::{
    BackgroundCapability, ConfigBlob, DesignFrame, GroupSet, PanelDescriptor, PanelDrawError,
    PanelSet,
};
use indicate_instrument_scene::{Anchor, LayerId, PaintMode, SceneWriter};
use indicate_instrument_state::{GroupId, PanelData, Sig, SignalStatus};
use indicate_instrument_symbology::{fmt_label, palette, safety};

const FRAME: DesignFrame = DesignFrame {
    width: 1200.0,
    height: 600.0,
};

/// A transparent instrument overlay, independent of the conventional PFD set.
pub const SVS_DESCRIPTOR: PanelDescriptor = PanelDescriptor {
    id: "svs-replay",
    title: "SVS replay",
    required_layers: (1 << LayerId::Tapes.to_u8()) | (1 << LayerId::Annunciation.to_u8()),
    required_groups: GroupSet::of(&[
        GroupId::Air,
        GroupId::Kinematics,
        GroupId::Altitude,
        GroupId::Attitude,
        GroupId::Heading,
        GroupId::Trust,
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
pub const SVS_SET: PanelSet = PanelSet {
    id: "svs-replay",
    panels: &[SVS_DESCRIPTOR],
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
    readout(scene, "IAS KT", data.ias_kt, GroupId::Air, [32.0, 225.0])?;
    readout(
        scene,
        "GS KT",
        data.gs_kt,
        GroupId::Kinematics,
        [32.0, 350.0],
    )?;
    let altitude_label = fmt_label!(16, "{} FT", data.altitude.class.label());
    readout(
        scene,
        altitude_label.as_str(),
        data.altitude.value_ft,
        if data.altitude.class == indicate_instrument_state::AltitudeClass::LocalRelative {
            GroupId::Kinematics
        } else {
            GroupId::Air
        },
        [944.0, 225.0],
    )?;
    readout(
        scene,
        "VS FPM",
        data.vsi_fpm,
        GroupId::Kinematics,
        [944.0, 350.0],
    )?;
    let track = Sig::with_status(data.track_rad.value.to_degrees(), data.track_rad.status);
    readout(
        scene,
        match data.heading.reference {
            indicate_instrument_state::HeadingReference::Magnetic => "TRK M",
            indicate_instrument_state::HeadingReference::SimLocalTrue => "TRK SIM",
            _ => "TRK T",
        },
        track,
        GroupId::Kinematics,
        [488.0, 40.0],
    )?;
    scene.end_layer(LayerId::Tapes)?;
    annunciations(data, scene)
}

fn annunciations(data: &PanelData, scene: &mut SceneWriter<'_>) -> Result<(), PanelDrawError> {
    scene.begin_layer(LayerId::Annunciation)?;
    let attitude = data.roll_rad.status.worst(data.pitch_rad.status);
    if !attitude.shows_value() {
        scene.fill_color(safety::FAILURE_RED)?;
        scene.text(
            600.0,
            532.0,
            17.0,
            Anchor::CENTER,
            if attitude == SignalStatus::Failed {
                "ATT FAILED"
            } else {
                "ATT MISSING"
            },
        )?;
    } else if attitude != SignalStatus::Valid {
        scene.fill_color(palette::AMBER)?;
        scene.text(600.0, 532.0, 17.0, Anchor::CENTER, "ATT CHECK")?;
    }
    if !data.heading.value_rad.status.shows_value() {
        scene.fill_color(safety::FAILURE_RED)?;
        scene.text(
            600.0,
            558.0,
            15.0,
            Anchor::CENTER,
            if data.heading.value_rad.status == SignalStatus::Failed {
                "HDG FAILED - TRACK VIEW"
            } else {
                "HDG MISSING - TRACK VIEW"
            },
        )?;
    }
    scene.end_layer(LayerId::Annunciation)?;
    Ok(())
}

fn readout(
    scene: &mut SceneWriter<'_>,
    label: &str,
    signal: Sig<f32>,
    group: GroupId,
    position: [f32; 2],
) -> Result<(), PanelDrawError> {
    let [x, y] = position;
    scene.fill_color(indicate_instrument_scene::Rgba8::rgba(8, 16, 24, 210))?;
    scene.rect(PaintMode::Fill, x, y, 224.0, 106.0)?;
    scene.fill_color(palette::WHITE)?;
    scene.text(x + 14.0, y + 19.0, 15.0, Anchor::MIDDLE_LEFT, label)?;
    let in_range = signal.value.is_finite() && signal.value.abs() < 1_000_000.0;
    if signal.status.shows_value() && in_range {
        scene.fill_color(if signal.status == SignalStatus::Valid {
            palette::WHITE
        } else {
            palette::AMBER
        })?;
        let value = fmt_label!(16, "{:.0}", signal.value);
        scene.text_attributed(
            group.to_u8(),
            x + 210.0,
            y + 57.0,
            28.0,
            Anchor::MIDDLE_RIGHT,
            value.as_str(),
        )?;
    } else {
        scene.fill_color(safety::FAILURE_RED)?;
        scene.text(x + 210.0, y + 57.0, 28.0, Anchor::MIDDLE_RIGHT, "---")?;
    }
    if signal.status != SignalStatus::Valid || !in_range {
        let status = if signal.status.shows_value() && !in_range {
            "RANGE"
        } else {
            match signal.status {
                SignalStatus::Degraded => "CHECK",
                SignalStatus::Stale => "STALE",
                SignalStatus::Failed => "FAILED",
                _ => "MISSING",
            }
        };
        scene.text(x + 210.0, y + 88.0, 13.0, Anchor::MIDDLE_RIGHT, status)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests;
