//! Region-family fixtures: a declared readout surface must be one the
//! group's readout really uses.
#![allow(clippy::expect_used, clippy::panic)]

use indicate_alerts::AlertOutput;
use indicate_instrument_registry::{
    BackgroundCapability, ConfigBlob, DesignFrame, GroupSet, PanelDescriptor, PanelDrawError,
    Region, Registry,
};
use indicate_instrument_scene::{Anchor, LayerId, Rgba8, SceneWriter};
use indicate_instrument_state::{GroupId, PanelData};

use super::super::{AdmissionError, admit};
use super::FIXTURE_FRAME;

/// The surface the fixture panel declares for `Air`.
const READOUT: Region = Region {
    x: 20.0,
    y: 20.0,
    width: 120.0,
    height: 40.0,
};

/// A second surface over blank canvas: nothing the panel draws claims
/// `Air` anywhere near it.
const BARREN: Region = Region {
    x: 300.0,
    y: 250.0,
    width: 120.0,
    height: 40.0,
};

/// Paints the `Air` readout inside [`READOUT`] and a scale ladder well
/// outside it — the shape every tape panel has, and the reason the
/// family cannot demand that all claimed ink sit inside a region.
fn draw_tape(
    data: &PanelData,
    _config: &ConfigBlob<'_>,
    _alerts: Option<&AlertOutput>,
    _frame: DesignFrame,
    scene: &mut SceneWriter<'_>,
) -> Result<(), PanelDrawError> {
    scene.begin_layer(LayerId::Tapes)?;
    scene.save()?;
    scene.fill_color(Rgba8::rgb(255, 255, 255))?;
    if data.ias_kt.status.shows_value() {
        let air = GroupId::Air.to_u8();
        scene.text_attributed(air, 30.0, 45.0, 14.0, Anchor::MIDDLE_LEFT, "120")?;
        for (index, rung) in ["100", "110", "130"].iter().enumerate() {
            let y = 120.0 + 30.0 * index as f32;
            scene.text_attributed(air, 200.0, y, 14.0, Anchor::MIDDLE_LEFT, rung)?;
        }
    } else {
        scene.text(30.0, 45.0, 14.0, Anchor::MIDDLE_LEFT, "---")?;
    }
    scene.restore()?;
    scene.end_layer(LayerId::Tapes)?;
    Ok(())
}

const fn probe(id: &'static str, group_regions: &'static [(GroupId, Region)]) -> PanelDescriptor {
    PanelDescriptor {
        id,
        title: "Probe",
        required_layers: 1 << 2, // Tapes
        required_groups: GroupSet::of(&[GroupId::Air]),
        frame_min: FIXTURE_FRAME,
        frame_max: FIXTURE_FRAME,
        frame_step: (1.0, 1.0),
        aspect_min: 1.30,
        aspect_max: 1.37,
        canonical_frames: &[FIXTURE_FRAME],
        background: BackgroundCapability::NotUsed,
        config_schema: &[],
        group_regions,
        extreme_states: &[],
        raster_baselines: &[],
        draw: draw_tape,
    }
}

/// The witness only has to exist somewhere in the matrix: the readout
/// dashes out under withholding and paints no claimed run at all in
/// those cases, and the region is still the surface it uses.
#[test]
fn a_populated_region_is_admitted() {
    static POPULATED: [PanelDescriptor; 1] = [probe("populated", &[(GroupId::Air, READOUT)])];
    let registry = Registry::new(&POPULATED).expect("structurally valid");
    admit(&registry).expect("the readout paints inside the region it declares");
}

#[test]
fn a_region_over_blank_space_is_refused() {
    static BARE: [PanelDescriptor; 1] = [probe("bare", &[(GroupId::Air, BARREN)])];
    let registry = Registry::new(&BARE).expect("structurally valid");
    let refusal = admit(&registry).expect_err("nothing claiming Air is drawn in that region");
    let AdmissionError::GroupRegionEmpty {
        panel,
        group,
        region,
        frame,
    } = refusal
    else {
        panic!("expected an empty region, got {refusal:?}");
    };
    assert_eq!(panel, "bare");
    assert_eq!(group, GroupId::Air);
    assert_eq!(region, BARREN);
    assert_eq!(frame, FIXTURE_FRAME);
}

/// One populated region does not vouch for another: every declared
/// surface answers for itself, or a panel could bury a fictional
/// region behind a real one.
#[test]
fn a_barren_region_beside_a_populated_one_is_still_refused() {
    static MIXED: [PanelDescriptor; 1] = [probe(
        "mixed",
        &[(GroupId::Air, READOUT), (GroupId::Air, BARREN)],
    )];
    let registry = Registry::new(&MIXED).expect("structurally valid");
    let refusal = admit(&registry).expect_err("the second region catches nothing");
    let AdmissionError::GroupRegionEmpty { region, .. } = refusal else {
        panic!("expected an empty region, got {refusal:?}");
    };
    assert_eq!(region, BARREN);
}

/// A sliver overlapping the underside of the first ladder rung's ink
/// and nothing else: claimed ink crosses it, but no claimed run is
/// drawn *at* it.
const GRAZE: Region = Region {
    x: 190.0,
    y: 126.0,
    width: 70.0,
    height: 16.0,
};

/// Overlap is not the test. A region clipped by the edge of a rung it
/// does not own would otherwise vouch for itself, which would make the
/// family satisfiable by any region drawn near enough to any ink.
#[test]
fn a_region_merely_grazed_by_claimed_ink_is_refused() {
    static GRAZED: [PanelDescriptor; 1] = [probe("grazed", &[(GroupId::Air, GRAZE)])];
    let registry = Registry::new(&GRAZED).expect("structurally valid");
    let refusal = admit(&registry).expect_err("no run is drawn at that surface");
    let AdmissionError::GroupRegionEmpty { region, .. } = refusal else {
        panic!("expected an empty region, got {refusal:?}");
    };
    assert_eq!(region, GRAZE);
}

/// Ink claiming the group outside every declared region is not a
/// defect: a scale ladder's rungs carry the group's claim because a
/// numeral must, and they sit outside the readout box on purpose.
#[test]
fn claimed_ink_outside_the_regions_is_not_a_defect() {
    static POPULATED: [PanelDescriptor; 1] = [probe("ladder", &[(GroupId::Air, READOUT)])];
    let registry = Registry::new(&POPULATED).expect("structurally valid");
    let report = admit(&registry).expect("the ladder rungs are honest ink outside the readout");
    assert!(report.cases > 0);
}
