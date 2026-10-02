#![allow(clippy::expect_used, clippy::panic)]

use indicate_alerts::AlertOutput;
use indicate_instrument_descriptor::{
    BackgroundCapability, ConfigBlob, CriticalityBands, DesignFrame, GroupSet, PanelCriticality,
    PanelDescriptor, PanelDrawError, Region,
};
use indicate_instrument_scene::SceneWriter;
use indicate_instrument_state::{GroupId, PanelData};

use super::{
    CompositionDescriptor, CompositionError, MAX_COMPOSITION_SLOTS, Slot, validate_composition,
};
use crate::registry::Registry;

pub(super) const FRAME: DesignFrame = DesignFrame {
    width: 480.0,
    height: 360.0,
};
pub(super) const SCREEN: DesignFrame = DesignFrame {
    width: 960.0,
    height: 720.0,
};

fn draw_nothing(
    _data: &PanelData,
    _config: &ConfigBlob<'_>,
    _alerts: Option<&AlertOutput>,
    _frame: DesignFrame,
    _scene: &mut SceneWriter<'_>,
) -> Result<(), PanelDrawError> {
    Ok(())
}

const fn base(id: &'static str, background: BackgroundCapability) -> PanelDescriptor {
    PanelDescriptor {
        id,
        title: "Panel",
        required_layers: 0b0000_0110,
        required_groups: GroupSet::of(&[GroupId::Air]),
        frame_min: FRAME,
        frame_max: FRAME,
        frame_step: (1.0, 1.0),
        aspect_min: 1.30,
        aspect_max: 1.37,
        canonical_frames: &[FRAME],
        background,
        config_schema: &[],
        group_regions: &[],
        extreme_states: &[],
        raster_baselines: &[],
        draw: draw_nothing,
    }
}

/// A readout surface in the top-left corner of the panel's own frame,
/// so a slot placed over that corner covers ordinary symbology.
const READOUT: &[(GroupId, Region)] = &[(
    GroupId::Air,
    Region {
        x: 10.0,
        y: 10.0,
        width: 100.0,
        height: 40.0,
    },
)];

/// A small 2:1 badge: an overlay sized to sit on part of a panel, so
/// the occlusion fixtures place something that is not a whole frame.
pub(super) const BADGE_FRAME: DesignFrame = DesignFrame {
    width: 200.0,
    height: 100.0,
};

pub(super) const PANELS: &[PanelDescriptor] = &[
    {
        let mut panel = base("low", BackgroundCapability::Opaque);
        panel.group_regions = READOUT;
        panel
    },
    base("high", BackgroundCapability::Opaque),
    base("overlay", BackgroundCapability::NotUsed),
    {
        let mut panel = base("badge", BackgroundCapability::NotUsed);
        panel.frame_min = BADGE_FRAME;
        panel.frame_max = BADGE_FRAME;
        panel.canonical_frames = &[BADGE_FRAME];
        panel.aspect_min = 1.9;
        panel.aspect_max = 2.1;
        panel
    },
];

pub(super) fn registry() -> Registry {
    Registry::new(PANELS).expect("fixture composes")
}

/// Bands measured for every fixture panel at the one frame they declare:
/// `low` warns in a band left of centre, the others warn nowhere.
pub(super) const BANDS: CriticalityBands = CriticalityBands {
    panels: &[
        PanelCriticality {
            panel: "low",
            frame: FRAME,
            band: Some(Region {
                x: 200.0,
                y: 150.0,
                width: 80.0,
                height: 60.0,
            }),
        },
        PanelCriticality {
            panel: "high",
            frame: FRAME,
            band: None,
        },
        PanelCriticality {
            panel: "overlay",
            frame: FRAME,
            band: None,
        },
        PanelCriticality {
            panel: "badge",
            frame: BADGE_FRAME,
            band: None,
        },
    ],
};

