#![allow(clippy::expect_used, clippy::panic)]

use indicate_alerts::AlertOutput;
use indicate_instrument_scene::{LAYER_COUNT, SceneWriter};
use indicate_instrument_state::{AircraftState, GroupId, PanelData};

use std::vec::Vec;

use indicate_instrument_descriptor::{
    BackgroundCapability, ConfigBlob, DesignFrame, ExtremeState, GroupSet, PanelDescriptor,
    PanelDrawError, PanelSet, Region,
};

use super::{Registry, RegistryError};

fn draw_nothing(
    _data: &PanelData,
    _config: &ConfigBlob<'_>,
    _alerts: Option<&AlertOutput>,
    _frame: DesignFrame,
    _scene: &mut SceneWriter<'_>,
) -> Result<(), PanelDrawError> {
    Ok(())
}

fn nothing_fed() -> AircraftState {
    AircraftState::default()
}

pub(super) const FRAME: DesignFrame = DesignFrame {
    width: 480.0,
    height: 360.0,
};
const CANONICAL: &[DesignFrame] = &[FRAME];

pub(super) const fn panel(id: &'static str) -> PanelDescriptor {
    PanelDescriptor {
        id,
        title: "Panel",
        required_layers: 0b0000_0110,
        required_groups: GroupSet::of(&[GroupId::Attitude, GroupId::Air]),
        frame_min: FRAME,
        frame_max: FRAME,
        frame_step: (1.0, 1.0),
        aspect_min: 1.30,
        aspect_max: 1.37,
        canonical_frames: CANONICAL,
        background: BackgroundCapability::NotUsed,
        config_schema: &[],
        group_regions: &[],
        extreme_states: &[],
        raster_baselines: &[],
        draw: draw_nothing,
    }
}

/// A panel with a real range: 480×360 up to 600×450, both 4:3, on a
/// 40×30 grid. The fixtures below need a range wide enough that a
/// constraint can be violated inside it.
pub(super) const RANGED_MAX: DesignFrame = DesignFrame {
    width: 600.0,
    height: 450.0,
};

pub(super) const fn ranged(id: &'static str) -> PanelDescriptor {
    let mut p = panel(id);
    p.frame_max = RANGED_MAX;
    p.frame_step = (40.0, 30.0);
    p.canonical_frames = &[FRAME, RANGED_MAX];
    p
}

#[test]
fn a_valid_composition_is_accepted_and_queryable() {
    static PANELS: [PanelDescriptor; 2] = [panel("alpha"), panel("beta-2")];
    let registry = Registry::new(&PANELS).expect("two well-formed panels");
    assert_eq!(registry.panels().count(), 2);
    assert_eq!(registry.by_id("beta-2").expect("registered").id, "beta-2");
    assert!(registry.by_id("gamma").is_none());
}

#[test]
fn an_empty_composition_is_refused() {
    assert_eq!(Registry::new(&[]).map(|_| ()), Err(RegistryError::Empty));
}

#[test]
fn malformed_and_duplicate_ids_are_refused() {
    static UPPER: [PanelDescriptor; 1] = [panel("PFD")];
    assert_eq!(
        Registry::new(&UPPER).map(|_| ()),
        Err(RegistryError::BadId { index: 0 })
    );
    static DUP: [PanelDescriptor; 2] = [panel("pfd"), panel("pfd")];
    assert_eq!(
        Registry::new(&DUP).map(|_| ()),
        Err(RegistryError::DuplicateId { index: 1 })
    );
}

#[test]
fn layer_mask_abuse_is_refused() {
    static NONE: [PanelDescriptor; 1] = [{
        let mut p = panel("pfd");
        p.required_layers = 0;
        p
    }];
    assert_eq!(
        Registry::new(&NONE).map(|_| ()),
        Err(RegistryError::NoRequiredLayers { index: 0 })
    );
    static BEYOND: [PanelDescriptor; 1] = [{
        let mut p = panel("pfd");
        p.required_layers = 1 << LAYER_COUNT;
        p
    }];
    assert_eq!(
        Registry::new(&BEYOND).map(|_| ()),
        Err(RegistryError::UndefinedLayerBits {
            index: 0,
            bits: 1 << LAYER_COUNT,
        })
    );
}

#[test]
fn schema_key_order_is_enforced() {
    use indicate_instrument_descriptor::ConfigKey;
    static UNSORTED: [PanelDescriptor; 1] = [{
        let mut p = panel("pfd");
        p.config_schema = &[ConfigKey(2), ConfigKey(1)];
        p
    }];
    assert_eq!(
        Registry::new(&UNSORTED).map(|_| ()),
        Err(RegistryError::SchemaKeysNotAscending { index: 0, key: 1 })
    );
}

