//! An integrated off-axis instrument preserves aircraft attitude without implying outside-world alignment.
use super::*;

/// Head-referenced speed, attitude, altitude, and vertical speed for looking away from the aircraft nose.
pub const HMD_GLANCE_DESCRIPTOR: PanelDescriptor = PanelDescriptor {
    id: "hmd-glance",
    title: "Head-worn off-axis instruments",
    draw,
    ..HMD_DESCRIPTOR
};

pub(super) fn draw(
    data: &PanelData,
    config: &ConfigBlob<'_>,
    alerts: Option<&AlertOutput>,
    _frame: DesignFrame,
    scene: &mut SceneWriter<'_>,
) -> Result<(), PanelDrawError> {
    config.require_schema(&[])?;
    super::draw_hwd(data, crate::DisplayContext::default(), true, alerts, scene)
}

pub(super) fn instruments(
    data: &PanelData,
    detailed: bool,
    scene: &mut SceneWriter<'_>,
) -> Result<(), PanelDrawError> {
    use indicate_instrument_symbology::flight::speed;
    speed::readout(scene, data, [378.0, 405.0], 27.0, 12.0, 150.0, HUD_GREEN)?;
    speed::target_readouts(scene, data, [378.0, 322.0], 12.0, 180.0)?;
    speed::envelope_summary(scene, data, [378.0, 265.0], 12.0)?;
    if detailed {
        readout::value(
            scene,
            "GS KT",
            data.gs_kt,
            GroupId::Kinematics,
            [378.0, 491.0],
            17.0,
        )?;
    }
    let label = fmt_label!(16, "{} FT", data.altitude.class.label());
    readout::value(
        scene,
        label.as_str(),
        data.altitude.value_ft,
        altitude_group(data),
        [828.0, 405.0],
        27.0,
    )?;
    readout::value(
        scene,
        "VS FPM",
        data.vsi_fpm,
        GroupId::Kinematics,
        [828.0, 491.0],
        17.0,
    )?;
    readout::baro(data, [828.0, 550.0], 12.0, scene)?;
    attitude::draw(data, scene)?;
    if data.presentation.unusual {
        scene.fill_color(safety::CAUTION_AMBER)?;
        scene.text(600.0, 281.0, 17.0, Anchor::CENTER, "UNUSUAL ATTITUDE")?;
    }
    let label = if data.presentation.inverted {
        "INVERTED"
    } else if data.presentation.nose_high {
        "NOSE HIGH"
    } else if data.presentation.nose_low {
        "NOSE LOW"
    } else if data.presentation.high_bank {
        "HIGH BANK"
    } else {
        "AIRCRAFT ATTITUDE"
    };
    scene.fill_color(if data.presentation.unusual {
        safety::CAUTION_AMBER
    } else {
        HUD_GREEN
    })?;
    scene.text(600.0, 521.0, 12.0, Anchor::CENTER, label)?;
    Ok(())
}
