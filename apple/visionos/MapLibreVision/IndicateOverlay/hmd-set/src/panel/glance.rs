//! An integrated off-axis instrument preserves aircraft attitude without implying outside-world alignment.
use super::*;

/// Head-referenced speed, attitude, altitude, and vertical speed for looking away from the aircraft nose.
pub const HMD_GLANCE_DESCRIPTOR: PanelDescriptor = PanelDescriptor {
    id: "hmd-glance",
    title: "Head-worn off-axis instruments",
    draw,
    ..HMD_DESCRIPTOR
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
    readout::value(
        scene,
        "IAS KT",
        data.ias_kt,
        GroupId::Air,
        [378.0, 405.0],
        27.0,
    )?;
    readout::value(
        scene,
        "GS KT",
        data.gs_kt,
        GroupId::Kinematics,
        [378.0, 491.0],
        17.0,
    )?;
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
    attitude::draw(data, scene)?;
    scene.fill_color(HUD_GREEN)?;
    scene.text(600.0, 521.0, 12.0, Anchor::CENTER, "AIRCRAFT ATTITUDE")?;
    heading::draw(data, scene)?;
    scene.end_layer(LayerId::Tapes)?;
    annunciation::draw(data, scene)
}
