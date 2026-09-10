use indicate_alerts::AlertOutput;
use indicate_instrument_descriptor::{
    BackgroundCapability, ConfigBlob, DesignFrame, GroupSet, PanelDescriptor, PanelDrawError,
    PanelSet,
};
use indicate_instrument_scene::{Anchor, LayerId, PaintMode, Rgba8, SceneWriter};

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
    title: "Head-mounted replay",
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
pub const HMD_SET: PanelSet = PanelSet {
    id: "hmd-replay",
    panels: &[HMD_DESCRIPTOR],
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
    readout(scene, "IAS KT", data.ias_kt, GroupId::Air, [72.0, 250.0])?;
    readout(
        scene,
        "GS KT",
        data.gs_kt,
        GroupId::Kinematics,
        [72.0, 365.0],
    )?;
    let altitude_label = fmt_label!(
        16,
        "{} FT",
        if data.altitude.value_ft.status.shows_value() {
            data.altitude.class.label()
        } else {
            "ALT"
        }
    );
    readout(
        scene,
        altitude_label.as_str(),
        data.altitude.value_ft,
        if data.altitude.class == indicate_instrument_state::AltitudeClass::LocalRelative {
            GroupId::Kinematics
        } else {
            GroupId::Air
        },
        [928.0, 250.0],
    )?;
    readout(
        scene,
        "VS FPM",
        data.vsi_fpm,
        GroupId::Kinematics,
        [928.0, 140.0],
    )?;
    compass(data, scene)?;
    scene.end_layer(LayerId::Tapes)?;
    annunciations(data, scene)
}

fn annunciations(data: &PanelData, scene: &mut SceneWriter<'_>) -> Result<(), PanelDrawError> {
    scene.begin_layer(LayerId::Annunciation)?;
    let attitude = data.roll_rad.status.worst(data.pitch_rad.status);
    if !attitude.shows_value() {
        scene.fill_color(if attitude == SignalStatus::Failed {
            safety::FAILURE_RED
        } else {
            palette::AMBER
        })?;
        scene.text(
            600.0,
            532.0,
            13.0,
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
        scene.fill_color(if data.heading.value_rad.status == SignalStatus::Failed {
            safety::FAILURE_RED
        } else {
            palette::AMBER
        })?;
        scene.text(
            600.0,
            558.0,
            12.0,
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
    scene.stroke(Rgba8::rgba(0, 12, 0, 180), 4.0)?;
    scene.rect(PaintMode::Stroke, x, y + 22.0, 200.0, 48.0)?;
    scene.stroke(HUD_GREEN, 1.2)?;
    scene.rect(PaintMode::Stroke, x, y + 22.0, 200.0, 48.0)?;
    scene.fill_color(HUD_GREEN)?;
    scene.text(x, y + 8.0, 13.0, Anchor::MIDDLE_LEFT, label)?;
    let in_range = signal.value.is_finite() && signal.value.abs() < 1_000_000.0;
    if signal.status.shows_value() && in_range {
        scene.fill_color(if signal.status == SignalStatus::Valid {
            HUD_GREEN
        } else {
            palette::AMBER
        })?;
        let value = fmt_label!(16, "{:.0}", signal.value);
        scene.text_attributed(
            group.to_u8(),
            x + 186.0,
            y + 46.0,
            24.0,
            Anchor::MIDDLE_RIGHT,
            value.as_str(),
        )?;
    } else {
        scene.fill_color(if signal.status == SignalStatus::Failed {
            safety::FAILURE_RED
        } else {
            palette::AMBER
        })?;
        scene.text(x + 186.0, y + 46.0, 28.0, Anchor::MIDDLE_RIGHT, "---")?;
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
        scene.text(x + 186.0, y + 86.0, 13.0, Anchor::MIDDLE_RIGHT, status)?;
    }
    Ok(())
}

fn compass(data: &PanelData, scene: &mut SceneWriter<'_>) -> Result<(), PanelDrawError> {
    let track = degrees(data.track_rad);
    let heading = degrees(data.heading.value_rad);
    readout(scene, "TRK T", track, GroupId::Kinematics, [355.0, 18.0])?;
    let reference = match data.heading.reference {
        indicate_instrument_state::HeadingReference::Magnetic => "M",
        indicate_instrument_state::HeadingReference::SimLocalTrue => "SIM",
        _ => "T",
    };
    let label = fmt_label!(16, "HDG {}", reference);
    readout(
        scene,
        label.as_str(),
        heading,
        GroupId::Heading,
        [645.0, 18.0],
    )?;
    Ok(())
}

fn degrees(mut signal: Sig<f32>) -> Sig<f32> {
    signal.value = (signal.value.to_degrees() % 360.0 + 360.0) % 360.0;
    signal
}

#[cfg(test)]
mod tests;