/// Bands no case ever witnessed ink in. Since an empty measurement is
/// the absence of a witness rather than a proof of absence, a
/// composition validated against these refuses every overlap — which is
/// what [`an_unwitnessed_band_refuses_obscuration`] is for.
pub(super) const UNWITNESSED_BANDS: CriticalityBands = CriticalityBands {
    panels: &[
        PanelCriticality {
            panel: "low",
            frame: FRAME,
            band: None,
        },
        PanelCriticality {
            panel: "high",
            frame: FRAME,
            band: None,
        },
        PanelCriticality {
            panel: "overlay",
            frame: FRAME,
            band: None,
        },
        PanelCriticality {
            panel: "badge",
            frame: BADGE_FRAME,
            band: None,
        },
    ],
};

pub(super) const fn rect(x: f32, y: f32, width: f32, height: f32) -> Region {
    Region {
        x,
        y,
        width,
        height,
    }
}

pub(super) const fn slot(panel: &'static str, rect: Region) -> Slot {
    Slot {
        panel,
        rect,
        occludes: &[],
    }
}

pub(super) const SIDE_BY_SIDE: CompositionDescriptor = CompositionDescriptor {
    screen: SCREEN,
    slots: &[
        slot("low", rect(0.0, 0.0, 480.0, 360.0)),
        slot("high", rect(480.0, 0.0, 480.0, 360.0)),
    ],
};

#[test]
fn side_by_side_validates() {
    validate_composition(&registry(), &SIDE_BY_SIDE, &BANDS).expect("no slot touches another");
}

#[test]
fn empty_composition_is_refused() {
    const COMPOSITION: CompositionDescriptor = CompositionDescriptor {
        screen: SCREEN,
        slots: &[],
    };
    assert_eq!(
        validate_composition(&registry(), &COMPOSITION, &BANDS),
        Err(CompositionError::NoSlots)
    );
}

#[test]
fn slot_count_over_the_ceiling_is_refused() {
    const NINE: &[Slot] = &[
        slot("low", rect(0.0, 0.0, 480.0, 360.0)),
        slot("high", rect(0.0, 360.0, 480.0, 360.0)),
        slot("high", rect(480.0, 0.0, 480.0, 360.0)),
        slot("high", rect(480.0, 360.0, 480.0, 360.0)),
        slot("high", rect(0.0, 0.0, 480.0, 360.0)),
        slot("high", rect(0.0, 0.0, 480.0, 360.0)),
        slot("high", rect(0.0, 0.0, 480.0, 360.0)),
        slot("high", rect(0.0, 0.0, 480.0, 360.0)),
        slot("high", rect(0.0, 0.0, 480.0, 360.0)),
    ];
    const COMPOSITION: CompositionDescriptor = CompositionDescriptor {
        screen: SCREEN,
        slots: NINE,
    };
    assert_eq!(
        validate_composition(&registry(), &COMPOSITION, &BANDS),
        Err(CompositionError::TooManySlots {
            slots: 9,
            ceiling: MAX_COMPOSITION_SLOTS,
        })
    );
}

#[test]
fn a_screen_that_is_not_a_frame_is_refused() {
    const BAD: DesignFrame = DesignFrame {
        width: 0.0,
        height: 720.0,
    };
    const COMPOSITION: CompositionDescriptor = CompositionDescriptor {
        screen: BAD,
        slots: &[slot("low", rect(0.0, 0.0, 480.0, 360.0))],
    };
    assert_eq!(
        validate_composition(&registry(), &COMPOSITION, &BANDS),
        Err(CompositionError::BadScreen { screen: BAD })
    );
}

#[test]
fn an_unregistered_panel_is_refused() {
    const COMPOSITION: CompositionDescriptor = CompositionDescriptor {
        screen: SCREEN,
        slots: &[slot("nowhere", rect(0.0, 0.0, 480.0, 360.0))],
    };
    assert_eq!(
        validate_composition(&registry(), &COMPOSITION, &BANDS),
        Err(CompositionError::UnknownPanel {
            slot: 0,
            panel: "nowhere",
        })
    );
}

