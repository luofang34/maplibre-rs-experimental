#![allow(clippy::expect_used, clippy::panic)]

use indicate_alerts::AlertOutput;
use indicate_instrument_scene::{MAX_SCENE_BYTES, SceneWriter};
use indicate_instrument_state::PanelData;

use indicate_instrument_descriptor::{
    BackgroundCapability, ConfigBlob, DesignFrame, GroupSet, PanelDescriptor, PanelDrawError,
};

use super::{DigestError, scene_digest};
use crate::registry::Registry;

fn draw_nothing(
    _data: &PanelData,
    _config: &ConfigBlob<'_>,
    _alerts: Option<&AlertOutput>,
    _frame: DesignFrame,
    _scene: &mut SceneWriter<'_>,
) -> Result<(), PanelDrawError> {
    Ok(())
}

const FRAME: DesignFrame = DesignFrame {
    width: 480.0,
    height: 360.0,
};

const fn panel(id: &'static str) -> PanelDescriptor {
    PanelDescriptor {
        id,
        title: "Panel",
        required_layers: 0b10,
        required_groups: GroupSet::EMPTY,
        frame_min: FRAME,
        frame_max: FRAME,
        frame_step: (1.0, 1.0),
        aspect_min: 1.30,
        aspect_max: 1.37,
        canonical_frames: &[FRAME],
        background: BackgroundCapability::NotUsed,
        config_schema: &[],
        group_regions: &[],
        extreme_states: &[],
        raster_baselines: &[],
        draw: draw_nothing,
    }
}

#[test]
fn the_digest_binds_panel_identity_and_composition() {
    static ONE: [PanelDescriptor; 1] = [panel("alpha")];
    static RENAMED: [PanelDescriptor; 1] = [panel("beta")];
    static TWO: [PanelDescriptor; 2] = [panel("alpha"), panel("beta")];
    let mut scratch = std::vec![0u8; MAX_SCENE_BYTES];
    let digest = |panels: &'static [PanelDescriptor], scratch: &mut [u8]| {
        scene_digest(&Registry::new(panels).expect("composes"), scratch).expect("digests")
    };
    let one = digest(&ONE, &mut scratch);
    assert_ne!(one, digest(&RENAMED, &mut scratch), "panel id is bound");
    assert_ne!(one, digest(&TWO, &mut scratch), "composition is bound");
}

#[test]
fn the_digest_binds_the_descriptor_contract() {
    static BASE: [PanelDescriptor; 1] = [panel("alpha")];
    static WIDER_MASK: [PanelDescriptor; 1] = [{
        let mut p = panel("alpha");
        p.required_layers = 0b110;
        p
    }];
    static WIDER_ASPECT: [PanelDescriptor; 1] = [{
        let mut p = panel("alpha");
        p.aspect_max = 1.5;
        p
    }];
    let mut scratch = std::vec![0u8; MAX_SCENE_BYTES];
    let digest = |panels: &'static [PanelDescriptor], scratch: &mut [u8]| {
        scene_digest(&Registry::new(panels).expect("composes"), scratch).expect("digests")
    };
    let base = digest(&BASE, &mut scratch);
    assert_ne!(
        base,
        digest(&WIDER_MASK, &mut scratch),
        "a weaker or stronger completeness gate is a different contract"
    );
    assert_ne!(
        base,
        digest(&WIDER_ASPECT, &mut scratch),
        "the frame constraints a shell may pick from are part of the contract"
    );
}

const BIG: DesignFrame = DesignFrame {
    width: 960.0,
    height: 720.0,
};

/// A panel spanning two canonical frames, one exactly twice the other.
const fn two_frame_panel() -> PanelDescriptor {
    let mut p = panel("alpha");
    p.frame_max = BIG;
    p.frame_step = (480.0, 360.0);
    p.canonical_frames = &[FRAME, BIG];
    p
}