#[test]
fn group_regions_must_stay_honest() {
    static FOREIGN: [PanelDescriptor; 1] = [{
        let mut p = panel("pfd");
        p.group_regions = &[(
            GroupId::Nav,
            Region {
                x: 0.0,
                y: 0.0,
                width: 10.0,
                height: 10.0,
            },
        )];
        p
    }];
    assert_eq!(
        Registry::new(&FOREIGN).map(|_| ()),
        Err(RegistryError::RegionGroupNotRequired {
            index: 0,
            group: GroupId::Nav as u8,
        })
    );
    static OUTSIDE: [PanelDescriptor; 1] = [{
        let mut p = panel("pfd");
        p.group_regions = &[(
            GroupId::Attitude,
            Region {
                x: 470.0,
                y: 0.0,
                width: 20.0,
                height: 10.0,
            },
        )];
        p
    }];
    assert_eq!(
        Registry::new(&OUTSIDE).map(|_| ()),
        Err(RegistryError::RegionOutsideFrame {
            index: 0,
            group: GroupId::Attitude as u8,
        })
    );
    // The floor is the frame a region must fit in: this one fits the
    // ceiling comfortably and hangs off the bottom of the minimum.
    static ONLY_FITS_THE_CEILING: [PanelDescriptor; 1] = [{
        let mut p = ranged("pfd");
        p.group_regions = &[(
            GroupId::Attitude,
            Region {
                x: 0.0,
                y: 0.0,
                width: 480.0,
                height: 400.0,
            },
        )];
        p
    }];
    assert_eq!(
        Registry::new(&ONLY_FITS_THE_CEILING).map(|_| ()),
        Err(RegistryError::RegionOutsideFrame {
            index: 0,
            group: GroupId::Attitude as u8,
        })
    );
}

#[test]
fn extreme_state_ids_must_be_unique_and_well_formed() {
    static DUP: [PanelDescriptor; 1] = [{
        let mut p = panel("pfd");
        p.extreme_states = &[
            ExtremeState {
                id: "unusual-nose-high",
                build: nothing_fed,
            },
            ExtremeState {
                id: "unusual-nose-high",
                build: nothing_fed,
            },
        ];
        p
    }];
    assert_eq!(
        Registry::new(&DUP).map(|_| ()),
        Err(RegistryError::DuplicateExtremeId {
            index: 0,
            position: 1,
        })
    );
}

// --- Composition across provider crates (#6) ---

static ALPHA: PanelSet = PanelSet {
    id: "alpha-set",
    panels: &[panel("alpha")],
};
static BETA: PanelSet = PanelSet {
    id: "beta-set",
    panels: &[panel("beta")],
};

#[test]
fn sets_compose_in_shell_order_and_stay_queryable() {
    static SETS: [&PanelSet; 2] = [&ALPHA, &BETA];
    let registry = Registry::from_sets(&SETS).expect("two well-formed sets");
    let ids: Vec<&str> = registry.panels().map(|panel| panel.id).collect();
    assert_eq!(ids, ["alpha", "beta"], "set order is composition order");
    assert_eq!(registry.by_id("beta").expect("registered").id, "beta");
    assert_eq!(registry.sets().len(), 2);
}

/// The property this mechanism exists for: a shell that composes two
/// sets claiming the same panel gets a refusal at init, not whichever
/// panel happened to be listed first.
#[test]
fn a_panel_id_claimed_by_two_sets_is_refused() {
    static CLASH: PanelSet = PanelSet {
        id: "clash-set",
        panels: &[panel("alpha")],
    };
    static SETS: [&PanelSet; 2] = [&ALPHA, &CLASH];
    assert_eq!(
        Registry::from_sets(&SETS).map(|_| ()),
        Err(RegistryError::DuplicateId { index: 1 }),
    );
}

#[test]
fn per_panel_rules_still_run_over_a_composition_of_sets() {
    static UPPER: PanelSet = PanelSet {
        id: "upper-set",
        panels: &[panel("Alpha")],
    };
    static SETS: [&PanelSet; 2] = [&ALPHA, &UPPER];
    assert_eq!(
        Registry::from_sets(&SETS).map(|_| ()),
        Err(RegistryError::BadId { index: 1 }),
        "a malformed id must not ride in unchecked because it arrived in a set",
    );
}

#[test]
fn two_sets_sharing_an_id_are_refused() {
    static TWIN: PanelSet = PanelSet {
        id: "alpha-set",
        panels: &[panel("other")],
    };
    static SETS: [&PanelSet; 2] = [&ALPHA, &TWIN];
    assert_eq!(
        Registry::from_sets(&SETS).map(|_| ()),
        Err(RegistryError::DuplicateSetId { set: 1 }),
    );
}

#[test]
fn a_set_contributing_no_panels_is_refused() {
    static HOLLOW: PanelSet = PanelSet {
        id: "hollow-set",
        panels: &[],
    };
    static SETS: [&PanelSet; 2] = [&ALPHA, &HOLLOW];
    assert_eq!(
        Registry::from_sets(&SETS).map(|_| ()),
        Err(RegistryError::EmptySet { set: 1 }),
    );
}

#[test]
fn a_malformed_set_id_is_refused() {
    static SHOUTY: PanelSet = PanelSet {
        id: "Alpha_Set",
        panels: &[panel("gamma")],
    };
    static SETS: [&PanelSet; 1] = [&SHOUTY];
    assert_eq!(
        Registry::from_sets(&SETS).map(|_| ()),
        Err(RegistryError::BadSetId { set: 0 }),
    );
}

#[test]
fn a_composition_naming_no_sets_is_refused() {
    assert_eq!(
        Registry::from_sets(&[]).map(|_| ()),
        Err(RegistryError::NoSets),
    );
}

/// A bare slice still composes: a single-provider shell never learns
/// that sets exist.
#[test]
fn an_anonymous_composition_names_no_sets() {
    static PANELS: [PanelDescriptor; 1] = [panel("alpha")];
    let registry = Registry::new(&PANELS).expect("composes");
    assert!(registry.sets().is_empty());
    assert_eq!(registry.panels().count(), 1);
}