#[test]
fn a_degenerate_slot_rect_is_refused() {
    const FLAT: Region = rect(0.0, 0.0, 480.0, 0.0);
    const COMPOSITION: CompositionDescriptor = CompositionDescriptor {
        screen: SCREEN,
        slots: &[slot("low", FLAT)],
    };
    assert_eq!(
        validate_composition(&registry(), &COMPOSITION, &BANDS),
        Err(CompositionError::SlotRectDegenerate {
            slot: 0,
            rect: FLAT
        })
    );
}

#[test]
fn a_slot_off_the_screen_is_refused() {
    const OFF: Region = rect(600.0, 400.0, 480.0, 360.0);
    const COMPOSITION: CompositionDescriptor = CompositionDescriptor {
        screen: SCREEN,
        slots: &[slot("low", OFF)],
    };
    assert_eq!(
        validate_composition(&registry(), &COMPOSITION, &BANDS),
        Err(CompositionError::SlotOutsideScreen {
            slot: 0,
            rect: OFF,
            screen: SCREEN,
        })
    );
}

#[test]
fn a_slot_sized_off_the_declared_frame_is_refused() {
    const COMPOSITION: CompositionDescriptor = CompositionDescriptor {
        screen: SCREEN,
        slots: &[slot("low", rect(0.0, 0.0, 400.0, 300.0))],
    };
    assert_eq!(
        validate_composition(&registry(), &COMPOSITION, &BANDS),
        Err(CompositionError::SlotFrameUnsupported {
            slot: 0,
            panel: "low",
            frame: DesignFrame {
                width: 400.0,
                height: 300.0,
            },
        })
    );
}

#[test]
fn a_panel_with_no_measured_band_is_refused() {
    const COMPOSITION: CompositionDescriptor = CompositionDescriptor {
        screen: SCREEN,
        slots: &[slot("low", rect(0.0, 0.0, 480.0, 360.0))],
    };
    assert_eq!(
        validate_composition(&registry(), &COMPOSITION, &CriticalityBands::EMPTY),
        Err(CompositionError::CriticalityUnknown {
            slot: 0,
            panel: "low",
            frame: FRAME,
        })
    );
}

#[test]
fn a_slot_buried_under_opaque_slots_is_refused() {
    const COMPOSITION: CompositionDescriptor = CompositionDescriptor {
        screen: SCREEN,
        slots: &[
            slot("low", rect(0.0, 0.0, 480.0, 360.0)),
            slot("high", rect(0.0, 0.0, 480.0, 360.0)),
        ],
    };
    assert_eq!(
        validate_composition(&registry(), &COMPOSITION, &UNWITNESSED_BANDS),
        Err(CompositionError::DeadSlot { slot: 0 })
    );
}

/// The dead-slot rule counts opaque covers only, and this is the pair
/// that proves it: byte-identical geometry to the case above, differing
/// only in the covering panel's `background`. An `Opaque` cover buries
/// the slot and is refused as `DeadSlot`; a `NotUsed` cover does not
/// bury it, so rule 5 stays silent and the refusal comes from the
/// criticality floor instead — full-rect coverage of a panel that paints
/// warnings is never admissible, whoever declares it.
#[test]
fn an_overlay_does_not_bury_the_slot_beneath_it() {
    const COMPOSITION: CompositionDescriptor = CompositionDescriptor {
        screen: SCREEN,
        slots: &[
            slot("low", rect(0.0, 0.0, 480.0, 360.0)),
            Slot {
                panel: "overlay",
                rect: rect(0.0, 0.0, 480.0, 360.0),
                occludes: &["low"],
            },
        ],
    };
    let refusal = validate_composition(&registry(), &COMPOSITION, &BANDS)
        .expect_err("the overlay covers the warning band beneath it");
    assert!(
        matches!(refusal, CompositionError::CriticalityObscured { .. }),
        "an overlay must not read as a dead-slot cover, got {refusal:?}"
    );
}

