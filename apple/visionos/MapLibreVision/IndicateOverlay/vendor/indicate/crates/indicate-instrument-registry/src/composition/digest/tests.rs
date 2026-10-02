#![allow(clippy::expect_used, clippy::panic)]

use indicate_instrument_descriptor::DesignFrame;
use indicate_instrument_scene::MAX_SCENE_BYTES;

use std::vec;

use super::composition_digest;
use crate::composition::tests::{SCREEN, SIDE_BY_SIDE, rect, registry, slot};
use crate::composition::{CompositionDescriptor, Slot};

#[test]
fn the_digest_covers_the_screen_the_rects_and_the_occlusions() {
    let registry = registry();
    let mut scratch = vec![0u8; MAX_SCENE_BYTES];
    let base = composition_digest(&registry, &SIDE_BY_SIDE, &mut scratch).expect("digests");
    assert_eq!(
        base,
        composition_digest(&registry, &SIDE_BY_SIDE, &mut scratch).expect("digests"),
        "the digest is a function of the declaration alone"
    );

    const MOVED: CompositionDescriptor = CompositionDescriptor {
        screen: SCREEN,
        slots: &[
            slot("low", rect(0.0, 0.0, 480.0, 360.0)),
            slot("high", rect(480.0, 360.0, 480.0, 360.0)),
        ],
    };
    assert_ne!(
        base,
        composition_digest(&registry, &MOVED, &mut scratch).expect("digests"),
        "a moved slot is a different screen"
    );

    const WIDER: CompositionDescriptor = CompositionDescriptor {
        screen: DesignFrame {
            width: 1200.0,
            height: 720.0,
        },
        slots: SIDE_BY_SIDE.slots,
    };
    assert_ne!(
        base,
        composition_digest(&registry, &WIDER, &mut scratch).expect("digests"),
        "a different screen is a different composition"
    );

    const DECLARED: CompositionDescriptor = CompositionDescriptor {
        screen: SCREEN,
        slots: &[
            slot("low", rect(0.0, 0.0, 480.0, 360.0)),
            Slot {
                panel: "high",
                rect: rect(480.0, 0.0, 480.0, 360.0),
                occludes: &["low"],
            },
        ],
    };
    assert_ne!(
        base,
        composition_digest(&registry, &DECLARED, &mut scratch).expect("digests"),
        "an obscuration declaration is part of the contract"
    );

    const REORDERED: CompositionDescriptor = CompositionDescriptor {
        screen: SCREEN,
        slots: &[
            slot("high", rect(480.0, 0.0, 480.0, 360.0)),
            slot("low", rect(0.0, 0.0, 480.0, 360.0)),
        ],
    };
    assert_ne!(
        base,
        composition_digest(&registry, &REORDERED, &mut scratch).expect("digests"),
        "slot index is z, so order is contract"
    );
}