fn draw_frame_rect(
    _data: &PanelData,
    _config: &ConfigBlob<'_>,
    _alerts: Option<&AlertOutput>,
    frame: DesignFrame,
    scene: &mut SceneWriter<'_>,
) -> Result<(), PanelDrawError> {
    scene.begin_layer(indicate_instrument_scene::LayerId::Attitude)?;
    scene.rect(
        indicate_instrument_scene::PaintMode::Fill,
        0.0,
        0.0,
        frame.width,
        frame.height,
    )?;
    scene.end_layer(indicate_instrument_scene::LayerId::Attitude)?;
    Ok(())
}

/// The same drawing with the smallest frame's size baked in: identical
/// bytes to [`draw_frame_rect`] at the floor, and stale everywhere else.
fn draw_fixed_rect(
    _data: &PanelData,
    _config: &ConfigBlob<'_>,
    _alerts: Option<&AlertOutput>,
    _frame: DesignFrame,
    scene: &mut SceneWriter<'_>,
) -> Result<(), PanelDrawError> {
    scene.begin_layer(indicate_instrument_scene::LayerId::Attitude)?;
    scene.rect(
        indicate_instrument_scene::PaintMode::Fill,
        0.0,
        0.0,
        FRAME.width,
        FRAME.height,
    )?;
    scene.end_layer(indicate_instrument_scene::LayerId::Attitude)?;
    Ok(())
}

/// The frame is an emission input, and the digest takes it at every
/// canonical size: two panels whose contract blocks are identical and
/// whose drawings agree at the floor still differ once a second
/// canonical frame is pinned — but only if the frame reaches the draw
/// and the draw is repeated there.
#[test]
fn the_digest_draws_every_canonical_frame_and_the_panel_sees_it() {
    static ONE_SIZED: [PanelDescriptor; 1] = [{
        let mut p = panel("alpha");
        p.draw = draw_frame_rect;
        p
    }];
    static ONE_FIXED: [PanelDescriptor; 1] = [{
        let mut p = panel("alpha");
        p.draw = draw_fixed_rect;
        p
    }];
    static TWO_SIZED: [PanelDescriptor; 1] = [{
        let mut p = two_frame_panel();
        p.draw = draw_frame_rect;
        p
    }];
    static TWO_FIXED: [PanelDescriptor; 1] = [{
        let mut p = two_frame_panel();
        p.draw = draw_fixed_rect;
        p
    }];
    let mut scratch = std::vec![0u8; MAX_SCENE_BYTES];
    let digest = |panels: &'static [PanelDescriptor], scratch: &mut [u8]| {
        scene_digest(&Registry::new(panels).expect("composes"), scratch).expect("digests")
    };
    assert_eq!(
        digest(&ONE_SIZED, &mut scratch),
        digest(&ONE_FIXED, &mut scratch),
        "at the floor alone the two drawings are byte-identical"
    );
    assert_ne!(
        digest(&TWO_SIZED, &mut scratch),
        digest(&TWO_FIXED, &mut scratch),
        "the second canonical frame is drawn, and the panel lays out against it"
    );
}

fn draw_one_layer(
    _data: &PanelData,
    _config: &ConfigBlob<'_>,
    _alerts: Option<&AlertOutput>,
    _frame: DesignFrame,
    scene: &mut SceneWriter<'_>,
) -> Result<(), PanelDrawError> {
    scene.begin_layer(indicate_instrument_scene::LayerId::Attitude)?;
    scene.end_layer(indicate_instrument_scene::LayerId::Attitude)?;
    Ok(())
}

#[test]
fn an_undersized_scratch_fails_typed_not_truncated() {
    static ONE: [PanelDescriptor; 1] = [{
        let mut p = panel("alpha");
        p.draw = draw_one_layer;
        p
    }];
    let registry = Registry::new(&ONE).expect("composes");
    // Too small for the writer's own header: refused before any draw.
    let mut none = [0u8; 0];
    assert!(matches!(
        scene_digest(&registry, &mut none),
        Err(DigestError::Scratch { len: 0 })
    ));
    // Big enough to open the writer, too small for the panel's layer:
    // the panel's own refusal, with the panel and state named.
    let mut tiny = [0u8; 2];
    assert!(matches!(
        scene_digest(&registry, &mut tiny),
        Err(DigestError::Draw { panel: "alpha", .. })
    ));
}