#[test]
fn covering_a_readout_without_declaring_it_is_refused() {
    const COMPOSITION: CompositionDescriptor = CompositionDescriptor {
        screen: SCREEN,
        slots: &[
            slot("low", rect(0.0, 0.0, 480.0, 360.0)),
            slot("badge", rect(0.0, 0.0, 200.0, 100.0)),
        ],
    };
    assert_eq!(
        validate_composition(&registry(), &COMPOSITION, &BANDS),
        Err(CompositionError::UndeclaredOcclusion {
            upper: 1,
            lower: 0,
            panel: "low",
            region: rect(10.0, 10.0, 100.0, 40.0),
        })
    );
}

#[test]
fn declaring_the_occlusion_permits_covering_a_readout() {
    const COMPOSITION: CompositionDescriptor = CompositionDescriptor {
        screen: SCREEN,
        slots: &[
            slot("low", rect(0.0, 0.0, 480.0, 360.0)),
            Slot {
                panel: "badge",
                rect: rect(0.0, 0.0, 200.0, 100.0),
                occludes: &["low"],
            },
        ],
    };
    validate_composition(&registry(), &COMPOSITION, &BANDS)
        .expect("a declared obscuration may cover ordinary symbology");
}

/// An empty measurement is the absence of a witness, not a proof of
/// absence. A panel whose band no case ever drew ink in refuses
/// obscuration outright — the alternative would make an unexercised
/// failure cue the easiest thing on a screen to cover.
#[test]
fn an_unwitnessed_band_refuses_obscuration() {
    const COMPOSITION: CompositionDescriptor = CompositionDescriptor {
        screen: SCREEN,
        slots: &[
            slot("low", rect(0.0, 0.0, 480.0, 360.0)),
            Slot {
                panel: "badge",
                rect: rect(0.0, 0.0, 200.0, 100.0),
                occludes: &["low"],
            },
        ],
    };
    assert_eq!(
        validate_composition(&registry(), &COMPOSITION, &UNWITNESSED_BANDS),
        Err(CompositionError::CriticalityUnwitnessed {
            upper: 1,
            lower: 0,
            panel: "low",
        })
    );
}

#[test]
fn declaring_the_occlusion_does_not_permit_covering_a_warning() {
    // The same declaration as above, against a panel whose measured
    // Annunciation band lies under the covering slot: AIR-OUT-011's
    // floor, which no `occludes` entry reaches.
    const COMPOSITION: CompositionDescriptor = CompositionDescriptor {
        screen: SCREEN,
        slots: &[
            slot("low", rect(0.0, 0.0, 480.0, 360.0)),
            Slot {
                panel: "badge",
                rect: rect(180.0, 120.0, 200.0, 100.0),
                occludes: &["low"],
            },
        ],
    };
    assert_eq!(
        validate_composition(&registry(), &COMPOSITION, &BANDS),
        Err(CompositionError::CriticalityObscured {
            upper: 1,
            lower: 0,
            panel: "low",
            band: rect(200.0, 150.0, 80.0, 60.0),
        })
    );
}

#[test]
fn the_band_is_placed_at_the_slot_origin() {
    // The lower slot sits at (480, 360), so its warning band moves with
    // it: an overlay over the *screen* origin covers nothing.
    const COMPOSITION: CompositionDescriptor = CompositionDescriptor {
        screen: SCREEN,
        slots: &[
            slot("low", rect(480.0, 360.0, 480.0, 360.0)),
            Slot {
                panel: "badge",
                rect: rect(480.0, 360.0, 200.0, 100.0),
                occludes: &["low"],
            },
        ],
    };
    validate_composition(&registry(), &COMPOSITION, &BANDS)
        .expect("the band translated with its slot");
}
