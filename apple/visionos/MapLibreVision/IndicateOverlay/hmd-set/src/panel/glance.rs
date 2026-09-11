//! A compact off-boresight readout keeps the outside view clear.
use super::*;

/// Minimal head-referenced readouts for looking away from the virtual instrument panel.
pub const HMD_GLANCE_DESCRIPTOR: PanelDescriptor = PanelDescriptor {
    id: "hmd-glance",
    title: "Head-mounted glance",
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
    let (speed, label, group) = if data.ias_kt.status.shows_value() {
        (data.ias_kt, "IAS KT", GroupId::Air)
    } else {
        (data.gs_kt, "GS KT", GroupId::Kinematics)
    };
    readout(scene, label, speed, group, [270.0, 350.0])?;
    let altitude = fmt_label!(16, "{} FT", data.altitude.class.label());
    readout(
        scene,
        altitude.as_str(),
        data.altitude.value_ft,
        if data.altitude.class == indicate_instrument_state::AltitudeClass::LocalRelative {
            GroupId::Kinematics
        } else {
            GroupId::Air
        },
        [730.0, 350.0],
    )?;
    readout(
        scene,
        "VS FPM",
        data.vsi_fpm,
        GroupId::Kinematics,
        [500.0, 430.0],
    )?;
    super::attitude::draw(data, scene)?;
    scene.end_layer(LayerId::Tapes)?;
    annunciations(data, scene)
}
